use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use directories::BaseDirs;
use eyre::{ContextCompat, Result, WrapErr};
use serde::{Deserialize, Serialize};

use crate::constants::APP_DIR_NAME;

const DEFAULT_CONSENSUS_RPC: &str = "https://ethereum.operationsolarstorm.org";
fn default_consensus_rpcs() -> Vec<String> {
    vec![DEFAULT_CONSENSUS_RPC.to_string()]
}
fn default_execution_rpcs() -> Vec<String> {
    vec!["https://eth.drpc.org".to_string()]
}

fn default_show_tray_gas_price() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_consensus_rpcs")]
    pub consensus_rpcs: Vec<String>,
    #[serde(default = "default_execution_rpcs")]
    pub execution_rpcs: Vec<String>,
    #[serde(default = "default_following_interval")]
    pub following_check_interval_mins: u64,
    #[serde(default = "default_show_tray_gas_price")]
    pub show_tray_gas_price: bool,
    #[serde(default)]
    pub start_on_login: bool,
    #[serde(default)]
    pub dns_setup_attempted: bool,
    #[serde(default)]
    pub dns_setup_installed: bool,
}

fn default_following_interval() -> u64 {
    30
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
    let home = data_home_dir()?;
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
        consensus_rpcs: default_consensus_rpcs(),
        execution_rpcs: default_execution_rpcs(),
        following_check_interval_mins: default_following_interval(),
        show_tray_gas_price: default_show_tray_gas_price(),
        start_on_login: false,
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

fn data_home_dir() -> Result<PathBuf> {
    if let Ok(user) = std::env::var("SUDO_USER") {
        if !user.is_empty() && user != "root" {
            if let Ok(home) = home_dir_for_user(&user) {
                return Ok(home);
            }
        }
    }

    let base = BaseDirs::new().wrap_err("Failed to resolve base directories")?;
    Ok(base.home_dir().to_path_buf())
}

fn home_dir_for_user(user: &str) -> Result<PathBuf> {
    let output = match std::env::consts::OS {
        "macos" => Command::new("dscl")
            .arg(".")
            .arg("-read")
            .arg(format!("/Users/{user}"))
            .arg("NFSHomeDirectory")
            .output(),
        "linux" => Command::new("getent").arg("passwd").arg(user).output(),
        other => {
            return Err(eyre::eyre!(
                "Unsupported OS for sudo user home resolution: {other}"
            ));
        }
    }
    .wrap_err("Failed to resolve sudo user home directory")?;

    if !output.status.success() {
        return Err(eyre::eyre!("Failed to resolve sudo user home directory"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let home = if std::env::consts::OS == "macos" {
        stdout
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| eyre::eyre!("Failed to parse sudo user home directory"))?
            .to_string()
    } else {
        stdout
            .trim()
            .split(':')
            .nth(5)
            .ok_or_else(|| eyre::eyre!("Failed to parse sudo user home directory"))?
            .to_string()
    };

    Ok(PathBuf::from(home))
}
