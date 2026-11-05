//! Executed adapter acceptance plus two ignored, unrun historical native scenarios.
//! See docs/testing/context-recovery.md for the revised evidence boundary.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::Request,
    response::Response,
    routing::any,
};
use serde_json::{Value, json};
use tines_mcp::{
    config::Config,
    tines::TinesClient,
    tools::{self, contract::LocalLimits},
};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct Observed {
    method: String,
    uri: String,
    body: Value,
    authorization: String,
}
#[derive(Clone)]
struct Reply {
    status: u16,
    body: String,
}
#[derive(Clone)]
struct Mock {
    requests: Arc<Mutex<Vec<Observed>>>,
    replies: Arc<Mutex<VecDeque<Reply>>>,
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
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let authorization = request.headers()["authorization"]
        .to_str()
        .unwrap()
        .to_owned();
    let bytes = to_bytes(request.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    state.requests.lock().unwrap().push(Observed {
        method,
        uri,
        body,
        authorization,
    });
    // Catch every request, including unexpected authority/preflight/retry calls.
    let reply = state.replies.lock().unwrap().pop_front().unwrap_or(Reply {
        status: 500,
        body: r#"{"error":{"code":"unexpected_request","message":"unexpected request"}}"#.into(),
    });
    Response::builder()
        .status(reply.status)
        .header("content-type", "application/json")
        .header("retry-after", "7")
        .header("location", "/unexpected-redirect")
        .body(Body::from(reply.body))
        .unwrap()
}
async fn upstream(replies: impl IntoIterator<Item = (u16, Value)>) -> Upstream {
    let state = Mock {
        requests: Default::default(),
        replies: Arc::new(Mutex::new(
            replies
                .into_iter()
                .map(|(status, body)| Reply {
                    status,
                    body: body.to_string(),
                })
                .collect(),
        )),
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
fn client(url: &str, token: &str) -> TinesClient {
    let config = Config::from_lookup(|name| match name {
        "TINES_BASE_URL" => Some(url.into()),
        "TINES_API_TOKEN" => Some(token.into()),
        "TINES_ALLOW_HTTP" => Some("true".into()),
        _ => None,
    })
    .unwrap();
    TinesClient::new(config.tines.as_ref().unwrap()).unwrap()
}
async fn call(client: &TinesClient, name: &str, args: Value) -> Value {
    let result = tools::dispatch(
        client,
        name,
        args.as_object().unwrap().clone(),
        &LocalLimits::default(),
        CancellationToken::new(),
    )
    .await;
    let value = serde_json::to_value(result).unwrap();
    let envelope: Value =
        serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(envelope, value["structuredContent"]);
    assert!(serde_json::to_vec(&value).unwrap().len() <= 512 * 1024);
    value
}
fn envelope(value: &Value) -> &Value {
    &value["structuredContent"]
}
fn assert_write(value: &Value, outcome: &str) {
    assert_eq!(
        envelope(value)["meta"]["write"]["outcome"],
        outcome,
        "{value}"
    );
}
fn assert_requests(upstream: &Upstream, expected: &[(&str, &str, Value)]) {
    let requests = upstream.state.requests.lock().unwrap();
    assert_eq!(requests.len(), expected.len(), "{requests:?}");
    for (actual, (method, uri, body)) in requests.iter().zip(expected) {
        assert_eq!(actual.method, *method);
        assert_eq!(actual.uri, *uri);
        assert_eq!(actual.body, *body);
        assert_eq!(actual.authorization, "Bearer acceptance-token");
    }
}

#[tokio::test]
async fn every_context_and_operational_success_uses_exactly_one_native_request() {
    for source in [
        include_str!("fixtures/context.json"),
        include_str!("fixtures/operations-read.json"),
        include_str!("fixtures/recovery.json"),
    ] {
        let fixtures: Value = serde_json::from_str(source).unwrap();
        for fixture in fixtures["successes"].as_array().unwrap() {
            let name = fixture["tool"].as_str().unwrap();
            let args = fixture["arguments"].clone();
            let plan = tools::prepare(
                name,
                args.as_object().unwrap().clone(),
                &LocalLimits::default(),
            )
            .unwrap();
            let native = fixture["response"]["body"]["value"].clone();
            let status = fixture["response"]["status"].as_u64().unwrap() as u16;
            let upstream = upstream([(status, native.clone())]).await;
            let result = call(&client(&upstream.url, "acceptance-token"), name, args).await;
            assert_eq!(result["isError"], false, "{name}: {result}");
            assert_eq!(envelope(&result)["data"], native);
            assert_eq!(envelope(&result)["meta"]["http_status"], status);
            if let Some(receipt) = fixture.get("receipt") {
                assert_write(&result, "confirmed");
                assert_eq!(envelope(&result)["meta"]["write"]["receipt"], *receipt);
            } else {
                assert!(envelope(&result)["meta"].get("write").is_none());
            }
            let requests = upstream.state.requests.lock().unwrap();
            assert_eq!(requests.len(), 1, "{name}: {requests:?}");
            assert_eq!(
                requests[0].method,
                serde_json::to_value(plan.endpoint.method)
                    .unwrap()
                    .as_str()
                    .unwrap()
            );
            assert_eq!(requests[0].authorization, "Bearer acceptance-token");
            let expected_body = match plan.body {
                tools::contract::RequestBody::Json(v) => v,
                tools::contract::RequestBody::Empty => Value::Null,
                _ => panic!(),
            };
            assert_eq!(requests[0].body, expected_body);
            let mut expected_uri = format!("/api/v1/{}", plan.endpoint.segments().join("/"));
            if !plan.query.is_empty() {
                expected_uri.push('?');
                expected_uri.push_str(
                    &url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(&plan.query)
                        .finish(),
                );
            }
            assert_eq!(requests[0].uri, expected_uri, "{name}");
        }
    }
}

#[tokio::test]
async fn stale_context_witness_preserves_current_item_without_read_merge_or_retry() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/context.json")).unwrap();
    for fixture in fixtures["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["status"] == 409)
    {
        let name = fixture["tool"].as_str().unwrap();
        let native = fixture["error"].clone();
        let upstream = upstream([(409, json!({"error":native}))]).await;
        let args = if name == "edit_context" {
            json!({"context_id":"ctx_raw","body":"replacement","expected_version":1})
        } else {
            json!({"context_id":"ctx_raw","text":"append","expected_version":1})
        };
        let result = call(
            &client(&upstream.url, "acceptance-token"),
            name,
            args.clone(),
        )
        .await;
        assert_eq!(result["isError"], true);
        assert_eq!(envelope(&result)["error"], native);
        assert_eq!(envelope(&result)["meta"]["http_status"], 409);
        assert_eq!(
            envelope(&result)["error"]["details"]["current"]["version"],
            3
        );
        assert_write(
            &result,
            if name == "edit_context" {
                "rejected"
            } else {
                "unknown"
            },
        );
        let mut body = args;
        body.as_object_mut().unwrap().remove("context_id");
        assert_requests(
            &upstream,
            &[(
                if name == "edit_context" {
                    "PATCH"
                } else {
                    "POST"
                },
                if name == "edit_context" {
                    "/api/v1/context/ctx_raw"
                } else {
                    "/api/v1/context/ctx_raw/append"
                },
                body,
            )],
        );
    }
}

#[tokio::test]
async fn native_permission_and_recovery_errors_are_forwarded_once_without_authority_preflight() {
    for (source, fallback_args) in [
        (
            include_str!("fixtures/context.json"),
            json!({"context_id":"ctx_raw","expected_version":1}),
        ),
        (include_str!("fixtures/recovery.json"), Value::Null),
        (include_str!("fixtures/operations-read.json"), json!({})),
    ] {
        let fixtures: Value = serde_json::from_str(source).unwrap();
        for fixture in fixtures["errors"].as_array().unwrap() {
            let name = fixture["tool"].as_str().unwrap();
            let args = match name {
                "resume_issue" => json!({"issue_id":"iss_target"}),
                "cancel_run" => json!({"run_id":"arun_target"}),
                "get_runner" => json!({"runner_id":"rnr_local"}),
                "append_context" => json!({"context_id":"ctx_raw","text":"x","expected_version":1}),
                _ => fallback_args.clone(),
            };
            let status = fixture["status"].as_u64().unwrap() as u16;
            let upstream = upstream([(status, json!({"error":fixture["error"]}))]).await;
            let result = call(&client(&upstream.url, "acceptance-token"), name, args).await;
            assert_eq!(result["isError"], true, "{result}");
            assert_eq!(envelope(&result)["error"], fixture["error"], "{name}");
            assert_eq!(envelope(&result)["meta"]["http_status"], status);
            assert_eq!(upstream.state.requests.lock().unwrap().len(), 1, "{name}");
        }
    }
}

#[tokio::test]
async fn cancellation_acceptance_and_lifecycle_inspection_are_separate_calls() {
    let pending = json!({"id":"arun_target","issue_id":"iss_target","status":"running","outcome":null,"cancel_requested_at":42});
    let ended = json!({"id":"arun_target","issue_id":"iss_target","status":"canceled","outcome":"interrupted","ended_at":43});
    let upstream = upstream([(200, pending.clone()), (200, ended.clone())]).await;
    let client = client(&upstream.url, "acceptance-token");
    let accepted = call(&client, "cancel_run", json!({"run_id":"arun_target"})).await;
    assert_write(&accepted, "confirmed");
    assert_eq!(envelope(&accepted)["data"], pending);
    assert_eq!(
        envelope(&accepted)["meta"]["write"]["receipt"]["status"],
        "running"
    );
    assert_requests(
        &upstream,
        &[("POST", "/api/v1/runs/arun_target/cancel", Value::Null)],
    );
    let observed = call(&client, "get_run", json!({"run_id":"arun_target"})).await;
    assert_eq!(observed["isError"], false);
    assert_eq!(envelope(&observed)["data"], ended);
    assert!(envelope(&observed)["meta"].get("write").is_none());
    assert_requests(
        &upstream,
        &[
            ("POST", "/api/v1/runs/arun_target/cancel", Value::Null),
            ("GET", "/api/v1/runs/arun_target", Value::Null),
        ],
    );
}

#[tokio::test]
async fn reserved_path_and_project_query_are_encoded_once_and_patch_nulls_survive() {
    let upstream = upstream([
        (200, json!({"id":"ctx/a?b#%ü","kind":"repo","version":2})),
        (200, json!({"groups":[],"total":0})),
    ])
    .await;
    let client = client(&upstream.url, "acceptance-token");
    let result = call(
        &client,
        "edit_context",
        json!({"context_id":"ctx/a?b#%ü","project_id":null,"repo_branch":"","expected_version":1}),
    )
    .await;
    assert_write(&result, "confirmed");
    assert_eq!(
        call(
            &client,
            "get_supervisor_queue",
            json!({"project":"project /?& ü"})
        )
        .await["isError"],
        false
    );
    assert_requests(
        &upstream,
        &[
            (
                "PATCH",
                "/api/v1/context/ctx%2Fa%3Fb%23%25%C3%BC",
                json!({"project_id":null,"repo_branch":"","expected_version":1}),
            ),
            (
                "GET",
                "/api/v1/supervisor/queue?project=project+%2F%3F%26+%C3%BC",
                Value::Null,
            ),
        ],
    );
}

#[tokio::test]
async fn invalid_arguments_and_predispatch_cancellation_send_nothing() {
    let upstream = upstream([]).await;
    let client = client(&upstream.url, "acceptance-token");
    for (name, args) in [
        (
            "create_context",
            json!({"kind":"prompt","name":"x","body":"x","extra":true}),
        ),
        ("edit_context", json!({"context_id":".."})),
        (
            "append_context",
            json!({"context_id":"ctx_raw","text":null}),
        ),
        ("resume_issue", json!({"issue_id":"iss_target","wait":true})),
        ("cancel_run", json!({"run_id":"arun_target","force":true})),
        ("get_supervisor_queue", json!({"project":null})),
        ("list_runners", json!({"project":"p"})),
        ("get_runner", json!({"runner_id":".."})),
        ("get_supervisor_settings", json!({"project":"p"})),
    ] {
        let result = call(&client, name, args).await;
        assert_eq!(result["isError"], true, "{name}: {result}");
        assert!(envelope(&result)["meta"]["http_status"].is_null());
    }
    for (name, args) in [
        ("resume_issue", json!({"issue_id":"i"})),
        ("cancel_run", json!({"run_id":"r"})),
    ] {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = tools::dispatch(
            &client,
            name,
            args.as_object().unwrap().clone(),
            &LocalLimits::default(),
            cancellation,
        )
        .await;
        assert_write(&serde_json::to_value(result).unwrap(), "not_attempted");
    }
    assert_requests(&upstream, &[]);
}

#[tokio::test]
async fn dispatched_writes_never_retry_http_failures_redirects_or_invalid_successes() {
    for name in [
        "create_context",
        "edit_context",
        "append_context",
        "resume_issue",
        "cancel_run",
    ] {
        for status in [302, 401, 403, 404, 409, 422, 429, 500, 503, 200] {
            let args = match name {
                "create_context" => json!({"kind":"prompt","name":"x","body":"x","project_id":"p"}),
                "edit_context" => json!({"context_id":"c","body":"x","expected_version":1}),
                "append_context" => json!({"context_id":"c","text":"x","expected_version":1}),
                "resume_issue" => json!({"issue_id":"i"}),
                _ => json!({"run_id":"r"}),
            };
            let upstream = upstream([(status, json!({"error":{"code":"native_failure","message":"failed","details":{"domain":"control_plane","access":"write"}}}))]).await;
            let result = call(&client(&upstream.url, "acceptance-token"), name, args).await;
            assert_eq!(result["isError"], true);
            assert_eq!(envelope(&result)["meta"]["http_status"], status);
            assert_write(&result, "unknown");
            assert_eq!(upstream.state.requests.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn cancellation_after_dispatch_is_unknown_and_does_not_send_a_recovery_request() {
    use tokio::{
        io::AsyncReadExt,
        sync::oneshot,
        time::{Duration, timeout},
    };
    for (name, args, target) in [
        (
            "edit_context",
            json!({"context_id":"c","body":"x","expected_version":1}),
            "PATCH /api/v1/context/c ",
        ),
        (
            "append_context",
            json!({"context_id":"c","text":"x","expected_version":1}),
            "POST /api/v1/context/c/append ",
        ),
        (
            "resume_issue",
            json!({"issue_id":"i"}),
            "POST /api/v1/issues/i/resume ",
        ),
        (
            "cancel_run",
            json!({"run_id":"r"}),
            "POST /api/v1/runs/r/cancel ",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = oneshot::channel();
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buffer[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            tx.send(String::from_utf8(request).unwrap()).unwrap();
            // Keep the first response open and detect any second connection.
            assert!(
                timeout(Duration::from_millis(150), listener.accept())
                    .await
                    .is_err()
            );
        });
        let client = client(&url, "acceptance-token");
        let cancellation = CancellationToken::new();
        let token = cancellation.clone();
        let operation = tokio::spawn(async move {
            tools::dispatch(
                &client,
                name,
                args.as_object().unwrap().clone(),
                &LocalLimits::default(),
                token,
            )
            .await
        });
        let request = timeout(Duration::from_secs(5), rx).await.unwrap().unwrap();
        assert!(request.starts_with(target), "{request}");
        cancellation.cancel();
        let result = timeout(Duration::from_secs(5), operation)
            .await
            .unwrap()
            .unwrap();
        let result = serde_json::to_value(result).unwrap();
        assert_eq!(result["isError"], true);
        assert_write(&result, "unknown");
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn recovery_response_loss_and_truncation_never_resend_or_inspect_automatically() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        time::{Duration, timeout},
    };
    for (name, args, target) in [
        (
            "resume_issue",
            json!({"issue_id":"i"}),
            "POST /api/v1/issues/i/resume ",
        ),
        (
            "cancel_run",
            json!({"run_id":"r"}),
            "POST /api/v1/runs/r/cancel ",
        ),
    ] {
        for truncated in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let peer = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                assert!(String::from_utf8(bytes).unwrap().starts_with(target));
                if truncated {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nRetry-After: 7\r\nContent-Length: 500\r\n\r\n{\"id\":").await.unwrap();
                }
                socket.shutdown().await.unwrap();
                drop(socket);
                assert!(
                    timeout(Duration::from_millis(150), listener.accept())
                        .await
                        .is_err()
                );
            });
            let result = call(&client(&url, "acceptance-token"), name, args.clone()).await;
            assert_eq!(result["isError"], true);
            assert_write(&result, "unknown");
            if truncated {
                assert_eq!(envelope(&result)["meta"]["http_status"], 200);
                assert_eq!(envelope(&result)["meta"]["retry_after"], "7");
            }
            peer.await.unwrap();
        }
    }
}

// These checks deliberately have no fallback to the supervisor's TINES_API_KEY.
// Historical, unrun scenarios; outside the revised mcp-14 gate.
// Do not provision or run them for this issue. See docs/testing/context-recovery.md.
fn native_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name} for the disposable native scenario"))
}
fn native_url() -> String {
    assert_eq!(native_env("MCP14_NATIVE_DISPOSABLE"), "yes");
    let base = native_env("MCP14_NATIVE_BASE_URL");
    let parsed = url::Url::parse(&base).unwrap();
    assert_eq!(parsed.scheme(), "http");
    assert!(matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]")
    ));
    assert_eq!(parsed.path(), "/");
    assert!(parsed.username().is_empty() && parsed.password().is_none());
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
    base
}
async fn assert_native_revision(base: &str) {
    // Explicit setup read outside any tool call; not an authority preflight.
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .unwrap()
        .get(format!("{}/api/version", base.trim_end_matches('/')))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let identity: Value = response.json().await.unwrap();
    assert_eq!(
        identity["commit"],
        "af6cb25febf1b45d7eff32251f78fcf6355aed4a"
    );
}
fn native_success(result: &Value) -> &Value {
    assert_eq!(result["isError"], false, "{result}");
    &envelope(result)["data"]
}
fn native_denial(result: &Value, code: &str, domain: Option<&str>) {
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(envelope(result)["meta"]["http_status"], 403, "{result}");
    assert_eq!(envelope(result)["error"]["code"], code, "{result}");
    if let Some(domain) = domain {
        assert_eq!(envelope(result)["error"]["details"]["domain"], domain);
    }
}

