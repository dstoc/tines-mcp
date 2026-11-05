mod support;

use serde_json::{Value, json};
use support::direct_calls::{self, BodyFixture, ResponseFixture, inspect};
use tines_mcp::tools::{self, contract::*, reads};

fn prepare(name: &str, args: Value) -> PreparedCall {
    tools::prepare(
        name,
        args.as_object().unwrap().clone(),
        &LocalLimits::default(),
    )
    .unwrap()
}

fn response(body: ResponseBody) -> UpstreamResponse {
    UpstreamResponse {
        status: 200,
        observed_at_ms: 123,
        headers: ResponseHeaders::default(),
        body,
    }
}

fn json_project(plan: &PreparedCall, data: Value) -> Projected {
    plan.project(response(ResponseBody::Json(data))).unwrap()
}

fn bytes_project(
    plan: &PreparedCall,
    bytes: &[u8],
    mime: &str,
) -> Result<Projected, ProjectionError> {
    plan.project(UpstreamResponse {
        headers: ResponseHeaders {
            content_type: Some(mime.into()),
            content_length: Some(bytes.len() as u64),
            content_disposition: Some("attachment; filename=example".into()),
            retry_after: None,
        },
        ..response(ResponseBody::Bytes(bytes.to_vec()))
    })
}

fn samples() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("list_projects", "default", json!({})),
        ("get_project", "default", json!({"project_id":"p"})),
        ("list_issues", "default", json!({})),
        ("get_issue", "id", json!({"issue_id":"i"})),
        (
            "get_issue",
            "project_number",
            json!({"project_id":"p","number":7}),
        ),
        ("list_issue_artifacts", "default", json!({"issue_id":"i"})),
        (
            "get_issue_artifact",
            "default",
            json!({"issue_id":"i","name":"proposal"}),
        ),
        (
            "read_artifact_content",
            "default",
            json!({"issue_id":"i","name":"proposal"}),
        ),
        ("list_workflows", "default", json!({})),
        ("get_workflow", "default", json!({"workflow_id":"w"})),
        ("list_labels", "default", json!({})),
        ("list_events", "default", json!({})),
        ("list_context", "default", json!({})),
        ("get_context", "default", json!({"context_id":"c"})),
        ("get_issue_context", "default", json!({"issue_id":"i"})),
        ("get_issue_journal", "default", json!({"issue_id":"i"})),
        ("list_runs", "default", json!({})),
        ("get_run", "default", json!({"run_id":"r"})),
        ("get_run_log", "tail", json!({"run_id":"r","mode":"tail"})),
        ("get_run_log", "full", json!({"run_id":"r","mode":"full"})),
        ("explain_dispatch", "default", json!({"issue_id":"i"})),
    ]
}

