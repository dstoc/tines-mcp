//! Pure domain registration and one-request dispatch.
//! Domain preparation and response projection cannot access the HTTP client.

pub mod artifacts;
pub mod context;
pub mod contract;
pub mod issues;
pub mod operations_read;
pub mod reads;
pub mod recovery;
pub mod relations;
pub mod transitions;
pub mod uploads;

use rmcp::model::Tool;
use serde_json::{Map, Value};

use contract::{LocalError, LocalLimits, PreparedCall};

pub type Prepare = fn(&str, Map<String, Value>, &LocalLimits) -> Result<PreparedCall, LocalError>;

/// A module supplies definitions and pure preparation. The dispatcher selects
/// by advertised name; a selected module must reject unknown argument fields.
/// No user-supplied method/path/body operation is registered here.
pub struct Module {
    pub name: &'static str,
    pub definitions: fn(&LocalLimits) -> Vec<Tool>,
    pub prepare: Prepare,
}

pub const MODULES: &[Module] = &[
    reads::MODULE,
    issues::MODULE,
    transitions::MODULE,
    relations::MODULE,
    artifacts::MODULE,
    uploads::MODULE,
    context::MODULE,
    operations_read::MODULE,
    recovery::MODULE,
];

pub fn definitions(limits: &LocalLimits) -> Vec<Tool> {
    MODULES
        .iter()
        .flat_map(|module| (module.definitions)(limits))
        .collect()
}

/// Only definitions supplied by a module can select its preparation function.
/// The dispatcher invokes the executor once on the returned single call.
pub fn prepare(
    name: &str,
    arguments: Map<String, Value>,
    limits: &LocalLimits,
) -> Result<PreparedCall, LocalError> {
    prepare_in(MODULES, name, arguments, limits)
}

fn prepare_in(
    modules: &[Module],
    name: &str,
    arguments: Map<String, Value>,
    limits: &LocalLimits,
) -> Result<PreparedCall, LocalError> {
    let mut owners = modules.iter().filter(|module| {
        (module.definitions)(limits)
            .iter()
            .any(|tool| tool.name == name)
    });
    let Some(owner) = owners.next() else {
        return Err(LocalError::invalid("tool", "unknown tool"));
    };
    if owners.next().is_some() {
        return Err(LocalError {
            code: "duplicate_tool",
            field: None,
            message: "Tool has multiple owners".into(),
        });
    }
    (owner.prepare)(name, arguments, limits)
}

#[cfg(test)]
#[path = "registration_tests.rs"]
mod registration_tests;

/// Dispatch an advertised domain tool through pure preparation and the sole
/// executor. Integrations can use this entry point without owning shared wiring.
pub async fn dispatch(
    client: &crate::tines::TinesClient,
    name: &str,
    arguments: Map<String, Value>,
    limits: &LocalLimits,
    cancellation: tokio_util::sync::CancellationToken,
) -> rmcp::model::CallToolResult {
    dispatch_in(MODULES, client, name, arguments, limits, cancellation).await
}

pub(crate) async fn dispatch_in(
    modules: &[Module],
    client: &crate::tines::TinesClient,
    name: &str,
    arguments: Map<String, Value>,
    limits: &LocalLimits,
    cancellation: tokio_util::sync::CancellationToken,
) -> rmcp::model::CallToolResult {
    match prepare_in(modules, name, arguments, limits) {
        Ok(plan) => client.execute_prepared(plan, cancellation).await,
        Err(error) => {
            // An advertised mutation rejected during local preparation was never
            // attempted. Unknown tools have no selected operation to classify.
            let write = modules
                .iter()
                .flat_map(|m| (m.definitions)(limits))
                .find(|tool| tool.name == name)
                .is_some_and(|tool| {
                    tool.annotations.as_ref().and_then(|a| a.read_only_hint) != Some(true)
                });
            client.direct_result(contract::ResultEnvelope {
                data: Value::Null,
                error: Some(contract::ResultError {
                    code: error.code.into(),
                    message: error.message,
                    details: error.field.map(|field| serde_json::json!({"field": field})),
                }),
                meta: contract::ResultMeta {
                    http_status: None,
                    observed_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis(),
                    omissions: vec![],
                    links: vec![],
                    retry_after: None,
                    write: write.then_some(contract::WriteInfo {
                        outcome: contract::WriteOutcome::NotAttempted,
                        committed: None,
                        receipt: None,
                    }),
                },
            })
        }
    }
}
