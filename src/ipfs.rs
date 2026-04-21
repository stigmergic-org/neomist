use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use eyre::{Result, WrapErr};
use flate2::read::GzDecoder;
use hex::encode as hex_encode;
use sha2::{Digest, Sha512};
use tar::Archive;

const KUBO_VERSION: &str = "v0.40.1";
const IPFS_API_PORT: u16 = 5001;
const MANAGED_GATEWAY_PORT: u16 = 58080;
const KUBO_DIST_BASE: &str = "https://dist.ipfs.tech/kubo";

pub struct KuboManager {
    gateway_port: u16,
    ipfs_path: Option<PathBuf>,
    repo_dir: Option<PathBuf>,
    child: Arc<Mutex<Option<Child>>>,
    offline: Arc<AtomicBool>,
}

impl KuboManager {
    pub fn gateway_port(&self) -> u16 {
        self.gateway_port
    }

    pub fn is_managed(&self) -> bool {
        self.ipfs_path.is_some()
    }

    pub fn is_offline(&self) -> bool {
        self.offline.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    pub fn set_offline(&self, offline: bool) -> Result<bool> {
        if !self.is_managed() || offline == self.is_offline() {
            return Ok(false);
        }

        tracing::info!("Restarting IPFS daemon (offline={offline})");

        let ipfs_path = self
            .ipfs_path
            .clone()
            .ok_or_else(|| eyre::eyre!("Managed IPFS binary missing"))?;
        let repo_dir = self
            .repo_dir
            .clone()
            .ok_or_else(|| eyre::eyre!("Managed IPFS repo missing"))?;

        let mut guard = self
            .child
            .lock()
            .map_err(|_| eyre::eyre!("Failed to lock kubo process"))?;
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let child = start_kubo_daemon(&ipfs_path, &repo_dir, offline)?;
        *guard = Some(child);
        self.offline.store(offline, Ordering::SeqCst);
        Ok(true)
    }
}

pub fn bundled_kubo_version() -> &'static str {
    KUBO_VERSION
}

pub async fn init_kubo(http_client: reqwest::Client, base_dir: PathBuf) -> Result<KuboManager> {
    tracing::info!("Starting IPFS (kubo)");
    tracing::info!("Checking for existing IPFS API on port {IPFS_API_PORT}");
    if check_existing_ipfs(&http_client).await {
        let gateway_port = fetch_gateway_port(&http_client).await.unwrap_or(8080);
        tracing::info!("Using existing IPFS on port {IPFS_API_PORT} (gateway {gateway_port})");
        return Ok(KuboManager {
            gateway_port,
            ipfs_path: None,
            repo_dir: None,
            child: Arc::new(Mutex::new(None)),
            offline: Arc::new(AtomicBool::new(false)),
        });
    }

    let (bin_dir, repo_dir, download_dir) = kubo_paths(&base_dir);
    tracing::info!("Downloading Kubo {KUBO_VERSION}");
    let ipfs_path = download_kubo(&http_client, &bin_dir, &download_dir).await?;
    tracing::info!("Ensuring IPFS repo at {}", repo_dir.display());
    ensure_repo_initialized(&ipfs_path, &repo_dir)?;
    tracing::info!("Configuring IPFS gateway on port {MANAGED_GATEWAY_PORT}");
    update_gateway_config(&repo_dir, MANAGED_GATEWAY_PORT)?;

    tracing::info!("Starting IPFS daemon (offline=false)");
    let child = start_kubo_daemon(&ipfs_path, &repo_dir, false)?;
    tracing::info!("Waiting for IPFS API to become ready");
    wait_for_ipfs(&http_client).await?;
    tracing::info!("Managed IPFS started (gateway {MANAGED_GATEWAY_PORT})");

    Ok(KuboManager {
        gateway_port: MANAGED_GATEWAY_PORT,
        ipfs_path: Some(ipfs_path),
        repo_dir: Some(repo_dir),
        child: Arc::new(Mutex::new(Some(child))),
        offline: Arc::new(AtomicBool::new(false)),
    })
}

