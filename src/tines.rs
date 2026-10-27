use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    future::Future,
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use reqwest::{
    Client, Response, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use serde_json::{Map, Value, json};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::{Secret, TinesConfig};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOOL_DEADLINE: Duration = Duration::from_secs(45);
const MAX_TOOL_CALLS: usize = 4;
const MAX_UPSTREAM_REQUESTS: usize = 8;
const MAX_JSON_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_ERROR_TEXT_CHARS: usize = 512;
const MAX_ERROR_CODE_CHARS: usize = 128;
const RATE_LIMIT_PER_SECOND: f64 = 10.0;
const RATE_LIMIT_BURST: f64 = 20.0;

/// A fixed set of Tines API route templates. Dynamic values are always encoded
/// as path segments by `url::Url` before the request is sent.
#[derive(Clone, Copy, Debug)]
pub enum Route<'a> {
    Projects,
    Project { project_id: &'a str },
    Issues,
    Issue { issue_id: &'a str },
    ProjectIssue { project_id: &'a str, number: u64 },
    Workflows,
    Workflow { workflow_id: &'a str },
    Labels,
    Context,
    ContextItem { context_id: &'a str },
    IssueContext { issue_id: &'a str },
    IssueJournal { issue_id: &'a str },
    IssueArtifacts { issue_id: &'a str },
    IssueArtifact { issue_id: &'a str, name: &'a str },
    ArtifactContent { issue_id: &'a str, name: &'a str },
    Runs,
    Run { run_id: &'a str },
    RunLog { run_id: &'a str },
    IssueDispatch { issue_id: &'a str },
    Events,
}

impl Route<'_> {
    fn template(self) -> &'static str {
        match self {
            Self::Projects => "/projects",
            Self::Project { .. } => "/projects/{id}",
            Self::Issues => "/issues",
            Self::Issue { .. } => "/issues/{id}",
            Self::ProjectIssue { .. } => "/projects/{id}/issues/{number}",
            Self::Workflows => "/workflows",
            Self::Workflow { .. } => "/workflows/{id}",
            Self::Labels => "/labels",
            Self::Context => "/context",
            Self::ContextItem { .. } => "/context/{id}",
            Self::IssueContext { .. } => "/issues/{id}/context",
            Self::IssueJournal { .. } => "/issues/{id}/journal",
            Self::IssueArtifacts { .. } => "/issues/{id}/artifacts",
            Self::IssueArtifact { .. } => "/issues/{id}/artifacts/{name}",
            Self::ArtifactContent { .. } => "/issues/{id}/artifacts/{name}/content",
            Self::Runs => "/runs",
            Self::Run { .. } => "/runs/{id}",
            Self::RunLog { .. } => "/runs/{id}/log",
            Self::IssueDispatch { .. } => "/issues/{id}/dispatch",
            Self::Events => "/events",
        }
    }

    fn push_segments(self, segments: &mut url::PathSegmentsMut<'_>) -> Result<(), TinesError> {
        match self {
            Self::Projects => {
                segments.push("projects");
            }
            Self::Project { project_id } => {
                segments.push("projects");
                push_dynamic_segment(segments, project_id)?;
            }
            Self::Issues => {
                segments.push("issues");
            }
            Self::Issue { issue_id } => {
                segments.push("issues");
                push_dynamic_segment(segments, issue_id)?;
            }
            Self::ProjectIssue { project_id, number } => {
                segments.push("projects");
                push_dynamic_segment(segments, project_id)?;
                segments.push("issues").push(&number.to_string());
            }
            Self::Workflows => {
                segments.push("workflows");
            }
            Self::Workflow { workflow_id } => {
                segments.push("workflows");
                push_dynamic_segment(segments, workflow_id)?;
            }
            Self::Labels => {
                segments.push("labels");
            }
            Self::Context => {
                segments.push("context");
            }
            Self::ContextItem { context_id } => {
                segments.push("context");
                push_dynamic_segment(segments, context_id)?;
            }
            Self::IssueContext { issue_id } => {
                segments.push("issues");
                push_dynamic_segment(segments, issue_id)?;
                segments.push("context");
            }
            Self::IssueJournal { issue_id } => {
                segments.push("issues");
                push_dynamic_segment(segments, issue_id)?;
                segments.push("journal");
            }
            Self::IssueArtifacts { issue_id } => {
                segments.push("issues");
                push_dynamic_segment(segments, issue_id)?;
                segments.push("artifacts");
            }
            Self::IssueArtifact { issue_id, name } => {
                segments.push("issues");
                push_dynamic_segment(segments, issue_id)?;
                segments.push("artifacts");
                push_dynamic_segment(segments, name)?;
            }
            Self::ArtifactContent { issue_id, name } => {
                segments.push("issues");
                push_dynamic_segment(segments, issue_id)?;
                segments.push("artifacts");
                push_dynamic_segment(segments, name)?;
                segments.push("content");
            }
            Self::Runs => {
                segments.push("runs");
            }
            Self::Run { run_id } => {
                segments.push("runs");
                push_dynamic_segment(segments, run_id)?;
            }
            Self::RunLog { run_id } => {
                segments.push("runs");
                push_dynamic_segment(segments, run_id)?;
                segments.push("log");
            }
            Self::IssueDispatch { issue_id } => {
                segments.push("issues");
                push_dynamic_segment(segments, issue_id)?;
                segments.push("dispatch");
            }
            Self::Events => {
                segments.push("events");
            }
        }
        Ok(())
    }
}

fn push_dynamic_segment(
    segments: &mut url::PathSegmentsMut<'_>,
    value: &str,
) -> Result<(), TinesError> {
    if value.is_empty() || value == "." || value == ".." {
        return Err(TinesError::InvalidRoute);
    }
    segments.push(value);
    Ok(())
}

/// Encoded query pairs for an explicitly selected route.
#[derive(Clone, Debug, Default)]
pub struct Query {
    pairs: Vec<(String, String)>,
}

impl Query {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&mut self, name: impl Into<String>, value: impl ToString) -> &mut Self {
        self.pairs.push((name.into(), value.to_string()));
        self
    }
}

#[derive(Clone)]
pub struct TinesClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    http: Client,
    base_url: Url,
    web_url: Url,
    authorization: HeaderValue,
    api_token: Secret,
    excluded_project_ids: HashSet<String>,
    tool_slots: Arc<Semaphore>,
    request_slots: Arc<Semaphore>,
    rate_limit: RateLimit,
    tool_deadline: Duration,
    next_diagnostic_id: AtomicU64,
}

struct RateLimit {
    bucket: Mutex<TokenBucket>,
}

struct TokenBucket {
    tokens: f64,
    last_updated: Instant,
}

impl RateLimit {
    fn new() -> Self {
        Self {
            bucket: Mutex::new(TokenBucket {
                tokens: RATE_LIMIT_BURST,
                last_updated: Instant::now(),
            }),
        }
    }

    fn admit(&self) -> bool {
        let now = Instant::now();
        let mut bucket = self
            .bucket
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let elapsed = now.duration_since(bucket.last_updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * RATE_LIMIT_PER_SECOND).min(RATE_LIMIT_BURST);
        bucket.last_updated = now;
        if bucket.tokens < 1.0 {
            false
        } else {
            bucket.tokens -= 1.0;
            true
        }
    }
}

impl TinesClient {
    pub fn new(config: &TinesConfig) -> Result<Self, TinesClientBuildError> {
        Self::with_deadline(config, TOOL_DEADLINE)
    }

    fn with_deadline(
        config: &TinesConfig,
        tool_deadline: Duration,
    ) -> Result<Self, TinesClientBuildError> {
        let authorization_value = format!("Bearer {}", config.api_token.expose());
        let mut authorization =
            HeaderValue::from_str(&authorization_value).map_err(|_| TinesClientBuildError)?;
        authorization.set_sensitive(true);

        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TOOL_DEADLINE)
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| TinesClientBuildError)?;

        Ok(Self {
            inner: Arc::new(ClientInner {
                http,
                base_url: config.base_url.clone(),
                web_url: config.web_url.clone(),
                authorization,
                api_token: config.api_token.clone(),
                excluded_project_ids: config.excluded_project_ids().iter().cloned().collect(),
                tool_slots: Arc::new(Semaphore::new(MAX_TOOL_CALLS)),
                request_slots: Arc::new(Semaphore::new(MAX_UPSTREAM_REQUESTS)),
                rate_limit: RateLimit::new(),
                tool_deadline,
                next_diagnostic_id: AtomicU64::new(1),
            }),
        })
    }

    /// Starts a tool operation without queuing it behind other admitted work.
    /// The deadline covers the closure and every upstream request it performs.
    pub async fn run_tool<T, F, Fut>(
        &self,
        cancellation: CancellationToken,
        operation: F,
    ) -> Result<T, TinesError>
    where
        F: FnOnce(TinesToolCall) -> Fut,
        Fut: Future<Output = Result<T, TinesError>>,
    {
        let permit = self
            .inner
            .tool_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| TinesError::ConcurrentToolLimit)?;
        if !self.inner.rate_limit.admit() {
            return Err(TinesError::RateLimited);
        }

        let deadline = Instant::now() + self.inner.tool_deadline;
        let call = TinesToolCall {
            inner: self.inner.clone(),
            cancellation: cancellation.clone(),
            deadline,
            _tool_permit: permit,
            parent_issues: Mutex::new(HashMap::new()),
        };

        tokio::select! {
            _ = cancellation.cancelled() => Err(TinesError::Cancelled),
            _ = sleep_until(deadline) => Err(TinesError::Timeout),
            result = operation(call) => result,
        }
    }
}

pub struct TinesToolCall {
    inner: Arc<ClientInner>,
    cancellation: CancellationToken,
    deadline: Instant,
    _tool_permit: OwnedSemaphorePermit,
    parent_issues: Mutex<HashMap<String, Value>>,
}

impl TinesToolCall {
    pub fn has_project_exclusions(&self) -> bool {
        !self.inner.excluded_project_ids.is_empty()
    }

    pub async fn get_json(&self, route: Route<'_>, query: &Query) -> Result<Value, TinesError> {
        self.get_json_with_status(route, query)
            .await
            .map(|(value, _)| value)
    }

    /// Fetches one JSON response and keeps its successful HTTP status so later
    /// schema validation can report the upstream response that was inspected.
    pub async fn get_json_with_status(
        &self,
        route: Route<'_>,
        query: &Query,
    ) -> Result<(Value, u16), TinesError> {
        let (body, status, _, diagnostic_id) = self
            .get_body_with_status(route, query, MAX_JSON_BODY_BYTES, "application/json")
            .await?;
        let value = serde_json::from_slice(&body).map_err(|_| TinesError::InvalidJson {
            status,
            diagnostic_id,
        })?;
        Ok((value, status))
    }

    /// Fetches a bounded byte response and returns its successful status and
    /// declared MIME type without interpreting its contents.
    pub async fn get_bytes_with_status(
        &self,
        route: Route<'_>,
        query: &Query,
        limit_bytes: usize,
    ) -> Result<(Vec<u8>, u16, Option<String>), TinesError> {
        self.get_body_with_status(route, query, limit_bytes, "application/octet-stream")
            .await
            .map(|(body, status, content_type, _)| (body, status, content_type))
    }

    async fn get_body_with_status(
        &self,
        route: Route<'_>,
        query: &Query,
        limit_bytes: usize,
        accept: &'static str,
    ) -> Result<(Vec<u8>, u16, Option<String>, u64), TinesError> {
        let request_permit = self
            .inner
            .request_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| TinesError::ConcurrentRequestLimit)?;
        let diagnostic_id = self
            .inner
            .next_diagnostic_id
            .fetch_add(1, Ordering::Relaxed);
        let route_template = route.template();
        let started = Instant::now();
        let url = build_url(&self.inner.base_url, route, query)?;
        let request = self
            .inner
            .http
            .get(url)
            .header(ACCEPT, accept)
            .header(AUTHORIZATION, self.inner.authorization.clone())
            .build()
            .map_err(|_| TinesError::Transport { diagnostic_id })?;

        let inner = self.inner.clone();
        let secret = self.inner.api_token.clone();
        let future = async move {
            let response = inner.http.execute(request).await.map_err(|error| {
                if error.is_timeout() {
                    tracing::debug!(
                        diagnostic_id,
                        route = route_template,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        outcome = "timeout",
                        "Tines GET failed"
                    );
                    TinesError::Timeout
                } else {
                    tracing::debug!(
                        diagnostic_id,
                        route = route_template,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        outcome = "transport",
                        "Tines GET failed"
                    );
                    TinesError::Transport { diagnostic_id }
                }
            })?;
            let status = response.status();
            let retry_after =
                safe_retry_after(response.headers().get(RETRY_AFTER), secret.expose());
            tracing::debug!(
                diagnostic_id,
                route = route_template,
                status = status.as_u16(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Tines GET received response"
            );

            if status.is_success() {
                let content_type = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let body = read_bounded_body(response, limit_bytes)
                    .await
                    .map_err(|error| match error {
                        BodyReadError::TooLarge => TinesError::BodyTooLarge {
                            status: status.as_u16(),
                            limit_bytes,
                            diagnostic_id,
                        },
                        BodyReadError::Transport => TinesError::Transport { diagnostic_id },
                    })?;
                Ok((body, status.as_u16(), content_type, diagnostic_id))
            } else {
                let body = read_bounded_body(response, MAX_ERROR_BODY_BYTES).await;
                Err(decode_upstream_error(
                    status,
                    retry_after,
                    body,
                    secret.expose(),
                    diagnostic_id,
                ))
            }
        };

        let _request_permit = request_permit;
        tokio::select! {
            _ = self.cancellation.cancelled() => Err(TinesError::Cancelled),
            _ = sleep_until(self.deadline) => Err(TinesError::Timeout),
            result = future => result,
        }
    }

    /// Parent issue details may be reused only for this tool call.
    pub fn cached_parent_issue(&self, issue_id: &str) -> Option<Value> {
        self.parent_issues
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(issue_id)
            .cloned()
    }

    pub fn remember_parent_issue(&self, issue_id: String, issue: Value) {
        self.parent_issues
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(issue_id, issue);
    }

    pub fn is_project_excluded(&self, project_id: &str) -> bool {
        self.inner.excluded_project_ids.contains(project_id)
    }

    pub fn browser_link(&self, path_segments: &[&str]) -> Result<String, TinesError> {
        let mut url = self.inner.web_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| TinesError::InvalidRoute)?;
            segments.pop_if_empty();
            for segment in path_segments {
                push_dynamic_segment(&mut segments, segment)?;
            }
        }
        url.set_query(None);
        url.set_fragment(None);
        Ok(url.into())
    }
}

fn build_url(base_url: &Url, route: Route<'_>, query: &Query) -> Result<Url, TinesError> {
    let mut url = base_url.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| TinesError::InvalidRoute)?;
        segments.pop_if_empty().push("api").push("v1");
        route.push_segments(&mut segments)?;
    }
    if !query.pairs.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(query.pairs.iter().map(|(name, value)| (name, value)));
    }
    Ok(url)
}

