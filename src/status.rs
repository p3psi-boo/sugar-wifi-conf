use std::collections::HashMap;
use std::time::Duration;

use crate::exec::{run_checked, ExecOptions};
use crate::wifi;

// Node parity: wifi-name and ip-address are notify characteristics that refresh every 5 seconds.

pub async fn get_wifi_name_node_style() -> String {
    let opts = status_exec_options();
    let iface = wifi::detect_wifi_interface().await;

    if let Ok(out) = run_checked("nmcli", ["-t", "-f", "ACTIVE,SSID", "dev", "wifi"], opts.clone()).await {
        let s = String::from_utf8_lossy(&out.stdout);
        for line in s.lines() {
            let mut parts = line.splitn(2, ':');
            let active = parts.next().unwrap_or("");
            let ssid = parts.next().unwrap_or("");
            if active == "yes" && !ssid.is_empty() {
                return ssid.to_string();
            }
        }
    }

    let out = match run_checked("iw", ["dev", iface.as_str(), "link"], opts).await {
        Ok(o) => o,
        Err(_) => return "Not available".to_string(),
    };

    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("SSID:") {
            let ssid = rest.trim();
            if !ssid.is_empty() {
                return ssid.to_string();
            }
        }
    }
    "Not available".to_string()
}

pub async fn get_ip_address_node_style() -> String {
    let opts = status_exec_options();
    let iface = wifi::detect_wifi_interface().await;

    let out = match run_checked("ip", ["-j", "-4", "addr", "show"], opts).await {
        Ok(o) => o,
        Err(_) => return "--".to_string(),
    };

    let value: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(_) => return "--".to_string(),
    };

    let mut iface_ips: HashMap<String, Vec<String>> = HashMap::new();
    let Some(arr) = value.as_array() else {
        return "--".to_string();
    };

    for iface_obj in arr {
        let ifname = match iface_obj.get("ifname").and_then(|v| v.as_str()) {
            Some(name) if name != "lo" => name,
            _ => continue,
        };
        let Some(addr_info) = iface_obj.get("addr_info").and_then(|v| v.as_array()) else {
            continue;
        };
        for info in addr_info {
            if info.get("family").and_then(|v| v.as_str()) != Some("inet") {
                continue;
            }
            let Some(local) = info.get("local").and_then(|v| v.as_str()) else {
                continue;
            };
            if local == "127.0.0.1" || local.starts_with("169.254.") {
                continue;
            }
            iface_ips
                .entry(ifname.to_string())
                .or_default()
                .push(local.to_string());
        }
    }

    if iface_ips.is_empty() {
        return "--".to_string();
    }

    let mut ordered_ifaces = Vec::new();
    if iface_ips.contains_key(&iface) {
        ordered_ifaces.push(iface.clone());
    }
    if iface != "eth0" && iface_ips.contains_key("eth0") {
        ordered_ifaces.push("eth0".to_string());
    }
    let mut rest: Vec<String> = iface_ips
        .keys()
        .filter(|k| !ordered_ifaces.contains(k))
        .cloned()
        .collect();
    rest.sort();
    ordered_ifaces.extend(rest);

    let mut ips = Vec::new();
    for name in ordered_ifaces {
        if let Some(items) = iface_ips.get(&name) {
            ips.extend(items.iter().cloned());
        }
    }
    ips.join(", ")
}

fn status_exec_options() -> ExecOptions {
    ExecOptions {
        timeout: Duration::from_secs(3),
        max_output_bytes: 32 * 1024,
    }
}
