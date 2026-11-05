use std::{
    error::Error,
    fmt,
    io::{self, Write},
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

use reqwest::{
    Client, Method, Response,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use serde_json::{Value, json};
use tokio::{
    sync::Semaphore,
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::{Secret, TinesConfig};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOOL_DEADLINE: Duration = Duration::from_secs(45);
const MAX_TOOL_CALLS: usize = 4;
const MAX_ERROR_TEXT_CHARS: usize = 512;
const MAX_ERROR_CODE_CHARS: usize = 128;
const RATE_LIMIT_PER_SECOND: f64 = 10.0;
const RATE_LIMIT_BURST: f64 = 20.0;

#[derive(Clone)]
pub struct TinesClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    http: Client,
    base_url: Url,
    authorization: HeaderValue,
    api_token: Secret,
    tool_slots: Arc<Semaphore>,
    rate_limit: RateLimit,
    tool_deadline: Duration,
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
                authorization,
                api_token: config.api_token.clone(),
                tool_slots: Arc::new(Semaphore::new(MAX_TOOL_CALLS)),
                rate_limit: RateLimit::new(),
                tool_deadline,
            }),
        })
    }
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

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

// The sole authenticated execution boundary for all registered tools.
// Domain code receives neither the client nor a request-capable projection hook.
mod direct {
    use super::*;
    use crate::tools::contract as d;
    use reqwest::header::{CONTENT_DISPOSITION, CONTENT_LENGTH};

    impl TinesClient {
        /// Execute a prepared operation once, then project and budget it locally.
        /// No preflight, redirect, retry, enrichment read or recovery request exists.
        pub async fn execute_prepared(
            &self,
            plan: d::PreparedCall,
            cancellation: CancellationToken,
        ) -> rmcp::model::CallToolResult {
            let started = Instant::now();
            let deadline = started + self.inner.tool_deadline;
            let write = plan.endpoint.method.is_write();
            let mut meta = d::ResultMeta {
                http_status: None,
                observed_at_ms: now_millis(),
                omissions: vec![],
                links: vec![],
                retry_after: None,
                write: write.then_some(d::WriteInfo {
                    outcome: d::WriteOutcome::NotAttempted,
                    committed: None,
                    receipt: None,
                }),
            };
            // Keep admission through projection and final serialization as well.
            let admission = self.inner.tool_slots.clone().try_acquire_owned();
            let execution = match &admission {
                Err(_) => Err(failure(
                    false,
                    None,
                    "tool_concurrency_limit",
                    "Concurrent tool limit reached",
                )),
                Ok(_permit) => {
                    if !self.inner.rate_limit.admit() {
                        Err(failure(
                            false,
                            None,
                            "rate_limited",
                            "Local rate limit reached",
                        ))
                    } else {
                        self.execute_once(&plan, &cancellation, deadline).await
                    }
                }
            };
            let request_count = usize::from(match &execution {
                Ok(_) => true,
                Err(failure) => failure.dispatched,
            });
            let envelope = match execution {
                Ok(response) => {
                    meta.http_status = Some(response.status);
                    meta.observed_at_ms = response.observed_at_ms;
                    meta.retry_after = response.headers.retry_after.clone();
                    match plan.project(response) {
                        Ok(projected) => {
                            if let Some(info) = &mut meta.write {
                                info.outcome = d::WriteOutcome::Confirmed;
                                info.receipt = projected.receipt;
                            }
                            meta.links = projected.links;
                            meta.omissions = projected.omissions;
                            d::ResultEnvelope {
                                data: projected.data,
                                error: None,
                                meta,
                            }
                        }
                        Err(error) => {
                            if let Some(info) = &mut meta.write {
                                info.outcome = if error.receipt.is_some() {
                                    d::WriteOutcome::Confirmed
                                } else {
                                    d::WriteOutcome::Unknown
                                };
                                info.receipt = error.receipt;
                            }
                            d::ResultEnvelope {
                                data: Value::Null,
                                error: Some(d::ResultError {
                                    code: error.code.into(),
                                    message: error.message,
                                    details: None,
                                }),
                                meta,
                            }
                        }
                    }
                }
                Err(failure) => {
                    meta.http_status = failure.http_status;
                    meta.observed_at_ms = failure.observed_at_ms;
                    meta.retry_after = failure.retry_after;
                    if let Some(info) = &mut meta.write {
                        let committed = failure
                            .error
                            .details
                            .as_ref()
                            .and_then(|v| v.get("committed"))
                            .and_then(Value::as_bool);
                        info.committed = committed;
                        info.outcome = if !failure.dispatched {
                            d::WriteOutcome::NotAttempted
                        } else if committed == Some(true) {
                            d::WriteOutcome::Confirmed
                        } else if committed == Some(false)
                            || plan.noncommitting_errors.iter().any(|rule| {
                                failure
                                    .http_status
                                    .is_some_and(|status| rule.matches(status, &failure.error))
                            })
                        {
                            d::WriteOutcome::Rejected
                        } else {
                            d::WriteOutcome::Unknown
                        };
                    }
                    d::ResultEnvelope {
                        data: Value::Null,
                        error: Some(failure.error),
                        meta,
                    }
                }
            };
            tracing::debug!(
                route = plan.endpoint.template,
                request_count,
                method = ?plan.endpoint.method,
                status = envelope.meta.http_status,
                elapsed_ms = started.elapsed().as_millis() as u64,
                outcome = envelope.meta.write.as_ref().map(|v| v.outcome).map(|v| format!("{v:?}")),
                "Tines direct call completed"
            );
            self.direct_result(envelope)
        }

