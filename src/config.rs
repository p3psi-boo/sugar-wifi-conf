use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

pub const NAME_DEFAULT: &str = "raspberrypi";
pub const KEY_DEFAULT: &str = "pisugar";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub name: String,
    pub key: String,
    pub custom_config_path: Option<PathBuf>,
    pub custom_config: Option<CustomConfigFile>,
}

impl AppConfig {
    pub fn from_args(
        custom_config_path: Option<PathBuf>,
        name: String,
        key: String,
        custom_config: Option<CustomConfigFile>,
    ) -> Self {
        Self {
            name,
            key,
            custom_config_path,
            custom_config,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: NAME_DEFAULT.to_string(),
            key: KEY_DEFAULT.to_string(),
            custom_config_path: None,
            custom_config: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct CustomConfigFile {
    // Node implementation accepted either `info` or `items`.
    #[serde(default, alias = "items")]
    pub info: Vec<CustomInfoItem>,

    #[serde(default)]
    pub commands: Vec<CustomCommandItem>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CustomInfoItem {
    pub label: String,
    pub command: String,

    #[serde(default)]
    pub interval: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CustomCommandItem {
    pub label: String,
    pub command: String,

    // Optional: some configurations may carry an explicit uuid/suffix.
    // Node code historically referenced `item.uuid`.
    #[serde(default)]
    pub uuid: Option<String>,
}

pub fn load_custom_config(path: &Path) -> anyhow::Result<CustomConfigFile> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read custom config at {}", path.display()))?;
    let cfg: CustomConfigFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse custom config at {}", path.display()))?;
    Ok(cfg)
}
