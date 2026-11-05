//! Executor and HTTP acceptance. Mock replies prove forwarding, not native gate semantics.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, Response, StatusCode},
    routing::any,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tines_mcp::{
    config::Config,
    server,
    tines::TinesClient,
    tools::{
        self,
        contract::{LocalLimits, MAX_RESULT_BYTES},
    },
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const PUT: &str = "put_issue_artifact";
const UPLOAD: &str = "upload_issue_artifact_file";
const REAFFIRM: &str = "reaffirm_issue_artifact";
const PROPOSAL: &str = "# Proposal: exact bytes\n\nCafé 🦀\r\n\"quotes\" \\ path\n";
const DEFAULT_LIMIT: usize = 2 * 1024 * 1024;

struct Observed {
    method: String,
    uri: String,
    content_type: Option<String>,
    bytes: Vec<u8>,
}
struct Reply {
    status: u16,
    content_type: &'static str,
    bytes: Vec<u8>,
    gate: Option<Arc<Semaphore>>,
}
impl Reply {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            bytes: serde_json::to_vec(&value).unwrap(),
            gate: None,
        }
    }
    fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            content_type: "application/octet-stream",
            bytes: bytes.into(),
            gate: None,
        }
    }
}
#[derive(Clone)]
struct Mock {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    requests: Arc<Mutex<Vec<Observed>>>,
    seen: mpsc::UnboundedSender<()>,
}
struct Upstream {
    url: String,
    state: Mock,
    seen: mpsc::UnboundedReceiver<()>,
    task: JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn handler(State(state): State<Mock>, request: Request<Body>) -> Response<Body> {
    assert_eq!(
        request.headers()["Authorization"],
        "Bearer acceptance-secret"
    );
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 26 * 1024 * 1024).await.unwrap().to_vec();
    state.requests.lock().unwrap().push(Observed {
        method: parts.method.to_string(),
        uri: parts.uri.to_string(),
        content_type: parts
            .headers
            .get("Content-Type")
            .map(|v| v.to_str().unwrap().to_owned()),
        bytes,
    });
    state.seen.send(()).unwrap();
    let reply = state
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected extra upstream request");
    if let Some(gate) = reply.gate {
        gate.acquire().await.unwrap().forget();
    }
    Response::builder()
        .status(reply.status)
        .header("Content-Type", reply.content_type)
        .header("Retry-After", "13")
        .header("Location", "/must-not-follow")
        .body(Body::from(reply.bytes))
        .unwrap()
}
async fn upstream(replies: Vec<Reply>) -> Upstream {
    let (tx, seen) = mpsc::unbounded_channel();
    let state = Mock {
        replies: Arc::new(Mutex::new(replies.into())),
        requests: Default::default(),
        seen: tx,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(any(handler))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Upstream {
        url,
        state,
        seen,
        task,
    }
}
fn config(url: &str, limit: usize) -> Config {
    Config::from_lookup(|key| match key {
        "TINES_BASE_URL" => Some(url.into()),
        "TINES_API_TOKEN" => Some("acceptance-secret".into()),
        "TINES_ALLOW_HTTP" => Some("true".into()),
        "MCP_MAX_REQUEST_BYTES" => Some(limit.to_string()),
        _ => None,
    })
    .unwrap()
}
fn client(upstream: &Upstream) -> TinesClient {
    TinesClient::new(config(&upstream.url, DEFAULT_LIMIT).tines.as_ref().unwrap()).unwrap()
}
fn app(upstream: &Upstream, limit: usize) -> Router {
    let config = config(&upstream.url, limit);
    server::router_with_client(&config, Some(client(upstream)), CancellationToken::new())
}
fn artifact(kind: &str, version: u64) -> Value {
    json!({"id":"ctx-proposal", "issue_id":"iss-fixture", "name":"proposal", "artifact_type":kind,
        "description":"native description", "version_count":version, "fresh":true,
        "current_version":{"version":version,"created_at":1793800000000_u64,"reaffirmed_from":null,
        "filename":"proposal.md","content_type":"text/markdown"}})
}
fn upload(content: &str, encoding: &str) -> Value {
    json!({"issue_id":"iss-fixture","name":"proposal","filename":"draft ü &?#.md",
        "content_type":"text/markdown; charset=utf-8","encoding":encoding,"content":content})
}
fn check_result(result: &rmcp::model::CallToolResult) -> Value {
    assert!(serde_json::to_vec(result).unwrap().len() <= MAX_RESULT_BYTES);
    let envelope = result.structured_content.clone().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.content[0].as_text().unwrap().text).unwrap(),
        envelope
    );
    assert_eq!(result.is_error, Some(envelope.get("error").is_some()));
    envelope
}
async fn call(client: &TinesClient, name: &str, args: Value) -> Value {
    check_result(
        &tools::dispatch(
            client,
            name,
            args.as_object().unwrap().clone(),
            &LocalLimits::default(),
            CancellationToken::new(),
        )
        .await,
    )
}
fn wire(name: &str, args: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap()
}
async fn rpc(app: Router, method: &str, bytes: Vec<u8>, declared: bool) -> (StatusCode, Value) {
    let len = bytes.len();
    let body = if declared {
        Body::from(bytes)
    } else {
        // An unknown-length streaming body, rather than an exact-size hint.
        let (mut tx, stream) = http_body_util::Channel::<_, std::convert::Infallible>::new(1);
        tokio::spawn(async move {
            for chunk in bytes.chunks(16384) {
                if tx
                    .send_data(axum::body::Bytes::copy_from_slice(chunk))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        Body::new(stream)
    };
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .header("Mcp-Method", method);
    if declared {
        request = request.header("Content-Length", len.to_string());
    }
    let response = app.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn count(upstream: &Upstream, expected: usize) {
    assert_eq!(upstream.state.requests.lock().unwrap().len(), expected);
    assert!(
        upstream.state.replies.lock().unwrap().is_empty(),
        "unconsumed expected response"
    );
}

#[tokio::test]
async fn proposal_versions_metadata_and_current_reaffirmation_are_explicit_calls() {
    let first = artifact("text", 1);
    let revised = format!("{PROPOSAL}\nRevision two\n");
    let second = artifact("text", 2);
    let mut meta = second.clone();
    meta["description"] = json!("reviewed metadata");
    let mut reaffirmed = artifact("text", 3);
    reaffirmed["current_version"]["reaffirmed_from"] = json!(2);
    let mut detail = reaffirmed.clone();
    detail["versions"] = json!([
        first["current_version"],
        second["current_version"],
        reaffirmed["current_version"]
    ]);
    let upstream = upstream(vec![
        Reply::json(200, first.clone()),
        Reply::json(200, second.clone()),
        Reply::json(200, meta.clone()),
        Reply::json(200, reaffirmed.clone()),
        Reply::json(200, detail.clone()),
        Reply::bytes(PROPOSAL),
        Reply::bytes(revised.clone()),
    ])
    .await;
    let client = client(&upstream);
    for (args, expected) in [
        (
            json!({"issue_id":"iss-fixture","name":"proposal","type":"text","content":PROPOSAL}),
            first,
        ),
        (
            json!({"issue_id":"iss-fixture","name":"proposal","content":revised}),
            second,
        ),
        (
            json!({"issue_id":"iss-fixture","name":"proposal","description":"reviewed metadata"}),
            meta,
        ),
    ] {
        let result = call(&client, PUT, args).await;
        assert_eq!(result["data"], expected);
        assert_eq!(result["meta"]["write"]["outcome"], "confirmed");
    }
    let result = call(
        &client,
        REAFFIRM,
        json!({"issue_id":"iss-fixture","name":"proposal"}),
    )
    .await;
    assert_eq!(result["data"], reaffirmed);
    assert_eq!(
        result["meta"]["write"]["receipt"]["current_version"]["reaffirmed_from"],
        2
    );
    let metadata = call(
        &client,
        "get_issue_artifact",
        json!({"issue_id":"iss-fixture","name":"proposal"}),
    )
    .await;
    assert_eq!(metadata["data"], detail);
    for version in [Some(1), None] {
        let mut args = json!({"issue_id":"iss-fixture","name":"proposal"});
        if let Some(version) = version {
            args["version"] = json!(version);
        }
        let result = call(&client, "read_artifact_content", args).await;
        assert!(result.get("error").is_none(), "{result}");
        assert_eq!(
            STANDARD
                .decode(result["data"]["content"].as_str().unwrap())
                .unwrap(),
            if version == Some(1) {
                PROPOSAL.as_bytes()
            } else {
                revised.as_bytes()
            }
        );
    }
    count(&upstream, 7);
    let requests = upstream.state.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|r| r.method.as_str())
            .collect::<Vec<_>>(),
        ["PUT", "PUT", "PUT", "POST", "GET", "GET", "GET"]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[2].bytes).unwrap(),
        json!({"description":"reviewed metadata"})
    );
    assert!(requests[3].bytes.is_empty());
    assert_eq!(
        requests[3].uri,
        "/api/v1/issues/iss-fixture/artifacts/proposal/reaffirm"
    );
    assert_eq!(
        requests[4].uri,
        "/api/v1/issues/iss-fixture/artifacts/proposal"
    );
    assert_eq!(
        requests[5].uri,
        "/api/v1/issues/iss-fixture/artifacts/proposal/content?version=1"
    );
    assert_eq!(
        requests[6].uri,
        "/api/v1/issues/iss-fixture/artifacts/proposal/content"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].bytes).unwrap()["content"],
        PROPOSAL
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].bytes).unwrap()["content"],
        revised
    );
}

