use super::*;
use crate::{
    config::Config,
    tools::{self, contract as d},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, Response},
    routing::any,
};
use serde_json::Map;
use std::sync::atomic::AtomicUsize;
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::timeout};

struct Observed {
    method: String,
    uri: String,
    body: Vec<u8>,
    content_type: Option<String>,
    authorized: bool,
}
#[derive(Clone)]
struct Mock {
    status: u16,
    body: Vec<u8>,
    gate: Option<Arc<Semaphore>>,
    count: Arc<AtomicUsize>,
    tx: mpsc::UnboundedSender<Observed>,
}
struct Server {
    client: TinesClient,
    count: Arc<AtomicUsize>,
    rx: mpsc::UnboundedReceiver<Observed>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn client(url: &str, deadline: Duration) -> TinesClient {
    let config = Config::from_lookup(|key| match key {
        "TINES_BASE_URL" => Some(url.to_owned()),
        "TINES_API_TOKEN" => Some("executor-secret".into()),
        "TINES_ALLOW_HTTP" => Some("true".into()),
        _ => None,
    })
    .unwrap();
    TinesClient::with_deadline(config.tines.as_ref().unwrap(), deadline).unwrap()
}
async fn serve(
    status: u16,
    body: Vec<u8>,
    gate: Option<Arc<Semaphore>>,
    deadline: Duration,
) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::unbounded_channel();
    let count = Arc::new(AtomicUsize::new(0));
    let state = Mock {
        status,
        body,
        gate,
        count: count.clone(),
        tx,
    };
    let app = Router::new().fallback(any(handler)).with_state(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server {
        client: client(&url, deadline),
        count,
        rx,
        task,
    }
}
async fn mock(status: u16, body: impl Into<Vec<u8>>) -> Server {
    serve(status, body.into(), None, Duration::from_secs(5)).await
}
async fn handler(State(state): State<Mock>, req: Request<Body>) -> Response<Body> {
    state.count.fetch_add(1, Ordering::SeqCst);
    let (parts, body) = req.into_parts();
    let body = to_bytes(body, d::MAX_FILE_BYTES).await.unwrap().to_vec();
    state
        .tx
        .send(Observed {
            method: parts.method.to_string(),
            uri: parts.uri.to_string(),
            body,
            content_type: parts
                .headers
                .get(CONTENT_TYPE)
                .map(|v| v.to_str().unwrap().to_owned()),
            authorized: parts.headers.get(AUTHORIZATION).unwrap() == "Bearer executor-secret",
        })
        .unwrap();
    if let Some(gate) = state.gate {
        gate.acquire().await.unwrap().forget();
    }
    Response::builder()
        .status(state.status)
        .header("location", "/must-not-follow")
        .header(RETRY_AFTER, "17")
        .body(Body::from(state.body))
        .unwrap()
}
fn plan(method: d::Method) -> d::PreparedCall {
    d::PreparedCall {
        endpoint: d::Endpoint::new(method, "/fixtures/{id}", ["fixtures", "x/y?z#ü"]).unwrap(),
        query: vec![("q".into(), "a&b".into()), ("q".into(), "2".into())],
        body: d::RequestBody::Empty,
        response: d::ResponseContract {
            statuses: &[200, 201],
            kind: d::ResponseKind::Json,
        },
        projection: d::Projection {
            context: Value::Null,
            project: |response, _| {
                let d::ResponseBody::Json(data) = response.body else {
                    unreachable!()
                };
                Ok(d::Projected {
                    receipt: Some(d::Receipt(
                        serde_json::from_value(json!({"id": data["id"]})).unwrap(),
                    )),
                    data,
                    links: vec![],
                    omissions: vec![],
                })
            },
        },
        noncommitting_errors: &[],
    }
}
async fn execute(server: &Server, plan: d::PreparedCall) -> Value {
    let result = server
        .client
        .execute_prepared(plan, CancellationToken::new())
        .await;
    assert!(serialized_size(&result) <= d::MAX_RESULT_BYTES);
    let value = result.structured_content.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.content[0].as_text().unwrap().text).unwrap(),
        value
    );
    assert_eq!(result.is_error, Some(value.get("error").is_some()));
    value
}
fn outcome(value: &Value) -> &str {
    value["meta"]["write"]["outcome"].as_str().unwrap()
}
fn once(server: &Server) {
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn methods_bodies_encoding_and_authorization_send_exactly_one_request() {
    for method in [
        d::Method::Get,
        d::Method::Post,
        d::Method::Patch,
        d::Method::Put,
        d::Method::Delete,
    ] {
        let mut server = mock(200, br#"{"id":"native-id"}"#.to_vec()).await;
        let mut call = plan(method);
        call.body = match method {
            d::Method::Patch | d::Method::Post => {
                d::RequestBody::Json(json!({"text":"verbatim\nü"}))
            }
            d::Method::Put => d::RequestBody::Bytes {
                bytes: vec![0, 255, 10],
                content_type: d::ContentType::new("application/custom".into()).unwrap(),
            },
            _ => d::RequestBody::Empty,
        };
        let value = execute(&server, call).await;
        assert_eq!(value["data"]["id"], "native-id");
        if method.is_write() {
            assert_eq!(outcome(&value), "confirmed");
        } else {
            assert!(value["meta"].get("write").is_none());
        }
        let req = server.rx.recv().await.unwrap();
        assert_eq!(
            req.method,
            serde_json::to_value(method).unwrap().as_str().unwrap()
        );
        assert_eq!(req.uri, "/api/v1/fixtures/x%2Fy%3Fz%23%C3%BC?q=a%26b&q=2");
        assert!(req.authorized);
        match method {
            d::Method::Patch | d::Method::Post => {
                assert_eq!(
                    serde_json::from_slice::<Value>(&req.body).unwrap(),
                    json!({"text":"verbatim\nü"})
                );
                assert_eq!(req.content_type.as_deref(), Some("application/json"));
            }
            d::Method::Put => {
                assert_eq!(req.body, vec![0, 255, 10]);
                assert_eq!(req.content_type.as_deref(), Some("application/custom"));
            }
            _ => assert!(req.body.is_empty()),
        }
        once(&server);
    }
}

#[tokio::test]
async fn bytes_and_expected_204_project_without_another_request() {
    for empty in [false, true] {
        let server = mock(
            if empty { 204 } else { 200 },
            if empty { vec![] } else { vec![0, 255] },
        )
        .await;
        let mut call = plan(d::Method::Delete);
        call.response = d::ResponseContract {
            statuses: if empty { &[204] } else { &[200] },
            kind: if empty {
                d::ResponseKind::Empty
            } else {
                d::ResponseKind::Bytes { max_bytes: 2 }
            },
        };
        call.projection.project = |response, _| {
            Ok(d::Projected {
                data: match response.body {
                    d::ResponseBody::Empty => Value::Null,
                    d::ResponseBody::Bytes(bytes) => json!(bytes),
                    _ => unreachable!(),
                },
                receipt: Some(d::Receipt::default()),
                links: vec![],
                omissions: vec![],
            })
        };
        let value = execute(&server, call).await;
        assert_eq!(outcome(&value), "confirmed");
        assert!(value.get("error").is_none());
        if !empty {
            assert_eq!(value["data"], json!([0, 255]));
        }
        once(&server);
    }
}

#[tokio::test]
async fn redirects_rate_limits_and_native_errors_do_not_retry_or_infer_rejection() {
    for status in [302, 400, 401, 403, 404, 408, 409, 429, 500, 503] {
        let server = mock(status, br#"{"error":{"code":"native.conflict/code","message":"Native message","details":{"current_item":{"value":"keep me"},"allowed_transitions":["Go"],"requirements":[{"permission":"write"}]}}}"#.to_vec()).await;
        let value = execute(&server, plan(d::Method::Post)).await;
        assert_eq!(outcome(&value), "unknown");
        assert_eq!(value["meta"]["http_status"], status);
        assert_eq!(value["meta"]["retry_after"], "17");
        assert_eq!(value["error"]["code"], "native.conflict/code");
        assert_eq!(
            value["error"]["details"]["current_item"]["value"],
            "keep me"
        );
        assert_eq!(
            value["error"]["details"]["requirements"][0]["permission"],
            "write"
        );
        once(&server);
    }
}

#[tokio::test]
async fn explicit_native_commitment_and_proven_endpoint_rule_control_outcomes() {
    for committed in [None, Some(false), Some(true)] {
        let server = mock(409, serde_json::to_vec(&json!({"error":{"code":"version_conflict","message":"Conflict","details": committed.map(|v| json!({"committed":v}))}})).unwrap()).await;
        let mut call = plan(d::Method::Patch);
        call.noncommitting_errors = &[d::NoncommittingError {
            status: 409,
            code: "version_conflict",
            native_test: "pinned regression proof",
        }];
        let value = execute(&server, call).await;
        assert_eq!(
            outcome(&value),
            if committed == Some(true) {
                "confirmed"
            } else {
                "rejected"
            }
        );
        assert_eq!(value["meta"]["write"]["committed"], json!(committed));
        assert!(value.get("error").is_some());
        once(&server);
    }
}

#[tokio::test]
async fn malformed_or_oversized_responses_and_unexpected_status_are_ambiguous() {
    for (status, body, kind, code) in [
        (
            200,
            b"not-json".to_vec(),
            d::ResponseKind::Json,
            "invalid_upstream_json",
        ),
        (
            500,
            b"not-json".to_vec(),
            d::ResponseKind::Json,
            "upstream_error",
        ),
        (
            200,
            vec![0; 3],
            d::ResponseKind::Bytes { max_bytes: 2 },
            "upstream_body_too_large",
        ),
        (
            200,
            vec![b' '; d::MAX_SUCCESS_JSON_BYTES + 1],
            d::ResponseKind::Json,
            "upstream_body_too_large",
        ),
        (
            500,
            vec![b' '; d::MAX_ERROR_BYTES + 1],
            d::ResponseKind::Json,
            "upstream_body_too_large",
        ),
        (
            202,
            b"{}".to_vec(),
            d::ResponseKind::Json,
            "unexpected_status",
        ),
    ] {
        let server = mock(status, body).await;
        let mut call = plan(d::Method::Post);
        call.response.kind = kind;
        let value = execute(&server, call).await;
        assert_eq!(value["error"]["code"], code);
        assert_eq!(value["meta"]["http_status"], status);
        assert_eq!(outcome(&value), "unknown");
        once(&server);
    }
}

#[tokio::test]
async fn output_overflow_is_success_with_compact_receipt_and_full_wire_budget() {
    let server = mock(
        201,
        serde_json::to_vec(&json!({"id":"created-id","text":"\"\\".repeat(100*1024)})).unwrap(),
    )
    .await;
    let value = execute(&server, plan(d::Method::Post)).await;
    assert_eq!(value["data"], Value::Null);
    assert!(value.get("error").is_none());
    assert_eq!(outcome(&value), "confirmed");
    assert_eq!(value["meta"]["write"]["receipt"]["id"], "created-id");
    assert!(!value["meta"]["omissions"].as_array().unwrap().is_empty());
    once(&server);
}

#[tokio::test]
async fn bounded_details_preserve_native_code_and_commitment_and_redact_all_strings() {
    for committed in [false, true] {
        let server = mock(409, serde_json::to_vec(&json!({"error":{"code":"version_conflict","message":"executor-secret conflict","details":{"committed":committed,"executor-secret":{"nested":["executor-secret", "\"\\".repeat(100*1024)]}}}})).unwrap()).await;
        let value = execute(&server, plan(d::Method::Patch)).await;
        assert_eq!(value["error"]["code"], "version_conflict");
        assert_eq!(value["meta"]["write"]["committed"], committed);
        assert_eq!(
            outcome(&value),
            if committed { "confirmed" } else { "rejected" }
        );
        assert!(!value.to_string().contains("executor-secret"));
        assert_eq!(value["error"]["details"], json!({"committed":committed}));
        assert!(!value["meta"]["omissions"].as_array().unwrap().is_empty());
        once(&server);
    }
    let server = mock(
        200,
        br#"{"id":"executor-secret","executor-secret":{"nested":"executor-secret"}}"#.to_vec(),
    )
    .await;
    let value = execute(&server, plan(d::Method::Post)).await;
    assert!(!value.to_string().contains("executor-secret"));
    assert_eq!(value["meta"]["write"]["receipt"]["id"], "[REDACTED]");
    once(&server);
}

#[tokio::test]
async fn projection_failure_preserves_validated_receipt_but_is_still_an_error() {
    let server = mock(200, b"{}".to_vec()).await;
    let mut call = plan(d::Method::Post);
    call.projection.project = |_, _| {
        Err(
            d::ProjectionError::new("projection_failed", "Optional projection failed")
                .with_receipt(d::Receipt(
                    serde_json::from_value(json!({"id":"native-id"})).unwrap(),
                )),
        )
    };
    let value = execute(&server, call).await;
    assert_eq!(outcome(&value), "confirmed");
    assert_eq!(value["meta"]["write"]["receipt"]["id"], "native-id");
    assert_eq!(value["error"]["code"], "projection_failed");
    once(&server);
}

#[tokio::test]
async fn cancellation_before_and_after_dispatch_and_deadlines_never_retry() {
    for mode in [0, 1, 2] {
        let mut server = serve(
            200,
            b"{}".to_vec(),
            Some(Arc::new(Semaphore::new(0))),
            if mode == 2 {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(5)
            },
        )
        .await;
        let cancellation = CancellationToken::new();
        if mode == 0 {
            cancellation.cancel();
        }
        let token = cancellation.clone();
        let client = server.client.clone();
        let task =
            tokio::spawn(
                async move { client.execute_prepared(plan(d::Method::Post), token).await },
            );
        if mode == 1 {
            timeout(Duration::from_secs(1), server.rx.recv())
                .await
                .unwrap()
                .unwrap();
            cancellation.cancel();
        }
        let value = task.await.unwrap().structured_content.unwrap();
        assert_eq!(
            outcome(&value),
            if mode == 0 {
                "not_attempted"
            } else {
                "unknown"
            }
        );
        assert_eq!(
            value["error"]["code"],
            if mode == 2 {
                "upstream_timeout"
            } else {
                "request_cancelled"
            }
        );
        assert_eq!(server.count.load(Ordering::SeqCst), usize::from(mode != 0));
    }
}

#[tokio::test]
async fn dropped_response_does_not_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let recorded = count.clone();
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 8192];
            loop {
                socket.readable().await.unwrap();
                match socket.try_read(&mut bytes) {
                    Ok(count) => {
                        assert!(count > 0);
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                    Err(error) => panic!("mock read failed: {error}"),
                }
            }
            recorded.fetch_add(1, Ordering::SeqCst);
            drop(socket);
        }
    });
    let value = client(&url, Duration::from_secs(5))
        .execute_prepared(plan(d::Method::Post), CancellationToken::new())
        .await
        .structured_content
        .unwrap();
    assert_eq!(outcome(&value), "unknown");
    assert_eq!(value["error"]["code"], "upstream_transport_error");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn dispatcher_prepares_locally_and_rejected_plans_send_zero_requests() {
    fn definitions(_: &d::LocalLimits) -> Vec<rmcp::model::Tool> {
        vec![
            rmcp::model::Tool::new("fixture", "fixture", Arc::new(Map::new()))
                .with_annotations(rmcp::model::ToolAnnotations::new().read_only(false)),
        ]
    }
    fn prepare(
        _: &str,
        args: Map<String, Value>,
        _: &d::LocalLimits,
    ) -> Result<d::PreparedCall, d::LocalError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            valid: bool,
        }
        let args: Args = d::arguments(args)?;
        if !args.valid {
            return Err(d::LocalError::invalid("valid", "must be true"));
        }
        Ok(plan(d::Method::Post))
    }
    let modules = [tools::Module {
        name: "fixture",
        definitions,
        prepare,
    }];
    let server = mock(200, br#"{"id":"native-id"}"#.to_vec()).await;
    for args in [json!({"valid":false}), json!({"valid":true,"extra":1})] {
        let value = tools::dispatch_in(
            &modules,
            &server.client,
            "fixture",
            args.as_object().unwrap().clone(),
            &d::LocalLimits::default(),
            CancellationToken::new(),
        )
        .await
        .structured_content
        .unwrap();
        assert_eq!(outcome(&value), "not_attempted");
        assert_eq!(value["error"]["code"], "invalid_input");
        assert_eq!(server.count.load(Ordering::SeqCst), 0);
    }
    let value = tools::dispatch_in(
        &modules,
        &server.client,
        "fixture",
        json!({"valid":true}).as_object().unwrap().clone(),
        &d::LocalLimits::default(),
        CancellationToken::new(),
    )
    .await
    .structured_content
    .unwrap();
    assert_eq!(outcome(&value), "confirmed");
    once(&server);
}

