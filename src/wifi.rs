use std::path::Path;
use std::sync::OnceLock;

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use anyhow::{anyhow, Context};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::time::{sleep, Duration};

use crate::exec::{run, run_checked, ExecOptions};
use crate::proto;

pub const DEFAULT_WIFI_EXEC_TIMEOUT: Duration = Duration::from_secs(20);
pub const DEFAULT_WIFI_MAX_OUTPUT_BYTES: usize = 16 * 1024;
pub const DEFAULT_WIFI_INTERFACE: &str = "wlan0";

#[derive(Debug, Clone, Copy)]
pub enum WifiStepKind {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct WifiStep {
    pub kind: WifiStepKind,
    pub message: String,
}

impl WifiStep {
    fn info(message: impl Into<String>) -> Self {
        Self {
            kind: WifiStepKind::Info,
            message: message.into(),
        }
    }

    fn warn(message: impl Into<String>) -> Self {
        Self {
            kind: WifiStepKind::Warn,
            message: message.into(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            kind: WifiStepKind::Error,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WifiStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

const WPA_SUPPLICANT_CONF_PATH: &str = "/etc/wpa_supplicant/wpa_supplicant.conf";
const NETWORK_INTERFACES_PATH: &str = "/etc/network/interfaces";

fn wifi_exec_options() -> ExecOptions {
    ExecOptions {
        timeout: DEFAULT_WIFI_EXEC_TIMEOUT,
        max_output_bytes: DEFAULT_WIFI_MAX_OUTPUT_BYTES,
    }
}

static WIFI_IFACE: OnceLock<String> = OnceLock::new();

pub async fn detect_wifi_interface() -> String {
    if let Some(cached) = WIFI_IFACE.get() {
        return cached.clone();
    }
    let iface = detect_wifi_interface_inner().await;
    WIFI_IFACE.get_or_init(|| iface).clone()
}

async fn detect_wifi_interface_inner() -> String {
    let Ok(mut dir) = tokio::fs::read_dir("/sys/class/net").await else {
        return DEFAULT_WIFI_INTERFACE.to_string();
    };

    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "lo" {
            continue;
        }
        let wireless = entry.path().join("wireless");
        if tokio::fs::metadata(&wireless).await.is_ok() {
            return name;
        }
    }

    DEFAULT_WIFI_INTERFACE.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiRequest {
    pub key: String,
    pub ssid: String,
    pub password: String,
}

#[derive(Debug, Error)]
pub enum ParseWifiRequestError {
    #[error("payload is not valid UTF-8")]
    InvalidUtf8,

    #[error("invalid syntax: expected key%&%ssid%&%password")]
    InvalidSyntax,
}

/// Parse a complete (already reassembled) client payload.
///
/// Input must be the bytes *without* the `proto::END_TAG` terminator.
/// Delimiter is `proto::CONCAT_TAG` ("%&%").
pub fn parse_wifi_request(payload: &[u8]) -> Result<WifiRequest, ParseWifiRequestError> {
    let s = std::str::from_utf8(payload).map_err(|_| ParseWifiRequestError::InvalidUtf8)?;
    let parts: Vec<&str> = s.splitn(4, proto::CONCAT_TAG).collect();
    if parts.len() < 3 {
        return Err(ParseWifiRequestError::InvalidSyntax);
    }

    Ok(WifiRequest {
        key: parts[0].to_string(),
        ssid: parts[1].to_string(),
        password: parts[2].to_string(),
    })
}

/// Configure Wi-Fi and return status lines to stream via NOTIFY_MESSAGE.
///
/// This function does not emit BLE notifications itself; it only returns the
/// strings to send.
pub async fn configure_wifi(ssid: &str, password: &str) -> Vec<String> {
    let steps = configure_wifi_steps(ssid, password).await;
    steps.into_iter().map(|s| s.to_string()).collect()
}

pub async fn configure_wifi_steps(ssid: &str, password: &str) -> Vec<WifiStep> {
    let mut msgs = Vec::new();

    // Avoid leaking secrets over BLE.
    let ssid_display = if ssid.is_empty() { "<empty>" } else { ssid };

    msgs.push(WifiStep::info(format!(
        "Configuring WiFi for SSID: {ssid_display}"
    )));
    msgs.push(WifiStep::info("Detecting NetworkManager (nmcli)..."));

    let opts = wifi_exec_options();
    let iface = detect_wifi_interface().await;

    match wlan_managed_by_network_manager(&iface, opts.clone()).await {
        Ok(true) => {
    msgs.push(WifiStep::info(format!(
        "{iface} is managed by NetworkManager; using nmcli"
    )));
            if let Err(e) = configure_wifi_nm(&iface, ssid, password, opts.clone(), &mut msgs).await {
                msgs.push(WifiStep::warn(format!(
                    "NetworkManager configuration failed: {e}"
                )));
                msgs.push(WifiStep::info("Falling back to wpa_supplicant..."));
                configure_wifi_wpa_supplicant(&iface, ssid, password, opts, &mut msgs).await;
            }
        }
        Ok(false) => {
            msgs.push(WifiStep::info(format!(
                "{iface} is not managed by NetworkManager; using wpa_supplicant"
            )));
            configure_wifi_wpa_supplicant(&iface, ssid, password, opts, &mut msgs).await;
        }
        Err(e) => {
            msgs.push(WifiStep::warn(format!("Failed to query nmcli: {e}")));
            msgs.push(WifiStep::info(
                "Assuming no NetworkManager; using wpa_supplicant",
            ));
            configure_wifi_wpa_supplicant(&iface, ssid, password, opts, &mut msgs).await;
        }
    }

    msgs
}

// --- Node parity helpers ---

/// Node `INPUT` (deprecated) path: always uses wpa_supplicant config rewrite and returns a single
/// `setMessage(...)` string.
pub async fn set_wifi_wpa_node_style(ssid: &str, password: &str) -> String {
    let opts = wifi_exec_options();
    let iface = detect_wifi_interface().await;

    // Rewrite wpa_supplicant.conf (Node uses writeFileSync).
    if let Err(e) =
        update_wpa_supplicant_conf_node_style(Path::new(WPA_SUPPLICANT_CONF_PATH), ssid, password).await
    {
        return format!("{e}");
    }

    // If wlan0 isn't OK in /etc/network/interfaces, Node asks for reboot.
    match is_interface_ok(Path::new(NETWORK_INTERFACES_PATH), &iface) {
        Ok(true) => {}
        Ok(false) => return "OK. Please reboot.".to_string(),
        Err(e) => {
            // Node would throw; keep a readable error but continue attempting.
            return format!("{e}");
        }
    }

    let mut res_msg = String::new();
    let mut max_try_times: i32 = 10;

    if let Ok(method) = restart_wpa_supplicant(&iface, opts.clone()).await {
        res_msg = format!("wpa_supplicant restarted via {method}");
        return format!("{max_try_times} {res_msg}");
    }

    // Best-effort kill existing wpa_supplicant.
    let _ = run("killall", ["wpa_supplicant"], opts.clone()).await;
    while max_try_times > 0 {
        sleep(Duration::from_secs(2)).await;
        match run_checked(
            "wpa_supplicant",
            [
                "-B",
                "-i",
                iface.as_str(),
                "-c/etc/wpa_supplicant/wpa_supplicant.conf",
            ],
            opts.clone(),
        )
        .await
        {
            Ok(out) => {
                // Node uses execSync(...).toString(), which keeps trailing newlines.
                res_msg = String::from_utf8_lossy(&out.stdout).to_string();
                break;
            }
            Err(_e) => {
                res_msg = "Command failed.".to_string();
            }
        }
        max_try_times -= 1;
    }

    format!("{max_try_times} {res_msg}")
}

/// Node `checkWlan0Managed` heuristic.
pub async fn check_wlan0_managed_node_style() -> bool {
    let opts = wifi_exec_options();
    let iface = detect_wifi_interface().await;
    wlan_managed_by_network_manager(&iface, opts).await.unwrap_or(false)
}

/// Node `setWifiNm`: no notify message is produced.
pub async fn set_wifi_nm_node_style(ssid: &str, password: &str) {
    let opts = wifi_exec_options();
    let iface = detect_wifi_interface().await;

    // Node uses exec() with string commands; keep behavior similar but pass args.
    let _ = run(
        "nmcli",
        ["connection", "delete", "id", ssid],
        opts.clone(),
    )
    .await;

    let _ = run_checked(
        "nmcli",
        [
            "connection",
            "add",
            "type",
            "wifi",
            "ifname",
            iface.as_str(),
            "con-name",
            ssid,
            "ssid",
            ssid,
            "wifi-sec.key-mgmt",
            "wpa-psk",
            "wifi-sec.psk",
            password,
        ],
        opts.clone(),
    )
    .await;

    let _ = run_checked("nmcli", ["connection", "up", ssid], opts).await;
}

async fn wlan_managed_by_network_manager(
    iface: &str,
    opts: ExecOptions,
) -> Result<bool, crate::exec::ExecError> {
    // Match the Node heuristic: consider wlan0 managed if nmcli reports it as
    // connected or disconnected.
    let out = run_checked("nmcli", ["--wait", "10", "dev", "status"], opts).await?;
    let stdout = out.stdout_lossy();
    for line in stdout.lines() {
        if !line.contains(iface) {
            continue;
        }
        if line.contains("connected") || line.contains("disconnected") {
            return Ok(true);
        }
        return Ok(false);
    }
    Ok(false)
}

async fn configure_wifi_nm(
    iface: &str,
    ssid: &str,
    password: &str,
    opts: ExecOptions,
    msgs: &mut Vec<WifiStep>,
) -> Result<(), crate::exec::ExecError> {
    // Best-effort delete to avoid duplicates.
    // Ignore errors (connection may not exist).
    let _ = run(
        "nmcli",
        ["--wait", "10", "connection", "delete", "id", ssid],
        opts.clone(),
    )
    .await;

    msgs.push(WifiStep::info("Adding WiFi connection with nmcli..."));
    let _ = run_checked(
        "nmcli",
        [
            "--wait",
            "20",
            "connection",
            "add",
            "type",
            "wifi",
            "ifname",
            iface,
            "con-name",
            ssid,
            "ssid",
            ssid,
            "wifi-sec.key-mgmt",
            "wpa-psk",
            "wifi-sec.psk",
            password,
        ],
        opts.clone(),
    )
    .await?;

    msgs.push(WifiStep::info("Bringing WiFi connection up..."));
    let out = run_checked(
        "nmcli",
        ["--wait", "20", "connection", "up", ssid],
        opts,
    )
    .await?;

    let details = out.stdout_lossy();
    if !details.is_empty() {
        msgs.push(WifiStep::info(details));
    }
    msgs.push(WifiStep::info("WiFi configured via NetworkManager"));
    Ok(())
}

async fn configure_wifi_wpa_supplicant(
    iface: &str,
    ssid: &str,
    password: &str,
    opts: ExecOptions,
    msgs: &mut Vec<WifiStep>,
) {
    // TODO: Consider a more robust implementation:
    // - Use `wpa_cli` when available.
    // - Respect systemd services: `systemctl restart wpa_supplicant@wlan0`.
    // - Better parsing of wpa_supplicant.conf (preserve formatting and comments).

    msgs.push(WifiStep::info(format!(
        "Updating {WPA_SUPPLICANT_CONF_PATH}..."
    )));
    let conf_path = Path::new(WPA_SUPPLICANT_CONF_PATH);
    match update_wpa_supplicant_conf(conf_path, ssid, password).await {
        Ok(()) => msgs.push(WifiStep::info("wpa_supplicant.conf updated")),
        Err(e) => {
            msgs.push(WifiStep::error(format!(
                "Failed to update wpa_supplicant.conf: {e}"
            )));
            return;
        }
    }

    match is_interface_ok(Path::new(NETWORK_INTERFACES_PATH), iface) {
        Ok(true) => {}
        Ok(false) => {
            msgs.push(WifiStep::warn("OK. Please reboot."));
            return;
        }
        Err(e) => {
            msgs.push(WifiStep::warn(format!(
                "Warning: failed to check {iface} config: {e}"
            )));
            // Continue anyway.
        }
    }

    msgs.push(WifiStep::info("Restarting wpa_supplicant..."));
    match restart_wpa_supplicant(iface, opts.clone()).await {
        Ok(method) => {
            msgs.push(WifiStep::info(format!(
                "wpa_supplicant restarted via {method}"
            )));
            return;
        }
        Err(e) => {
            msgs.push(WifiStep::warn(format!("Failed to restart wpa_supplicant: {e}")));
            msgs.push(WifiStep::info(
                "Falling back to manual wpa_supplicant start...",
            ));
        }
    }

    let mut last_err: Option<String> = None;
    let mut attempts_left = 10u32;
    while attempts_left > 0 {
        msgs.push(WifiStep::info(format!(
            "Starting wpa_supplicant (attempt {}/10)...",
            11 - attempts_left
        )));
        sleep(Duration::from_secs(2)).await;

        match run_checked(
            "wpa_supplicant",
            [
                "-B",
                "-i",
                iface,
                "-c/etc/wpa_supplicant/wpa_supplicant.conf",
            ],
            opts.clone(),
        )
        .await
        {
            Ok(out) => {
                let details = out.stdout_lossy();
                if !details.is_empty() {
                    msgs.push(WifiStep::info(details));
                }
                msgs.push(WifiStep::info("wpa_supplicant started"));
                return;
            }
            Err(e) => {
                last_err = Some(e.to_string());
                attempts_left -= 1;
            }
        }
    }

    if let Some(e) = last_err {
        msgs.push(WifiStep::error(format!(
            "Failed to start wpa_supplicant: {e}"
        )));
    } else {
        msgs.push(WifiStep::error("Failed to start wpa_supplicant"));
    }
}

async fn restart_wpa_supplicant(iface: &str, opts: ExecOptions) -> anyhow::Result<&'static str> {
    if run_checked("wpa_cli", ["-i", iface, "reconfigure"], opts.clone())
        .await
        .is_ok()
    {
        return Ok("wpa_cli reconfigure");
    }

    let unit = format!("wpa_supplicant@{iface}");
    let err = match run_checked("systemctl", ["restart", unit.as_str()], opts).await {
        Ok(_) => return Ok("systemctl restart"),
        Err(e) => e.to_string(),
    };

    Err(anyhow!("wpa_supplicant restart failed: {err}"))
}

async fn write_atomic_preserve(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    let meta = tokio::fs::metadata(path).await.ok();
    let filename = path
        .file_name()
        .ok_or_else(|| anyhow!("missing filename for {}", path.display()))?;
    let tmp_name = format!(
        ".{}.tmp.{}",
        filename.to_string_lossy(),
        std::process::id()
    );
    let tmp_path = path.with_file_name(tmp_name);

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp_path)
        .await
        .with_context(|| format!("open temp file {}", tmp_path.display()))?;
    file.write_all(content)
        .await
        .with_context(|| format!("write temp file {}", tmp_path.display()))?;
    file.sync_all()
        .await
        .with_context(|| format!("sync temp file {}", tmp_path.display()))?;

    if let Some(meta) = meta {
        let mode = meta.mode() & 0o777;
        tokio::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode))
            .await
            .with_context(|| format!("set permissions on {}", tmp_path.display()))?;

        let uid = meta.uid();
        let gid = meta.gid();
        let c_path = std::ffi::CString::new(tmp_path.as_os_str().as_bytes())
            .context("tmp path contains NUL")?;
        let rc = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
        if rc != 0 {
            return Err(anyhow!("failed to chown {}", tmp_path.display()));
        }
    }

