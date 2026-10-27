use std::io;

use axum::{Router, http::StatusCode, routing::get};
use rmcp::{
    ServerHandler,
    model::{Implementation, ServerCapabilities, ServerConfig},
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

#[derive(Clone)]
struct BootstrapServer;

impl ServerHandler for BootstrapServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tines-mcp", env!("CARGO_PKG_VERSION")))
    }
}

pub fn router(config: &Config, cancellation_token: CancellationToken) -> Router {
    let server_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(config.allowed_hosts.clone())
        .with_allowed_origins(config.allowed_origins.clone())
        .enforce_origin_validation()
        .with_cancellation_token(cancellation_token);
    let mcp = StreamableHttpService::new(
        || Ok(BootstrapServer),
        LocalSessionManager::default().into(),
        server_config,
    );

    Router::new()
        .route("/healthz", get(healthz))
        .nest_service("/mcp", mcp)
}

pub async fn run(
    listener: TcpListener,
    config: &Config,
    cancellation_token: CancellationToken,
) -> io::Result<()> {
    let app = router(config, cancellation_token.clone());
    axum::serve(listener, app)
        .with_graceful_shutdown(cancellation_token.cancelled_owned())
        .await
}

async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}
