mod support;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use support::direct_calls::{self, BodyFixture, PlanFixture, ResponseFixture};
use tines_mcp::tools::{self, contract::*};

fn plan(
    method: Method,
    body: RequestBody,
    kind: ResponseKind,
    statuses: &'static [u16],
) -> PreparedCall {
    PreparedCall {
        endpoint: Endpoint::new(
            method,
            "/fixtures/{id}",
            ["fixtures", "id/with?reserved#&% ü"],
        )
        .unwrap(),
        query: vec![
            ("label".into(), "one&two".into()),
            ("label".into(), "ü".into()),
        ],
        body,
        response: ResponseContract { statuses, kind },
        projection: Projection {
            project: project_fixture,
            context: json!({"input_id":"input_is_not_receipt_evidence"}),
        },
        noncommitting_errors: &[],
    }
}

// A fixture-only projector illustrates the frozen callback boundary. Feature
// modules must implement and test their own endpoint-specific validations.
fn project_fixture(response: UpstreamResponse, _: &Value) -> Result<Projected, ProjectionError> {
    let (data, receipt) = match response.body {
        ResponseBody::Json(data) => {
            let id = data
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| ProjectionError::new("invalid_receipt", "Response ID is missing"))?;
            let receipt = Receipt(json!({"id":id}).as_object().unwrap().clone());
            (data, receipt)
        }
        ResponseBody::Empty => (Value::Null, Receipt::default()),
        ResponseBody::Bytes(bytes) => (
            json!({"bytes":bytes, "content_type":response.headers.content_type}),
            Receipt::default(),
        ),
    };
    Ok(Projected {
        data,
        receipt: Some(receipt),
        links: vec![],
        omissions: vec![],
    })
}

fn response(value: Value) -> ResponseFixture {
    serde_json::from_value(value).unwrap()
}

#[test]
fn pure_helpers_inspect_one_request_and_project_extensible_json() {
    let call = plan(
        Method::Post,
        RequestBody::Json(json!({"title":"  literal $VAR `text` 🦀  "})),
        ResponseKind::Json,
        &[201],
    );
    let expected: PlanFixture = serde_json::from_value(json!({
        "method":"POST", "template":"/fixtures/{id}",
        "segments":["fixtures", "id/with?reserved#&% ü"],
        "query":[["label","one&two"],["label","ü"]],
        "body":{"kind":"json", "value":{"title":"  literal $VAR `text` 🦀  "}}
    }))
    .unwrap();
    assert_eq!(direct_calls::inspect(&call), expected);
    let native = json!({"id":"returned_id", "project":{"id":"member_project"}, "unknown":{"permission":"native"}, "redacted":["workspace"], "nullable":null});
    let projected = direct_calls::project(
        &call,
        response(json!({"status":201,"body":{"kind":"json","value":native}})),
    )
    .unwrap();
    assert_eq!(projected.data, native);
    assert_eq!(
        serde_json::to_value(projected.receipt).unwrap(),
        json!({"id":"returned_id"})
    );

    // The response must supply its own ID; the local input cannot confirm it.
    let error = direct_calls::project(
        &call,
        response(json!({"status":201,"body":{"kind":"json","value":{"title":"no ID"}}})),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "invalid_receipt");
}

#[test]
fn bytes_and_empty_receipts_have_distinct_contracts() {
    let call = plan(
        Method::Put,
        RequestBody::Bytes {
            bytes: vec![0, 255, 10],
            content_type: ContentType::new("application/octet-stream".into()).unwrap(),
        },
        ResponseKind::Bytes { max_bytes: 1024 },
        &[200],
    );
    assert_eq!(
        direct_calls::inspect(&call).body,
        BodyFixture::Bytes {
            bytes: vec![0, 255, 10],
            content_type: "application/octet-stream".into()
        }
    );
    let projected = direct_calls::project(&call, response(json!({"status":200,"body":{"kind":"bytes","bytes":[0,255,10],"content_type":"application/octet-stream"}}))).unwrap();
    assert_eq!(
        projected.data,
        json!({"bytes":[0,255,10], "content_type":"application/octet-stream"})
    );

    let call = plan(
        Method::Delete,
        RequestBody::Empty,
        ResponseKind::Empty,
        &[204],
    );
    assert_eq!(direct_calls::inspect(&call).body, BodyFixture::Empty);
    let projected = direct_calls::project(
        &call,
        response(json!({"status":204,"body":{"kind":"empty"}})),
    )
    .unwrap();
    assert!(projected.data.is_null());
    assert_eq!(serde_json::to_value(projected.receipt).unwrap(), json!({}));
    for wrong in [
        json!({"status":200,"body":{"kind":"empty"}}),
        json!({"status":204,"body":{"kind":"json","value":null}}),
    ] {
        assert!(direct_calls::project(&call, response(wrong)).is_err());
    }
}

