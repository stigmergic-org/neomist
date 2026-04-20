use std::time::{Duration, UNIX_EPOCH};

use eyre::{Result, WrapErr};
use reqwest::multipart;
use serde::Serialize;
use tracing::warn;
use url::form_urlencoded;

use crate::constants::MFS_CACHE_DIR;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct CachedDomain {
    pub domain: String,
    pub cached_at: String,
    pub local_size: u64,
    pub full_size: u64,
    pub auto_seeding: bool,
    pub visit_url: String,
    pub versions: Vec<CachedVersion>,
}

#[derive(Debug, Serialize)]
pub struct CachedVersion {
    pub timestamp: u64,
    pub cid: String,
    pub cached_at: String,
    pub local_size: u64,
    pub full_size: u64,
    pub visit_url: String,
}

#[derive(Debug)]
struct MfsStat {
    hash: String,
    local_size: u64,
    full_size: u64,
}

pub async fn list_cached_domains(state: &AppState) -> Result<Vec<CachedDomain>> {
    let sites = list_mfs_dir(state, MFS_CACHE_DIR).await?;
    let mut results = Vec::new();

    for site in sites {
        if site == "" || site == "." || site == ".." {
            continue;
        }

        let versions = list_site_versions(state, &site).await?;
        if versions.is_empty() {
            continue;
        }

        let auto_seeding = read_autoseed_flag(state, &site).await.unwrap_or(false);
        let site_path = format!("{MFS_CACHE_DIR}/{site}");
        let (local_size, full_size) = match mfs_stat_with_local(state, &site_path).await {
            Ok(stat) => (stat.local_size, stat.full_size),
            Err(err) => {
                warn!("Failed to stat cache domain {site}: {err}");
                (
                    versions.iter().map(|version| version.local_size).sum(),
                    versions.iter().map(|version| version.full_size).sum(),
                )
            }
        };

        let cached_at = versions
            .first()
            .map(|version| version.cached_at.clone())
            .unwrap_or_default();
        let visit_url = versions
            .first()
            .map(|version| version.visit_url.clone())
            .unwrap_or_default();

        results.push(CachedDomain {
            domain: site,
            cached_at,
            local_size,
            full_size,
            auto_seeding,
            visit_url,
            versions,
        });
    }

    results.sort_by(|a, b| a.domain.cmp(&b.domain));
    Ok(results)
}

pub async fn total_storage_used(state: &AppState) -> Result<u64> {
    let url = format!(
        "{}/api/v0/files/stat?arg={}&with-local=true",
        state.ipfs_api_url,
        encode_arg(MFS_CACHE_DIR)
    );
    let response: serde_json::Value = state
        .http_client
        .post(url)
        .send()
        .await
        .wrap_err("Failed to stat MFS cache")?
        .json()
        .await
        .wrap_err("Failed to parse MFS stat")?;

    let local = response
        .get("Local")
        .and_then(|v| v.as_u64())
        .or_else(|| response.get("SizeLocal").and_then(|v| v.as_u64()))
        .or_else(|| response.get("CumulativeSize").and_then(|v| v.as_u64()))
        .unwrap_or(0);
    Ok(local)
}

pub async fn toggle_autoseed(state: &AppState, domain: &str, enable: bool) -> Result<()> {
    let safe_domain = domain.replace('/', "_");
    let path = format!("{MFS_CACHE_DIR}/{safe_domain}/autoseed");

    if enable {
        let dir_path = format!("{MFS_CACHE_DIR}/{safe_domain}");
        let mkdir_url = format!(
            "{}/api/v0/files/mkdir?arg={}&parents=true",
            state.ipfs_api_url,
            encode_arg(&dir_path)
        );
        let _ = state.http_client.post(mkdir_url).send().await;

        let url = format!(
            "{}/api/v0/files/write?arg={}&create=true&truncate=true&parents=true",
            state.ipfs_api_url,
            encode_arg(&path)
        );
        let form = multipart::Form::new().part("data", multipart::Part::text("true"));
        let response = state
            .http_client
            .post(url)
            .multipart(form)
            .send()
            .await
            .wrap_err("Failed to write autoseed flag")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(eyre::eyre!(
                "Failed to write autoseed flag (status {}): {}",
                status,
                body
            ));
        }
    } else {
        let url = format!(
            "{}/api/v0/files/rm?arg={}",
            state.ipfs_api_url,
            encode_arg(&path)
        );
        let response = state
            .http_client
            .post(url)
            .send()
            .await
            .wrap_err("Failed to remove autoseed flag")?;
        if !response.status().is_success() {
            return Err(eyre::eyre!("Failed to remove autoseed flag"));
        }
    }

    if enable {
        if let Ok(Some(latest_cid)) = crate::ens::latest_cached_cid(state, domain).await {
            let _ = crate::ens::pin_cid(state, &latest_cid).await;
        }
    }

    Ok(())
}

