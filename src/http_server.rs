use std::convert::Infallible;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{
    HeaderMap, HeaderValue, Request, Response, StatusCode,
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
        ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_HEADERS, CACHE_CONTROL, CONTENT_TYPE,
        HOST, ORIGIN, REFERER, VARY,
    },
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
use crate::site_error;
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

struct ProxySiteErrorContext {
    host: String,
    path: String,
    title: &'static str,
    summary: &'static str,
}

pub async fn run_https_server(state: AppState, certs: std::sync::Arc<CertManager>) -> Result<()> {
    info!(
        "Starting local-only HTTPS server for node.localhost, neomist.localhost, ipfs.localhost, and *.ipfs.localhost"
    );
    let eth_router = Router::new()
        .route("/rpc", post(proxy_rpc).options(proxy_rpc_preflight))
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
                        let wants_html_error_page = site_error::prefers_html_error_page(req.headers());
                        let request_path = req.uri().path().to_string();
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
                                Err(_) => Ok(site_error::build_site_error_response(
                                    StatusCode::BAD_GATEWAY,
                                    wants_html_error_page,
                                    "Gateway routing failed",
                                    "NeoMist could not route this IPFS request to local Kubo gateway.",
                                    Some("IPFS gateway routing error"),
                                    host_only,
                                    &request_path,
                                )),
                            }
                        } else if is_ens_host(host_only) {
                            match ens_router.oneshot(req).await {
                                Ok(resp) => Ok::<_, Infallible>(resp),
                                Err(_) => Ok(site_error::build_site_error_response(
                                    StatusCode::BAD_GATEWAY,
                                    wants_html_error_page,
                                    "Content routing failed",
                                    "NeoMist could not route this .eth or .wei request to local content loader.",
                                    Some("ENS routing error"),
                                    host_only,
                                    &request_path,
                                )),
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

fn url_header_matches_neomist_ui(value: &HeaderValue) -> bool {
    url_header_matches_host(value, is_neomist_ui_host)
}

fn url_header_matches_rpc_origin(value: &HeaderValue) -> bool {
    url_header_matches_host(value, is_allowed_rpc_origin_host)
}

fn url_header_matches_host(value: &HeaderValue, host_matches: fn(&str) -> bool) -> bool {
    let raw = match value.to_str() {
        Ok(raw) => raw,
        Err(_) => return false,
    };
    let parsed = match url::Url::parse(raw) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };

    parsed.scheme() == "https"
        && parsed.host_str().map(host_matches).unwrap_or(false)
}

fn is_neomist_ui_host(host: &str) -> bool {
    host.eq_ignore_ascii_case(NEOMIST_UI_HOST)
}

fn is_allowed_rpc_origin_host(host: &str) -> bool {
    is_neomist_ui_host(host) || is_ens_host(host)
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
        None,
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
    let request_path = parts.uri.path().to_string();

    let mut url = format!(
        "http://{host_only}:{}{}",
        state.ipfs_gateway_port,
        request_path
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
        Some(ProxySiteErrorContext {
            host: host_only.to_string(),
            path: request_path,
            title: "Content load failed",
            summary: "NeoMist could not load content from local Kubo gateway.",
        }),
    )
    .await
}

async fn proxy_ipfs_request(
    parts: axum::http::request::Parts,
    body: Body,
    url: String,
    request_error: &'static str,
    response_error: &'static str,
    error_page: Option<ProxySiteErrorContext>,
) -> Response<Body> {
    let wants_html_error_page = error_page
        .as_ref()
        .map(|_| site_error::prefers_html_error_page(&parts.headers))
        .unwrap_or(false);
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
            return build_proxy_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "IPFS proxy client unavailable",
                error_page.as_ref(),
                wants_html_error_page,
                Some("IPFS proxy client unavailable"),
            );
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
        Err(err) => {
            let detail = format!("{request_error}: {err}");
            return build_proxy_error_response(
                StatusCode::BAD_GATEWAY,
                request_error,
                error_page.as_ref(),
                wants_html_error_page,
                Some(&detail),
            );
        }
    };

    let status = upstream.status();
    let upstream_content_type = upstream
        .headers()
        .get(CONTENT_TYPE)
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
                let (title, summary) = ipfs_gateway_error_copy(status);
                let error_page_override = error_page.as_ref().map(|error_page| ProxySiteErrorContext {
                    host: error_page.host.clone(),
                    path: error_page.path.clone(),
                    title,
                    summary,
                });
                return build_proxy_error_response(
                    status,
                    response_error,
                    error_page_override.as_ref(),
                    true,
                    detail.as_deref(),
                );
            }

            let mut builder = Response::builder().status(status);
            for (name, value) in upstream_headers {
                builder = builder.header(name, value);
            }
            builder.body(Body::from(bytes)).unwrap()
        }
        Err(err) => {
            let detail = format!("{response_error}: {err}");
            build_proxy_error_response(
                StatusCode::BAD_GATEWAY,
                response_error,
                error_page.as_ref(),
                wants_html_error_page,
                Some(&detail),
            )
        }
    }
}