async fn read_bounded_body(
    mut response: Response,
    limit_bytes: usize,
) -> Result<Vec<u8>, BodyReadError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit_bytes as u64)
    {
        return Err(BodyReadError::TooLarge);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| BodyReadError::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > limit_bytes {
            return Err(BodyReadError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

enum BodyReadError {
    TooLarge,
    Transport,
}

fn decode_upstream_error(
    status: StatusCode,
    retry_after: Option<String>,
    body: Result<Vec<u8>, BodyReadError>,
    secret: &str,
    diagnostic_id: u64,
) -> TinesError {
    let body = match body {
        Ok(body) => body,
        Err(BodyReadError::TooLarge) => {
            return TinesError::Upstream {
                status: status.as_u16(),
                code: "upstream_error".to_owned(),
                message: format!(
                    "Tines returned an error body larger than the {MAX_ERROR_BODY_BYTES}-byte limit."
                ),
                details: None,
                retry_after,
                diagnostic_id,
            };
        }
        Err(BodyReadError::Transport) => {
            return TinesError::Upstream {
                status: status.as_u16(),
                code: "upstream_error".to_owned(),
                message: format!(
                    "Tines returned HTTP {} but its error body could not be read.",
                    status.as_u16()
                ),
                details: None,
                retry_after,
                diagnostic_id,
            };
        }
    };

    let parsed = serde_json::from_slice::<Value>(&body).ok();
    let error = parsed.as_ref().and_then(|value| value.get("error"));
    let code = error
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .map(|value| safe_error_code(value, secret))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "upstream_error".to_owned());
    let message = error
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .map(|value| safe_error_message(value, secret))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            format!(
                "Tines returned HTTP {} with a malformed or non-JSON error response.",
                status.as_u16()
            )
        });
    let details = safe_error_details(error, secret);

    TinesError::Upstream {
        status: status.as_u16(),
        code,
        message,
        details,
        retry_after,
        diagnostic_id,
    }
}

