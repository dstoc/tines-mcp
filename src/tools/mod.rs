//! Registration seams for the direct-call implementation.
//!
//! mcp-00 registers empty module interfaces, not unfinished MCP tools. mcp-01
//! owns dispatcher/executor wiring; feature packages only edit their own module
//! and tests. Existing runtime tools stay in read_tools until the integration gates.

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
/// mcp-01 will invoke the executor once on the returned single call.
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
