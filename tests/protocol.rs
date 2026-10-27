use std::future::Future;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use http_body_util::BodyExt;
use rmcp::{
    ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
        ServerCapabilities, ServerConfig,
    },
    service::{RequestContext, RoleServer},
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use tines_mcp::{
    config::Config,
    server,
    tines::{Query, Route, TinesClient},
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const DISCOVER_2026: &str = include_str!("fixtures/discover-2026-07-28.json");
const DISCOVER_2025: &str = include_str!("fixtures/discover-2025-11-25.json");
const INITIALIZE_2025: &str = include_str!("fixtures/initialize-2025-11-25.json");
const INITIALIZE_2026_FALLBACK: &str = include_str!("fixtures/initialize-2026-07-28-fallback.json");

#[derive(Clone)]
struct UpstreamAuthFailure {
    result: CallToolResult,
}

impl ServerHandler for UpstreamAuthFailure {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tines-mcp-test", "0.1.0"))
    }

    fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Ok(self.result.clone().into()))
    }
}

fn upstream_auth_error_app(result: CallToolResult) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(vec!["localhost".to_owned()])
        .enforce_origin_validation();
    let service = StreamableHttpService::new(
        move || {
            Ok(UpstreamAuthFailure {
                result: result.clone(),
            })
        },
        LocalSessionManager::default().into(),
        config,
    );
    Router::new().nest_service("/mcp", service)
}

fn app() -> Router {
    let config = Config::from_lookup(|_| None).expect("default config should be valid");
    server::router(&config, CancellationToken::new())
}

fn app_with_allowed_origins(origins: &str) -> Router {
    let config =
        Config::from_lookup(|name| (name == "MCP_ALLOWED_ORIGINS").then(|| origins.to_owned()))
            .expect("configured origins should be valid");
    server::router(&config, CancellationToken::new())
}

async fn discovery_status(app: Router, origin: Option<&str>) -> StatusCode {
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover");
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    let response = app
        .oneshot(
            request
                .body(Body::from(DISCOVER_2026))
                .expect("discovery request should build"),
        )
        .await
        .expect("in-process HTTP request should succeed");
    response.status()
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
        assert_eq!(response.headers().get("Cache-Control").unwrap(), "no-store");
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
    let body = response_json(response).await;
    let tools = body["result"]["tools"]
        .as_array()
        .expect("read tools should be listed");
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "list_projects",
            "get_project",
            "list_issues",
            "get_issue",
            "list_workflows",
            "get_workflow",
            "list_labels",
            "list_context",
            "get_context",
            "get_issue_context",
            "get_issue_journal",
        ]
    );
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

#[tokio::test]
async fn configured_default_origin_matches_default_port_only() {
    let app = app_with_allowed_origins("https://client.example");

    for origin in ["https://client.example", "https://client.example:443"] {
        assert_eq!(
            discovery_status(app.clone(), Some(origin)).await,
            StatusCode::OK,
            "default-port equivalent origin {origin} should be allowed"
        );
    }
    assert_eq!(
        discovery_status(app.clone(), Some("https://client.example:8443")).await,
        StatusCode::FORBIDDEN,
        "a different port should be rejected"
    );
    assert_eq!(
        discovery_status(app, None).await,
        StatusCode::OK,
        "requests without an Origin header should remain allowed"
    );
}

#[tokio::test]
async fn configured_custom_origin_port_remains_exact() {
    let app = app_with_allowed_origins("https://client.example:8443");

    assert_eq!(
        discovery_status(app.clone(), Some("https://client.example:8443")).await,
        StatusCode::OK
    );
    assert_eq!(
        discovery_status(app.clone(), Some("https://client.example:9443")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        discovery_status(app, Some("https://client.example")).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn upstream_401_and_403_remain_mcp_tool_errors() {
    for status in [401, 403] {
        let upstream_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock Tines listener should bind");
        let upstream_address = upstream_listener
            .local_addr()
            .expect("mock Tines listener should have an address");
        let upstream_status = StatusCode::from_u16(status).expect("test status should be valid");
        let upstream_app = Router::new().route(
            "/api/v1/projects",
            get(move || async move {
                axum::http::Response::builder()
                    .status(upstream_status)
                    .header(header::WWW_AUTHENTICATE, "Bearer")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"error":{"code":"access_denied","message":"Tines refused the configured credential."}}"#,
                    ))
                    .expect("mock upstream response should build")
            }),
        );
        let upstream_task = tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app)
                .await
                .expect("mock Tines server should remain available");
        });
        let base_url = format!("http://{upstream_address}");
        let values = [
            ("TINES_BASE_URL", base_url.as_str()),
            ("TINES_API_TOKEN", "local-test-token"),
            ("TINES_ALLOW_HTTP", "true"),
        ];
        let config = Config::from_lookup(|name| {
            values
                .iter()
                .find_map(|(key, value)| (*key == name).then(|| (*value).to_owned()))
        })
        .expect("mock Tines config should validate locally");
        let client = TinesClient::new(config.tines.as_ref().unwrap())
            .expect("client build should not contact Tines");
        let error = client
            .run_tool(CancellationToken::new(), |call| async move {
                let query = Query::new();
                call.get_json(Route::Projects, &query).await
            })
            .await
            .expect_err("upstream authorization failure should become a tool error");

        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": status,
                    "method": "tools/call",
                    "params": {"name": "auth-test", "arguments": {}}
                })
                .to_string(),
            ))
            .expect("tool request should build");
        let response = upstream_auth_error_app(error.to_mcp_result())
            .oneshot(request)
            .await
            .expect("in-process MCP request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("WWW-Authenticate").is_none());
        let body = response_json(response).await;
        assert_eq!(body["result"]["isError"], true);
        assert_eq!(
            body["result"]["structuredContent"]["error"]["status"],
            status
        );
        assert_eq!(
            body["result"]["structuredContent"]["error"]["source"],
            "upstream"
        );
        upstream_task.abort();
    }
}

#[tokio::test]
async fn mcp_request_body_limit_rejects_oversized_content_without_echoing_it() {
    let private_body = "private-request-body".repeat(4096);
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .body(Body::from(private_body.clone()))
        .expect("oversized request should build");
    let response = app()
        .oneshot(request)
        .await
        .expect("in-process HTTP request should be handled");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("response body should be readable")
        .to_bytes();
    assert!(!String::from_utf8_lossy(&body).contains(&private_body));
}