fn safe_error_details(error: Option<&Value>, secret: &str) -> Option<Value> {
    let details = error?.get("details")?.as_object()?;
    let mut safe = Map::new();
    for (key, value) in details {
        match key.as_str() {
            "committed" if value.is_boolean() => {
                safe.insert(key.clone(), value.clone());
            }
            "expected_revision" | "current_revision" | "ceiling" | "limit" if value.is_number() => {
                safe.insert(key.clone(), value.clone());
            }
            "field" | "operation" | "domain" | "access" | "reason"
                if value.as_str().is_some_and(is_safe_detail_identifier) =>
            {
                let value = value.as_str().expect("string checked above");
                safe.insert(key.clone(), Value::String(redact_secret(value, secret)));
            }
            _ => {}
        }
    }
    (!safe.is_empty()).then_some(Value::Object(safe))
}

fn is_safe_detail_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
}

fn safe_error_code(value: &str, secret: &str) -> String {
    let value = redact_secret(value, secret);
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
        .take(MAX_ERROR_CODE_CHARS)
        .collect()
}

fn safe_error_message(value: &str, secret: &str) -> String {
    let value = redact_secret(value, secret);
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_ERROR_TEXT_CHARS)
        .collect()
}

fn redact_secret(value: &str, secret: &str) -> String {
    if secret.is_empty() {
        value.to_owned()
    } else {
        value.replace(secret, "[REDACTED]")
    }
}

fn safe_retry_after(value: Option<&HeaderValue>, secret: &str) -> Option<String> {
    let value = value?.to_str().ok()?;
    if value.len() > 128
        || value.contains(secret)
        || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return None;
    }
    Some(value.to_owned())
}

#[derive(Debug)]
pub struct TinesClientBuildError;

impl fmt::Display for TinesClientBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Tines client configuration is invalid")
    }
}

impl Error for TinesClientBuildError {}

#[derive(Debug)]
pub enum TinesError {
    Validation {
        field: &'static str,
        message: &'static str,
    },
    Visibility {
        resource: &'static str,
    },
    NotConfigured,
    Schema {
        status: Option<u16>,
        code: &'static str,
        message: &'static str,
    },
    Cancelled,
    Timeout,
    Transport {
        diagnostic_id: u64,
    },
    InvalidJson {
        status: u16,
        diagnostic_id: u64,
    },
    InvalidRoute,
    BodyTooLarge {
        status: u16,
        limit_bytes: usize,
        diagnostic_id: u64,
    },
    ContentTooLarge {
        status: u16,
        limit_bytes: usize,
        diagnostic_id: u64,
    },
    ResultTooLarge {
        limit_bytes: usize,
    },
    Upstream {
        status: u16,
        code: String,
        message: String,
        details: Option<Value>,
        retry_after: Option<String>,
        diagnostic_id: u64,
    },
    RateLimited,
    ConcurrentToolLimit,
    ConcurrentRequestLimit,
}

impl fmt::Display for TinesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message())
    }
}

impl Error for TinesError {}

