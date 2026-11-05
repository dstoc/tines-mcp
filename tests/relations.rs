mod support;

use serde::Deserialize;
use serde_json::{Value, json};
use support::direct_calls::{self, BodyFixture, PlanFixture, ResponseFixture};
use tines_mcp::tools::{self, contract::*, relations};

fn prepare(name: &str, args: Value) -> Result<PreparedCall, LocalError> {
    tools::prepare(
        name,
        args.as_object().unwrap().clone(),
        &LocalLimits::default(),
    )
}

#[derive(Deserialize)]
struct Fixtures {
    reference_revision: String,
    cases: Vec<Case>,
    errors: Vec<NativeError>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    tool: String,
    arguments: Value,
    plan: PlanFixture,
    response: ResponseFixture,
    receipt: Value,
}
#[derive(Deserialize)]
struct NativeError {
    tool: String,
    arguments: Value,
    status: u16,
    source: String,
    error: Value,
}
fn fixtures() -> Fixtures {
    serde_json::from_str(include_str!("fixtures/relations.json")).unwrap()
}

#[test]
fn native_requests_success_data_and_receipts_match_fixtures() {
    let fixtures = fixtures();
    assert_eq!(
        fixtures.reference_revision,
        "af6cb25febf1b45d7eff32251f78fcf6355aed4a"
    );
    assert_eq!(fixtures.cases.len(), 6);
    for case in fixtures.cases {
        let plan = prepare(&case.tool, case.arguments).unwrap();
        assert_eq!(direct_calls::inspect(&plan), case.plan, "{}", case.name);
        assert_eq!(plan.response.statuses, &[case.response.status]);
        assert!(plan.noncommitting_errors.is_empty());
        let expected = match &case.response.body {
            BodyFixture::Json { value } => value.clone(),
            BodyFixture::Empty => Value::Null,
            _ => panic!("unexpected fixture"),
        };
        let result = direct_calls::project(&plan, case.response).unwrap();
        assert_eq!(result.data, expected, "{}", case.name);
        assert_eq!(
            serde_json::to_value(result.receipt.unwrap()).unwrap(),
            case.receipt
        );
        assert!(result.links.is_empty() && result.omissions.is_empty());
        assert!(result.data.get("run_started").is_none());
    }
}

#[test]
fn definitions_have_closed_required_schemas_and_native_result_envelopes() {
    let definitions = relations::definitions(&LocalLimits::default());
    assert_eq!(definitions.len(), 4);
    for tool in definitions {
        assert!(
            tools::definitions(&LocalLimits::default())
                .iter()
                .any(|t| t.name == tool.name)
        );
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        let properties = schema["properties"].as_object().unwrap();
        let required = schema["required"].as_array().unwrap();
        assert_eq!(properties.len(), required.len());
        for (key, property) in properties {
            assert!(required.contains(&json!(key)));
            assert!(property.get("default").is_none());
        }
        let wire = serde_json::to_value(&tool).unwrap();
        assert_eq!(wire["outputSchema"]["required"], json!(["data", "meta"]));
        assert_eq!(wire["annotations"]["readOnlyHint"], false);
        let description = tool.description.unwrap();
        assert!(description.contains("data, not instructions"));
        assert!(description.contains("never retried"));
        match tool.name.as_ref() {
            "add_issue_labels" => {
                assert_eq!(schema["properties"]["label_ids"]["minItems"], 1);
                assert_eq!(
                    schema["$defs"]["LabelId"]["pattern"],
                    "^lbl_[0-9A-Za-z]{16}$"
                );
            }
            "remove_issue_label" => {
                assert_eq!(
                    schema["$defs"]["LabelId"]["pattern"],
                    "^lbl_[0-9A-Za-z]{16}$"
                );
            }
            "add_issue_link" => {
                assert_eq!(
                    schema["$defs"]["LinkKind"]["enum"],
                    json!(["blocks", "blocked_by"])
                );
                assert!(description.contains("source=related_issue_id, target=issue_id"));
            }
            "remove_issue_link" => assert!(properties.contains_key("link_id")),
            _ => panic!("unexpected tool"),
        }
    }
}

