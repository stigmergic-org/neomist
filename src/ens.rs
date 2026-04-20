use alloy::ens::{ENS_ADDRESS, namehash};
use alloy::primitives::{Address, B256, Bytes as AlloyBytes, address, keccak256};
use alloy::providers::DynProvider;
use alloy::sol;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header::HOST};
use eyre::{Report, Result, WrapErr};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;
use url::form_urlencoded;

use crate::constants::MFS_CACHE_DIR;
use crate::state::AppState;

sol! {
    #[sol(rpc)]
    contract EnsRegistry {
        function resolver(bytes32 node) view returns (address);
    }

    #[sol(rpc)]
    interface EnsResolver {
        function contenthash(bytes32 node) view returns (bytes);
    }

    #[sol(rpc)]
    contract WeiNameService {
        function contenthash(bytes32 node) view returns (bytes);
    }
}

const WEI_NODE: B256 =
    alloy::primitives::b256!("0xa82820059d5df798546bcc2985157a77c3eef25eba9ba01899927333efacbd6f");
const WEI_REGISTRY: Address = address!("0x0000000000696760E15f265e828DB644A0c242EB");
const OFFLINE_LOOKUP_PATTERNS: &[&str] = &[
    "dns error",
    "error sending request",
    "failed to lookup address information",
    "out of sync",
    "seconds behind",
    "network is unreachable",
    "no route to host",
    "temporarily unavailable",
    "connection refused",
    "connection reset",
    "host is down",
    "host unreachable",
    "network unreachable",
    "timed out",
    "timeout",
];

pub async fn proxy_request(state: &AppState, request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let host = parts
        .headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let host_only = host.split(':').next().unwrap_or("");
    let ens_name = host_only.trim_end_matches(".localhost");

    let path = parts.uri.path();
    let query = parts.uri.query();

    let (cid, refresh_cache) = match resolve_contenthash(&state.ens_provider, ens_name).await {
        Ok(Some(cid)) => (cid, true),
        Ok(None) => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "text/plain; charset=utf-8")
                .body(Body::from("No IPFS contenthash found"))
                .unwrap();
        }
        Err(err) => {
            if is_offline_lookup_error(&err) {
                match latest_cached_cid(state, ens_name).await {
                    Ok(Some(cid)) => {
                        warn!(
                            "ENS lookup failed for {ens_name} while offline, using cached CID {cid}: {err:?}"
                        );
                        (cid, false)
                    }
                    Ok(None) => {
                        warn!(
                            "ENS lookup failed for {ens_name} while offline and no cached record is available: {err:?}"
                        );
                        return ens_lookup_failed_response();
                    }
                    Err(cache_err) => {
                        warn!(
                            "ENS lookup failed for {ens_name} and cached fallback lookup failed: {err:?}; cache error: {cache_err:?}"
                        );
                        return ens_lookup_failed_response();
                    }
                }
            } else {
                warn!("ENS lookup failed for {ens_name}: {err:?}");
                return ens_lookup_failed_response();
            }
        }
    };

    if refresh_cache && let Err(err) = update_mfs_cache(state, ens_name, &cid).await {
        warn!("MFS cache update failed for {ens_name}: {err}");
    }

    let mut url = format!(
        "http://{}.ipfs.localhost:{}{}",
        cid, state.ipfs_gateway_port, path
    );
    if let Some(query) = query {
        url.push('?');
        url.push_str(query);
    }

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("Invalid request body"))
                .unwrap();
        }
    };

    let mut req_builder = state.http_client.request(parts.method.clone(), url);

    for (name, value) in parts.headers.iter() {
        let name_str = name.as_str();
        if name_str.eq_ignore_ascii_case("host")
            || name_str.eq_ignore_ascii_case("connection")
            || name_str.eq_ignore_ascii_case("keep-alive")
            || name_str.eq_ignore_ascii_case("proxy-authenticate")
            || name_str.eq_ignore_ascii_case("proxy-authorization")
            || name_str.eq_ignore_ascii_case("te")
            || name_str.eq_ignore_ascii_case("trailers")
            || name_str.eq_ignore_ascii_case("transfer-encoding")
            || name_str.eq_ignore_ascii_case("upgrade")
        {
            continue;
        }
        req_builder = req_builder.header(name, value);
    }

    let upstream = match req_builder.body(body_bytes).send().await {
        Ok(resp) => resp,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::from("IPFS gateway request failed"))
                .unwrap();
        }
    };

    let status = upstream.status();
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream.headers().iter() {
        let name_str = name.as_str();
        if name_str.eq_ignore_ascii_case("connection")
            || name_str.eq_ignore_ascii_case("keep-alive")
            || name_str.eq_ignore_ascii_case("proxy-authenticate")
            || name_str.eq_ignore_ascii_case("proxy-authorization")
            || name_str.eq_ignore_ascii_case("te")
            || name_str.eq_ignore_ascii_case("trailers")
            || name_str.eq_ignore_ascii_case("transfer-encoding")
            || name_str.eq_ignore_ascii_case("upgrade")
        {
            continue;
        }
        builder = builder.header(name, value);
    }

    match upstream.bytes().await {
        Ok(bytes) => builder.body(Body::from(bytes)).unwrap(),
        Err(_) => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::from("IPFS gateway response failed"))
            .unwrap(),
    }
}