fn ipfs_gateway_error_copy(status: StatusCode) -> (&'static str, &'static str) {
    if status == StatusCode::NOT_FOUND {
        (
            "Page not found",
            "Requested IPFS path was not found in content served through local Kubo gateway.",
        )
    } else if status.is_server_error() {
        (
            "Kubo gateway error",
            "Local Kubo gateway failed before requested IPFS content could load.",
        )
    } else {
        (
            "Content load failed",
            "Requested IPFS content could not be loaded from local Kubo gateway.",
        )
    }
}

fn build_proxy_error_response(
    status: StatusCode,
    fallback_message: &'static str,
    error_page: Option<&ProxySiteErrorContext>,
    wants_html_error_page: bool,
    detail: Option<&str>,
) -> Response<Body> {
    if let Some(error_page) = error_page.filter(|_| wants_html_error_page) {
        return site_error::build_site_error_response(
            status,
            true,
            error_page.title,
            error_page.summary,
            detail,
            &error_page.host,
            &error_page.path,
        );
    }

    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CACHE_CONTROL, "no-store")
        .body(Body::from(
            detail.unwrap_or(fallback_message).to_string(),
        ))
        .unwrap()
}

fn host_without_port(host: &str) -> &str {
    host.split(':').next().unwrap_or(host)
}

fn is_ens_host(host: &str) -> bool {
    host.ends_with(".eth.localhost")
        || host.ends_with(".wei.localhost")
        || host.ends_with(".eth")
        || host.ends_with(".wei")
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
    headers: HeaderMap,
    body_bytes: Bytes,
) -> Response<Body> {
    let cors_origin = match rpc_cors_origin(&headers) {
        Ok(cors_origin) => cors_origin,
        Err(response) => return response,
    };

    let response = match state
        .http_client
        .post(&state.helios_rpc_url)
        .header("content-type", "application/json")
        .body(body_bytes)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return build_rpc_response(
                StatusCode::BAD_GATEWAY,
                Body::from("Failed to reach local Helios RPC"),
                None,
                cors_origin.as_ref(),
            );
        }
    };

    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .cloned();

    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return build_rpc_response(
                StatusCode::BAD_GATEWAY,
                Body::from("Failed to read local Helios RPC response"),
                None,
                cors_origin.as_ref(),
            );
        }
    };

    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }

    let Ok(mut response) = builder.body(Body::from(bytes)) else {
        return build_rpc_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            Body::from("Failed to build RPC response"),
            None,
            cors_origin.as_ref(),
        );
    };

    if let Some(origin) = cors_origin.as_ref() {
        add_rpc_cors_headers(response.headers_mut(), origin);
    }

    response
}

async fn proxy_rpc_preflight(headers: HeaderMap) -> Response<Body> {
    let Some(origin) = headers.get(ORIGIN) else {
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Body::from("Forbidden"))
            .unwrap();
    };

    if !url_header_matches_rpc_origin(origin) {
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Body::from("Forbidden"))
            .unwrap();
    }

    let mut response = Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap();
    let response_headers = response.headers_mut();
    add_rpc_cors_headers(response_headers, origin);
    response_headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, OPTIONS"),
    );

    if let Some(request_headers) = headers.get(ACCESS_CONTROL_REQUEST_HEADERS) {
        response_headers.insert(ACCESS_CONTROL_ALLOW_HEADERS, request_headers.clone());
        response_headers.append(VARY, HeaderValue::from_static("Access-Control-Request-Headers"));
    } else {
        response_headers.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type"),
        );
    }

    response
}

fn rpc_cors_origin(headers: &HeaderMap) -> std::result::Result<Option<HeaderValue>, Response<Body>> {
    let Some(origin) = headers.get(ORIGIN) else {
        return Ok(None);
    };

    if url_header_matches_rpc_origin(origin) {
        Ok(Some(origin.clone()))
    } else {
        Err(Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Body::from("Forbidden"))
            .unwrap())
    }
}

fn add_rpc_cors_headers(headers: &mut HeaderMap, origin: &HeaderValue) {
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    headers.append(VARY, HeaderValue::from_static("Origin"));
}

fn build_rpc_response(
    status: StatusCode,
    body: Body,
    content_type: Option<HeaderValue>,
    cors_origin: Option<&HeaderValue>,
) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }

    let mut response = builder.body(body).unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::empty())
            .unwrap()
    });

    if let Some(origin) = cors_origin {
        add_rpc_cors_headers(response.headers_mut(), origin);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::is_loopback_peer;
    use super::request_origin_matches_neomist_ui;
    use super::url_header_matches_rpc_origin;
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
    fn accepts_eth_origin_for_rpc() {
        assert!(url_header_matches_rpc_origin(&HeaderValue::from_static(
            "https://evmnow.eth",
        )));
    }

    #[test]
    fn accepts_eth_localhost_origin_for_rpc() {
        assert!(url_header_matches_rpc_origin(&HeaderValue::from_static(
            "https://evmnow.eth.localhost",
        )));
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