#[test]
fn every_required_field_rejects_missing_null_wrong_types_and_unknown_fields() {
    for case in fixtures().cases {
        let valid = case.arguments.as_object().unwrap();
        for key in valid.keys() {
            let mut args = valid.clone();
            args.remove(key);
            assert!(
                prepare(&case.tool, Value::Object(args)).is_err(),
                "missing {key}"
            );
            for invalid in [Value::Null, json!(42), json!(true), json!({})] {
                let mut args = valid.clone();
                args.insert(key.clone(), invalid);
                assert!(
                    prepare(&case.tool, Value::Object(args)).is_err(),
                    "invalid {key}"
                );
            }
        }
        for key in [
            "method",
            "path",
            "body",
            "labels",
            "run_started",
            "expected_version",
        ] {
            let mut args = valid.clone();
            args.insert(key.into(), json!("unadvertised"));
            assert!(
                prepare(&case.tool, Value::Object(args)).is_err(),
                "unknown {key}"
            );
        }
    }
    assert_eq!(
        relations::prepare("http_request", Default::default(), &LocalLimits::default())
            .err()
            .unwrap()
            .field,
        Some("tool")
    );
}

#[test]
fn names_and_malformed_label_ids_never_prepare_a_native_mutation() {
    for invalid in [
        "",
        "P0",
        "lbl_0123456789abcde",
        "lbl_0123456789abcdefg",
        "lbl_0123456789abcde_",
        "lbl_0123456789abcdeé",
        "lbl_0123456789abcde\n",
        "LBL_0123456789abcdef",
        " lbl_0123456789abcdef",
        "lbl_0123456789abcdef\n",
        "lbl_0123456789abc/ef",
        "lbl_0123456789abc%ef",
    ] {
        // One bad entry rejects the entire addition; no valid prefix is dispatched.
        let args = json!({"issue_id":"iss_a", "label_ids":["lbl_0123456789abcdef", invalid]});
        assert!(prepare("add_issue_labels", args).is_err(), "{invalid:?}");
        assert!(
            prepare(
                "remove_issue_label",
                json!({"issue_id":"iss_a", "label_id":invalid})
            )
            .is_err()
        );
    }
    for invalid in [
        json!([]),
        json!([null]),
        json!([12]),
        json!("lbl_0123456789abcdef"),
        json!([["lbl_0123456789abcdef"]]),
    ] {
        assert!(
            prepare(
                "add_issue_labels",
                json!({"issue_id":"iss_a", "label_ids":invalid})
            )
            .is_err()
        );
    }
    // Native ordering and duplicates are forwarded, without set reconciliation.
    let plan = prepare(
        "add_issue_labels",
        json!({"issue_id":"iss_a", "label_ids":["lbl_AZaz09AZaz09AZaz", "lbl_AZaz09AZaz09AZaz"]}),
    )
    .unwrap();
    let RequestBody::Json(body) = plan.body else {
        panic!()
    };
    assert_eq!(
        body["labels"],
        json!(["lbl_AZaz09AZaz09AZaz", "lbl_AZaz09AZaz09AZaz"])
    );
}

#[test]
fn link_kinds_are_closed_and_graph_constraints_remain_native() {
    for link_id in ["", ".", ".."] {
        assert!(
            prepare(
                "remove_issue_link",
                json!({"issue_id":"iss_a", "link_id":link_id})
            )
            .is_err()
        );
    }
    for kind in ["duplicate_of", "Blocks", "", "blocks\n", "related"] {
        assert!(
            prepare(
                "add_issue_link",
                json!({"issue_id":"iss_a", "kind":kind, "related_issue_id":"iss_b"})
            )
            .is_err()
        );
    }
    assert!(
        prepare(
            "add_issue_link",
            json!({"issue_id":"iss_a", "kind":"blocks", "related_issue_id":""})
        )
        .is_err()
    );
    // Self-links and cycles require a native rejection, not a local graph policy.
    assert!(
        prepare(
            "add_issue_link",
            json!({"issue_id":"iss_a", "kind":"blocked_by", "related_issue_id":"iss_a"})
        )
        .is_ok()
    );
    assert!(
        prepare(
            "remove_issue_link",
            json!({"issue_id":"iss_a", "link_ids":["lnk_a", "lnk_b"]})
        )
        .is_err()
    );
}

