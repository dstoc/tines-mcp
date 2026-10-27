use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
    routing::any,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tines_mcp::{config::Config, server, tines::TinesClient};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Clone)]
struct MockState {
    body: Arc<Mutex<Value>>,
    responses: Arc<Mutex<HashMap<String, Value>>>,
    requests: Arc<Mutex<Vec<String>>>,
    path_responses: Arc<Mutex<HashMap<String, (StatusCode, Value)>>>,
    replies: Arc<Mutex<HashMap<String, VecDeque<MockReply>>>>,
}

struct MockReply {
    status: StatusCode,
    content_type: String,
    body: Vec<u8>,
}

impl MockReply {
    fn json(body: Value) -> Self {
        Self::json_with_status(StatusCode::OK, body)
    }

    fn json_with_status(status: StatusCode, body: Value) -> Self {
        Self {
            status,
            content_type: "application/json".to_owned(),
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn bytes(body: impl Into<Vec<u8>>, content_type: &str) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: content_type.to_owned(),
            body: body.into(),
        }
    }
}

impl MockState {
    fn set_body(&self, body: Value) {
        *self.body.lock().unwrap() = body;
    }

    fn set_response(&self, path: &str, body: Value) {
        self.responses.lock().unwrap().insert(path.to_owned(), body);
    }

    fn set_path_response(&self, path: &str, status: StatusCode, body: Value) {
        self.path_responses
            .lock()
            .unwrap()
            .insert(path.to_owned(), (status, body));
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn set_replies(&self, path: &str, replies: impl IntoIterator<Item = MockReply>) {
        self.replies
            .lock()
            .unwrap()
            .insert(path.to_owned(), replies.into_iter().collect());
    }
}

struct MockUpstream {
    base_url: String,
    state: MockState,
    task: JoinHandle<()>,
}

async fn mock_handler(State(state): State<MockState>, request: Request<Body>) -> Response {
    let uri = request.uri().to_string();
    let path = uri.split('?').next().unwrap_or_default().to_owned();
    state.requests.lock().unwrap().push(uri);
    let queued_reply = state
        .replies
        .lock()
        .unwrap()
        .get_mut(&path)
        .and_then(VecDeque::pop_front);
    let path_response = state.path_responses.lock().unwrap().get(&path).cloned();
    let response_body = state.responses.lock().unwrap().get(&path).cloned();
    let reply = queued_reply
        .or_else(|| path_response.map(|(status, body)| MockReply::json_with_status(status, body)))
        .or_else(|| response_body.map(MockReply::json))
        .unwrap_or_else(|| MockReply::json(state.body.lock().unwrap().clone()));
    Response::builder()
        .status(reply.status)
        .header("Content-Type", reply.content_type)
        .body(Body::from(reply.body))
        .unwrap()
}

async fn start_mock(body: Value) -> MockUpstream {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock upstream should bind");
    let address = listener
        .local_addr()
        .expect("mock upstream should have an address");
    let state = MockState {
        body: Arc::new(Mutex::new(body)),
        responses: Arc::new(Mutex::new(HashMap::new())),
        requests: Arc::new(Mutex::new(Vec::new())),
        path_responses: Arc::new(Mutex::new(HashMap::new())),
        replies: Arc::new(Mutex::new(HashMap::new())),
    };
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .fallback(any(mock_handler))
                .with_state(task_state),
        )
        .await
        .expect("mock upstream should keep serving");
    });
    MockUpstream {
        base_url: format!("http://{address}"),
        state,
        task,
    }
}

fn configured_app(upstream: &MockUpstream, excluded: &[&str]) -> Router {
    let mut values = vec![
        ("TINES_BASE_URL", upstream.base_url.clone()),
        ("TINES_API_TOKEN", "local-test-token".to_owned()),
        ("TINES_ALLOW_HTTP", "true".to_owned()),
    ];
    if !excluded.is_empty() {
        values.push(("TINES_EXCLUDED_PROJECT_IDS", excluded.join(",")));
    }
    let config = Config::from_lookup(|name| {
        values
            .iter()
            .find_map(|(key, value)| (*key == name).then(|| value.clone()))
    })
    .expect("test config should validate");
    let client = config
        .tines
        .as_ref()
        .map(TinesClient::new)
        .transpose()
        .expect("test Tines client should build");
    server::router_with_client(&config, client, CancellationToken::new())
}

fn unconfigured_app() -> Router {
    let config = Config::from_lookup(|_| None).expect("default config should validate");
    server::router(&config, CancellationToken::new())
}

async fn json_response(response: Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response should be readable")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response should contain JSON")
}

async fn call_tool(app: Router, name: &str, args: Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": name, "arguments": args}
            })
            .to_string(),
        ))
        .expect("tool call request should build");
    let response = app
        .oneshot(request)
        .await
        .expect("MCP request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    json_response(response).await["result"]["structuredContent"].clone()
}

async fn list_tools(app: Router) -> Vec<Value> {
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            })
            .to_string(),
        ))
        .expect("tools/list request should build");
    let response = app
        .oneshot(request)
        .await
        .expect("MCP request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    json_response(response).await["result"]["tools"]
        .as_array()
        .expect("tool list should be an array")
        .clone()
}

