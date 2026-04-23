use std::convert::Infallible;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{
    HeaderMap, Request, Response, StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE, HOST, ORIGIN, REFERER},
};
use axum::response::{IntoResponse, Json};
use axum::routing::{any, get, post};
use eyre::{Result, WrapErr};
use hyper_util::rt::TokioIo;
use include_dir::{Dir, File, include_dir};
use mime_guess::from_path;
use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::{ServerConfig, sign::CertifiedKey};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tracing::{error, info, warn};

use crate::app_setup;
use crate::cache;
use crate::certs::CertManager;
use crate::config::{AppConfig, save_config};
use crate::ens;
use crate::state::AppState;

const PRIMARY_HTTPS_PORT: u16 = 443;
const NEOMIST_UI_HOST: &str = "neomist.localhost";
static UI_DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/ui/dist");
static IPFS_PROXY_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static HELIOS_VERSION: OnceLock<String> = OnceLock::new();

#[derive(Debug, serde::Serialize)]
struct SaveResponse {
    success: bool,
    error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct AboutResponse {
    neomist: ComponentVersion,
    helios: ComponentVersion,
    kubo: KuboVersion,
}

#[derive(Debug, serde::Serialize)]
struct ComponentVersion {
    version: String,
}

#[derive(Debug, serde::Serialize)]
struct KuboVersion {
    mode: String,
    version: String,
}

#[derive(Debug, serde::Deserialize)]
struct IpfsVersionResponse {
    #[serde(rename = "Version")]
    version: String,
}

pub async fn run_https_server(state: AppState, certs: std::sync::Arc<CertManager>) -> Result<()> {
    info!(
        "Starting local-only HTTPS server for node.localhost, neomist.localhost, ipfs.localhost, and *.ipfs.localhost"
    );
    let eth_router = Router::new()
        .route("/rpc", post(proxy_rpc))
        .route("/health", get(healthcheck))
        .route("/api/about", get(get_about))
        .route("/api/cached-domains", get(get_cached_domains))
        .route("/api/total-storage", get(get_total_storage))
        .route("/api/toggle-auto-seed", post(toggle_auto_seed))
        .route("/api/clear-cache", post(clear_cache))
        .route("/api/helios/checkpoints", get(get_checkpoints))
        .route("/api/config", get(get_config).post(save_config_handler))
        .route("/", get(serve_ui))
        .route("/*path", get(serve_ui))
        .with_state(state.clone());

    let ens_router = Router::new()
        .route("/", any(ens_lookup))
        .route("/*path", any(ens_lookup))
        .with_state(state.clone());

    let ipfs_api_router = Router::new()
        .route("/", any(proxy_ipfs_api))
        .route("/*path", any(proxy_ipfs_api))
        .with_state(state.clone());

    let ipfs_gateway_router = Router::new()
        .route("/", any(proxy_ipfs_gateway))
        .route("/*path", any(proxy_ipfs_gateway))
        .with_state(state.clone());

    let listeners = bind_https_sockets(PRIMARY_HTTPS_PORT).await;
    if listeners.is_empty() {
        return Err(eyre::eyre!(
            "Failed to bind any HTTPS listener on port {PRIMARY_HTTPS_PORT}. NeoMist requires port {PRIMARY_HTTPS_PORT} for local HTTPS. Make sure no other service is already using the port."
        ));
    }

    let tls_config = build_tls_config(certs)?;
    let acceptor = TlsAcceptor::from(std::sync::Arc::new(tls_config));
    let mut listener_tasks = tokio::task::JoinSet::new();

    for listener in listeners {
        let local_addr = listener
            .local_addr()
            .wrap_err("Failed to inspect HTTPS listener address")?;
        info!("HTTPS server listening on {local_addr}");

        listener_tasks.spawn(run_https_listener(
            listener,
            acceptor.clone(),
            eth_router.clone(),
            ens_router.clone(),
            ipfs_api_router.clone(),
            ipfs_gateway_router.clone(),
        ));
    }

    while let Some(result) = listener_tasks.join_next().await {
        match result {
            Ok(Ok(())) => warn!("HTTPS listener exited unexpectedly"),
            Ok(Err(err)) => warn!("HTTPS listener error: {err}"),
            Err(err) => warn!("HTTPS listener task failed: {err}"),
        }
    }

    Err(eyre::eyre!("All HTTPS listeners exited"))
}

async fn bind_https_sockets(port: u16) -> Vec<TcpListener> {
    let mut listeners = Vec::new();

    for addr in [
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
    ] {
        match TcpListener::bind(addr).await {
            Ok(listener) => listeners.push(listener),
            Err(err) if !listeners.is_empty() && err.kind() == std::io::ErrorKind::AddrInUse => {}
            Err(err) => warn!("Failed to bind HTTPS listener on {addr}: {err}"),
        }
    }

    listeners
}

async fn run_https_listener(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    eth_router: Router,
    ens_router: Router,
    ipfs_api_router: Router,
    ipfs_gateway_router: Router,
) -> Result<()> {
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .wrap_err("Failed to accept connection")?;

        if !is_loopback_peer(peer) {
            warn!("Rejected non-loopback HTTPS connection from {peer}");
            continue;
        }

        let acceptor = acceptor.clone();
        let eth_router = eth_router.clone();
        let ens_router = ens_router.clone();
        let ipfs_api_router = ipfs_api_router.clone();
        let ipfs_gateway_router = ipfs_gateway_router.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(err) => {
                    warn!("TLS accept error: {err}");
                    return;
                }
            };

