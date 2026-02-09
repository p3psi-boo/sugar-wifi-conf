mod config;
#[cfg(feature = "ble")]
mod ble;

#[cfg(not(feature = "ble"))]
mod ble {
    use crate::config::AppConfig;

    pub async fn run(_cfg: AppConfig) -> anyhow::Result<()> {
        anyhow::bail!("ble feature disabled at compile time")
    }
}
mod exec;
mod proto;
mod status;
mod uuid;
mod wifi;
mod custom_info;
mod custom_command;

use std::path::PathBuf;

use clap::Parser;
use tracing::{info, warn};

#[derive(Debug, Parser)]
#[command(name = "sugar-wifi-conf-rs")]
#[command(about = "Rust BLE WiFi config service (compatible with PiSugar clients)")]
struct Args {
    /// BLE advertised name (default: raspberrypi)
    #[arg(long, default_value_t = config::NAME_DEFAULT.to_string())]
    name: String,

    /// Shared key used by client requests (default: pisugar)
    #[arg(long, default_value_t = config::KEY_DEFAULT.to_string())]
    key: String,

    /// Path to custom_config.json
    #[arg(long)]
    custom_config: Option<PathBuf>,

    /// Validate config and exit
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let custom_config = match &args.custom_config {
        Some(path) => match config::load_custom_config(path) {
            Ok(custom) => {
                info!(info_items = custom.info.len(), command_items = custom.commands.len(), "loaded custom_config.json");
                Some(custom)
            }
            Err(e) => {
                warn!(error = %e, "failed to load custom_config.json");
                return Err(e);
            }
        },
        None => None,
    };

    let cfg = config::AppConfig::from_args(args.custom_config, args.name, args.key, custom_config);

    info!(name = %cfg.name, "starting");
    info!(service_uuid = uuid::SERVICE_ID, "ble service uuid");
    info!(custom_config = ?cfg.custom_config_path, "custom config path");

    if args.dry_run {
        info!("dry-run ok");
        return Ok(());
    }

    // Minimal BLE plumbing: advertise + register a GATT application.
    // This intentionally only exposes a couple of readable characteristics for now.
    ble::run(cfg).await
}
