//! Full registry contract. Native semantics and the intended client remain separate gates.
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
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use tines_mcp::{
    config::Config,
    server,
    tines::TinesClient,
    tools::{self, contract::LocalLimits},
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/full-surface-discovery.json")).unwrap()
}
async fn rpc(app: Router, version: &str, method: &str, mut params: Value) -> Value {
    if version == "2026-07-28" {
        params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":version,
            "io.modelcontextprotocol/clientCapabilities":{},
            "io.modelcontextprotocol/clientInfo":{"name":"full-surface-acceptance","version":"1"}});
    }
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", version)
        .header("Mcp-Method", method);
    if let Some(name) = params.get("name").and_then(Value::as_str) {
        builder = builder.header("Mcp-Name", name);
    }
    let request = builder
        .body(Body::from(
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
        ))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        panic!(
            "{version} {method} {params}: {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body.get("error").is_none(), "{body}");
    body["result"].clone()
}

#[tokio::test]
async fn complete_discovery_matches_reviewed_schemas_and_annotations_on_both_protocols() {
    let config = Config::from_lookup(|_| None).unwrap();
    let mut expected = fixture();
    let output = expected["output_schema"].clone();
    for tool in expected["tools"].as_array_mut().unwrap() {
        tool["outputSchema"] = output.clone();
    }
    for version in ["2025-11-25", "2026-07-28"] {
        let result = rpc(
            server::router(&config, CancellationToken::new()),
            version,
            "tools/list",
            json!({}),
        )
        .await;
        let mut actual = result["tools"].as_array().unwrap().clone();
        actual.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        assert_eq!(actual, expected["tools"].as_array().unwrap().clone());
        assert!(result.get("nextCursor").is_none());
    }
    let definitions = tools::definitions(&LocalLimits::default());
    let catalog: Value =
        serde_json::from_str(include_str!("fixtures/direct-api-contracts.json")).unwrap();
    let catalog_names: BTreeSet<_> = catalog["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["tool"].as_str().unwrap())
        .collect();
    let names: BTreeSet<_> = definitions.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, catalog_names);
    assert_eq!(names.len(), definitions.len());
    assert_eq!(names.len(), 41);
    for tool in definitions {
        assert_eq!(
            tool.input_schema["additionalProperties"], false,
            "{}",
            tool.name
        );
        let annotations = tool.annotations.unwrap();
        assert_eq!(annotations.open_world_hint, Some(true), "{}", tool.name);
        let read = annotations.read_only_hint.unwrap();
        assert_eq!(annotations.idempotent_hint, Some(read), "{}", tool.name);
        if read {
            assert_eq!(annotations.destructive_hint, Some(false));
        }
        assert!(tool.output_schema.is_some());
    }
}