async fn check_existing_ipfs(http_client: &reqwest::Client) -> bool {
    let url = format!("http://127.0.0.1:{IPFS_API_PORT}/api/v0/version");
    http_client
        .post(url)
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

async fn fetch_gateway_port(http_client: &reqwest::Client) -> Result<u16> {
    let url = format!("http://127.0.0.1:{IPFS_API_PORT}/api/v0/config?arg=Addresses.Gateway");
    let response: serde_json::Value = http_client
        .post(url)
        .send()
        .await
        .wrap_err("Failed to call IPFS config")?
        .json()
        .await
        .wrap_err("Failed to decode IPFS config response")?;

    let gateway = response
        .get("Value")
        .and_then(|value| value.as_str())
        .unwrap_or("");

    if let Some(port) = gateway.split("/tcp/").nth(1) {
        if let Ok(port) = port.parse::<u16>() {
            return Ok(port);
        }
    }

    Ok(8080)
}

async fn wait_for_ipfs(http_client: &reqwest::Client) -> Result<()> {
    let mut attempts = 0;
    while attempts < 40 {
        if check_existing_ipfs(http_client).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        attempts += 1;
    }

    Err(eyre::eyre!("Timed out waiting for IPFS API"))
}

async fn download_kubo(
    http_client: &reqwest::Client,
    bin_dir: &Path,
    download_dir: &Path,
) -> Result<PathBuf> {
    let (os, arch) = kubo_platform_target()?;
    let filename = format!("kubo_{KUBO_VERSION}_{os}-{arch}.tar.gz");
    let checksum_name = format!("{filename}.sha512");

    fs::create_dir_all(bin_dir).wrap_err("Failed to create kubo bin dir")?;
    fs::create_dir_all(download_dir).wrap_err("Failed to create kubo download dir")?;

    let ipfs_path = bin_dir.join("ipfs");
    if ipfs_path.exists() {
        if ipfs_path.is_file() {
            match installed_kubo_version(&ipfs_path)? {
                Some(installed_version)
                    if normalized_kubo_version(&installed_version)
                        == normalized_kubo_version(KUBO_VERSION) =>
                {
                    tracing::info!("Using cached Kubo {installed_version}");
                    return Ok(ipfs_path);
                }
                Some(installed_version) => {
                    tracing::info!(
                        "Replacing cached Kubo {installed_version} with {KUBO_VERSION}"
                    );
                }
                None => {
                    tracing::info!(
                        "Replacing cached Kubo binary because installed version could not be determined"
                    );
                }
            }

            fs::remove_file(&ipfs_path).wrap_err("Failed to remove stale kubo binary")?;
        } else {
            fs::remove_dir_all(&ipfs_path).wrap_err("Failed to remove stale kubo directory")?;
        }
    }

    let checksum_url = format!("{KUBO_DIST_BASE}/{KUBO_VERSION}/{checksum_name}");
    let checksum_text = http_client
        .get(checksum_url)
        .send()
        .await
        .wrap_err("Failed to fetch kubo checksum")?
        .text()
        .await
        .wrap_err("Failed to read kubo checksum")?;

    let expected_hash = checksum_text
        .split_whitespace()
        .next()
        .ok_or_else(|| eyre::eyre!("Checksum not found for {filename}"))?
        .to_lowercase();

    let tarball_url = format!("{KUBO_DIST_BASE}/{KUBO_VERSION}/{filename}");
    let bytes = http_client
        .get(tarball_url)
        .send()
        .await
        .wrap_err("Failed to download kubo tarball")?
        .bytes()
        .await
        .wrap_err("Failed to read kubo tarball")?;

    let mut hasher = Sha512::new();
    hasher.update(&bytes);
    let actual_hash = hex_encode(hasher.finalize());
    if actual_hash != expected_hash {
        return Err(eyre::eyre!(
            "Kubo checksum mismatch: expected {expected_hash}, got {actual_hash}"
        ));
    }

    let tarball_path = download_dir.join(&filename);
    fs::write(&tarball_path, &bytes).wrap_err("Failed to write kubo tarball")?;

    let extract_dir = download_dir.join(format!("kubo_{KUBO_VERSION}_{os}-{arch}"));
    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir).wrap_err("Failed to clear kubo extract dir")?;
    }
    fs::create_dir_all(&extract_dir).wrap_err("Failed to create kubo extract dir")?;

    let tar = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(tar);
    archive
        .unpack(&extract_dir)
        .wrap_err("Failed to extract kubo archive")?;

    let extracted_binary = find_ipfs_binary(&extract_dir)?;
    fs::copy(&extracted_binary, &ipfs_path).wrap_err("Failed to install kubo binary")?;

    set_ipfs_permissions(&ipfs_path)?;
    Ok(ipfs_path)
}