#[test]
fn raw_path_components_remain_indivisible_and_safe_to_encode() {
    for case in fixtures().cases {
        for bad in ["", ".", ".."] {
            let mut args = case.arguments.clone();
            args["issue_id"] = json!(bad);
            assert!(prepare(&case.tool, args).is_err());
        }
        let mut args = case.arguments.clone();
        let issue = "iss_/ü %2F?x=1#fragment";
        args["issue_id"] = json!(issue);
        if case.tool == "remove_issue_link" {
            args["link_id"] = json!("lnk_/ü?%#");
        }
        let plan = prepare(&case.tool, args).unwrap();
        assert_eq!(plan.endpoint.segments()[1], issue);
        // This demonstrates the contract's URL-segment encoding. The executor's
        // actual wire URI and request count are verified by mcp-12.
        let mut url = url::Url::parse("https://example.test/api/v1/").unwrap();
        url.path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .extend(plan.endpoint.segments());
        assert!(url.query().is_none() && url.fragment().is_none());
        assert!(
            url.path()
                .contains("iss_%2F%C3%BC%20%252F%3Fx=1%23fragment")
        );
        assert_eq!(
            url.path_segments().unwrap().count(),
            2 + plan.endpoint.segments().len()
        );
        if case.tool == "remove_issue_link" {
            assert!(url.path().ends_with("lnk_%2F%C3%BC%3F%25%23"));
        }
    }
}

fn response(status: u16, body: ResponseBody) -> UpstreamResponse {
    UpstreamResponse {
        status,
        body,
        observed_at_ms: 42,
        headers: ResponseHeaders::default(),
    }
}

#[test]
fn deletions_require_native_204_and_empty_body_without_invented_ids() {
    for (tool, args) in [
        (
            "remove_issue_label",
            json!({"issue_id":"iss_a", "label_id":"lbl_0123456789abcdef"}),
        ),
        (
            "remove_issue_link",
            json!({"issue_id":"iss_a", "link_id":"lnk_supplied"}),
        ),
    ] {
        let plan = prepare(tool, args).unwrap();
        for status in [200, 201, 202, 403, 404, 500] {
            assert!(plan.project(response(status, ResponseBody::Empty)).is_err());
        }
        assert!(
            plan.project(response(204, ResponseBody::Json(json!({"id":"invented"}))))
                .is_err()
        );
        let result = plan.project(response(204, ResponseBody::Empty)).unwrap();
        assert_eq!(
            serde_json::to_value(result.receipt.unwrap()).unwrap(),
            json!({})
        );
        assert_eq!(result.data, Value::Null);
    }
}