    tokio::fs::rename(&tmp_path, path)
        .await
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), path.display()))?;
    Ok(())
}

async fn update_wpa_supplicant_conf(path: &Path, ssid: &str, password: &str) -> anyhow::Result<()> {
    let original = tokio::fs::read_to_string(path).await?;
    let (prefix, blocks) = split_wpa_network_blocks(&original);

    let mut kept: Vec<String> = Vec::new();
    let mut max_priority: i32 = 0;
    for b in blocks {
        let (b_ssid, b_prio) = parse_wpa_network_block_meta(&b);
        if let Some(p) = b_prio {
            max_priority = std::cmp::max(max_priority, p);
        }
        if b_ssid.as_deref() == Some(ssid) {
            continue;
        }
        kept.push(b);
    }

    let ssid_escaped = wpa_escape(ssid);
    let psk_line = wpa_psk_line(ssid, password, wifi_exec_options()).await;
    let new_block = format!(
        "network={{\n\t\tssid=\"{}\"\n\t\tscan_ssid=1\n\t\t{}\n\t\tpriority={}\n\t}}",
        ssid_escaped,
        psk_line,
        max_priority + 1
    );
    kept.push(new_block);

    let prefix = prefix.replace("Country=", "country=");
    let content = format!("{}\n\t{}", prefix.trim_end(), kept.join("\n\t"));
    write_atomic_preserve(path, content.as_bytes()).await?;
    Ok(())
}

