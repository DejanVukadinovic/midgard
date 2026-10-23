use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, PgConnection, Row};
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    audit,
    authn::{ApiJson, Caller},
    endpoint_policy::validate_endpoint,
    error::db,
    health,
    routes::identities::authorized_for_slug,
    state::AppState,
};

#[derive(Deserialize)]
pub struct ImplementsSpec {
    pub contract: String,
    pub version: String,
    pub digest: String,
}

#[derive(Deserialize)]
pub struct DeploymentSpec {
    pub environment: String,
    pub endpoint: String,
}

#[derive(Deserialize)]
pub struct RegisterReq {
    pub slug: String,
    pub release_version: String,
    pub implements: Vec<ImplementsSpec>,
    #[serde(default)]
    pub deployments: Vec<DeploymentSpec>,
}

fn valid_env(e: &str) -> bool {
    !e.is_empty()
        && e.len() <= 32
        && e.chars().next().unwrap().is_ascii_lowercase()
        && e.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_impl_slug(s: &str) -> bool {
    (2..=64).contains(&s.len())
        && s.chars().next().unwrap().is_ascii_lowercase()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

async fn implementation_view(st: &AppState, id: Uuid) -> ApiResult<Value> {
    let i = sqlx::query(
        "SELECT id, slug, release_version, status, owner_identity FROM implementations WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .map_err(db)?
    .ok_or_else(|| Problem::not_found("implementation not found"))?;
    let impls: Vec<Value> = sqlx::query("SELECT c.name, c.version, ic.contract_digest FROM implementation_contracts ic JOIN contracts c ON c.id=ic.contract_id WHERE ic.implementation_id=$1 ORDER BY c.name, c.major, c.minor, c.patch")
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| json!({"contract": r.get::<String,_>("name"), "version": r.get::<String,_>("version"), "digest": r.get::<String,_>("contract_digest")}))
        .collect();
    let deps: Vec<Value> = sqlx::query("SELECT id, environment, logical_endpoint, status, last_health_at FROM deployments WHERE implementation_id=$1 ORDER BY environment")
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| json!({"id": r.get::<Uuid,_>("id"), "environment": r.get::<String,_>("environment"), "endpoint": r.get::<String,_>("logical_endpoint"), "status": r.get::<String,_>("status"), "last_health_at": r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_health_at")}))
        .collect();
    Ok(json!({
        "id": i.get::<Uuid,_>("id"), "slug": i.get::<String,_>("slug"), "release_version": i.get::<String,_>("release_version"),
        "status": i.get::<String,_>("status"), "implements": impls, "deployments": deps,
    }))
}

async fn upsert_deployment(
    st: &AppState,
    conn: &mut PgConnection,
    impl_id: Uuid,
    d: &DeploymentSpec,
) -> ApiResult<Uuid> {
    if !valid_env(&d.environment) {
        return Err(Problem::bad_request(
            "invalid_environment",
            "environment must match ^[a-z][a-z0-9-]{0,31}$",
        ));
    }
    let url = validate_endpoint(&d.endpoint, &st.cfg.endpoint_allowed_hosts)
        .map_err(|e| Problem::unprocessable("endpoint_rejected", e))?;
    // Normalized: no trailing slash.
    let endpoint = url.as_str().trim_end_matches('/').to_string();
    let row = sqlx::query(
        "INSERT INTO deployments (implementation_id, environment, logical_endpoint, status) VALUES ($1,$2,$3,'registered')
         ON CONFLICT (implementation_id, environment) DO UPDATE
           SET logical_endpoint = EXCLUDED.logical_endpoint,
               status = CASE WHEN deployments.logical_endpoint = EXCLUDED.logical_endpoint AND deployments.status <> 'disabled' THEN deployments.status ELSE 'registered' END,
               consecutive_failures = CASE WHEN deployments.logical_endpoint = EXCLUDED.logical_endpoint THEN deployments.consecutive_failures ELSE 0 END,
               updated_at = now()
         RETURNING id",
    )
    .bind(impl_id)
    .bind(&d.environment)
    .bind(&endpoint)
    .fetch_one(conn)
    .await
    .map_err(db)?;
    Ok(row.get("id"))
}

pub async fn register(
    State(st): State<AppState>,
    caller: Caller,
    ApiJson(req): ApiJson<RegisterReq>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if !valid_impl_slug(&req.slug) {
        return Err(Problem::bad_request(
            "invalid_slug",
            "slug must match ^[a-z][a-z0-9-]{1,63}$",
        ));
    }
    // Identity scoped to this slug; reveals nothing about existing registrations on failure.
    if !authorized_for_slug(&st, caller.subject(), &req.slug).await? {
        return Err(Problem::forbidden(
            "identity is not approved to register this implementation",
        ));
    }
    Version::parse(&req.release_version).map_err(|_| {
        Problem::bad_request("invalid_release_version", "release_version must be semver")
    })?;
    if req.implements.is_empty() || req.implements.len() > 32 || req.deployments.len() > 16 {
        return Err(Problem::bad_request(
            "invalid_request",
            "1-32 contracts and at most 16 deployments",
        ));
    }

    let mut tx = st.pool.begin().await.map_err(db)?;
    // Resolve and verify every claimed contract digest against the published version.
    let mut contract_ids = Vec::new();
    for c in &req.implements {
        let r = sqlx::query("SELECT id, digest FROM contracts WHERE name=$1 AND version=$2")
            .bind(&c.contract)
            .bind(&c.version)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .ok_or_else(|| {
                Problem::unprocessable(
                    "contract_not_published",
                    format!("{}@{} is not published", c.contract, c.version),
                )
            })?;
        if r.get::<String, _>("digest") != c.digest {
            return Err(Problem::unprocessable(
                "contract_digest_mismatch",
                format!(
                    "digest for {}@{} does not match the published version",
                    c.contract, c.version
                ),
            ));
        }
        contract_ids.push((r.get::<Uuid, _>("id"), c.digest.clone()));
    }

    let existing =
        sqlx::query("SELECT id, owner_identity FROM implementations WHERE slug=$1 FOR UPDATE")
            .bind(&req.slug)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
    let impl_id: Uuid = match existing {
        Some(r) => {
            if r.get::<String, _>("owner_identity") != caller.subject() && !caller.is_operator(&st) {
                return Err(Problem::forbidden("implementation is owned by another identity"));
            }
            let id: Uuid = r.get("id");
            sqlx::query("UPDATE implementations SET release_version=$2, updated_at=now() WHERE id=$1").bind(id).bind(&req.release_version).execute(&mut *tx).await.map_err(db)?;
            id
        }
        None => sqlx::query("INSERT INTO implementations (slug, release_version, owner_identity) VALUES ($1,$2,$3) RETURNING id")
            .bind(&req.slug)
            .bind(&req.release_version)
            .bind(caller.subject())
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?
            .get("id"),
    };

    // Replace the implemented set; refuse to drop a contract version that is currently bound.
    let keep: Vec<Uuid> = contract_ids.iter().map(|c| c.0).collect();
    let dropped_bound = sqlx::query(
        "SELECT 1 FROM implementation_contracts ic
           JOIN contracts c ON c.id = ic.contract_id
           JOIN contract_bindings b ON b.implementation_id = ic.implementation_id AND b.contract_name = c.name AND b.major_version = c.major
          WHERE ic.implementation_id=$1 AND NOT (ic.contract_id = ANY($2))
            AND NOT EXISTS (SELECT 1 FROM implementation_contracts ic2 JOIN contracts c2 ON c2.id=ic2.contract_id
                             WHERE ic2.implementation_id=$1 AND ic2.contract_id = ANY($2) AND c2.name=c.name AND c2.major=c.major)
          LIMIT 1",
    )
    .bind(impl_id)
    .bind(&keep)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    if dropped_bound.is_some() {
        return Err(Problem::conflict(
            "implementation_bound",
            "cannot stop implementing a contract major that is bound to an application; unbind first",
        ));
    }
    sqlx::query("DELETE FROM implementation_contracts WHERE implementation_id=$1 AND NOT (contract_id = ANY($2))").bind(impl_id).bind(&keep).execute(&mut *tx).await.map_err(db)?;
    for (cid, digest) in &contract_ids {
        sqlx::query("INSERT INTO implementation_contracts (implementation_id, contract_id, contract_digest, verification_status) VALUES ($1,$2,$3,'claimed') ON CONFLICT DO NOTHING")
            .bind(impl_id)
            .bind(cid)
            .bind(digest)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
    }
    let mut dep_ids = Vec::new();
    for d in &req.deployments {
        dep_ids.push(upsert_deployment(&st, &mut tx, impl_id, d).await?);
    }
    audit::record(
        &mut tx,
        &caller,
        "implementation.register",
        &format!("implementation:{impl_id}"),
        None,
        Some(json!({"slug": req.slug, "release": req.release_version})),
    )
    .await?;
    tx.commit().await.map_err(db)?;

    for id in dep_ids {
        let st = st.clone();
        tokio::spawn(async move { health::probe_one(&st, id).await });
    }
    Ok((
        StatusCode::OK,
        Json(implementation_view(&st, impl_id).await?),
    ))
}

async fn owner_or_operator(st: &AppState, caller: &Caller, impl_id: Uuid) -> ApiResult<()> {
    let r = sqlx::query("SELECT owner_identity FROM implementations WHERE id=$1")
        .bind(impl_id)
        .fetch_optional(&st.pool)
        .await
        .map_err(db)?;
    match r {
        Some(r)
            if r.get::<String, _>("owner_identity") == caller.subject()
                || caller.is_operator(st) =>
        {
            Ok(())
        }
        // Same answer for "missing" and "not yours".
        _ => Err(Problem::forbidden("not permitted for this implementation")),
    }
}

pub async fn get(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    owner_or_operator(&st, &caller, id).await?;
    Ok(Json(implementation_view(&st, id).await?))
}

pub async fn list(State(st): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let ids: Vec<Uuid> = sqlx::query("SELECT id FROM implementations ORDER BY slug LIMIT 500")
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .iter()
        .map(|r| r.get("id"))
        .collect();
    let mut items = Vec::new();
    for id in ids {
        items.push(implementation_view(&st, id).await?);
    }
    Ok(Json(json!({"items": items})))
}

#[derive(Deserialize)]
pub struct PatchReq {
    pub status: Option<String>,
    pub release_version: Option<String>,
}

pub async fn patch(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
    ApiJson(p): ApiJson<PatchReq>,
) -> ApiResult<Json<Value>> {
    owner_or_operator(&st, &caller, id).await?;
    if let Some(s) = &p.status
        && !matches!(s.as_str(), "active" | "disabled")
    {
        return Err(Problem::bad_request(
            "invalid_status",
            "status must be active or disabled",
        ));
    }
    if let Some(v) = &p.release_version {
        Version::parse(v).map_err(|_| {
            Problem::bad_request("invalid_release_version", "release_version must be semver")
        })?;
    }
    let mut tx = st.pool.begin().await.map_err(db)?;
    sqlx::query("UPDATE implementations SET status=COALESCE($2,status), release_version=COALESCE($3,release_version), updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(&p.status)
        .bind(&p.release_version)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    audit::record(
        &mut tx,
        &caller,
        "implementation.update",
        &format!("implementation:{id}"),
        None,
        Some(json!({"status": p.status, "release_version": p.release_version})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(Json(implementation_view(&st, id).await?))
}

pub async fn add_deployment(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
    ApiJson(d): ApiJson<DeploymentSpec>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    owner_or_operator(&st, &caller, id).await?;
    let mut tx = st.pool.begin().await.map_err(db)?;
    let dep = upsert_deployment(&st, &mut tx, id, &d).await?;
    audit::record(
        &mut tx,
        &caller,
        "deployment.register",
        &format!("deployment:{dep}"),
        None,
        Some(json!({"environment": d.environment})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    let st2 = st.clone();
    tokio::spawn(async move { health::probe_one(&st2, dep).await });
    Ok((StatusCode::OK, Json(implementation_view(&st, id).await?)))
}

pub async fn delete_deployment(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let row = sqlx::query("SELECT implementation_id, environment FROM deployments WHERE id=$1")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .map_err(db)?;
    let Some(row) = row else {
        return Err(Problem::forbidden("not permitted for this deployment"));
    };
    let (impl_id, env): (Uuid, String) = (row.get("implementation_id"), row.get("environment"));
    owner_or_operator(&st, &caller, impl_id).await?;
    let bound = sqlx::query(
        "SELECT 1 FROM contract_bindings WHERE implementation_id=$1 AND environment=$2 LIMIT 1",
    )
    .bind(impl_id)
    .bind(&env)
    .fetch_optional(&st.pool)
    .await
    .map_err(db)?;
    if bound.is_some() {
        return Err(Problem::conflict(
            "deployment_bound",
            "deployment is bound to an application; unbind or disable the implementation instead",
        ));
    }
    let mut tx = st.pool.begin().await.map_err(db)?;
    sqlx::query("DELETE FROM deployments WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    audit::record(
        &mut tx,
        &caller,
        "deployment.delete",
        &format!("deployment:{id}"),
        None,
        None,
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}
