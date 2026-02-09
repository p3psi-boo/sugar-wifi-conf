# sugar-wifi-conf-rs

Rust rewrite of the BLE WiFi configuration service.

Constraints:
- Client unchanged: UUIDs + protocol must stay compatible.
- Target: Linux armv7 + arm64.

## Usage

```text
sugar-wifi-conf-rs --name <BLE_NAME> --key <SHARED_KEY> --custom-config <PATH> [--dry-run]
```

Flags:
- `--name` BLE advertised name (default: `raspberrypi`)
- `--key` shared key used by client requests (default: `pisugar`)
- `--custom-config` path to `custom_config.json`
- `--dry-run` validate config and exit

Env var equivalents:
- `RUST_LOG` controls log level (e.g. `info`, `debug`)
- `SUGAR_WIFI_CONF_RS_CUSTOM_CONFIG` is used by the systemd unit to populate `--custom-config`
- `SUGAR_WIFI_CONF_RS_BIN` is used by the systemd unit to locate the binary

## Runtime dependencies

- BlueZ with DBus enabled (system `bluetoothd` service running)
- DBus permissions to register GATT/advertisements and query adapter state
- WiFi backend: NetworkManager or wpa_supplicant available (depending on the command path)

## Dev

Dev:

## Dev environment (Nix)

From the repo root:

```bash
nix develop
```

Then build/run from this crate:

```bash
cargo build --manifest-path sugar-wifi-conf-rs/Cargo.toml
RUST_LOG=info sudo ./sugar-wifi-conf-rs/target/debug/sugar-wifi-conf-rs --key pisugar --custom-config ./custom_config.json
```

From the crate directory:

```bash
cd sugar-wifi-conf-rs
cargo build
RUST_LOG=info sudo ./target/debug/sugar-wifi-conf-rs --key pisugar --custom-config ../custom_config.json
```

## systemd

A template unit is provided in `sugar-wifi-conf-rs/systemd/sugar-wifi-conf-rs@.service`.
The instance name (`@<instance>`) is used as the shared key (mapped to `--key`).

Hardening recommendations:
- Prefer a dedicated service user with only the required capabilities
- Use `NoNewPrivileges=yes`, `PrivateTmp=yes`, `ProtectSystem=strict`, `ProtectHome=yes`
- Limit privileges with `CapabilityBoundingSet=` and `AmbientCapabilities=`
- Only use `sudo` for install, daemon-reload, and editing unit drop-ins

Install + start (example uses key `pisugar`):

```bash
sudo install -m 0644 sugar-wifi-conf-rs/systemd/sugar-wifi-conf-rs@.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sugar-wifi-conf-rs@pisugar.service
```

Override the binary/config path (recommended) via a drop-in:

```bash
sudo systemctl edit sugar-wifi-conf-rs@pisugar.service
```

```ini
[Service]
Environment="SUGAR_WIFI_CONF_RS_BIN=/usr/local/bin/sugar-wifi-conf-rs"
Environment="SUGAR_WIFI_CONF_RS_CUSTOM_CONFIG=/opt/sugar-wifi-config/custom_config.json"
```

Logs:

```bash
sudo journalctl -u sugar-wifi-conf-rs@pisugar.service -f
```
