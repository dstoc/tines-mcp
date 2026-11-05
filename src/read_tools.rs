//! MCP service facade. Registered tools use the shared one-request dispatcher.

use rmcp::{
    ServerHandler,
    model::{
        CallToolRequestMethod, CallToolRequestParams, CallToolResponse, Implementation,
        ListToolsResult, ServerCapabilities, ServerConfig, Tool,
    },
    service::{RequestContext, RoleServer},
};
use serde_json::{Value, json};

use crate::tines::TinesClient;

#[derive(Clone)]
pub struct ReadTools {
    tines: Option<TinesClient>,
    limits: crate::tools::contract::LocalLimits,
}

impl ReadTools {
    pub fn new(tines: Option<TinesClient>) -> Self {
        Self::with_limits(tines, crate::tools::contract::LocalLimits::default())
    }

    pub fn with_limits(
        tines: Option<TinesClient>,
        limits: crate::tools::contract::LocalLimits,
    ) -> Self {
        Self { tines, limits }
    }

    fn tool_definitions(&self) -> Vec<Tool> {
        crate::tools::definitions(&self.limits)
    }
}
impl ServerHandler for ReadTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tines-mcp", env!("CARGO_PKG_VERSION")))
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tool_definitions())))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_definitions()
            .into_iter()
            .find(|tool| tool.name.as_ref() == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let limits = self.limits;
        let registered = crate::tools::definitions(&limits)
            .into_iter()
            .find(|tool| tool.name == request.name);
        let result = if let Some(tool) = registered {
            if let Some(tines) = &self.tines {
                crate::tools::dispatch(
                    tines,
                    &request.name,
                    request.arguments.unwrap_or_default(),
                    &limits,
                    context.ct,
                )
                .await
            } else {
                unconfigured_result(
                    &request,
                    tool.annotations.as_ref().and_then(|a| a.read_only_hint) != Some(true),
                    &limits,
                )
            }
        } else {
            return Err(rmcp::ErrorData::method_not_found::<CallToolRequestMethod>());
        };
        Ok(result.into())
    }
}

fn unconfigured_result(
    request: &CallToolRequestParams,
    write: bool,
    limits: &crate::tools::contract::LocalLimits,
) -> rmcp::model::CallToolResult {
    use crate::tools::contract::{
        ResultEnvelope, ResultError, ResultMeta, WriteInfo, WriteOutcome,
    };
    let error = match crate::tools::prepare(
        &request.name,
        request.arguments.clone().unwrap_or_default(),
        limits,
    ) {
        Err(error) => ResultError {
            code: error.code.into(),
            message: error.message,
            details: error.field.map(|field| json!({"field":field})),
        },
        Ok(_) => ResultError {
            code: "tines_not_configured".into(),
            message: "Tines API settings are not configured for this server.".into(),
            details: None,
        },
    };
    let envelope = ResultEnvelope {
        data: Value::Null,
        error: Some(error),
        meta: ResultMeta {
            http_status: None,
            observed_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            omissions: vec![],
            links: vec![],
            retry_after: None,
            write: write.then_some(WriteInfo {
                outcome: WriteOutcome::NotAttempted,
                committed: None,
                receipt: None,
            }),
        },
    };
    rmcp::model::CallToolResult::structured_error(
        serde_json::to_value(envelope).expect("local error envelope serializes"),
    )
}
