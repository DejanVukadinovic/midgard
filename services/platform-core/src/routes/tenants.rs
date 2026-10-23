use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, Row};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    audit,
    authn::{ApiJson, Caller},
    common::{
        active_grant, require_active, resolve_user_context, valid_contract_name, valid_org_id,
        workload_app,
    },
    error::db,
    state::AppState,
};

fn tenant_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({
        "id": r.get::<Uuid,_>("id"),
        "organization_id": r.get::<String,_>("zitadel_organization_id"),
        "application_id": r.get::<Uuid,_>("application_id"),
        "status": r.get::<String,_>("status"),
    })
}

pub async fn create(
    State(st): State<AppState>,
    caller: Caller,
    Path((org, app_id)): Path<(String, Uuid)>,
) -> ApiResult<Response> {
    caller.require_operator(&st)?;
    if !valid_org_id(&org) {
        return Err(Problem::bad_request(
            "invalid_organization",
            "organization id has an invalid format",
        ));
    }
    let app_status: Option<String> = sqlx::query("SELECT status FROM applications WHERE id=$1")
        .bind(app_id)
        .fetch_optional(&st.pool)
        .await
        .map_err(db)?
        .map(|r| r.get("status"));
    match app_status.as_deref() {
        Some("active") => {}
        Some(_) => {
            return Err(Problem::conflict(
                "invalid_state",
                "application is not active",
            ));
        }
        None => return Err(Problem::not_found("application not found")),
    }
    let mut tx = st.pool.begin().await.map_err(db)?;
    let inserted = sqlx::query(
        "INSERT INTO tenants (zitadel_organization_id, application_id, status) VALUES ($1,$2,'active')
         ON CONFLICT (zitadel_organization_id, application_id) DO NOTHING RETURNING id, zitadel_organization_id, application_id, status",
    )
    .bind(&org)
    .bind(app_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    let (code, row) = match inserted {
        Some(r) => {
            let id: Uuid = r.get("id");
            audit::record(&mut tx, &caller, "tenant.create", &format!("tenant:{id}"), None, Some(tenant_json(&r))).await?;
            (StatusCode::CREATED, r)
        }
        None => (
            StatusCode::OK,
            sqlx::query("SELECT id, zitadel_organization_id, application_id, status FROM tenants WHERE zitadel_organization_id=$1 AND application_id=$2")
                .bind(&org)
                .bind(app_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?,
        ),
    };
    tx.commit().await.map_err(db)?;
    Ok((code, Json(tenant_json(&row))).into_response())
}

pub async fn get(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let r = sqlx::query(
        "SELECT id, zitadel_organization_id, application_id, status FROM tenants WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .map_err(db)?
    .ok_or_else(|| Problem::not_found("tenant not found"))?;
    Ok(Json(tenant_json(&r)))
}

async fn set_status(
    st: &AppState,
    caller: &Caller,
    id: Uuid,
    to: &'static str,
    from: &'static str,
) -> ApiResult<Json<Value>> {
    caller.require_operator(st)?;
    let mut tx = st.pool.begin().await.map_err(db)?;
    let r = sqlx::query("UPDATE tenants SET status=$2 WHERE id=$1 AND status=$3 RETURNING id, zitadel_organization_id, application_id, status")
        .bind(id)
        .bind(to)
        .bind(from)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| Problem::conflict("invalid_state", format!("tenant must be {from} to become {to}")))?;
    audit::record(
        &mut tx,
        caller,
        &format!("tenant.{to}"),
        &format!("tenant:{id}"),
        Some(json!({"status": from})),
        Some(json!({"status": to})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(tenant_json(&r)))
}

pub async fn suspend(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    set_status(&st, &caller, id, "suspended", "active").await
}

pub async fn activate(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    set_status(&st, &caller, id, "active", "suspended").await
}

#[derive(Deserialize)]
pub struct ResolveReq {
    pub user_token: String,
    #[serde(default)]
    pub organization_id: Option<String>,
    /// Optionally also require an active grant for this contract.
    #[serde(default)]
    pub contract: Option<String>,
}

/// Delegated tenant resolution: an application workload presents a *user's* access token.
/// A machine token alone never yields a tenant context.
pub async fn resolve(
    State(st): State<AppState>,
    caller: Caller,
    ApiJson(req): ApiJson<ResolveReq>,
) -> ApiResult<Json<Value>> {
    let app = workload_app(&st.pool, &caller).await?;
    require_active(&app)?;
    if let Some(o) = &req.organization_id
        && !valid_org_id(o)
    {
        return Err(Problem::bad_request(
            "invalid_organization",
            "organization id has an invalid format",
        ));
    }
    if let Some(c) = &req.contract
        && (!valid_contract_name(c) || active_grant(&st.pool, app.id, c).await?.is_none())
    {
        return Err(Problem::forbidden(
            "application has no grant for this contract",
        ));
    }
    let ctx =
        resolve_user_context(&st, &app, &req.user_token, req.organization_id.as_deref()).await?;
    Ok(Json(
        serde_json::to_value(ctx).map_err(platform_http::internal_error)?,
    ))
}
