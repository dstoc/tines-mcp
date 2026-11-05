mod support;

use serde::Deserialize;
use serde_json::{Value, json};
use support::direct_calls::{PlanFixture, ResponseFixture, inspect, project};
use tines_mcp::tools::{self, contract::*, operations_read};

#[derive(Deserialize)]
struct Fixtures {
    reference_revision: String,
    native_sources: Vec<String>,
    successes: Vec<Success>,
    errors: Vec<Failure>,
}

#[derive(Deserialize)]
struct Success {
    name: String,
    tool: String,
    arguments: Value,
    plan: PlanFixture,
    response: ResponseFixture,
}

#[derive(Deserialize)]
struct Failure {
    tool: String,
    status: u16,
    error: ResultError,
}

fn fixtures() -> Fixtures {
    serde_json::from_str(include_str!("fixtures/operations-read.json")).unwrap()
}

fn prepare(name: &str, args: Value) -> Result<PreparedCall, LocalError> {
    // Exercise public registration as well as the feature module.
    tools::prepare(
        name,
        args.as_object().unwrap().clone(),
        &LocalLimits::default(),
    )
}

fn args(name: &str) -> Value {
    if name == "get_runner" {
        json!({"runner_id":"rnr_local"})
    } else {
        json!({})
    }
}

#[test]
fn operational_plans_and_native_json_match_fixtures() {
    let fixtures = fixtures();
    assert_eq!(
        fixtures.reference_revision,
        "af6cb25febf1b45d7eff32251f78fcf6355aed4a"
    );
    assert!(!fixtures.native_sources.is_empty());
    assert_eq!(fixtures.successes.len(), 8);
    for fixture in fixtures.successes {
        let plan = prepare(&fixture.tool, fixture.arguments).unwrap();
        assert_eq!(inspect(&plan), fixture.plan, "{}", fixture.name);
        assert_eq!(plan.response.statuses, [200]);
        assert_eq!(plan.response.kind, ResponseKind::Json);
        assert!(plan.noncommitting_errors.is_empty());
        assert!(plan.projection.context.is_null());
        let expected = match &fixture.response.body {
            support::direct_calls::BodyFixture::Json { value } => value.clone(),
            _ => panic!("expected native JSON fixture"),
        };
        let projected = project(&plan, fixture.response).unwrap();
        assert_eq!(projected.data, expected, "{}", fixture.name);
        assert!(projected.receipt.is_none());
        assert!(projected.links.is_empty());
        assert!(projected.omissions.is_empty());
    }
}

#[test]
fn native_filters_and_runner_ids_are_raw_values_for_the_executor_to_encode() {
    for project in [
        "prj_1",
        "project A",
        "",
        "  literal  ",
        "A/B?project=other&x=1#🦀",
    ] {
        let plan = prepare("get_supervisor_queue", json!({"project":project})).unwrap();
        assert_eq!(plan.query, [("project".into(), project.into())]);
        assert_eq!(plan.endpoint.segments(), ["supervisor", "queue"]);
    }
    let id = "rnr/a?b=c#🦀";
    let plan = prepare("get_runner", json!({"runner_id":id})).unwrap();
    assert_eq!(plan.endpoint.segments(), ["runners", id]);
    assert!(plan.query.is_empty());
}