            let service =
                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let eth_router = eth_router.clone();
                    let ens_router = ens_router.clone();
                    let ipfs_api_router = ipfs_api_router.clone();
                    let ipfs_gateway_router = ipfs_gateway_router.clone();

                    async move {
                        let req = req.map(Body::new);
                        let host = req
                            .headers()
                            .get(HOST)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_lowercase();
                        let host_only = host_without_port(&host);

                        if host_only == "neomist.localhost" {
                            match eth_router.oneshot(req).await {
                                Ok(resp) => Ok::<_, Infallible>(resp),
                                Err(_) => Ok(Response::builder()
                                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                                    .body(Body::from("UI routing error"))
                                    .unwrap()),
                            }
                        } else if host_only == "ipfs.localhost" {
                            match ipfs_api_router.oneshot(req).await {
                                Ok(resp) => Ok::<_, Infallible>(resp),
                                Err(_) => Ok(Response::builder()
                                    .status(StatusCode::BAD_GATEWAY)
                                    .body(Body::from("IPFS API routing error"))
                                    .unwrap()),
                            }
                        } else if is_ipfs_gateway_host(host_only) {
                            match ipfs_gateway_router.oneshot(req).await {
                                Ok(resp) => Ok::<_, Infallible>(resp),
                                Err(_) => Ok(Response::builder()
                                    .status(StatusCode::BAD_GATEWAY)
                                    .body(Body::from("IPFS gateway routing error"))
                                    .unwrap()),
                            }
                        } else if host_only.ends_with(".eth.localhost")
                            || host_only.ends_with(".wei.localhost")
                            || host_only.ends_with(".eth")
                            || host_only.ends_with(".wei")
                        {
                            match ens_router.oneshot(req).await {
                                Ok(resp) => Ok::<_, Infallible>(resp),
                                Err(_) => Ok(Response::builder()
                                    .status(StatusCode::BAD_GATEWAY)
                                    .body(Body::from("ENS routing error"))
                                    .unwrap()),
                            }
                        } else {
                            Ok(Response::builder()
                                .status(StatusCode::NOT_FOUND)
                                .body(Body::from("Unknown host"))
                                .unwrap())
                        }
                    }
                });

            let io = TokioIo::new(tls_stream);
            if let Err(err) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                warn!("HTTPS connection error: {err}");
            }
        });
    }
}

fn is_loopback_peer(peer: SocketAddr) -> bool {
    peer.ip().is_loopback() || peer.ip().to_canonical().is_loopback()
}

fn require_neomist_ui_request(headers: &HeaderMap) -> std::result::Result<(), Response<Body>> {
    if request_origin_matches_neomist_ui(headers) {
        return Ok(());
    }

    Err(Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Body::from("Forbidden"))
        .unwrap())
}