#[derive(Clone)]
struct Mock {
    requests: Arc<Mutex<Vec<(String, String)>>>,
    status: Arc<Mutex<u16>>,
}
async fn handler(State(state): State<Mock>, request: Request<Body>) -> Response {
    state
        .requests
        .lock()
        .unwrap()
        .push((request.method().to_string(), request.uri().to_string()));
    Response::builder().status(*state.status.lock().unwrap())
        .header("Content-Type", "application/json").header("Location", "/must-not-follow")
        .body(Body::from(r#"{"error":{"code":"native_failure","message":"Native failure","details":{"opaque":[null,"literal data"]}}}"#)).unwrap()
}

fn inputs() -> BTreeMap<String, Value> {
    let mut inputs = BTreeMap::new();
    for source in [
        include_str!("fixtures/context.json"),
        include_str!("fixtures/operations-read.json"),
        include_str!("fixtures/recovery.json"),
    ] {
        let fixture: Value = serde_json::from_str(source).unwrap();
        for case in fixture["successes"].as_array().unwrap() {
            inputs
                .entry(case["tool"].as_str().unwrap().to_owned())
                .or_insert_with(|| case["arguments"].clone());
        }
    }
    for (name, args) in [
        (
            "create_issue",
            json!({"project_id":"prj_a","title":"Backlog issue"}),
        ),
        (
            "edit_issue",
            json!({"issue_id":"iss_a","description":"Changed"}),
        ),
        (
            "append_issue_comment",
            json!({"issue_id":"iss_a","body":"Comment"}),
        ),
        (
            "transition_issue",
            json!({"issue_id":"iss_a","action":"Start"}),
        ),
        ("get_current_api_key_authority", json!({})),
        ("get_issue_consent", json!({"issue_id":"iss_a"})),
        (
            "add_issue_labels",
            json!({"issue_id":"iss_a","label_ids":["lbl_0123456789abcdef"]}),
        ),
        (
            "remove_issue_label",
            json!({"issue_id":"iss_a","label_id":"lbl_0123456789abcdef"}),
        ),
        (
            "add_issue_link",
            json!({"issue_id":"iss_a","related_issue_id":"iss_b","kind":"blocked_by"}),
        ),
        (
            "remove_issue_link",
            json!({"issue_id":"iss_a","link_id":"lnk_a"}),
        ),
        (
            "put_issue_artifact",
            json!({"issue_id":"iss_a","name":"proposal","type":"text","content":"Proposal"}),
        ),
        (
            "reaffirm_issue_artifact",
            json!({"issue_id":"iss_a","name":"proposal"}),
        ),
        (
            "upload_issue_artifact_file",
            json!({"issue_id":"iss_a","name":"proposal","filename":"proposal.md","content_type":"text/markdown","encoding":"utf8","content":"Proposal"}),
        ),
        ("list_projects", json!({})),
        ("get_project", json!({"project_id":"prj_a"})),
        ("list_issues", json!({})),
        ("get_issue", json!({"issue_id":"iss_a"})),
        ("list_issue_artifacts", json!({"issue_id":"iss_a"})),
        (
            "get_issue_artifact",
            json!({"issue_id":"iss_a","name":"proposal"}),
        ),
        (
            "read_artifact_content",
            json!({"issue_id":"iss_a","name":"proposal"}),
        ),
        ("list_workflows", json!({})),
        ("get_workflow", json!({"workflow_id":"wf_a"})),
        ("list_labels", json!({})),
        ("list_events", json!({})),
        ("list_context", json!({})),
        ("get_context", json!({"context_id":"ctx_a"})),
        ("get_issue_context", json!({"issue_id":"iss_a"})),
        ("get_issue_journal", json!({"issue_id":"iss_a"})),
        ("list_runs", json!({})),
        ("get_run", json!({"run_id":"run_a"})),
        ("get_run_log", json!({"run_id":"run_a","mode":"full"})),
        ("explain_dispatch", json!({"issue_id":"iss_a"})),
    ] {
        inputs.insert(name.into(), args);
    }
    inputs
}

#[tokio::test]
async fn every_advertised_tool_rejects_unknown_fields_and_sends_one_request_on_native_errors() {
    let state = Mock {
        requests: Default::default(),
        status: Arc::new(Mutex::new(403)),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mock = Router::new()
        .fallback(any(handler))
        .with_state(state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let config = Config::from_lookup(|name| match name {
        "TINES_BASE_URL" => Some(url.clone()),
        "TINES_API_TOKEN" => Some("fixture-token".into()),
        "TINES_ALLOW_HTTP" => Some("true".into()),
        _ => None,
    })
    .unwrap();
    // Fresh client per invocation avoids a local rate limit masking the HTTP assertion.
    let app = || {
        server::router_with_client(
            &config,
            Some(TinesClient::new(config.tines.as_ref().unwrap()).unwrap()),
            CancellationToken::new(),
        )
    };
    let cases = inputs();
    assert_eq!(
        cases.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        tools::definitions(&LocalLimits::default())
            .iter()
            .map(|t| t.name.as_ref())
            .collect()
    );
    for version in ["2025-11-25", "2026-07-28"] {
        let before = state.requests.lock().unwrap().len();
        rpc(app(), version, "tools/list", json!({})).await;
        assert_eq!(state.requests.lock().unwrap().len(), before);
        for (name, args) in &cases {
            let mut bad = args.clone();
            bad["unadvertised_field"] = json!(true);
            let result = rpc(
                app(),
                version,
                "tools/call",
                json!({"name":name,"arguments":bad}),
            )
            .await;
            assert_eq!(result["isError"], true, "{name}: {result}");
            assert_eq!(state.requests.lock().unwrap().len(), before, "{name}");
        }
        for (name, args) in &cases {
            let plan = tools::prepare(
                name,
                args.as_object().unwrap().clone(),
                &LocalLimits::default(),
            )
            .unwrap();
            for status in [302, 403, 429, 500] {
                *state.status.lock().unwrap() = status;
                let before = state.requests.lock().unwrap().len();
                let result = rpc(
                    app(),
                    version,
                    "tools/call",
                    json!({"name":name,"arguments":args}),
                )
                .await;
                assert_eq!(result["isError"], true, "{name}: {result}");
                let envelope = &result["structuredContent"];
                assert_eq!(envelope["meta"]["http_status"], status, "{name}");
                assert_eq!(envelope["error"]["code"], "native_failure");
                if plan.endpoint.method.is_write() {
                    assert_eq!(envelope["meta"]["write"]["outcome"], "unknown");
                }
                let requests = state.requests.lock().unwrap();
                assert_eq!(requests.len(), before + 1, "{name}: {status}");
                let method = format!("{:?}", plan.endpoint.method).to_uppercase();
                assert_eq!(requests.last().unwrap().0, method);
            }
        }
    }
    task.abort();
}
