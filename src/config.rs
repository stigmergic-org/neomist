use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use directories::BaseDirs;
use eyre::{ContextCompat, Result, WrapErr};
use serde::{Deserialize, Serialize};

use crate::constants::APP_DIR_NAME;

const DEFAULT_CONSENSUS_RPC: &str = "https://ethereum.operationsolarstorm.org";
pub const NEOMIST_DATA_DIR_ENV: &str = "NEOMIST_DATA_DIR";
pub const NEOMIST_CONFIG_DIR_ENV: &str = "NEOMIST_CONFIG_DIR";
pub const NEOMIST_CACHE_DIR_ENV: &str = "NEOMIST_CACHE_DIR";
pub const NEOMIST_REAL_USER_ENV: &str = "NEOMIST_REAL_USER";

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
    #[serde(default = "default_helios_enabled")]
    pub helios_enabled: bool,
}

fn default_following_interval() -> u64 {
    30
}

fn default_helios_enabled() -> bool {
    true
}

pub fn config_dir_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(NEOMIST_CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(runtime_user_home_dir()?.join(".config").join(APP_DIR_NAME))
}

pub fn config_dir() -> Result<PathBuf> {
    let path = config_dir_path()?;
    fs::create_dir_all(&path).wrap_err("Failed to create config directory")?;
    Ok(path)
}

pub fn config_path() -> Result<PathBuf> {
    let path = config_dir()?.join("config.json");
    ensure_parent_dir(&path)?;
    Ok(path)
}

pub fn data_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(NEOMIST_DATA_DIR_ENV) {
        let dir = PathBuf::from(path);
        fs::create_dir_all(&dir).wrap_err("Failed to create data directory")?;
        return Ok(dir);
    }

    let home = data_home_dir()?;
    let dir = home.join(".local").join("share").join(APP_DIR_NAME);
    fs::create_dir_all(&dir).wrap_err("Failed to create data directory")?;
    Ok(dir)
}

pub fn cache_dir_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(NEOMIST_CACHE_DIR_ENV) {
        return Ok(PathBuf::from(path));
    }
    Ok(runtime_user_home_dir()?.join(".cache").join(APP_DIR_NAME))
}

pub fn cache_dir() -> Result<PathBuf> {
    let dir = cache_dir_path()?;
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
        helios_enabled: true,
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).wrap_err("Failed to create config directory")?;
    }
    Ok(())
}

fn data_home_dir() -> Result<PathBuf> {
    runtime_user_home_dir()
}

pub fn runtime_user_home_dir() -> Result<PathBuf> {
    if let Some(user) = preferred_real_user() {
        if let Ok(home) = home_dir_for_user(&user) {
            return Ok(home);
        }
    }

    let base = BaseDirs::new().wrap_err("Failed to resolve base directories")?;
    Ok(base.home_dir().to_path_buf())
}

fn preferred_real_user() -> Option<String> {
    std::env::var(NEOMIST_REAL_USER_ENV)
        .ok()
        .filter(|user| !user.is_empty() && user != "root")
        .or_else(|| {
            std::env::var("SUDO_USER")
                .ok()
                .filter(|user| !user.is_empty() && user != "root")
        })
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

#[cfg(test)]
mod tests {
    use super::{
        APP_DIR_NAME, NEOMIST_CACHE_DIR_ENV, NEOMIST_CONFIG_DIR_ENV, NEOMIST_REAL_USER_ENV,
        cache_dir_path, config_dir_path,
    };
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn config_dir_prefers_explicit_override() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var(NEOMIST_CONFIG_DIR_ENV, "/tmp/neomist-config-test");
            std::env::remove_var(NEOMIST_CACHE_DIR_ENV);
            std::env::remove_var(NEOMIST_REAL_USER_ENV);
            std::env::remove_var("SUDO_USER");
        }

        assert_eq!(
            config_dir_path().unwrap(),
            PathBuf::from("/tmp/neomist-config-test")
        );

        unsafe {
            std::env::remove_var(NEOMIST_CONFIG_DIR_ENV);
        }
    }

    #[test]
    fn cache_dir_uses_real_user_home_when_overridden_user_is_present() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(NEOMIST_CACHE_DIR_ENV);
            std::env::remove_var(NEOMIST_CONFIG_DIR_ENV);
            std::env::set_var(NEOMIST_REAL_USER_ENV, "root");
            std::env::set_var("SUDO_USER", "root");
            std::env::set_var("HOME", "/tmp/neomist-home-test");
        }

        assert_eq!(
            cache_dir_path().unwrap(),
            PathBuf::from("/tmp/neomist-home-test/.cache").join(APP_DIR_NAME)
        );

        unsafe {
            std::env::remove_var(NEOMIST_REAL_USER_ENV);
            std::env::remove_var("SUDO_USER");
            std::env::remove_var("HOME");
        }
    }
}