#[tokio::test]
#[ignore = "unrun historical native scenario; outside revised gate; see docs/testing/context-recovery.md"]
async fn native_context_versions_scopes_and_run_key_authority() {
    let url = native_url();
    assert_native_revision(&url).await;
    let owner = client(&url, &native_env("MCP14_NATIVE_OWNER_TOKEN"));
    let scoped = client(&url, &native_env("MCP14_NATIVE_PROJECT_TOKEN"));
    let run = client(&url, &native_env("MCP14_NATIVE_RUN_TOKEN"));
    let project = native_env("MCP14_NATIVE_PROJECT_ID");
    let other_project = native_env("MCP14_NATIVE_OTHER_PROJECT_ID");
    let own_issue = native_env("MCP14_NATIVE_RUN_KEY_ISSUE_ID");
    let name = format!(
        "mcp14-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let created = call(
        &scoped,
        "create_context",
        json!({"kind":"prompt","name":name,"body":"first","project_id":project}),
    )
    .await;
    let data = native_success(&created);
    assert_write(&created, "confirmed");
    assert_eq!(data["scope"]["project_id"], project);
    let id = data["id"].as_str().unwrap();
    let version = data["version"].as_u64().unwrap();
    let edited = call(
        &scoped,
        "edit_context",
        json!({"context_id":id,"body":"current","expected_version":version}),
    )
    .await;
    let current = native_success(&edited).clone();
    assert_eq!(current["version"], version + 1);
    for (tool, field) in [("edit_context", "body"), ("append_context", "text")] {
        let conflict = call(
            &scoped,
            tool,
            json!({"context_id":id,field:"stale","expected_version":version}),
        )
        .await;
        assert_eq!(conflict["isError"], true);
        assert_eq!(envelope(&conflict)["meta"]["http_status"], 409);
        assert_eq!(envelope(&conflict)["error"]["code"], "version_conflict");
        assert_eq!(envelope(&conflict)["error"]["details"]["current"], current);
        let read = call(&scoped, "get_context", json!({"context_id":id})).await;
        assert_eq!(*native_success(&read), current);
    }
    let appended = call(
        &scoped,
        "append_context",
        json!({"context_id":id,"text":"next","expected_version":version+1}),
    )
    .await;
    assert_eq!(native_success(&appended)["version"], version + 2);
    assert_eq!(native_success(&appended)["body"], "current\n\nnext");
    // Moving into global scope must independently require workspace write.
    let denied = call(
        &scoped,
        "edit_context",
        json!({"context_id":id,"project_id":null,"expected_version":version+2}),
    )
    .await;
    native_denial(&denied, "insufficient_permissions", Some("workspace"));
    let read = call(&owner, "get_context", json!({"context_id":id})).await;
    assert_eq!(native_success(&read)["scope"]["project_id"], project);
    assert_eq!(native_success(&read)["version"], version + 2);
    for args in [
        json!({"kind":"prompt","name":format!("{name}-global"),"body":"global"}),
        json!({"kind":"prompt","name":format!("{name}-other"),"body":"other","project_id":other_project}),
    ] {
        let domain = if args.get("project_id").is_some() {
            "project"
        } else {
            "workspace"
        };
        native_denial(
            &call(&scoped, "create_context", args).await,
            "insufficient_permissions",
            Some(domain),
        );
    }
    let issue_context = call(&run, "create_context", json!({"kind":"prompt","name":format!("{name}-own"),"body":"own issue","issue_id":own_issue})).await;
    assert_eq!(
        native_success(&issue_context)["scope"]["issue_id"],
        own_issue
    );
    native_denial(&call(&run, "create_context", json!({"kind":"prompt","name":format!("{name}-project"),"body":"project","project_id":project})).await, "run_key_forbidden", None);
    // With project write but no control-plane write, even env writes are native denials.
    native_denial(
        &call(
            &scoped,
            "create_context",
            json!({"kind":"env","name":"MCP14_FIXTURE","value":"synthetic","project_id":project}),
        )
        .await,
        "insufficient_permissions",
        Some("control_plane"),
    );
}

