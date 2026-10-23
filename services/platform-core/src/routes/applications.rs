use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use platform_contracts::parse_constraint;
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, Row};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    audit,
    authn::{ApiJson, Caller, idempotency_key},
    common::valid_contract_name,
    error::{db, is_unique_violation},
    provisioning::{ClientSpec, ProvisionError, ProvisionRequest},
    state::{AppState, Metrics},
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Requirement {
    pub contract: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegisterReq {
    pub slug: String,
    pub display_name: String,
    #[serde(default)]
    pub zitadel_project_id: Option<String>,
    #[serde(default)]
    pub oauth_clients: Vec<ClientSpec>,
    #[serde(default)]
    pub requirements: Vec<Requirement>,
}

fn valid_slug(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn valid_redirect(kind: &str, uri: &str) -> bool {
    let Ok(u) = url::Url::parse(uri) else {
        return false;
    };
    if u.fragment().is_some() || uri.len() > 512 {
        return false;
    }
    match u.scheme() {
        "https" => true,
        "http" => matches!(u.host_str(), Some("localhost" | "127.0.0.1")),
        "javascript" | "data" | "file" | "vbscript" | "blob" => false,
        _ => kind == "mobile", // custom app schemes only for native clients
    }
}

fn validate(req: &RegisterReq) -> Result<(), Problem> {
    if !valid_slug(&req.slug) {
        return Err(Problem::bad_request(
            "invalid_slug",
            "slug must match ^[a-z][a-z0-9-]{1,31}$",
        ));
    }
    if req.display_name.trim().is_empty() || req.display_name.len() > 100 {
        return Err(Problem::bad_request(
            "invalid_display_name",
            "display_name must be 1-100 characters",
        ));
    }
    if req.oauth_clients.len() > 8 {
        return Err(Problem::bad_request(
            "too_many_clients",
            "at most 8 oauth clients",
        ));
    }
    for c in &req.oauth_clients {
        if !matches!(c.kind.as_str(), "web" | "mobile" | "backend") {
            return Err(Problem::bad_request(
                "invalid_client_kind",
                "kind must be web, mobile or backend",
            ));
        }
        if c.kind != "backend" && c.redirect_uris.is_empty() && c.zitadel_client_id.is_none() {
            return Err(Problem::bad_request(
                "invalid_redirect_uri",
                "web/mobile clients need at least one redirect URI",
            ));
        }
        if let Some(bad) = c.redirect_uris.iter().find(|u| !valid_redirect(&c.kind, u)) {
            return Err(Problem::bad_request(
                "invalid_redirect_uri",
                format!("redirect URI rejected: {bad}"),
            ));
        }
    }
    for r in &req.requirements {
        if !valid_contract_name(&r.contract) {
            return Err(Problem::bad_request(
                "invalid_requirement",
                format!("invalid contract name `{}`", r.contract),
            ));
        }
        parse_constraint(&r.version)
            .map_err(|e| Problem::bad_request("invalid_version_constraint", e.to_string()))?;
    }
    Ok(())
}

fn to_provision(req: &RegisterReq) -> ProvisionRequest {
    ProvisionRequest {
        slug: req.slug.clone(),
        display_name: req.display_name.clone(),
        existing_project_id: req.zitadel_project_id.clone(),
        clients: req.oauth_clients.clone(),
    }
}

pub async fn application_view(st: &AppState, id: Uuid) -> ApiResult<Value> {
    let a = sqlx::query(
        "SELECT id, slug, display_name, zitadel_project_id, status FROM applications WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .map_err(db)?
    .ok_or_else(|| Problem::not_found("application not found"))?;
    let clients: Vec<Value> = sqlx::query("SELECT zitadel_client_id, kind, status FROM oauth_clients WHERE application_id=$1 ORDER BY kind, zitadel_client_id")
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| json!({"client_id": r.get::<String,_>("zitadel_client_id"), "kind": r.get::<String,_>("kind"), "status": r.get::<String,_>("status")}))
        .collect();
    let reqs: Vec<Value> = sqlx::query("SELECT contract_name, version_constraint FROM application_requirements WHERE application_id=$1 ORDER BY contract_name")
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| json!({"contract": r.get::<String,_>("contract_name"), "version": r.get::<String,_>("version_constraint")}))
        .collect();
    Ok(json!({
        "id": a.get::<Uuid,_>("id"), "slug": a.get::<String,_>("slug"), "display_name": a.get::<String,_>("display_name"),
        "zitadel_project_id": a.get::<Option<String>,_>("zitadel_project_id"), "status": a.get::<String,_>("status"),
        "oauth_clients": clients, "requirements": reqs,
    }))
}

pub async fn register(
    State(st): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    ApiJson(req): ApiJson<RegisterReq>,
) -> ApiResult<Response> {
    caller.require_operator(&st)?;
    let key = idempotency_key(&headers)?;
    validate(&req)?;
    let pr = to_provision(&req);
    st.provisioner
        .validate(&pr)
        .map_err(|e| Problem::bad_request("invalid_provisioning_request", e.to_string()))?;
    let hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&req).map_err(platform_http::internal_error)?,
    ));

    // Step 1: durable intent (operation + pending application), or resume an existing operation.
    let mut tx = st.pool.begin().await.map_err(db)?;
    let existing = sqlx::query("SELECT id, request_hash, status, application_id FROM provisioning_operations WHERE idempotency_key=$1 FOR UPDATE")
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
    let (op_id, app_id) = match existing {
        Some(r) => {
            if r.get::<String, _>("request_hash") != hash {
                return Err(Problem::conflict(
                    "idempotency_conflict",
                    "Idempotency-Key was already used with a different payload",
                ));
            }
            let (op_id, app_id): (Uuid, Option<Uuid>) = (r.get("id"), r.get("application_id"));
            if r.get::<String, _>("status") == "succeeded" {
                tx.rollback().await.ok();
                let view =
                    application_view(&st, app_id.ok_or_else(platform_http::Problem::internal)?)
                        .await?;
                return Ok((
                    StatusCode::OK,
                    Json(
                        json!({"operation_id": op_id, "status": "succeeded", "application": view}),
                    ),
                )
                    .into_response());
            }
            (op_id, app_id.ok_or_else(Problem::internal)?)
        }
        None => {
            let app_id: Uuid = match sqlx::query("INSERT INTO applications (slug, display_name, status) VALUES ($1,$2,'pending') RETURNING id")
                .bind(&req.slug)
                .bind(&req.display_name)
                .fetch_one(&mut *tx)
                .await
            {
                Ok(r) => r.get("id"),
                Err(e) if is_unique_violation(&e) => return Err(Problem::conflict("slug_taken", "an application with this slug already exists")),
                Err(e) => return Err(db(e)),
            };
            let op_id: Uuid = sqlx::query(
                "INSERT INTO provisioning_operations (idempotency_key, request_hash, kind, desired_state, application_id, actor)
                 VALUES ($1,$2,'application.register',$3,$4,$5) RETURNING id",
            )
            .bind(&key)
            .bind(&hash)
            .bind(serde_json::to_value(&req).map_err(platform_http::internal_error)?)
            .bind(app_id)
            .bind(caller.subject())
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?
            .get("id");
            (op_id, app_id)
        }
    };
    tx.commit().await.map_err(db)?;

    // Step 2: IdP provisioning (idempotent reconcile; safe to repeat).
    let provisioned = match st.provisioner.provision(&pr).await {
        Ok(p) => p,
        Err(e) => {
            Metrics::inc(&st.metrics.provisioning_failures);
            let msg = e.to_string();
            sqlx::query("UPDATE provisioning_operations SET status='failed', step='idp', error=$2, updated_at=now() WHERE id=$1")
                .bind(op_id)
                .bind(&msg)
                .execute(&st.pool)
                .await
                .map_err(db)?;
            let status = if matches!(e, ProvisionError::Invalid(_)) {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
            };
            return Err(Problem::new(
                status,
                "provisioning_failed",
                format!("operation {op_id} failed: {msg}; retry with the same Idempotency-Key"),
            ));
        }
    };

    // Step 3: persist mappings and activate atomically.
    let mut tx = st.pool.begin().await.map_err(db)?;
    let upd = sqlx::query("UPDATE applications SET zitadel_project_id=$2, status='active', updated_at=now() WHERE id=$1 AND status IN ('pending')")
        .bind(app_id)
        .bind(&provisioned.project_id)
        .execute(&mut *tx)
        .await;
    match upd {
        Ok(_) => {}
        Err(e) if is_unique_violation(&e) => {
            return Err(Problem::conflict(
                "project_already_linked",
                "that identity project is already linked to another application",
            ));
        }
        Err(e) => return Err(db(e)),
    }
    for (kind, client_id) in &provisioned.clients {
        let r = sqlx::query(
            "INSERT INTO oauth_clients (application_id, zitadel_client_id, kind) VALUES ($1,$2,$3)
             ON CONFLICT (zitadel_client_id) DO UPDATE SET kind = EXCLUDED.kind WHERE oauth_clients.application_id = $1",
        )
        .bind(app_id)
        .bind(client_id)
        .bind(kind)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        if r.rows_affected() == 0 {
            return Err(Problem::conflict(
                "client_already_linked",
                "an OAuth client is already linked to another application",
            ));
        }
    }
    for r in &req.requirements {
        sqlx::query(
            "INSERT INTO application_requirements (application_id, contract_name, version_constraint) VALUES ($1,$2,$3)
             ON CONFLICT (application_id, contract_name) DO UPDATE SET version_constraint = EXCLUDED.version_constraint",
        )
        .bind(app_id)
        .bind(&r.contract)
        .bind(&r.version)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    }
    sqlx::query("UPDATE provisioning_operations SET status='succeeded', step='complete', error=NULL, updated_at=now() WHERE id=$1")
        .bind(op_id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    audit::record(
        &mut tx,
        &caller,
        "application.register",
        &format!("application:{app_id}"),
        None,
        Some(json!({"slug": req.slug, "project": provisioned.project_id})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Metrics::inc(&st.metrics.registrations);

    let view = application_view(&st, app_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"operation_id": op_id, "status": "succeeded", "application": view})),
    )
        .into_response())
}

