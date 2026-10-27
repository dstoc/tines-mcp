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

use crate::tines::{Query, ResultEnvelope, Route, TinesClient, TinesError, TinesToolCall};

const MAX_PAGE_SIZE: u32 = 100;
const MAX_CURSOR_CHARS: usize = 4096;

#[derive(Clone)]
pub struct ReadTools {
    tines: Option<TinesClient>,
}

impl ReadTools {
    pub fn new(tines: Option<TinesClient>) -> Self {
        Self { tines }
    }

    fn tool_definitions() -> Vec<Tool> {
        let annotations = || {
            ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
                .idempotent(true)
                .open_world(true)
        };
        let output_schema = result_schema();
        let get_issue_schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "issue_id": {
                    "type": "string",
                    "description": "Immutable issue ID. Use this instead of a project and number when available."
                },
                "project_id": {
                    "type": "string",
                    "description": "Immutable project ID for a human-facing issue-number lookup."
                },
                "number": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Positive per-project issue number; required with project_id."
                }
            },
            "oneOf": [
                {
                    "required": ["issue_id"],
                    "not": {"anyOf": [
                        {"required": ["project_id"]},
                        {"required": ["number"]}
                    ]}
                },
                {
                    "required": ["project_id", "number"],
                    "not": {"required": ["issue_id"]}
                }
            ]
        })
        .as_object()
        .expect("get_issue schema is an object")
        .clone();

        vec![
            tool::<ListProjectsArgs>(
                "list_projects",
                "List visible projects. Archived projects are hidden by default; choose archived=true, false, or all to change that filter.",
                output_schema.clone(),
            )
            .with_annotations(annotations()),
            tool::<GetProjectArgs>(
                "get_project",
                "Read one visible project by its immutable project ID. Owner and shared-member projections retain their native field differences.",
                output_schema.clone(),
            )
            .with_annotations(annotations()),
            Tool::new(
                "list_issues",
                "List one native page from the global issue list. Supports project, workflow, state, category, schedule, labels, search, readiness, done, duplicate, archived, and pagination filters. Descriptions are omitted by default; keep filters unchanged when continuing with next_cursor.",
                schema_for_type::<ListIssuesArgs>(),
            )
            .with_annotations(annotations())
            .with_raw_output_schema(output_schema.clone()),
            Tool::new(
                "get_issue",
                "Read one issue by issue_id, or by project_id and positive per-project number. Use issue_id when available; transferred issues are checked against their current project.",
                Arc::new(get_issue_schema),
            )
            .with_annotations(annotations())
            .with_raw_output_schema(output_schema.clone()),
            Tool::new(
                "list_workflows",
                "List visible workflows, optionally using a project ID to ask for the workflow library available to that project. Workflow definitions and issue states remain distinct.",
                schema_for_type::<ListWorkflowsArgs>(),
            )
            .with_annotations(annotations())
            .with_raw_output_schema(output_schema.clone()),
            Tool::new(
                "get_workflow",
                "Read one workflow definition by its immutable workflow ID, including its states, transitions, and native revision fields.",
                schema_for_type::<GetWorkflowArgs>(),
            )
            .with_annotations(annotations())
            .with_raw_output_schema(output_schema.clone()),
            Tool::new(
                "list_labels",
                "List the complete workspace label vocabulary and native usage counts.",
                schema_for_type::<EmptyArgs>(),
            )
            .with_annotations(annotations())
            .with_raw_output_schema(output_schema.clone()),
            tool::<ListEventsArgs>(
                "list_events",
                "Read one live page of existing Tines activity events. Supports project, issue, event type, state, inclusive since, exclusive until, and native cursor filters. Keep filters unchanged when continuing; the cursor is not a durable replay checkpoint.",
                output_schema,
            )
            .with_annotations(annotations()),
        ]
    }

    async fn execute<F, Fut>(
        &self,
        cancellation: CancellationToken,
        operation: F,
    ) -> rmcp::model::CallToolResult
    where
        F: FnOnce(TinesToolCall) -> Fut,
        Fut: Future<Output = Result<ReadData, TinesError>>,
    {
        let Some(tines) = &self.tines else {
            return TinesError::NotConfigured.to_mcp_result();
        };
        match tines.run_tool(cancellation, operation).await {
            Ok(result) => ResultEnvelope::new(result.data)
                .with_links(result.links)
                .with_omissions(result.omissions)
                .to_mcp_result(),
            Err(error) => error.to_mcp_result(),
        }
    }

    async fn list_projects(
        &self,
        args: ListProjectsArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        self.execute(cancellation, move |call| async move {
            let mut query = Query::new();
            if let Some(archived) = args.archived {
                query.append("archived", archived.as_str());
            }
            let (mut data, status) = call.get_json_with_status(Route::Projects, &query).await?;
            let items = collection_items_mut(&mut data, status, "projects")?;
            if call.has_project_exclusions() {
                retain_visible_project_items(items, &call, status)?;
            }
            let links = items
                .iter()
                .filter_map(|item| {
                    item.get("id").and_then(Value::as_str).and_then(|id| {
                        project_link(&call, id, item.get("name").and_then(Value::as_str))
                    })
                })
                .collect();
            normalize_unpaginated_cursor(&mut data, status)?;
            Ok(ReadData::new(data).with_links(links))
        })
        .await
    }

    async fn get_project(
        &self,
        args: GetProjectArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        if args.project_id.trim().is_empty() {
            return validation("project_id", "must be a non-empty project ID");
        }
        self.execute(cancellation, move |call| async move {
            reject_excluded(&call, &args.project_id, "project")?;
            let (data, status) = call
                .get_json_with_status(
                    Route::Project {
                        project_id: &args.project_id,
                    },
                    &Query::new(),
                )
                .await?;
            let id = required_string(&data, "id", status, "project")?;
            if id != args.project_id {
                return Err(schema_error(
                    status,
                    "Tines returned a different project than requested.",
                ));
            }
            reject_excluded(&call, id, "project")?;
            let links = project_link(&call, id, data.get("name").and_then(Value::as_str))
                .into_iter()
                .collect();
            Ok(ReadData::new(data).with_links(links))
        })
        .await
    }

    async fn list_issues(
        &self,
        args: ListIssuesArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        if let Some(project_id) = args.project_id.as_deref()
            && project_id.trim().is_empty()
        {
            return validation("project_id", "must be a non-empty project ID");
        }
        if args
            .limit
            .is_some_and(|limit| !(1..=MAX_PAGE_SIZE).contains(&limit))
        {
            return validation("limit", "must be between 1 and 100");
        }
        if args
            .cursor
            .as_deref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.chars().count() > MAX_CURSOR_CHARS)
        {
            return validation(
                "cursor",
                "must be non-empty and no longer than 4096 characters",
            );
        }

        self.execute(cancellation, move |call| async move {
            if let Some(project_id) = args.project_id.as_deref() {
                reject_excluded(&call, project_id, "project")?;
            }
            let mut query = Query::new();
            if let Some(project_id) = args.project_id.as_deref() {
                query.append("project", project_id);
            }
            append_optional(&mut query, "workflow", args.workflow_id.as_deref());
            append_optional(&mut query, "state", args.state.as_deref());
            if let Some(category) = args.category {
                query.append("category", category.as_str());
            }
            append_optional(&mut query, "schedule", args.schedule_id.as_deref());
            for label in &args.labels {
                query.append("label", label);
            }
            append_optional(&mut query, "q", args.search.as_deref());
            if let Some(ready) = args.ready {
                query.append("ready", ready);
            }
            if let Some(hide_done) = args.hide_done {
                query.append("hide_done", hide_done);
            }
            if let Some(hide_duplicates) = args.hide_duplicates {
                query.append("hide_duplicates", hide_duplicates);
            }
            if let Some(archived) = args.archived {
                query.append("archived", archived.as_str());
            }
            query.append("brief", if args.descriptions { "0" } else { "1" });
            if let Some(limit) = args.limit {
                query.append("limit", limit);
            }
            append_optional(&mut query, "cursor", args.cursor.as_deref());

            let (mut data, status) = call.get_json_with_status(Route::Issues, &query).await?;
            let items = collection_items_mut(&mut data, status, "issues")?;
            if call.has_project_exclusions() {
                retain_visible_issue_items(items, &call, status)?;
            }
            let links = items
                .iter()
                .filter_map(|item| {
                    let project_id = item.get("project_id").and_then(Value::as_str)?;
                    let number = item.get("number").and_then(Value::as_u64)?;
                    issue_link(
                        &call,
                        project_id,
                        number,
                        item.get("project_name").and_then(Value::as_str),
                    )
                })
                .collect();
            let next_cursor = required_cursor(&data, status)?;
            if args.cursor.as_deref().is_some_and(|cursor| next_cursor == Some(cursor)) {
                return Err(TinesError::Schema {
                    status: Some(status),
                    code: "repeated_cursor",
                    message: "Tines returned the same continuation cursor for this issue page.",
                });
            }
            let omissions = if args.descriptions {
                Vec::new()
            } else {
                vec!["Issue descriptions were omitted by the native brief list mode; call get_issue or set descriptions=true to include them.".to_owned()]
            };
            Ok(ReadData {
                data,
                links,
                omissions,
            })
        })
        .await
    }

    async fn get_issue(
        &self,
        args: GetIssueArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        let lookup = match args.lookup() {
            Ok(lookup) => lookup,
            Err(error) => return error.to_mcp_result(),
        };
        self.execute(cancellation, move |call| async move {
            let (data, status) = match lookup {
                IssueLookup::Id(issue_id) => {
                    let (data, status) = call
                        .get_json_with_status(
                            Route::Issue {
                                issue_id: &issue_id,
                            },
                            &Query::new(),
                        )
                        .await?;
                    let returned_id = required_string(&data, "id", status, "issue")?;
                    if returned_id != issue_id {
                        return Err(schema_error(
                            status,
                            "Tines returned a different issue than requested.",
                        ));
                    }
                    (data, status)
                }
                IssueLookup::ProjectNumber { project_id, number } => {
                    reject_excluded(&call, &project_id, "project")?;
                    call.get_json_with_status(
                        Route::ProjectIssue {
                            project_id: &project_id,
                            number,
                        },
                        &Query::new(),
                    )
                    .await?
                }
            };
            let current_project_id = issue_project_id(&data, status)?;
            reject_excluded(&call, current_project_id, "issue")?;
            let number = data.get("number").and_then(Value::as_u64);
            let project_name = data
                .get("project_name")
                .and_then(Value::as_str)
                .or_else(|| data.pointer("/project/name").and_then(Value::as_str));
            let links = number
                .and_then(|number| issue_link(&call, current_project_id, number, project_name))
                .into_iter()
                .collect();
            Ok(ReadData::new(data).with_links(links))
        })
        .await
    }

    async fn list_workflows(
        &self,
        args: ListWorkflowsArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        if let Some(project_id) = args.project_id.as_deref()
            && project_id.trim().is_empty()
        {
            return validation("project_id", "must be a non-empty project ID");
        }
        self.execute(cancellation, move |call| async move {
            let mut query = Query::new();
            if let Some(project_id) = args.project_id.as_deref() {
                reject_excluded(&call, project_id, "project")?;
                query.append("project", project_id);
            }
            let (mut data, status) = call.get_json_with_status(Route::Workflows, &query).await?;
            let items = collection_items_mut(&mut data, status, "workflows")?;
            let links = items
                .iter()
                .filter_map(|item| {
                    item.get("id").and_then(Value::as_str).and_then(|id| {
                        workflow_link(&call, id, item.get("name").and_then(Value::as_str))
                    })
                })
                .collect();
            normalize_unpaginated_cursor(&mut data, status)?;
            Ok(ReadData::new(data).with_links(links))
        })
        .await
    }

    async fn get_workflow(
        &self,
        args: GetWorkflowArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        if args.workflow_id.trim().is_empty() {
            return validation("workflow_id", "must be a non-empty workflow ID");
        }
        self.execute(cancellation, move |call| async move {
            let (data, status) = call
                .get_json_with_status(
                    Route::Workflow {
                        workflow_id: &args.workflow_id,
                    },
                    &Query::new(),
                )
                .await?;
            let id = required_string(&data, "id", status, "workflow")?;
            if id != args.workflow_id {
                return Err(schema_error(
                    status,
                    "Tines returned a different workflow than requested.",
                ));
            }
            let links = workflow_link(&call, id, data.get("name").and_then(Value::as_str))
                .into_iter()
                .collect();
            Ok(ReadData::new(data).with_links(links))
        })
        .await
    }

    async fn list_labels(
        &self,
        _args: EmptyArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        self.execute(cancellation, move |call| async move {
            let (mut data, status) = call
                .get_json_with_status(Route::Labels, &Query::new())
                .await?;
            collection_items_mut(&mut data, status, "labels")?;
            normalize_unpaginated_cursor(&mut data, status)?;
            Ok(ReadData::new(data))
        })
        .await
    }

    async fn list_events(
        &self,
        args: ListEventsArgs,
        cancellation: CancellationToken,
    ) -> rmcp::model::CallToolResult {
        if args
            .project_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return validation("project_id", "must be a non-empty project ID");
        }
        if args
            .issue_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return validation("issue_id", "must be a non-empty issue ID");
        }
        if args
            .event_types
            .iter()
            .any(|event_type| event_type.trim().is_empty())
        {
            return validation("event_types", "must contain non-empty event type strings");
        }
        if args
            .state
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return validation("state", "must be a non-empty state ID or name");
        }
        if args.since.as_ref().is_some_and(TimeBound::is_empty)
            || args.until.as_ref().is_some_and(TimeBound::is_empty)
        {
            return validation(
                "time_bound",
                "must be epoch milliseconds or a non-empty ISO 8601 timestamp",
            );
        }
        if args
            .limit
            .is_some_and(|limit| !(1..=MAX_PAGE_SIZE).contains(&limit))
        {
            return validation("limit", "must be between 1 and 100");
        }
        if args
            .cursor
            .as_deref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.chars().count() > MAX_CURSOR_CHARS)
        {
            return validation(
                "cursor",
                "must be non-empty and no longer than 4096 characters",
            );
        }

        self.execute(cancellation, move |call| async move {
            let mut query = Query::new();
            if let Some(project_id) = args.project_id.as_deref() {
                reject_excluded(&call, project_id, "project")?;
                query.append("project", project_id);
            }
            if let Some(issue_id) = args.issue_id.as_deref() {
                if call.has_project_exclusions() {
                    let project_id = current_issue_project_id(&call, issue_id).await?;
                    reject_excluded(&call, &project_id, "issue")?;
                }
                query.append("issue", issue_id);
            }
            if !args.event_types.is_empty() {
                query.append("type", args.event_types.join(","));
            }
            append_optional(&mut query, "state", args.state.as_deref());
            if let Some(since) = args.since.as_ref() {
                query.append("since", since.as_query_value());
            }
            if let Some(until) = args.until.as_ref() {
                query.append("until", until.as_query_value());
            }
            if let Some(limit) = args.limit {
                query.append("limit", limit);
            }
            append_optional(&mut query, "cursor", args.cursor.as_deref());

            let (mut data, status) = call.get_json_with_status(Route::Events, &query).await?;
            let items = collection_items_mut(&mut data, status, "events")?;
            if call.has_project_exclusions() {
                retain_visible_event_items(items, &call, status).await?;
            }
            let next_cursor = required_cursor(&data, status)?;
            if args
                .cursor
                .as_deref()
                .is_some_and(|cursor| next_cursor == Some(cursor))
            {
                return Err(TinesError::Schema {
                    status: Some(status),
                    code: "repeated_cursor",
                    message: "Tines returned the same continuation cursor for this event page.",
                });
            }
            Ok(ReadData::new(data))
        })
        .await
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
        std::future::ready(Ok(
            ListToolsResult::with_all_items(Self::tool_definitions()),
        ))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Self::tool_definitions()
            .into_iter()
            .find(|tool| tool.name.as_ref() == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let cancellation = context.ct;
        let result = match request.name.as_ref() {
            "list_projects" => {
                self.list_projects(parse_args(&request)?, cancellation)
                    .await
            }
            "get_project" => self.get_project(parse_args(&request)?, cancellation).await,
            "list_issues" => self.list_issues(parse_args(&request)?, cancellation).await,
            "get_issue" => {
                if let Some(field) =
                    ["issue_id", "project_id", "number"]
                        .into_iter()
                        .find(|field| {
                            request
                                .arguments
                                .as_ref()
                                .and_then(|arguments| arguments.get(*field))
                                .is_some_and(Value::is_null)
                        })
                {
                    return Ok(validation(field, "must be omitted instead of set to null").into());
                }
                self.get_issue(parse_args(&request)?, cancellation).await
            }
            "list_workflows" => {
                self.list_workflows(parse_args(&request)?, cancellation)
                    .await
            }
            "get_workflow" => self.get_workflow(parse_args(&request)?, cancellation).await,
            "list_labels" => self.list_labels(parse_args(&request)?, cancellation).await,
            "list_events" => self.list_events(parse_args(&request)?, cancellation).await,
            _ => return Err(rmcp::ErrorData::method_not_found::<CallToolRequestMethod>()),
        };
        Ok(result.into())
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListProjectsArgs {
    /// false (default) hides archived projects, true shows only archived projects, and all shows both.
    archived: Option<ArchivedFilter>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetProjectArgs {
    /// Immutable Tines project ID.
    project_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListIssuesArgs {
    /// Restrict to one project by immutable ID.
    project_id: Option<String>,
    /// Filter by workflow ID or an accessible workflow name.
    workflow_id: Option<String>,
    /// Filter by effective state ID or name.
    state: Option<String>,
    /// Filter by the effective state's category.
    category: Option<IssueCategory>,
    /// Filter by scheduled task ID.
    schedule_id: Option<String>,
    /// Every supplied label ID or name must be present.
    #[serde(default)]
    labels: Vec<String>,
    /// Literal title and description substring search.
    search: Option<String>,
    /// Return issues whose blockers are all effectively done.
    ready: Option<bool>,
    /// Hide issues in done states.
    hide_done: Option<bool>,
    /// Hide duplicate issues. The upstream default is true.
    hide_duplicates: Option<bool>,
    /// false (default) hides archived projects, true shows only archived projects, and all shows both. A selected project is listed regardless of archive status.
    archived: Option<ArchivedFilter>,
    /// Include description bodies in list items. Defaults to false.
    #[serde(default)]
    descriptions: bool,
    /// Native page size from 1 through 100. The Tines default is 50.
    limit: Option<u32>,
    /// Opaque native cursor from the previous page; retain all filters when continuing.
    cursor: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetIssueArgs {
    /// Immutable issue ID. Use this instead of a project and number when available.
    issue_id: Option<String>,
    /// Immutable project ID for a human-facing issue-number lookup.
    project_id: Option<String>,
    /// Positive per-project issue number; required with project_id.
    number: Option<u64>,
}

impl GetIssueArgs {
    fn lookup(self) -> Result<IssueLookup, TinesError> {
        match (self.issue_id, self.project_id, self.number) {
            (Some(issue_id), None, None) if !issue_id.trim().is_empty() => {
                Ok(IssueLookup::Id(issue_id))
            }
            (None, Some(project_id), Some(number))
                if !project_id.trim().is_empty() && number > 0 =>
            {
                Ok(IssueLookup::ProjectNumber { project_id, number })
            }
            (Some(_), None, None) => Err(TinesError::Validation {
                field: "issue_id",
                message: "must be a non-empty issue ID",
            }),
            (None, Some(_), Some(0)) => Err(TinesError::Validation {
                field: "number",
                message: "must be a positive issue number",
            }),
            _ => Err(TinesError::Validation {
                field: "lookup",
                message: "provide issue_id alone, or provide project_id and a positive number",
            }),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListWorkflowsArgs {
    /// Return the workflow library available to this immutable project ID.
    project_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetWorkflowArgs {
    /// Immutable Tines workflow ID.
    workflow_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListEventsArgs {
    /// Restrict to one project by immutable ID.
    project_id: Option<String>,
    /// Restrict to one issue by immutable ID.
    issue_id: Option<String>,
    /// Event type strings to include. This list is open-ended as Tines adds event types.
    #[serde(default)]
    event_types: Vec<String>,
    /// Workflow state ID or name referenced by an event payload.
    state: Option<String>,
    /// Inclusive lower time bound as epoch milliseconds or an ISO 8601 timestamp.
    since: Option<TimeBound>,
    /// Exclusive upper time bound as epoch milliseconds or an ISO 8601 timestamp.
    until: Option<TimeBound>,
    /// Native page size from 1 through 100. The Tines default is 50.
    limit: Option<u32>,
    /// Opaque native cursor from the previous page; retain all filters when continuing.
    cursor: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum TimeBound {
    EpochMilliseconds(u64),
    Iso8601(String),
}

impl TimeBound {
    fn is_empty(&self) -> bool {
        matches!(self, Self::Iso8601(value) if value.trim().is_empty())
    }

    fn as_query_value(&self) -> String {
        match self {
            Self::EpochMilliseconds(value) => value.to_string(),
            Self::Iso8601(value) => value.clone(),
        }
    }
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ArchivedFilter {
    True,
    False,
    All,
}

impl ArchivedFilter {
    fn as_str(&self) -> &'static str {
        match self {
            Self::True => "true",
            Self::False => "false",
            Self::All => "all",
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum IssueCategory {
    Backlog,
    Active,
    AwaitingHuman,
    Done,
}

impl IssueCategory {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Active => "active",
            Self::AwaitingHuman => "awaiting_human",
            Self::Done => "done",
        }
    }
}

enum IssueLookup {
    Id(String),
    ProjectNumber { project_id: String, number: u64 },
}

struct ReadData {
    data: Value,
    links: Vec<Value>,
    omissions: Vec<String>,
}

impl ReadData {
    fn new(data: Value) -> Self {
        Self {
            data,
            links: Vec::new(),
            omissions: Vec::new(),
        }
    }

    fn with_links(mut self, links: Vec<Value>) -> Self {
        self.links = links;
        self
    }
}

fn project_link(call: &TinesToolCall, project_id: &str, name: Option<&str>) -> Option<Value> {
    let href = call.browser_link(&["projects", project_id]).ok()?;
    Some(json!({
        "rel": "project",
        "href": href,
        "label": name.map_or_else(|| "Open project".to_owned(), |name| format!("Open project {name}"))
    }))
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

fn workflow_link(call: &TinesToolCall, workflow_id: &str, name: Option<&str>) -> Option<Value> {
    let href = call.browser_link(&["workflows", workflow_id]).ok()?;
    Some(json!({
        "rel": "workflow",
        "href": href,
        "label": name.map_or_else(|| "Open workflow".to_owned(), |name| format!("Open workflow {name}"))
    }))
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

fn append_optional(query: &mut Query, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        query.append(name, value);
    }
}

fn validation(field: &'static str, message: &'static str) -> rmcp::model::CallToolResult {
    TinesError::Validation { field, message }.to_mcp_result()
}

fn schema_error(status: u16, message: &'static str) -> TinesError {
    TinesError::Schema {
        status: Some(status),
        code: "invalid_upstream_shape",
        message,
    }
}

fn collection_items_mut<'a>(
    data: &'a mut Value,
    status: u16,
    resource: &'static str,
) -> Result<&'a mut Vec<Value>, TinesError> {
    let object = data.as_object_mut().ok_or_else(|| {
        schema_error(
            status,
            "Tines returned a collection response that was not an object.",
        )
    })?;
    object
        .get_mut("items")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| match resource {
            "projects" => {
                schema_error(status, "Tines project response was missing an items array.")
            }
            "issues" => schema_error(status, "Tines issue response was missing an items array."),
            "workflows" => schema_error(
                status,
                "Tines workflow response was missing an items array.",
            ),
            "events" => schema_error(status, "Tines event response was missing an items array."),
            _ => schema_error(status, "Tines label response was missing an items array."),
        })
}

fn normalize_unpaginated_cursor(data: &mut Value, status: u16) -> Result<(), TinesError> {
    let object = data.as_object_mut().ok_or_else(|| {
        schema_error(
            status,
            "Tines returned a collection response that was not an object.",
        )
    })?;
    match object.get("next_cursor") {
        None | Some(Value::Null) => {
            object.insert("next_cursor".to_owned(), Value::Null);
            Ok(())
        }
        Some(_) => Err(schema_error(
            status,
            "Tines returned a cursor for an endpoint that is not paginated.",
        )),
    }
}

async fn retain_visible_event_items(
    items: &mut Vec<Value>,
    call: &TinesToolCall,
    status: u16,
) -> Result<(), TinesError> {
    let mut visible = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let recorded_project = optional_attribution_id(&item, "project_id", status)?;
        let issue_id = optional_attribution_id(&item, "issue_id", status)?;
        if recorded_project.is_some_and(|id| call.is_project_excluded(id)) {
            continue;
        }

        let current_project = if let Some(issue_id) = issue_id {
            match event_issue_project_id(&item, status)? {
                Some(project_id) => Some(project_id.to_owned()),
                None => Some(current_issue_project_id(call, issue_id).await?),
            }
        } else {
            None
        };
        if current_project
            .as_deref()
            .is_some_and(|id| call.is_project_excluded(id))
        {
            continue;
        }
        visible.push(item);
    }
    *items = visible;
    Ok(())
}

fn optional_attribution_id<'a>(
    event: &'a Value,
    field: &'static str,
    status: u16,
) -> Result<Option<&'a str>, TinesError> {
    match event.get(field) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value)),
        _ => Err(schema_error(
            status,
            "Tines event response was missing valid project and issue attribution fields.",
        )),
    }
}

fn event_issue_project_id(event: &Value, status: u16) -> Result<Option<&str>, TinesError> {
    match event.get("issue_ref") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(issue_ref)) => match issue_ref.get("project_id") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(project_id)) if !project_id.is_empty() => Ok(Some(project_id)),
            _ => Err(schema_error(
                status,
                "Tines event issue reference had an invalid current project ID.",
            )),
        },
        _ => Err(schema_error(
            status,
            "Tines event issue reference was neither an object nor null.",
        )),
    }
}