#[tokio::test]
#[ignore = "unrun historical native scenario; outside revised gate; see docs/testing/context-recovery.md"]
async fn native_operational_permissions_resume_and_cancellation_observations() {
    let url = native_url();
    assert_native_revision(&url).await;
    let owner = client(&url, &native_env("MCP14_NATIVE_OWNER_TOKEN"));
    let scoped = client(&url, &native_env("MCP14_NATIVE_PROJECT_TOKEN"));
    let run_key = client(&url, &native_env("MCP14_NATIVE_RUN_TOKEN"));
    let project = native_env("MCP14_NATIVE_PROJECT_ID");
    let parked = native_env("MCP14_NATIVE_PARKED_ISSUE_ID");
    let run_id = native_env("MCP14_NATIVE_CANCEL_RUN_ID");
    let runner_id = native_env("MCP14_NATIVE_RUNNER_ID");
    for (tool, args) in [
        ("list_runners", json!({})),
        ("get_runner", json!({"runner_id":runner_id})),
        ("get_supervisor_queue", json!({"project":project})),
        ("get_supervisor_settings", json!({})),
    ] {
        native_success(&call(&owner, tool, args.clone()).await);
        native_denial(
            &call(&scoped, tool, args).await,
            "insufficient_permissions",
            Some("control_plane"),
        );
    }
    let settings = call(&run_key, "get_supervisor_settings", json!({})).await;
    assert_eq!(native_success(&settings)["github_pat_hint"], Value::Null);
    assert!(
        native_success(&settings)
            .as_object()
            .unwrap()
            .contains_key("github_pat_hint")
    );
    let before = call(&owner, "get_issue", json!({"issue_id":parked})).await;
    assert_eq!(native_success(&before)["needs_attention"], true);
    assert!(native_success(&before)["attempt_count"].as_u64().unwrap() > 0);
    native_denial(
        &call(&scoped, "resume_issue", json!({"issue_id":parked})).await,
        "insufficient_permissions",
        Some("control_plane"),
    );
    native_denial(
        &call(&run_key, "resume_issue", json!({"issue_id":parked})).await,
        "run_key_forbidden",
        None,
    );
    let unchanged = call(&owner, "get_issue", json!({"issue_id":parked})).await;
    assert_eq!(native_success(&unchanged)["needs_attention"], true);
    assert_eq!(
        native_success(&unchanged)["attempt_count"],
        native_success(&before)["attempt_count"]
    );
    let resumed = call(&owner, "resume_issue", json!({"issue_id":parked})).await;
    assert_write(&resumed, "confirmed");
    assert_eq!(native_success(&resumed)["needs_attention"], false);
    assert_eq!(native_success(&resumed)["attempt_count"], 0);
    // Dispatch is observed independently; no lifecycle field is fabricated.
    native_success(&call(&owner, "explain_dispatch", json!({"issue_id":parked})).await);
    native_denial(
        &call(&scoped, "cancel_run", json!({"run_id":run_id})).await,
        "insufficient_permissions",
        Some("control_plane"),
    );
    native_denial(
        &call(&run_key, "cancel_run", json!({"run_id":run_id})).await,
        "run_key_forbidden",
        None,
    );
    let accepted = call(&owner, "cancel_run", json!({"run_id":run_id})).await;
    let pending = native_success(&accepted);
    assert_write(&accepted, "confirmed");
    assert!(
        matches!(pending["status"].as_str(), Some("launching" | "running")),
        "the shared running fixture must expose pending cancellation"
    );
    assert!(pending["outcome"].is_null());
    assert_eq!(
        envelope(&accepted)["meta"]["write"]["receipt"]["status"],
        envelope(&accepted)["data"]["status"]
    );
    let observed = call(&owner, "get_run", json!({"run_id":run_id})).await;
    let data = native_success(&observed);
    assert_eq!(data["id"], run_id);
    // A real daemon must finish this run. Each poll is an explicit new MCP read.
    let mut status = data["status"].as_str().unwrap().to_owned();
    for _ in 0..30 {
        if !["assigned", "launching", "running"].contains(&status.as_str()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let inspection = call(&owner, "get_run", json!({"run_id":run_id})).await;
        status = native_success(&inspection)["status"]
            .as_str()
            .unwrap()
            .to_owned();
    }
    assert_eq!(
        status, "canceled",
        "fixture runner did not acknowledge cancellation within 30 seconds"
    );
    let ended = call(&owner, "cancel_run", json!({"run_id":run_id})).await;
    assert_eq!(ended["isError"], true);
    assert_eq!(envelope(&ended)["meta"]["http_status"], 422);
    assert_eq!(envelope(&ended)["error"]["code"], "run_already_ended");
}

#[tokio::test]
async fn large_conflicts_keep_native_code_and_large_success_keeps_confirmed_receipt() {
    let current = json!({"id":"c","kind":"prompt","version":3,"body":"\"\\".repeat(200 * 1024)});
    let upstream = upstream([(409, json!({"error":{"code":"version_conflict","message":"changed","details":{"current":current}}})), (200, current)]).await;
    let client = client(&upstream.url, "acceptance-token");
    let args = json!({"context_id":"c","body":"replacement","expected_version":1});
    let conflict = call(&client, "edit_context", args.clone()).await;
    assert_eq!(conflict["isError"], true);
    assert_eq!(envelope(&conflict)["meta"]["http_status"], 409);
    assert_eq!(envelope(&conflict)["error"]["code"], "version_conflict");
    assert_write(&conflict, "rejected");
    assert!(
        !envelope(&conflict)["meta"]["omissions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(upstream.state.requests.lock().unwrap().len(), 1);
    let success = call(&client, "edit_context", args).await;
    assert_eq!(success["isError"], false);
    assert_write(&success, "confirmed");
    assert!(envelope(&success)["data"].is_null());
    assert_eq!(envelope(&success)["meta"]["write"]["receipt"]["version"], 3);
    assert_eq!(upstream.state.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn mcp_transport_invokes_recovery_once_for_both_protocol_revisions() {
    use tower::ServiceExt;
    for version in ["2025-11-25", "2026-07-28"] {
        let native = json!({"id":"r","status":"running","outcome":null});
        let upstream = upstream([(200, native.clone())]).await;
        let config = Config::from_lookup(|name| match name {
            "TINES_BASE_URL" => Some(upstream.url.clone()),
            "TINES_API_TOKEN" => Some("acceptance-token".into()),
            "TINES_ALLOW_HTTP" => Some("true".into()),
            _ => None,
        })
        .unwrap();
        let app = tines_mcp::server::router_with_client(
            &config,
            Some(client(&upstream.url, "acceptance-token")),
            CancellationToken::new(),
        );
        let mut params = json!({"name":"cancel_run","arguments":{"run_id":"r"}});
        if version == "2026-07-28" {
            params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":version,
                "io.modelcontextprotocol/clientCapabilities":{},
                "io.modelcontextprotocol/clientInfo":{"name":"mcp14-acceptance","version":"1"}});
        }
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-protocol-version", version)
                    .header("mcp-method", "tools/call")
                    .header("mcp-name", "cancel_run")
                    .body(Body::from(
                        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":params})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let rpc: Value = serde_json::from_slice(&bytes).unwrap();
        let result = &rpc["result"];
        assert_eq!(result["isError"], false, "{rpc}");
        assert_eq!(envelope(result)["data"], native);
        assert_write(result, "confirmed");
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, *envelope(result));
        assert_requests(&upstream, &[("POST", "/api/v1/runs/r/cancel", Value::Null)]);
    }
}