#[tokio::test]
async fn body_disconnect_cancellation_and_deadline_keep_received_status() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for mode in [0, 1, 2] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let recorded = count.clone();
        let (sent, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            recorded.fetch_add(1, Ordering::SeqCst);
            socket
                .write_all(
                    b"HTTP/1.1 201 Created\r\nContent-Length: 100\r\nRetry-After: 17\r\n\r\n{",
                )
                .await
                .unwrap();
            sent.send(()).unwrap();
            if mode != 0 {
                std::future::pending::<()>().await;
            }
            // Keep accepting to make accidental retries observable.
            drop(socket);
            loop {
                let (_socket, _) = listener.accept().await.unwrap();
                recorded.fetch_add(1, Ordering::SeqCst);
            }
        });
        let cancellation = CancellationToken::new();
        let token = cancellation.clone();
        let client = client(
            &url,
            if mode == 2 {
                Duration::from_millis(150)
            } else {
                Duration::from_secs(5)
            },
        );
        let task =
            tokio::spawn(
                async move { client.execute_prepared(plan(d::Method::Post), token).await },
            );
        received.await.unwrap();
        if mode == 1 {
            // Wait for the client to consume headers before cancelling the body.
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancellation.cancel();
        }
        let value = timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(outcome(&value), "unknown");
        assert_eq!(value["meta"]["http_status"], 201);
        assert_eq!(value["meta"]["retry_after"], "17");
        assert_eq!(
            value["error"]["code"],
            match mode {
                0 => "upstream_transport_error",
                1 => "request_cancelled",
                _ => "upstream_timeout",
            }
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn admission_rejects_fifth_call_without_sending_or_queuing() {
    let mut server = serve(
        200,
        b"{}".to_vec(),
        Some(Arc::new(Semaphore::new(0))),
        Duration::from_secs(5),
    )
    .await;
    let cancellation = CancellationToken::new();
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let client = server.client.clone();
        let token = cancellation.clone();
        tasks.push(tokio::spawn(async move {
            client.execute_prepared(plan(d::Method::Post), token).await
        }));
    }
    for _ in 0..4 {
        timeout(Duration::from_secs(1), server.rx.recv())
            .await
            .unwrap()
            .unwrap();
    }
    let value = execute(&server, plan(d::Method::Post)).await;
    assert_eq!(value["error"]["code"], "tool_concurrency_limit");
    assert_eq!(outcome(&value), "not_attempted");
    assert_eq!(server.count.load(Ordering::SeqCst), 4);
    cancellation.cancel();
    for task in tasks {
        assert_eq!(
            outcome(&task.await.unwrap().structured_content.unwrap()),
            "unknown"
        );
    }
}