async fn update_wpa_supplicant_conf_node_style(
    path: &Path,
    ssid: &str,
    password: &str,
) -> anyhow::Result<()> {
    // Mirrors Node `setWifiWpa` formatting/logic closely.
    let mut data = tokio::fs::read_to_string(path).await?;
    let (prefix, blocks) = split_wpa_network_blocks(&data);

    let mut wifi_array: Vec<String> = Vec::new();
    let mut max_priority: i32 = 0;
    for b in blocks {
        let (b_ssid, b_prio) = parse_wpa_network_block_meta(&b);
        if let Some(p) = b_prio {
            max_priority = std::cmp::max(max_priority, p);
        }
        if b_ssid.as_deref() != Some(ssid) {
            wifi_array.push(b);
        }
        // Node removes each matched block from `data` using replace; our split already did.
    }

    data = prefix.replace("Country=", "country=");
    let ssid_escaped = wpa_escape(ssid);
    let psk_line = wpa_psk_line(ssid, password, wifi_exec_options()).await;
    wifi_array.push(format!(
        "network={{\n\t\tssid=\"{}\"\n\t\tscan_ssid=1\n\t\t{}\n\t\tpriority={}\n\t}}",
        ssid_escaped,
        psk_line,
        max_priority + 1
    ));

    let content = format!("{}\n\t{}", data, wifi_array.join("\n\t"));
    write_atomic_preserve(path, content.as_bytes()).await?;
    Ok(())
}