fn set_ipfs_permissions(ipfs_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(ipfs_path)
            .wrap_err("Failed to stat kubo binary")?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(ipfs_path, perms).wrap_err("Failed to set kubo permissions")?;
    }

    #[cfg(target_os = "macos")]
    {
        if xattr::get(ipfs_path, "com.apple.quarantine")
            .wrap_err("Failed to read kubo quarantine attribute")?
            .is_some()
        {
            xattr::remove(ipfs_path, "com.apple.quarantine")
                .wrap_err("Failed to remove kubo quarantine attribute")?;
        }
    }

    Ok(())
}

fn installed_kubo_version(ipfs_path: &Path) -> Result<Option<String>> {
    let output = Command::new(ipfs_path)
        .arg("version")
        .arg("--number")
        .output()
        .wrap_err("Failed to run cached kubo version check")?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            "Cached kubo version check failed (status: {}). stdout: {} stderr: {}",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
        return Ok(None);
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        return Ok(None);
    }

    Ok(Some(version))
}

fn normalized_kubo_version(version: &str) -> &str {
    version.trim().trim_start_matches('v')
}

fn ensure_repo_initialized(ipfs_path: &Path, repo_dir: &Path) -> Result<()> {
    if repo_dir.join("config").exists() {
        return Ok(());
    }

    fs::create_dir_all(repo_dir).wrap_err("Failed to create IPFS repo dir")?;
    set_ipfs_permissions(ipfs_path)?;

    let output = Command::new(ipfs_path)
        .arg("init")
        .env("IPFS_PATH", repo_dir)
        .output()
        .wrap_err("Failed to run kubo init")?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre::eyre!(
            "kubo init failed (status: {}). stdout: {} stderr: {}",
            output.status,
            stdout.trim(),
            stderr.trim()
        ));
    }

    Ok(())
}

fn update_gateway_config(repo_dir: &Path, gateway_port: u16) -> Result<()> {
    let config_path = repo_dir.join("config");
    let contents = fs::read_to_string(&config_path).wrap_err("Failed to read kubo config")?;
    let mut config: serde_json::Value =
        serde_json::from_str(&contents).wrap_err("Failed to parse kubo config")?;

    config["Addresses"]["Gateway"] =
        serde_json::Value::String(format!("/ip4/127.0.0.1/tcp/{gateway_port}"));

    config["API"]["HTTPHeaders"] = serde_json::json!({
        "Access-Control-Allow-Origin": [
            "https://webui.ipfs.io",
            "https://ipfs.localhost",
            "http://127.0.0.1:5001"
        ],
        "Access-Control-Allow-Methods": ["GET", "POST", "PUT"],
        "Access-Control-Allow-Headers": ["X-Requested-With", "Content-Type"]
    });

    let updated =
        serde_json::to_string_pretty(&config).wrap_err("Failed to serialize kubo config")?;
    fs::write(&config_path, updated).wrap_err("Failed to write kubo config")?;
    Ok(())
}

fn start_kubo_daemon(ipfs_path: &Path, repo_dir: &Path, offline: bool) -> Result<Child> {
    let mut command = Command::new(ipfs_path);
    command.arg("daemon");
    if offline {
        command.arg("--offline");
    }
    command
        .env("IPFS_PATH", repo_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .wrap_err("Failed to start kubo daemon")
}

fn kubo_platform_target() -> Result<(String, String)> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => return Err(eyre::eyre!("Unsupported OS: {other}")),
    };

    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => return Err(eyre::eyre!("Unsupported arch: {other}")),
    };

    Ok((os.to_string(), arch.to_string()))
}

fn kubo_paths(base_dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let ipfs_dir = base_dir.join("ipfs");
    let bin_dir = ipfs_dir.join("bin");
    let repo_dir = ipfs_dir.join("repo");
    let download_dir = ipfs_dir.join("downloads");
    (bin_dir, repo_dir, download_dir)
}

fn find_ipfs_binary(dir: &Path) -> Result<PathBuf> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir).wrap_err("Failed to read kubo extract dir")? {
            let entry = entry.wrap_err("Failed to read kubo extract entry")?;
            let path = entry.path();
            if path.is_dir() {
                if let Ok(found) = find_ipfs_binary(&path) {
                    return Ok(found);
                }
            } else if path.file_name().map(|name| name == "ipfs").unwrap_or(false) {
                return Ok(path);
            }
        }
    }

    Err(eyre::eyre!("IPFS binary not found after extraction"))
}