impl TinesError {
    fn fields(
        &self,
    ) -> (
        &'static str,
        String,
        String,
        Option<u16>,
        Option<String>,
        Option<u64>,
    ) {
        match self {
            Self::Validation { field, message } => (
                "validation",
                "invalid_input".to_owned(),
                format!("{field}: {message}"),
                None,
                None,
                None,
            ),
            Self::Visibility { resource } => (
                "visibility",
                "resource_unavailable".to_owned(),
                format!("{resource} is missing or inaccessible."),
                None,
                None,
                None,
            ),
            Self::NotConfigured => (
                "configuration",
                "tines_not_configured".to_owned(),
                "Tines API settings are not configured for this server.".to_owned(),
                None,
                None,
                None,
            ),
            Self::Schema {
                status,
                code,
                message,
            } => (
                "schema",
                (*code).to_owned(),
                (*message).to_owned(),
                *status,
                None,
                None,
            ),
            Self::Cancelled => (
                "cancelled",
                "request_cancelled".to_owned(),
                "The tool call was cancelled.".to_owned(),
                None,
                None,
                None,
            ),
            Self::Timeout => (
                "timeout",
                "upstream_timeout".to_owned(),
                "The tool call exceeded its 45-second deadline.".to_owned(),
                None,
                None,
                None,
            ),
            Self::Transport { diagnostic_id } => (
                "transport",
                "upstream_transport_error".to_owned(),
                "The Tines request failed during transport.".to_owned(),
                None,
                None,
                Some(*diagnostic_id),
            ),
            Self::InvalidJson {
                status,
                diagnostic_id,
            } => (
                "schema",
                "invalid_upstream_json".to_owned(),
                "Tines returned a response that was not valid JSON.".to_owned(),
                Some(*status),
                None,
                Some(*diagnostic_id),
            ),
            Self::InvalidRoute => (
                "validation",
                "invalid_route".to_owned(),
                "The fixed Tines route could not be constructed.".to_owned(),
                None,
                None,
                None,
            ),
            Self::BodyTooLarge {
                status,
                limit_bytes,
                diagnostic_id,
            } => (
                "schema",
                "upstream_body_too_large".to_owned(),
                format!("Tines response exceeded the {limit_bytes}-byte body limit."),
                Some(*status),
                None,
                Some(*diagnostic_id),
            ),
            Self::ContentTooLarge {
                status,
                diagnostic_id,
                ..
            } => (
                "schema",
                "content_source_too_large".to_owned(),
                "Tines content exceeded the 1 MiB source limit.".to_owned(),
                Some(*status),
                None,
                Some(*diagnostic_id),
            ),
            Self::ResultTooLarge { limit_bytes } => (
                "schema",
                "tool_result_too_large".to_owned(),
                format!("The serialized tool result exceeded the {limit_bytes}-byte limit."),
                None,
                None,
                None,
            ),
            Self::Upstream {
                status,
                code,
                message,
                retry_after,
                diagnostic_id,
                ..
            } => (
                "upstream",
                code.clone(),
                message.clone(),
                Some(*status),
                retry_after.clone(),
                Some(*diagnostic_id),
            ),
            Self::RateLimited => (
                "rate_limit",
                "tool_rate_limited".to_owned(),
                "The tool call rate limit was exceeded.".to_owned(),
                None,
                None,
                None,
            ),
            Self::ConcurrentToolLimit => (
                "rate_limit",
                "tool_concurrency_limit".to_owned(),
                "The maximum number of concurrent tool calls was reached.".to_owned(),
                None,
                None,
                None,
            ),
            Self::ConcurrentRequestLimit => (
                "rate_limit",
                "upstream_concurrency_limit".to_owned(),
                "The maximum number of concurrent Tines requests was reached.".to_owned(),
                None,
                None,
                None,
            ),
        }
    }

    fn message(&self) -> String {
        self.fields().2
    }

    pub fn to_mcp_result(&self) -> rmcp::model::CallToolResult {
        let (source, code, message, status, retry_after, diagnostic_id) = self.fields();
        let details = match self {
            Self::Upstream { details, .. } => details.clone(),
            Self::ContentTooLarge { limit_bytes, .. } => Some(json!({"limit_bytes": limit_bytes})),
            _ => None,
        };
        mcp_result(
            json!({
                "data": null,
                "meta": empty_meta(),
                "error": {
                    "source": source,
                    "code": code,
                    "message": message,
                    "status": status,
                    "details": details,
                    "retry_after": retry_after,
                    "diagnostic_id": diagnostic_id,
                }
            }),
            true,
        )
    }
}

/// Stable success envelope returned as both MCP structured content and JSON text.
pub struct ResultEnvelope {
    data: Value,
    observed_at_ms: u128,
    links: Vec<Value>,
    omissions: Vec<String>,
}

impl ResultEnvelope {
    pub fn new(data: Value) -> Self {
        Self {
            data,
            observed_at_ms: now_millis(),
            links: Vec::new(),
            omissions: Vec::new(),
        }
    }

    pub fn with_links(mut self, links: Vec<Value>) -> Self {
        self.links = links;
        self
    }

    pub fn with_omissions(mut self, omissions: Vec<String>) -> Self {
        self.omissions = omissions;
        self
    }

    pub fn to_mcp_result(&self) -> rmcp::model::CallToolResult {
        let value = json!({
            "data": self.data,
            "meta": {
                "observed_at_ms": self.observed_at_ms,
                "links": self.links,
                "omissions": self.omissions,
            }
        });
        let result = mcp_result(value, false);
        let serialized_size = serialized_size(&result);
        if serialized_size > MAX_TOOL_RESULT_BYTES {
            return TinesError::ResultTooLarge {
                limit_bytes: MAX_TOOL_RESULT_BYTES,
            }
            .to_mcp_result();
        }
        result
    }
}

const MAX_TOOL_RESULT_BYTES: usize = 512 * 1024;

struct CountingWriter {
    bytes_written: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes_written = self.bytes_written.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialized_size(value: &rmcp::model::CallToolResult) -> usize {
    let mut writer = CountingWriter { bytes_written: 0 };
    serde_json::to_writer(&mut writer, value).expect("MCP tool results always serialize");
    writer.bytes_written
}

fn mcp_result(value: Value, is_error: bool) -> rmcp::model::CallToolResult {
    use rmcp::model::{CallToolResult, ContentBlock};

    let text = serde_json::to_string(&value).expect("JSON values always serialize");
    let mut result = if is_error {
        CallToolResult::error(vec![ContentBlock::text(text)])
    } else {
        CallToolResult::success(vec![ContentBlock::text(text)])
    };
    result.structured_content = Some(value);
    result
}

fn empty_meta() -> Value {
    json!({
        "observed_at_ms": now_millis(),
        "links": [],
        "omissions": [],
    })
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{Method, Request, Response, StatusCode, header},
        routing::any,
    };
    use serde_json::{Value, json};
    use tokio::{
        net::TcpListener,
        sync::{Semaphore, mpsc},
        task::JoinHandle,
        time::timeout,
    };
    use tokio_util::sync::CancellationToken;
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::fmt::MakeWriter;

