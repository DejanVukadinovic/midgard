use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, Row};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    audit,
    authn::{ApiJson, Caller},
    common::valid_contract_name,
    error::db,
    state::AppState,
};

#[derive(Deserialize)]
pub struct GrantReq {
    pub contract: String,
    pub permissions: Vec<String>,
}

async fn app_exists(st: &AppState, id: Uuid) -> ApiResult<()> {
    sqlx::query("SELECT 1 FROM applications WHERE id=$1")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .map_err(db)?
        .map(|_| ())
        .ok_or_else(|| Problem::not_found("application not found"))
}

pub async fn create_grant(
    State(st): State<AppState>,
    caller: Caller,
    Path(app_id): Path<Uuid>,
    ApiJson(g): ApiJson<GrantReq>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    caller.require_operator(&st)?;
    app_exists(&st, app_id).await?;
    if !valid_contract_name(&g.contract) || g.permissions.is_empty() || g.permissions.len() > 64 {
        return Err(Problem::bad_request(
            "invalid_grant",
            "valid contract name and 1-64 permissions required",
        ));
    }
    let mut perms = g.permissions.clone();
    perms.sort();
    perms.dedup();
    // Permissions must be a subset of what any published version of the contract declares.
    let declared: Vec<String> =
        sqlx::query("SELECT DISTINCT unnest(permissions) AS p FROM contracts WHERE name=$1")
            .bind(&g.contract)
            .fetch_all(&st.pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(|r| r.get("p"))
            .collect();
    if declared.is_empty()
        && sqlx::query("SELECT 1 FROM contracts WHERE name=$1 LIMIT 1")
            .bind(&g.contract)
            .fetch_optional(&st.pool)
            .await
            .map_err(db)?
            .is_none()
    {
        return Err(Problem::unprocessable(
            "contract_not_published",
            "no published version of this contract",
        ));
    }
    if let Some(bad) = perms.iter().find(|p| !declared.contains(p)) {
        return Err(Problem::unprocessable(
            "unknown_permission",
            format!("permission `{bad}` is not declared by the contract"),
        ));
    }
    let mut tx = st.pool.begin().await.map_err(db)?;
    let existing = sqlx::query("SELECT id, permissions FROM contract_grants WHERE application_id=$1 AND contract_name=$2 AND status='active' FOR UPDATE")
        .bind(app_id)
        .bind(&g.contract)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
    if let Some(e) = existing {
        let mut cur: Vec<String> = e.get("permissions");
        cur.sort();
        if cur == perms {
            return Ok((
                StatusCode::OK,
                Json(
                    json!({"id": e.get::<Uuid,_>("id"), "contract": g.contract, "permissions": perms, "status": "active"}),
                ),
            ));
        }
        return Err(Problem::conflict(
            "grant_exists",
            "an active grant with different permissions exists; revoke it first",
        ));
    }
    let id: Uuid = sqlx::query("INSERT INTO contract_grants (application_id, contract_name, permissions) VALUES ($1,$2,$3) RETURNING id")
        .bind(app_id)
        .bind(&g.contract)
        .bind(&perms)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?
        .get("id");
    audit::record(
        &mut tx,
        &caller,
        "grant.create",
        &format!("application:{app_id}/grant:{id}"),
        None,
        Some(json!({"contract": g.contract, "permissions": perms})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "contract": g.contract, "permissions": perms, "status": "active"})),
    ))
}

