mod support;

use serde_json::{Value, json};
use support::direct_calls::{self, BodyFixture, ResponseFixture};
use tines_mcp::tools::{
    self,
    contract::{LocalLimits, PreparedCall},
    issues,
};

fn prepare(name: &str, args: Value) -> PreparedCall {
    tools::prepare(
        name,
        args.as_object().unwrap().clone(),
        &LocalLimits::default(),
    )
    .unwrap_or_else(|e| panic!("{e:?}"))
}

fn reject(name: &str, args: Value) {
    let error = issues::prepare(
        name,
        args.as_object().unwrap().clone(),
        &LocalLimits::default(),
    )
    .err()
    .expect("local rejection");
    assert_eq!(error.code, "invalid_input");
    assert!(!error.message.contains("secret_canary"));
}

fn project(
    call: &PreparedCall,
    status: u16,
    data: Value,
) -> Result<tines_mcp::tools::contract::Projected, tines_mcp::tools::contract::ProjectionError> {
    let fixture: ResponseFixture =
        serde_json::from_value(json!({"status":status,"body":{"kind":"json","value":data}}))
            .unwrap();
    direct_calls::project(call, fixture)
}

#[test]
fn native_request_fixtures_are_exact_single_calls() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/issue-content.json")).unwrap();
    for fixture in fixtures["requests"].as_array().unwrap() {
        let call = prepare(
            fixture["tool"].as_str().unwrap(),
            fixture["arguments"].clone(),
        );
        let expected = serde_json::from_value(fixture["plan"].clone()).unwrap();
        assert_eq!(direct_calls::inspect(&call), expected);
        assert!(call.noncommitting_errors.is_empty());
    }
}

