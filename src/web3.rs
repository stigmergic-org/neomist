use std::str::FromStr;

use alloy::primitives::Address;
use alloy::providers::DynProvider;
use alloy::sol;
use axum::body::Body;
use axum::http::{
    HeaderName, HeaderValue, Method, Request, Response, StatusCode,
    header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, HOST, LOCATION},
};
use eyre::{Result, WrapErr};
use percent_encoding::percent_decode_str;
use url::{Url, form_urlencoded};

use crate::ens;
use crate::site_error;
use crate::state::AppState;

const LOCAL_HOST: &str = "neomist.localhost";
const WEB3_ROUTE_PREFIX: &str = "/web3";
const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

sol! {
    struct KeyValue {
        string key;
        string value;
    }

    #[sol(rpc)]
    interface Web3Site {
        function resolveMode() external view returns (bytes32);
        function request(string[] memory resource, KeyValue[] memory params)
            external
            view
            returns (uint16 statusCode, string memory body, KeyValue[] memory headers);
        function html() external view returns (string memory);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedAuthority {
    raw: String,
    host: String,
    chain_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedRequest {
    authority: ParsedAuthority,
    resource_path: String,
    resource: Vec<String>,
    query: Option<String>,
    fragment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RouteAction {
    Redirect(String),
    Target(ParsedRequest),
}

pub async fn proxy_request(state: &AppState, request: Request<Body>) -> Response<Body> {
    let (parts, _body) = request.into_parts();
    let wants_html_error_page = site_error::prefers_html_error_page(&parts.headers);
    let request_path = parts.uri.path().to_string();
    let local_host = parts
        .headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or(LOCAL_HOST);

    if parts.method != Method::GET && parts.method != Method::HEAD {
        return text_or_site_error(
            StatusCode::METHOD_NOT_ALLOWED,
            wants_html_error_page,
            "Method not allowed",
            "NeoMist web3 gateway supports only GET and HEAD requests.",
            Some("Allowed methods: GET, HEAD"),
            local_host,
            &request_path,
            Some((ALLOW, "GET, HEAD")),
        );
    }

    let parsed = match parse_local_request(parts.uri.path(), parts.uri.query()) {
        Ok(RouteAction::Redirect(location)) => {
            return redirect_response(&location);
        }
        Ok(RouteAction::Target(parsed)) => parsed,
        Err(err) => {
            return text_or_site_error(
                StatusCode::BAD_REQUEST,
                wants_html_error_page,
                "Invalid web3 URL",
                "NeoMist could not parse this web3 request.",
                Some(&err.to_string()),
                local_host,
                &request_path,
                None,
            );
        }
    };

    if let Some(chain_id) = parsed.authority.chain_id
        && chain_id != 1
    {
        let detail = format!(
            "Unsupported chain id {chain_id}. NeoMist currently supports only Ethereum mainnet (chain id 1)."
        );
        return text_or_site_error(
            StatusCode::BAD_REQUEST,
            wants_html_error_page,
            "Unsupported chain",
            "NeoMist can load only mainnet web3 content right now.",
            Some(&detail),
            local_host,
            &request_path,
            None,
        );
    }

    let contract = match resolve_contract_address(state, &parsed.authority).await {
        Ok(Some(address)) => address,
        Ok(None) => {
            let detail = format!(
                "NeoMist could not resolve `{}` to mainnet contract address.",
                parsed.authority.host
            );
            return text_or_site_error(
                StatusCode::NOT_FOUND,
                wants_html_error_page,
                "No contract address",
                "This web3 target does not resolve to contract address on Ethereum mainnet.",
                Some(&detail),
                local_host,
                &request_path,
                None,
            );
        }
        Err(err) => {
            return text_or_site_error(
                StatusCode::BAD_GATEWAY,
                wants_html_error_page,
                "Address resolution failed",
                "NeoMist could not resolve this web3 target on Ethereum mainnet.",
                Some(&format!("{err:#}")),
                local_host,
                &request_path,
                None,
            );
        }
    };

    match render_web3_content(state.ens_provider.as_ref(), contract, &parsed, parts.method == Method::HEAD).await {
        Ok(response) => response,
        Err(err) => text_or_site_error(
            StatusCode::BAD_GATEWAY,
            wants_html_error_page,
            "Web3 content load failed",
            "NeoMist resolved this web3 target but could not load contract response.",
            Some(&format!("{err:#}")),
            local_host,
            &request_path,
            None,
        ),
    }
}

async fn render_web3_content(
    provider: &DynProvider,
    contract_address: Address,
    request: &ParsedRequest,
    head_only: bool,
) -> Result<Response<Body>> {
    let contract = Web3Site::new(contract_address, provider);

    if contract
        .resolveMode()
        .call()
        .await
        .ok()
        .is_some_and(is_erc5219_mode)
    {
        let params = build_query_params(request.query.as_deref());
        let response = contract
            .request(request.resource.clone(), params)
            .call()
            .await
            .wrap_err("ERC-5219 request() call failed")?;
        let (status_code, body, headers): (u16, String, Vec<KeyValue>) = response.into();
        let status =
            StatusCode::from_u16(status_code).wrap_err("ERC-5219 request() returned invalid status code")?;
        return Ok(build_contract_response(
            status,
            head_only,
            body,
            headers,
            None,
            None,
        ));
    }

    if request.resource.is_empty() {
        let body: String = contract.html().call().await.wrap_err("html() call failed")?;
        return Ok(build_contract_response(
            StatusCode::OK,
            head_only,
            body,
            Vec::new(),
            Some("text/html; charset=utf-8"),
            Some(IMMUTABLE_CACHE_CONTROL),
        ));
    }

    Err(eyre::eyre!(
        "Contract does not expose ERC-5219 request() mode for resource path `{}`",
        request.resource_path
    ))
}

async fn resolve_contract_address(state: &AppState, authority: &ParsedAuthority) -> Result<Option<Address>> {
    if let Ok(address) = Address::from_str(&authority.host) {
        return Ok(Some(address));
    }

    ens::resolve_address(state, &authority.host).await
}

fn build_query_params(query: Option<&str>) -> Vec<KeyValue> {
    let Some(query) = query else {
        return Vec::new();
    };

    form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| KeyValue {
            key: key.into_owned(),
            value: value.into_owned(),
        })
        .collect()
}

fn build_contract_response(
    status: StatusCode,
    head_only: bool,
    body: String,
    headers: Vec<KeyValue>,
    default_content_type: Option<&str>,
    default_cache_control: Option<&str>,
) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    let mut has_content_type = false;
    let mut has_cache_control = false;

    for header in headers {
        let key = header.key.trim();
        let value = header.value.trim();
        let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
            continue;
        };
        if should_drop_response_header(&name) {
            continue;
        }
        let Ok(value) = HeaderValue::from_str(value) else {
            continue;
        };
        if name == CONTENT_TYPE {
            has_content_type = true;
        }
        if name == CACHE_CONTROL {
            has_cache_control = true;
        }
        builder = builder.header(name, value);
    }

    if !has_content_type
        && let Some(content_type) = default_content_type
    {
        builder = builder.header(CONTENT_TYPE, content_type);
    }
    if !has_cache_control
        && let Some(cache_control) = default_cache_control
    {
        builder = builder.header(CACHE_CONTROL, cache_control);
    }

    builder
        .body(if head_only {
            Body::empty()
        } else {
            Body::from(body)
        })
        .unwrap()
}

fn should_drop_response_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str().to_ascii_lowercase().as_str(),
        "connection"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "set-cookie"
            | "set-cookie2"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn is_erc5219_mode(mode: impl AsRef<[u8]>) -> bool {
    let bytes = mode.as_ref();
    bytes.starts_with(b"5219") && bytes[4..].iter().all(|byte| *byte == 0)
}

fn parse_local_request(path: &str, query: Option<&str>) -> Result<RouteAction> {
    if path == WEB3_ROUTE_PREFIX || path == "/web3/" {
        return Ok(RouteAction::Redirect("/".to_string()));
    }

    let tail = path
        .strip_prefix(&format!("{WEB3_ROUTE_PREFIX}/"))
        .ok_or_else(|| eyre::eyre!("Path must begin with `/web3/`"))?;

    if tail.is_empty() {
        return Ok(RouteAction::Redirect("/".to_string()));
    }

    if let Some(location) = decode_protocol_handler_target(tail)? {
        return Ok(RouteAction::Redirect(location));
    }

    parse_canonical_target(tail, query)
}

fn decode_protocol_handler_target(encoded_tail: &str) -> Result<Option<String>> {
    let decoded = percent_decode_str(encoded_tail)
        .decode_utf8()
        .wrap_err("Protocol handler target is not valid UTF-8")?;

    if !decoded.starts_with("web3://") && !decoded.starts_with("web+web3://") {
        return Ok(None);
    }

    let url = Url::parse(&decoded).wrap_err("Protocol handler target is not valid web3 URL")?;
    if !matches!(url.scheme(), "web3" | "web+web3") {
        return Err(eyre::eyre!("Unsupported scheme `{}`", url.scheme()));
    }

    let host = url
        .host_str()
        .ok_or_else(|| eyre::eyre!("web3 URL is missing authority"))?;
    let authority = canonical_authority(host, url.port());
    let mut location = format!("{WEB3_ROUTE_PREFIX}/{authority}");

    let path = url.path();
    if path == "/" || path.is_empty() {
        location.push('/');
    } else {
        location.push_str(path);
    }

    if let Some(query) = url.query() {
        location.push('?');
        location.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        location.push('#');
        location.push_str(fragment);
    }

    Ok(Some(location))
}

fn parse_canonical_target(tail: &str, query: Option<&str>) -> Result<RouteAction> {
    let Some((authority_raw, remainder)) = tail.split_once('/') else {
        return Ok(RouteAction::Redirect(canonical_local_path(
            tail,
            "/",
            query,
            None,
        )));
    };

    if authority_raw.is_empty() {
        return Err(eyre::eyre!("web3 URL authority is empty"));
    }

    let authority = parse_authority(authority_raw)?;
    let resource_path = format!("/{remainder}");
    let resource = decode_resource_segments(remainder)?;

    Ok(RouteAction::Target(ParsedRequest {
        authority,
        resource_path,
        resource,
        query: query.map(str::to_string),
        fragment: None,
    }))
}

fn parse_authority(raw: &str) -> Result<ParsedAuthority> {
    let mut host = raw;
    let mut chain_id = None;

    if let Some((candidate_host, candidate_chain_id)) = raw.rsplit_once(':') {
        if !candidate_host.is_empty() {
            let parsed_chain_id = candidate_chain_id
                .parse::<u64>()
                .wrap_err("web3 URL chain id must be numeric")?;
            host = candidate_host;
            chain_id = Some(parsed_chain_id);
        }
    }

    if host.is_empty() {
        return Err(eyre::eyre!("web3 URL authority is empty"));
    }

    let host_lower = host.to_ascii_lowercase();
    let host_is_supported_name = host_lower.ends_with(".eth") || host_lower.ends_with(".wei");
    if !host_is_supported_name && Address::from_str(host).is_err() {
        return Err(eyre::eyre!(
            "web3 URL authority must be 0x contract address, .eth name, or .wei name"
        ));
    }

    Ok(ParsedAuthority {
        raw: raw.to_string(),
        host: if host_is_supported_name {
            host_lower
        } else {
            host.to_string()
        },
        chain_id,
    })
}

fn decode_resource_segments(remainder: &str) -> Result<Vec<String>> {
    remainder
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            percent_decode_str(segment)
                .decode_utf8()
                .map(|segment| segment.into_owned())
                .map_err(|_| eyre::eyre!("web3 URL path contains invalid UTF-8 segment"))
        })
        .collect()
}