    use crate::config::Config;

    use super::{
        MAX_ERROR_BODY_BYTES, MAX_JSON_BODY_BYTES, MAX_TOOL_RESULT_BYTES, Query, ResultEnvelope,
        Route, TinesClient, TinesError, read_bounded_body,
    };

    #[derive(Clone)]
    struct MockReply {
        status: StatusCode,
        body: String,
        location: Option<String>,
        retry_after: Option<String>,
        delay: Duration,
        gate: Option<Arc<Semaphore>>,
        content_length: Option<usize>,
    }

    struct ObservedRequest {
        method: Method,
        target: String,
        token_matches: bool,
    }

    #[derive(Clone)]
    struct MockState {
        reply: MockReply,
        expected_token: Arc<str>,
        observations: mpsc::UnboundedSender<ObservedRequest>,
        count: Arc<AtomicUsize>,
    }

    struct MockServer {
        base_url: String,
        observations: mpsc::UnboundedReceiver<ObservedRequest>,
        count: Arc<AtomicUsize>,
        task: JoinHandle<()>,
    }

    #[derive(Clone)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    struct LogWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for LogWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for LogBuffer {
        type Writer = LogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            LogWriter(self.0.clone())
        }
    }

    impl MockReply {
        fn json(status: StatusCode, body: impl Into<String>) -> Self {
            Self {
                status,
                body: body.into(),
                location: None,
                retry_after: None,
                delay: Duration::ZERO,
                gate: None,
                content_length: None,
            }
        }
    }

    async fn mock_handler(
        State(state): State<MockState>,
        request: Request<Body>,
    ) -> Response<Body> {
        state.count.fetch_add(1, Ordering::SeqCst);
        let target = request
            .uri()
            .path_and_query()
            .map(ToString::to_string)
            .unwrap_or_else(|| request.uri().path().to_owned());
        let token_matches = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == format!("Bearer {}", state.expected_token.as_ref()));
        let _ = state.observations.send(ObservedRequest {
            method: request.method().clone(),
            target,
            token_matches,
        });
        if !state.reply.delay.is_zero() {
            tokio::time::sleep(state.reply.delay).await;
        }
        if let Some(gate) = &state.reply.gate
            && let Ok(permit) = gate.acquire().await
        {
            permit.forget();
        }

        let mut builder = Response::builder()
            .status(state.reply.status)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(location) = &state.reply.location {
            builder = builder.header(header::LOCATION, location);
        }
        if let Some(retry_after) = &state.reply.retry_after {
            builder = builder.header(header::RETRY_AFTER, retry_after);
        }
        if let Some(length) = state.reply.content_length {
            builder = builder.header(header::CONTENT_LENGTH, length);
        }
        builder
            .body(Body::from(state.reply.body.clone()))
            .expect("mock response should build")
    }

    async fn start_mock(reply: MockReply) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener should bind");
        let address = listener
            .local_addr()
            .expect("mock listener should have an address");
        let (observations_tx, observations) = mpsc::unbounded_channel();
        let count = Arc::new(AtomicUsize::new(0));
        let state = MockState {
            reply,
            expected_token: Arc::from("local-test-token"),
            observations: observations_tx,
            count: count.clone(),
        };
        let app = Router::new().fallback(any(mock_handler)).with_state(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock server should remain available");
        });
        MockServer {
            base_url: format!("http://{address}"),
            observations,
            count,
            task,
        }
    }

    fn make_client(base_url: &str) -> TinesClient {
        make_client_with_deadline(base_url, Duration::from_secs(2))
    }

    fn make_client_with_deadline(base_url: &str, deadline: Duration) -> TinesClient {
        let values = [
            ("TINES_BASE_URL", base_url),
            ("TINES_API_TOKEN", "local-test-token"),
            ("TINES_ALLOW_HTTP", "true"),
        ];
        let config = Config::from_lookup(|name| {
            values
                .iter()
                .find_map(|(key, value)| (*key == name).then(|| (*value).to_owned()))
        })
        .expect("local mock configuration should validate");
        TinesClient::with_deadline(
            config.tines.as_ref().expect("Tines config should exist"),
            deadline,
        )
        .expect("client should build without contacting Tines")
    }

    fn empty_query() -> Query {
        Query::new()
    }

    async fn call(
        client: &TinesClient,
        route: Route<'_>,
        query: &Query,
    ) -> Result<Value, TinesError> {
        client
            .run_tool(CancellationToken::new(), |call| async move {
                call.get_json(route, query).await
            })
            .await
    }

    fn stop(server: MockServer) {
        server.task.abort();
    }

    #[tokio::test]
    async fn sends_only_get_to_a_fixed_route_and_encodes_path_and_query_values() {
        let mut server = start_mock(MockReply::json(StatusCode::OK, r#"{"ok":true}"#)).await;
        let client = make_client(&server.base_url);
        let mut query = Query::new();
        query.append("search", "a&b=c +");
        let result = call(
            &client,
            Route::Project {
                project_id: "team/name%2Fone",
            },
            &query,
        )
        .await
        .expect("mock JSON response should parse");
        assert_eq!(result, json!({"ok": true}));

        let request = server
            .observations
            .recv()
            .await
            .expect("mock request should be observed");
        assert_eq!(request.method, Method::GET);
        assert_eq!(
            request.target,
            "/api/v1/projects/team%2Fname%252Fone?search=a%26b%3Dc+%2B"
        );
        assert!(request.token_matches);
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
        stop(server);
    }

    #[tokio::test]
    async fn every_route_variant_maps_to_its_fixed_api_template() {
        let mut server = start_mock(MockReply::json(StatusCode::OK, "{}")).await;
        let client = make_client(&server.base_url);
        let cases = [
            (Route::Projects, "/api/v1/projects"),
            (
                Route::Project {
                    project_id: "prj_1",
                },
                "/api/v1/projects/prj_1",
            ),
            (Route::Issues, "/api/v1/issues"),
            (Route::Issue { issue_id: "iss_1" }, "/api/v1/issues/iss_1"),
            (
                Route::ProjectIssue {
                    project_id: "prj_1",
                    number: 7,
                },
                "/api/v1/projects/prj_1/issues/7",
            ),
            (Route::Workflows, "/api/v1/workflows"),
            (
                Route::Workflow {
                    workflow_id: "wfl_1",
                },
                "/api/v1/workflows/wfl_1",
            ),
            (Route::Labels, "/api/v1/labels"),
            (Route::Context, "/api/v1/context"),
            (
                Route::ContextItem {
                    context_id: "ctx_1",
                },
                "/api/v1/context/ctx_1",
            ),
            (
                Route::IssueContext { issue_id: "iss_1" },
                "/api/v1/issues/iss_1/context",
            ),
            (
                Route::IssueJournal { issue_id: "iss_1" },
                "/api/v1/issues/iss_1/journal",
            ),
            (
                Route::IssueArtifacts { issue_id: "iss_1" },
                "/api/v1/issues/iss_1/artifacts",
            ),
            (
                Route::IssueArtifact {
                    issue_id: "iss_1",
                    name: "report",
                },
                "/api/v1/issues/iss_1/artifacts/report",
            ),
            (
                Route::ArtifactContent {
                    issue_id: "iss_1",
                    name: "report",
                },
                "/api/v1/issues/iss_1/artifacts/report/content",
            ),
            (Route::Runs, "/api/v1/runs"),
            (Route::Run { run_id: "run_1" }, "/api/v1/runs/run_1"),
            (Route::RunLog { run_id: "run_1" }, "/api/v1/runs/run_1/log"),
            (
                Route::IssueDispatch { issue_id: "iss_1" },
                "/api/v1/issues/iss_1/dispatch",
            ),
            (Route::Events, "/api/v1/events"),
        ];

        for (route, expected_target) in cases {
            call(&client, route, &empty_query())
                .await
                .expect("mock response should parse");
            let request = server
                .observations
                .recv()
                .await
                .expect("route request should be observed");
            assert_eq!(request.method, Method::GET);
            assert_eq!(request.target, expected_target);
            assert!(request.token_matches);
        }
        assert_eq!(server.count.load(Ordering::SeqCst), 20);
        stop(server);
    }

    #[tokio::test]
    async fn dot_and_empty_dynamic_path_segments_are_rejected_before_request() {
        let mut server = start_mock(MockReply::json(StatusCode::OK, "{}")).await;
        let client = make_client(&server.base_url);
        let invalid_routes = [
            Route::Project { project_id: ".." },
            Route::Issue { issue_id: "." },
            Route::Issue { issue_id: "" },
            Route::ProjectIssue {
                project_id: "..",
                number: 1,
            },
            Route::Workflow { workflow_id: ".." },
            Route::ContextItem { context_id: ".." },
            Route::IssueContext { issue_id: ".." },
            Route::IssueJournal { issue_id: ".." },
            Route::IssueArtifacts { issue_id: ".." },
            Route::IssueArtifact {
                issue_id: "iss_1",
                name: "..",
            },
            Route::IssueArtifact {
                issue_id: "..",
                name: "report",
            },
            Route::ArtifactContent {
                issue_id: "..",
                name: "report",
            },
            Route::ArtifactContent {
                issue_id: "iss_1",
                name: "..",
            },
            Route::Run { run_id: ".." },
            Route::RunLog { run_id: ".." },
            Route::IssueDispatch { issue_id: ".." },
        ];

        for route in invalid_routes {
            assert!(matches!(
                call(&client, route, &empty_query()).await,
                Err(TinesError::InvalidRoute)
            ));
        }
        assert_eq!(server.count.load(Ordering::SeqCst), 0);
        assert!(server.observations.try_recv().is_err());
        stop(server);
    }

    #[tokio::test]
    async fn initialization_validation_builds_the_client_without_an_upstream_request() {
        let mut server = start_mock(MockReply::json(StatusCode::OK, "{}")).await;
        let _client = make_client(&server.base_url);
        assert!(
            timeout(Duration::from_millis(50), server.observations.recv())
                .await
                .is_err()
        );
        assert_eq!(server.count.load(Ordering::SeqCst), 0);
        stop(server);
    }

    #[tokio::test]
    async fn redirect_responses_are_not_followed() {
        let mut reply = MockReply::json(
            StatusCode::FOUND,
            r#"{"error":{"code":"redirect","message":"redirect"}}"#,
        );
        reply.location = Some("http://127.0.0.1:1/callback?secret=do-not-fetch".to_owned());
        let mut server = start_mock(reply).await;
        let client = make_client(&server.base_url);
        let error = call(&client, Route::Projects, &empty_query())
            .await
            .expect_err("redirect is an upstream error");
        assert!(matches!(error, TinesError::Upstream { status: 302, .. }));
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
        assert!(server.observations.recv().await.is_some());
        stop(server);
    }

    #[tokio::test]
    async fn authorization_errors_are_safe_mcp_tool_errors_with_equivalent_json_text() {
        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            let body = r#"{"error":{"code":"access_denied","message":"credential local-test-token refused","details":{"field":"expected_revision","expected_revision":2,"raw_response":"do-not-expose-this-body"}}}"#;
            let server = start_mock(MockReply::json(status, body)).await;
            let client = make_client(&server.base_url);
            let error = call(&client, Route::Projects, &empty_query())
                .await
                .expect_err("upstream authorization failure should fail the tool");
            let result = error.to_mcp_result();
            assert_eq!(result.is_error, Some(true));
            let structured = result
                .structured_content
                .expect("tool error should have structured content");
            let text = result.content[0]
                .as_text()
                .expect("tool error should include JSON text")
                .text
                .clone();
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), structured);
            assert_eq!(structured["error"]["status"], status.as_u16());
            assert_eq!(structured["error"]["code"], "access_denied");
            assert_eq!(
                structured["error"]["details"],
                json!({"field": "expected_revision", "expected_revision": 2})
            );
            assert!(text.contains("[REDACTED]"));
            assert!(!text.contains("local-test-token"));
            assert!(!text.contains("raw_response"));
            assert!(!text.contains(body));
            stop(server);
        }
    }

    #[tokio::test]
    async fn logs_and_diagnostics_exclude_tokens_bodies_identifiers_and_query_values() {
        let raw_body = r#"{"error":{"code":"failure","message":"local-test-token","raw":"request-private-body"}}"#;
        let server = start_mock(MockReply::json(StatusCode::BAD_REQUEST, raw_body)).await;
        let client = make_client(&server.base_url);
        let writer = LogBuffer(Arc::new(Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(writer.clone())
            .finish();
        let request = async {
            let mut query = Query::new();
            query.append("search", "query-private-value");
            call(
                &client,
                Route::Project {
                    project_id: "path-private-value",
                },
                &query,
            )
            .await
        }
        .with_subscriber(subscriber);
        let error = request.await.expect_err("upstream error should fail");
        let diagnostic = error.to_mcp_result();
        let text = diagnostic.content[0]
            .as_text()
            .expect("error should include JSON text")
            .text
            .clone();
        let logs = String::from_utf8(
            writer
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
        .expect("logs should be UTF-8");
        for private_value in [
            "local-test-token",
            raw_body,
            "request-private-body",
            "path-private-value",
            "query-private-value",
        ] {
            assert!(!text.contains(private_value));
            assert!(!logs.contains(private_value));
        }
        stop(server);
    }

    #[tokio::test]
    async fn malformed_error_responses_keep_status_without_returning_the_body() {
        let raw_body = "<html>upstream proxy echoed private response</html>";
        let server = start_mock(MockReply::json(StatusCode::BAD_GATEWAY, raw_body)).await;
        let client = make_client(&server.base_url);
        let error = call(&client, Route::Projects, &empty_query())
            .await
            .expect_err("bad gateway should be an upstream tool error");
        let result = error.to_mcp_result();
        let text = result.content[0]
            .as_text()
            .expect("upstream error should include text")
            .text
            .clone();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["error"]["status"], 502);
        assert_eq!(structured["error"]["code"], "upstream_error");
        assert!(!text.contains(raw_body));
        assert!(!text.contains("proxy echoed private response"));
        stop(server);
    }

    #[tokio::test]
    async fn successful_non_json_responses_return_bounded_schema_diagnostics() {
        let raw_body = "private-success-response-body";
        let server = start_mock(MockReply::json(StatusCode::OK, raw_body)).await;
        let client = make_client(&server.base_url);
        let error = call(&client, Route::Projects, &empty_query())
            .await
            .expect_err("non-JSON success body should fail schema validation");
        assert!(matches!(error, TinesError::InvalidJson { status: 200, .. }));
        let result = error.to_mcp_result();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["error"]["status"], 200);
        let text = result.content[0]
            .as_text()
            .expect("schema error should include JSON text")
            .text
            .clone();
        assert!(text.contains("invalid_upstream_json"));
        assert!(!text.contains(raw_body));
        stop(server);
    }

    #[tokio::test]
    async fn oversized_upstream_error_body_keeps_status_and_omits_body() {
        let raw_body = "x".repeat(MAX_ERROR_BODY_BYTES + 1);
        let mut reply = MockReply::json(StatusCode::BAD_GATEWAY, &raw_body);
        reply.content_length = Some(raw_body.len());
        let server = start_mock(reply).await;
        let client = make_client(&server.base_url);
        let error = call(&client, Route::Projects, &empty_query())
            .await
            .expect_err("oversized upstream error body should be bounded");
        let text = error.to_mcp_result().content[0]
            .as_text()
            .expect("upstream error should include JSON text")
            .text
            .clone();
        assert!(text.contains("8192-byte limit"));
        assert!(text.contains("502"));
        assert!(!text.contains(&raw_body));
        stop(server);
    }

    #[tokio::test]
    async fn oversized_success_body_schema_error_keeps_received_status() {
        let raw_body = "x".repeat(MAX_JSON_BODY_BYTES + 1);
        let mut reply = MockReply::json(StatusCode::OK, &raw_body);
        reply.content_length = Some(raw_body.len());
        let server = start_mock(reply).await;
        let client = make_client(&server.base_url);
        let error = call(&client, Route::Projects, &empty_query())
            .await
            .expect_err("oversized success body should be a schema error");
        assert!(matches!(
            error,
            TinesError::BodyTooLarge { status: 200, .. }
        ));
        let result = error.to_mcp_result();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["error"]["status"], 200);
        assert_eq!(structured["error"]["code"], "upstream_body_too_large");
        assert!(
            !result.content[0]
                .as_text()
                .expect("schema error should include text")
                .text
                .contains(&raw_body)
        );
        stop(server);
    }

    #[tokio::test]
    async fn upstream_404_remains_missing_or_inaccessible_with_native_status_and_code() {
        let server = start_mock(MockReply::json(
            StatusCode::NOT_FOUND,
            r#"{"error":{"code":"not_found","message":"Not found"}}"#,
        ))
        .await;
        let client = make_client(&server.base_url);
        let error = call(&client, Route::Issues, &empty_query())
            .await
            .expect_err("404 should remain an upstream error");
        let value = error.to_mcp_result().structured_content.unwrap();
        assert_eq!(value["error"]["source"], "upstream");
        assert_eq!(value["error"]["status"], 404);
        assert_eq!(value["error"]["code"], "not_found");
        assert_eq!(value["error"]["message"], "Not found");
        stop(server);
    }

    #[tokio::test]
    async fn error_headers_cannot_echo_the_token_into_diagnostics() {
        let mut reply = MockReply::json(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"code":"rate_limited","message":"Try later"}}"#,
        );
        reply.retry_after = Some("local-test-token".to_owned());
        let server = start_mock(reply).await;
        let client = make_client(&server.base_url);
        let error = call(&client, Route::Events, &empty_query())
            .await
            .expect_err("429 should be an upstream tool error");
        let text = error.to_mcp_result().content[0]
            .as_text()
            .expect("error should have text")
            .text
            .clone();
        assert!(!text.contains("local-test-token"));
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap()["error"]["retry_after"],
            Value::Null
        );
        stop(server);
    }

    #[tokio::test]
    async fn oversized_streaming_bodies_are_rejected_while_reading() {
        let server = start_mock(MockReply::json(StatusCode::OK, "12345")).await;
        let response = reqwest::Client::new()
            .get(&server.base_url)
            .send()
            .await
            .expect("mock response should arrive");
        assert!(matches!(
            read_bounded_body(response, 4).await,
            Err(super::BodyReadError::TooLarge)
        ));
        assert_eq!(MAX_JSON_BODY_BYTES, 32 * 1024 * 1024);
        assert_eq!(MAX_ERROR_BODY_BYTES, 8 * 1024);
        stop(server);
    }

    #[tokio::test]
    async fn cancellation_drops_an_in_flight_upstream_request() {
        let mut reply = MockReply::json(StatusCode::OK, "{}");
        reply.delay = Duration::from_secs(5);
        let mut server = start_mock(reply).await;
        let client = make_client(&server.base_url);
        let cancellation = CancellationToken::new();
        let request_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            client
                .run_tool(request_cancellation, |call| async move {
                    call.get_json(Route::Projects, &Query::new()).await
                })
                .await
        });
        let _observed = timeout(Duration::from_secs(1), server.observations.recv())
            .await
            .expect("upstream request should start")
            .expect("mock request should be observed");
        cancellation.cancel();
        assert!(matches!(task.await.unwrap(), Err(TinesError::Cancelled)));
        stop(server);
    }

    #[tokio::test]
    async fn tool_deadline_covers_the_upstream_request() {
        let mut reply = MockReply::json(StatusCode::OK, "{}");
        reply.delay = Duration::from_millis(100);
        let server = start_mock(reply).await;
        let client = make_client_with_deadline(&server.base_url, Duration::from_millis(20));
        let error = call(&client, Route::Projects, &empty_query())
            .await
            .expect_err("request should exceed the test deadline");
        assert!(matches!(error, TinesError::Timeout));
        stop(server);
    }

    #[tokio::test]
    async fn tool_and_upstream_concurrency_limits_reject_without_queuing() {
        let mut reply = MockReply::json(StatusCode::OK, "{}");
        reply.gate = Some(Arc::new(Semaphore::new(0)));
        let mut server = start_mock(reply).await;
        let client = make_client(&server.base_url);

        let mut cancellations = Vec::new();
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let cancellation = CancellationToken::new();
            cancellations.push(cancellation.clone());
            let client = client.clone();
            tasks.push(tokio::spawn(async move {
                client
                    .run_tool(cancellation, |call| async move {
                        call.get_json(Route::Projects, &Query::new()).await
                    })
                    .await
            }));
        }
        for _ in 0..4 {
            timeout(Duration::from_secs(1), server.observations.recv())
                .await
                .expect("each admitted tool should send one request")
                .expect("mock request should be observed");
        }
        let rejected = client
            .run_tool(CancellationToken::new(), |_call| async move {
                Ok::<_, TinesError>(())
            })
            .await;
        assert!(matches!(rejected, Err(TinesError::ConcurrentToolLimit)));

        for cancellation in cancellations {
            cancellation.cancel();
        }
        for task in tasks {
            assert!(matches!(task.await.unwrap(), Err(TinesError::Cancelled)));
        }
        stop(server);
    }

    #[tokio::test]
    async fn upstream_semaphore_caps_in_flight_requests_at_eight() {
        let mut reply = MockReply::json(StatusCode::OK, "{}");
        reply.gate = Some(Arc::new(Semaphore::new(0)));
        let mut server = start_mock(reply).await;
        let client = make_client(&server.base_url);
        let (attempted_tx, mut attempted_rx) = mpsc::unbounded_channel();
        let mut cancellations = Vec::new();
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let client = client.clone();
            let cancellation = CancellationToken::new();
            cancellations.push(cancellation.clone());
            let attempted_tx = attempted_tx.clone();
            tasks.push(tokio::spawn(async move {
                client
                    .run_tool(cancellation, |call| async move {
                        let first_query = Query::new();
                        let second_query = Query::new();
                        let third_query = Query::new();
                        let first_tx = attempted_tx.clone();
                        let second_tx = attempted_tx.clone();
                        let third_tx = attempted_tx.clone();
                        let first = async {
                            let _ = first_tx.send(());
                            call.get_json(Route::Projects, &first_query).await
                        };
                        let second = async {
                            let _ = second_tx.send(());
                            call.get_json(Route::Issues, &second_query).await
                        };
                        let third = async {
                            let _ = third_tx.send(());
                            call.get_json(Route::Events, &third_query).await
                        };
                        let (first, second, third) = tokio::join!(first, second, third);
                        first?;
                        second?;
                        third
                    })
                    .await
            }));
        }
        for _ in 0..8 {
            timeout(Duration::from_secs(1), server.observations.recv())
                .await
                .expect("eight upstream requests should start")
                .expect("mock request should be observed");
        }
        assert_eq!(server.count.load(Ordering::SeqCst), 8);
        for _ in 0..12 {
            timeout(Duration::from_secs(1), attempted_rx.recv())
                .await
                .expect("requests beyond the bound should be rejected promptly")
                .expect("request attempt should be observed");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.count.load(Ordering::SeqCst), 8);

        for cancellation in cancellations {
            cancellation.cancel();
        }
        for task in tasks {
            assert!(matches!(
                task.await.unwrap(),
                Err(TinesError::Cancelled | TinesError::ConcurrentRequestLimit)
            ));
        }
        stop(server);
    }

    #[tokio::test]
    async fn request_errors_do_not_retry() {
        let server = start_mock(MockReply::json(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":{"code":"busy","message":"try later"}}"#,
        ))
        .await;
        let client = make_client(&server.base_url);
        let error = call(&client, Route::Projects, &empty_query())
            .await
            .expect_err("service error should be returned");
        assert!(matches!(error, TinesError::Upstream { status: 503, .. }));
        assert_eq!(server.count.load(Ordering::SeqCst), 1);
        stop(server);
    }

    #[tokio::test]
    async fn default_rate_limit_allows_a_burst_of_twenty_then_rejects() {
        let server = start_mock(MockReply::json(StatusCode::OK, "{}")).await;
        let client = make_client(&server.base_url);
        for _ in 0..20 {
            client
                .run_tool(CancellationToken::new(), |_call| async {
                    Ok::<_, TinesError>(())
                })
                .await
                .expect("default burst should admit twenty calls");
        }
        assert!(matches!(
            client
                .run_tool(CancellationToken::new(), |_call| async {
                    Ok::<_, TinesError>(())
                })
                .await,
            Err(TinesError::RateLimited)
        ));
        assert_eq!(server.count.load(Ordering::SeqCst), 0);
        stop(server);
    }

    #[tokio::test]
    async fn success_envelope_uses_structured_content_and_equivalent_text() {
        let result = ResultEnvelope::new(json!({"items": [1, 2], "next_cursor": null}))
            .with_links(vec![json!({"rel": "self", "href": "/projects"})])
            .with_omissions(vec!["none".to_owned()])
            .to_mcp_result();
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.unwrap();
        let text = result.content[0]
            .as_text()
            .expect("success should include JSON text")
            .text
            .clone();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), structured);
        assert_eq!(structured["data"]["items"], json!([1, 2]));
        assert!(structured["meta"]["observed_at_ms"].is_number());
    }

    #[test]
    fn near_limit_results_fit_when_the_complete_serialized_result_is_bounded() {
        let data = json!({"text": "x".repeat(255 * 1024)});
        let result = ResultEnvelope::new(data).to_mcp_result();
        assert_eq!(result.is_error, Some(false));
        let serialized_size = serde_json::to_vec(&result)
            .expect("MCP result should serialize")
            .len();
        assert_eq!(super::serialized_size(&result), serialized_size);
        assert!(serialized_size > MAX_TOOL_RESULT_BYTES - (4 * 1024));
        assert!(serialized_size <= MAX_TOOL_RESULT_BYTES);
    }

    #[test]
    fn escaping_heavy_results_over_the_complete_wire_limit_fail() {
        let data = json!({"text": "\"\\".repeat(100 * 1024)});
        let result = ResultEnvelope::new(data).to_mcp_result();
        assert_eq!(result.is_error, Some(true));
        let value = result.structured_content.as_ref().unwrap();
        assert_eq!(value["error"]["code"], "tool_result_too_large");
        assert!(value["data"].is_null());
        let serialized_size = serde_json::to_vec(&result)
            .expect("bounded error result should serialize")
            .len();
        assert!(serialized_size <= MAX_TOOL_RESULT_BYTES);
    }
}