#[test]
fn creation_omits_unsupplied_fields_and_retains_explicit_empty_arrays() {
    for extra in [
        json!({}),
        json!({"label_ids":[],"blocked_by":[],"blocks":[]}),
    ] {
        let mut args = json!({"project_id":"prj_owner","title":"Title"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let call = prepare("create_issue", args);
        let mut expected = json!({"title":"Title"});
        if extra.get("label_ids").is_some() {
            expected.as_object_mut().unwrap().extend(
                json!({"labels":[],"blocked_by":[],"blocks":[]})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
        }
        assert_eq!(
            direct_calls::inspect(&call).body,
            BodyFixture::Json { value: expected }
        );
    }
}

#[test]
fn edit_requires_prose_and_keeps_patch_bodies_sparse() {
    reject("edit_issue", json!({"issue_id":"iss_requested"}));
    for fields in [
        json!({"title":"  Title  "}),
        json!({"description":""}),
        json!({"title":"Title","description":"\n literal $VAR `text` \n"}),
    ] {
        let mut args = fields.clone();
        args["issue_id"] = json!("iss_requested");
        assert_eq!(
            direct_calls::inspect(&prepare("edit_issue", args)).body,
            BodyFixture::Json { value: fields }
        );
    }
}

#[test]
fn unknown_fields_scheduling_and_privileged_native_fields_are_rejected() {
    let forbidden = [
        "schedule",
        "duplicate_of",
        "allow_my_agents",
        "disclosure_version",
        "allow_my_agents_on_future_instances",
        "pinned_runner_id",
        "pinned_tier",
        "expected_decision_revision",
        "expected_workflow_revision",
        "secret_canary",
        "labels",
        "method",
        "path",
        "headers",
    ];
    for (tool, base) in [
        (
            "create_issue",
            json!({"project_id":"prj_owner","title":"Title"}),
        ),
        (
            "edit_issue",
            json!({"issue_id":"iss_requested","title":"Title"}),
        ),
        (
            "append_issue_comment",
            json!({"issue_id":"iss_requested","body":"Body"}),
        ),
    ] {
        for field in forbidden {
            for value in [Value::Null, json!("secret_canary")] {
                let mut args = base.clone();
                args[field] = value;
                reject(tool, args);
            }
        }
    }
    for field in [
        "state",
        "workflow_id",
        "label_ids",
        "blocked_by",
        "blocks",
        "expected_sharing_revision",
    ] {
        let mut args = json!({"issue_id":"iss_requested","description":"Prose"});
        args[field] = json!("secret_canary");
        reject("edit_issue", args);
    }
    reject("no_such_tool", json!({}));
}

#[test]
fn optional_fields_reject_null_and_wrong_types() {
    for field in [
        "description",
        "workflow_id",
        "state",
        "label_ids",
        "blocked_by",
        "blocks",
        "expected_sharing_revision",
    ] {
        let mut args = json!({"project_id":"prj_owner","title":"Title"});
        args[field] = Value::Null;
        reject("create_issue", args);
    }
    for field in ["title", "description"] {
        let mut args = json!({"issue_id":"iss_requested"});
        args[field] = Value::Null;
        reject("edit_issue", args);
    }
    for args in [
        json!({"project_id":"prj_owner","title":"Title","label_ids":["P0"]}),
        json!({"project_id":"prj_owner","title":"Title","label_ids":["lbl_0123456789abcde_"]}),
        json!({"project_id":"prj_owner","title":"Title","blocked_by":[null]}),
        json!({"project_id":"prj_owner","title":"Title","blocks":[""]}),
        json!({"project_id":"prj_owner","title":"Title","expected_sharing_revision":-1}),
        json!({"project_id":"prj_owner","title":"Title","expected_sharing_revision":1.5}),
        json!({"project_id":"prj_owner","title":42}),
    ] {
        reject("create_issue", args);
    }
    for field in ["project_id", "title"] {
        let mut args = json!({"project_id":"prj_owner","title":"Title"});
        args.as_object_mut().unwrap().remove(field);
        reject("create_issue", args);
    }
    for args in [
        json!({"issue_id":"iss_requested"}),
        json!({"body":"Body"}),
        json!({"issue_id":"iss_requested","body":null}),
    ] {
        reject("append_issue_comment", args);
    }
}

#[test]
fn lengths_count_utf16_before_native_trimming_and_preserve_prose() {
    for title in [
        "x".repeat(500),
        "🦀".repeat(250),
        format!(" {} ", "x".repeat(498)),
    ] {
        let args = json!({"project_id":"prj_owner","title":title});
        let call = prepare("create_issue", args);
        assert_eq!(
            direct_calls::inspect(&call).body,
            BodyFixture::Json {
                value: json!({"title":title})
            }
        );
        prepare(
            "edit_issue",
            json!({"issue_id":"iss_requested","title":title}),
        );
    }
    for title in [
        "x".repeat(501),
        format!("{}x", "🦀".repeat(250)),
        format!(" {} ", "x".repeat(499)),
    ] {
        reject(
            "create_issue",
            json!({"project_id":"prj_owner","title":title}),
        );
        reject(
            "edit_issue",
            json!({"issue_id":"iss_requested","title":title}),
        );
    }
    for text in ["x".repeat(100_000), "🦀".repeat(50_000)] {
        for (tool, mut args, field) in [
            (
                "create_issue",
                json!({"project_id":"prj_owner","title":"Title"}),
                "description",
            ),
            (
                "edit_issue",
                json!({"issue_id":"iss_requested"}),
                "description",
            ),
            (
                "append_issue_comment",
                json!({"issue_id":"iss_requested"}),
                "body",
            ),
        ] {
            args[field] = json!(text);
            let inspected = direct_calls::inspect(&prepare(tool, args.clone()));
            let BodyFixture::Json { value } = inspected.body else {
                panic!()
            };
            assert_eq!(value[field], text);
            args[field] = json!(format!("{text}x"));
            reject(tool, args);
        }
    }
    prepare(
        "create_issue",
        json!({"project_id":"prj_owner","title":"Title","state":"🦀".repeat(50)}),
    );
    reject(
        "create_issue",
        json!({"project_id":"prj_owner","title":"Title","state":"🦀".repeat(51)}),
    );
    for field in ["blocked_by", "blocks"] {
        let mut args = json!({"project_id":"prj_owner","title":"Title"});
        args[field] = json!(["x".repeat(100)]);
        prepare("create_issue", args.clone());
        args[field] = json!(["🦀".repeat(50)]);
        prepare("create_issue", args.clone());
        args[field] = json!(["🦀".repeat(51)]);
        reject("create_issue", args);
    }
}

#[test]
fn ecmascript_blank_rules_match_native_and_do_not_trim() {
    for blank in ["", " \n\t", "\u{feff}", "\u{a0}\u{2028}\u{3000}"] {
        reject(
            "create_issue",
            json!({"project_id":"prj_owner","title":blank}),
        );
        reject(
            "edit_issue",
            json!({"issue_id":"iss_requested","title":blank}),
        );
        reject(
            "append_issue_comment",
            json!({"issue_id":"iss_requested","body":blank}),
        );
        prepare(
            "edit_issue",
            json!({"issue_id":"iss_requested","description":blank}),
        );
    }
    for text in ["\u{85}", "\u{feff}  literal $VAR `text` 🦀 \n"] {
        let call = prepare(
            "append_issue_comment",
            json!({"issue_id":"iss_requested","body":text}),
        );
        assert_eq!(
            direct_calls::inspect(&call).body,
            BodyFixture::Json {
                value: json!({"body":text})
            }
        );
    }
    for bad in ["", ".", "..", "\u{feff}"] {
        reject("create_issue", json!({"project_id":bad,"title":"Title"}));
        reject("edit_issue", json!({"issue_id":bad,"title":"Title"}));
        reject(
            "append_issue_comment",
            json!({"issue_id":bad,"body":"Body"}),
        );
    }
}

#[test]
fn owner_member_and_minimal_response_fixtures_preserve_native_data() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/issue-content.json")).unwrap();
    for fixture in fixtures["responses"].as_array().unwrap() {
        let call = prepare(
            fixture["tool"].as_str().unwrap(),
            fixture["arguments"].clone(),
        );
        let result = project(
            &call,
            fixture["status"].as_u64().unwrap() as u16,
            fixture["native"].clone(),
        )
        .unwrap();
        assert_eq!(result.data, fixture["native"]);
        assert_eq!(
            serde_json::to_value(result.receipt).unwrap(),
            fixture["receipt"]
        );
        assert!(result.links.is_empty() && result.omissions.is_empty());
    }
}

#[test]
fn receipts_require_response_identity_and_exact_success_status() {
    for (tool, args, status) in [
        (
            "create_issue",
            json!({"project_id":"prj_owner","title":"Title"}),
            201,
        ),
        (
            "edit_issue",
            json!({"issue_id":"iss_requested","title":"Title"}),
            200,
        ),
    ] {
        let call = prepare(tool, args);
        for data in [
            Value::Null,
            json!({}),
            json!({"id":null}),
            json!({"id":""}),
            json!({"id":".."}),
        ] {
            assert!(
                project(&call, status, data)
                    .err()
                    .unwrap()
                    .receipt
                    .is_none()
            );
        }
        assert!(project(&call, 202, json!({"id":"iss_requested"})).is_err());
        let wrong = if tool == "create_issue" {
            json!({"id":"iss_new","project":{"id":"prj_wrong"}})
        } else {
            json!({"id":"iss_wrong"})
        };
        assert_eq!(
            project(&call, status, wrong).err().unwrap().code,
            "invalid_receipt"
        );
    }
    let call = prepare(
        "append_issue_comment",
        json!({"issue_id":"iss_requested","body":"Body"}),
    );
    for data in [
        json!({"id":"cmt_new"}),
        json!({"issue_id":"iss_requested"}),
        json!({"id":"","issue_id":"iss_requested"}),
        json!({"id":"cmt_new","issue_id":"iss_other"}),
        json!({"id":"cmt_new","issue_id":null}),
    ] {
        assert_eq!(
            project(&call, 201, data).err().unwrap().code,
            "invalid_receipt"
        );
    }
    assert!(
        project(
            &call,
            200,
            json!({"id":"cmt_new","issue_id":"iss_requested"})
        )
        .is_err()
    );
}

#[test]
fn schemas_and_annotations_match_the_pure_surface() {
    let definitions = issues::definitions(&LocalLimits::new(123_456).unwrap());
    assert_eq!(definitions.len(), 3);
    for tool in definitions {
        let schema = Value::Object((*tool.input_schema).clone());
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"].get("schedule").is_none());
        assert!(tool.description.as_ref().unwrap().contains("123456 bytes"));
        let annotations = serde_json::to_value(tool.annotations.unwrap()).unwrap();
        assert_eq!(annotations["readOnlyHint"], false);
        assert_eq!(annotations["idempotentHint"], false);
        assert_eq!(annotations["destructiveHint"], tool.name == "edit_issue");
        let out = tool.output_schema.unwrap();
        assert_eq!(out["required"], json!(["data", "meta"]));
        match tool.name.as_ref() {
            "create_issue" => {
                assert_eq!(schema["required"], json!(["project_id", "title"]));
                assert!(schema["properties"].get("label_ids").is_some());
                assert!(schema["properties"].get("labels").is_none());
                for field in ["description", "workflow_id", "state"] {
                    assert_eq!(schema["properties"][field]["type"], "string");
                }
                for field in ["label_ids", "blocked_by", "blocks"] {
                    assert_eq!(schema["properties"][field]["type"], "array");
                }
                assert_eq!(
                    schema["properties"]["expected_sharing_revision"]["type"],
                    "integer"
                );
                assert!(
                    tool.description
                        .unwrap()
                        .contains("initial state may be active")
                );
            }
            "edit_issue" => {
                assert_eq!(schema["required"], json!(["issue_id"]));
                assert_eq!(
                    schema["anyOf"],
                    json!([{"required":["title"]}, {"required":["description"]}])
                );
                assert_eq!(schema["properties"].as_object().unwrap().len(), 3);
                for field in ["title", "description"] {
                    assert_eq!(schema["properties"][field]["type"], "string");
                }
                assert!(tool.description.unwrap().contains("no revision witness"));
            }
            "append_issue_comment" => {
                assert_eq!(schema["required"], json!(["issue_id", "body"]));
            }
            _ => panic!("unexpected tool"),
        }
    }
}

#[test]
fn native_conflict_and_permission_fixtures_retain_error_evidence() {
    // The executor owns HTTP error decoding/classification. These pure fixtures
    // pin native evidence without treating every HTTP error as noncommitting.
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/issue-content.json")).unwrap();
    for fixture in fixtures["errors"].as_array().unwrap() {
        let native = fixture["error"].clone();
        let error: tines_mcp::tools::contract::ResultError =
            serde_json::from_value(native.clone()).unwrap();
        assert_eq!(serde_json::to_value(error).unwrap(), native);
        assert!(!fixture["native_source"].as_str().unwrap().is_empty());
        let tool = fixture["tool"].as_str().unwrap();
        let args = match tool {
            "create_issue" => json!({"project_id":"prj_owner","title":"Title"}),
            "edit_issue" => json!({"issue_id":"iss_requested","title":"Title"}),
            "append_issue_comment" => json!({"issue_id":"iss_requested","body":"Body"}),
            _ => panic!(),
        };
        let call = prepare(tool, args);
        assert!(call.noncommitting_errors.is_empty());
        assert!(
            project(
                &call,
                fixture["status"].as_u64().unwrap() as u16,
                json!({"error":native})
            )
            .is_err()
        );
        if native["code"] == "conflict" {
            assert_eq!(native["details"]["committed"], false);
            assert_eq!(native["details"]["current_sharing_revision"], 4);
        }
    }
}