#[tokio::test]
async fn small_native_details_redact_nested_keys_and_values_without_filtering_them() {
    let server = mock(403, br#"{"error":{"code":"permission_denied","message":"executor-secret","details":{"executor-secret":{"nested":["executor-secret"]},"permissions":{"domain":"project","access":"write"},"committed":false}}}"#.to_vec()).await;
    let value = execute(&server, plan(d::Method::Patch)).await;
    assert_eq!(value["error"]["message"], "[REDACTED]");
    assert_eq!(
        value["error"]["details"]["[REDACTED]"]["nested"],
        json!(["[REDACTED]"])
    );
    assert_eq!(
        value["error"]["details"]["permissions"],
        json!({"domain":"project","access":"write"})
    );
    assert_eq!(outcome(&value), "rejected");
    assert!(!value.to_string().contains("executor-secret"));
    once(&server);
}

#[tokio::test]
async fn success_without_a_validated_write_receipt_is_unknown() {
    let server = mock(200, b"{}".to_vec()).await;
    let mut call = plan(d::Method::Post);
    call.projection.project = |_, _| {
        Ok(d::Projected {
            data: json!({"id":"unvalidated"}),
            receipt: None,
            links: vec![],
            omissions: vec![],
        })
    };
    let value = execute(&server, call).await;
    assert_eq!(value["error"]["code"], "invalid_receipt");
    assert_eq!(outcome(&value), "unknown");
    once(&server);
}

#[tokio::test]
async fn pathological_optional_metadata_and_receipt_fields_cannot_break_output_bound() {
    for huge_receipt in [false, true] {
        let server = mock(200, b"{}".to_vec()).await;
        let mut call = plan(d::Method::Post);
        call.projection.context = json!(huge_receipt);
        call.projection.project = |_, huge_receipt| {
            let mut receipt =
                serde_json::from_value::<Map<String, Value>>(json!({"id":"native-id"})).unwrap();
            if huge_receipt == &json!(true) {
                receipt.insert("oversized".into(), json!("x".repeat(600 * 1024)));
            }
            Ok(d::Projected {
                data: Value::Null,
                receipt: Some(d::Receipt(receipt)),
                links: vec![json!("x".repeat(600 * 1024))],
                omissions: vec![],
            })
        };
        let value = execute(&server, call).await;
        assert_eq!(outcome(&value), "confirmed");
        assert_eq!(value["meta"]["write"]["receipt"]["id"], "native-id");
        assert!(value["meta"]["write"]["receipt"].get("oversized").is_none());
        assert!(value["meta"]["links"].as_array().unwrap().is_empty());
        assert_eq!(
            value["meta"]["omissions"]
                .to_string()
                .contains("Oversized receipt"),
            huge_receipt
        );
        once(&server);
    }
}

#[tokio::test]
async fn rate_limit_rejects_after_twenty_without_dispatch_and_refills() {
    // Control the token bucket clock without pausing real socket I/O.
    let mut server = mock(200, br#"{"id":"returned"}"#.to_vec()).await;
    for _ in 0..20 {
        server
            .client
            .inner
            .rate_limit
            .bucket
            .lock()
            .unwrap()
            .last_updated = Instant::now() + Duration::from_secs(60);
        let value = execute(&server, plan(d::Method::Get)).await;
        assert!(value.get("error").is_none());
        server.rx.recv().await.unwrap();
    }
    let rejected = execute(&server, plan(d::Method::Post)).await;
    assert_eq!(rejected["error"]["code"], "rate_limited");
    assert_eq!(rejected["meta"]["write"]["outcome"], "not_attempted");
    assert_eq!(server.count.load(Ordering::SeqCst), 20);
    server
        .client
        .inner
        .rate_limit
        .bucket
        .lock()
        .unwrap()
        .last_updated = Instant::now() - Duration::from_secs(1);
    let value = execute(&server, plan(d::Method::Get)).await;
    assert!(value.get("error").is_none());
    assert_eq!(server.count.load(Ordering::SeqCst), 21);
}

#[test]
fn complete_result_budget_counts_escaping_and_both_representations() {
    for text in ["a", "\"\\\n"] {
        let render = |size| mcp_result(json!({"data":text.repeat(size)}), false);
        let mut low = 0;
        let mut high = d::MAX_RESULT_BYTES;
        while low + 1 < high {
            let middle = (low + high) / 2;
            if serialized_size(&render(middle)) <= d::MAX_RESULT_BYTES {
                low = middle;
            } else {
                high = middle;
            }
        }
        let result = render(low);
        assert_eq!(
            serialized_size(&result),
            serde_json::to_vec(&result).unwrap().len()
        );
        assert!(serialized_size(&result) <= d::MAX_RESULT_BYTES);
        assert!(serialized_size(&render(high)) > d::MAX_RESULT_BYTES);
    }
}