pub async fn revoke_grant(
    State(st): State<AppState>,
    caller: Caller,
    Path((app_id, grant_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    caller.require_operator(&st)?;
    let mut tx = st.pool.begin().await.map_err(db)?;
    let r = sqlx::query("UPDATE contract_grants SET status='revoked', revoked_at=now() WHERE id=$1 AND application_id=$2 AND status='active' RETURNING contract_name, permissions")
        .bind(grant_id)
        .bind(app_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| Problem::not_found("active grant not found"))?;
    audit::record(&mut tx, &caller, "grant.revoke", &format!("application:{app_id}/grant:{grant_id}"), Some(json!({"contract": r.get::<String,_>("contract_name"), "permissions": r.get::<Vec<String>,_>("permissions")})), None).await?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_grants(
    State(st): State<AppState>,
    caller: Caller,
    Path(app_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let items: Vec<Value> = sqlx::query("SELECT id, contract_name, permissions, status FROM contract_grants WHERE application_id=$1 ORDER BY created_at")
        .bind(app_id)
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| json!({"id": r.get::<Uuid,_>("id"), "contract": r.get::<String,_>("contract_name"), "permissions": r.get::<Vec<String>,_>("permissions"), "status": r.get::<String,_>("status")}))
        .collect();
    Ok(Json(json!({"items": items})))
}

#[derive(Deserialize)]
pub struct BindReq {
    pub major: i32,
    pub environment: String,
    /// Implementation slug.
    pub implementation: String,
}

pub async fn put_binding(
    State(st): State<AppState>,
    caller: Caller,
    Path((app_id, contract)): Path<(Uuid, String)>,
    ApiJson(b): ApiJson<BindReq>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    app_exists(&st, app_id).await?;
    if !valid_contract_name(&contract)
        || b.major < 0
        || b.environment.is_empty()
        || b.environment.len() > 32
    {
        return Err(Problem::bad_request(
            "invalid_binding",
            "invalid contract, major or environment",
        ));
    }
    let mut tx = st.pool.begin().await.map_err(db)?;
    let imp = sqlx::query("SELECT id, status FROM implementations WHERE slug=$1")
        .bind(&b.implementation)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .ok_or_else(|| {
            Problem::unprocessable("implementation_not_found", "unknown implementation")
        })?;
    if imp.get::<String, _>("status") != "active" {
        return Err(Problem::conflict(
            "implementation_disabled",
            "implementation is disabled",
        ));
    }
    let impl_id: Uuid = imp.get("id");
    let implements = sqlx::query("SELECT 1 FROM implementation_contracts ic JOIN contracts c ON c.id=ic.contract_id WHERE ic.implementation_id=$1 AND c.name=$2 AND c.major=$3 LIMIT 1")
        .bind(impl_id)
        .bind(&contract)
        .bind(b.major)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
    if implements.is_none() {
        return Err(Problem::unprocessable(
            "contract_not_implemented",
            "implementation does not implement this contract major",
        ));
    }
    let before = sqlx::query("SELECT i.slug FROM contract_bindings b JOIN implementations i ON i.id=b.implementation_id WHERE b.application_id=$1 AND b.contract_name=$2 AND b.major_version=$3 AND b.environment=$4")
        .bind(app_id)
        .bind(&contract)
        .bind(b.major)
        .bind(&b.environment)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .map(|r| json!({"implementation": r.get::<String,_>("slug")}));
    sqlx::query(
        "INSERT INTO contract_bindings (application_id, contract_name, major_version, environment, implementation_id) VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (application_id, contract_name, major_version, environment) DO UPDATE SET implementation_id=EXCLUDED.implementation_id, updated_at=now()",
    )
    .bind(app_id)
    .bind(&contract)
    .bind(b.major)
    .bind(&b.environment)
    .bind(impl_id)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    let after =
        json!({"implementation": b.implementation, "major": b.major, "environment": b.environment});
    audit::record(
        &mut tx,
        &caller,
        "binding.put",
        &format!("application:{app_id}/binding:{contract}"),
        before,
        Some(after.clone()),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(
        json!({"contract": contract, "major": b.major, "environment": b.environment, "implementation": b.implementation}),
    ))
}

#[derive(Deserialize)]
pub struct UnbindQuery {
    pub major: i32,
    pub environment: String,
}

pub async fn delete_binding(
    State(st): State<AppState>,
    caller: Caller,
    Path((app_id, contract)): Path<(Uuid, String)>,
    Query(q): Query<UnbindQuery>,
) -> ApiResult<StatusCode> {
    caller.require_operator(&st)?;
    let mut tx = st.pool.begin().await.map_err(db)?;
    let r = sqlx::query("DELETE FROM contract_bindings WHERE application_id=$1 AND contract_name=$2 AND major_version=$3 AND environment=$4")
        .bind(app_id)
        .bind(&contract)
        .bind(q.major)
        .bind(&q.environment)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    if r.rows_affected() == 0 {
        return Err(Problem::not_found("binding not found"));
    }
    audit::record(
        &mut tx,
        &caller,
        "binding.delete",
        &format!("application:{app_id}/binding:{contract}"),
        Some(json!({"major": q.major, "environment": q.environment})),
        None,
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}
