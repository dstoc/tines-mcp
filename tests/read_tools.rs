use std::{
    collections::HashMap,
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
    requests: Arc<Mutex<Vec<String>>>,
    path_responses: Arc<Mutex<HashMap<String, (StatusCode, Value)>>>,
}

impl MockState {
    fn set_body(&self, body: Value) {
        *self.body.lock().unwrap() = body;
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
}

struct MockUpstream {
    base_url: String,
    state: MockState,
    task: JoinHandle<()>,
}

async fn mock_handler(State(state): State<MockState>, request: Request<Body>) -> Response {
    let uri = request.uri().to_string();
    state.requests.lock().unwrap().push(uri.clone());
    let path = uri.split('?').next().unwrap_or(&uri);
    let (status, body) = state
        .path_responses
        .lock()
        .unwrap()
        .get(path)
        .cloned()
        .unwrap_or_else(|| (StatusCode::OK, state.body.lock().unwrap().clone()));
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
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
        requests: Arc::new(Mutex::new(Vec::new())),
        path_responses: Arc::new(Mutex::new(HashMap::new())),
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
            "list_workflows",
            "get_workflow",
            "list_labels",
            "list_events",
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