#[test]
fn every_read_variant_builds_one_get_matching_the_approved_catalog() {
    let catalog: Value =
        serde_json::from_str(include_str!("fixtures/direct-api-contracts.json")).unwrap();
    let endpoints: Vec<_> = catalog["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["module"] == "reads")
        .collect();
    assert_eq!(samples().len(), endpoints.len());
    let projected = direct_calls::project(
        &prepare("get_project", json!({"project_id":"p"})),
        ResponseFixture {
            status: 200,
            observed_at_ms: 123,
            content_type: None,
            body: BodyFixture::Json {
                value: json!({"id":"p"}),
            },
        },
    )
    .unwrap();
    assert_eq!(projected.data, json!({"id":"p"}));
    for (name, variant, args) in samples() {
        let plan = prepare(name, args);
        let fixture = inspect(&plan);
        let contract = endpoints
            .iter()
            .find(|e| e["tool"] == name && e["variant"] == variant)
            .unwrap();
        assert_eq!(fixture.method, Method::Get, "{name}/{variant}");
        assert_eq!(fixture.template, contract["path"], "{name}/{variant}");
        assert_eq!(fixture.body, BodyFixture::Empty);
        assert_eq!(plan.response.statuses, &[200]);
        assert!(plan.noncommitting_errors.is_empty());
        match contract["success"]["kind"].as_str().unwrap() {
            "json" => assert_eq!(plan.response.kind, ResponseKind::Json),
            "bytes" => assert_eq!(
                plan.response.kind,
                ResponseKind::Bytes {
                    max_bytes: MAX_CONTENT_SOURCE_BYTES
                }
            ),
            _ => panic!("unexpected catalog kind"),
        }
    }
}

#[test]
fn discovery_is_read_only_strict_and_matches_preparation() {
    let definitions = reads::definitions(&LocalLimits::default());
    assert_eq!(definitions.len(), 19);
    for tool in definitions {
        assert_eq!(
            tool.input_schema.get("additionalProperties"),
            Some(&json!(false))
        );
        assert!(tool.output_schema.is_some());
        let annotations = serde_json::to_value(tool.annotations.unwrap()).unwrap();
        assert_eq!(annotations["readOnlyHint"], true);
        assert_eq!(annotations["destructiveHint"], false);
        assert_eq!(annotations["idempotentHint"], true);
        let (_, _, mut args) = samples()
            .into_iter()
            .find(|(name, _, _)| *name == tool.name)
            .unwrap();
        args["unadvertised"] = json!("do another request");
        assert!(
            reads::prepare(
                &tool.name,
                args.as_object().unwrap().clone(),
                &LocalLimits::default()
            )
            .is_err()
        );
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object);
        for (key, property) in properties.into_iter().flatten() {
            let (_, _, mut args) = samples()
                .into_iter()
                .find(|(name, _, _)| *name == tool.name)
                .unwrap();
            args[key] = Value::Null;
            assert!(
                reads::prepare(
                    &tool.name,
                    args.as_object().unwrap().clone(),
                    &LocalLimits::default()
                )
                .is_err(),
                "{} accepts null for {key}",
                tool.name
            );
            assert_ne!(property.get("type"), Some(&json!(["string", "null"])));
        }
    }
}

#[test]
fn issue_filters_are_native_and_repeat_labels_without_changing_values() {
    let p = prepare(
        "list_issues",
        json!({
            "project_id":"p", "workflow_id":"w", "state":"Open", "category":"active", "schedule_id":"s",
            "labels":["one & two", "three"], "search":"literal ?&ü", "ready":false,
            "hide_done":true, "hide_duplicates":false, "archived":"all", "descriptions":true,
            "limit":100, "cursor":"opaque+&="
        }),
    );
    assert_eq!(
        p.query,
        [
            ("project", "p"),
            ("workflow", "w"),
            ("state", "Open"),
            ("category", "active"),
            ("schedule", "s"),
            ("label", "one & two"),
            ("label", "three"),
            ("q", "literal ?&ü"),
            ("ready", "false"),
            ("hide_done", "true"),
            ("hide_duplicates", "false"),
            ("archived", "all"),
            ("brief", "0"),
            ("limit", "100"),
            ("cursor", "opaque+&="),
        ]
        .map(|(k, v)| (k.into(), v.into()))
    );
    assert_eq!(
        prepare("list_issues", json!({})).query,
        [("brief".into(), "1".into())]
    );
    let native = json!({"items":[{"id":"i","description":"body"}],"next_cursor":null});
    assert!(json_project(&p, native.clone()).omissions.is_empty());
    let brief = json_project(&prepare("list_issues", json!({})), native.clone());
    assert_eq!(brief.data, native);
    assert_eq!(brief.omissions.len(), 1);
}