fn request_origin_matches_neomist_ui(headers: &HeaderMap) -> bool {
    if let Some(origin) = headers.get(ORIGIN) {
        return url_header_matches_neomist_ui(origin);
    }

    if let Some(referer) = headers.get(REFERER) {
        return url_header_matches_neomist_ui(referer);
    }

    false
}

fn url_header_matches_neomist_ui(value: &axum::http::HeaderValue) -> bool {
    let raw = match value.to_str() {
        Ok(raw) => raw,
        Err(_) => return false,
    };
    let parsed = match url::Url::parse(raw) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };

    parsed.scheme() == "https"
        && parsed
            .host_str()
            .map(|host| host.eq_ignore_ascii_case(NEOMIST_UI_HOST))
            .unwrap_or(false)
}

async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    Json(config)
}

async fn save_config_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut new_config): Json<AppConfig>,
) -> Response<Body> {
    if let Err(response) = require_neomist_ui_request(&headers) {
        return response;
    }

    let current_config = state.config.read().await.clone();

    // Preserve internal state flags from current config.
    new_config.dns_setup_attempted = current_config.dns_setup_attempted;
    new_config.dns_setup_installed = current_config.dns_setup_installed;

    let start_on_login_changed = new_config.start_on_login != current_config.start_on_login;
    if start_on_login_changed {
        if let Err(err) = app_setup::sync_start_on_login(new_config.start_on_login) {
            error!("Failed to update start-on-login setting: {err}");
            return Json(SaveResponse {
                success: false,
                error: Some(err.to_string()),
            })
            .into_response();
        }
    }

    match save_config(&state.config_path, &new_config) {
        Ok(_) => {
            state
                .tray_state
                .set_show_gas_price(new_config.show_tray_gas_price);
            let mut config_guard = state.config.write().await;
            *config_guard = new_config.clone();
            Json(SaveResponse {
                success: true,
                error: None,
            })
            .into_response()
        }
        Err(err) => {
            if start_on_login_changed {
                if let Err(revert_err) =
                    app_setup::sync_start_on_login(current_config.start_on_login)
                {
                    error!(
                        "Failed to revert start-on-login setting after config save failure: {revert_err}"
                    );
                }
            }
            error!("Failed to save config: {err}");
            Json(SaveResponse {
                success: false,
                error: Some(err.to_string()),
            })
            .into_response()
        }
    }
}

async fn serve_ui(req: Request<Body>) -> Response<Body> {
    let path = req.uri().path();
    if path.starts_with("/api/") {
        return not_found();
    }

    let asset_path = path.trim_start_matches('/');
    if asset_path.is_empty() {
        return asset_response("index.html");
    }

    if let Some(file) = UI_DIST.get_file(asset_path) {
        return file_response(asset_path, file);
    }

    if asset_path.starts_with("assets/")
        || asset_path.ends_with(".js")
        || asset_path.ends_with(".css")
    {
        return not_found();
    }

    asset_response("index.html")
}

fn asset_response(path: &str) -> Response<Body> {
    match UI_DIST.get_file(path) {
        Some(file) => file_response(path, file),
        None => not_found(),
    }
}

fn file_response(path: &str, file: &File) -> Response<Body> {
    let mime = from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, mime.as_ref())
        .header(CACHE_CONTROL, cache)
        .body(Body::from(file.contents().to_vec()))
        .unwrap()
}

fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not found"))
        .unwrap()
}

async fn healthcheck() -> impl IntoResponse {
    StatusCode::OK
}

async fn get_cached_domains(State(state): State<AppState>) -> Response<Body> {
    match cache::list_cached_domains(&state).await {
        Ok(domains) => Json(domains).into_response(),
        Err(err) => {
            error!("Failed to list cached domains: {err}");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Failed to list cached domains"))
                .unwrap()
        }
    }
}

async fn get_total_storage(State(state): State<AppState>) -> Response<Body> {
    match cache::total_storage_used(&state).await {
        Ok(total) => Json(serde_json::json!({
            "totalUsed": format_bytes(total)
        }))
        .into_response(),
        Err(err) => {
            error!("Failed to get total storage: {err}");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Failed to get total storage"))
                .unwrap()
        }
    }
}