fn wpa_escape(s: &str) -> String {
    // Minimal escaping for quoted values.
    // This matches wpa_supplicant's "\"" and "\\" expectations.
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

async fn wpa_psk_line(ssid: &str, password: &str, opts: ExecOptions) -> String {
    if let Ok(out) = run_checked("wpa_passphrase", [ssid, password], opts).await {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            let line = line.trim();
            if line.starts_with("psk=") && !line.starts_with('#') {
                let psk = line.trim_start_matches("psk=").trim();
                if !psk.is_empty() {
                    return format!("psk={psk}");
                }
            }
        }
    }

    let psk_escaped = wpa_escape(password);
    format!("psk=\"{psk_escaped}\"")
}

fn split_wpa_network_blocks(contents: &str) -> (String, Vec<String>) {
    let mut prefix = String::new();
    let mut blocks: Vec<String> = Vec::new();

    let mut i = 0usize;
    let needle = b"network={";
    let bytes = contents.as_bytes();

    while i < bytes.len() {
        let rel = bytes[i..]
            .windows(needle.len())
            .position(|w| w == needle);
        let Some(rel) = rel else {
            prefix.push_str(&String::from_utf8_lossy(&bytes[i..]));
            break;
        };
        let start = i + rel;
        prefix.push_str(&String::from_utf8_lossy(&bytes[i..start]));

        let mut j = start + needle.len();
        let mut depth = 1usize;
        let mut in_quote = false;
        let mut escape = false;

        while j < bytes.len() {
            let b = bytes[j];
            if in_quote {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    in_quote = false;
                }
                j += 1;
                continue;
            }

            if b == b'"' {
                in_quote = true;
                j += 1;
                continue;
            }
            if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    j += 1;
                    break;
                }
            }
            j += 1;
        }

        if depth != 0 || j > bytes.len() {
            prefix.push_str(&String::from_utf8_lossy(&bytes[start..]));
            break;
        }

        blocks.push(String::from_utf8_lossy(&bytes[start..j]).to_string());
        i = j;
    }

    (prefix, blocks)
}