pub async fn list(State(st): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let ids: Vec<Uuid> = sqlx::query("SELECT id FROM applications ORDER BY slug LIMIT 500")
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .iter()
        .map(|r| r.get("id"))
        .collect();
    let mut out = Vec::new();
    for id in ids {
        out.push(application_view(&st, id).await?);
    }
    Ok(Json(json!({"items": out})))
}

pub async fn get(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    Ok(Json(application_view(&st, id).await?))
}

#[derive(Deserialize)]
pub struct PatchReq {
    pub display_name: Option<String>,
    pub requirements: Option<Vec<Requirement>>,
}

pub async fn patch(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
    ApiJson(p): ApiJson<PatchReq>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    if let Some(n) = &p.display_name
        && (n.trim().is_empty() || n.len() > 100)
    {
        return Err(Problem::bad_request(
            "invalid_display_name",
            "display_name must be 1-100 characters",
        ));
    }
    for r in p.requirements.iter().flatten() {
        if !valid_contract_name(&r.contract) {
            return Err(Problem::bad_request(
                "invalid_requirement",
                "invalid contract name",
            ));
        }
        parse_constraint(&r.version)
            .map_err(|e| Problem::bad_request("invalid_version_constraint", e.to_string()))?;
    }
    let before = application_view(&st, id).await?;
    let mut tx = st.pool.begin().await.map_err(db)?;
    if let Some(n) = &p.display_name {
        sqlx::query("UPDATE applications SET display_name=$2, updated_at=now() WHERE id=$1")
            .bind(id)
            .bind(n)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
    }
    if let Some(reqs) = &p.requirements {
        sqlx::query("DELETE FROM application_requirements WHERE application_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        for r in reqs {
            sqlx::query("INSERT INTO application_requirements (application_id, contract_name, version_constraint) VALUES ($1,$2,$3) ON CONFLICT (application_id, contract_name) DO UPDATE SET version_constraint=EXCLUDED.version_constraint")
                .bind(id)
                .bind(&r.contract)
                .bind(&r.version)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
    }
    let after = {
        tx.commit().await.map_err(db)?;
        application_view(&st, id).await?
    };
    let mut c = st.pool.acquire().await.map_err(db)?;
    audit::record(
        &mut c,
        &caller,
        "application.update",
        &format!("application:{id}"),
        Some(before),
        Some(after.clone()),
    )
    .await?;
    Ok(Json(after))
}

pub async fn suspend(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let mut tx = st.pool.begin().await.map_err(db)?;
    let r = sqlx::query("UPDATE applications SET status='suspended', updated_at=now() WHERE id=$1 AND status IN ('active','suspended')")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    if r.rows_affected() == 0 {
        return Err(Problem::conflict(
            "invalid_state",
            "only an active application can be suspended",
        ));
    }
    audit::record(
        &mut tx,
        &caller,
        "application.suspend",
        &format!("application:{id}"),
        None,
        Some(json!({"status":"suspended"})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(application_view(&st, id).await?))
}

pub async fn activate(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let mut tx = st.pool.begin().await.map_err(db)?;
    let r = sqlx::query("UPDATE applications SET status='active', updated_at=now() WHERE id=$1 AND status='suspended'")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    if r.rows_affected() == 0 {
        return Err(Problem::conflict(
            "invalid_state",
            "only a suspended application can be reactivated",
        ));
    }
    audit::record(
        &mut tx,
        &caller,
        "application.activate",
        &format!("application:{id}"),
        None,
        Some(json!({"status":"active"})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(application_view(&st, id).await?))
}