#[test]
fn event_context_and_run_filters_preserve_native_names_and_time_bounds() {
    let events = prepare(
        "list_events",
        json!({"project_id":"p", "issue_id":"i", "event_types":["issue.created","new.kind"],"state":"s","since":42,"until":"2026-11-05T00:00:00Z","limit":2,"cursor":"event-cursor"}),
    );
    assert_eq!(
        events.query,
        [
            ("project", "p"),
            ("issue", "i"),
            ("type", "issue.created,new.kind"),
            ("state", "s"),
            ("since", "42"),
            ("until", "2026-11-05T00:00:00Z"),
            ("limit", "2"),
            ("cursor", "event-cursor")
        ]
        .map(|(k, v)| (k.into(), v.into()))
    );
    let context = prepare(
        "list_context",
        json!({"project_id":"p","state_id":"s","label_id":"l","issue_id":"i","kind":"env","search":"literal", "exact":false,"archived":"true","limit":1,"cursor":"context-cursor"}),
    );
    assert_eq!(
        context.query,
        [
            ("project", "p"),
            ("state", "s"),
            ("label", "l"),
            ("issue", "i"),
            ("kind", "env"),
            ("q", "literal"),
            ("exact", "false"),
            ("archived", "true"),
            ("limit", "1"),
            ("cursor", "context-cursor")
        ]
        .map(|(k, v)| (k.into(), v.into()))
    );
    let runs = prepare(
        "list_runs",
        json!({"issue_id":"i","project_id":"p","runner_id":"r","workflow_id":"w","state_id":"s","active":true,"limit":3,"cursor":"run-cursor"}),
    );
    assert_eq!(
        runs.query,
        [
            ("issue", "i"),
            ("project", "p"),
            ("runner", "r"),
            ("workflow", "w"),
            ("state", "s"),
            ("active", "1"),
            ("limit", "3"),
            ("cursor", "run-cursor")
        ]
        .map(|(k, v)| (k.into(), v.into()))
    );
    assert!(prepare("list_runs", json!({})).query.is_empty());
}

#[test]
fn path_segments_and_selectors_are_unescaped_until_execution() {
    let p = prepare(
        "read_artifact_content",
        json!({"issue_id":"i/ü","name":"a?&/#","version":3,"path":"folder/space ü.txt"}),
    );
    assert_eq!(
        p.endpoint.segments(),
        ["issues", "i/ü", "artifacts", "a?&/#", "content"]
    );
    assert_eq!(
        p.query,
        [
            ("version".into(), "3".into()),
            ("path".into(), "folder/space ü.txt".into())
        ]
    );
    assert_eq!(
        prepare("get_issue", json!({"project_id":"p","number":7}))
            .endpoint
            .segments(),
        ["projects", "p", "issues", "7"]
    );
    assert_eq!(
        prepare("list_projects", json!({"archived":"false"})).query,
        [("archived".into(), "false".into())]
    );
    assert_eq!(
        prepare("list_workflows", json!({"project_id":"p"})).query,
        [("project".into(), "p".into())]
    );
}

#[test]
fn invalid_inputs_fail_locally_before_a_plan_exists() {
    for (name, args) in [
        ("get_issue", json!({})),
        ("get_issue", json!({"issue_id":"i","number":1})),
        ("get_issue", json!({"project_id":"p","number":0})),
        ("get_project", json!({"project_id":".."})),
        ("get_run", json!({"run_id":" "})),
        ("list_issues", json!({"limit":0})),
        ("list_events", json!({"limit":101})),
        ("list_context", json!({"cursor":""})),
        ("list_runs", json!({"cursor":"x".repeat(4097)})),
        ("list_runs", json!({"active":false})),
        ("list_runs", json!({"state_id":"s"})),
        ("list_events", json!({"event_types":[" "]})),
        ("list_events", json!({"since":""})),
        (
            "read_artifact_content",
            json!({"issue_id":"i","name":"a","version":0}),
        ),
        (
            "read_artifact_content",
            json!({"issue_id":"i","name":"a","offset":1}),
        ),
        ("get_run_log", json!({"run_id":"r","mode":"auto"})),
        (
            "get_run_log",
            json!({"run_id":"r","mode":"tail","chunk_bytes":3}),
        ),
        (
            "get_run_log",
            json!({"run_id":"r","mode":"full","expected_sha256":"bad"}),
        ),
        (
            "get_run_log",
            json!({"run_id":"r","mode":"full","offset":1048577,"expected_sha256":"0".repeat(64)}),
        ),
    ] {
        assert!(
            reads::prepare(
                name,
                args.as_object().unwrap().clone(),
                &LocalLimits::default()
            )
            .is_err(),
            "{name}: {args}"
        );
    }
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        "a/./b",
        "a//b",
        "a\\b",
        "a\0b",
    ] {
        assert!(
            reads::prepare(
                "read_artifact_content",
                json!({"issue_id":"i","name":"a","path":path})
                    .as_object()
                    .unwrap()
                    .clone(),
                &LocalLimits::default()
            )
            .is_err()
        );
    }
}

