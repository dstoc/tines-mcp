//! Reusable pure helpers: `mod support;` in each feature integration test.
//! No server, token, executor or network runtime is needed.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tines_mcp::tools::contract::{
    Method, PreparedCall, Projected, ProjectionError, RequestBody, ResponseBody, ResponseHeaders,
    UpstreamResponse,
};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyFixture {
    Empty,
    Json {
        value: Value,
    },
    Bytes {
        bytes: Vec<u8>,
        content_type: String,
    },
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanFixture {
    pub method: Method,
    pub template: String,
    pub segments: Vec<String>,
    pub query: Vec<(String, String)>,
    pub body: BodyFixture,
}

/// Inspect raw segments/pairs. Encoding is the executor's responsibility and
/// must be verified separately in HTTP-level tests, including error paths.
pub fn inspect(plan: &PreparedCall) -> PlanFixture {
    PlanFixture {
        method: plan.endpoint.method,
        template: plan.endpoint.template.to_owned(),
        segments: plan.endpoint.segments().to_vec(),
        query: plan.query.clone(),
        body: match &plan.body {
            RequestBody::Empty => BodyFixture::Empty,
            RequestBody::Json(value) => BodyFixture::Json {
                value: value.clone(),
            },
            RequestBody::Bytes {
                bytes,
                content_type,
            } => BodyFixture::Bytes {
                bytes: bytes.clone(),
                content_type: content_type.as_str().to_owned(),
            },
        },
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseFixture {
    pub status: u16,
    #[serde(default)]
    pub observed_at_ms: u128,
    #[serde(default)]
    pub content_type: Option<String>,
    pub body: BodyFixture,
}

pub fn project(
    plan: &PreparedCall,
    response: ResponseFixture,
) -> Result<Projected, ProjectionError> {
    let (body, content_type) = match response.body {
        BodyFixture::Empty => (ResponseBody::Empty, response.content_type),
        BodyFixture::Json { value } => (ResponseBody::Json(value), response.content_type),
        BodyFixture::Bytes {
            bytes,
            content_type,
        } => (ResponseBody::Bytes(bytes), Some(content_type)),
    };
    plan.project(UpstreamResponse {
        status: response.status,
        observed_at_ms: response.observed_at_ms,
        headers: ResponseHeaders {
            content_type,
            ..Default::default()
        },
        body,
    })
}
