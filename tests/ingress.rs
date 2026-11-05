use axum::{
    Router,
    body::{Body, Bytes},
    http::{Request, StatusCode},
    routing::any,
};
use http_body_util::Channel;
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tines_mcp::{config::Config, server, tines::TinesClient};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const DISCOVER: &str = include_str!("fixtures/discover-2026-07-28.json");
fn config(limit: Option<usize>) -> Config {
    Config::from_lookup(|name| {
        if name == "MCP_MAX_REQUEST_BYTES" {
            limit.map(|n| n.to_string())
        } else {
            None
        }
    })
    .unwrap()
}
fn request(body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .body(body)
        .unwrap()
}
fn channel() -> (http_body_util::channel::Sender<Bytes, Infallible>, Body) {
    let (sender, body) = Channel::new(1);
    (sender, Body::new(body))
}
async fn status(app: Router, req: Request<Body>, expected: StatusCode) {
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), expected);
    assert_eq!(response.headers()["Cache-Control"], "no-store");
}
#[tokio::test]
async fn exact_wire_boundary_counts_escaping_metadata_and_envelope() {
    let mut json: serde_json::Value = serde_json::from_str(DISCOVER).unwrap();
    json["params"]["_meta"]["payload"] = serde_json::json!("\"\\\n".repeat(25000));
    let wire = serde_json::to_vec(&json).unwrap();
    assert!(wire.len() > 64 * 1024);
    for declared in [false, true] {
        for delta in [0, 1] {
            let app = server::router(&config(Some(wire.len() - delta)), CancellationToken::new());
            let mut req = request(Body::from(wire.clone()));
            if declared {
                req.headers_mut()
                    .insert("Content-Length", wire.len().to_string().parse().unwrap());
            }
            status(
                app,
                req,
                if delta == 0 {
                    StatusCode::OK
                } else {
                    StatusCode::PAYLOAD_TOO_LARGE
                },
            )
            .await;
        }
    }
}
#[tokio::test]
async fn default_and_larger_configured_budgets_bound_streams() {
    let mut wire = DISCOVER.as_bytes().to_vec();
    wire.resize(2 * 1024 * 1024, b' ');
    for (limit, extra, expected) in [
        (None, 0, StatusCode::OK),
        (None, 1, StatusCode::PAYLOAD_TOO_LARGE),
        (Some(3 * 1024 * 1024), 1, StatusCode::OK),
        (Some(36 * 1024 * 1024), 4 * 1024 * 1024, StatusCode::OK),
    ] {
        let mut bytes = wire.clone();
        bytes.resize(bytes.len() + extra, b' ');
        let (mut sender, body) = channel();
        let producer = tokio::spawn(async move {
            for chunk in bytes.chunks(16384) {
                if sender
                    .send_data(Bytes::copy_from_slice(chunk))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        status(
            server::router(&config(limit), CancellationToken::new()),
            request(body),
            expected,
        )
        .await;
        producer.await.unwrap();
    }
}
#[tokio::test(start_paused = true)]
async fn length_rejection_does_not_poll_a_stalled_body() {
    let (_sender, body) = channel();
    let mut req = request(body);
    req.headers_mut()
        .insert("Content-Length", "1025".parse().unwrap());
    let start = tokio::time::Instant::now();
    status(
        server::router(&config(Some(1024)), CancellationToken::new()),
        req,
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    assert_eq!(start.elapsed(), std::time::Duration::ZERO);
}
#[tokio::test(start_paused = true)]
async fn stalled_readers_release_permits_at_total_deadline_and_gets_bypass_admission() {
    let app = server::router(&config(None), CancellationToken::new());
    let mut senders = Vec::new();
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let (sender, body) = channel();
        senders.push(sender);
        tasks.push(tokio::spawn(app.clone().oneshot(request(body))));
        tokio::task::yield_now().await;
    }
    let (mut rejected_sender, rejected_body) = channel();
    let before_rejection = tokio::time::Instant::now();
    status(
        app.clone(),
        request(rejected_body),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(before_rejection.elapsed(), std::time::Duration::ZERO);
    assert!(rejected_sender.send_data(Bytes::new()).await.is_err());
    let (_sender, body) = channel();
    let req = Request::builder()
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Accept", "text/event-stream")
        .body(body)
        .unwrap();
    let start = tokio::time::Instant::now();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_ne!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(start.elapsed(), std::time::Duration::ZERO);
    // Progress does not reset the total 30-second body deadline.
    tokio::time::advance(std::time::Duration::from_secs(29)).await;
    for sender in &mut senders {
        sender.send_data(Bytes::from_static(b" ")).await.unwrap();
    }
    tokio::task::yield_now().await;
    assert!(tasks.iter().all(|task| !task.is_finished()));
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    for task in tasks {
        assert_eq!(
            task.await.unwrap().unwrap().status(),
            StatusCode::REQUEST_TIMEOUT
        );
    }
    status(app, request(Body::from(DISCOVER)), StatusCode::OK).await;
}
#[tokio::test]
async fn dropped_requests_and_shutdown_drop_stalled_bodies() {
    let cancellation = CancellationToken::new();
    let app = server::router(&config(None), cancellation.clone());
    let (mut sender, body) = channel();
    let task = tokio::spawn(app.clone().oneshot(request(body)));
    tokio::task::yield_now().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(sender.send_data(Bytes::new()).await.is_err());
    status(app.clone(), request(Body::from(DISCOVER)), StatusCode::OK).await;
    let (mut sender, body) = channel();
    let task = tokio::spawn(app.oneshot(request(body)));
    tokio::task::yield_now().await;
    cancellation.cancel();
    assert_eq!(
        task.await.unwrap().unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(sender.send_data(Bytes::new()).await.is_err());
}
#[tokio::test]
async fn rejected_ingress_and_discovery_make_zero_upstream_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let upstream = Router::new().fallback(any(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        async { StatusCode::INTERNAL_SERVER_ERROR }
    }));
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let config = Config::from_lookup(|name| match name {
        "TINES_BASE_URL" => Some(base.clone()),
        "TINES_API_TOKEN" => Some("fixture-token".into()),
        "TINES_ALLOW_HTTP" => Some("true".into()),
        "MCP_MAX_REQUEST_BYTES" => Some("1024".into()),
        _ => None,
    })
    .unwrap();
    let client = TinesClient::new(config.tines.as_ref().unwrap()).unwrap();
    let app = server::router_with_client(&config, Some(client), CancellationToken::new());
    status(app.clone(), request(Body::from(DISCOVER)), StatusCode::OK).await;
    let call = serde_json::json!({"jsonrpc":"2.0", "id":1,"method":"tools/call","params":{"name":"list_projects","arguments":{},"_meta":{"padding":"x".repeat(1024)}}}).to_string();
    for declared in [false, true] {
        let mut req = request(Body::from(call.clone()));
        req.headers_mut().remove("Mcp-Method");
        if declared {
            req.headers_mut()
                .insert("Content-Length", call.len().to_string().parse().unwrap());
        }
        status(app.clone(), req, StatusCode::PAYLOAD_TOO_LARGE).await;
    }
    let mut senders = Vec::new();
    let mut readers = Vec::new();
    for _ in 0..4 {
        let (sender, body) = channel();
        senders.push(sender);
        readers.push(tokio::spawn(app.clone().oneshot(request(body))));
        tokio::task::yield_now().await;
    }
    status(
        app,
        request(Body::from(call)),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    for reader in readers {
        reader.abort();
        let _ = reader.await;
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn nested_service_paths_cannot_bypass_ingress() {
    let app = server::router(&config(Some(1024)), CancellationToken::new());
    for path in ["/mcp/", "/mcp/nested"] {
        let mut req = request(Body::from("x".repeat(1025)));
        *req.uri_mut() = path.parse().unwrap();
        status(app.clone(), req, StatusCode::PAYLOAD_TOO_LARGE).await;
    }
}

#[test]
fn invalid_request_budget_fails_before_server_startup() {
    for value in ["", "0", "37748737", "invalid", "999999999999999999999999"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_tines-mcp"))
            .env_clear()
            .env("MCP_MAX_REQUEST_BYTES", value)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("MCP_MAX_REQUEST_BYTES"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("listening"));
    }
}

#[cfg(unix)]
#[test]
fn non_unicode_request_budget_fails_at_startup() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tines-mcp"))
        .env_clear()
        .env("MCP_MAX_REQUEST_BYTES", OsString::from_vec(vec![0xff]))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("MCP_MAX_REQUEST_BYTES"));
}

#[tokio::test]
async fn declared_length_cannot_replace_counting_actual_bytes() {
    let app = server::router(&config(Some(1024)), CancellationToken::new());
    let mut req = request(Body::from("x".repeat(1025)));
    req.headers_mut()
        .insert("Content-Length", "10".parse().unwrap());
    status(app, req, StatusCode::PAYLOAD_TOO_LARGE).await;
}
