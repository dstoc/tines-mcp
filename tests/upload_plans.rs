mod support;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use support::direct_calls::{self, BodyFixture, ResponseFixture};
use tines_mcp::tools::{self, contract::*, uploads};

const TOOL: &str = "upload_issue_artifact_file";

fn args(encoding: &str, content: &str) -> Value {
    json!({"issue_id":"iss_input", "name":"proposal", "filename":"proposal.md", "content_type":"text/markdown; charset=utf-8", "encoding":encoding, "content":content})
}

fn prepare(value: Value) -> Result<PreparedCall, LocalError> {
    tools::prepare(
        TOOL,
        value.as_object().unwrap().clone(),
        &LocalLimits::default(),
    )
}

fn body(plan: &PreparedCall) -> &[u8] {
    let RequestBody::Bytes { bytes, .. } = &plan.body else {
        panic!("expected raw bytes")
    };
    bytes
}

fn response(status: u16, native: Value) -> ResponseFixture {
    serde_json::from_value(json!({"status":status,"body":{"kind":"json","value":native}})).unwrap()
}

fn artifact() -> Value {
    json!({
        "id":"ctx_returned", "issue_id":"iss_returned", "name":"returned-name", "artifact_type":"file",
        "description":"separate descriptive metadata", "version_count":3, "fresh":false,
        "current_version":{"version":3,"filename":"report.json","content_type":"application/json","size_bytes":0,"reaffirmed_from":null,"created_at":1793840000000_u64},
        "unknown_native":{"permission":"preserved"}
    })
}

#[test]
fn unicode_text_is_one_exact_raw_put_and_filename_is_only_query_metadata() {
    let text = "  # Proposal 🦀\nUse $VAR and `code`.\r\n\0\"\\  ";
    let mut value = args("utf8", text);
    value["filename"] = json!("报告 &résumé+#?%.md");
    let plan = prepare(value).unwrap();
    let inspected = direct_calls::inspect(&plan);
    assert_eq!(inspected.method, Method::Put);
    assert_eq!(
        inspected.template,
        "/issues/{issue_id}/artifacts/{name}/file"
    );
    assert_eq!(
        inspected.segments,
        ["issues", "iss_input", "artifacts", "proposal", "file"]
    );
    assert_eq!(
        inspected.query,
        [("filename".into(), "报告 &résumé+#?%.md".into())]
    );
    assert_eq!(
        inspected.body,
        BodyFixture::Bytes {
            bytes: text.as_bytes().to_vec(),
            content_type: "text/markdown; charset=utf-8".into()
        }
    );
    // The raw pair round-trips through the executor's URL-library encoding.
    // Actual executor request counts/encoding remain the mcp-13 HTTP gate.
    let mut url =
        url::Url::parse("https://example.invalid/api/v1/issues/i/artifacts/a/file").unwrap();
    url.query_pairs_mut().extend_pairs(&plan.query);
    assert_eq!(
        url.query_pairs().collect::<Vec<_>>(),
        [("filename".into(), "报告 &résumé+#?%.md".into())]
    );
    assert!(!url.as_str().contains("résumé"));
    assert_eq!(plan.response.statuses, &[200]);
    assert!(matches!(plan.response.kind, ResponseKind::Json));
    assert!(plan.noncommitting_errors.is_empty());
}

#[test]
fn utf8_and_base64_preserve_empty_and_binary_bytes() {
    for encoding in ["utf8", "base64"] {
        assert!(body(&prepare(args(encoding, "")).unwrap()).is_empty());
    }
    for size in [1, 2, 3, 4, 255, 256, 257] {
        let bytes: Vec<u8> = (0..size).map(|n| n as u8).collect();
        let mut value = args("base64", &STANDARD.encode(&bytes));
        value["content_type"] = json!("application/octet-stream");
        assert_eq!(body(&prepare(value).unwrap()), bytes);
    }
    // A named JSON file is a raw upload, not a metadata JSON request.
    let mut value = args("utf8", "{\"description\":\"literal file contents\"}");
    value["content_type"] = json!("application/json");
    value["filename"] = json!("report.json");
    assert_eq!(
        body(&prepare(value).unwrap()),
        b"{\"description\":\"literal file contents\"}"
    );
}

