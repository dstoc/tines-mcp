//! Pure contracts for the direct-call surface. No authenticated client belongs here.
//!
//! Domain modules own their fixed endpoints and strict argument schemas. The executor
//! owns URL encoding, authentication, dispatch, response parsing, redaction and final
//! result budgeting. A projector can only inspect one already received response.

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

pub use crate::content::{MAX_CONTENT_CHUNK_BYTES, MAX_CONTENT_SOURCE_BYTES};

pub const DEFAULT_MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
pub const HARD_MAX_REQUEST_BYTES: usize = 36 * 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 512 * 1024;
pub const MAX_SUCCESS_JSON_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ERROR_BYTES: usize = 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_CONCURRENT_CALLS: usize = 4;
pub const MAX_CONCURRENT_POST_BODIES: usize = 4;
pub const BODY_DEADLINE: Duration = Duration::from_secs(30);
pub const TOOL_DEADLINE: Duration = Duration::from_secs(45);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Effective local configuration, available before any upstream operation.
/// mcp-02 supplies this to discovery and preparation from ingress configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalLimits {
    max_request_bytes: usize,
}

impl LocalLimits {
    pub fn new(max_request_bytes: usize) -> Result<Self, LocalError> {
        if !(1..=HARD_MAX_REQUEST_BYTES).contains(&max_request_bytes) {
            return Err(LocalError::invalid(
                "max_request_bytes",
                "must be between 1 and 37748736 bytes",
            ));
        }
        Ok(Self { max_request_bytes })
    }

    /// The entire serialized JSON-RPC HTTP body counts, including its envelope.
    pub fn max_request_bytes(self) -> usize {
        self.max_request_bytes
    }
}

impl Default for LocalLimits {
    fn default() -> Self {
        Self {
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Patch,
    Put,
    Delete,
}

impl Method {
    pub fn is_write(self) -> bool {
        self != Self::Get
    }
}

/// One endpoint chosen by a domain module, never by an MCP argument.
/// Segments are unescaped: the executor pushes them individually into `Url`.
/// `template` is a fixed, body-free log label relative to `/api/v1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub method: Method,
    pub template: &'static str,
    segments: Vec<String>,
}

impl Endpoint {
    pub fn new(
        method: Method,
        template: &'static str,
        segments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, LocalError> {
        let segments: Vec<String> = segments.into_iter().map(Into::into).collect();
        // Url::path_segments_mut ignores dot segments; reject rather than retarget.
        if segments.is_empty()
            || segments
                .iter()
                .any(|s| s.is_empty() || s == "." || s == "..")
        {
            return Err(LocalError::invalid(
                "path",
                "contains an empty or dot segment",
            ));
        }
        Ok(Self {
            method,
            template,
            segments,
        })
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }
}

/// Query order and repeated keys are preserved. The executor URL-encodes them.
pub type Query = Vec<(String, String)>;

/// Raw uploads may set only Content-Type. No arbitrary request-header map exists.
/// Bodies deliberately do not implement Debug: they can contain secret context.
pub enum RequestBody {
    Empty,
    Json(Value),
    Bytes {
        bytes: Vec<u8>,
        content_type: ContentType,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct ContentType(String);

impl ContentType {
    pub fn new(value: String) -> Result<Self, LocalError> {
        if value.is_empty() || !value.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(LocalError::invalid(
                "content_type",
                "must be a nonempty safe header value",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The executor enforces byte limits while buffering, before projection.
pub enum ResponseKind {
    Json,
    Bytes { max_bytes: usize },
    Empty,
}

/// Accept only endpoint-specific statuses. A 2xx alone is not a write receipt.
#[derive(Clone, Copy, Debug)]
pub struct ResponseContract {
    pub statuses: &'static [u16],
    pub kind: ResponseKind,
}

/// Parsed successful response. Invalid JSON and over-limit bodies are executor
/// failures, not a successful JSON response with fabricated null data.
pub struct UpstreamResponse {
    pub status: u16,
    pub observed_at_ms: u128,
    pub headers: ResponseHeaders,
    pub body: ResponseBody,
}

/// Executor failure, including native errors and invalid/over-limit success
/// bodies. Keep dispatch evidence even when no response/status was received.
/// The executor redacts and bounds `error` before exposing this value.
pub struct ExecutionFailure {
    pub dispatched: bool,
    pub http_status: Option<u16>,
    pub observed_at_ms: u128,
    pub retry_after: Option<String>,
    pub error: ResultError,
}

pub type ExecutionResult = Result<UpstreamResponse, ExecutionFailure>;

/// Only observed response metadata; no artifact identity/version is inferred.
#[derive(Default)]
pub struct ResponseHeaders {
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub content_disposition: Option<String>,
    pub retry_after: Option<String>,
}

pub enum ResponseBody {
    Json(Value),
    Bytes(Vec<u8>),
    Empty,
}

/// A synchronous function pointer plus local data, not a closure or async request.
/// Validation must extract the receipt before optional enrichment/output budgeting.
pub struct Projection {
    pub project: fn(UpstreamResponse, &Value) -> Result<Projected, ProjectionError>,
    pub context: Value,
}

/// Exactly one request. There is no operation list, fallback or next-request hook.
pub struct PreparedCall {
    pub endpoint: Endpoint,
    pub query: Query,
    pub body: RequestBody,
    pub response: ResponseContract,
    pub projection: Projection,
    pub noncommitting_errors: &'static [NoncommittingError],
}

impl PreparedCall {
    /// Executes only local projection, for both the executor and pure fixtures.
    pub fn project(&self, response: UpstreamResponse) -> Result<Projected, ProjectionError> {
        if !self.response.statuses.contains(&response.status) {
            return Err(ProjectionError::new(
                "unexpected_status",
                "Unexpected success status",
            ));
        }
        let matches_kind = matches!(
            (self.response.kind, &response.body),
            (ResponseKind::Json, ResponseBody::Json(_))
                | (ResponseKind::Bytes { .. }, ResponseBody::Bytes(_))
                | (ResponseKind::Empty, ResponseBody::Empty)
        );
        if !matches_kind {
            return Err(ProjectionError::new(
                "invalid_response",
                "Unexpected response body kind",
            ));
        }
        let projected = (self.projection.project)(response, &self.projection.context)?;
        if self.endpoint.method.is_write() && projected.receipt.is_none() {
            return Err(ProjectionError::new(
                "invalid_receipt",
                "Write projection did not validate a success receipt",
            ));
        }
        Ok(projected)
    }
}

/// Native JSON stays extensible. Do not deserialize it into a closed request type.
pub struct Projected {
    pub data: Value,
    pub receipt: Option<Receipt>,
    pub links: Vec<Value>,
    pub omissions: Vec<String>,
}

/// Small, validated fields copied from the response, never input IDs masquerading
/// as create evidence. Use native names for identifiers/revisions/status fields.
/// 204 operations use an empty receipt; the expected 204 is their evidence.
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(transparent)]
pub struct Receipt(pub Map<String, Value>);

impl std::fmt::Debug for Receipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Receipt([REDACTED])")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionError {
    pub code: &'static str,
    pub message: String,
    /// Set only after endpoint-specific success validation. Optional content
    /// projection failure must not erase evidence that the write committed.
    pub receipt: Option<Receipt>,
}

impl ProjectionError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            receipt: None,
        }
    }

    pub fn with_receipt(mut self, receipt: Receipt) -> Self {
        self.receipt = Some(receipt);
        self
    }
}

/// Local failure: zero outbound requests, `not_attempted` for a selected write.
/// Errors must describe fields, not repeat untrusted argument values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalError {
    pub code: &'static str,
    pub field: Option<&'static str>,
    pub message: String,
}

impl LocalError {
    pub fn invalid(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: "invalid_input",
            field: Some(field),
            message: message.into(),
        }
    }
}

/// An endpoint-specific exception to the conservative unknown-on-error default.
/// Add a rule only with a native regression test that checks no mutation/event
/// committed. Cite revision, test path and test name in `native_test` and mirror
/// it in the checked-in endpoint fixture. A status class is never sufficient.
#[derive(Clone, Copy, Debug)]
pub struct NoncommittingError {
    pub status: u16,
    pub code: &'static str,
    pub native_test: &'static str,
}

impl NoncommittingError {
    /// Exact endpoint rule; never override explicit native commitment evidence.
    pub fn matches(&self, status: u16, error: &ResultError) -> bool {
        status == self.status
            && error.code == self.code
            && !self.native_test.is_empty()
            && error
                .details
                .as_ref()
                .and_then(|d| d.get("committed"))
                .is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome {
    NotAttempted,
    Rejected,
    Confirmed,
    Unknown,
}

/// Preserve explicit native commitment separately from the adapter outcome.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct WriteInfo {
    pub outcome: WriteOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Receipt>,
}

/// Native errors retain bounded extensible details. mcp-01 owns decoding,
/// configured-token redaction, evidence classification and output reduction.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ResultError {
    pub code: String,
    pub message: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub details: Option<Value>,
}

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ResultMeta {
    pub http_status: Option<u16>,
    pub observed_at_ms: u128,
    pub omissions: Vec<String>,
    pub links: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write: Option<WriteInfo>,
}

/// Serialize this same envelope to text and structured content. `error.is_some()`
/// determines isError. A validated success reduced for output size has data:null,
/// its receipt, an omission and no error. mcp-01 budgets the complete MCP result.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ResultEnvelope {
    pub data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResultError>,
    pub meta: ResultMeta,
}