pub async fn pin_cid(state: &AppState, cid: &str) -> Result<()> {
    let pin_url = format!(
        "{}/api/v0/pin/add?arg={}",
        state.ipfs_api_url,
        encode_arg(&format!("/ipfs/{cid}"))
    );
    let response = state
        .http_client
        .post(pin_url)
        .send()
        .await
        .wrap_err("Failed to pin CID")?;
    if !response.status().is_success() {
        return Err(eyre::eyre!("Pin failed with status {}", response.status()));
    }
    Ok(())
}

pub async fn update_mfs_cache(state: &AppState, site: &str, cid: &str) -> Result<()> {
    let base_path = cache_base_path(site);

    let latest = latest_mfs_entry(state, &base_path).await?;
    if let Some((_timestamp, existing_cid)) = &latest {
        if existing_cid == cid {
            return Ok(());
        }
    }

    let mut ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .wrap_err("Failed to get timestamp")?
        .as_secs();

    let mut target = format!("{base_path}/{ts}");
    let mut attempts = 0;
    while mfs_path_exists(state, &target).await? && attempts < 5 {
        ts += 1;
        target = format!("{base_path}/{ts}");
        attempts += 1;
    }

    let copy_url = format!(
        "{}/api/v0/files/cp?arg={}&arg={}&parents=true",
        state.ipfs_api_url,
        encode_arg(&format!("/ipfs/{cid}")),
        encode_arg(&target)
    );
    let response = state
        .http_client
        .post(copy_url)
        .send()
        .await
        .wrap_err("Failed to copy CID into MFS")?;
    if !response.status().is_success() {
        return Err(eyre::eyre!(
            "MFS copy failed with status {}",
            response.status()
        ));
    }

    Ok(())
}

async fn latest_mfs_entry(state: &AppState, base_path: &str) -> Result<Option<(String, String)>> {
    let list_url = format!(
        "{}/api/v0/files/ls?arg={}",
        state.ipfs_api_url,
        encode_arg(base_path)
    );
    let response = state
        .http_client
        .post(list_url)
        .send()
        .await
        .wrap_err("Failed to list MFS directory")?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let body: serde_json::Value = response
        .json()
        .await
        .wrap_err("Failed to parse MFS ls response")?;

    let entries = body
        .get("Entries")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut latest: Option<(String, String)> = None;
    let mut latest_ts: u64 = 0;

    for entry in entries {
        let name = match entry.get("Name").and_then(|value| value.as_str()) {
            Some(name) => name,
            None => continue,
        };
        let ts: u64 = match name.parse() {
            Ok(ts) => ts,
            Err(_) => continue,
        };

        if ts >= latest_ts {
            let path = format!("{}/{}", base_path, name);
            if let Ok(hash) = mfs_stat_hash(state, &path).await {
                latest_ts = ts;
                latest = Some((name.to_string(), hash));
            }
        }
    }

    Ok(latest)
}

