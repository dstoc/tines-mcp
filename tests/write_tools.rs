use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode},
    response::Response,
    routing::any,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tines_mcp::{config::Config, server, tines::TinesClient};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    body: String,
}

struct ObservedRequest {
    method: Method,
    path: String,
    authorization: String,
    body: Value,
}

#[derive(Clone)]
struct Mock {
    issue: Arc<Mutex<Reply>>,
    comment: Arc<Mutex<Reply>>,
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
}

struct Upstream {
    url: String,
    state: Mock,
    task: JoinHandle<()>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handler(State(state): State<Mock>, request: Request<Body>) -> Response {
    let method = request.method().clone();
    let path = request.uri().to_string();
    let auth = request
        .headers()
        .get("Authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    state.requests.lock().unwrap().push(ObservedRequest {
        method: method.clone(),
        path,
        authorization: auth,
        body: payload,
    });
    let reply = if method == Method::POST {
        state.comment.lock().unwrap().clone()
    } else {
        state.issue.lock().unwrap().clone()
    };
    Response::builder()
        .status(reply.status)
        .header("Content-Type", "application/json")
        .header("Retry-After", "7")
        .header("Location", "/unexpected-redirect")
        .body(Body::from(reply.body))
        .unwrap()
}

async fn upstream() -> Upstream {
    let state = Mock {
        issue: Arc::new(Mutex::new(Reply {
            status: StatusCode::OK,
            body: json!({
                "id":"iss-a", "project_id":"prj-a", "project_name":"Project", "number":12
            })
            .to_string(),
        })),
        comment: Arc::new(Mutex::new(Reply {
            status: StatusCode::CREATED,
            body: json!({
                "id":"cmt-a", "issue_id":"iss-a", "body":"native body", "created_at":12345,
                "actor":{"display_name":"Writer"}
            })
            .to_string(),
        })),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(any(handler))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Upstream { url, state, task }
}

fn app(base_url: &str) -> Router {
    let config = Config::from_lookup(|name| match name {
        "TINES_BASE_URL" => Some(base_url.to_owned()),
        "TINES_API_TOKEN" => Some("test-write-token".to_owned()),
        "TINES_ALLOW_HTTP" => Some("true".to_owned()),
        _ => None,
    })
    .unwrap();
    let client = TinesClient::new(config.tines.as_ref().unwrap()).unwrap();
    server::router_with_client(&config, Some(client), CancellationToken::new())
}

async fn rpc(app: Router, method: &str, mut params: Value, version: &str) -> (StatusCode, Value) {
    if version == "2026-07-28" {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion":version,
            "io.modelcontextprotocol/clientCapabilities":{},
            "io.modelcontextprotocol/clientInfo":{"name":"write-fixture","version":"1"}
        });
    }
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", version)
        .header("Mcp-Method", method);
    if let Some(name) = params.get("name").and_then(Value::as_str) {
        request = request.header("Mcp-Name", name);
    }
    let response = app
        .oneshot(
            request
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    (status, body)
}

async fn append(app: Router, args: Value) -> Value {
    append_with_version(app, args, "2025-11-25").await
}

async fn append_with_version(app: Router, args: Value, version: &str) -> Value {
    let (status, body) = rpc(
        app,
        "tools/call",
        json!({"name":"append_issue_comment","arguments":args}),
        version,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    if let Some(result) = body.get("result") {
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            result["structuredContent"]
        );
    }
    body
}

fn assert_outcome(body: &Value, outcome: &str) {
    assert_eq!(body["result"]["isError"], true, "{body}");
    assert_eq!(
        body["result"]["structuredContent"]["error"]["details"]["write_outcome"], outcome,
        "{body}"
    );
}

