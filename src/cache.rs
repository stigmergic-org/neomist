use std::time::{Duration, UNIX_EPOCH};

use eyre::{Result, WrapErr};
use serde::Serialize;
use url::form_urlencoded;
use reqwest::multipart;

use crate::constants::MFS_CACHE_DIR;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct CachedDomain {
    pub domain: String,
    pub cid: String,
    pub last_cached: String,
    pub auto_seeding: bool,
}

pub async fn list_cached_domains(state: &AppState) -> Result<Vec<CachedDomain>> {
    let sites = list_mfs_dir(state, MFS_CACHE_DIR).await?;
    let mut results = Vec::new();

    for site in sites {
        if site == "" || site == "." || site == ".." {
            continue;
        }

        if let Some((timestamp, cid)) = latest_site_entry(state, &site).await? {
            let auto_seeding = read_autoseed_flag(state, &site).await.unwrap_or(false);
            let last_cached = timestamp_to_iso(timestamp).unwrap_or_else(|| "".to_string());
            results.push(CachedDomain {
                domain: site,
                cid,
                last_cached,
                auto_seeding,
            });
        }
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

    Ok(())
}

pub async fn clear_cache(state: &AppState, domain: &str) -> Result<()> {
    let safe_domain = domain.replace('/', "_");
    let path = format!("{MFS_CACHE_DIR}/{safe_domain}");
    let url = format!(
        "{}/api/v0/files/rm?arg={}&recursive=true",
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

async fn latest_site_entry(state: &AppState, site: &str) -> Result<Option<(u64, String)>> {
    let site_path = format!("{MFS_CACHE_DIR}/{site}");
    let entries = list_mfs_dir(state, &site_path).await?;
    let mut latest_ts: u64 = 0;
    let mut latest_cid: Option<String> = None;

    for entry in entries {
        if entry == "autoseed" {
            continue;
        }
        let ts = match entry.parse::<u64>() {
            Ok(ts) => ts,
            Err(_) => continue,
        };
        if ts >= latest_ts {
            let stat_path = format!("{site_path}/{entry}");
            if let Ok(cid) = mfs_stat_hash(state, &stat_path).await {
                latest_ts = ts;
                latest_cid = Some(cid);
            }
        }
    }

    Ok(latest_cid.map(|cid| (latest_ts, cid)))
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

    let body: serde_json::Value = response
        .json()
        .await
        .wrap_err("Failed to parse MFS ls")?;
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

async fn mfs_stat_hash(state: &AppState, path: &str) -> Result<String> {
    let url = format!(
        "{}/api/v0/files/stat?arg={}",
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
    let body: serde_json::Value = response
        .json()
        .await
        .wrap_err("Failed to parse MFS stat")?;
    let hash = body
        .get("Hash")
        .and_then(|value| value.as_str())
        .ok_or_else(|| eyre::eyre!("MFS stat missing Hash"))?;
    Ok(hash.to_string())
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
