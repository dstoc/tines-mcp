use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tines_mcp::{config::Config, server};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const DISCOVER_2026: &str = include_str!("fixtures/discover-2026-07-28.json");
const DISCOVER_2025: &str = include_str!("fixtures/discover-2025-11-25.json");
const INITIALIZE_2025: &str = include_str!("fixtures/initialize-2025-11-25.json");
const INITIALIZE_2026_FALLBACK: &str = include_str!("fixtures/initialize-2026-07-28-fallback.json");

fn app() -> Router {
    let config = Config::from_lookup(|_| None).expect("default config should be valid");
    server::router(&config, CancellationToken::new())
}

async fn post_fixture(
    app: Router,
    version: &str,
    method: &str,
    fixture: &str,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", version);
    if method == "server/discover" {
        request = request.header("Mcp-Method", method);
    }
    app.oneshot(
        request
            .body(Body::from(fixture.to_owned()))
            .expect("fixture request should build"),
    )
    .await
    .expect("in-process HTTP request should succeed")
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body should be readable")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response should contain JSON")
}

#[tokio::test]
async fn discovery_supports_both_protocol_revisions_and_only_advertises_tools() {
    for (version, fixture) in [("2026-07-28", DISCOVER_2026), ("2025-11-25", DISCOVER_2025)] {
        let response = post_fixture(app(), version, "server/discover", fixture).await;
        assert_eq!(response.status(), StatusCode::OK, "version {version}");
        assert!(response.headers().get("Mcp-Session-Id").is_none());
        let body = response_json(response).await;
        let result = &body["result"];
        let versions = result["supportedVersions"]
            .as_array()
            .expect("supported versions should be listed");
        assert!(versions.contains(&json!("2026-07-28")));
        assert!(versions.contains(&json!("2025-11-25")));
        assert_eq!(result["capabilities"], json!({"tools": {}}));
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "tines-mcp"
        );
    }
}

#[tokio::test]
async fn legacy_initialization_and_tool_discovery_are_stateless() {
    let response = post_fixture(app(), "2025-11-25", "initialize", INITIALIZE_2025).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("Mcp-Session-Id").is_none());
    let initialized = response_json(response).await;
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(initialized["result"]["capabilities"], json!({"tools": {}}));

    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .body(Body::from(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}).to_string(),
        ))
        .expect("tools/list request should build");
    let response = app()
        .oneshot(request)
        .await
        .expect("in-process HTTP request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("Mcp-Session-Id").is_none());
    assert_eq!(response_json(response).await["result"]["tools"], json!([]));
}

#[tokio::test]
async fn sdk_falls_back_to_legacy_initialization_for_the_2026_revision() {
    let response = post_fixture(app(), "2026-07-28", "initialize", INITIALIZE_2026_FALLBACK).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("Mcp-Session-Id").is_none());
    let initialized = response_json(response).await;
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(initialized["result"]["capabilities"], json!({"tools": {}}));
}

#[tokio::test]
async fn health_is_liveness_only_and_server_cancels_cleanly_without_tines_config() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header("Host", "localhost")
                .body(Body::empty())
                .expect("health request should build"),
        )
        .await
        .expect("health request should succeed");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.headers().get("Cache-Control"), None);

    let config = Config::from_lookup(|_| None).expect("default config should be valid");
    assert!(config.tines.is_none());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind without upstream credentials");
    let cancellation = CancellationToken::new();
    let server_cancellation = cancellation.clone();
    let server_task =
        tokio::spawn(async move { server::run(listener, &config, server_cancellation).await });
    cancellation.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), server_task)
        .await
        .expect("server should stop after cancellation")
        .expect("server task should complete")
        .expect("server should stop cleanly");
}

#[tokio::test]
async fn transport_rejects_unlisted_hosts_and_origins() {
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "evil.example")
        .header("Content-Type", "application/json")
        .body(Body::from(DISCOVER_2026))
        .expect("request should build");
    let response = app()
        .oneshot(request)
        .await
        .expect("in-process HTTP request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Origin", "https://evil.example")
        .header("Content-Type", "application/json")
        .body(Body::from(DISCOVER_2026))
        .expect("request should build");
    let response = app()
        .oneshot(request)
        .await
        .expect("in-process HTTP request should succeed");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