async fn toggle_auto_seed(State(state): State<AppState>, req: Request<Body>) -> Response<Body> {
    if let Err(response) = require_neomist_ui_request(req.headers()) {
        return response;
    }

    let query = req.uri().query().unwrap_or("");
    let params: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
    let domain = match params.get("domain") {
        Some(domain) => domain,
        None => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("domain is required"))
                .unwrap();
        }
    };
    let enable = params
        .get("enable")
        .map(|value| value == "true")
        .unwrap_or(false);

    match cache::toggle_autoseed(&state, domain, enable).await {
        Ok(()) => Json(serde_json::json!({ "success": true })).into_response(),
        Err(err) => {
            error!("Failed to toggle auto-seed: {err}");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Failed to toggle auto-seed"))
                .unwrap()
        }
    }
}

async fn clear_cache(State(state): State<AppState>, req: Request<Body>) -> Response<Body> {
    if let Err(response) = require_neomist_ui_request(req.headers()) {
        return response;
    }

    let query = req.uri().query().unwrap_or("");
    let params: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
    let domain = match params.get("domain") {
        Some(domain) => domain,
        None => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("domain is required"))
                .unwrap();
        }
    };
    let version = params.get("version").map(String::as_str);

    match cache::clear_cache(&state, domain, version).await {
        Ok(()) => Json(serde_json::json!({ "success": true })).into_response(),
        Err(err) => {
            error!("Failed to clear cache: {err}");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Failed to clear cache"))
                .unwrap()
        }
    }
}

async fn get_checkpoints(State(state): State<AppState>) -> Response<Body> {
    let guard = state.checkpoint_history.read().await;
    let checkpoints: Vec<String> = guard.iter().cloned().collect();
    Json(serde_json::json!({ "checkpoints": checkpoints })).into_response()
}

async fn get_about(State(state): State<AppState>) -> Response<Body> {
    let kubo_version = fetch_kubo_version(&state).await.unwrap_or_else(|| {
        if state.managed_ipfs {
            crate::ipfs::bundled_kubo_version().to_string()
        } else {
            "unknown".to_string()
        }
    });

    Json(AboutResponse {
        neomist: ComponentVersion {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        helios: ComponentVersion {
            version: helios_version().to_string(),
        },
        kubo: KuboVersion {
            mode: if state.managed_ipfs {
                "managed".to_string()
            } else {
                "external".to_string()
            },
            version: kubo_version,
        },
    })
    .into_response()
}

fn helios_version() -> &'static str {
    HELIOS_VERSION
        .get_or_init(|| {
            locked_dependency_version("helios").unwrap_or_else(|| "unknown".to_string())
        })
        .as_str()
}

fn locked_dependency_version(package: &str) -> Option<String> {
    let lockfile = include_str!("../Cargo.lock");
    let marker = format!("name = \"{package}\"\nversion = \"");
    let start = lockfile.find(&marker)? + marker.len();
    let end = lockfile[start..].find('"')? + start;
    Some(lockfile[start..end].to_string())
}

async fn fetch_kubo_version(state: &AppState) -> Option<String> {
    let response = state
        .http_client
        .post(format!(
            "{}/api/v0/version",
            state.ipfs_api_url.trim_end_matches('/')
        ))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }

    response
        .json::<IpfsVersionResponse>()
        .await
        .ok()
        .map(|body| body.version)
}

async fn ens_lookup(State(state): State<AppState>, request: Request<Body>) -> impl IntoResponse {
    ens::proxy_request(&state, request).await
}

async fn proxy_ipfs_api(State(state): State<AppState>, request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    if parts.uri.path() == "/webui" {
        return Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header("location", "/webui/")
            .body(Body::empty())
            .unwrap();
    }

    let mut url = format!(
        "{}{}",
        state.ipfs_api_url.trim_end_matches('/'),
        parts.uri.path()
    );
    if let Some(query) = parts.uri.query() {
        url.push('?');
        url.push_str(query);
    }

    proxy_ipfs_request(
        parts,
        body,
        url,
        "IPFS request failed",
        "IPFS response failed",
    )
    .await
}

async fn proxy_ipfs_gateway(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let host = parts
        .headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let host_only = host_without_port(&host);

    let mut url = format!(
        "http://{host_only}:{}{}",
        state.ipfs_gateway_port,
        parts.uri.path()
    );
    if let Some(query) = parts.uri.query() {
        url.push('?');
        url.push_str(query);
    }

    proxy_ipfs_request(
        parts,
        body,
        url,
        "IPFS gateway request failed",
        "IPFS gateway response failed",
    )
    .await
}