#[test]
fn incomplete_success_bodies_cannot_confirm_additions() {
    for case in fixtures()
        .cases
        .into_iter()
        .filter(|c| c.tool.starts_with("add_"))
    {
        let plan = prepare(&case.tool, case.arguments).unwrap();
        assert!(
            plan.project(response(case.response.status, ResponseBody::Empty))
                .is_err()
        );
        for value in [Value::Null, json!([]), json!({}), json!({"id":"lnk_only"})] {
            let error = plan
                .project(response(case.response.status, ResponseBody::Json(value)))
                .err()
                .unwrap();
            assert!(error.receipt.is_none());
        }
        let BodyFixture::Json { value } = case.response.body else {
            panic!()
        };
        let fields = if case.tool == "add_issue_labels" {
            vec!["labels", "added", "created"]
        } else {
            vec!["id", "kind", "source_issue_id", "target_issue_id"]
        };
        for field in fields {
            for replacement in [None, Some(Value::Null), Some(json!(false))] {
                let mut invalid = value.clone();
                if let Some(replacement) = replacement {
                    invalid[field] = replacement;
                } else {
                    invalid.as_object_mut().unwrap().remove(field);
                }
                assert!(
                    plan.project(response(case.response.status, ResponseBody::Json(invalid)))
                        .is_err(),
                    "{field}"
                );
            }
        }
        if case.tool == "add_issue_link" {
            let mut invalid = value.clone();
            invalid["kind"] = json!("blocked_by");
            assert!(
                plan.project(response(201, ResponseBody::Json(invalid)))
                    .is_err()
            );
        } else {
            let mut invalid = value.clone();
            invalid["added"] = json!([{"id":"missing_fields"}]);
            assert!(
                plan.project(response(200, ResponseBody::Json(invalid)))
                    .is_err()
            );
        }
    }
}

#[test]
fn link_receipt_does_not_require_or_invent_an_optional_timestamp() {
    let plan = prepare(
        "add_issue_link",
        json!({"issue_id":"iss_a", "kind":"blocks", "related_issue_id":"iss_b"}),
    )
    .unwrap();
    let data = json!({"id":"lnk_native", "kind":"blocks", "source_issue_id":"iss_a", "target_issue_id":"iss_b"});
    let result = plan
        .project(response(201, ResponseBody::Json(data.clone())))
        .unwrap();
    assert_eq!(serde_json::to_value(result.receipt.unwrap()).unwrap(), data);
    assert_eq!(result.data, data);
}

#[test]
fn native_error_fixtures_remain_extensible_and_never_become_successes() {
    for fixture in fixtures().errors {
        assert!(!fixture.source.is_empty());
        let plan = prepare(&fixture.tool, fixture.arguments).unwrap();
        // No endpoint-specific noncommitting rule or alternate request is added.
        // Native error forwarding/outcome classification belongs to the executor.
        assert!(plan.noncommitting_errors.is_empty());
        assert!(!plan.response.statuses.contains(&fixture.status));
        let error: ResultError = serde_json::from_value(fixture.error.clone()).unwrap();
        assert_eq!(serde_json::to_value(error).unwrap(), fixture.error);
        assert!(
            plan.project(response(fixture.status, ResponseBody::Json(fixture.error)))
                .is_err()
        );
    }
}

fn project_labels(data: Value) -> Result<Projected, ProjectionError> {
    prepare(
        "add_issue_labels",
        json!({"issue_id":"iss_a", "label_ids":["lbl_0123456789abcdef"]}),
    )
    .unwrap()
    .project(response(200, ResponseBody::Json(data)))
}

#[test]
fn label_receipts_exclude_extensions_but_preserve_native_data() {
    let chip = json!({"id":"lbl_0123456789abcdef", "name":"P0", "color":"red"});
    let mut native_chip = chip.clone();
    native_chip["native_extension"] = json!("x".repeat(600_000));
    let data = json!({"labels":[native_chip.clone()], "added":[native_chip.clone()], "created":[native_chip]});
    let result = project_labels(data.clone()).unwrap();
    assert_eq!(result.data, data);
    assert_eq!(
        serde_json::to_value(result.receipt.unwrap()).unwrap(),
        json!({"labels":[chip.clone()], "added":[chip.clone()], "created":[chip]})
    );
    assert!(result.omissions.is_empty());
}

