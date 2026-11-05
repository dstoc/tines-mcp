mod support;

use serde::Deserialize;
use serde_json::{Value, json};
use support::direct_calls::{self, BodyFixture, PlanFixture, ResponseFixture};
use tines_mcp::tools::{self, context, contract::*};

fn prepare(name: &str, args: Value) -> Result<PreparedCall, LocalError> {
    context::prepare(
        name,
        args.as_object().unwrap().clone(),
        &LocalLimits::default(),
    )
}
fn body(call: &PreparedCall) -> Value {
    match &call.body {
        RequestBody::Json(v) => v.clone(),
        _ => panic!("expected JSON"),
    }
}
fn response(status: u16, data: Value) -> ResponseFixture {
    serde_json::from_value(json!({"status":status,"body":{"kind":"json","value":data}})).unwrap()
}

#[test]
fn native_plans_and_receipts_match_context_fixtures() {
    #[derive(Deserialize)]
    struct Fixture {
        tool: String,
        arguments: Value,
        plan: PlanFixture,
        response: ResponseFixture,
        receipt: Value,
    }
    #[derive(Deserialize)]
    struct Fixtures {
        source: String,
        successes: Vec<Fixture>,
    }
    let fixtures: Fixtures = serde_json::from_str(include_str!("fixtures/context.json")).unwrap();
    assert!(fixtures.source.contains("af6cb25"));
    for fixture in fixtures.successes {
        let call = tools::prepare(
            &fixture.tool,
            fixture.arguments.as_object().unwrap().clone(),
            &LocalLimits::default(),
        )
        .unwrap();
        assert_eq!(direct_calls::inspect(&call), fixture.plan);
        let BodyFixture::Json { value: native } = &fixture.response.body else {
            panic!()
        };
        let native = native.clone();
        let projected = direct_calls::project(&call, fixture.response).unwrap();
        assert_eq!(projected.data, native);
        assert_eq!(
            serde_json::to_value(projected.receipt).unwrap(),
            fixture.receipt
        );
        assert!(projected.links.is_empty());
        assert!(projected.omissions.is_empty());
        // Projection context holds only expected identity/kind, never payloads.
        assert!(
            call.projection
                .context
                .as_object()
                .unwrap()
                .keys()
                .all(|key| ["id", "kind"].contains(&key.as_str()))
        );
    }
}

#[test]
fn native_missing_null_empty_and_value_semantics_survive_preparation() {
    for field in [
        "project_id",
        "workflow_state_id",
        "issue_id",
        "label_id",
        "repo_branch",
        "repo_dir",
        "hint",
    ] {
        for value in [Value::Null, json!(""), json!("  literal 🦀  ")] {
            let mut args = json!({"context_id":"ctx_raw"});
            args[field] = value.clone();
            assert_eq!(
                body(&prepare("edit_context", args).unwrap()),
                json!({field:value})
            );
        }
    }
    assert_eq!(
        body(&prepare("edit_context", json!({"context_id":"ctx_raw"})).unwrap()),
        json!({})
    );
    for field in ["description", "body", "repo_url", "value", "name"] {
        let mut args = json!({"context_id":"ctx_raw"});
        args[field] = json!("");
        assert_eq!(
            body(&prepare("edit_context", args).unwrap()),
            json!({field:""})
        );
    }
    for fields in [
        json!({"files":[]}),
        json!({"secret":false}),
        json!({"position":-1}),
        json!({"expected_version":0}),
    ] {
        let mut args = fields.clone();
        args["context_id"] = json!("ctx_raw");
        assert_eq!(body(&prepare("edit_context", args).unwrap()), fields);
    }
    let args = json!({"kind":"prompt","name":"raw","body":"","project_id":null,"workflow_state_id":null,"issue_id":null,"label_id":null});
    assert_eq!(
        body(&prepare("create_context", args.clone()).unwrap()),
        args
    );
    let global = json!({"kind":"prompt","name":"raw","body":""});
    assert_eq!(
        body(&prepare("create_context", global.clone()).unwrap()),
        global
    );
}

#[test]
fn append_uses_native_text_without_a_read_or_rewrite() {
    let text = "  literal $VALUE `command` 🦀\n\n ";
    for version in [None, Some(7)] {
        let mut args = json!({"context_id":"ctx/a?b#c","text":text});
        if let Some(v) = version {
            args["expected_version"] = json!(v);
        }
        let call = prepare("append_context", args.clone()).unwrap();
        assert_eq!(call.endpoint.method, Method::Post);
        assert_eq!(
            call.endpoint.segments(),
            &["context", "ctx/a?b#c", "append"]
        );
        assert!(call.query.is_empty());
        args.as_object_mut().unwrap().remove("context_id");
        assert_eq!(body(&call), args);
    }
}