fn canonical_authority(host: &str, chain_id: Option<u16>) -> String {
    match chain_id {
        Some(chain_id) => format!("{host}:{chain_id}"),
        None => host.to_string(),
    }
}

fn canonical_local_path(
    authority: &str,
    resource_path: &str,
    query: Option<&str>,
    fragment: Option<&str>,
) -> String {
    let mut location = format!("{WEB3_ROUTE_PREFIX}/{authority}");
    if resource_path == "/" {
        location.push('/');
    } else {
        location.push_str(resource_path);
    }
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        location.push('?');
        location.push_str(query);
    }
    if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
        location.push('#');
        location.push_str(fragment);
    }
    location
}

fn redirect_response(location: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(LOCATION, location)
        .body(Body::empty())
        .unwrap()
}

fn text_or_site_error(
    status: StatusCode,
    wants_html: bool,
    title: &str,
    summary: &str,
    detail: Option<&str>,
    host: &str,
    path: &str,
    extra_header: Option<(HeaderName, &'static str)>,
) -> Response<Body> {
    if wants_html {
        let mut response = site_error::build_site_error_response(status, true, title, summary, detail, host, path);
        if let Some((name, value)) = extra_header {
            response.headers_mut().insert(name, HeaderValue::from_static(value));
        }
        return response;
    }

    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CACHE_CONTROL, "no-store");
    if let Some((name, value)) = extra_header {
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from(detail.unwrap_or(summary).to_string()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::{ParsedAuthority, RouteAction, decode_protocol_handler_target, parse_local_request};

    #[test]
    fn decodes_protocol_handler_target_into_canonical_local_path() {
        let location = decode_protocol_handler_target(
            "web%2Bweb3%3A%2F%2F0x000000000a4a4f895734cf70700b6f84aadbca6c%2Fapp.js%3Fv%3D2%23frag",
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            location,
            "/web3/0x000000000a4a4f895734cf70700b6f84aadbca6c/app.js?v=2#frag"
        );
    }

    #[test]
    fn adds_trailing_slash_for_root_contract_target() {
        let action = parse_local_request(
            "/web3/0x000000000a4a4f895734cf70700b6f84aadbca6c",
            Some("x=1"),
        )
        .unwrap();

        assert_eq!(
            action,
            RouteAction::Redirect(
                "/web3/0x000000000a4a4f895734cf70700b6f84aadbca6c/?x=1".to_string(),
            )
        );
    }

    #[test]
    fn parses_canonical_target_segments_and_query() {
        let action = parse_local_request(
            "/web3/vitalik.eth:1/assets/app.js",
            Some("v=2&lang=en"),
        )
        .unwrap();

        assert_eq!(
            action,
            RouteAction::Target(super::ParsedRequest {
                authority: ParsedAuthority {
                    raw: "vitalik.eth:1".to_string(),
                    host: "vitalik.eth".to_string(),
                    chain_id: Some(1),
                },
                resource_path: "/assets/app.js".to_string(),
                resource: vec!["assets".to_string(), "app.js".to_string()],
                query: Some("v=2&lang=en".to_string()),
                fragment: None,
            })
        );
    }

    #[test]
    fn rejects_unsupported_authority_shape() {
        let err = parse_local_request("/web3/not-a-contract/", None).unwrap_err();
        assert!(
            err.to_string()
                .contains("web3 URL authority must be 0x contract address, .eth name, or .wei name")
        );
    }
}
