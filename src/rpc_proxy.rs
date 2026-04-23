use axum::Router;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::{any, post};
use hyper::body::Bytes;
use reqwest::StatusCode;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::state::AppState;

pub async fn run_internal_proxy(state: AppState) -> eyre::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let app = Router::new()
        .route("/execution", post(execution_proxy_handler))
        .route("/execution/*path", post(execution_proxy_handler))
        .route("/consensus", any(consensus_proxy_handler))
        .route("/consensus/*path", any(consensus_proxy_handler))
        .with_state(state);

    info!("Starting internal RPC proxy on 127.0.0.1:{port}");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!("Internal RPC proxy failed: {}", e);
        }
    });

    Ok(port)
}

async fn execution_proxy_handler(State(state): State<AppState>, body: Bytes) -> Response {
    let rpcs = {
        let config = state.config.read().await;
        config.execution_rpcs.clone()
    };

    if rpcs.is_empty() {
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(axum::body::Body::from("No execution RPCs configured"))
            .unwrap();
    }

    for rpc in rpcs {
        let req = state
            .http_client
            .post(&rpc)
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()
            .await;

        if let Ok(res) = req {
            if res.status().is_success() {
                let status = res.status();
                let bytes = res.bytes().await.unwrap_or_default();
                return Response::builder()
                    .status(status)
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(bytes))
                    .unwrap();
            } else {
                warn!("Execution proxy: {} returned status {}", rpc, res.status());
            }
        } else if let Err(e) = req {
            warn!("Execution proxy: request to {} failed: {}", rpc, e);
        }
    }

    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(axum::body::Body::from(
            "All configured execution RPCs failed",
        ))
        .unwrap()
}

async fn consensus_proxy_handler(State(state): State<AppState>, req: Request) -> Response {
    let rpcs = {
        let config = state.config.read().await;
        config.consensus_rpcs.clone()
    };

    if rpcs.is_empty() {
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(axum::body::Body::from("No consensus RPCs configured"))
            .unwrap();
    }

    let method = req.method().clone();
    let original_uri = req.uri().clone();
    let original_path = original_uri.path();
    let proxy_path = original_path
        .strip_prefix("/consensus")
        .unwrap_or(original_path);
    let query = original_uri
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let path_query = format!("{}{}", proxy_path, query);
    let req_headers = req.headers().clone();

    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => Bytes::new(),
    };

    for rpc in rpcs {
        let url = format!("{}{}", rpc.trim_end_matches('/'), path_query);
        let mut builder = state.http_client.request(method.clone(), &url);

        for (k, v) in req_headers.iter() {
            if k != hyper::header::HOST {
                builder = builder.header(k, v.clone());
            }
        }

        let client_req = builder.body(body_bytes.clone()).send().await;

        if let Ok(res) = client_req {
            if res.status().is_success() {
                let status = res.status();
                let res_headers = res.headers().clone();
                let bytes = res.bytes().await.unwrap_or_default();

                let mut response_builder = Response::builder().status(status);
                for (k, v) in res_headers.iter() {
                    response_builder = response_builder.header(k, v.clone());
                }

                return response_builder
                    .body(axum::body::Body::from(bytes))
                    .unwrap_or_else(|_| {
                        Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(axum::body::Body::empty())
                            .unwrap()
                    });
            } else {
                warn!("Consensus proxy: {} returned status {}", rpc, res.status());
            }
        } else if let Err(e) = client_req {
            warn!("Consensus proxy: request to {} failed: {}", rpc, e);
        }
    }

    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(axum::body::Body::from(
            "All configured consensus RPCs failed",
        ))
        .unwrap()
}
