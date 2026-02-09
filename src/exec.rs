use std::ffi::{OsStr, OsString};
use std::fmt;
use std::process::ExitStatus;
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct ExecOptions {
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

impl ExecOutput {
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    program: String,
    args: Vec<OsString>,
}

impl CommandSpec {
    fn new(program: &str, args: Vec<OsString>) -> Self {
        Self {
            program: program.to_string(),
            args,
        }
    }
}

impl fmt::Display for CommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.program)?;
        for arg in &self.args {
            write!(f, " {}", arg.to_string_lossy())?;
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("failed to spawn {command}: {source}")]
    Spawn {
        command: CommandSpec,
        #[source]
        source: std::io::Error,
    },

    #[error("timeout after {timeout:?} running {command}")]
    Timeout { command: CommandSpec, timeout: Duration },

    #[error("failed to wait for {command}: {source}")]
    Wait {
        command: CommandSpec,
        #[source]
        source: std::io::Error,
    },

    #[error("failed reading process output for {command}: {source}")]
    OutputRead {
        command: CommandSpec,
        #[source]
        source: std::io::Error,
    },

    #[error("failed reading {stream} for {command}: {source}")]
    ReadTaskJoin {
        command: CommandSpec,
        stream: &'static str,
        #[source]
        source: tokio::task::JoinError,
    },

    #[error(
        "{command} exited with status {status:?} (stdout: {stdout_preview:?}, stderr: {stderr_preview:?})"
    )]
    NonZeroExit {
        command: CommandSpec,
        status: Option<i32>,
        stdout_preview: String,
        stderr_preview: String,
    },
}

fn preview_lossy(bytes: &[u8], max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }

    let truncated = bytes.len() > max_len;
    let slice = if truncated { &bytes[..max_len] } else { bytes };
    let mut s = String::from_utf8_lossy(slice).trim().to_string();
    if truncated {
        s.push_str("...");
    }
    s
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(mut r: R, cap: usize) -> Result<(Vec<u8>, bool), std::io::Error> {
    let mut buf = Vec::new();
    buf.reserve(std::cmp::min(cap, 8 * 1024));

    let mut truncated = false;
    let mut tmp = [0u8; 4096];
    loop {
        let n = r.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        if buf.len() < cap {
            let remaining = cap - buf.len();
            let take = std::cmp::min(remaining, n);
            buf.extend_from_slice(&tmp[..take]);
            if take < n {
                truncated = true;
            }
        } else {
            // Keep draining to avoid blocking the child if it keeps writing.
            truncated = true;
        }
    }
    Ok((buf, truncated))
}

async fn join_read_task(
    task: tokio::task::JoinHandle<Result<(Vec<u8>, bool), std::io::Error>>,
    command: &CommandSpec,
    stream: &'static str,
) -> Result<(Vec<u8>, bool), ExecError> {
    task.await
        .map_err(|source| ExecError::ReadTaskJoin {
            command: command.clone(),
            stream,
            source,
        })?
        .map_err(|source| ExecError::OutputRead {
            command: command.clone(),
            source,
        })
}

/// Run a command without invoking a shell.
///
/// This is safe against shell injection because `program` and `args` are passed
/// directly to execve.
pub async fn run<I, S>(program: &str, args: I, opts: ExecOptions) -> Result<ExecOutput, ExecError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args.into_iter().map(|s| s.as_ref().to_os_string()).collect();
    let command = CommandSpec::new(program, args);

    let mut cmd = Command::new(&command.program);
    cmd.args(&command.args);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|source| ExecError::Spawn {
        command: command.clone(),
        source,
    })?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let cap = opts.max_output_bytes;
    let stdout_task = tokio::spawn(async move { read_capped(stdout, cap).await });
    let stderr_task = tokio::spawn(async move { read_capped(stderr, cap).await });

    let timeout = time::sleep(opts.timeout);
    tokio::pin!(timeout);

    let status = tokio::select! {
        _ = &mut timeout => {
            // Best-effort termination.
            stdout_task.abort();
            stderr_task.abort();
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(ExecError::Timeout { command: command.clone(), timeout: opts.timeout });
        }
        status = child.wait() => {
            status.map_err(|source| ExecError::Wait {
                command: command.clone(),
                source,
            })?
        }
    };

    let (stdout, stdout_truncated) = join_read_task(stdout_task, &command, "stdout").await?;
    let (stderr, stderr_truncated) = join_read_task(stderr_task, &command, "stderr").await?;

    Ok(ExecOutput {
        status,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

pub async fn run_checked<I, S>(program: &str, args: I, opts: ExecOptions) -> Result<ExecOutput, ExecError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args.into_iter().map(|s| s.as_ref().to_os_string()).collect();
    let out = run(program, &args, opts).await?;
    if out.status.success() {
        return Ok(out);
    }

    Err(ExecError::NonZeroExit {
        command: CommandSpec::new(program, args),
        status: out.status.code(),
        stdout_preview: preview_lossy(&out.stdout, 512),
        stderr_preview: preview_lossy(&out.stderr, 512),
    })
}

/// Run a shell snippet via `sh -c`.
///
/// Node parity: custom-info and custom-command execute commands through a shell.
pub async fn run_sh(command: &str, opts: ExecOptions) -> Result<ExecOutput, ExecError> {
    run("sh", ["-c", command], opts).await
}

pub async fn run_sh_checked(command: &str, opts: ExecOptions) -> Result<ExecOutput, ExecError> {
    run_checked("sh", ["-c", command], opts).await
}