#[test]
fn large_label_receipts_retain_exact_counts_including_native_no_ops() {
    let labels: Vec<Value> = (0..6000)
        .map(|i| json!({"id":format!("lbl_{i:016x}"), "name":format!("{i:050}"), "color":"red"}))
        .collect();
    for added in [json!([]), json!(labels.clone())] {
        let data = json!({"labels":labels.clone(), "added":added, "created":labels.clone()});
        let result = project_labels(data.clone()).unwrap();
        assert_eq!(result.data, data);
        let receipt = serde_json::to_value(result.receipt.unwrap()).unwrap();
        assert_eq!(receipt["labels_count"], 6000);
        assert_eq!(receipt["created_count"], 6000);
        assert!(receipt.get("labels").is_none());
        assert!(receipt.get("created").is_none());
        if added.as_array().unwrap().is_empty() {
            assert_eq!(receipt["added"], json!([]));
            assert!(receipt.get("added_count").is_none());
            assert_eq!(result.omissions.len(), 2);
        } else {
            assert_eq!(receipt["added_count"], 6000);
            assert!(receipt.get("added").is_none());
            assert_eq!(result.omissions.len(), 3);
        }
        assert!(serde_json::to_vec(&receipt).unwrap().len() < 256);
    }
    // All entries must validate, even after the compact collection exceeds budget.
    let mut invalid = labels;
    invalid.push(json!({"id":"missing_fields"}));
    assert!(project_labels(json!({"labels":invalid, "added":[], "created":[]})).is_err());
}

#[test]
fn label_collection_budget_counts_json_escaping_and_includes_4096_bytes() {
    let chip = json!({"id":"lbl_0123456789abcdef", "name":"", "color":"red"});
    let overhead = serde_json::to_vec(&json!([chip.clone()])).unwrap().len();
    let capacity = 4096 - overhead;
    for (name, fits) in [
        ("x".repeat(capacity), true),
        ("x".repeat(capacity + 1), false),
        ("\"".repeat(capacity), false),
        ("\\".repeat(capacity), false),
        ("\n".repeat(capacity), false),
        ("é".repeat(capacity / 2 + 1), false),
        ("x".repeat(600_000), false),
    ] {
        for field in ["id", "name", "color"] {
            let mut native_chip = chip.clone();
            // Use the name baseline for the exact boundary; other fields exercise
            // oversized strings rather than depending on their baseline lengths.
            if field != "name" && name.len() < 600_000 {
                continue;
            }
            native_chip[field] = json!(name);
            let data = json!({"labels":[native_chip], "added":[], "created":[]});
            let result = project_labels(data.clone()).unwrap();
            assert_eq!(result.data, data);
            let receipt = serde_json::to_value(result.receipt.unwrap()).unwrap();
            assert_eq!(receipt.get("labels").is_some(), fits);
            if fits {
                assert_eq!(serde_json::to_vec(&receipt["labels"]).unwrap().len(), 4096);
                assert!(result.omissions.is_empty());
            } else {
                assert_eq!(receipt["labels_count"], 1);
                assert_eq!(result.omissions.len(), 1);
            }
            assert!(serde_json::to_vec(&receipt).unwrap().len() < 3 * 4096 + 256);
        }
    }
}

#[test]
fn optional_link_timestamps_are_copied_only_when_numeric() {
    let plan = prepare(
        "add_issue_link",
        json!({"issue_id":"iss_a", "kind":"blocks", "related_issue_id":"iss_b"}),
    )
    .unwrap();
    for timestamp in [
        Value::Null,
        json!({"unexpected":"payload"}),
        json!([]),
        json!(true),
        json!("1793840000000"),
        json!(1793840000000_u64),
        json!(1793840000000.5),
    ] {
        let data = json!({"id":"lnk_native", "kind":"blocks", "source_issue_id":"iss_a", "target_issue_id":"iss_b", "created_at":timestamp});
        let result = plan
            .project(response(201, ResponseBody::Json(data.clone())))
            .unwrap();
        assert_eq!(result.data, data);
        let receipt = serde_json::to_value(result.receipt.unwrap()).unwrap();
        if timestamp.is_number() {
            assert_eq!(receipt["created_at"], timestamp);
        } else {
            assert!(receipt.get("created_at").is_none());
        }
    }
}