#[tokio::test]
async fn native_receipt_and_links_preserve_body_and_repeated_calls_send_one_post_each() {
    let upstream = upstream().await;
    let app = app(&upstream.url);
    let body = "  **Findings**\n\"quotes\" \\ path 🦀\n\u{85}  ";
    for version in ["2025-11-25", "2026-07-28"] {
        let result = append_with_version(
            app.clone(),
            json!({"issue_id":"iss-a","body":body}),
            version,
        )
        .await;
        assert_eq!(result["result"]["isError"], false);
        let data = &result["result"]["structuredContent"]["data"];
        assert_eq!(
            *data,
            serde_json::from_str::<Value>(&upstream.state.comment.lock().unwrap().body).unwrap()
        );
        assert_eq!(data["id"], "cmt-a");
        let links = &result["result"]["structuredContent"]["meta"]["links"];
        assert_eq!(
            links[0]["href"],
            format!("{}/issues/prj-a/12", upstream.url)
        );
    }
    let requests = upstream.state.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for pair in requests.as_chunks::<2>().0 {
        assert_eq!(
            (pair[0].method.clone(), pair[0].path.as_str()),
            (Method::GET, "/api/v1/issues/iss-a")
        );
        assert_eq!(
            (pair[1].method.clone(), pair[1].path.as_str()),
            (Method::POST, "/api/v1/issues/iss-a/comments")
        );
        assert_eq!(pair[1].authorization, "Bearer test-write-token");
        assert_eq!(pair[1].body, json!({"body":body}));
    }
}

#[tokio::test]
async fn strict_arguments_and_semantic_validation_never_send_a_request() {
    let upstream = upstream().await;
    let app = app(&upstream.url);
    for args in [
        json!({}),
        json!({"issue_id":"iss-a"}),
        json!({"body":"text"}),
        json!({"issue_id":null,"body":"text"}),
        json!({"issue_id":"iss-a","body":null}),
        json!({"issue_id":12,"body":"text"}),
        json!({"issue_id":"iss-a","body":false}),
        json!({"issue_id":"iss-a","body":"text","author":"other"}),
    ] {
        let result = append(app.clone(), args).await;
        assert_eq!(result["error"]["code"], -32602, "{result}");
    }
    for issue_id in ["", "  ", ".", ".."] {
        let result = append(app.clone(), json!({"issue_id":issue_id,"body":"text"})).await;
        assert_outcome(&result, "not_attempted");
    }
    for body in ["", " \t\r\n", "\u{feff}", "\u{a0}\u{2028}\u{3000}"] {
        let result = append(app.clone(), json!({"issue_id":"iss-a","body":body})).await;
        assert_outcome(&result, "not_attempted");
    }
    assert!(upstream.state.requests.lock().unwrap().is_empty());
    let result = append(app, json!({"issue_id":"iss-a","body":"\u{85}"})).await;
    assert_eq!(result["result"]["isError"], false);
}

#[tokio::test]
async fn transport_request_cap_remains_64_kib() {
    let upstream = upstream().await;
    let app = app(&upstream.url);
    let (status, _) = rpc(
        app,
        "tools/call",
        json!({"name":"append_issue_comment", "arguments":{
            "issue_id":"iss-a", "body":"x".repeat(64 * 1024)
        }}),
        "2025-11-25",
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(upstream.state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_issue_identity_and_parent_api_failures_prevent_post() {
    for issue in [
        json!({}),
        json!({"id":"other","project_id":"prj-a","number":12}),
    ] {
        let upstream = upstream().await;
        upstream.state.issue.lock().unwrap().body = issue.to_string();
        let result = append(
            app(&upstream.url),
            json!({"issue_id":"iss-a","body":"text"}),
        )
        .await;
        assert_outcome(&result, "not_attempted");
        assert_eq!(
            result["result"]["structuredContent"]["error"]["status"],
            200
        );
        let requests = upstream.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::GET);
    }
    for status in [401, 403, 404] {
        let upstream = upstream().await;
        *upstream.state.issue.lock().unwrap() = Reply {
            status: StatusCode::from_u16(status).unwrap(),
            body: json!({"error":{"code":"parent_refused","message":"API refused"}}).to_string(),
        };
        let result = append(
            app(&upstream.url),
            json!({"issue_id":"iss-a","body":"text"}),
        )
        .await;
        assert_outcome(&result, "not_attempted");
        let error = &result["result"]["structuredContent"]["error"];
        assert_eq!(error["status"], status);
        assert_eq!(error["code"], "parent_refused");
        let requests = upstream.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::GET);
    }
}

#[tokio::test]
async fn token_visible_issues_append_with_optional_link_metadata() {
    let upstream = upstream().await;
    for (issue, project_id) in [
        (json!({"id":"iss-a"}), None),
        (json!({"id":"iss-a","number":12}), None),
        (
            json!({"id":"iss-a","project_id":"prj-owner","number":12}),
            Some("prj-owner"),
        ),
        (
            json!({"id":"iss-a","project":{"id":"prj-shared","name":"Shared"},"number":12}),
            Some("prj-shared"),
        ),
    ] {
        upstream.state.issue.lock().unwrap().body = issue.to_string();
        upstream.state.requests.lock().unwrap().clear();
        let result = append(
            app(&upstream.url),
            json!({"issue_id":"iss-a","body":"text"}),
        )
        .await;
        assert_eq!(result["result"]["isError"], false, "{result}");
        let links = &result["result"]["structuredContent"]["meta"]["links"];
        if let Some(project_id) = project_id {
            assert_eq!(
                links[0]["href"],
                format!("{}/issues/{project_id}/12", upstream.url)
            );
        } else {
            assert!(
                links.is_null() || links.as_array().is_some_and(Vec::is_empty),
                "{result}"
            );
        }
        let requests = upstream.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, Method::GET);
        assert_eq!(requests[1].method, Method::POST);
        assert_eq!(requests[1].path, "/api/v1/issues/iss-a/comments");
        assert_eq!(requests[1].authorization, "Bearer test-write-token");
    }
}