pub async fn latest_cached_cid(state: &AppState, site: &str) -> Result<Option<String>> {
    Ok(latest_mfs_entry(state, &cache_base_path(site))
        .await?
        .map(|(_, cid)| cid))
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
        return Err(eyre::eyre!(
            "MFS stat failed with status {}",
            response.status()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .wrap_err("Failed to parse MFS stat response")?;
    let hash = body
        .get("Hash")
        .and_then(|value| value.as_str())
        .ok_or_else(|| eyre::eyre!("MFS stat missing Hash"))?;
    Ok(hash.to_string())
}

async fn mfs_path_exists(state: &AppState, path: &str) -> Result<bool> {
    let url = format!(
        "{}/api/v0/files/stat?arg={}",
        state.ipfs_api_url,
        encode_arg(path)
    );
    let response = state.http_client.post(url).send().await;
    Ok(response
        .map(|resp| resp.status().is_success())
        .unwrap_or(false))
}

fn encode_arg(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn cache_base_path(site: &str) -> String {
    let safe_site = site.replace('/', "_");
    format!("{MFS_CACHE_DIR}/{safe_site}")
}

fn ens_lookup_failed_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Body::from("ENS lookup failed"))
        .unwrap()
}

fn is_offline_lookup_error(err: &Report) -> bool {
    err.chain()
        .any(|cause| looks_like_offline_lookup_message(&cause.to_string()))
}

fn looks_like_offline_lookup_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    OFFLINE_LOOKUP_PATTERNS
        .iter()
        .any(|pattern| message.contains(pattern))
}

fn decode_ipfs_contenthash(bytes: &AlloyBytes) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }

    let mut value: u64 = 0;
    let mut shift = 0;
    let mut index = 0;

    for byte in bytes.iter() {
        let low = (byte & 0x7f) as u64;
        value |= low << shift;
        index += 1;

        if (byte & 0x80) == 0 {
            break;
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }

    if index == 0 || index >= bytes.len() {
        return None;
    }

    const IPFS_CODEC: u64 = 0xe3;
    if value != IPFS_CODEC {
        return None;
    }

    let cid_bytes = &bytes[index..];
    let cid = cid::Cid::try_from(cid_bytes).ok()?;
    Some(cid.to_string())
}

pub async fn resolve_contenthash(provider: &DynProvider, host: &str) -> Result<Option<String>> {
    if host.ends_with(".wei") {
        return resolve_wei_ipfs(provider, host).await;
    }
    resolve_ens_ipfs(provider, host).await
}

async fn resolve_ens_ipfs(provider: &DynProvider, ens_name: &str) -> Result<Option<String>> {
    let node: B256 = namehash(ens_name);
    let registry = EnsRegistry::new(ENS_ADDRESS, provider);
    let resolver_addr: Address = registry
        .resolver(node)
        .call()
        .await
        .wrap_err("Failed to resolve ENS resolver address")?;

    if resolver_addr == Address::ZERO {
        return Ok(None);
    }

    let resolver = EnsResolver::new(resolver_addr, provider);
    let contenthash = resolver
        .contenthash(node)
        .call()
        .await
        .wrap_err("Failed to resolve ENS contenthash")?;

    Ok(decode_ipfs_contenthash(&contenthash))
}

async fn resolve_wei_ipfs(provider: &DynProvider, host: &str) -> Result<Option<String>> {
    let node = wei_namehash(host);
    let contract = WeiNameService::new(WEI_REGISTRY, provider);
    let contenthash = contract
        .contenthash(node)
        .call()
        .await
        .wrap_err("Failed to resolve .wei contenthash")?;

    Ok(decode_ipfs_contenthash(&contenthash))
}

fn wei_namehash(name: &str) -> B256 {
    let mut trimmed = name;
    if let Some(base) = name.strip_suffix(".wei") {
        trimmed = base;
    }

    let mut node = WEI_NODE;
    if trimmed.is_empty() {
        return node;
    }

    for label in trimmed.rsplit('.') {
        let lower = label
            .chars()
            .map(|ch| {
                if ch.is_ascii_uppercase() {
                    ch.to_ascii_lowercase()
                } else {
                    ch
                }
            })
            .collect::<String>();

        let label_hash = keccak256(lower.as_bytes());
        let mut buffer = [0u8; 64];
        buffer[..32].copy_from_slice(node.as_slice());
        buffer[32..].copy_from_slice(label_hash.as_slice());
        node = keccak256(buffer);
    }

    node
}

#[cfg(test)]
mod tests {
    use super::looks_like_offline_lookup_message;

    #[test]
    fn detects_network_unreachable_lookup_failures() {
        assert!(looks_like_offline_lookup_message(
            "error sending request for url (https://rpc.example): Network is unreachable (os error 51)"
        ));
        assert!(looks_like_offline_lookup_message(
            "failed to lookup address information: nodename nor servname provided, or not known"
        ));
        assert!(looks_like_offline_lookup_message(
            "server returned an error response: error code 1: out of sync: 1774266794 seconds behind"
        ));
    }

    #[test]
    fn ignores_non_network_lookup_failures() {
        assert!(!looks_like_offline_lookup_message(
            "failed to resolve ENS contenthash: execution reverted"
        ));
    }
}
