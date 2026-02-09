// UUIDs MUST match `sugar-uuid.js` (client compatibility).

pub const BASE_UUID_PREFIX: &str = "FD2B4448AA0F4A15A62FEB0BE77A";

macro_rules! base_uuid {
    ($suffix:literal) => {
        concat!("FD2B4448AA0F4A15A62FEB0BE77A", $suffix)
    };
}

pub const SERVICE_ID: &str = base_uuid!("0000");
pub const SERVICE_NAME: &str = base_uuid!("0001");
pub const DEVICE_MODEL: &str = base_uuid!("0002");
pub const WIFI_NAME: &str = base_uuid!("0003");
pub const IP_ADDRESS: &str = base_uuid!("0004");
pub const INPUT: &str = base_uuid!("0005");
pub const NOTIFY_MESSAGE: &str = base_uuid!("0006");
pub const INPUT_SEP: &str = base_uuid!("0007");
pub const CUSTOM_COMMAND_INPUT: &str = base_uuid!("0008");
pub const CUSTOM_COMMAND_NOTIFY: &str = base_uuid!("0009");

// Dynamic characteristic UUID prefixes.
pub const CUSTOM_INFO_LABEL_PREFIX: &str = "FD2BCCCA";
pub const CUSTOM_INFO_COUNT: &str = "FD2BCCAA0000";
pub const CUSTOM_INFO_PREFIX: &str = "FD2BCCCB";
pub const CUSTOM_COMMAND_LABEL_PREFIX: &str = "FD2BCCCC";
pub const CUSTOM_COMMAND_COUNT: &str = "FD2BCCAC0000";

/// Format an index (0-based) into the 4-hex suffix used by the PiSugar protocol.
pub fn suffix_for_index0(index0: usize) -> String {
    let n = (index0 as u32) + 1;
    format!("{n:04X}")
}

/// Convert raw 12/32-hex UUIDs into the dashed form BlueZ expects.
pub fn bluez_uuid(raw: &str) -> anyhow::Result<String> {
    let is_hex = |s: &str| s.as_bytes().iter().all(|b| b.is_ascii_hexdigit());
    match raw.len() {
        32 => {
            if !is_hex(raw) {
                anyhow::bail!("invalid 32-hex UUID: {raw}");
            }
            let (a, rest) = raw.split_at(8);
            let (b, rest) = rest.split_at(4);
            let (c, rest) = rest.split_at(4);
            let (d, e) = rest.split_at(4);
            Ok(format!("{a}-{b}-{c}-{d}-{e}"))
        }
        12 => {
            if !is_hex(raw) {
                anyhow::bail!("invalid 12-hex UUID: {raw}");
            }
            Ok(format!("00000000-0000-0000-0000-{raw}"))
        }
        _ => anyhow::bail!("unsupported UUID length: {raw}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_hex(s: &str) -> bool {
        s.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
    }

    #[test]
    fn uuids_are_32_hex_chars() {
        let ids = [
            SERVICE_ID,
            SERVICE_NAME,
            DEVICE_MODEL,
            WIFI_NAME,
            IP_ADDRESS,
            INPUT,
            NOTIFY_MESSAGE,
            INPUT_SEP,
            CUSTOM_COMMAND_INPUT,
            CUSTOM_COMMAND_NOTIFY,
        ];

        for id in ids {
            assert_eq!(id.len(), 32);
            assert!(is_hex(id));
        }
    }
}