#[tokio::test]
async fn raw_upload_sends_exact_utf8_binary_empty_and_json_bytes_without_metadata_write() {
    for bytes in [
        PROPOSAL.as_bytes().to_vec(),
        (0..=255).collect(),
        vec![],
        br#"{"valid":"json"}"#.to_vec(),
    ] {
        let upstream = upstream(vec![Reply::json(200, artifact("file", 1))]).await;
        let mut args = upload(&STANDARD.encode(&bytes), "base64");
        args["content_type"] = json!("application/json");
        let result = call(&client(&upstream), UPLOAD, args).await;
        assert_eq!(result["meta"]["write"]["outcome"], "confirmed");
        count(&upstream, 1);
        let requests = upstream.state.requests.lock().unwrap();
        assert_eq!(requests[0].bytes, bytes);
        assert_eq!(
            requests[0].content_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(requests[0].method, "PUT");
        assert_eq!(
            requests[0].uri,
            "/api/v1/issues/iss-fixture/artifacts/proposal/file?filename=draft+%C3%BC+%26%3F%23.md"
        );
    }
    let upstream = upstream(vec![Reply::json(200, artifact("file", 1))]).await;
    let result = call(&client(&upstream), UPLOAD, upload(PROPOSAL, "utf8")).await;
    assert!(result.get("error").is_none());
    count(&upstream, 1);
    assert_eq!(
        upstream.state.requests.lock().unwrap()[0].bytes,
        PROPOSAL.as_bytes()
    );
}

#[tokio::test]
async fn default_and_larger_ingress_limits_count_the_upload_envelope() {
    for declared in [true, false] {
        for (limit, extra, accepted) in [
            (DEFAULT_LIMIT, 0, true),
            (DEFAULT_LIMIT, 1, false),
            (3 * 1024 * 1024, 1, true),
        ] {
            let upstream = upstream(if accepted {
                vec![Reply::json(200, artifact("file", 1))]
            } else {
                vec![]
            })
            .await;
            let args = upload(&"\"\\\n🦀".repeat(25000), "utf8");
            let mut bytes = wire(UPLOAD, args.clone());
            assert!(bytes.len() < DEFAULT_LIMIT);
            bytes.resize(DEFAULT_LIMIT + extra, b' ');
            let (status, body) = rpc(app(&upstream, limit), "tools/call", bytes, declared).await;
            assert_eq!(
                status,
                if accepted {
                    StatusCode::OK
                } else {
                    StatusCode::PAYLOAD_TOO_LARGE
                },
                "{body}"
            );
            count(&upstream, usize::from(accepted));
            if accepted {
                assert_eq!(body["result"]["isError"], false, "{body}");
                assert_eq!(
                    upstream.state.requests.lock().unwrap()[0].bytes,
                    args["content"].as_str().unwrap().as_bytes()
                );
            }
        }
    }
}

#[tokio::test]
async fn a_real_base64_payload_above_default_requires_a_larger_budget() {
    let bytes: Vec<u8> = (0..=255).cycle().take(1600 * 1024).collect();
    let args = upload(&STANDARD.encode(&bytes), "base64");
    let wire = wire(UPLOAD, args);
    assert!(wire.len() > DEFAULT_LIMIT);
    for limit in [DEFAULT_LIMIT, 3 * 1024 * 1024] {
        let accepted = limit > DEFAULT_LIMIT;
        let upstream = upstream(if accepted {
            vec![Reply::json(200, artifact("file", 1))]
        } else {
            vec![]
        })
        .await;
        let (status, result) = rpc(app(&upstream, limit), "tools/call", wire.clone(), true).await;
        assert_eq!(
            status,
            if accepted {
                StatusCode::OK
            } else {
                StatusCode::PAYLOAD_TOO_LARGE
            }
        );
        count(&upstream, usize::from(accepted));
        if accepted {
            assert_eq!(result["result"]["isError"], false);
            assert_eq!(upstream.state.requests.lock().unwrap()[0].bytes, bytes);
        }
    }
}

#[tokio::test]
async fn serialized_output_cap_keeps_compact_success_receipt_without_payload() {
    for tool in [PUT, UPLOAD, REAFFIRM] {
        let mut response = artifact(if tool == UPLOAD { "file" } else { "text" }, 7);
        // Current native artifact metadata omits document content. Stress the
        // extensible native JSON contract with a large unknown metadata field.
        response["extension_metadata"] = json!("\"\\".repeat(64 * 1024));
        // Native JSON can fit the read cap while its duplicated MCP text and
        // structured content, including escaping, cannot fit the result cap.
        assert!(serde_json::to_vec(&response).unwrap().len() < MAX_RESULT_BYTES);
        let upstream = upstream(vec![Reply::json(200, response)]).await;
        let args = match tool {
            UPLOAD => upload(PROPOSAL, "utf8"),
            PUT => {
                json!({"issue_id":"iss-fixture","name":"proposal","type":"text","content":PROPOSAL})
            }
            _ => json!({"issue_id":"iss-fixture","name":"proposal"}),
        };
        let result = tools::dispatch(
            &client(&upstream),
            tool,
            args.as_object().unwrap().clone(),
            &LocalLimits::default(),
            CancellationToken::new(),
        )
        .await;
        let envelope = check_result(&result);
        assert_eq!(result.is_error, Some(false));
        assert!(envelope["data"].is_null());
        assert_eq!(envelope["meta"]["write"]["outcome"], "confirmed");
        let receipt = &envelope["meta"]["write"]["receipt"];
        assert_eq!(receipt["version_count"], 7);
        assert_eq!(receipt["current_version"]["version"], 7);
        assert_eq!(receipt["fresh"], true);
        assert!(receipt["current_version"].get("content").is_none());
        assert!(!envelope["meta"]["omissions"].as_array().unwrap().is_empty());
        assert!(serde_json::to_vec(&result).unwrap().len() < 4096);
        count(&upstream, 1);
    }
}

#[tokio::test]
async fn native_failures_and_invalid_successes_never_retry_artifact_writes() {
    for tool in [PUT, UPLOAD, REAFFIRM] {
        for (status, body, outcome) in [
            (
                401,
                json!({"error":{"code":"unauthorized","message":"denied"}}),
                "unknown",
            ),
            (
                403,
                json!({"error":{"code":"insufficient_permissions","message":"denied","details":{"required":"project:write"}}}),
                "unknown",
            ),
            (
                404,
                json!({"error":{"code":"not_found","message":"missing"}}),
                "unknown",
            ),
            (
                422,
                json!({"error":{"code":"artifact_version_limit","message":"50 versions"}}),
                "unknown",
            ),
            (
                409,
                json!({"error":{"code":"conflict","message":"rejected","details":{"committed":false}}}),
                "rejected",
            ),
            (
                500,
                json!({"error":{"code":"readback_failed","message":"uncertain"}}),
                "unknown",
            ),
            (200, json!({"id":"incomplete-receipt"}), "unknown"),
        ] {
            let upstream = upstream(vec![Reply::json(status, body.clone())]).await;
            let args = if tool == UPLOAD {
                upload(PROPOSAL, "utf8")
            } else if tool == PUT {
                json!({"issue_id":"iss-fixture","name":"proposal","content":PROPOSAL})
            } else {
                json!({"issue_id":"iss-fixture","name":"proposal"})
            };
            let result = call(&client(&upstream), tool, args).await;
            assert!(result.get("error").is_some(), "{result}");
            assert_eq!(result["meta"]["http_status"], status);
            assert_eq!(result["meta"]["retry_after"], "13");
            assert_eq!(result["meta"]["write"]["outcome"], outcome, "{result}");
            if status != 200 {
                assert_eq!(result["error"]["code"], body["error"]["code"]);
            }
            count(&upstream, 1);
        }
        let mut reply = Reply::bytes(b"invalid JSON".to_vec());
        reply.content_type = "application/json";
        let upstream = upstream(vec![reply]).await;
        let args = if tool == UPLOAD {
            upload(PROPOSAL, "utf8")
        } else {
            json!({"issue_id":"iss-fixture","name":"proposal"})
        };
        let result = call(&client(&upstream), tool, args).await;
        assert_eq!(result["meta"]["write"]["outcome"], "unknown");
        assert_eq!(result["meta"]["http_status"], 200);
        count(&upstream, 1);
    }
}

#[tokio::test]
async fn cancelled_upload_distinguishes_before_and_after_dispatch_without_retry() {
    for dispatched in [false, true] {
        let gate = Arc::new(Semaphore::new(0));
        let mut reply = Reply::json(200, artifact("file", 1));
        reply.gate = Some(gate.clone());
        let mut upstream = upstream(if dispatched { vec![reply] } else { vec![] }).await;
        let client = client(&upstream);
        let cancellation = CancellationToken::new();
        if !dispatched {
            cancellation.cancel();
        }
        let ct = cancellation.clone();
        let task = tokio::spawn(async move {
            tools::dispatch(
                &client,
                UPLOAD,
                upload(PROPOSAL, "utf8").as_object().unwrap().clone(),
                &LocalLimits::default(),
                ct,
            )
            .await
        });
        if dispatched {
            tokio::time::timeout(std::time::Duration::from_secs(5), upstream.seen.recv())
                .await
                .unwrap()
                .unwrap();
            cancellation.cancel();
        }
        let result = check_result(&task.await.unwrap());
        assert_eq!(
            result["meta"]["write"]["outcome"],
            if dispatched {
                "unknown"
            } else {
                "not_attempted"
            }
        );
        gate.add_permits(1);
        count(&upstream, usize::from(dispatched));
    }
}

#[tokio::test]
async fn unsupported_transfer_fields_fail_locally_and_urls_are_only_literal_content() {
    let upstream = upstream(vec![]).await;
    let client = client(&upstream);
    for (field, value) in [
        ("url", json!("http://127.0.0.1/private")),
        ("path", json!("/etc/passwd")),
        ("attachment_id", json!("attachment-123")),
        ("description", json!("metadata")),
        ("expected_version", json!(1)),
    ] {
        let mut args = upload(PROPOSAL, "utf8");
        args[field] = value;
        let result = call(&client, UPLOAD, args).await;
        assert_eq!(result["meta"]["write"]["outcome"], "not_attempted");
    }
    let mut args = upload("not canonical base64!", "base64");
    assert_eq!(
        call(&client, UPLOAD, args.clone()).await["meta"]["write"]["outcome"],
        "not_attempted"
    );
    args["content_type"] = json!("text/plain\r\nInjected: yes");
    assert_eq!(
        call(&client, UPLOAD, args).await["meta"]["write"]["outcome"],
        "not_attempted"
    );
    count(&upstream, 0);
    let upstream = self::upstream(vec![Reply::json(200, artifact("file", 1))]).await;
    let literal = format!("{}/must-not-fetch", upstream.url);
    call(&self::client(&upstream), UPLOAD, upload(&literal, "utf8")).await;
    count(&upstream, 1);
    assert_eq!(
        upstream.state.requests.lock().unwrap()[0].bytes,
        literal.as_bytes()
    );
}

#[tokio::test]
async fn discovery_exposes_strict_artifact_contracts_and_effective_request_limit() {
    for limit in [DEFAULT_LIMIT, 3 * 1024 * 1024] {
        let upstream = upstream(vec![]).await;
        let (status, body) = rpc(
            app(&upstream, limit),
            "tools/list",
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
                .unwrap(),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let tools = body["result"]["tools"].as_array().unwrap();
        let fixture: Value =
            serde_json::from_str(include_str!("fixtures/artifact-acceptance.json")).unwrap();
        assert_eq!(fixture["request_limits"]["default"], DEFAULT_LIMIT);
        assert_eq!(fixture["request_limits"]["result"], MAX_RESULT_BYTES);
        for expected in fixture["discovery"].as_array().unwrap() {
            let name = expected["name"].as_str().unwrap();
            let matches: Vec<_> = tools.iter().filter(|t| t["name"] == name).collect();
            assert_eq!(matches.len(), 1, "{name}");
            let tool = matches[0];
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
            for (actual, expected_key) in [
                ("readOnlyHint", "read_only"),
                ("destructiveHint", "destructive"),
                ("idempotentHint", "idempotent"),
            ] {
                assert_eq!(
                    tool["annotations"][actual], expected[expected_key],
                    "{name}"
                );
            }
            assert_eq!(
                tool["inputSchema"]["required"], expected["required"],
                "{name}"
            );
            for field in expected["excluded"].as_array().unwrap() {
                assert!(
                    tool["inputSchema"]["properties"]
                        .get(field.as_str().unwrap())
                        .is_none(),
                    "{name}: {field}"
                );
            }
            assert!(tool["outputSchema"].is_object());
            if name == UPLOAD {
                assert!(
                    tool["description"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("cap is {limit} bytes")),
                    "{tool}"
                );
                assert_eq!(
                    tool["inputSchema"]["$defs"]["Encoding"]["enum"],
                    json!(["utf8", "base64"])
                );
            }
        }
        count(&upstream, 0);
    }
}

/// Opt-in native test. Seed a disposable Backlog issue through the Tines CLI,
/// then supply TINES_ARTIFACT_TEST_URL, TINES_ARTIFACT_TEST_TOKEN and
/// TINES_ARTIFACT_TEST_ISSUE_ID. Never defaults to the runner's ambient key.
/// This verifies the pinned SDK client, not the intended OpenAI client/tunnel.
#[tokio::test]
#[ignore = "requires an explicitly selected disposable native Tines issue and credential"]
async fn native_artifact_versions_and_sdk_upload_bytes() {
    use rmcp::{
        ServiceExt as _,
        model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation},
        transport::StreamableHttpClientTransport,
    };
    use sha2::{Digest, Sha256};

    let base = std::env::var("TINES_ARTIFACT_TEST_URL").expect("set TINES_ARTIFACT_TEST_URL");
    let token = std::env::var("TINES_ARTIFACT_TEST_TOKEN").expect("set TINES_ARTIFACT_TEST_TOKEN");
    let issue = std::env::var("TINES_ARTIFACT_TEST_ISSUE_ID")
        .expect("seed a disposable issue through existing setup first");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let slot = format!("mcp-13-{stamp}");
    let request_limit = std::env::var("TINES_ARTIFACT_TEST_REQUEST_BYTES")
        .map(|v| v.parse::<usize>().unwrap())
        .unwrap_or(DEFAULT_LIMIT);

    // Count every outbound request, including unexpected endpoints and failures.
    // The proxy stores no Authorization headers and disables retries/redirects.
    #[derive(Clone)]
    struct Proxy {
        base: String,
        client: reqwest::Client,
        requests: Arc<Mutex<Vec<(String, String)>>>,
    }
    async fn forward(State(proxy): State<Proxy>, req: Request<Body>) -> Response<Body> {
        let (parts, body) = req.into_parts();
        proxy
            .requests
            .lock()
            .unwrap()
            .push((parts.method.to_string(), parts.uri.to_string()));
        let bytes = to_bytes(body, 26 * 1024 * 1024).await.unwrap();
        let mut request = proxy.client.request(
            parts.method,
            format!("{}{}", proxy.base.trim_end_matches('/'), parts.uri),
        );
        for name in ["authorization", "content-type"] {
            if let Some(value) = parts.headers.get(name) {
                request = request.header(name, value);
            }
        }
        let response = request
            .body(bytes)
            .send()
            .await
            .expect("native request should complete");
        let mut builder = Response::builder().status(response.status());
        for name in ["content-type", "content-disposition", "retry-after"] {
            if let Some(value) = response.headers().get(name) {
                builder = builder.header(name, value);
            }
        }
        builder
            .body(Body::from(response.bytes().await.unwrap()))
            .unwrap()
    }
    struct Tasks(Vec<JoinHandle<()>>);
    impl Drop for Tasks {
        fn drop(&mut self) {
            for task in &self.0 {
                task.abort();
            }
        }
    }
    let mut tasks = Tasks(vec![]);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let proxy = Proxy {
        base,
        requests: requests.clone(),
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(std::time::Duration::from_secs(40))
            .build()
            .unwrap(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", listener.local_addr().unwrap());
    let proxy_app = Router::new().fallback(any(forward)).with_state(proxy);
    tasks.0.push(tokio::spawn(async move {
        axum::serve(listener, proxy_app).await.unwrap();
    }));
    let config = Config::from_lookup(|key| match key {
        "TINES_BASE_URL" => Some(proxy_url.clone()),
        "TINES_API_TOKEN" => Some(token.clone()),
        "MCP_MAX_REQUEST_BYTES" => Some(request_limit.to_string()),
        "TINES_ALLOW_HTTP" => Some("true".into()),
        _ => None,
    })
    .unwrap();
    let client = TinesClient::new(config.tines.as_ref().unwrap()).unwrap();
    let app = server::router_with_client(&config, Some(client), CancellationToken::new());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_url = format!("http://{}/mcp", listener.local_addr().unwrap());
    tasks.0.push(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let sdk = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("mcp-13-native-sdk-acceptance", "1"),
    )
    .serve(StreamableHttpClientTransport::from_uri(mcp_url))
    .await
    .unwrap();
    assert!(
        sdk.list_all_tools()
            .await
            .unwrap()
            .iter()
            .any(|tool| tool.name == UPLOAD)
    );
    assert!(
        requests.lock().unwrap().is_empty(),
        "negotiation and discovery must not contact Tines"
    );
    macro_rules! invoke {
        ($tool:expr,$args:expr) => {{
            let before = requests.lock().unwrap().len();
            let result = sdk
                .call_tool(
                    CallToolRequestParams::new($tool)
                        .with_arguments($args.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            let result = check_result(&result);
            assert_eq!(
                requests.lock().unwrap().len(),
                before + 1,
                "native call must send exactly one request"
            );
            assert!(result.get("error").is_none(), "{result}");
            result
        }};
    }
    let fixture = invoke!("get_issue", json!({"issue_id":issue}));
    assert_eq!(
        fixture["data"]["state"]["category"], "backlog",
        "use an idle disposable Backlog fixture"
    );
    let args = json!({"issue_id":issue,"name":slot,"type":"text","content":PROPOSAL});
    let created = invoke!(PUT, args.clone());
    assert_eq!(created["data"]["version_count"], 1);
    assert_eq!(created["data"]["artifact_type"], "text");
    assert_eq!(created["data"]["fresh"], true);
    let same = invoke!(PUT, args);
    assert_eq!(
        same["data"]["version_count"], 2,
        "identical PUTs still append native versions"
    );
    let revised = format!("{PROPOSAL}\nNative revision 🦀\n");
    let revision = invoke!(PUT, json!({"issue_id":issue,"name":slot,"content":revised}));
    assert_eq!(revision["data"]["version_count"], 3);
    let metadata = invoke!(
        PUT,
        json!({"issue_id":issue,"name":slot,"description":"metadata only"})
    );
    assert_eq!(metadata["data"]["version_count"], 3);
    assert_eq!(metadata["data"]["fresh"], revision["data"]["fresh"]);
    assert_eq!(
        metadata["data"]["current_version"],
        revision["data"]["current_version"]
    );
    let reaffirmed = invoke!(REAFFIRM, json!({"issue_id":issue,"name":slot}));
    assert_eq!(reaffirmed["data"]["version_count"], 4);
    assert_eq!(reaffirmed["data"]["current_version"]["reaffirmed_from"], 3);
    let detail = invoke!("get_issue_artifact", json!({"issue_id":issue,"name":slot}));
    assert_eq!(detail["data"]["versions"].as_array().unwrap().len(), 4);
    for (version, expected) in [(1, PROPOSAL), (4, revised.as_str())] {
        let read = invoke!(
            "read_artifact_content",
            json!({"issue_id":issue,"name":slot,"version":version})
        );
        assert_eq!(read["data"]["content"], expected);
        assert_eq!(
            read["data"]["sha256"],
            Sha256::digest(expected.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
    }
    for (suffix, bytes, encoding, mime) in [
        (
            "utf8",
            PROPOSAL.as_bytes().to_vec(),
            "utf8",
            "text/markdown; charset=utf-8",
        ),
        (
            "binary",
            (0..=255).collect(),
            "base64",
            "application/octet-stream",
        ),
        ("empty", vec![], "base64", "application/json"),
    ] {
        let name = format!("{slot}-{suffix}");
        let content = if encoding == "utf8" {
            String::from_utf8(bytes.clone()).unwrap()
        } else {
            STANDARD.encode(&bytes)
        };
        let uploaded = invoke!(
            UPLOAD,
            json!({"issue_id":issue,"name":name,"filename":"draft ü &?#.md","content_type":mime,"encoding":encoding,"content":content})
        );
        assert_eq!(uploaded["data"]["artifact_type"], "file");
        assert_eq!(uploaded["data"]["version_count"], 1);
        let read = invoke!(
            "read_artifact_content",
            json!({"issue_id":issue,"name":name})
        );
        let actual = if read["data"]["encoding"] == "base64" {
            STANDARD
                .decode(read["data"]["content"].as_str().unwrap())
                .unwrap()
        } else {
            read["data"]["content"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec()
        };
        assert_eq!(actual, bytes);
        assert_eq!(
            read["data"]["sha256"],
            Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        eprintln!(
            "native SDK verified {suffix}: {} decoded bytes",
            bytes.len()
        );
    }
    let listed = invoke!("list_issue_artifacts", json!({"issue_id":issue}));
    assert!(
        listed["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == slot)
    );
    // Digesting the complete source proves byte identity even when the MCP
    // read returns only its first 32 KiB. A larger upload remains subject to
    // the separate 1 MiB read cap; verify its bytes with an explicit native GET.
    for size in [1024 * 1024, 1600 * 1024] {
        if size > 1024 * 1024 && request_limit == DEFAULT_LIMIT {
            continue;
        }
        let bytes: Vec<u8> = (0..=255).cycle().take(size).collect();
        let name = format!("{slot}-size-{size}");
        let args = json!({"issue_id":issue,"name":name,"filename":"binary.dat","content_type":"application/octet-stream","encoding":"base64","content":STANDARD.encode(&bytes)});
        let wire_bytes = wire(UPLOAD, args.clone()).len();
        let uploaded = invoke!(UPLOAD, args);
        assert_eq!(uploaded["data"]["current_version"]["size_bytes"], size);
        if size <= 1024 * 1024 {
            let read = invoke!(
                "read_artifact_content",
                json!({"issue_id":issue,"name":name})
            );
            assert_eq!(
                read["data"]["sha256"],
                Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            assert_eq!(
                STANDARD
                    .decode(read["data"]["content"].as_str().unwrap())
                    .unwrap(),
                bytes[..32768]
            );
        } else {
            let before = requests.lock().unwrap().len();
            let read = sdk
                .call_tool(
                    CallToolRequestParams::new("read_artifact_content").with_arguments(
                        json!({"issue_id":issue,"name":name})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await
                .unwrap();
            let envelope = check_result(&read);
            assert_eq!(envelope["error"]["code"], "upstream_body_too_large");
            assert_eq!(envelope["meta"]["http_status"], 200);
            assert_eq!(requests.lock().unwrap().len(), before + 1);
        }
        let before = requests.lock().unwrap().len();
        let response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .unwrap()
            .get(format!(
                "{proxy_url}/api/v1/issues/{issue}/artifacts/{name}/content"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.bytes().await.unwrap().as_ref(), bytes);
        assert_eq!(
            requests.lock().unwrap().len(),
            before + 1,
            "one explicit native verification GET"
        );
        eprintln!(
            "native SDK verified size: {size} decoded bytes, {wire_bytes} fixture JSON-RPC bytes, configured budget {request_limit}"
        );
    }
    let compact = invoke!(
        PUT,
        json!({"issue_id":issue,"name":format!("{slot}-compact"),"type":"text","content":"\"\\".repeat(64*1024)})
    );
    assert!(compact["data"]["current_version"].get("content").is_none());
    assert!(serde_json::to_vec(&compact).unwrap().len() < 4096);
    assert_eq!(compact["meta"]["write"]["outcome"], "confirmed");
    assert_eq!(compact["meta"]["write"]["receipt"]["version_count"], 1);
    assert!(
        compact["meta"]["write"]["receipt"]["current_version"]
            .get("content")
            .is_none()
    );
    for (tool, args, status, code) in [
        (
            PUT,
            json!({"issue_id":issue,"name":slot,"type":"file"}),
            422,
            "artifact_type_mismatch",
        ),
        (
            "get_issue_artifact",
            json!({"issue_id":issue,"name":format!("{slot}-missing")}),
            404,
            "not_found",
        ),
    ] {
        let before = requests.lock().unwrap().len();
        let result = sdk
            .call_tool(
                CallToolRequestParams::new(tool).with_arguments(args.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        let envelope = check_result(&result);
        assert_eq!(envelope["meta"]["http_status"], status, "{envelope}");
        assert_eq!(envelope["error"]["code"], code, "{envelope}");
        if tool == PUT {
            assert_eq!(envelope["meta"]["write"]["outcome"], "unknown");
        }
        assert_eq!(
            requests.lock().unwrap().len(),
            before + 1,
            "native failures must not retry"
        );
    }
    let before = requests.lock().unwrap().len();
    let mut unsupported = json!({"issue_id":issue,"name":slot,"filename":"x","content_type":"text/plain","encoding":"utf8","content":"x"});
    unsupported["url"] = json!("https://invalid.example/must-not-fetch");
    let result = sdk
        .call_tool(
            CallToolRequestParams::new(UPLOAD)
                .with_arguments(unsupported.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        check_result(&result)["meta"]["write"]["outcome"],
        "not_attempted"
    );
    assert_eq!(requests.lock().unwrap().len(), before);
    eprintln!(
        "native SDK acceptance completed: issue {issue}, slot {slot}, {before} total upstream requests; intended-client and gate transitions remain separate"
    );
    sdk.cancel().await.unwrap();
}

#[tokio::test]
async fn pinned_sdk_client_transports_inline_proposal_and_large_binary() {
    use rmcp::{
        ServiceExt as _,
        model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation},
        transport::StreamableHttpClientTransport,
    };
    for (limit, size) in [(DEFAULT_LIMIT, 1024 * 1024), (3 * 1024 * 1024, 1600 * 1024)] {
        let upstream = upstream(vec![
            Reply::json(200, artifact("file", 1)),
            Reply::json(200, artifact("file", 2)),
        ])
        .await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let app = app(&upstream, limit);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let sdk = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("mcp-13-sdk-test", "1"),
        )
        .serve(StreamableHttpClientTransport::from_uri(url))
        .await
        .unwrap();
        let tools = sdk.list_all_tools().await.unwrap();
        assert!(
            tools
                .iter()
                .find(|t| t.name == UPLOAD)
                .unwrap()
                .description
                .as_ref()
                .unwrap()
                .contains(&format!("cap is {limit} bytes"))
        );
        assert!(upstream.state.requests.lock().unwrap().is_empty());
        let bytes: Vec<u8> = (0..=255).cycle().take(size).collect();
        for args in [
            upload(PROPOSAL, "utf8"),
            upload(&STANDARD.encode(&bytes), "base64"),
        ] {
            let result = sdk
                .call_tool(
                    CallToolRequestParams::new(UPLOAD)
                        .with_arguments(args.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            assert_eq!(
                check_result(&result)["meta"]["write"]["outcome"],
                "confirmed"
            );
        }
        count(&upstream, 2);
        {
            let requests = upstream.state.requests.lock().unwrap();
            assert_eq!(requests[0].bytes, PROPOSAL.as_bytes());
            assert_eq!(requests[1].bytes, bytes);
        }
        sdk.cancel().await.unwrap();
        task.abort();
    }
}
