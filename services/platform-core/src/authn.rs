use axum::{
    extract::{FromRequest, FromRequestParts, Request},
    http::{header, request::Parts},
};
use platform_auth::{AuthError, VerifiedToken};
use platform_http::{Problem, REQUEST_ID_HEADER};
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::state::AppState;

/// A verified bearer-token caller. Authorization (operator/publisher/deployer/workload) is
/// decided per route by the `require_*` helpers.
pub struct Caller {
    pub token: VerifiedToken,
    pub request_id: Option<String>,
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        let request_id = parts
            .headers
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let raw = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
            })
            .ok_or_else(|| {
                Problem::unauthorized("missing bearer token").with_request_id(request_id.clone())
            })?;
        match state.verifier.verify(raw.trim()).await {
            Ok(token) => Ok(Caller { token, request_id }),
            Err(AuthError::JwksUnavailable) => Err(Problem::unavailable(
                "idp_unavailable",
                "identity provider keys unavailable",
            )
            .with_request_id(request_id)),
            Err(e) => {
                tracing::info!(reason = %e, "token rejected");
                Err(Problem::unauthorized("invalid access token").with_request_id(request_id))
            }
        }
    }
}

impl Caller {
    pub fn subject(&self) -> &str {
        &self.token.subject
    }

    pub fn require_operator(&self, state: &AppState) -> Result<(), Problem> {
        if state
            .cfg
            .operator_subjects
            .iter()
            .any(|s| s == self.subject())
        {
            Ok(())
        } else {
            Err(Problem::forbidden("operator privileges required"))
        }
    }

    pub fn is_operator(&self, state: &AppState) -> bool {
        state
            .cfg
            .operator_subjects
            .iter()
            .any(|s| s == self.subject())
    }

    /// Publisher for `namespace_prefix` (operators are unrestricted).
    pub fn require_publisher(&self, state: &AppState, contract_id: &str) -> Result<(), Problem> {
        if self.is_operator(state) {
            return Ok(());
        }
        match state.cfg.publishers.get(self.subject()) {
            Some(prefixes)
                if prefixes.is_empty()
                    || prefixes.iter().any(|p| contract_id.starts_with(p.as_str())) =>
            {
                Ok(())
            }
            _ => Err(Problem::forbidden(
                "contract publisher privileges required for this namespace",
            )),
        }
    }
}

/// JSON body extractor that reports malformed input as an RFC 9457 problem.
pub struct ApiJson<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = Problem;

    async fn from_request(req: Request, state: &S) -> Result<Self, Problem> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(ApiJson(v)),
            Err(e) => {
                let status = e.status();
                let code = if status == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
                    "payload_too_large"
                } else {
                    "invalid_body"
                };
                Err(Problem::new(status, code, e.body_text()))
            }
        }
    }
}

pub fn idempotency_key(headers: &axum::http::HeaderMap) -> Result<String, Problem> {
    let k = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty() && k.len() <= 128)
        .ok_or_else(|| {
            Problem::bad_request(
                "idempotency_key_required",
                "Idempotency-Key header (1-128 chars) is required",
            )
        })?;
    Ok(k.to_string())
}

pub fn new_request_id() -> String {
    Uuid::new_v4().to_string()
}