#[tokio::test]
async fn native_rejections_and_ambiguous_server_failures_keep_status_and_never_retry() {
    for status in [401, 403, 404, 408, 422, 429, 500, 503] {
        let upstream = upstream().await;
        *upstream.state.comment.lock().unwrap() = Reply { status:StatusCode::from_u16(status).unwrap(),
            body:json!({"error":{"code":"native_failure","message":"API refused or failed","details":{"field":"body"}}}).to_string() };
        let result = append(
            app(&upstream.url),
            json!({"issue_id":"iss-a","body":"private comment"}),
        )
        .await;
        assert_eq!(result["result"]["isError"], true);
        let error = &result["result"]["structuredContent"]["error"];
        assert_eq!(error["status"], status);
        assert_eq!(error["code"], "native_failure");
        assert_eq!(error["details"]["field"], "body");
        assert_eq!(error["retry_after"], "7");
        if status >= 500 || status == 408 {
            assert_outcome(&result, "unknown");
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("Inspect the issue")
            );
        } else {
            assert!(error["details"].get("write_outcome").is_none());
        }
        assert!(!result.to_string().contains("private comment"));
        assert_eq!(upstream.state.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn invalid_success_and_redirect_responses_report_unknown_without_retrying() {
    for (status, body) in [
        (201, "not json".to_owned()),
        (201, "[]".to_owned()),
        (201, json!({"issue_id":"iss-a"}).to_string()),
        (201, json!({"id":"","issue_id":"iss-a"}).to_string()),
        (201, json!({"id":"cmt-a","issue_id":"other"}).to_string()),
        (302, "redirect".to_owned()),
        (403, "not native json".to_owned()),
        (201, "x".repeat(32 * 1024 * 1024 + 1)),
    ] {
        let upstream = upstream().await;
        *upstream.state.comment.lock().unwrap() = Reply {
            status: StatusCode::from_u16(status).unwrap(),
            body,
        };
        let result = append(
            app(&upstream.url),
            json!({"issue_id":"iss-a","body":"private text"}),
        )
        .await;
        assert_outcome(&result, "unknown");
        assert_eq!(
            result["result"]["structuredContent"]["error"]["status"],
            status
        );
        assert_eq!(upstream.state.requests.lock().unwrap().len(), 2);
        assert!(!result.to_string().contains("private text"));
    }
}

#[tokio::test]
async fn oversized_success_result_keeps_created_receipt_even_when_link_context_is_oversized() {
    for oversized_link in [false, true] {
        let upstream = upstream().await;
        let comment = json!({"id":"cmt-a","issue_id":"iss-a","body":"\"\\".repeat(100 * 1024)});
        upstream.state.comment.lock().unwrap().body = comment.to_string();
        if oversized_link {
            upstream.state.issue.lock().unwrap().body = json!({"id":"iss-a","project_id":"prj-a","project_name":"x".repeat(512 * 1024),"number":12}).to_string();
        }
        let result = append(
            app(&upstream.url),
            json!({"issue_id":"iss-a","body":"text"}),
        )
        .await;
        assert_outcome(&result, "created");
        let error = &result["result"]["structuredContent"]["error"];
        assert_eq!(error["code"], "tool_result_too_large");
        assert_eq!(error["details"]["comment_id"], "cmt-a");
        assert!(serde_json::to_vec(&result["result"]).unwrap().len() <= 512 * 1024);
        assert_eq!(upstream.state.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn both_protocol_revisions_discover_twenty_tools_with_accurate_annotations() {
    let config = Config::from_lookup(|_| None).unwrap();
    let app = server::router(&config, CancellationToken::new());
    for version in ["2025-11-25", "2026-07-28"] {
        let (_, result) = rpc(app.clone(), "tools/list", json!({}), version).await;
        let tools = result["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("version {version}: {result}"));
        assert_eq!(tools.len(), 20);
        for tool in tools {
            let write = tool["name"] == "append_issue_comment";
            assert_eq!(
                tool["annotations"],
                json!({"readOnlyHint":!write,"destructiveHint":false,"idempotentHint":!write,"openWorldHint":true})
            );
            if write {
                assert_eq!(tool["inputSchema"]["additionalProperties"], false);
                assert_eq!(tool["inputSchema"]["required"], json!(["issue_id", "body"]));
            }
        }
    }
    let result = append(app, json!({"issue_id":"iss-a","body":"text"})).await;
    assert_outcome(&result, "not_attempted");
}

#[tokio::test]
async fn lost_response_and_truncated_body_report_unknown_and_do_not_resend() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        sync::mpsc,
        time::{Duration, timeout},
    };

    for truncated in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (header_len, content_len) = loop {
                    let mut buffer = [0; 1024];
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while bytes.len() < header_len + content_len {
                    let mut buffer = [0; 1024];
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let post = bytes.starts_with(b"POST ");
                tx.send(post).unwrap();
                if !post {
                    let body = r#"{"id":"iss-a","project_id":"prj-a","number":12}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                } else if truncated {
                    socket.write_all(b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 200\r\nConnection: close\r\n\r\n{\"id\":").await.unwrap();
                }
                // Simulate an append committed by the server, then a lost
                // response (or a read failure after successful status headers).
                socket.shutdown().await.unwrap();
            }
        });
        let result = append(app(&url), json!({"issue_id":"iss-a","body":"text"})).await;
        assert_outcome(&result, "unknown");
        assert!(!rx.recv().await.unwrap());
        assert!(rx.recv().await.unwrap());
        assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        task.abort();
    }
}

#[tokio::test]
async fn immutable_issue_id_is_encoded_as_one_route_segment() {
    let upstream = upstream().await;
    let issue_id = "iss/a?#%";
    upstream.state.issue.lock().unwrap().body = json!({"id":issue_id}).to_string();
    upstream.state.comment.lock().unwrap().body =
        json!({"id":"cmt-a","issue_id":issue_id}).to_string();
    let result = append(
        app(&upstream.url),
        json!({"issue_id":issue_id,"body":"text"}),
    )
    .await;
    assert_eq!(result["result"]["isError"], false);
    let requests = upstream.state.requests.lock().unwrap();
    assert_eq!(requests[0].path, "/api/v1/issues/iss%2Fa%3F%23%25");
    assert_eq!(requests[1].path, "/api/v1/issues/iss%2Fa%3F%23%25/comments");
}

#[tokio::test]
async fn an_untrusted_oversized_receipt_id_cannot_overflow_the_error_result() {
    let upstream = upstream().await;
    upstream.state.comment.lock().unwrap().body =
        json!({"id":"x".repeat(512 * 1024),"issue_id":"iss-a"}).to_string();
    let result = append(
        app(&upstream.url),
        json!({"issue_id":"iss-a","body":"text"}),
    )
    .await;
    assert_outcome(&result, "created");
    assert!(serde_json::to_vec(&result["result"]).unwrap().len() <= 512 * 1024);
    assert!(
        result["result"]["structuredContent"]["error"]["details"]
            .get("comment_id")
            .is_none()
    );
    assert_eq!(upstream.state.requests.lock().unwrap().len(), 2);
}
