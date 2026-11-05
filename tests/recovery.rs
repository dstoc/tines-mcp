mod support;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use support::direct_calls::{self, BodyFixture, ResponseFixture};
use tines_mcp::tools::{self, contract::*, recovery};

#[derive(Deserialize)]
struct Fixtures {
    reference_revision: String,
    successes: Vec<Success>,
    errors: Vec<NativeError>,
}
#[derive(Deserialize)]
struct Success {
    name: String,
    tool: String,
    arguments: Map<String, Value>,
    response: ResponseFixture,
    receipt: Value,
}
#[derive(Deserialize)]
struct NativeError {
    tool: String,
    status: u16,
    expected_outcome: WriteOutcome,
    error: ResultError,
}
fn fixtures() -> Fixtures {
    serde_json::from_str(include_str!("fixtures/recovery.json")).unwrap()
}
fn prepare(tool: &str, args: Value) -> Result<PreparedCall, LocalError> {
    tools::prepare(
        tool,
        args.as_object().unwrap().clone(),
        &LocalLimits::default(),
    )
}

#[test]
fn recovery_plans_target_only_supplied_ids_with_one_bodyless_post() {
    // Raw reserved characters remain one segment; only the executor encodes them.
    for (tool, field, resource, operation) in [
        ("resume_issue", "issue_id", "issues", "resume"),
        ("cancel_run", "run_id", "runs", "cancel"),
    ] {
        let id = "immutable/with?reserved#% &ü";
        let plan = prepare(tool, json!({field:id})).unwrap();
        let inspected = direct_calls::inspect(&plan);
        assert_eq!(inspected.method, Method::Post);
        assert_eq!(
            inspected.template,
            format!("/{resource}/{{{field}}}/{operation}")
        );
        assert_eq!(inspected.segments, [resource, id, operation]);
        assert!(inspected.query.is_empty());
        assert_eq!(inspected.body, BodyFixture::Empty);
        assert_eq!(plan.response.statuses, [200]);
        assert_eq!(plan.response.kind, ResponseKind::Json);
        assert!(plan.noncommitting_errors.is_empty());
    }
}

#[test]
fn recovery_schemas_are_closed_and_runtime_rejects_unadvertised_inputs() {
    let definitions = recovery::definitions(&LocalLimits::default());
    assert_eq!(definitions.len(), 2);
    for tool in definitions {
        let field = if tool.name == "resume_issue" {
            "issue_id"
        } else {
            "run_id"
        };
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!([field]));
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
        assert_eq!(schema["properties"][field]["type"], "string");
        assert_eq!(schema["properties"][field]["minLength"], 1);
        let wire = serde_json::to_value(&tool).unwrap();
        assert_eq!(wire["annotations"]["readOnlyHint"], false);
        assert_eq!(wire["annotations"]["destructiveHint"], true);
        assert_eq!(wire["annotations"]["idempotentHint"], false);
        assert_eq!(wire["annotations"]["openWorldHint"], true);
        assert!(tool.output_schema.is_some());
        for args in [
            json!({}),
            json!({field:null}),
            json!({field:25}),
            json!({field:""}),
            json!({field:"."}),
            json!({field:".."}),
        ] {
            assert!(prepare(&tool.name, args).is_err());
        }
        for extra in [
            "body",
            "project_id",
            "number",
            "active",
            "force",
            "release_claim",
            "wait",
            "resume",
            "method",
            "path",
            "expected_revision",
        ] {
            let error = prepare(&tool.name, json!({field:"target",extra:true}))
                .err()
                .unwrap();
            assert_eq!(error.code, "invalid_input");
        }
        assert!(prepare(&tool.name, json!({field:"target"})).is_ok());
    }
    assert!(recovery::prepare("cancel_current", Map::new(), &LocalLimits::default()).is_err());
}

