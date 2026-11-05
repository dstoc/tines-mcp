//! MCP service facade. Registered tools use the shared one-request dispatcher.
//! Only the legacy comment fallback remains until mcp-04 replaces it.

use std::{future::Future, sync::Arc};

use rmcp::{
    ServerHandler,
    handler::server::common::schema_for_type,
    model::{
        CallToolRequestMethod, CallToolRequestParams, CallToolResponse, Implementation,
        ListToolsResult, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    schemars::JsonSchema,
    service::{RequestContext, RoleServer},
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::tines::{
    Query, ResultEnvelope, Route, TinesClient, TinesError, TinesToolCall, WriteAttempt,
};

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
        let mut definitions = crate::tools::definitions(&self.limits);
        // The comment module owns its migration. Prefer its registered definition
        // when available so registration never creates a duplicate legacy tool.
        if !definitions
            .iter()
            .any(|tool| tool.name == "append_issue_comment")
        {
            definitions.push(tool::<AppendIssueCommentArgs>(
                "append_issue_comment",
                "Append a comment to an existing issue by immutable issue_id using the configured Tines token. Repeating the call can create another comment. If the outcome is uncertain, inspect the issue before deciding whether to append again.",
                result_schema(),
            ).with_annotations(ToolAnnotations::new().read_only(false).destructive(false).idempotent(false).open_world(true)));
        }
        definitions
    }
    async fn append_issue_comment(
        &self,
        args: AppendIssueCommentArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        let write = WriteAttempt::default();
        if let Err(error) = args.validate() {
            return write.error(error).to_mcp_result();
        }
        let Some(tines) = &self.tines else {
            return write.error(TinesError::NotConfigured).to_mcp_result();
        };
        let attempt = write.clone();
        let result = tines
            .run_tool(cancellation, move |call| async move {
                let links = issue_child_link(&call, &args.issue_id)
                    .await?
                    .into_iter()
                    .collect();
                let (data, status) = call
                    .post_json(
                        Route::IssueComments {
                            issue_id: &args.issue_id,
                        },
                        &json!({"body": args.body}),
                        &attempt,
                    )
                    .await?;
                let comment_id = required_comment_id(&data, status)?.to_owned();
                if data.get("issue_id").and_then(Value::as_str) != Some(args.issue_id.as_str()) {
                    return Err(schema_error(
                        status,
                        "Tines comment receipt did not match the requested issue.",
                    ));
                }
                attempt.created(comment_id.clone());
                Ok((CommentData::new(data).with_links(links), comment_id))
            })
            .await;
        match result {
            Ok((result, comment_id)) => ResultEnvelope::new(result.data)
                .with_links(result.links)
                .with_write_receipt(comment_id)
                .to_mcp_result(),
            Err(error) => write.error(error).to_mcp_result(),
        }
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
        } else if request.name == "append_issue_comment" {
            self.append_issue_comment(parse_args(&request)?, context.ct)
                .await
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
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AppendIssueCommentArgs {
    /// Immutable issue ID; project names and issue numbers are not accepted.
    issue_id: String,
    /// Comment text, preserved verbatim. Nonblank, at most 100,000 UTF-16 code units.
    body: String,
}

impl AppendIssueCommentArgs {
    fn validate(&self) -> Result<(), TinesError> {
        if self.issue_id.trim().is_empty() || matches!(self.issue_id.as_str(), "." | "..") {
            return Err(TinesError::Validation {
                field: "issue_id",
                message: "must be a non-empty issue ID and valid path segment",
            });
        }
        // ECMAScript String.trim uses WhiteSpace and LineTerminator, including
        // FEFF but excluding U+0085 (which Rust's str::trim treats as whitespace).
        let blank = self.body.chars().all(|c| {
            matches!(c,
                '\u{0009}'..='\u{000D}' | '\u{0020}' | '\u{00A0}' | '\u{1680}' |
                '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' |
                '\u{205F}' | '\u{3000}' | '\u{FEFF}'
            )
        });
        if blank {
            return Err(TinesError::Validation {
                field: "body",
                message: "must contain non-whitespace text",
            });
        }
        if self.body.encode_utf16().count() > 100_000 {
            return Err(TinesError::Validation {
                field: "body",
                message: "must be at most 100,000 UTF-16 code units",
            });
        }
        Ok(())
    }
}

struct CommentData {
    data: Value,
    links: Vec<Value>,
}
impl CommentData {
    fn new(data: Value) -> Self {
        Self {
            data,
            links: Vec::new(),
        }
    }
    fn with_links(mut self, links: Vec<Value>) -> Self {
        self.links = links;
        self
    }
}
fn issue_link(
    call: &TinesToolCall,
    project_id: &str,
    number: u64,
    project_name: Option<&str>,
) -> Option<Value> {
    let number_segment = number.to_string();
    let href = call
        .browser_link(&["issues", project_id, &number_segment])
        .ok()?;
    let label = project_name.map_or_else(
        || format!("Open issue {number}"),
        |project| format!("Open issue {project}/{number}"),
    );
    Some(json!({"rel": "issue", "href": href, "label": label}))
}

fn tool<T: JsonSchema + std::any::Any>(
    name: &'static str,
    description: &'static str,
    output: Arc<Map<String, Value>>,
) -> Tool {
    Tool::new(name, description, schema_for_type::<T>()).with_raw_output_schema(output)
}

fn result_schema() -> Arc<Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "data": {},
                "meta": {
                    "type": "object",
                    "properties": {
                        "observed_at_ms": {"type": "integer"},
                        "links": {"type": "array", "items": {"type": "object"}},
                        "omissions": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["observed_at_ms", "links", "omissions"]
                }
            },
            "required": ["data", "meta"]
        })
        .as_object()
        .expect("result schema is an object")
        .clone(),
    )
}

