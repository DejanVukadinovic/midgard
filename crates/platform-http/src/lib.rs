//! Shared HTTP conventions: RFC 9457 problem responses and request IDs.

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

pub const PROBLEM_JSON: &str = "application/problem+json";
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// An RFC 9457 problem document with a stable machine-readable `code`.
#[derive(Debug, Clone)]
pub struct Problem {
    pub status: u16,
    pub code: &'static str,
    pub detail: String,
    pub request_id: Option<String>,
    pub retry_after_secs: Option<u32>,
}

impl Problem {
    pub fn new(status: StatusCode, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status: status.as_u16(),
            code,
            detail: detail.into(),
            request_id: None,
            retry_after_secs: None,
        }
    }

    pub fn bad_request(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, detail)
    }
    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", detail)
    }
    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", detail)
    }
    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", detail)
    }
    pub fn conflict(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, detail)
    }
    pub fn unprocessable(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, detail)
    }
    pub fn unavailable(code: &'static str, detail: impl Into<String>) -> Self {
        let mut p = Self::new(StatusCode::SERVICE_UNAVAILABLE, code, detail);
        p.retry_after_secs = Some(5);
        p
    }
    pub fn internal() -> Self {
        // Never leak internals; details go to logs.
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    }
    pub fn with_request_id(mut self, id: Option<String>) -> Self {
        self.request_id = id;
        self
    }

    fn body(&self, status: StatusCode) -> serde_json::Value {
        let mut v = serde_json::json!({
            "type": format!("https://midgard.invalid/problems/{}", self.code),
            "title": status.canonical_reason().unwrap_or("Error"),
            "status": self.status,
            "code": self.code,
            "detail": self.detail,
        });
        if let Some(id) = &self.request_id {
            v["request_id"] = serde_json::Value::String(id.clone());
        }
        v
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut resp = (status, Json(self.body(status))).into_response();
        resp.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        if let Some(Ok(v)) = self
            .retry_after_secs
            .map(|s| HeaderValue::from_str(&s.to_string()))
        {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        resp
    }
}

pub type ApiResult<T> = Result<T, Problem>;

/// Log the underlying error and return an opaque 500.
pub fn internal_error<E: std::fmt::Display>(e: E) -> Problem {
    tracing::error!(error = %e, "internal error");
    Problem::internal()
}