#[test]
fn schemas_and_runtime_inputs_are_strict_and_nonnull() {
    let definitions = operations_read::definitions(&LocalLimits::default());
    assert_eq!(definitions.len(), 4);
    for tool in definitions {
        assert_eq!(tool.input_schema["additionalProperties"], false);
        let wire = serde_json::to_value(&tool).unwrap();
        assert_eq!(wire["annotations"]["readOnlyHint"], true);
        assert_eq!(wire["annotations"]["destructiveHint"], false);
        assert_eq!(wire["annotations"]["idempotentHint"], true);
        assert_eq!(wire["outputSchema"]["required"], json!(["data", "meta"]));
        let schema = &tool.input_schema;
        let mut valid = args(&tool.name);
        assert!(prepare(&tool.name, valid.clone()).is_ok());
        for field in [
            "method",
            "path",
            "headers",
            "body",
            "cursor",
            "limit",
            "active",
            "runner_id",
            "project",
        ] {
            if (tool.name == "get_runner" && field == "runner_id")
                || (tool.name == "get_supervisor_queue" && field == "project")
            {
                continue;
            }
            valid[field] = json!("unadvertised_canary");
            let error = prepare(&tool.name, valid.clone()).err().unwrap();
            assert_eq!(error.code, "invalid_input");
            assert!(!format!("{error:?}").contains("unadvertised_canary"));
            valid.as_object_mut().unwrap().remove(field);
        }
        if tool.name == "get_supervisor_queue" {
            assert!(schema.get("required").is_none());
            assert_eq!(schema["properties"]["project"]["type"], "string");
            for value in [Value::Null, json!(42), json!([]), json!({}), json!(true)] {
                assert!(prepare(&tool.name, json!({"project":value})).is_err());
            }
        } else if tool.name == "get_runner" {
            assert_eq!(schema["required"], json!(["runner_id"]));
            assert_eq!(schema["properties"]["runner_id"]["type"], "string");
            assert_eq!(schema["properties"]["runner_id"]["minLength"], 1);
            assert!(prepare(&tool.name, json!({})).is_err());
            for value in [
                Value::Null,
                json!(42),
                json!(true),
                json!({}),
                json!([]),
                json!(""),
                json!("."),
                json!(".."),
            ] {
                assert!(prepare(&tool.name, json!({"runner_id":value})).is_err());
            }
        } else {
            assert!(
                schema
                    .get("properties")
                    .is_none_or(|value| value == &json!({}))
            );
        }
    }
    assert_eq!(
        operations_read::prepare("http_request", Default::default(), &LocalLimits::default())
            .err()
            .unwrap()
            .field,
        Some("tool")
    );
}

#[test]
fn native_failures_cannot_be_projected_as_offline_or_empty_queue() {
    // Native errors use the executor failure path. These pure checks ensure a
    // domain projector cannot accept them as a successful operational snapshot.
    let fixtures = fixtures();
    assert_eq!(fixtures.errors.len(), 5);
    for fixture in fixtures.errors {
        let plan = prepare(&fixture.tool, args(&fixture.tool)).unwrap();
        let native_error = serde_json::to_value(&fixture.error).unwrap();
        let response = ResponseFixture {
            status: fixture.status,
            observed_at_ms: 42,
            content_type: None,
            body: support::direct_calls::BodyFixture::Json {
                value: json!({"error":native_error}),
            },
        };
        assert_eq!(
            project(&plan, response).err().unwrap().code,
            "unexpected_status"
        );
        // Native details and statuses fit the shared envelope without domain
        // normalization. Actual HTTP error decoding is an integration check.
        let envelope = ResultEnvelope {
            data: Value::Null,
            error: Some(fixture.error),
            meta: ResultMeta {
                http_status: Some(fixture.status),
                observed_at_ms: 42,
                omissions: vec![],
                links: vec![],
                retry_after: None,
                write: None,
            },
        };
        let wire = serde_json::to_value(envelope).unwrap();
        assert_eq!(wire["error"], native_error);
        assert_eq!(wire["meta"]["http_status"], fixture.status);
        assert!(wire["data"].is_null());
        assert!(wire["meta"].get("write").is_none());
    }
    for tool in operations_read::definitions(&LocalLimits::default()) {
        let plan = prepare(&tool.name, args(&tool.name)).unwrap();
        for status in [201, 204, 401, 403, 404, 429, 500, 503] {
            assert!(
                plan.project(UpstreamResponse {
                    status,
                    observed_at_ms: 42,
                    headers: ResponseHeaders::default(),
                    body: ResponseBody::Json(json!({}))
                })
                .is_err()
            );
        }
        for body in [ResponseBody::Empty, ResponseBody::Bytes(vec![])] {
            assert_eq!(
                plan.project(UpstreamResponse {
                    status: 200,
                    observed_at_ms: 42,
                    headers: ResponseHeaders::default(),
                    body
                })
                .err()
                .unwrap()
                .code,
                "invalid_response"
            );
        }
    }
}