#[test]
fn all_native_pages_keep_empty_items_with_a_live_cursor() {
    for name in ["list_issues", "list_events", "list_context", "list_runs"] {
        let p = prepare(name, json!({"cursor":"previous"}));
        let native = json!({"items":[],"next_cursor":"continue","new_field":null});
        assert_eq!(json_project(&p, native.clone()).data, native);
        let repeated = p.project(response(ResponseBody::Json(
            json!({"items":[],"next_cursor":"previous"}),
        )));
        assert_eq!(repeated.err().unwrap().code, "repeated_cursor");
        assert!(
            p.project(response(ResponseBody::Json(json!({"items":[]}))))
                .is_err()
        );
    }
    for name in [
        "list_projects",
        "list_workflows",
        "list_labels",
        "list_issue_artifacts",
    ] {
        let args = if name == "list_issue_artifacts" {
            json!({"issue_id":"i"})
        } else {
            json!({})
        };
        let native = json!({"items":[]});
        assert_eq!(
            json_project(&prepare(name, args), native.clone()).data,
            native
        );
    }
}

#[test]
fn owner_member_missing_null_redacted_and_unknown_fields_are_preserved() {
    let native = [
        json!({"id":"i","project_id":"current","number":7,"effective_state":{"id":"s"},"open_blockers":[],"active_run":null}),
        json!({"id":"i","mode":"member","project":{"id":"current","owner":{"id":"owner"}},"number":7,"latest_run":{"status":"running","outcome":null},"capabilities":{"execute":false}}),
        json!({"id":"i","mode":"member","project":null,"sensitive":"[REDACTED]","new_native_field":{"arbitrary":true}}),
    ];
    for data in native {
        let result = json_project(&prepare("get_issue", json!({"issue_id":"i"})), data.clone());
        assert_eq!(result.data, data);
        assert_eq!(
            result.links.is_empty(),
            data.get("project").is_some_and(Value::is_null)
        );
    }
    let project = json!({"id":"p","mode":"member","owner":{"id":"u"},"redacted":null});
    assert_eq!(
        json_project(
            &prepare("get_project", json!({"project_id":"p"})),
            project.clone()
        )
        .data,
        project
    );
    let context = json!({"id":"c","kind":"env","version":4,"secret":true,"redacted":true});
    assert_eq!(
        json_project(
            &prepare("get_context", json!({"context_id":"c"})),
            context.clone()
        )
        .data,
        context
    );
}

#[test]
fn links_use_only_current_observed_coordinates_without_parent_enrichment() {
    let p = prepare("get_issue", json!({"project_id":"old","number":3}));
    let data = json!({"id":"i","project_id":"new/ü","number":7});
    let out = json_project(&p, data);
    assert_eq!(out.links[0]["href"], "/issues/new%2F%C3%BC/7");
    let events = json!({"items":[{"project_id":"historical","issue_ref":{"project_id":"current","number":8}}, {"project_id":"historical","issue_ref":{"number":9}}, {"project_id":null}],"next_cursor":null});
    let out = json_project(&prepare("list_events", json!({})), events.clone());
    assert_eq!(out.data, events);
    assert_eq!(out.links.len(), 1);
    assert_eq!(out.links[0]["href"], "/issues/current/8");
    for (name, args, data) in [
        (
            "get_run",
            json!({"run_id":"r"}),
            json!({"id":"r","issue_id":"i","status":"running","outcome":null}),
        ),
        ("get_issue", json!({"issue_id":"i"}), json!({"id":"i"})),
        (
            "explain_dispatch",
            json!({"issue_id":"i"}),
            json!({"eligible":false}),
        ),
        (
            "get_issue_artifact",
            json!({"issue_id":"i","name":"a"}),
            json!({"id":"a","issue_id":"i","name":"a","artifact_type":"text","current_version":{"version":2},"versions":[]}),
        ),
    ] {
        assert!(json_project(&prepare(name, args), data).links.is_empty());
    }
}

