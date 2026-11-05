use super::*;
use contract::{
    Endpoint, Method, Projected, Projection, RequestBody, ResponseBody, ResponseContract,
    ResponseKind,
};
use serde::Deserialize;
use serde_json::json;

fn definitions_a(limits: &LocalLimits) -> Vec<Tool> {
    vec![Tool::new(
        "fixture_a",
        format!("Wire cap: {}", limits.max_request_bytes()),
        rmcp::handler::server::common::schema_for_type::<Args>(),
    )]
}

fn definitions_b(_: &LocalLimits) -> Vec<Tool> {
    vec![Tool::new(
        "fixture_b",
        "Second independent module",
        rmcp::handler::server::common::schema_for_type::<Args>(),
    )]
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Args {
    id: String,
}

fn prepare_a(
    _: &str,
    args: Map<String, Value>,
    _: &LocalLimits,
) -> Result<PreparedCall, LocalError> {
    let args: Args = contract::arguments(args)?;
    Ok(PreparedCall {
        endpoint: Endpoint::new(Method::Get, "/fixtures/{id}", ["fixtures", &args.id])?,
        query: vec![],
        body: RequestBody::Empty,
        response: ResponseContract {
            statuses: &[200],
            kind: ResponseKind::Json,
        },
        projection: Projection {
            context: Value::Null,
            project: |response, _| {
                let ResponseBody::Json(data) = response.body else {
                    unreachable!()
                };
                Ok(Projected {
                    data,
                    receipt: None,
                    links: vec![],
                    omissions: vec![],
                })
            },
        },
        noncommitting_errors: &[],
    })
}

fn prepare_b(_: &str, _: Map<String, Value>, _: &LocalLimits) -> Result<PreparedCall, LocalError> {
    Err(LocalError::invalid("fixture_b", "module b owns this call"))
}

const A: Module = Module {
    name: "a",
    definitions: definitions_a,
    prepare: prepare_a,
};
const B: Module = Module {
    name: "b",
    definitions: definitions_b,
    prepare: prepare_b,
};

#[test]
fn modules_supply_independent_definitions_and_preparation() {
    let limits = LocalLimits::new(12345).unwrap();
    assert!(
        definitions_a(&limits)[0]
            .description
            .as_ref()
            .unwrap()
            .contains("12345")
    );
    let args = json!({"id":"x/y?z"}).as_object().unwrap().clone();
    let plan = prepare_in(&[A, B], "fixture_a", args, &limits).unwrap();
    assert_eq!(plan.endpoint.segments(), &["fixtures", "x/y?z"]);
    let error = prepare_in(&[A, B], "fixture_b", Map::new(), &limits)
        .err()
        .unwrap();
    assert_eq!(error.field, Some("fixture_b"));
    let error = prepare_in(
        &[A, B],
        "fixture_a",
        json!({"id":"x", "method":"DELETE"})
            .as_object()
            .unwrap()
            .clone(),
        &limits,
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "invalid_input");
}

#[test]
fn duplicate_and_unknown_tools_cannot_dispatch() {
    let limits = LocalLimits::default();
    assert_eq!(
        prepare_in(&[A, A], "fixture_a", Map::new(), &limits)
            .err()
            .unwrap()
            .code,
        "duplicate_tool"
    );
    assert_eq!(
        prepare_in(&[A, B], "http_request", Map::new(), &limits)
            .err()
            .unwrap()
            .field,
        Some("tool")
    );
}

#[test]
fn module_ownership_is_allocated_with_unique_domain_tools() {
    let names: Vec<_> = MODULES.iter().map(|module| module.name).collect();
    assert_eq!(
        names,
        [
            "reads",
            "issues",
            "transitions",
            "relations",
            "artifacts",
            "uploads",
            "context",
            "operations_read",
            "recovery"
        ]
    );
    let limits = LocalLimits::default();
    let tools = definitions(&limits);
    let mut tool_names = std::collections::HashSet::new();
    for tool in &tools {
        assert!(tool_names.insert(tool.name.as_ref()), "duplicate tool name");
        assert_ne!(tool.name, "http_request");
    }
    for module in MODULES {
        assert_eq!(
            (module.prepare)("http_request", Map::new(), &limits)
                .err()
                .unwrap()
                .field,
            Some("tool")
        );
    }
}