fn parse_wpa_network_block_meta(block: &str) -> (Option<String>, Option<i32>) {
    let ssid = parse_quoted_value(block, "ssid=");
    let prio = parse_int_value(block, "priority=");
    (ssid, prio)
}

fn parse_quoted_value(s: &str, key: &str) -> Option<String> {
    let idx = s.find(key)?;
    let rest = &s[idx + key.len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('"')?;

    let mut out = String::new();
    let mut escape = false;
    for ch in rest.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        match ch {
            '\\' => escape = true,
            '"' => return Some(out),
            _ => out.push(ch),
        }
    }
    None
}

fn parse_int_value(s: &str, key: &str) -> Option<i32> {
    let idx = s.find(key)?;
    let rest = &s[idx + key.len()..];
    let rest = rest.trim_start();
    let mut digits = String::new();
    for ch in rest.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    digits.parse::<i32>().ok()
}

fn is_interface_ok(path: &Path, iface: &str) -> anyhow::Result<bool> {
    let data = std::fs::read_to_string(path)?;
    let mut found_iface = false;
    let mut is_ok = true;
    for raw_line in data.lines() {
        let line = raw_line.trim();
        if found_iface && line.starts_with("iface ") && !line.starts_with('#') {
            found_iface = false;
        }
        if line.starts_with("iface ") && !line.starts_with('#') {
            let mut parts = line.split_whitespace();
            let _iface_kw = parts.next();
            if let Some(name) = parts.next() {
                if name == iface {
                    found_iface = true;
                }
            }
        }
        if found_iface && line.contains("nohook wpa_supplicant") && !line.starts_with('#') {
            is_ok = false;
        }
    }
    Ok(is_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_payload() {
        let req = parse_wifi_request(b"pisugar%&%MyWiFi%&%pass").unwrap();
        assert_eq!(
            req,
            WifiRequest {
                key: "pisugar".to_string(),
                ssid: "MyWiFi".to_string(),
                password: "pass".to_string(),
            }
        );
    }

    #[test]
    fn parse_rejects_too_few_fields() {
        let err = parse_wifi_request(b"pisugar%&%onlyssid").unwrap_err();
        assert!(matches!(err, ParseWifiRequestError::InvalidSyntax));
    }

    #[test]
    fn parse_rejects_invalid_utf8() {
        let payload = [0xffu8, 0xfeu8, 0xfdu8];
        let err = parse_wifi_request(&payload).unwrap_err();
        assert!(matches!(err, ParseWifiRequestError::InvalidUtf8));
    }

    #[test]
    fn parse_accepts_extra_fields_for_compat() {
        let req = parse_wifi_request(b"k%&%s%&%p%&%ignored").unwrap();
        assert_eq!(req.key, "k");
        assert_eq!(req.ssid, "s");
        assert_eq!(req.password, "p");
    }
}