fn decoded_query(uri: &str) -> Vec<(String, String)> {
    let (_, query) = uri.split_once('?').expect("request should have query");
    url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn artifact_version(version: u32, content_type: &str) -> Value {
    json!({
        "version": version,
        "filename": "artifact.bin",
        "content_type": content_type,
        "size_bytes": 12,
        "file_count": null,
        "url": null,
        "title": null,
        "pr_repo_url": null,
        "pr_number": null,
        "reaffirmed_from": null,
        "actor": null,
        "created_at": "2026-10-28T00:00:00.000Z"
    })
}

fn artifact_detail(
    id: &str,
    issue_id: &str,
    name: &str,
    artifact_type: &str,
    versions: Vec<Value>,
) -> Value {
    let current_version = versions.last().cloned().unwrap_or(Value::Null);
    json!({
        "id": id,
        "issue_id": issue_id,
        "name": name,
        "artifact_type": artifact_type,
        "description": null,
        "version_count": versions.len(),
        "current_version": current_version,
        "versions": versions,
        "fresh": true,
        "created_at": "2026-10-28T00:00:00.000Z",
        "updated_at": "2026-10-28T00:00:00.000Z"
    })
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn set_issue_parent_replies(
    state: &MockState,
    issue_id: &str,
    project_id: &str,
    number: u64,
    count: usize,
) {
    state.set_replies(
        &format!("/api/v1/issues/{issue_id}"),
        (0..count).map(|_| {
            MockReply::json(json!({
                "id": issue_id,
                "project_id": project_id,
                "number": number,
                "project_name": "Project"
            }))
        }),
    );
}

#[tokio::test]
async fn tools_are_discoverable_and_issue_lookup_schema_requires_exactly_one_lookup_form() {
    let tools = list_tools(unconfigured_app()).await;
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
            "list_issue_artifacts",
            "get_issue_artifact",
            "read_artifact_content",
            "list_workflows",
            "get_workflow",
            "list_labels",
            "list_events",
            "list_context",
            "get_context",
            "get_issue_context",
            "get_issue_journal",
        ]
    );
    assert!(
        tools
            .iter()
            .all(|tool| tool["annotations"]["readOnlyHint"] == true)
    );
    let get_issue = tools
        .iter()
        .find(|tool| tool["name"] == "get_issue")
        .unwrap();
    assert_eq!(
        get_issue["inputSchema"]["oneOf"][0]["required"],
        json!(["issue_id"])
    );
    assert_eq!(
        get_issue["inputSchema"]["oneOf"][1]["required"],
        json!(["project_id", "number"])
    );
    assert_eq!(
        get_issue["inputSchema"]["properties"]["issue_id"]["type"],
        "string"
    );
    assert_eq!(
        get_issue["inputSchema"]["properties"]["number"]["type"],
        "integer"
    );
    assert_eq!(
        get_issue["inputSchema"]["properties"]["number"]["minimum"],
        1
    );
    assert_eq!(
        tools
            .iter()
            .find(|tool| tool["name"] == "get_project")
            .unwrap()["inputSchema"]["required"],
        json!(["project_id"])
    );
    assert_eq!(
        tools
            .iter()
            .find(|tool| tool["name"] == "get_workflow")
            .unwrap()["inputSchema"]["required"],
        json!(["workflow_id"])
    );
    let list_context = tools
        .iter()
        .find(|tool| tool["name"] == "list_context")
        .unwrap();
    assert_eq!(
        list_context["inputSchema"]["properties"]["project_id"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        list_context["inputSchema"]["properties"]["state_id"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        list_context["inputSchema"]["properties"]["label_id"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        list_context["inputSchema"]["properties"]["issue_id"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        tools
            .iter()
            .find(|tool| tool["name"] == "get_context")
            .unwrap()["inputSchema"]["required"],
        json!(["context_id"])
    );
    let list_events = tools
        .iter()
        .find(|tool| tool["name"] == "list_events")
        .unwrap();
    assert_eq!(
        list_events["inputSchema"]["properties"]["event_types"]["type"],
        "array"
    );
    assert_eq!(
        list_events["inputSchema"]["properties"]["event_types"]["items"]["type"],
        "string"
    );
    assert_eq!(
        list_events["inputSchema"]["properties"]["state"]["description"],
        "Filter by workflow state ID referenced by an event payload."
    );
}

#[tokio::test]
async fn artifact_metadata_is_separate_and_ordinary_issue_reads_do_not_expand_artifacts() {
    let upstream = start_mock(json!({})).await;
    let app = configured_app(&upstream, &[]);
    let detail = artifact_detail(
        "artifact-1",
        "issue-a",
        "design-doc",
        "text",
        vec![artifact_version(1, "text/markdown")],
    );
    set_issue_parent_replies(&upstream.state, "issue-a", "project-a", 7, 3);
    upstream.state.set_replies(
        "/api/v1/issues/issue-a/artifacts",
        [MockReply::json(json!({
            "items": [{
                "id": "artifact-1",
                "issue_id": "issue-a",
                "name": "design-doc",
                "artifact_type": "text",
                "current_version": {"version": 1, "content_type": "text/markdown"}
            }]
        }))],
    );
    upstream.state.set_replies(
        "/api/v1/issues/issue-a/artifacts/design-doc",
        [MockReply::json(detail.clone())],
    );

    let issue = call_tool(app.clone(), "get_issue", json!({"issue_id": "issue-a"})).await;
    assert_eq!(issue["data"]["id"], "issue-a");
    assert_eq!(upstream.state.requests(), ["/api/v1/issues/issue-a"]);

    let listed = call_tool(
        app.clone(),
        "list_issue_artifacts",
        json!({"issue_id": "issue-a"}),
    )
    .await;
    assert_eq!(listed["data"]["items"][0]["id"], "artifact-1");
    assert_eq!(listed["data"]["next_cursor"], Value::Null);

    let fetched = call_tool(
        app,
        "get_issue_artifact",
        json!({"issue_id": "issue-a", "name": "design-doc"}),
    )
    .await;
    assert_eq!(fetched["data"]["versions"][0]["version"], 1);
    assert_eq!(fetched["data"]["id"], "artifact-1");
    assert_eq!(fetched["meta"]["links"][0]["rel"], "issue");
    assert_eq!(
        upstream.state.requests(),
        [
            "/api/v1/issues/issue-a",
            "/api/v1/issues/issue-a",
            "/api/v1/issues/issue-a/artifacts",
            "/api/v1/issues/issue-a",
            "/api/v1/issues/issue-a/artifacts/design-doc"
        ]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn artifact_reads_check_the_current_issue_project_before_expansion() {
    let upstream = start_mock(json!({})).await;
    upstream.state.set_replies(
        "/api/v1/issues/issue-hidden",
        [MockReply::json(json!({
            "id": "issue-hidden",
            "project_id": "project-hidden",
            "number": 9
        }))],
    );
    let app = configured_app(&upstream, &["project-hidden"]);
    let result = call_tool(
        app,
        "list_issue_artifacts",
        json!({"issue_id": "issue-hidden"}),
    )
    .await;
    assert_eq!(result["error"]["source"], "visibility");
    assert_eq!(result["error"]["code"], "resource_unavailable");
    assert_eq!(upstream.state.requests(), ["/api/v1/issues/issue-hidden"]);
    upstream.task.abort();
}

#[tokio::test]
async fn folder_content_uses_selected_version_and_path_and_keeps_utf8_chunks_on_boundaries() {
    let mut first_version = artifact_version(1, "application/octet-stream");
    first_version["files"] = json!([{
        "path": "src/main.rs",
        "content_type": "text/plain",
        "size_bytes": 32771
    }]);
    first_version["file_count"] = json!(1);
    let mut current_version = artifact_version(2, "application/octet-stream");
    current_version["files"] = json!([{
        "path": "src/main.rs",
        "content_type": "text/plain",
        "size_bytes": 4
    }]);
    current_version["file_count"] = json!(1);
    let detail = artifact_detail(
        "folder-id",
        "issue-folder",
        "source",
        "folder",
        vec![first_version, current_version],
    );
    let mut content = vec![b'a'; 32767];
    content.extend_from_slice("☃z".as_bytes());
    let upstream = start_mock(detail.clone()).await;
    set_issue_parent_replies(&upstream.state, "issue-folder", "project-a", 1, 2);
    upstream.state.set_replies(
        "/api/v1/issues/issue-folder/artifacts/source/content",
        [
            MockReply::bytes(content.clone(), "text/plain; charset=utf-8"),
            MockReply::bytes(content.clone(), "text/plain; charset=utf-8"),
        ],
    );
    let app = configured_app(&upstream, &[]);
    let first = call_tool(
        app.clone(),
        "read_artifact_content",
        json!({"issue_id": "issue-folder", "name": "source", "version": 1, "path": "src/main.rs"}),
    )
    .await;
    assert_eq!(first["data"]["encoding"], "utf-8");
    assert_eq!(first["data"]["source"]["version"], 1);
    assert_eq!(first["data"]["source"]["path"], "src/main.rs");
    assert_eq!(first["data"]["total_bytes"], 32771);
    assert_eq!(first["data"]["returned_length"], 32767);
    assert_eq!(
        first["data"]["content"].as_str().unwrap(),
        "a".repeat(32767)
    );
    assert_eq!(first["data"]["next_offset"], 32767);

    let second = call_tool(
        app,
        "read_artifact_content",
        json!({
            "issue_id": "issue-folder",
            "name": "source",
            "version": 1,
            "path": "src/main.rs",
            "offset": 32767,
            "expected_sha256": first["data"]["sha256"],
            "expected_artifact_id": "folder-id"
        }),
    )
    .await;
    assert_eq!(second["data"]["content"], "☃z");
    assert_eq!(second["data"]["returned_length"], 4);
    assert_eq!(second["data"]["offset"], 32767);
    assert_eq!(second["data"]["next_offset"], Value::Null);
    assert_eq!(second["data"]["sha256"], first["data"]["sha256"]);

    let requests = upstream.state.requests();
    assert_eq!(
        requests.len(),
        8,
        "each chunk re-fetches metadata and bytes"
    );
    assert_eq!(
        requests[2].split('?').next().unwrap(),
        "/api/v1/issues/issue-folder/artifacts/source/content"
    );
    assert_eq!(
        decoded_query(&requests[2]),
        [
            ("version".to_owned(), "1".to_owned()),
            ("path".to_owned(), "src/main.rs".to_owned())
        ]
    );
    assert_eq!(
        decoded_query(&requests[6]),
        [
            ("version".to_owned(), "1".to_owned()),
            ("path".to_owned(), "src/main.rs".to_owned())
        ]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn omitted_version_is_pinned_and_binary_chunks_are_base64_with_repeated_bounded_fetches() {
    let detail = artifact_detail(
        "binary-id",
        "issue-binary",
        "archive",
        "file",
        vec![
            artifact_version(2, "application/octet-stream"),
            artifact_version(3, "application/octet-stream"),
        ],
    );
    let bytes = (0..32770).map(|index| index as u8).collect::<Vec<_>>();
    let upstream = start_mock(detail.clone()).await;
    set_issue_parent_replies(&upstream.state, "issue-binary", "project-a", 2, 2);
    upstream.state.set_replies(
        "/api/v1/issues/issue-binary/artifacts/archive/content",
        [
            MockReply::bytes(bytes.clone(), "application/octet-stream"),
            MockReply::bytes(bytes.clone(), "application/octet-stream"),
        ],
    );
    let app = configured_app(&upstream, &[]);
    let first = call_tool(
        app.clone(),
        "read_artifact_content",
        json!({"issue_id": "issue-binary", "name": "archive"}),
    )
    .await;
    assert_eq!(first["data"]["encoding"], "base64");
    assert_eq!(first["data"]["source"]["version"], 3);
    assert_eq!(first["data"]["total_bytes"], 32770);
    assert_eq!(first["data"]["returned_length"], 32768);
    assert_eq!(first["data"]["next_offset"], 32768);
    assert_eq!(
        BASE64
            .decode(first["data"]["content"].as_str().unwrap())
            .unwrap(),
        bytes[..32768]
    );

    let second = call_tool(
        app,
        "read_artifact_content",
        json!({
            "issue_id": "issue-binary",
            "name": "archive",
            "version": 3,
            "offset": 32768,
            "expected_sha256": first["data"]["sha256"],
            "expected_artifact_id": "binary-id"
        }),
    )
    .await;
    assert_eq!(second["data"]["source"]["version"], 3);
    assert_eq!(second["data"]["offset"], 32768);
    assert_eq!(second["data"]["returned_length"], 2);
    assert_eq!(second["data"]["next_offset"], Value::Null);
    assert_eq!(
        BASE64
            .decode(second["data"]["content"].as_str().unwrap())
            .unwrap(),
        bytes[32768..]
    );
    assert_eq!(second["data"]["sha256"], first["data"]["sha256"]);

    let requests = upstream.state.requests();
    assert_eq!(requests.len(), 8);
    assert_eq!(
        decoded_query(&requests[2]),
        [("version".to_owned(), "3".to_owned())]
    );
    assert_eq!(
        decoded_query(&requests[6]),
        [("version".to_owned(), "3".to_owned())]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn deleted_and_recreated_artifact_fails_identity_recheck() {
    let old = artifact_detail(
        "artifact-old",
        "issue-recreated",
        "report",
        "text",
        vec![artifact_version(1, "text/plain")],
    );
    let replacement = artifact_detail(
        "artifact-new",
        "issue-recreated",
        "report",
        "text",
        vec![artifact_version(1, "text/plain")],
    );
    let upstream = start_mock(old.clone()).await;
    set_issue_parent_replies(&upstream.state, "issue-recreated", "project-a", 3, 1);
    upstream.state.set_replies(
        "/api/v1/issues/issue-recreated/artifacts/report",
        [MockReply::json(old), MockReply::json(replacement)],
    );
    upstream.state.set_replies(
        "/api/v1/issues/issue-recreated/artifacts/report/content",
        [MockReply::bytes("body", "text/plain")],
    );
    let app = configured_app(&upstream, &[]);
    let result = call_tool(
        app,
        "read_artifact_content",
        json!({"issue_id": "issue-recreated", "name": "report"}),
    )
    .await;
    assert_eq!(result["error"]["code"], "content_changed");
    assert_eq!(
        upstream.state.requests(),
        [
            "/api/v1/issues/issue-recreated",
            "/api/v1/issues/issue-recreated/artifacts/report",
            "/api/v1/issues/issue-recreated/artifacts/report/content?version=1",
            "/api/v1/issues/issue-recreated/artifacts/report"
        ]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn continuation_detects_changed_source_digest_and_rejects_missing_identity_fields() {
    let detail = artifact_detail(
        "stable-artifact-id",
        "issue-changing",
        "payload",
        "file",
        vec![artifact_version(1, "application/octet-stream")],
    );
    let original = vec![0x41; 32769];
    let mut changed = original.clone();
    changed[0] = 0x42;
    let upstream = start_mock(detail.clone()).await;
    set_issue_parent_replies(&upstream.state, "issue-changing", "project-a", 4, 2);
    upstream.state.set_replies(
        "/api/v1/issues/issue-changing/artifacts/payload/content",
        [
            MockReply::bytes(original.clone(), "application/octet-stream"),
            MockReply::bytes(changed, "application/octet-stream"),
        ],
    );
    let app = configured_app(&upstream, &[]);
    let first = call_tool(
        app.clone(),
        "read_artifact_content",
        json!({"issue_id": "issue-changing", "name": "payload", "version": 1}),
    )
    .await;
    assert_eq!(first["data"]["next_offset"], 32768);
    let changed_result = call_tool(
        app.clone(),
        "read_artifact_content",
        json!({
            "issue_id": "issue-changing",
            "name": "payload",
            "version": 1,
            "offset": 32768,
            "expected_sha256": first["data"]["sha256"],
            "expected_artifact_id": "stable-artifact-id"
        }),
    )
    .await;
    assert_eq!(changed_result["error"]["code"], "content_changed");
    assert_eq!(upstream.state.requests().len(), 8);
    upstream.task.abort();

    let upstream = start_mock(detail).await;
    set_issue_parent_replies(&upstream.state, "issue-changing", "project-a", 4, 1);
    let app = configured_app(&upstream, &[]);
    for args in [
        json!({"issue_id": "issue-changing", "name": "payload", "offset": 1, "expected_sha256": "0".repeat(64), "expected_artifact_id": "stable-artifact-id"}),
        json!({"issue_id": "issue-changing", "name": "payload", "version": 1, "offset": 1, "expected_sha256": "0".repeat(64)}),
        json!({"issue_id": "issue-changing", "name": "payload", "version": 1, "offset": 1, "expected_artifact_id": "stable-artifact-id"}),
    ] {
        let result = call_tool(app.clone(), "read_artifact_content", args).await;
        assert_eq!(result["error"]["source"], "validation");
    }
    assert!(upstream.state.requests().is_empty());
    let mismatched_id = call_tool(
        app,
        "read_artifact_content",
        json!({
            "issue_id": "issue-changing",
            "name": "payload",
            "version": 1,
            "offset": 1,
            "expected_sha256": "0".repeat(64),
            "expected_artifact_id": "replaced-artifact-id"
        }),
    )
    .await;
    assert_eq!(mismatched_id["error"]["code"], "content_changed");
    assert_eq!(
        upstream.state.requests(),
        [
            "/api/v1/issues/issue-changing",
            "/api/v1/issues/issue-changing/artifacts/payload"
        ]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn text_continuation_rejects_a_caller_offset_inside_a_utf8_code_point() {
    let detail = artifact_detail(
        "text-id",
        "issue-utf8",
        "note",
        "text",
        vec![artifact_version(1, "text/plain")],
    );
    let bytes = "a☃z".as_bytes().to_vec();
    let upstream = start_mock(detail).await;
    set_issue_parent_replies(&upstream.state, "issue-utf8", "project-a", 5, 1);
    upstream.state.set_replies(
        "/api/v1/issues/issue-utf8/artifacts/note/content",
        [MockReply::bytes(bytes.clone(), "text/plain")],
    );
    let app = configured_app(&upstream, &[]);
    let result = call_tool(
        app,
        "read_artifact_content",
        json!({
            "issue_id": "issue-utf8",
            "name": "note",
            "version": 1,
            "offset": 2,
            "expected_sha256": sha256(&bytes),
            "expected_artifact_id": "text-id"
        }),
    )
    .await;
    assert_eq!(result["error"]["source"], "validation");
    assert_eq!(
        result["error"]["message"],
        "offset: must be a UTF-8 character boundary for text content"
    );
    upstream.task.abort();
}

#[tokio::test]
async fn folder_content_requires_a_path_and_link_artifacts_remain_references() {
    let mut version = artifact_version(1, "application/octet-stream");
    version["files"] = json!([{
        "path": "docs/readme.md",
        "content_type": "text/markdown",
        "size_bytes": 4
    }]);
    version["file_count"] = json!(1);
    let folder = artifact_detail(
        "folder-id",
        "issue-folder-path",
        "docs",
        "folder",
        vec![version],
    );
    let upstream = start_mock(folder).await;
    set_issue_parent_replies(&upstream.state, "issue-folder-path", "project-a", 6, 1);
    upstream.state.set_replies(
        "/api/v1/issues/issue-folder-path/artifacts/docs/content",
        [MockReply::json_with_status(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error": {"code": "folder_path_required", "message": "Choose a file path"}}),
        )],
    );
    let app = configured_app(&upstream, &[]);
    let missing_path = call_tool(
        app,
        "read_artifact_content",
        json!({"issue_id": "issue-folder-path", "name": "docs"}),
    )
    .await;
    assert_eq!(missing_path["error"]["source"], "upstream");
    assert_eq!(missing_path["error"]["status"], 422);
    assert_eq!(missing_path["error"]["code"], "folder_path_required");
    assert_eq!(
        upstream.state.requests(),
        [
            "/api/v1/issues/issue-folder-path",
            "/api/v1/issues/issue-folder-path/artifacts/docs",
            "/api/v1/issues/issue-folder-path/artifacts/docs/content?version=1"
        ]
    );
    upstream.task.abort();

    let mut version = artifact_version(1, "text/markdown");
    version["url"] = json!("https://example.invalid/spec");
    let link = artifact_detail("link-id", "issue-link", "spec", "link", vec![version]);
    let upstream = start_mock(link).await;
    set_issue_parent_replies(&upstream.state, "issue-link", "project-a", 7, 1);
    let app = configured_app(&upstream, &[]);
    let result = call_tool(
        app,
        "get_issue_artifact",
        json!({"issue_id": "issue-link", "name": "spec"}),
    )
    .await;
    assert_eq!(
        result["data"]["versions"][0]["url"],
        "https://example.invalid/spec"
    );
    assert_eq!(
        upstream.state.requests(),
        [
            "/api/v1/issues/issue-link",
            "/api/v1/issues/issue-link/artifacts/spec"
        ]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn artifact_content_source_over_one_mib_preserves_metadata_without_returning_bytes() {
    let detail = artifact_detail(
        "large-id",
        "issue-large",
        "large-file",
        "file",
        vec![artifact_version(1, "application/octet-stream")],
    );
    let upstream = start_mock(detail).await;
    set_issue_parent_replies(&upstream.state, "issue-large", "project-a", 8, 1);
    upstream.state.set_replies(
        "/api/v1/issues/issue-large/artifacts/large-file/content",
        [MockReply::bytes(
            vec![b'x'; 1024 * 1024 + 1],
            "application/octet-stream",
        )],
    );
    let app = configured_app(&upstream, &[]);
    let result = call_tool(
        app,
        "read_artifact_content",
        json!({"issue_id": "issue-large", "name": "large-file"}),
    )
    .await;
    assert_eq!(result["error"]["source"], "schema");
    assert_eq!(result["error"]["status"], 200);
    assert_eq!(result["error"]["code"], "content_source_too_large");
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("1 MiB")
    );
    assert_eq!(result["error"]["details"]["limit_bytes"], 1024 * 1024);
    assert!(result["error"]["diagnostic_id"].is_number());
    assert_eq!(
        result["data"]["source"],
        json!({
            "issue_id": "issue-large",
            "artifact_id": "large-id",
            "name": "large-file",
            "version": 1,
            "path": null
        })
    );
    assert_eq!(result["data"]["mime_type"], "application/octet-stream");
    assert!(result["data"].get("content").is_none());
    assert_eq!(result["meta"]["links"][0]["rel"], "issue");
    assert_eq!(
        result["meta"]["links"][0]["href"],
        format!("{}/issues/project-a/8", upstream.base_url)
    );
    assert_eq!(upstream.state.requests().len(), 4);
    upstream.task.abort();
}

#[tokio::test]
async fn issue_lookup_rejects_partial_conflicting_and_nonpositive_aliases_without_upstream_reads() {
    let upstream = start_mock(json!({})).await;
    let app = configured_app(&upstream, &[]);
    for args in [
        json!({}),
        json!({"project_id": "project-a"}),
        json!({"number": 4}),
        json!({"issue_id": "issue-a", "project_id": "project-a", "number": 4}),
        json!({"project_id": "project-a", "number": 0}),
        json!({"issue_id": null, "project_id": "project-a", "number": 4}),
    ] {
        let result = call_tool(app.clone(), "get_issue", args).await;
        assert_eq!(result["error"]["source"], "validation");
    }
    assert!(upstream.state.requests().is_empty());
    upstream.task.abort();
}

#[tokio::test]
async fn issue_descriptions_are_opt_in_and_disable_native_brief_mode() {
    let upstream = start_mock(json!({"items": [], "next_cursor": null})).await;
    let app = configured_app(&upstream, &[]);
    let result = call_tool(app, "list_issues", json!({"descriptions": true})).await;
    assert_eq!(result["meta"]["omissions"], json!([]));
    assert_eq!(
        decoded_query(upstream.state.requests().first().unwrap()),
        [("brief".to_owned(), "0".to_owned())]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn issue_list_maps_filters_and_preserves_opaque_cursor_on_an_excluded_only_page() {
    let upstream = start_mock(json!({
        "items": [
            {
                "id": "issue-visible",
                "project_id": "project-visible",
                "project_name": "Visible",
                "number": 22,
                "owner": {"id": "owner-1", "name": "Owner"},
                "state": {"id": "state-1", "name": "Doing"},
                "effective_state": {"id": "state-1", "name": "Doing"}
            },
            {
                "id": "issue-hidden",
                "project_id": "project-hidden",
                "project_name": "Hidden",
                "number": 23,
                "owner": {"id": "owner-2", "name": "Hidden"}
            }
        ],
        "next_cursor": "cursor+/=opaque"
    }))
    .await;
    let app = configured_app(&upstream, &["project-hidden"]);
    let args = json!({
        "project_id": "project-visible",
        "workflow_id": "workflow-1",
        "state": "Doing",
        "category": "awaiting_human",
        "schedule_id": "schedule-1",
        "labels": ["docs", "p1"],
        "search": "a & b",
        "ready": true,
        "hide_done": true,
        "hide_duplicates": false,
        "archived": "all",
        "limit": 2,
        "cursor": "before+/=opaque"
    });
    let first = call_tool(app.clone(), "list_issues", args.clone()).await;
    assert_eq!(first["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["data"]["items"][0]["id"], "issue-visible");
    assert_eq!(
        first["data"]["next_cursor"], "cursor+/=opaque",
        "the adapter must preserve the upstream cursor byte-for-byte"
    );
    assert_eq!(
        first["data"]["items"][0]["owner"],
        json!({"id": "owner-1", "name": "Owner"})
    );
    assert_eq!(
        first["meta"]["links"][0]["href"],
        format!("{}/issues/project-visible/22", upstream.base_url)
    );
    assert_eq!(first["meta"]["links"].as_array().unwrap().len(), 1);
    let request = upstream.state.requests().remove(0);
    assert!(request.starts_with("/api/v1/issues?"));
    let expected_query = [
        ("project", "project-visible"),
        ("workflow", "workflow-1"),
        ("state", "Doing"),
        ("category", "awaiting_human"),
        ("schedule", "schedule-1"),
        ("label", "docs"),
        ("label", "p1"),
        ("q", "a & b"),
        ("ready", "true"),
        ("hide_done", "true"),
        ("hide_duplicates", "false"),
        ("archived", "all"),
        ("brief", "1"),
        ("limit", "2"),
        ("cursor", "before+/=opaque"),
    ]
    .map(|(key, value)| (key.to_owned(), value.to_owned()));
    assert_eq!(decoded_query(&request), expected_query);

    upstream.state.set_body(json!({
        "items": [{"id": "issue-hidden", "project_id": "project-hidden"}],
        "next_cursor": "still+/=opaque"
    }));
    let second = call_tool(app, "list_issues", args).await;
    assert_eq!(second["data"]["items"], json!([]));
    assert_eq!(second["data"]["next_cursor"], "still+/=opaque");
    assert_eq!(upstream.state.requests().len(), 2);
    upstream.task.abort();
}

#[tokio::test]
async fn project_exclusions_use_ids_for_direct_and_returned_issue_ownership() {
    let upstream = start_mock(json!({
        "id": "issue-moved",
        "project_id": "project-hidden",
        "number": 8,
        "title": "Transferred issue"
    }))
    .await;
    let app = configured_app(&upstream, &["project-hidden"]);

    let direct_project = call_tool(
        app.clone(),
        "get_project",
        json!({"project_id": "project-hidden"}),
    )
    .await;
    assert_eq!(direct_project["error"]["source"], "visibility");
    assert!(upstream.state.requests().is_empty());

    let direct_alias = call_tool(
        app.clone(),
        "get_issue",
        json!({"project_id": "project-hidden", "number": 8}),
    )
    .await;
    assert_eq!(direct_alias["error"]["source"], "visibility");
    assert!(upstream.state.requests().is_empty());

    let moved_alias = call_tool(
        app,
        "get_issue",
        json!({"project_id": "project-old", "number": 8}),
    )
    .await;
    assert_eq!(moved_alias["error"]["source"], "visibility");
    assert_eq!(upstream.state.requests().len(), 1);
    assert_eq!(
        upstream.state.requests()[0],
        "/api/v1/projects/project-old/issues/8"
    );
    upstream.task.abort();
}

#[tokio::test]
async fn exclusion_checks_fail_closed_when_a_list_row_has_no_immutable_project_id() {
    let upstream = start_mock(json!({
        "items": [{"id": "issue-with-redacted-owner"}],
        "next_cursor": "opaque-next"
    }))
    .await;
    let app = configured_app(&upstream, &["project-hidden"]);
    let issue_page = call_tool(app, "list_issues", json!({})).await;
    assert_eq!(issue_page["error"]["source"], "schema");
    assert_eq!(issue_page["error"]["status"], 200);
    assert!(issue_page["data"].is_null());
    assert_eq!(upstream.state.requests().len(), 1);
    upstream.task.abort();
}

#[tokio::test]
async fn owner_and_member_issue_projections_keep_native_missing_and_redacted_fields() {
    let owner = json!({
        "id": "issue-owner",
        "project_id": "project-a",
        "number": 3,
        "state": {"id": "state-open", "name": "Open"},
        "effective_state": {"id": "state-done", "name": "Done"},
        "open_blockers": [{"id": "blocker-1"}],
        "active_run": null
    });
    let upstream = start_mock(owner.clone()).await;
    let app = configured_app(&upstream, &[]);
    let owner_result =
        call_tool(app.clone(), "get_issue", json!({"issue_id": "issue-owner"})).await;
    assert_eq!(owner_result["data"], owner);

    let member = json!({
        "mode": "member",
        "id": "issue-member",
        "project": {
            "id": "project-a",
            "owner": {"id": "owner-a", "name": "Owner"}
        },
        "state": {"id": "state-open", "name": "Open"},
        "latest_run": {"status": "running", "run_id": "run-a"},
        "my_choice": {"value": "unset", "source": null},
        "capabilities": {"execute": false}
    });
    upstream.state.set_body(member.clone());
    let member_result = call_tool(app, "get_issue", json!({"issue_id": "issue-member"})).await;
    assert_eq!(member_result["data"], member);
    assert!(member_result["data"].get("project_id").is_none());
    assert!(member_result["data"].get("effective_state").is_none());
    assert_eq!(
        upstream.state.requests(),
        ["/api/v1/issues/issue-owner", "/api/v1/issues/issue-member"]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn project_workflow_and_label_tools_use_one_native_request_and_keep_shapes() {
    let upstream = start_mock(json!({
        "items": [
            {"id": "project-visible", "name": "Owned project"},
            {"id": "project-hidden", "name": "Member project", "owner": {"id": "owner"}}
        ],
        "next_cursor": null
    }))
    .await;
    let app = configured_app(&upstream, &["project-hidden"]);

    let projects = call_tool(app.clone(), "list_projects", json!({"archived": "all"})).await;
    assert_eq!(
        projects["data"]["items"],
        json!([{"id": "project-visible", "name": "Owned project"}])
    );
    assert_eq!(
        projects["meta"]["links"][0]["href"],
        format!("{}/projects/project-visible", upstream.base_url)
    );
    assert_eq!(upstream.state.requests(), ["/api/v1/projects?archived=all"]);

    upstream.state.set_body(json!({
        "items": [{"id": "workflow-1", "name": "Shared owner library"}],
        "next_cursor": null
    }));
    let workflows = call_tool(
        app.clone(),
        "list_workflows",
        json!({"project_id": "project-visible"}),
    )
    .await;
    assert_eq!(
        workflows["data"]["items"][0]["name"],
        "Shared owner library"
    );
    assert_eq!(
        workflows["meta"]["links"][0]["href"],
        format!("{}/workflows/workflow-1", upstream.base_url)
    );
    assert_eq!(
        upstream.state.requests().last().unwrap(),
        "/api/v1/workflows?project=project-visible"
    );

    upstream.state.set_body(json!({
        "items": [{"id": "label-1", "name": "docs", "issue_count": 4}]
    }));
    let labels = call_tool(app.clone(), "list_labels", json!({})).await;
    assert_eq!(labels["data"]["items"][0]["name"], "docs");
    assert_eq!(labels["data"]["next_cursor"], Value::Null);
    assert_eq!(upstream.state.requests().last().unwrap(), "/api/v1/labels");
    assert_eq!(upstream.state.requests().len(), 3);
    upstream.task.abort();
}

#[tokio::test]
async fn single_project_and_workflow_reads_map_ids_without_extra_requests() {
    let upstream = start_mock(json!({
        "id": "project-a",
        "owner": {"id": "owner-a", "name": "Owner"},
        "viewer_role": "member",
        "capabilities": {"read": true, "execute": false}
    }))
    .await;
    let app = configured_app(&upstream, &[]);
    let project = call_tool(
        app.clone(),
        "get_project",
        json!({"project_id": "project-a"}),
    )
    .await;
    assert_eq!(project["data"]["viewer_role"], "member");
    assert_eq!(
        project["meta"]["links"][0]["href"],
        format!("{}/projects/project-a", upstream.base_url)
    );
    assert_eq!(upstream.state.requests(), ["/api/v1/projects/project-a"]);

    upstream.state.set_body(json!({
        "id": "workflow-a",
        "revision": 9,
        "states": [{"id": "state-a", "category": "backlog"}],
        "transitions": []
    }));
    let workflow = call_tool(app, "get_workflow", json!({"workflow_id": "workflow-a"})).await;
    assert_eq!(workflow["data"]["revision"], 9);
    assert_eq!(
        workflow["meta"]["links"][0]["href"],
        format!("{}/workflows/workflow-a", upstream.base_url)
    );
    assert_eq!(
        upstream.state.requests().last().unwrap(),
        "/api/v1/workflows/workflow-a"
    );
    assert_eq!(upstream.state.requests().len(), 2);
    upstream.task.abort();
}

#[tokio::test]
async fn event_list_maps_filters_time_bounds_and_open_type_strings() {
    let upstream = start_mock(json!({"items": [], "next_cursor": "next+/=opaque"})).await;
    let app = configured_app(&upstream, &[]);
    let result = call_tool(
        app,
        "list_events",
        json!({
            "project_id": "project-a",
            "issue_id": "issue-a",
            "event_types": ["issue.transitioned", "future.event_type"],
            "state": "state-a",
            "since": 1793000000123u64,
            "until": "2026-10-28T12:30:00Z",
            "limit": 25,
            "cursor": "before+/=opaque"
        }),
    )
    .await;
    assert_eq!(result["data"]["items"], json!([]));
    assert_eq!(result["data"]["next_cursor"], "next+/=opaque");
    assert_eq!(
        decoded_query(&upstream.state.requests()[0]),
        [
            ("project", "project-a"),
            ("issue", "issue-a"),
            ("type", "issue.transitioned,future.event_type"),
            ("state", "state-a"),
            ("since", "1793000000123"),
            ("until", "2026-10-28T12:30:00Z"),
            ("limit", "25"),
            ("cursor", "before+/=opaque")
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
    );
    upstream.task.abort();
}

#[tokio::test]
async fn event_list_rejects_an_empty_workflow_state_id() {
    let upstream = start_mock(json!({})).await;
    let app = configured_app(&upstream, &[]);

    let result = call_tool(app, "list_events", json!({"state": "  "})).await;

    assert_eq!(result["error"]["source"], "validation");
    assert_eq!(
        result["error"]["message"],
        "state: must be a non-empty workflow state ID"
    );
    assert!(upstream.state.requests().is_empty());
    upstream.task.abort();
}

#[tokio::test]
async fn event_project_exclusions_check_recorded_and_current_issue_attribution() {
    let upstream = start_mock(json!({
        "items": [
            {"id": "historical-hidden", "project_id": "project-hidden", "issue_id": null},
            {
                "id": "current-hidden-from-reference",
                "project_id": "project-visible",
                "issue_id": "issue-ref-hidden",
                "issue_ref": {"project_id": "project-hidden"}
            },
            {
                "id": "current-hidden-from-lookup",
                "project_id": "project-visible",
                "issue_id": "issue-lookup-hidden",
                "issue_ref": null
            },
            {"id": "workspace", "project_id": null, "issue_id": null, "project_name": null},
            {"id": "visible", "project_id": "project-visible", "issue_id": null}
        ],
        "next_cursor": null
    }))
    .await;
    upstream.state.set_path_response(
        "/api/v1/issues/issue-lookup-hidden",
        StatusCode::OK,
        json!({"id": "issue-lookup-hidden", "project_id": "project-hidden"}),
    );
    let app = configured_app(&upstream, &["project-hidden"]);

    let result = call_tool(app, "list_events", json!({})).await;
    assert_eq!(
        result["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["workspace", "visible"]
    );
    assert_eq!(
        upstream.state.requests(),
        ["/api/v1/events", "/api/v1/issues/issue-lookup-hidden"]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn event_excluded_only_page_keeps_its_cursor_for_the_next_native_page() {
    let upstream = start_mock(json!({
        "items": [{"id": "hidden", "project_id": "project-hidden", "issue_id": null}],
        "next_cursor": "continue+/=opaque"
    }))
    .await;
    let app = configured_app(&upstream, &["project-hidden"]);

    let first = call_tool(app.clone(), "list_events", json!({"limit": 1})).await;
    assert_eq!(first["data"]["items"], json!([]));
    assert_eq!(first["data"]["next_cursor"], "continue+/=opaque");

    upstream.state.set_body(json!({
        "items": [{"id": "visible", "project_id": "project-visible", "issue_id": null}],
        "next_cursor": null
    }));
    let second = call_tool(
        app,
        "list_events",
        json!({"limit": 1, "cursor": "continue+/=opaque"}),
    )
    .await;
    assert_eq!(second["data"]["items"][0]["id"], "visible");
    assert_eq!(second["data"]["next_cursor"], Value::Null);
    assert_eq!(
        decoded_query(&upstream.state.requests()[1]),
        [
            ("limit".to_owned(), "1".to_owned()),
            ("cursor".to_owned(), "continue+/=opaque".to_owned())
        ]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn event_attribution_lookup_failure_fails_the_whole_page() {
    let upstream = start_mock(json!({
        "items": [{
            "id": "cannot-attribute",
            "project_id": "project-visible",
            "issue_id": "issue-unavailable",
            "issue_ref": null
        }],
        "next_cursor": "opaque-next"
    }))
    .await;
    upstream.state.set_path_response(
        "/api/v1/issues/issue-unavailable",
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error": {"code": "internal_error", "message": "lookup failed"}}),
    );
    let app = configured_app(&upstream, &["project-hidden"]);

    let result = call_tool(app, "list_events", json!({})).await;
    assert_eq!(result["error"]["source"], "upstream");
    assert_eq!(result["error"]["status"], 500);
    assert!(result["data"].is_null());
    assert_eq!(upstream.state.requests().len(), 2);
    upstream.task.abort();
}

#[tokio::test]
async fn context_list_maps_scope_filters_and_preserves_the_native_cursor() {
    let upstream = start_mock(json!({"items": [], "next_cursor": "next+/="})).await;
    let app = configured_app(&upstream, &[]);
    let result = call_tool(
        app,
        "list_context",
        json!({
            "project_id": "project-a",
            "state_id": "state-a",
            "label_id": "label-a",
            "issue_id": "issue-a",
            "kind": "skill",
            "search": "runners & clients",
            "exact": true,
            "archived": "all",
            "limit": 25,
            "cursor": "opaque+/="
        }),
    )
    .await;

    assert_eq!(result["data"]["next_cursor"], "next+/=");
    assert_eq!(
        decoded_query(&upstream.state.requests()[0]),
        [
            ("project", "project-a"),
            ("state", "state-a"),
            ("label", "label-a"),
            ("issue", "issue-a"),
            ("kind", "skill"),
            ("q", "runners & clients"),
            ("exact", "true"),
            ("archived", "all"),
            ("limit", "25"),
            ("cursor", "opaque+/="),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
    );
    upstream.task.abort();
}

#[tokio::test]
async fn context_list_resolves_issue_ownership_and_filters_excluded_scopes() {
    let upstream = start_mock(json!({})).await;
    upstream.state.set_response(
        "/api/v1/context",
        json!({
            "items": [
                {
                    "id": "issue-context",
                    "scope": {"project_id": null, "issue_id": "issue-member"},
                    "version": 3
                },
                {
                    "id": "project-context",
                    "scope": {"project_id": "project-hidden", "issue_id": null},
                    "version": 2
                },
                {
                    "id": "workspace-context",
                    "scope": {"project_id": null, "issue_id": null},
                    "version": 1
                }
            ],
            "next_cursor": "continue"
        }),
    );
    upstream.state.set_response(
        "/api/v1/issues/issue-member",
        json!({
            "mode": "member",
            "id": "issue-member",
            "project": {"id": "project-hidden", "name": "Hidden", "owner": {"id": "owner"}}
        }),
    );
    let app = configured_app(&upstream, &["project-hidden"]);

    let result = call_tool(app, "list_context", json!({})).await;

    assert_eq!(
        result["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["workspace-context"]
    );
    assert_eq!(result["data"]["next_cursor"], "continue");
    assert_eq!(
        upstream.state.requests(),
        ["/api/v1/context", "/api/v1/issues/issue-member"]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn context_issue_scope_filter_resolves_current_project_before_listing() {
    let upstream = start_mock(json!({"items": [], "next_cursor": null})).await;
    upstream.state.set_response(
        "/api/v1/issues/issue-member",
        json!({
            "mode": "member",
            "id": "issue-member",
            "project": {"id": "project-visible", "name": "Shared", "owner": {"id": "owner"}}
        }),
    );
    let app = configured_app(&upstream, &["project-hidden"]);

    call_tool(app, "list_context", json!({"issue_id": "issue-member"})).await;

    assert_eq!(
        upstream.state.requests(),
        [
            "/api/v1/issues/issue-member",
            "/api/v1/context?issue=issue-member"
        ]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn context_scope_refuses_to_return_data_when_issue_ownership_cannot_be_resolved() {
    let upstream = start_mock(json!({})).await;
    upstream.state.set_response(
        "/api/v1/context",
        json!({
            "items": [{
                "id": "context-item",
                "scope": {"project_id": null, "issue_id": "issue-unresolved"},
                "version": 1
            }],
            "next_cursor": null
        }),
    );
    upstream.state.set_response(
        "/api/v1/issues/issue-unresolved",
        json!({"mode": "member", "id": "issue-unresolved", "project": {"owner": {"id": "owner"}}}),
    );
    let app = configured_app(&upstream, &["project-hidden"]);

    let result = call_tool(app, "list_context", json!({})).await;

    assert_eq!(result["error"]["source"], "schema");
    assert_eq!(result["error"]["code"], "invalid_upstream_shape");
    assert_eq!(
        upstream.state.requests(),
        ["/api/v1/context", "/api/v1/issues/issue-unresolved"]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn get_context_preserves_versions_and_redacted_fields_without_inventing_values() {
    let context = json!({
        "id": "context-env",
        "kind": "env",
        "name": "TYPESAFE_API_KEY",
        "description": "Run credential",
        "scope": {"project_id": "project-visible", "issue_id": null},
        "secret": true,
        "value_set": true,
        "hint": "managed by runner",
        "position": 0,
        "version": 7,
        "created_at": 100,
        "updated_at": 200
    });
    let upstream = start_mock(json!({})).await;
    upstream
        .state
        .set_response("/api/v1/context/context-env", context.clone());
    let app = configured_app(&upstream, &["project-hidden"]);

    let result = call_tool(app, "get_context", json!({"context_id": "context-env"})).await;

    assert_eq!(result["data"], context);
    assert!(result["data"].get("value").is_none());
    assert_eq!(result["data"]["version"], 7);
    assert_eq!(upstream.state.requests(), ["/api/v1/context/context-env"]);
    upstream.task.abort();
}

#[tokio::test]
async fn issue_context_and_journal_preserve_native_projections_and_call_only_their_routes() {
    let effective_context = json!({
        "prompt": {
            "text": "inherited instructions",
            "parts": [{
                "item_id": "prompt-base",
                "name": "Shared setup",
                "body": "inherited instructions",
                "version": 4,
                "is_journal": false,
                "inherited_from": {
                    "state_id": "state-base",
                    "state_name": "Base",
                    "workflow_id": "workflow-a",
                    "workflow_name": "Standard"
                }
            }],
            "journal": {"item_id": "journal-a", "version": 2}
        },
        "skills": [],
        "repos": [],
        "env": [],
        "overridden": [],
        "conflicts": [],
        "shared_bundle": {
            "version": 1,
            "digest": "digest-a",
            "owner": {"id": "owner-a", "name": "Owner"}
        }
    });
    let journal = json!({
        "scope": {"project_id": "project-a", "workflow_state_id": "state-a", "label": "Project A · Doing"},
        "anchor": "current",
        "note": null,
        "item": null
    });
    let upstream = start_mock(json!({})).await;
    upstream
        .state
        .set_response("/api/v1/issues/issue-a/context", effective_context.clone());
    upstream
        .state
        .set_response("/api/v1/issues/issue-a/journal", journal.clone());
    let app = configured_app(&upstream, &[]);

    let context_result = call_tool(
        app.clone(),
        "get_issue_context",
        json!({"issue_id": "issue-a"}),
    )
    .await;
    assert_eq!(context_result["data"], effective_context);
    assert_eq!(
        upstream.state.requests(),
        ["/api/v1/issues/issue-a/context"]
    );

    let journal_result = call_tool(app, "get_issue_journal", json!({"issue_id": "issue-a"})).await;
    assert_eq!(journal_result["data"], journal);
    assert_eq!(
        upstream.state.requests(),
        [
            "/api/v1/issues/issue-a/context",
            "/api/v1/issues/issue-a/journal"
        ]
    );
    upstream.task.abort();
}

#[tokio::test]
async fn issue_context_ownership_resolution_fails_closed_for_redacted_member_projects() {
    let upstream = start_mock(json!({})).await;
    upstream.state.set_response(
        "/api/v1/issues/issue-member",
        json!({
            "mode": "member",
            "id": "issue-member",
            "project": {"owner": {"id": "owner"}}
        }),
    );
    let app = configured_app(&upstream, &["project-hidden"]);

    let result = call_tool(
        app,
        "get_issue_context",
        json!({"issue_id": "issue-member"}),
    )
    .await;

    assert_eq!(result["error"]["source"], "schema");
    assert_eq!(result["error"]["code"], "invalid_upstream_shape");
    assert_eq!(upstream.state.requests(), ["/api/v1/issues/issue-member"]);
    upstream.task.abort();
}

#[tokio::test]
async fn direct_context_and_issue_context_reads_block_excluded_ownership() {
    let upstream = start_mock(json!({})).await;
    upstream.state.set_response(
        "/api/v1/context/context-hidden",
        json!({
            "id": "context-hidden",
            "scope": {"project_id": "project-hidden", "issue_id": null},
            "version": 1
        }),
    );
    upstream.state.set_response(
        "/api/v1/issues/issue-hidden",
        json!({
            "mode": "member",
            "id": "issue-hidden",
            "project": {"id": "project-hidden", "name": "Hidden", "owner": {"id": "owner"}}
        }),
    );
    let app = configured_app(&upstream, &["project-hidden"]);

    for (name, args) in [
        ("get_context", json!({"context_id": "context-hidden"})),
        ("get_issue_context", json!({"issue_id": "issue-hidden"})),
        ("get_issue_journal", json!({"issue_id": "issue-hidden"})),
    ] {
        let result = call_tool(app.clone(), name, args).await;
        assert_eq!(result["error"]["source"], "visibility");
    }

    assert_eq!(
        upstream.state.requests(),
        [
            "/api/v1/context/context-hidden",
            "/api/v1/issues/issue-hidden",
            "/api/v1/issues/issue-hidden"
        ]
    );
    upstream.task.abort();
}