/// Nullable PATCH field: use `#[serde(default, skip_serializing_if =
/// "PatchField::is_missing")]` on the enclosing field. Plain Option<T> is for
/// optional non-null fields only (which must reject explicit null when present).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<T> PatchField<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for PatchField<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

impl<T: Serialize> Serialize for PatchField<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(value) => value.serialize(serializer),
            Self::Null => serializer.serialize_none(),
            Self::Missing => Err(serde::ser::Error::custom(
                "missing PATCH fields must be omitted",
            )),
        }
    }
}

impl<T: schemars::JsonSchema> schemars::JsonSchema for PatchField<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Nullable_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <Option<T>>::json_schema(generator)
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        format!("tines_mcp::tools::contract::PatchField<{}>", T::schema_id()).into()
    }
}

/// For optional non-null fields, combine with `#[serde(default)]`. Missing
/// becomes None; present values must deserialize as T, so null is not missing.
/// With schemars derive, use `#[schemars(with = "T")]` on that field as well,
/// to advertise the same non-null value type (replace T with the concrete type).
pub fn deserialize_optional<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// The selected module's request type must use `deny_unknown_fields`.
/// Do not return serde's raw message: custom errors can contain payload values.
pub fn arguments<T: serde::de::DeserializeOwned>(
    value: Map<String, Value>,
) -> Result<T, LocalError> {
    serde_json::from_value(Value::Object(value))
        .map_err(|_| LocalError::invalid("arguments", "must match the advertised tool schema"))
}

/// Shared by issue creation and relations; names must never reach get-or-create.
pub fn validate_label_id(value: &str) -> Result<(), LocalError> {
    if value.strip_prefix("lbl_").is_some_and(|suffix| {
        suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
    }) {
        Ok(())
    } else {
        Err(LocalError::invalid(
            "label_ids",
            "must contain existing lbl_ IDs with 16 alphanumeric characters",
        ))
    }
}