        // Keep the mcp-00 evidence contract by value; this private async result
        // does not cross an error-trait boundary or require a boxed allocation.
        #[allow(clippy::result_large_err)]
        async fn execute_once(
            &self,
            plan: &d::PreparedCall,
            cancellation: &CancellationToken,
            deadline: Instant,
        ) -> d::ExecutionResult {
            let mut url = self.inner.base_url.clone();
            {
                let mut segments = url.path_segments_mut().map_err(|_| {
                    failure(
                        false,
                        None,
                        "invalid_route",
                        "Cannot construct fixed endpoint",
                    )
                })?;
                segments.pop_if_empty().push("api").push("v1");
                for segment in plan.endpoint.segments() {
                    segments.push(segment);
                }
            }
            url.set_fragment(None);
            url.set_query(None);
            if !plan.query.is_empty() {
                url.query_pairs_mut().extend_pairs(&plan.query);
            }
            let method = match plan.endpoint.method {
                d::Method::Get => Method::GET,
                d::Method::Post => Method::POST,
                d::Method::Patch => Method::PATCH,
                d::Method::Put => Method::PUT,
                d::Method::Delete => Method::DELETE,
            };
            let accept = match plan.response.kind {
                d::ResponseKind::Bytes { .. } => "application/octet-stream",
                _ => "application/json",
            };
            let mut builder = self
                .inner
                .http
                .request(method, url)
                .header(AUTHORIZATION, self.inner.authorization.clone())
                .header(ACCEPT, accept);
            builder = match &plan.body {
                d::RequestBody::Empty => builder,
                d::RequestBody::Json(value) => builder.json(value),
                d::RequestBody::Bytes {
                    bytes,
                    content_type,
                } => builder
                    .header(CONTENT_TYPE, content_type.as_str())
                    .body(bytes.clone()),
            };
            let request = builder
                .build()
                .map_err(|_| failure(false, None, "invalid_request", "Cannot construct request"))?;
            // Explicit checks make an already cancelled/expired call zero-request.
            if cancellation.is_cancelled() {
                return Err(failure(
                    false,
                    None,
                    "request_cancelled",
                    "Tool call cancelled",
                ));
            }
            if Instant::now() >= deadline {
                return Err(failure(
                    false,
                    None,
                    "upstream_timeout",
                    "Tool deadline exceeded",
                ));
            }
            // Once execute is polled we cannot establish whether bytes reached Tines.
            // Keep response evidence outside the cancellable body-read future.
            let dispatched = std::sync::atomic::AtomicBool::new(false);
            let send = async {
                dispatched.store(true, Ordering::Relaxed);
                self.inner.http.execute(request).await
            };
            let response = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(failure(dispatched.load(Ordering::Relaxed), None, "request_cancelled", "Tool call cancelled")),
                _ = sleep_until(deadline) => return Err(failure(dispatched.load(Ordering::Relaxed), None, "upstream_timeout", "Tool deadline exceeded")),
                result = send => result.map_err(|error| {
                    if error.is_timeout() {
                        failure(true, None, "upstream_timeout", "Tool deadline exceeded")
                    } else {
                        failure(true, None, "upstream_transport_error", "Tines request failed during transport")
                    }
                })?,
            };
            let status = response.status();
            let http_status = status.as_u16();
            let observed_at_ms = now_millis();
            let headers = response.headers();
            let secret = self.inner.api_token.expose();
            let safe_header = |name| {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| safe_error_message(v, secret))
            };
            let headers = d::ResponseHeaders {
                content_type: safe_header(CONTENT_TYPE),
                content_length: headers
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok()),
                content_disposition: safe_header(CONTENT_DISPOSITION),
                retry_after: safe_retry_after(headers.get(RETRY_AFTER), secret),
            };
            let limit = if !status.is_success() {
                d::MAX_ERROR_BYTES
            } else {
                match plan.response.kind {
                    d::ResponseKind::Json => d::MAX_SUCCESS_JSON_BYTES,
                    d::ResponseKind::Bytes { max_bytes } => {
                        max_bytes.min(d::MAX_CONTENT_SOURCE_BYTES)
                    }
                    d::ResponseKind::Empty => 0,
                }
            };
            let body = tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(failure(true, Some(http_status), "request_cancelled", "Tool call cancelled")),
                _ = sleep_until(deadline) => Err(failure(true, Some(http_status), "upstream_timeout", "Tool deadline exceeded")),
                result = read_bounded_body(response, limit) => result.map_err(|error| match error {
                    BodyReadError::TooLarge => failure(true, Some(http_status), "upstream_body_too_large", "Tines response exceeded its byte limit"),
                    BodyReadError::Transport => failure(true, Some(http_status), "upstream_transport_error", "Tines response could not be read"),
                }),
            };
            if !status.is_success() {
                let mut error = failure(
                    true,
                    Some(http_status),
                    "upstream_error",
                    "Tines returned an unreadable or malformed error response",
                );
                if let Ok(bytes) = &body {
                    if let Ok(value) = serde_json::from_slice::<Value>(bytes)
                        && let Some(native) = value.get("error")
                    {
                        if let Some(code) = native.get("code").and_then(Value::as_str) {
                            error.error.code = code.into();
                        }
                        if let Some(message) = native.get("message").and_then(Value::as_str) {
                            error.error.message = message.into();
                        }
                        error.error.details = native.get("details").cloned();
                    }
                } else if let Err(failed) = body {
                    // Keep cancellation/size/transport identity with the native status.
                    error = failed;
                }
                error.observed_at_ms = observed_at_ms;
                error.retry_after = headers.retry_after;
                return Err(error);
            }
            let bytes = body.map_err(|mut error| {
                error.observed_at_ms = observed_at_ms;
                error.retry_after = headers.retry_after.clone();
                error
            })?;
            let body = match plan.response.kind {
                d::ResponseKind::Json => {
                    d::ResponseBody::Json(serde_json::from_slice(&bytes).map_err(|_| {
                        let mut error = failure(
                            true,
                            Some(http_status),
                            "invalid_upstream_json",
                            "Tines returned invalid JSON",
                        );
                        error.observed_at_ms = observed_at_ms;
                        error.retry_after = headers.retry_after.clone();
                        error
                    })?)
                }
                d::ResponseKind::Bytes { .. } => d::ResponseBody::Bytes(bytes),
                d::ResponseKind::Empty => d::ResponseBody::Empty,
            };
            Ok(d::UpstreamResponse {
                status: http_status,
                observed_at_ms,
                headers,
                body,
            })
        }

        /// Also used for local preparation errors; no execution or admission occurs.
        pub(crate) fn direct_result(
            &self,
            envelope: d::ResultEnvelope,
        ) -> rmcp::model::CallToolResult {
            bounded_result(envelope, self.inner.api_token.expose())
        }
    }

    fn failure(
        dispatched: bool,
        status: Option<u16>,
        code: &str,
        message: &str,
    ) -> d::ExecutionFailure {
        d::ExecutionFailure {
            dispatched,
            http_status: status,
            observed_at_ms: now_millis(),
            retry_after: None,
            error: d::ResultError {
                code: code.into(),
                message: message.into(),
                details: None,
            },
        }
    }

    // Redact keys as well as values, including success data, links and receipts.
    fn redact_json(value: &mut Value, secret: &str) {
        match value {
            Value::String(text) => *text = redact_secret(text, secret),
            Value::Array(values) => values.iter_mut().for_each(|v| redact_json(v, secret)),
            Value::Object(values) => {
                *values = std::mem::take(values)
                    .into_iter()
                    .map(|(key, mut value)| {
                        redact_json(&mut value, secret);
                        (redact_secret(&key, secret), value)
                    })
                    .collect();
            }
            _ => {}
        }
    }

    fn bounded_result(
        mut envelope: d::ResultEnvelope,
        secret: &str,
    ) -> rmcp::model::CallToolResult {
        if let Some(error) = &mut envelope.error {
            // Native codes need not use the legacy ASCII identifier allowlist.
            error.code = redact_secret(&error.code, secret)
                .chars()
                .take(MAX_ERROR_CODE_CHARS)
                .collect();
            error.message = safe_error_message(&error.message, secret);
        }
        redact_json(&mut envelope.data, secret);
        if let Some(details) = envelope.error.as_mut().and_then(|e| e.details.as_mut()) {
            redact_json(details, secret);
        }
        for link in &mut envelope.meta.links {
            redact_json(link, secret);
        }
        for omission in &mut envelope.meta.omissions {
            *omission = redact_secret(omission, secret);
        }
        if let Some(value) = &mut envelope.meta.retry_after {
            *value = safe_error_message(value, secret);
        }
        if let Some(receipt) = envelope
            .meta
            .write
            .as_mut()
            .and_then(|w| w.receipt.as_mut())
        {
            let mut value = Value::Object(std::mem::take(&mut receipt.0));
            redact_json(&mut value, secret);
            receipt.0 = value
                .as_object_mut()
                .map(std::mem::take)
                .unwrap_or_default();
        }
        let render = |e: &d::ResultEnvelope| {
            mcp_result(
                serde_json::to_value(e).expect("envelope serializes"),
                e.error.is_some(),
            )
        };
        let result = render(&envelope);
        if serialized_size(&result) <= d::MAX_RESULT_BYTES {
            return result;
        }
        drop(result);
        // Retain native status/code/message and commitment evidence on error.
        if let Some(error) = &mut envelope.error
            && let Some(details) = error.details.take()
        {
            error.details = details
                .get("committed")
                .and_then(Value::as_bool)
                .map(|committed| json!({"committed": committed}));
            envelope
                .meta
                .omissions
                .push("Native error details omitted to fit the MCP result limit".into());
        }
        if !envelope.data.is_null() {
            envelope.data = Value::Null;
            envelope
                .meta
                .omissions
                .push("Response data omitted to fit the MCP result limit".into());
        }
        let result = render(&envelope);
        if serialized_size(&result) <= d::MAX_RESULT_BYTES {
            return result;
        }
        drop(result);
        envelope.meta.links.clear();
        envelope.meta.omissions = vec![
            "Response data, details and optional metadata omitted to fit the MCP result limit"
                .into(),
        ];
        let result = render(&envelope);
        if serialized_size(&result) <= d::MAX_RESULT_BYTES {
            return result;
        }
        drop(result);
        // Receipts are compact by contract. Defensively omit oversized fields,
        // never truncate an identifier into a different, apparently valid ID.
        if let Some(receipt) = envelope
            .meta
            .write
            .as_mut()
            .and_then(|w| w.receipt.as_mut())
        {
            receipt.0.retain(|key, value| {
                key.len() <= 1024 && serde_json::to_vec(value).is_ok_and(|v| v.len() <= 8192)
            });
            while serde_json::to_vec(receipt)
                .expect("receipt serializes")
                .len()
                > 16 * 1024
            {
                let Some(key) = receipt.0.keys().next_back().cloned() else {
                    break;
                };
                receipt.0.remove(&key);
            }
            envelope
                .meta
                .omissions
                .push("Oversized receipt fields omitted".into());
        }
        render(&envelope)
    }
}

#[cfg(test)]
#[path = "direct_executor_tests.rs"]
mod direct_executor_tests;