#[test]
fn write_projection_requires_receipt_and_preserves_it_on_later_failure() {
    let mut call = plan(Method::Post, RequestBody::Empty, ResponseKind::Json, &[200]);
    call.projection.project = |response, _| {
        let ResponseBody::Json(data) = response.body else {
            unreachable!()
        };
        Ok(Projected {
            data,
            receipt: None,
            links: vec![],
            omissions: vec![],
        })
    };
    let fixture =
        || response(json!({"status":200,"body":{"kind":"json","value":{"id":"returned_id"}}}));
    assert_eq!(
        direct_calls::project(&call, fixture()).err().unwrap().code,
        "invalid_receipt"
    );
    call.projection.project = |response, context| {
        let validated = project_fixture(response, context)?;
        Err(
            ProjectionError::new("invalid_text_content", "Optional content was not UTF-8")
                .with_receipt(validated.receipt.unwrap()),
        )
    };
    let failure = direct_calls::project(&call, fixture()).err().unwrap();
    assert_eq!(failure.code, "invalid_text_content");
    assert_eq!(
        serde_json::to_value(failure.receipt).unwrap(),
        json!({"id":"returned_id"})
    );
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct PatchArgs {
    #[serde(default, skip_serializing_if = "PatchField::is_missing")]
    repo_branch: PatchField<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String")]
    body: Option<String>,
}

#[test]
fn nullable_patch_fields_preserve_missing_null_and_values() {
    for (wire, field) in [
        (json!({}), PatchField::Missing),
        (json!({"repo_branch":null}), PatchField::Null),
        (json!({"repo_branch":""}), PatchField::Value(String::new())),
        (
            json!({"repo_branch":"  🦀  "}),
            PatchField::Value("  🦀  ".into()),
        ),
    ] {
        let patch: PatchArgs = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(patch.repo_branch, field);
        assert_eq!(serde_json::to_value(&patch).unwrap(), wire);
    }
    assert!(serde_json::to_value(PatchField::<String>::Missing).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(PatchArgs)).unwrap();
    assert_eq!(
        schema["$defs"]["Nullable_string"]["type"],
        json!(["string", "null"])
    );
    assert!(schema.get("required").is_none());
    assert_eq!(schema["properties"]["body"]["type"], "string");
}

#[test]
fn strict_optional_nonnull_fields_do_not_treat_null_as_missing() {
    assert!(serde_json::from_value::<PatchArgs>(json!({"body":null})).is_err());
    let args = arguments::<PatchArgs>(
        json!({"body":"  literal text  "})
            .as_object()
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(args.body.as_deref(), Some("  literal text  "));
    let error = arguments::<PatchArgs>(
        json!({"secret_canary":"secret_canary"})
            .as_object()
            .unwrap()
            .clone(),
    )
    .unwrap_err();
    assert_eq!(error.code, "invalid_input");
    assert!(!format!("{error:?}").contains("secret_canary"));
}

#[test]
fn local_limits_and_shared_label_ids_do_not_require_upstream_state() {
    assert_eq!(LocalLimits::default().max_request_bytes(), 2 * 1024 * 1024);
    assert_eq!(
        LocalLimits::new(HARD_MAX_REQUEST_BYTES)
            .unwrap()
            .max_request_bytes(),
        36 * 1024 * 1024
    );
    assert!(LocalLimits::new(0).is_err());
    assert!(LocalLimits::new(HARD_MAX_REQUEST_BYTES + 1).is_err());
    assert_eq!(MAX_RESULT_BYTES, 512 * 1024);
    assert_eq!(MAX_ERROR_BYTES, 1024 * 1024);
    assert_eq!(MAX_SUCCESS_JSON_BYTES, 32 * 1024 * 1024);
    assert_eq!(MAX_FILE_BYTES, 25 * 1024 * 1024);
    assert_eq!(MAX_TEXT_BYTES, 256 * 1024);
    for valid in ["lbl_0123456789abcdef", "lbl_AZaz09AZaz09AZaz"] {
        assert!(validate_label_id(valid).is_ok());
    }
    for invalid in [
        "P0",
        "lbl_0123456789abcde",
        "lbl_0123456789abcdefg",
        "lbl_0123456789abcde_",
        "lbl_0123456789abcdeé",
    ] {
        assert!(validate_label_id(invalid).is_err());
    }
    for method in [Method::Post, Method::Patch, Method::Put, Method::Delete] {
        assert!(method.is_write());
    }
    assert!(!Method::Get.is_write());
}

#[test]
fn path_segments_and_content_type_cannot_retarget_or_inject_headers() {
    for bad in ["", ".", ".."] {
        assert!(Endpoint::new(Method::Get, "/fixtures/{id}", ["fixtures", bad]).is_err());
    }
    assert!(Endpoint::new(Method::Get, "/", Vec::<String>::new()).is_err());
    for bad in [
        "",
        "text/plain\r\nAuthorization: other",
        "text/plain\0",
        "text/ü",
    ] {
        assert!(ContentType::new(bad.into()).is_err());
    }
    assert_eq!(
        ContentType::new("text/markdown; charset=utf-8".into())
            .unwrap()
            .as_str(),
        "text/markdown; charset=utf-8"
    );
}

#[test]
fn envelope_keeps_native_error_evidence_outcomes_and_retry_after_separate() {
    let details = json!({"committed":true, "current":{"version":3}, "native_extension":[1,null]});
    let envelope = ResultEnvelope {
        data: Value::Null,
        error: Some(ResultError {
            code: "partial_failure".into(),
            message: "Native failure".into(),
            details: Some(details.clone()),
        }),
        meta: ResultMeta {
            http_status: Some(500),
            observed_at_ms: 42,
            omissions: vec![],
            links: vec![],
            retry_after: Some("10".into()),
            write: Some(WriteInfo {
                outcome: WriteOutcome::Confirmed,
                committed: Some(true),
                receipt: Some(Receipt(
                    json!({"id":"observed_id"}).as_object().unwrap().clone(),
                )),
            }),
        },
    };
    let wire = serde_json::to_value(&envelope).unwrap();
    assert_eq!(wire["error"]["details"], details);
    assert_eq!(wire["meta"]["write"]["outcome"], "confirmed");
    assert_eq!(wire["meta"]["retry_after"], "10");
    assert!(
        !wire["error"]["details"]
            .as_object()
            .unwrap()
            .contains_key("write_outcome")
    );
    assert!(serde_json::from_value::<ResultEnvelope>(wire).unwrap() == envelope);

    let mut success = envelope;
    success.error = None;
    success
        .meta
        .omissions
        .push("Native data omitted: serialized MCP result exceeded its budget".into());
    let wire = serde_json::to_value(success).unwrap();
    assert!(wire.get("error").is_none());
    assert_eq!(wire["meta"]["write"]["receipt"]["id"], "observed_id");
    assert_eq!(wire["data"], Value::Null);
}

#[test]
fn shared_result_schema_and_native_null_details_match_serialization() {
    let schema = serde_json::to_value(schemars::schema_for!(ResultEnvelope)).unwrap();
    assert_eq!(schema["required"], json!(["data", "meta"]));
    assert_eq!(
        schema["$defs"]["WriteOutcome"]["enum"],
        json!(["not_attempted", "rejected", "confirmed", "unknown"])
    );
    for wire in [
        json!({"code":"native", "message":"failed"}),
        json!({"code":"native", "message":"failed", "details":null}),
    ] {
        let error: ResultError = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(error).unwrap(), wire);
    }
}

#[test]
fn native_revision_and_permission_error_fixtures_preserve_details() {
    #[derive(Deserialize)]
    struct NativeErrorFixture {
        tool: String,
        status: u16,
        native_source: String,
        expected_outcome: WriteOutcome,
        error: ResultError,
    }
    #[derive(Deserialize)]
    struct Errors {
        error_fixtures: Vec<NativeErrorFixture>,
    }
    let fixtures: Errors =
        serde_json::from_str(include_str!("fixtures/direct-api-contracts.json")).unwrap();
    assert_eq!(fixtures.error_fixtures.len(), 4);
    for fixture in fixtures.error_fixtures {
        assert!(!fixture.native_source.is_empty());
        assert!(
            catalog()
                .endpoints
                .iter()
                .any(|entry| entry.tool == fixture.tool)
        );
        let error = serde_json::to_value(&fixture.error).unwrap();
        let envelope = ResultEnvelope {
            data: Value::Null,
            error: Some(fixture.error),
            meta: ResultMeta {
                http_status: Some(fixture.status),
                observed_at_ms: 42,
                omissions: vec![],
                links: vec![],
                retry_after: None,
                // mcp-01 must test classification against the same fixtures.
                write: Some(WriteInfo {
                    outcome: fixture.expected_outcome,
                    committed: error.pointer("/details/committed").and_then(Value::as_bool),
                    receipt: None,
                }),
            },
        };
        let wire = serde_json::to_value(envelope).unwrap();
        assert_eq!(wire["error"], error);
        assert_eq!(wire["meta"]["http_status"], fixture.status);
        assert!(
            !wire["error"]["details"]
                .as_object()
                .is_some_and(|details| details.contains_key("write_outcome"))
        );
    }
}

#[derive(Deserialize)]
struct Catalog {
    version: u32,
    reference_revision: String,
    endpoints: Vec<EndpointFixture>,
}
#[derive(Deserialize)]
struct EndpointFixture {
    tool: String,
    variant: String,
    module: String,
    method: Method,
    path: String,
    body: String,
    success: SuccessFixture,
    receipt: Option<ReceiptFixture>,
    semantics: Vec<String>,
    noncommitting_errors: Vec<RejectionFixture>,
}
#[derive(Deserialize)]
struct SuccessFixture {
    statuses: Vec<u16>,
    kind: String,
    native_shape: String,
}
#[derive(Deserialize)]
struct ReceiptFixture {
    required: Vec<String>,
    optional: Vec<String>,
}
#[derive(Deserialize)]
struct RejectionFixture {
    status: u16,
    code: String,
    native_test: String,
    proof: String,
}
fn catalog() -> Catalog {
    serde_json::from_str(include_str!("fixtures/direct-api-contracts.json")).unwrap()
}

#[test]
fn endpoint_catalog_records_receipts_ownership_and_native_contracts() {
    let catalog = catalog();
    assert_eq!(catalog.version, 1);
    assert_eq!(
        catalog.reference_revision,
        "af6cb25febf1b45d7eff32251f78fcf6355aed4a"
    );
    assert_eq!(catalog.endpoints.len(), 43);
    let mut variants = std::collections::HashSet::new();
    let mut tools = std::collections::HashSet::new();
    for entry in catalog.endpoints {
        assert!(variants.insert((entry.tool.clone(), entry.variant)));
        tools.insert(entry.tool);
        assert!(
            tools::MODULES
                .iter()
                .any(|module| module.name == entry.module)
        );
        assert!(entry.path.starts_with('/') && !entry.path.contains('?'));
        assert!(!entry.success.native_shape.is_empty());
        assert!(!entry.success.statuses.is_empty());
        assert!(
            entry
                .success
                .statuses
                .iter()
                .all(|s| (200..300).contains(s))
        );
        assert!(["json", "bytes", "empty"].contains(&entry.success.kind.as_str()));
        assert!(["json", "bytes", "empty"].contains(&entry.body.as_str()));
        assert_eq!(entry.receipt.is_some(), entry.method.is_write());
        if let Some(receipt) = entry.receipt {
            if entry.success.kind == "empty" {
                assert_eq!(entry.success.statuses, [204]);
                assert!(receipt.required.is_empty() && receipt.optional.is_empty());
            } else {
                assert!(!receipt.required.is_empty());
            }
            let mut paths = std::collections::HashSet::new();
            for pointer in receipt.required.iter().chain(&receipt.optional) {
                assert!(pointer.starts_with('/') && paths.insert(pointer));
            }
        }
        if entry.method.is_write() {
            assert!(!entry.semantics.is_empty());
        }
        for rule in entry.noncommitting_errors {
            assert_eq!(rule.status, 409);
            assert_eq!(rule.code, "version_conflict");
            assert!(rule.native_test.starts_with(&catalog.reference_revision));
            assert!(rule.proof.contains("unchanged"));
        }
    }
    assert_eq!(tools.len(), 41);
}

#[test]
fn tested_noncommitting_rules_are_endpoint_specific_and_exact() {
    let catalog = catalog();
    let patch = catalog
        .endpoints
        .iter()
        .find(|entry| entry.tool == "edit_context")
        .unwrap();
    let evidence = &patch.noncommitting_errors[0];
    let rule = NoncommittingError {
        status: 409,
        code: "version_conflict",
        native_test: "af6cb25febf1b45d7eff32251f78fcf6355aed4a apps/web/src/routes/api/v1/context/[id]/server.test.ts: redacts a conflict found at the commit, with %s",
    };
    assert_eq!(rule.native_test, evidence.native_test);
    let mut error = ResultError {
        code: "version_conflict".into(),
        message: "Changed".into(),
        details: Some(json!({"current":{"version":3}})),
    };
    assert!(rule.matches(409, &error));
    assert!(!rule.matches(403, &error));
    error.code = "permission_denied".into();
    assert!(!rule.matches(409, &error));
    error.code = "version_conflict".into();
    error.details = Some(json!({"committed":true}));
    assert!(!rule.matches(409, &error));
    // Explicit native evidence goes through the executor's native evidence path.
    error.details = Some(json!({"committed":false}));
    assert!(!rule.matches(409, &error));
    for tool in [
        "create_context",
        "append_context",
        "append_issue_comment",
        "transition_issue",
    ] {
        assert!(
            catalog
                .endpoints
                .iter()
                .find(|e| e.tool == tool)
                .unwrap()
                .noncommitting_errors
                .is_empty()
        );
    }
}