pub async fn clear_cache(state: &AppState, domain: &str, version: Option<&str>) -> Result<()> {
    let safe_domain = domain.replace('/', "_");
    let path = if let Some(version) = version {
        if version.is_empty() || !version.bytes().all(|ch| ch.is_ascii_digit()) {
            return Err(eyre::eyre!("Invalid cache version"));
        }
        format!("{MFS_CACHE_DIR}/{safe_domain}/{version}")
    } else {
        format!("{MFS_CACHE_DIR}/{safe_domain}")
    };

    let url = format!(
        "{}/api/v0/files/rm?arg={}&recursive=true&force=true",
        state.ipfs_api_url,
        encode_arg(&path)
    );
    let response = state
        .http_client
        .post(url)
        .send()
        .await
        .wrap_err("Failed to remove cache")?;
    if !response.status().is_success() {
        return Err(eyre::eyre!("Failed to remove cache"));
    }
    Ok(())
}

async fn list_site_versions(state: &AppState, site: &str) -> Result<Vec<CachedVersion>> {
    let site_path = format!("{MFS_CACHE_DIR}/{site}");
    let entries = list_mfs_dir(state, &site_path).await?;
    let mut versions = Vec::new();

    for entry in entries {
        if entry == "autoseed" {
            continue;
        }

        let ts = match entry.parse::<u64>() {
            Ok(ts) => ts,
            Err(_) => continue,
        };

        let stat_path = format!("{site_path}/{entry}");
        let stat = match mfs_stat_with_local(state, &stat_path).await {
            Ok(stat) => stat,
            Err(err) => {
                warn!("Failed to stat cache entry {site}/{entry}: {err}");
                continue;
            }
        };
        let visit_url = cached_gateway_url(&stat.hash);

        versions.push(CachedVersion {
            timestamp: ts,
            cid: stat.hash,
            cached_at: timestamp_to_iso(ts).unwrap_or_default(),
            local_size: stat.local_size,
            full_size: stat.full_size,
            visit_url,
        });
    }

    versions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(versions)
}

async fn read_autoseed_flag(state: &AppState, site: &str) -> Result<bool> {
    let path = format!("{MFS_CACHE_DIR}/{site}/autoseed");
    let url = format!(
        "{}/api/v0/files/read?arg={}",
        state.ipfs_api_url,
        encode_arg(&path)
    );
    let response = state.http_client.post(url).send().await;
    let response = match response {
        Ok(response) => response,
        Err(_) => return Ok(false),
    };

    if !response.status().is_success() {
        return Ok(false);
    }

    let text = response.text().await.unwrap_or_default();
    Ok(text.trim() == "true")
}

async fn list_mfs_dir(state: &AppState, path: &str) -> Result<Vec<String>> {
    let url = format!(
        "{}/api/v0/files/ls?arg={}",
        state.ipfs_api_url,
        encode_arg(path)
    );
    let response = state
        .http_client
        .post(url)
        .send()
        .await
        .wrap_err("Failed to list MFS directory")?;

    if !response.status().is_success() {
        return Ok(Vec::new());
    }

    let body: serde_json::Value = response.json().await.wrap_err("Failed to parse MFS ls")?;
    let entries = body
        .get("Entries")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut names = Vec::new();
    for entry in entries {
        if let Some(name) = entry.get("Name").and_then(|value| value.as_str()) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

async fn mfs_stat_with_local(state: &AppState, path: &str) -> Result<MfsStat> {
    let url = format!(
        "{}/api/v0/files/stat?arg={}&with-local=true",
        state.ipfs_api_url,
        encode_arg(path)
    );
    let response = state
        .http_client
        .post(url)
        .send()
        .await
        .wrap_err("Failed to stat MFS path")?;
    if !response.status().is_success() {
        return Err(eyre::eyre!("MFS stat failed"));
    }
    let body: serde_json::Value = response.json().await.wrap_err("Failed to parse MFS stat")?;
    let hash = body
        .get("Hash")
        .and_then(|value| value.as_str())
        .ok_or_else(|| eyre::eyre!("MFS stat missing Hash"))?;

    let local_size = body
        .get("SizeLocal")
        .and_then(|value| value.as_u64())
        .or_else(|| body.get("Local").and_then(|value| value.as_u64()))
        .or_else(|| body.get("CumulativeSize").and_then(|value| value.as_u64()))
        .or_else(|| body.get("Size").and_then(|value| value.as_u64()))
        .unwrap_or(0);

    let full_size = body
        .get("CumulativeSize")
        .and_then(|value| value.as_u64())
        .or_else(|| body.get("Size").and_then(|value| value.as_u64()))
        .or_else(|| body.get("SizeLocal").and_then(|value| value.as_u64()))
        .unwrap_or(local_size);

    Ok(MfsStat {
        hash: hash.to_string(),
        local_size,
        full_size,
    })
}

fn encode_arg(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn timestamp_to_iso(ts: u64) -> Option<String> {
    let duration = Duration::from_secs(ts);
    let time = UNIX_EPOCH.checked_add(duration)?;
    let datetime: chrono::DateTime<chrono::Utc> = time.into();
    Some(datetime.to_rfc3339())
}

fn cached_gateway_url(cid: &str) -> String {
    format!("https://{cid}.ipfs.localhost/")
}