#[test]
fn base64_is_strict_and_never_accepts_handles_urls_or_paths_as_sources() {
    for content in [
        "Zg",
        "Zg=",
        "Zg===",
        "=AAA",
        "AA=A",
        "!!!!",
        "_w==",
        "-w==",
        "Zh==",
        "Zm9=",
        "Zg==\n",
        " Zg==",
        "====",
        "🦀",
        "https://example.invalid/file",
        "/tmp/file.bin",
        "attachment_opaque",
    ] {
        let error = prepare(args("base64", content)).err().unwrap();
        assert_eq!(error.code, "invalid_input", "{content}");
        assert_eq!(error.field, Some("content"));
        assert!(
            !error.message.contains(content),
            "payload must not be echoed"
        );
    }
    // UTF-8 always treats source-looking strings as literal bytes.
    for content in [
        "/tmp/nonexistent-upload-file",
        "https://example.invalid/file",
        "attachment_opaque",
    ] {
        assert_eq!(
            body(&prepare(args("utf8", content)).unwrap()),
            content.as_bytes()
        );
    }
}

#[test]
fn native_byte_cap_is_exact_including_base64_padding_boundaries() {
    let limits = LocalLimits::new(HARD_MAX_REQUEST_BYTES).unwrap();
    for size in [
        MAX_FILE_BYTES - 2,
        MAX_FILE_BYTES - 1,
        MAX_FILE_BYTES,
        MAX_FILE_BYTES + 1,
        MAX_FILE_BYTES + 2,
        MAX_FILE_BYTES + 3,
    ] {
        let bytes = vec![255; size];
        let value = args("base64", &STANDARD.encode(&bytes));
        let result = uploads::prepare(TOOL, value.as_object().unwrap().clone(), &limits);
        if size <= MAX_FILE_BYTES {
            assert_eq!(body(&result.unwrap()), bytes);
        } else {
            assert_eq!(result.err().unwrap().field, Some("content"));
        }
    }
    // UTF-8 checks byte length, not character count.
    let text = "🦀".repeat(MAX_FILE_BYTES / 4);
    assert_eq!(
        body(&prepare(args("utf8", &text)).unwrap()).len(),
        MAX_FILE_BYTES
    );
    assert_eq!(
        prepare(args("utf8", &(text + "a"))).err().unwrap().field,
        Some("content")
    );
}

#[test]
fn schemas_and_runtime_reject_every_extra_missing_null_or_wrong_type_field() {
    let tool = uploads::definitions(&LocalLimits::default()).remove(0);
    assert_eq!(tool.name, TOOL);
    let schema = serde_json::to_value(&tool.input_schema).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    let fields = [
        "issue_id",
        "name",
        "filename",
        "content_type",
        "encoding",
        "content",
    ];
    assert_eq!(
        schema["properties"].as_object().unwrap().len(),
        fields.len()
    );
    for field in fields {
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(field))
        );
        let mut missing = args("utf8", "");
        missing.as_object_mut().unwrap().remove(field);
        assert!(prepare(missing).is_err());
        for invalid in [Value::Null, json!(123), json!({})] {
            let mut value = args("utf8", "");
            value[field] = invalid;
            assert!(prepare(value).is_err());
        }
    }
    for extra in [
        "description",
        "title",
        "path",
        "url",
        "attachment_id",
        "chunks",
        "headers",
        "expected_version",
        "type",
    ] {
        let mut value = args("utf8", "");
        value[extra] = json!("unadvertised");
        assert!(prepare(value).is_err(), "{extra}");
        assert!(schema["properties"].get(extra).is_none());
    }
    for encoding in ["UTF8", "binary", "url", "attachment"] {
        assert!(prepare(args(encoding, "")).is_err());
    }
    let encoding = &schema["$defs"]["Encoding"]["enum"];
    assert_eq!(encoding, &json!(["utf8", "base64"]));
    assert!(tool.output_schema.is_some());
    let annotations = tool.annotations.unwrap();
    assert_eq!(annotations.read_only_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(false));
}

#[test]
fn header_injection_and_retargeting_are_local_errors() {
    for content_type in [
        "",
        "text/plain\r\nAuthorization: secret",
        "text/plain\n",
        "\0",
        "text/🦀",
        "text/plain\t",
        "text/plain\u{7f}",
    ] {
        let mut value = args("utf8", "");
        value["content_type"] = json!(content_type);
        assert_eq!(prepare(value).err().unwrap().field, Some("content_type"));
    }
    for field in ["issue_id", "name"] {
        for segment in ["", ".", ".."] {
            let mut value = args("utf8", "");
            value[field] = json!(segment);
            assert!(prepare(value).is_err());
        }
    }
    assert_eq!(
        uploads::prepare("http_request", Default::default(), &LocalLimits::default())
            .err()
            .unwrap()
            .field,
        Some("tool")
    );
}

