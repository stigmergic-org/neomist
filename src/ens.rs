use alloy::ens::{ENS_ADDRESS, namehash};
use alloy::primitives::{Address, B256, Bytes as AlloyBytes, address, keccak256};
use alloy::providers::DynProvider;
use alloy::sol;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header::HOST};
use eyre::{Report, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;
use url::form_urlencoded;

use crate::constants::MFS_CACHE_DIR;
use crate::site_error;
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

const IPFS_CODEC: u64 = 0xe3;
const IPNS_CODEC: u64 = 0xe5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "protocol", content = "target", rename_all = "lowercase")]
pub enum ResolvedContenthash {
    Ipfs(String),
    Ipns(String),
}

impl ResolvedContenthash {
    pub fn target(&self) -> &str {
        match self {
            Self::Ipfs(target) | Self::Ipns(target) => target,
        }
    }

    pub fn gateway_url(&self, gateway_port: u16, path: &str) -> String {
        match self {
            Self::Ipfs(cid) => format!("http://{cid}.ipfs.localhost:{gateway_port}{path}"),
            Self::Ipns(name) => format!("http://127.0.0.1:{gateway_port}/ipns/{name}{path}"),
        }
    }

    fn resolved_cid(&self) -> Option<&str> {
        match self {
            Self::Ipfs(cid) => Some(cid),
            Self::Ipns(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ContenthashRecord {
    Missing,
    Supported(ResolvedContenthash),
    UnsupportedCodec(u64),
    Invalid(&'static str),
}

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
    let wants_html_error_page = site_error::prefers_html_error_page(&parts.headers);

    let (contenthash, refresh_cache) = match resolve_contenthash_record(&state.ens_provider, ens_name).await {
        Ok(ContenthashRecord::Supported(contenthash)) => (contenthash, true),
        Ok(ContenthashRecord::Missing) => {
            return site_error::build_site_error_response(
                StatusCode::NOT_FOUND,
                wants_html_error_page,
                "No contenthash record",
                "This domain does not have an IPFS or IPNS contenthash record, so NeoMist has no site content to load.",
                Some("Resolver returned no contenthash for this name."),
                ens_name,
                path,
            );
        }
        Ok(ContenthashRecord::UnsupportedCodec(codec)) => {
            let detail = format!(
                "Unsupported contenthash codec 0x{codec:x}. NeoMist currently supports only IPFS and IPNS contenthash records."
            );
            return site_error::build_site_error_response(
                StatusCode::BAD_GATEWAY,
                wants_html_error_page,
                "Unsupported contenthash",
                "This domain uses unsupported contenthash protocol. NeoMist supports only IPFS and IPNS contenthash records.",
                Some(&detail),
                ens_name,
                path,
            );
        }
        Ok(ContenthashRecord::Invalid(reason)) => {
            let detail = format!("Contenthash record is malformed: {reason}");
            return site_error::build_site_error_response(
                StatusCode::BAD_GATEWAY,
                wants_html_error_page,
                "Invalid contenthash record",
                "This domain has contenthash record, but resolver returned malformed data.",
                Some(&detail),
                ens_name,
                path,
            );
        }
        Err(err) => {
            if is_offline_lookup_error(&err) {
                match latest_cached_cid(state, ens_name).await {
                    Ok(Some(cid)) => {
                        warn!(
                            "ENS lookup failed for {ens_name} while offline, using cached CID {cid}: {err:?}"
                        );
                        (ResolvedContenthash::Ipfs(cid), false)
                    }
                    Ok(None) => {
                        warn!(
                            "ENS lookup failed for {ens_name} while offline and no cached record is available: {err:?}"
                        );
                        let detail = format!("Failed to resolve contenthash while offline: {err:#}");
                        return ens_lookup_failed_response(
                            ens_name,
                            path,
                            wants_html_error_page,
                            Some(&detail),
                        );
                    }
                    Err(cache_err) => {
                        warn!(
                            "ENS lookup failed for {ens_name} and cached fallback lookup failed: {err:?}; cache error: {cache_err:?}"
                        );
                        let detail = format!(
                            "Failed to resolve contenthash while offline: {err:#}. Cached fallback also failed: {cache_err:#}"
                        );
                        return ens_lookup_failed_response(
                            ens_name,
                            path,
                            wants_html_error_page,
                            Some(&detail),
                        );
                    }
                }
            } else {
                warn!("ENS lookup failed for {ens_name}: {err:?}");
                let detail = format!("{err:#}");
                return ens_lookup_failed_response(
                    ens_name,
                    path,
                    wants_html_error_page,
                    Some(&detail),
                );
            }
        }
    };

    if refresh_cache
        && let Err(err) = crate::cache::write_contenthash_metadata(state, ens_name, &contenthash).await
    {
        warn!("Contenthash metadata update failed for {ens_name}: {err}");
    }

    if refresh_cache && let Err(err) = update_mfs_cache(state, ens_name, &contenthash).await {
        warn!("MFS cache update failed for {ens_name}: {err}");
    }

    let mut url = contenthash.gateway_url(state.ipfs_gateway_port, path);
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
        Err(err) => {
            let detail = format!("IPFS gateway request failed: {err}");
            return site_error::build_site_error_response(
                StatusCode::BAD_GATEWAY,
                wants_html_error_page,
                "Content load failed",
                "NeoMist resolved this domain but could not load content from local Kubo gateway.",
                Some(&detail),
                ens_name,
                path,
            );
        }
    };

    let status = upstream.status();
    let upstream_content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let upstream_headers = upstream
        .headers()
        .iter()
        .filter_map(|(name, value)| {
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
            return None;
        }
        Some((name.clone(), value.clone()))
    })
        .collect::<Vec<_>>();

    match upstream.bytes().await {
        Ok(bytes) => {
            if wants_html_error_page && (status.is_client_error() || status.is_server_error()) {
                let detail = site_error::summarize_error_detail(upstream_content_type.as_deref(), &bytes)
                    .or_else(|| Some(status.to_string()));
                let (title, summary) = ens_gateway_error_copy(status);
                return site_error::build_site_error_response(
                    status,
                    true,
                    title,
                    summary,
                    detail.as_deref(),
                    ens_name,
                    path,
                );
            }

            let mut builder = Response::builder().status(status);
            for (name, value) in upstream_headers {
                builder = builder.header(name, value);
            }
            builder.body(Body::from(bytes)).unwrap()
        }
        Err(err) => {
            let detail = format!("IPFS gateway response failed: {err}");
            site_error::build_site_error_response(
                StatusCode::BAD_GATEWAY,
                wants_html_error_page,
                "Content load failed",
                "NeoMist resolved this domain but could not read response from local Kubo gateway.",
                Some(&detail),
                ens_name,
                path,
            )
        }
    }
}

pub async fn pin_cid(state: &AppState, cid: &str) -> Result<()> {
    pin_content(state, &ResolvedContenthash::Ipfs(cid.to_string())).await
}

pub async fn pin_content(state: &AppState, contenthash: &ResolvedContenthash) -> Result<()> {
    let pin_target = snapshot_ipfs_path(state, contenthash).await?;
    let pin_url = format!(
        "{}/api/v0/pin/add?arg={}",
        state.ipfs_api_url,
        encode_arg(&pin_target)
    );
    let response = state
        .http_client
        .post(pin_url)
        .send()
        .await
        .wrap_err("Failed to pin contenthash target")?;
    if !response.status().is_success() {
        return Err(eyre::eyre!("Pin failed with status {}", response.status()));
    }
    Ok(())
}

pub async fn update_mfs_cache(
    state: &AppState,
    site: &str,
    contenthash: &ResolvedContenthash,
) -> Result<bool> {
    let base_path = cache_base_path(site);
    let resolved_cid = snapshot_cid(state, contenthash).await?;

    let latest = latest_mfs_entry(state, &base_path).await?;
    if let Some((_timestamp, existing_cid)) = &latest {
        if existing_cid == &resolved_cid {
            return Ok(false);
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
        encode_arg(&format!("/ipfs/{resolved_cid}")),
        encode_arg(&target)
    );
    let response = state
        .http_client
        .post(copy_url)
        .send()
        .await
        .wrap_err("Failed to copy contenthash target into MFS")?;
    if !response.status().is_success() {
        return Err(eyre::eyre!(
            "MFS copy failed with status {}",
            response.status()
        ));
    }

    Ok(true)
}

async fn snapshot_ipfs_path(state: &AppState, contenthash: &ResolvedContenthash) -> Result<String> {
    Ok(format!("/ipfs/{}", snapshot_cid(state, contenthash).await?))
}

async fn snapshot_cid(state: &AppState, contenthash: &ResolvedContenthash) -> Result<String> {
    match contenthash.resolved_cid() {
        Some(cid) => Ok(cid.to_string()),
        None => resolve_ipns_cid(state, contenthash.target()).await,
    }
}

async fn resolve_ipns_cid(state: &AppState, name: &str) -> Result<String> {
    let url = format!(
        "{}/api/v0/name/resolve?arg={}&recursive=true",
        state.ipfs_api_url,
        encode_arg(&format!("/ipns/{name}"))
    );
    let response = state
        .http_client
        .post(url)
        .send()
        .await
        .wrap_err("Failed to resolve IPNS target")?;
    if !response.status().is_success() {
        return Err(eyre::eyre!(
            "IPNS resolve failed with status {}",
            response.status()
        ));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .wrap_err("Failed to parse IPNS resolve response")?;
    let path = body
        .get("Path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| eyre::eyre!("IPNS resolve response missing Path"))?;
    let cid = path
        .strip_prefix("/ipfs/")
        .and_then(|value| value.split('/').next())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre::eyre!("IPNS resolve response missing root CID"))?;
    Ok(cid.to_string())
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

fn ens_lookup_failed_response(
    ens_name: &str,
    path: &str,
    wants_html_error_page: bool,
    detail: Option<&str>,
) -> Response<Body> {
    site_error::build_site_error_response(
        StatusCode::BAD_GATEWAY,
        wants_html_error_page,
        "Name lookup failed",
        "NeoMist could not resolve contenthash for this .eth or .wei domain.",
        detail.or(Some("ENS lookup failed")),
        ens_name,
        path,
    )
}

fn ens_gateway_error_copy(status: StatusCode) -> (&'static str, &'static str) {
    if status == StatusCode::NOT_FOUND {
        (
            "Page not found",
            "Domain resolved, but requested path was not found in site content served through local Kubo gateway.",
        )
    } else if status.is_server_error() {
        (
            "Kubo gateway error",
            "NeoMist resolved this domain, but local Kubo gateway failed before site content could load.",
        )
    } else {
        (
            "Content load failed",
            "NeoMist resolved this domain, but local Kubo gateway returned an error before site could load.",
        )
    }
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

#[cfg(test)]
fn decode_contenthash(bytes: &AlloyBytes) -> Option<ResolvedContenthash> {
    match inspect_contenthash(bytes) {
        ContenthashRecord::Supported(contenthash) => Some(contenthash),
        ContenthashRecord::Missing
        | ContenthashRecord::UnsupportedCodec(_)
        | ContenthashRecord::Invalid(_) => None,
    }
}

fn inspect_contenthash(bytes: &AlloyBytes) -> ContenthashRecord {
    if bytes.is_empty() {
        return ContenthashRecord::Missing;
    }

    let Some((codec, index)) = decode_varint(bytes) else {
        return ContenthashRecord::Invalid("could not decode contenthash prefix");
    };

    match codec {
        IPFS_CODEC | IPNS_CODEC => {
            let cid_bytes = &bytes[index..];
            let Ok(cid) = cid::Cid::try_from(cid_bytes) else {
                return ContenthashRecord::Invalid("payload is not valid CID data");
            };

            match codec {
                IPFS_CODEC => ContenthashRecord::Supported(ResolvedContenthash::Ipfs(cid.to_string())),
                IPNS_CODEC => ContenthashRecord::Supported(ResolvedContenthash::Ipns(cid.to_string())),
                _ => unreachable!(),
            }
        }
        _ => ContenthashRecord::UnsupportedCodec(codec),
    }
}

async fn resolve_contenthash_record(
    provider: &DynProvider,
    host: &str,
) -> Result<ContenthashRecord> {
    if host.ends_with(".wei") {
        return resolve_wei_contenthash_record(provider, host).await;
    }
    resolve_ens_contenthash_record(provider, host).await
}

fn decode_varint(bytes: &AlloyBytes) -> Option<(u64, usize)> {
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

    Some((value, index))
}

pub async fn resolve_contenthash(
    provider: &DynProvider,
    host: &str,
) -> Result<Option<ResolvedContenthash>> {
    Ok(match resolve_contenthash_record(provider, host).await? {
        ContenthashRecord::Supported(contenthash) => Some(contenthash),
        ContenthashRecord::Missing
        | ContenthashRecord::UnsupportedCodec(_)
        | ContenthashRecord::Invalid(_) => None,
    })
}

async fn resolve_ens_contenthash_record(
    provider: &DynProvider,
    ens_name: &str,
) -> Result<ContenthashRecord> {
    let node: B256 = namehash(ens_name);
    let registry = EnsRegistry::new(ENS_ADDRESS, provider);
    let resolver_addr: Address = registry
        .resolver(node)
        .call()
        .await
        .wrap_err("Failed to resolve ENS resolver address")?;

    if resolver_addr == Address::ZERO {
        return Ok(ContenthashRecord::Missing);
    }

    let resolver = EnsResolver::new(resolver_addr, provider);
    let contenthash = resolver
        .contenthash(node)
        .call()
        .await
        .wrap_err("Failed to resolve ENS contenthash")?;

    Ok(inspect_contenthash(&contenthash))
}

async fn resolve_wei_contenthash_record(
    provider: &DynProvider,
    host: &str,
) -> Result<ContenthashRecord> {
    let node = wei_namehash(host);
    let contract = WeiNameService::new(WEI_REGISTRY, provider);
    let contenthash = contract
        .contenthash(node)
        .call()
        .await
        .wrap_err("Failed to resolve .wei contenthash")?;

    Ok(inspect_contenthash(&contenthash))
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
    use super::{
        ContenthashRecord, IPFS_CODEC, IPNS_CODEC, ResolvedContenthash, decode_contenthash,
        inspect_contenthash, looks_like_offline_lookup_message,
    };
    use alloy::primitives::Bytes as AlloyBytes;
    use cid::{Cid, multihash::Multihash};
    use sha2::{Digest as _, Sha256};

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

    #[test]
    fn decodes_ipfs_contenthash() {
        let cid = test_cid(0x70, b"neomist-ipfs");
        let bytes = encode_contenthash(IPFS_CODEC, &cid);

        assert_eq!(
            decode_contenthash(&bytes),
            Some(ResolvedContenthash::Ipfs(cid.to_string()))
        );
    }

    #[test]
    fn decodes_ipns_contenthash() {
        let cid = test_cid(0x72, b"neomist-ipns");
        let bytes = encode_contenthash(IPNS_CODEC, &cid);

        assert_eq!(
            decode_contenthash(&bytes),
            Some(ResolvedContenthash::Ipns(cid.to_string()))
        );
    }

    #[test]
    fn reports_unsupported_contenthash_codec() {
        let cid = test_cid(0x70, b"neomist-swarm");
        let bytes = encode_contenthash(0xe4, &cid);

        assert_eq!(inspect_contenthash(&bytes), ContenthashRecord::UnsupportedCodec(0xe4));
    }

    #[test]
    fn reports_invalid_supported_contenthash_payload() {
        let mut bytes = encode_varint(IPFS_CODEC);
        bytes.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);

        assert_eq!(
            inspect_contenthash(&AlloyBytes::from(bytes)),
            ContenthashRecord::Invalid("payload is not valid CID data")
        );
    }

    fn encode_contenthash(codec: u64, cid: &Cid) -> AlloyBytes {
        let mut bytes = encode_varint(codec);
        bytes.extend_from_slice(&cid.to_bytes());
        AlloyBytes::from(bytes)
    }

    fn test_cid(codec: u64, bytes: &[u8]) -> Cid {
        let digest = Sha256::digest(bytes);
        let multihash = Multihash::<64>::wrap(0x12, &digest).unwrap();
        Cid::new_v1(codec, multihash)
    }

    fn encode_varint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                break;
            }
        }
        bytes
    }
}
