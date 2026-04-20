use axum::{extract::Request, response::Response, routing::any, Router};
use std::net::SocketAddr;
use tokio::net::TcpListener;

async fn handler(req: Request) -> Response {
    let method = req.method().clone();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024).await.unwrap();
    Response::new(axum::body::Body::from("OK"))
}

fn main() {}