#[test]
fn discovery_advertises_effective_wire_cap_without_pretending_it_is_a_file_cap() {
    for cap in [12345, DEFAULT_MAX_REQUEST_BYTES, HARD_MAX_REQUEST_BYTES] {
        let limits = LocalLimits::new(cap).unwrap();
        let tool = uploads::definitions(&limits).remove(0);
        let description = tool.description.unwrap();
        assert!(description.contains(&format!("cap is {cap} bytes")));
        assert!(description.contains("26214400 bytes"));
        assert!(description.contains("envelope, metadata and JSON escaping"));
        assert!(description.contains("separate explicit metadata-only"));
    }
    // Preparation has no serialized HTTP envelope. Ingress owns the wire cap;
    // a base64 payload's size is not a substitute for the whole JSON-RPC body.
    let limits = LocalLimits::new(1).unwrap();
    assert!(
        uploads::prepare(
            TOOL,
            args("utf8", "literal").as_object().unwrap().clone(),
            &limits
        )
        .is_ok()
    );
}

#[test]
fn native_response_and_observed_receipt_are_preserved_without_input_evidence() {
    let plan = prepare(args("utf8", "")).unwrap();
    let native = artifact();
    let projected = direct_calls::project(&plan, response(200, native.clone())).unwrap();
    assert_eq!(projected.data, native);
    assert_eq!(
        serde_json::to_value(projected.receipt).unwrap(),
        json!({
            "id":"ctx_returned", "issue_id":"iss_returned", "name":"returned-name", "artifact_type":"file", "version_count":3, "fresh":false,
            "current_version":{"version":3,"reaffirmed_from":null,"created_at":1793840000000_u64}
        })
    );
    assert!(projected.links.is_empty());
    assert!(projected.omissions.is_empty());
    // Unknown/native optional data stays in data, but malformed optional receipt
    // values cannot inflate the compact success evidence.
    let mut native = artifact();
    native["current_version"]["reaffirmed_from"] = json!({"unexpected":"value"});
    native["current_version"]["created_at"] = json!("invalid");
    let projected = direct_calls::project(&plan, response(200, native.clone())).unwrap();
    assert_eq!(projected.data, native);
    assert_eq!(
        serde_json::to_value(projected.receipt).unwrap()["current_version"],
        json!({"version":3})
    );
}

#[test]
fn missing_or_invalid_receipt_fields_and_unexpected_statuses_cannot_confirm_uploads() {
    let plan = prepare(args("utf8", "")).unwrap();
    for pointer in [
        "/id",
        "/issue_id",
        "/name",
        "/artifact_type",
        "/version_count",
        "/fresh",
        "/current_version",
        "/current_version/version",
    ] {
        let mut native = artifact();
        *native.pointer_mut(pointer).unwrap() = Value::Null;
        assert_eq!(
            direct_calls::project(&plan, response(200, native))
                .err()
                .unwrap()
                .code,
            "invalid_receipt"
        );
    }
    for (pointer, invalid) in [
        ("/id", json!("")),
        ("/artifact_type", json!("text")),
        ("/version_count", json!(0)),
        ("/version_count", json!("3")),
        ("/fresh", json!(1)),
        ("/current_version/version", json!(-1)),
        ("/current_version/version", json!(1.5)),
    ] {
        let mut native = artifact();
        *native.pointer_mut(pointer).unwrap() = invalid;
        assert!(direct_calls::project(&plan, response(200, native)).is_err());
    }
    for status in [201, 204, 403, 409, 422, 500] {
        assert_eq!(
            direct_calls::project(&plan, response(status, artifact()))
                .err()
                .unwrap()
                .code,
            "unexpected_status"
        );
    }
    assert!(direct_calls::project(&plan, response(200, json!({}))).is_err());
}

#[test]
fn native_permission_version_cap_and_type_errors_have_no_local_success_or_rejection_rule() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/uploads.json")).unwrap();
    let plan = prepare(args("utf8", "")).unwrap();
    for fixture in fixtures["errors"].as_array().unwrap() {
        let status = fixture["status"].as_u64().unwrap() as u16;
        let error: ResultError = serde_json::from_value(fixture["error"].clone()).unwrap();
        assert!(
            plan.noncommitting_errors
                .iter()
                .all(|rule| !rule.matches(status, &error))
        );
        assert_eq!(
            direct_calls::project(&plan, response(status, fixture["error"].clone()))
                .err()
                .unwrap()
                .code,
            "unexpected_status"
        );
    }
}