#[test]
fn native_successes_preserve_status_outcome_nulls_extensions_and_no_op_resume() {
    let fixtures = fixtures();
    assert_eq!(
        fixtures.reference_revision,
        "af6cb25febf1b45d7eff32251f78fcf6355aed4a"
    );
    assert_eq!(fixtures.successes.len(), 7);
    for fixture in fixtures.successes {
        let plan =
            recovery::prepare(&fixture.tool, fixture.arguments, &LocalLimits::default()).unwrap();
        let native = match &fixture.response.body {
            BodyFixture::Json { value } => value.clone(),
            _ => panic!("expected JSON fixture"),
        };
        let projected = direct_calls::project(&plan, fixture.response).unwrap();
        assert_eq!(projected.data, native, "{}", fixture.name);
        assert_eq!(
            serde_json::to_value(&projected.receipt).unwrap(),
            fixture.receipt
        );
        assert!(projected.links.is_empty() && projected.omissions.is_empty());
        // As in mcp-00's pure fixtures, envelope construction illustrates the
        // executor handoff, not a test of its dispatch or classification code.
        let envelope = ResultEnvelope {
            data: projected.data,
            error: None,
            meta: ResultMeta {
                http_status: Some(200),
                observed_at_ms: 42,
                omissions: vec![],
                links: vec![],
                retry_after: None,
                write: Some(WriteInfo {
                    outcome: WriteOutcome::Confirmed,
                    committed: None,
                    receipt: projected.receipt,
                }),
            },
        };
        let wire = serde_json::to_value(envelope).unwrap();
        assert_eq!(wire["meta"]["http_status"], 200);
        assert_eq!(wire["meta"]["write"]["outcome"], "confirmed");
        if fixture.tool == "cancel_run" {
            assert_eq!(wire["data"]["status"], native["status"]);
            assert_eq!(wire["data"]["outcome"], native["outcome"]);
        } else {
            assert_eq!(wire["data"]["attempt_count"], 0);
            assert_eq!(wire["data"]["needs_attention"], false);
        }
    }
}

#[test]
fn missing_or_wrong_receipts_never_confirm_recovery() {
    for (tool, args, good) in [
        (
            "resume_issue",
            json!({"issue_id":"target"}),
            json!({"id":"target"}),
        ),
        (
            "cancel_run",
            json!({"run_id":"target"}),
            json!({"id":"target","status":"running","outcome":null}),
        ),
    ] {
        let plan = prepare(tool, args).unwrap();
        let mut invalid = vec![
            Value::Null,
            json!([]),
            json!({}),
            json!({"id":null}),
            json!({"id":"other"}),
        ];
        let mut without_id = good.clone();
        without_id.as_object_mut().unwrap().remove("id");
        invalid.push(without_id);
        if tool == "cancel_run" {
            for (key, value) in [
                ("status", Value::Null),
                ("status", json!("")),
                ("outcome", json!(42)),
            ] {
                let mut bad = good.clone();
                bad[key] = value;
                invalid.push(bad);
            }
            for key in ["status", "outcome"] {
                let mut bad = good.clone();
                bad.as_object_mut().unwrap().remove(key);
                invalid.push(bad);
            }
        }
        for native in invalid {
            let error = direct_calls::project(&plan, response(200, native))
                .err()
                .unwrap();
            assert_eq!(error.code, "invalid_receipt");
            assert!(error.receipt.is_none());
        }
        assert!(direct_calls::project(&plan, response(201, good.clone())).is_err());
        assert!(direct_calls::project(&plan, response(200, good)).is_ok());
    }
}
fn response(status: u16, data: Value) -> ResponseFixture {
    serde_json::from_value(json!({"status":status,"body":{"kind":"json","value":data}})).unwrap()
}

#[test]
fn native_permission_and_already_ended_errors_remain_errors_without_success_receipts() {
    let errors = fixtures().errors;
    assert_eq!(errors.len(), 5);
    for fixture in errors {
        let field = if fixture.tool == "resume_issue" {
            "issue_id"
        } else {
            "run_id"
        };
        let plan = prepare(&fixture.tool, json!({field:"target"})).unwrap();
        let native = serde_json::to_value(&fixture.error).unwrap();
        // A native 4xx cannot be passed off as a successful recovery response.
        assert!(direct_calls::project(&plan, response(fixture.status, native.clone())).is_err());
        assert!(plan.noncommitting_errors.is_empty());
        let envelope = ResultEnvelope {
            data: Value::Null,
            error: Some(fixture.error),
            meta: ResultMeta {
                http_status: Some(fixture.status),
                observed_at_ms: 42,
                omissions: vec![],
                links: vec![],
                retry_after: None,
                write: Some(WriteInfo {
                    outcome: fixture.expected_outcome,
                    committed: None,
                    receipt: None,
                }),
            },
        };
        let wire = serde_json::to_value(envelope).unwrap();
        assert_eq!(wire["error"], native);
        assert_eq!(wire["meta"]["http_status"], fixture.status);
        // The native rejection lacks explicit commitment evidence or a pinned
        // noncommitting regression proof; classification stays conservative.
        assert_eq!(wire["meta"]["write"]["outcome"], "unknown");
        assert!(wire["meta"]["write"].get("receipt").is_none());
    }
}