async fn current_issue_project_id(
    call: &TinesToolCall,
    issue_id: &str,
) -> Result<String, TinesError> {
    let (issue, status, should_cache) = match call.cached_parent_issue(issue_id) {
        Some(issue) => (issue, 200, false),
        None => {
            let (issue, status) = call
                .get_json_with_status(Route::Issue { issue_id }, &Query::new())
                .await?;
            (issue, status, true)
        }
    };
    let returned_id = required_string(&issue, "id", status, "issue")?;
    if returned_id != issue_id {
        return Err(schema_error(
            status,
            "Tines returned a different issue during event visibility checks.",
        ));
    }
    let project_id = issue_project_id(&issue, status)?.to_owned();
    if should_cache {
        call.remember_parent_issue(issue_id.to_owned(), issue);
    }
    Ok(project_id)
}

fn required_cursor(data: &Value, status: u16) -> Result<Option<&str>, TinesError> {
    match data.get("next_cursor") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(cursor)) => Ok(Some(cursor)),
        _ => Err(schema_error(
            status,
            "Tines paginated response was missing a valid next_cursor field.",
        )),
    }
}

fn retain_visible_project_items(
    items: &mut Vec<Value>,
    call: &TinesToolCall,
    status: u16,
) -> Result<(), TinesError> {
    let mut visible = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let project_id = required_string(&item, "id", status, "project")?;
        if !call.is_project_excluded(project_id) {
            visible.push(item);
        }
    }
    *items = visible;
    Ok(())
}

fn retain_visible_issue_items(
    items: &mut Vec<Value>,
    call: &TinesToolCall,
    status: u16,
) -> Result<(), TinesError> {
    let mut visible = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let project_id = required_string(&item, "project_id", status, "issue")?;
        if !call.is_project_excluded(project_id) {
            visible.push(item);
        }
    }
    *items = visible;
    Ok(())
}

fn reject_excluded(
    call: &TinesToolCall,
    project_id: &str,
    resource: &'static str,
) -> Result<(), TinesError> {
    if call.is_project_excluded(project_id) {
        Err(TinesError::Visibility { resource })
    } else {
        Ok(())
    }
}

fn required_string<'a>(
    data: &'a Value,
    field: &'static str,
    status: u16,
    resource: &'static str,
) -> Result<&'a str, TinesError> {
    data.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| match resource {
            "project" => schema_error(
                status,
                "Tines project response was missing its immutable ID.",
            ),
            "issue" => schema_error(
                status,
                "Tines issue response was missing its immutable project ID.",
            ),
            _ => schema_error(
                status,
                "Tines workflow response was missing its immutable ID.",
            ),
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