#[test]
fn artifact_metadata_and_version_history_are_returned_without_content_fetches() {
    let p = prepare(
        "get_issue_artifact",
        json!({"issue_id":"i","name":"proposal"}),
    );
    let data = json!({"id":"a","issue_id":"i","name":"proposal","artifact_type":"text","current_version":{"version":2},"versions":[{"version":1},{"version":2}],"new_native":null});
    assert_eq!(json_project(&p, data.clone()).data, data);
    assert_eq!(
        p.endpoint.segments(),
        ["issues", "i", "artifacts", "proposal"]
    );
}

#[test]
fn artifact_bytes_report_supplied_selectors_and_observed_headers_only() {
    for selectors in [json!({}), json!({"version":2,"path":"folder/a.txt"})] {
        let mut args = json!({"issue_id":"i","name":"a"});
        args.as_object_mut()
            .unwrap()
            .extend(selectors.as_object().unwrap().clone());
        let p = prepare("read_artifact_content", args);
        let result = bytes_project(&p, b"hello", "text/plain; charset=utf-8").unwrap();
        assert_eq!(result.data["content"], "hello");
        assert_eq!(
            result.data["source"].get("version"),
            selectors.get("version")
        );
        assert_eq!(result.data["source"].get("path"), selectors.get("path"));
        assert_eq!(
            result.data["headers"],
            json!({"content_type":"text/plain; charset=utf-8","content_length":5,"content_disposition":"attachment; filename=example"})
        );
        assert!(result.data["source"].get("artifact_id").is_none());
        assert!(result.data.get("artifact").is_none());
        assert_eq!(result.data["total_bytes"], 5);
        assert_eq!(
            result.data["sha256"],
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
    let legacy = json!({"issue_id":"i","name":"a","expected_artifact_id":"a"});
    assert!(
        reads::prepare(
            "read_artifact_content",
            legacy.as_object().unwrap().clone(),
            &LocalLimits::default()
        )
        .is_err()
    );
    let definition = reads::definitions(&LocalLimits::default())
        .into_iter()
        .find(|t| t.name == "read_artifact_content")
        .unwrap();
    assert!(
        definition.input_schema["properties"]
            .get("expected_artifact_id")
            .is_none()
    );
}

#[test]
fn bounded_continuations_keep_optional_version_and_detect_changing_digests() {
    let first = bytes_project(
        &prepare(
            "read_artifact_content",
            json!({"issue_id":"i","name":"a","chunk_bytes":4}),
        ),
        b"abcdef",
        "text/plain",
    )
    .unwrap();
    assert_eq!(first.data["content"], "abcd");
    assert_eq!(first.data["next_offset"], 4);
    let p = prepare(
        "read_artifact_content",
        json!({"issue_id":"i","name":"a","chunk_bytes":4,"offset":4,"expected_sha256":first.data["sha256"]}),
    );
    assert!(p.query.is_empty());
    let next = bytes_project(&p, b"abcdef", "text/plain").unwrap();
    assert_eq!(next.data["content"], "ef");
    assert_eq!(next.data["next_offset"], Value::Null);
    assert!(next.data["source"].get("version").is_none());
    assert_eq!(
        bytes_project(&p, b"abc", "text/plain").err().unwrap().code,
        "content_changed"
    );
    assert_eq!(
        bytes_project(&p, b"abcdeg", "text/plain")
            .err()
            .unwrap()
            .code,
        "content_changed"
    );
}

#[test]
fn utf8_boundaries_binary_encoding_and_source_limits_are_enforced() {
    let p = prepare(
        "read_artifact_content",
        json!({"issue_id":"i","name":"a","chunk_bytes":4}),
    );
    let first = bytes_project(&p, "abc☃z".as_bytes(), "text/plain").unwrap();
    assert_eq!(first.data["content"], "abc");
    assert_eq!(first.data["next_offset"], 3);
    let bad_offset = prepare(
        "read_artifact_content",
        json!({"issue_id":"i","name":"a","offset":4,"expected_sha256":first.data["sha256"]}),
    );
    assert_eq!(
        bytes_project(&bad_offset, "abc☃z".as_bytes(), "text/plain")
            .err()
            .unwrap()
            .code,
        "invalid_input"
    );
    let binary = bytes_project(&p, &[0, 1, 2, 253, 254, 255], "application/octet-stream").unwrap();
    assert_eq!(binary.data["encoding"], "base64");
    assert_eq!(binary.data["content"], "AAEC/Q==");
    assert_eq!(binary.data["returned_length"], 4);
    assert_eq!(
        bytes_project(&p, &[255], "text/plain").err().unwrap().code,
        "invalid_text_content"
    );
    assert_eq!(
        bytes_project(&p, &vec![b'a'; MAX_CONTENT_SOURCE_BYTES + 1], "text/plain")
            .err()
            .unwrap()
            .code,
        "content_source_too_large"
    );
    assert!(bytes_project(&p, &vec![b'a'; MAX_CONTENT_SOURCE_BYTES], "text/plain").is_ok());
}

#[test]
fn tail_and_full_log_modes_choose_one_endpoint_before_projection() {
    let tail = prepare(
        "get_run_log",
        json!({"run_id":"r","mode":"tail","chunk_bytes":4}),
    );
    let full = prepare(
        "get_run_log",
        json!({"run_id":"r","mode":"full","chunk_bytes":4}),
    );
    assert_eq!(tail.endpoint.segments(), ["runs", "r"]);
    assert_eq!(tail.response.kind, ResponseKind::Json);
    assert_eq!(full.endpoint.segments(), ["runs", "r", "log"]);
    let detail = json!({"id":"r","log":"abcdef","status":"running","outcome":null,"log_bytes_dropped":12,"log_full_bytes":18,"log_expired":false});
    let result = json_project(&tail, detail);
    assert_eq!(result.data["content"], "abcd");
    assert_eq!(result.data["run"]["status"], "running");
    assert!(result.data["run"].get("log_raw_bytes").is_none());
    assert!(result.data["headers"].as_object().unwrap().is_empty());
    let result = bytes_project(&full, b"full log", "text/plain").unwrap();
    assert_eq!(result.data["content"], "full");
    assert!(result.data.get("run").is_none());
    // Errors are consumed by the executor. Even an accidental projection of a
    // failed full response cannot choose a tail endpoint or return a success.
    for status in [401, 403, 404, 410, 503] {
        let result = full.project(UpstreamResponse {
            status,
            ..response(ResponseBody::Bytes(b"native error".to_vec()))
        });
        assert_eq!(result.err().unwrap().code, "unexpected_status");
        assert_eq!(full.endpoint.segments(), ["runs", "r", "log"]);
    }
}

#[test]
fn malformed_shapes_and_mismatched_ids_fail_without_another_request_plan() {
    for (name, _, args) in samples() {
        let p = prepare(name, args);
        if p.response.kind == ResponseKind::Json {
            assert!(
                p.project(response(ResponseBody::Json(Value::Null)))
                    .is_err(),
                "{name}"
            );
        }
    }
    for (name, key) in [
        ("get_project", "project_id"),
        ("get_issue", "issue_id"),
        ("get_workflow", "workflow_id"),
        ("get_context", "context_id"),
        ("get_run", "run_id"),
    ] {
        let p = prepare(name, json!({key:"expected"}));
        assert!(
            p.project(response(ResponseBody::Json(json!({"id":"different"}))))
                .is_err()
        );
    }
}
