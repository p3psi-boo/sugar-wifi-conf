use std::time::Duration;

use thiserror::Error;

use crate::config::CustomCommandItem;
use crate::exec::{self, ExecOptions};
use crate::proto::CONCAT_TAG;

pub const DEFAULT_CUSTOM_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
pub const DEFAULT_CUSTOM_COMMAND_MAX_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommandRequest {
    pub key: String,
    /// 4 hex digits, uppercase, 1-based index (0001 == first command).
    pub suffix: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CustomCommandParseError {
    #[error("invalid utf-8")]
    InvalidUtf8,

    #[error("invalid syntax")]
    InvalidSyntax,

    #[error("invalid uuid suffix")]
    InvalidSuffix,
}

/// Parse a framed request payload: `key%&%XXXX`.
///
/// `XXXX` is typically a 4-hex suffix; for compatibility we also accept:
/// - a short UUID like `FD2BCCCC0001` (last 4 hex digits are used)
/// - a dashed UUID (last segment is used, then last 4 hex digits)
pub fn parse_custom_command_request(payload: &[u8]) -> Result<CustomCommandRequest, CustomCommandParseError> {
    let s = std::str::from_utf8(payload).map_err(|_| CustomCommandParseError::InvalidUtf8)?;

    // Node implementation accepts >= 2 fields and ignores extras.
    let mut parts = s.splitn(3, CONCAT_TAG);
    let Some(key) = parts.next() else {
        return Err(CustomCommandParseError::InvalidSyntax);
    };
    let Some(raw) = parts.next() else {
        return Err(CustomCommandParseError::InvalidSyntax);
    };
    if key.is_empty() || raw.is_empty() {
        return Err(CustomCommandParseError::InvalidSyntax);
    }

    let suffix = normalize_suffix(raw).ok_or(CustomCommandParseError::InvalidSuffix)?;
    Ok(CustomCommandRequest {
        key: key.to_string(),
        suffix,
    })
}

fn normalize_suffix(raw: &str) -> Option<String> {
    // Match Node behavior: if a full UUID is provided, the last segment is used.
    let tail = raw.split('-').last().unwrap_or(raw);
    let tail = tail.trim();
    if tail.is_empty() {
        return None;
    }

    // Allow either exact 4 hex digits, or something longer (like FD2BCCCC0001).
    let bytes = tail.as_bytes();
    let sfx = if bytes.len() == 4 {
        bytes
    } else if bytes.len() > 4 {
        &bytes[bytes.len() - 4..]
    } else {
        return None;
    };

    if !sfx.iter().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let sfx = std::str::from_utf8(sfx).ok()?;
    Some(sfx.to_ascii_uppercase())
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CustomCommandError {
    #[error("Invalid syntax.")]
    InvalidSyntax,
    #[error("Invalid key.")]
    InvalidKey,
    #[error("Invalid UUID.")]
    InvalidUuid,
    #[error("Command not found.")]
    CommandNotFound,
    #[error("exec error: {0}")]
    ExecError(String),
}

impl From<CustomCommandParseError> for CustomCommandError {
    fn from(e: CustomCommandParseError) -> Self {
        match e {
            CustomCommandParseError::InvalidUtf8 | CustomCommandParseError::InvalidSyntax => {
                CustomCommandError::InvalidSyntax
            }
            CustomCommandParseError::InvalidSuffix => CustomCommandError::InvalidUuid,
        }
    }
}

fn index0_from_suffix(suffix: &str) -> Option<usize> {
    // Protocol is 1-based: 0001 => index 0.
    let n = u16::from_str_radix(suffix, 16).ok()?;
    if n == 0 {
        return None;
    }
    Some((n - 1) as usize)
}

pub fn command_for_suffix<'a>(commands: &'a [CustomCommandItem], suffix: &str) -> Option<&'a str> {
    for cmd in commands {
        if let Some(uuid) = cmd.uuid.as_ref() {
            if normalize_suffix(uuid).as_deref() == Some(suffix) {
                return Some(cmd.command.as_str());
            }
        }
    }

    let idx = index0_from_suffix(suffix)?;
    commands.get(idx).map(|c| c.command.as_str())
}

/// Execute a custom command.
///
/// - Validates `key`.
/// - Maps `suffix` -> configured command by index order (1-based suffix).
/// - Executes via `sh -c` for Node parity.
/// - Returns combined stdout + stderr (capped).
pub async fn run_custom_command(
    expected_key: &str,
    commands: &[CustomCommandItem],
    framed_payload: &[u8],
    opts: Option<ExecOptions>,
) -> Vec<u8> {
    let opts = opts.unwrap_or_else(|| ExecOptions {
        timeout: DEFAULT_CUSTOM_COMMAND_TIMEOUT,
        max_output_bytes: DEFAULT_CUSTOM_COMMAND_MAX_BYTES,
    });

    let req = match parse_custom_command_request(framed_payload) {
        Ok(r) => r,
        Err(e) => return CustomCommandError::from(e).to_string().into_bytes(),
    };

    if req.key != expected_key {
        return CustomCommandError::InvalidKey.to_string().into_bytes();
    }

    let Some(cmd) = command_for_suffix(commands, &req.suffix) else {
        return CustomCommandError::CommandNotFound.to_string().into_bytes();
    };

    // Node's `exec()` goes through the shell; do the same for compatibility.
    match exec::run("sh", ["-c", cmd], opts.clone()).await {
        Ok(out) => {
            let mut bytes = out.stdout;
            bytes.extend_from_slice(&out.stderr);
            bytes
        }
        Err(e) => format!("exec error: {e}").into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uuid;

    fn cmd(label: &str, command: &str) -> CustomCommandItem {
        CustomCommandItem {
            label: label.to_string(),
            command: command.to_string(),
            uuid: None,
        }
    }

    #[test]
    fn suffix_is_1_based_and_uppercase() {
        assert_eq!(uuid::suffix_for_index0(0), "0001");
        assert_eq!(uuid::suffix_for_index0(1), "0002");
        assert_eq!(uuid::suffix_for_index0(15), "0010");
    }

    #[test]
    fn parse_key_and_suffix() {
        let req = parse_custom_command_request(b"pisugar%&%000A").unwrap();
        assert_eq!(req.key, "pisugar");
        assert_eq!(req.suffix, "000A");
    }

    #[test]
    fn parse_accepts_long_uuid_and_dashed_uuid() {
        let req = parse_custom_command_request(b"k%&%FD2BCCCC0001").unwrap();
        assert_eq!(req.suffix, "0001");

        let req = parse_custom_command_request(b"k%&%fd2bcccc-0001").unwrap();
        assert_eq!(req.suffix, "0001");
    }

    #[test]
    fn mapping_is_1_based_index_order() {
        let commands = vec![cmd("a", "echo a"), cmd("b", "echo b")];

        assert_eq!(command_for_suffix(&commands, "0001"), Some("echo a"));
        assert_eq!(command_for_suffix(&commands, "0002"), Some("echo b"));
        assert_eq!(command_for_suffix(&commands, "0000"), None);
        assert_eq!(command_for_suffix(&commands, "0003"), None);
    }
}