fn parse_args<T: DeserializeOwned>(request: &CallToolRequestParams) -> Result<T, rmcp::ErrorData> {
    let value = Value::Object(request.arguments.clone().unwrap_or_default());
    serde_json::from_value(value)
        .map_err(|error| rmcp::ErrorData::invalid_params(error.to_string(), None))
}

fn schema_error(status: u16, message: &'static str) -> TinesError {
    TinesError::Schema {
        status: Some(status),
        code: "invalid_upstream_shape",
        message,
    }
}

fn required_comment_id(data: &Value, status: u16) -> Result<&str, TinesError> {
    data.get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            schema_error(
                status,
                "Tines comment receipt was missing its immutable ID.",
            )
        })
}

fn issue_project_id(data: &Value, status: u16) -> Result<&str, TinesError> {
    if let Some(project_id) = data.get("project_id").and_then(Value::as_str) {
        return (!project_id.is_empty())
            .then_some(project_id)
            .ok_or_else(|| schema_error(status, "Tines issue response had an empty project ID."));
    }
    if let Some(project_id) = data
        .get("project")
        .and_then(|project| project.get("id"))
        .and_then(Value::as_str)
    {
        return (!project_id.is_empty())
            .then_some(project_id)
            .ok_or_else(|| {
                schema_error(
                    status,
                    "Tines shared issue response had an empty project ID.",
                )
            });
    }
    Err(schema_error(
        status,
        "Tines issue response was missing its current project ID.",
    ))
}

async fn issue_child_link(
    call: &TinesToolCall,
    issue_id: &str,
) -> Result<Option<Value>, TinesError> {
    let (issue, status) = call
        .get_json_with_status(Route::Issue { issue_id }, &Query::new())
        .await?;
    let returned_id = issue
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            schema_error(status, "Tines issue response was missing its immutable ID.")
        })?;
    if returned_id != issue_id {
        return Err(schema_error(
            status,
            "Tines returned a different issue than requested.",
        ));
    }
    let project_id = match issue_project_id(&issue, status) {
        Ok(project_id) => project_id,
        Err(_) => return Ok(None),
    };
    let number = issue.get("number").and_then(Value::as_u64);
    let project_name = issue
        .get("project_name")
        .and_then(Value::as_str)
        .or_else(|| issue.pointer("/project/name").and_then(Value::as_str));
    Ok(number.and_then(|number| issue_link(call, project_id, number, project_name)))
}

#[cfg(test)]
mod comment_tests {
    use super::AppendIssueCommentArgs;

    #[test]
    fn comment_body_limit_counts_utf16_units_without_changing_the_body() {
        for body in ["x".repeat(100_000), "🦀".repeat(50_000)] {
            let args = AppendIssueCommentArgs {
                issue_id: "iss-a".to_owned(),
                body: body.clone(),
            };
            assert!(args.validate().is_ok());
            assert_eq!(args.body, body);
            let too_long = AppendIssueCommentArgs {
                issue_id: "iss-a".to_owned(),
                body: format!("{body}x"),
            };
            assert!(too_long.validate().is_err());
        }
    }
}
