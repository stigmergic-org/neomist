use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use eyre::{ContextCompat, Result, WrapErr};
use serde::{Deserialize, Serialize};

use crate::constants::APP_DIR_NAME;

const DEFAULT_CONSENSUS_RPC: &str = "https://ethereum.operationsolarstorm.org";
const DEFAULT_EXECUTION_RPC: &str = "https://eth.drpc.org";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub consensus_rpc: String,
    pub execution_rpc: String,
    #[serde(default)]
    pub dns_setup_attempted: bool,
    #[serde(default)]
    pub dns_setup_installed: bool,
}

pub fn config_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().wrap_err("Failed to resolve base directories")?;
    let home = base.home_dir();
    let path = home.join(".config").join(APP_DIR_NAME);
    fs::create_dir_all(&path).wrap_err("Failed to create config directory")?;
    Ok(path)
}

pub fn config_path() -> Result<PathBuf> {
    let path = config_dir()?.join("config.json");
    ensure_parent_dir(&path)?;
    Ok(path)
}

pub fn data_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().wrap_err("Failed to resolve base directories")?;
    let home = base.home_dir();
    let dir = home.join(".local").join("share").join(APP_DIR_NAME);
    fs::create_dir_all(&dir).wrap_err("Failed to create data directory")?;
    Ok(dir)
}

pub fn cache_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().wrap_err("Failed to resolve base directories")?;
    let dir = base.cache_dir().join(APP_DIR_NAME);
    fs::create_dir_all(&dir).wrap_err("Failed to create cache directory")?;
    Ok(dir)
}

pub fn load_or_create_config(path: &Path) -> Result<AppConfig> {
    if path.exists() {
        let contents = fs::read_to_string(path).wrap_err("Failed to read config file")?;
        let config: AppConfig =
            serde_json::from_str(&contents).wrap_err("Failed to parse config file")?;
        Ok(config)
    } else {
        let config = default_config();
        save_config(path, &config)?;
        Ok(config)
    }
}

pub fn save_config(path: &Path, config: &AppConfig) -> Result<()> {
    ensure_parent_dir(path)?;
    let contents = serde_json::to_string_pretty(config).wrap_err("Failed to serialize config")?;
    fs::write(path, contents).wrap_err("Failed to write config file")?;
    Ok(())
}

fn default_config() -> AppConfig {
    AppConfig {
        consensus_rpc: DEFAULT_CONSENSUS_RPC.to_string(),
        execution_rpc: DEFAULT_EXECUTION_RPC.to_string(),
        dns_setup_attempted: false,
        dns_setup_installed: false,
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).wrap_err("Failed to create config directory")?;
    }
    Ok(())
}
