use std::sync::atomic::Ordering;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, Row};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{authn::Caller, error::db, state::AppState};

pub async fn live() -> StatusCode {
    StatusCode::OK
}

/// Readiness: database reachable and migrated. IdP outages do not make the process unready
/// (token verification fails closed per request instead).
pub async fn ready(State(st): State<AppState>) -> StatusCode {
    match sqlx::query("SELECT 1 FROM applications LIMIT 1")
        .execute(&st.pool)
        .await
    {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub async fn metrics(State(st): State<AppState>) -> impl IntoResponse {
    let m = &st.metrics;
    let unhealthy: i64 = sqlx::query(
        "SELECT count(*) AS n FROM deployments WHERE status IN ('degraded','unavailable')",
    )
    .fetch_one(&st.pool)
    .await
    .map(|r| r.get("n"))
    .unwrap_or(-1);
    let bindings: i64 = sqlx::query("SELECT count(*) AS n FROM contract_bindings")
        .fetch_one(&st.pool)
        .await
        .map(|r| r.get("n"))
        .unwrap_or(-1);
    let body = format!(
        "# TYPE midgard_resolve_ok_total counter\nmidgard_resolve_ok_total {}\n# TYPE midgard_resolve_denied_total counter\nmidgard_resolve_denied_total {}\n# TYPE midgard_registrations_total counter\nmidgard_registrations_total {}\n# TYPE midgard_provisioning_failures_total counter\nmidgard_provisioning_failures_total {}\n# TYPE midgard_probe_failures_total counter\nmidgard_probe_failures_total {}\n# TYPE midgard_unhealthy_deployments gauge\nmidgard_unhealthy_deployments {}\n# TYPE midgard_active_bindings gauge\nmidgard_active_bindings {}\n",
        m.resolve_ok.load(Ordering::Relaxed),
        m.resolve_denied.load(Ordering::Relaxed),
        m.registrations.load(Ordering::Relaxed),
        m.provisioning_failures.load(Ordering::Relaxed),
        m.probe_failures.load(Ordering::Relaxed),
        unhealthy,
        bindings
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}

pub async fn get_operation(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let r = sqlx::query("SELECT id, kind, status, step, error, application_id FROM provisioning_operations WHERE id=$1")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .map_err(db)?
        .ok_or_else(|| Problem::not_found("operation not found"))?;
    Ok(Json(json!({
        "id": r.get::<Uuid,_>("id"), "kind": r.get::<String,_>("kind"), "status": r.get::<String,_>("status"),
        "step": r.get::<String,_>("step"), "error": r.get::<Option<String>,_>("error"), "application_id": r.get::<Option<Uuid>,_>("application_id"),
    })))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub target_prefix: Option<String>,
}

pub async fn audit_list(
    State(st): State<AppState>,
    caller: Caller,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let prefix = format!(
        "{}%",
        q.target_prefix.unwrap_or_default().replace(['%', '_'], "")
    );
    let items: Vec<Value> = sqlx::query("SELECT id, actor, action, target, before, after, request_id, created_at FROM audit_log WHERE target LIKE $1 ORDER BY id DESC LIMIT $2")
        .bind(prefix)
        .bind(limit)
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| json!({
            "id": r.get::<i64,_>("id"), "actor": r.get::<String,_>("actor"), "action": r.get::<String,_>("action"), "target": r.get::<String,_>("target"),
            "before": r.get::<Option<Value>,_>("before"), "after": r.get::<Option<Value>,_>("after"),
            "request_id": r.get::<Option<String>,_>("request_id"), "created_at": r.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),
        }))
        .collect();
    Ok(Json(json!({"items": items})))
}
