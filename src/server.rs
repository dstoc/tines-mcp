use std::io;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderValue, StatusCode, header::CACHE_CONTROL},
    middleware::{Next, from_fn},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::read_tools::ReadTools;
use crate::tines::TinesClient;

const MAX_MCP_REQUEST_BYTES: usize = 64 * 1024;

pub fn router(config: &Config, cancellation_token: CancellationToken) -> Router {
    router_with_client(config, None, cancellation_token)
}

pub fn router_with_client(
    config: &Config,
    tines: Option<TinesClient>,
    cancellation_token: CancellationToken,
) -> Router {
    let server_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(config.allowed_hosts.clone())
        .with_allowed_origins(config.allowed_origins.clone())
        .enforce_origin_validation()
        .with_cancellation_token(cancellation_token);
    let mcp = StreamableHttpService::new(
        move || Ok(ReadTools::new(tines.clone())),
        LocalSessionManager::default().into(),
        server_config,
    );

    Router::new()
        .route("/healthz", get(healthz))
        .nest_service("/mcp", mcp)
        .layer(from_fn(limit_mcp_request_body))
}

async fn limit_mcp_request_body(request: Request, next: Next) -> Response {
    if request.uri().path() != "/mcp" {
        return next.run(request).await;
    }

    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, MAX_MCP_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            let mut response = StatusCode::PAYLOAD_TOO_LARGE.into_response();
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return response;
        }
    };
    let mut response = next.run(Request::from_parts(parts, Body::from(body))).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn run(
    listener: TcpListener,
    config: &Config,
    cancellation_token: CancellationToken,
) -> io::Result<()> {
    let tines = config
        .tines
        .as_ref()
        .map(TinesClient::new)
        .transpose()
        .map_err(io::Error::other)?;
    let app = router_with_client(config, tines, cancellation_token.clone());
    axum::serve(listener, app)
        .with_graceful_shutdown(cancellation_token.cancelled_owned())
        .await
}

async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}