#[test]
fn schemas_and_runtime_reject_unadvertised_fields_and_nonnull_nulls() {
    let definitions = context::definitions(&LocalLimits::new(12345).unwrap());
    assert_eq!(definitions.len(), 3);
    for tool in definitions {
        let schema = Value::Object((*tool.input_schema).clone());
        assert_eq!(schema["additionalProperties"], false);
        assert!(tool.description.as_ref().unwrap().contains("12345"));
        assert!(tool.output_schema.is_some());
        let mut args = match tool.name.as_ref() {
            "create_context" => json!({"kind":"prompt","name":"x","body":""}),
            "edit_context" => json!({"context_id":"ctx_raw"}),
            _ => json!({"context_id":"ctx_raw","text":"x"}),
        };
        let properties = schema["properties"].as_object().unwrap();
        if tool.name != "create_context" {
            assert!(properties.contains_key("context_id"));
            assert!(
                schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("context_id"))
            );
        }
        for field in properties.keys() {
            if [
                "project_id",
                "workflow_state_id",
                "issue_id",
                "label_id",
                "repo_branch",
                "repo_dir",
                "hint",
            ]
            .contains(&field.as_str())
            {
                continue;
            }
            let mut invalid = args.clone();
            invalid[field] = Value::Null;
            assert!(
                prepare(tool.name.as_ref(), invalid).is_err(),
                "{} {field}",
                tool.name
            );
        }
        args["secret_canary"] = json!("secret_canary");
        let error = prepare(tool.name.as_ref(), args).err().unwrap();
        assert!(!format!("{error:?}").contains("secret_canary"));
        if let Some(body) = properties.get("body") {
            assert_eq!(body["type"], "string");
        }
        if let Some(version) = properties.get("expected_version") {
            assert_eq!(version["type"], "integer");
        }
        if let Some(files) = schema.pointer("/$defs/ContextFile") {
            assert_eq!(files["additionalProperties"], false);
        }
    }
    for invalid in [
        json!({"kind":"artifact","name":"x"}),
        json!({"kind":"prompt","name":"x","body":"x","repo_branch":null}),
        json!({"kind":"skill","name":"x","files":[{"path":"SKILL.md","content":"x","extra":true}]}),
    ] {
        assert!(prepare("create_context", invalid).is_err());
    }
    for invalid in [
        json!({"context_id":"ctx_raw","kind":"prompt"}),
        json!({"context_id":"ctx_raw","expected_version":1.5}),
        json!({"context_id":"ctx_raw","position":1.5}),
    ] {
        assert!(prepare("edit_context", invalid).is_err());
    }
    assert!(prepare("append_context", json!({"context_id":"ctx_raw","body":"x"})).is_err());
}

#[test]
fn receipt_requires_native_identity_kind_version_and_matching_target() {
    for (name, args, status) in [
        (
            "create_context",
            json!({"kind":"prompt","name":"x","body":""}),
            201,
        ),
        ("edit_context", json!({"context_id":"ctx_raw"}), 200),
        (
            "append_context",
            json!({"context_id":"ctx_raw","text":"x"}),
            200,
        ),
    ] {
        let call = prepare(name, args).unwrap();
        for invalid in [
            json!({}),
            json!({"id":"","kind":"prompt","version":1}),
            json!({"id":"ctx_raw","kind":"unknown","version":1}),
            json!({"id":"ctx_raw","kind":"prompt","version":0}),
            json!({"id":"ctx_raw","kind":"prompt","version":1.5}),
        ] {
            let error = direct_calls::project(&call, response(status, invalid))
                .err()
                .unwrap();
            assert_eq!(error.code, "invalid_receipt");
            assert!(error.receipt.is_none());
        }
        if name != "create_context" {
            assert!(
                direct_calls::project(
                    &call,
                    response(
                        status,
                        json!({"id":"different","kind":"prompt","version":1})
                    )
                )
                .is_err()
            );
        }
        if name != "edit_context" {
            assert!(
                direct_calls::project(
                    &call,
                    response(status, json!({"id":"ctx_raw","kind":"env","version":1}))
                )
                .is_err()
            );
        }
        assert!(
            direct_calls::project(
                &call,
                response(202, json!({"id":"ctx_raw","kind":"prompt","version":1}))
            )
            .is_err()
        );
    }
}

#[test]
fn native_error_details_and_endpoint_specific_conflict_evidence_survive() {
    #[derive(Deserialize)]
    struct Fixture {
        tool: String,
        status: u16,
        error: ResultError,
    }
    #[derive(Deserialize)]
    struct Fixtures {
        errors: Vec<Fixture>,
    }
    let source: Value = serde_json::from_str(include_str!("fixtures/context.json")).unwrap();
    let fixtures: Fixtures = serde_json::from_value(source.clone()).unwrap();
    for (index, fixture) in fixtures.errors.into_iter().enumerate() {
        // These pure features do not own HTTP failure projection. Verify their
        // fixtures use the shared extensible native error contract unchanged.
        assert_eq!(
            serde_json::to_value(&fixture.error).unwrap(),
            source["errors"][index]["error"]
        );
        let args = if fixture.tool == "append_context" {
            json!({"context_id":"ctx_raw","text":"x","expected_version":1})
        } else {
            json!({"context_id":"ctx_raw","expected_version":1})
        };
        let call = prepare(&fixture.tool, args).unwrap();
        let rejected = call
            .noncommitting_errors
            .iter()
            .any(|rule| rule.matches(fixture.status, &fixture.error));
        assert_eq!(
            rejected,
            fixture.tool == "edit_context" && fixture.status == 409
        );
        if fixture.status == 409 {
            assert_eq!(
                fixture.error.details.as_ref().unwrap()["current"]["version"],
                3
            );
        }
    }
}