async fn proxy_ipfs_request(
    parts: axum::http::request::Parts,
    body: Body,
    url: String,
    request_error: &'static str,
    response_error: &'static str,
) -> Response<Body> {
    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("Invalid request body"))
                .unwrap();
        }
    };

    let proxy_client = match ipfs_proxy_client() {
        Some(client) => client,
        None => {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("IPFS proxy client unavailable"))
                .unwrap();
        }
    };

    let mut req_builder = proxy_client.request(parts.method.clone(), url);
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
                .body(Body::from(request_error))
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
            .body(Body::from(response_error))
            .unwrap(),
    }
}

fn host_without_port(host: &str) -> &str {
    host.split(':').next().unwrap_or(host)
}

fn is_ipfs_gateway_host(host: &str) -> bool {
    let Some(prefix) = host.strip_suffix(".ipfs.localhost") else {
        return false;
    };
    !prefix.is_empty() && !prefix.contains('.')
}

fn ipfs_proxy_client() -> Option<&'static reqwest::Client> {
    if let Some(client) = IPFS_PROXY_CLIENT.get() {
        return Some(client);
    }

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let _ = IPFS_PROXY_CLIENT.set(client);
    IPFS_PROXY_CLIENT.get()
}

async fn proxy_rpc(
    State(state): State<AppState>,
    body_bytes: Bytes,
) -> Result<Response<Body>, StatusCode> {
    let response = state
        .http_client
        .post(&state.helios_rpc_url)
        .header("content-type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let bytes = response
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }

    builder
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::is_loopback_peer;
    use super::request_origin_matches_neomist_ui;
    use axum::http::{
        HeaderMap, HeaderValue,
        header::{ORIGIN, REFERER},
    };
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn accepts_ipv4_mapped_ipv6_loopback_peer() {
        let peer = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::from_bits(
                0x0000_0000_0000_0000_0000_ffff_7f00_0001,
            )),
            443,
        );

        assert!(is_loopback_peer(peer));
    }

    #[test]
    fn rejects_non_loopback_peer() {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 443);

        assert!(!is_loopback_peer(peer));
    }

    #[test]
    fn accepts_neomist_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ORIGIN,
            HeaderValue::from_static("https://neomist.localhost"),
        );

        assert!(request_origin_matches_neomist_ui(&headers));
    }

    #[test]
    fn accepts_neomist_origin_with_port() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ORIGIN,
            HeaderValue::from_static("https://neomist.localhost:8443"),
        );

        assert!(request_origin_matches_neomist_ui(&headers));
    }

    #[test]
    fn accepts_neomist_referer_when_origin_is_missing() {
        let mut headers = HeaderMap::new();
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://neomist.localhost/settings"),
        );

        assert!(request_origin_matches_neomist_ui(&headers));
    }

    #[test]
    fn rejects_cross_site_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("https://evil.example"));

        assert!(!request_origin_matches_neomist_ui(&headers));
    }

    #[test]
    fn rejects_missing_browser_context_headers() {
        assert!(!request_origin_matches_neomist_ui(&HeaderMap::new()));
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let k = 1024f64;
    let sizes = ["B", "KB", "MB", "GB", "TB"];
    let i = (bytes as f64).log(k).floor() as usize;
    let value = (bytes as f64) / k.powi(i as i32);
    format!("{:.2} {}", value, sizes[i])
}

fn build_tls_config(certs: std::sync::Arc<CertManager>) -> Result<ServerConfig> {
    let resolver = CertResolver { certs };
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(std::sync::Arc::new(resolver));
    Ok(config)
}

#[derive(Debug)]
struct CertResolver {
    certs: std::sync::Arc<CertManager>,
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<std::sync::Arc<CertifiedKey>> {
        let server_name = client_hello.server_name()?;
        let (cert_chain, key) = self.certs.get_chain_for_host(server_name).ok()?;
        any_supported_type(&key)
            .ok()
            .map(|signing_key| std::sync::Arc::new(CertifiedKey::new(cert_chain, signing_key)))
    }
}
