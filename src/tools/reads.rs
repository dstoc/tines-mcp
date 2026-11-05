//! Owned by mcp-03. Add pure definitions, typed arguments and preparation here.
//! No tools are advertised until this module has an implementation and fixtures.

use rmcp::model::Tool;
use serde_json::{Map, Value};

use super::{
    Module,
    contract::{LocalError, LocalLimits, PreparedCall},
};

pub const MODULE: Module = Module {
    name: "reads",
    definitions,
    prepare,
};

pub fn definitions(_limits: &LocalLimits) -> Vec<Tool> {
    Vec::new()
}

pub fn prepare(
    _name: &str,
    _arguments: Map<String, Value>,
    _limits: &LocalLimits,
) -> Result<PreparedCall, LocalError> {
    Err(LocalError::invalid("tool", "unknown tool"))
}
