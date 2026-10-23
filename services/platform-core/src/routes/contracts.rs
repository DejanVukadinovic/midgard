use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use platform_contracts::{ContractBundle, Manifest, compat};
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, Row};
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    audit,
    authn::{ApiJson, Caller},
    common::{active_grant, app_for_client},
    error::db,
    routes::identities::patterns_for,
    state::AppState,
};

#[derive(Deserialize)]
pub struct PublishReq {
    pub manifest: Manifest,
    pub openapi: String,
    #[serde(default)]
    pub semantics: String,
    /// Optional client-computed digest; must match the server-computed one.
    #[serde(default)]
    pub digest: Option<String>,
}

pub async fn publish(
    State(st): State<AppState>,
    caller: Caller,
    ApiJson(req): ApiJson<PublishReq>,
) -> ApiResult<Response> {
    caller.require_publisher(&st, &req.manifest.id)?;
    let bundle = ContractBundle::parse(req.manifest, req.openapi, req.semantics)
        .map_err(|e| Problem::bad_request("invalid_contract", e.to_string()))?;
    let digest = bundle.digest();
    if req.digest.as_deref().is_some_and(|d| d != digest) {
        return Err(Problem::bad_request(
            "digest_mismatch",
            format!("computed digest is {digest}"),
        ));
    }
    let m = &bundle.manifest;
    let version = m.version.to_string();

    let mut tx = st.pool.begin().await.map_err(db)?;
    // Serialize concurrent publishers of the same contract name.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&m.id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    if let Some(r) = sqlx::query("SELECT digest FROM contracts WHERE name=$1 AND version=$2")
        .bind(&m.id)
        .bind(&version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
    {
        return if r.get::<String, _>("digest") == digest {
            Ok((StatusCode::OK, Json(json!({"contract": m.id, "version": version, "digest": digest, "created": false}))).into_response())
        } else {
            Err(Problem::conflict(
                "contract_version_conflict",
                "this contract version is already published with different content; publish a new version",
            ))
        };
    }
    // Backward compatibility within the same major against the closest lower published version.
    let prev = sqlx::query("SELECT version, openapi FROM contracts WHERE name=$1 AND major=$2 AND (minor, patch) < ($3, $4) ORDER BY minor DESC, patch DESC LIMIT 1")
        .bind(&m.id)
        .bind(m.version.major as i32)
        .bind(m.version.minor as i32)
        .bind(m.version.patch as i32)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
    if let Some(p) = prev {
        let old: Value = serde_yaml::from_str(&p.get::<String, _>("openapi"))
            .map_err(platform_http::internal_error)?;
        let breaking = compat::breaking_changes(&old, &bundle.openapi_doc);
        if !breaking.is_empty() {
            return Err(Problem::unprocessable(
                "breaking_change_within_major",
                format!(
                    "breaking changes relative to {}: {}; publish a new major version",
                    p.get::<String, _>("version"),
                    breaking.join("; ")
                ),
            ));
        }
    }
    sqlx::query(
        "INSERT INTO contracts (name, version, major, minor, patch, digest, manifest, openapi, semantics, permissions, published_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(&m.id)
    .bind(&version)
    .bind(m.version.major as i32)
    .bind(m.version.minor as i32)
    .bind(m.version.patch as i32)
    .bind(&digest)
    .bind(serde_json::to_value(m).map_err(platform_http::internal_error)?)
    .bind(&bundle.openapi)
    .bind(&bundle.semantics)
    .bind(&m.permissions)
    .bind(caller.subject())
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    audit::record(
        &mut tx,
        &caller,
        "contract.publish",
        &format!("contract:{}@{}", m.id, version),
        None,
        Some(json!({"digest": digest})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"contract": m.id, "version": version, "digest": digest, "created": true})),
    )
        .into_response())
}

/// Readers: operators, publishers, approved deployment identities, and applications holding
/// an active grant for the contract.
pub async fn get_version(
    State(st): State<AppState>,
    caller: Caller,
    Path((name, version)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let allowed = if caller.is_operator(&st)
        || st.cfg.publishers.contains_key(caller.subject())
        || !patterns_for(&st, caller.subject()).await?.is_empty()
    {
        true
    } else if let Some(cid) = caller.token.client_id.as_deref() {
        match app_for_client(&st.pool, cid).await? {
            Some(app) if app.status == "active" => {
                active_grant(&st.pool, app.id, &name).await?.is_some()
            }
            _ => false,
        }
    } else {
        false
    };
    if !allowed {
        // Same answer as a missing contract: do not reveal existence.
        return Err(Problem::not_found("contract version not found"));
    }
    Version::parse(&version).map_err(|_| {
        Problem::bad_request("invalid_version", "version must be MAJOR.MINOR.PATCH")
    })?;
    let r = sqlx::query("SELECT name, version, digest, manifest, openapi, semantics, permissions, status FROM contracts WHERE name=$1 AND version=$2")
        .bind(&name)
        .bind(&version)
        .fetch_optional(&st.pool)
        .await
        .map_err(db)?
        .ok_or_else(|| Problem::not_found("contract version not found"))?;
    Ok(Json(json!({
        "contract": r.get::<String,_>("name"), "version": r.get::<String,_>("version"), "digest": r.get::<String,_>("digest"),
        "manifest": r.get::<Value,_>("manifest"), "openapi": r.get::<String,_>("openapi"), "semantics": r.get::<String,_>("semantics"),
        "permissions": r.get::<Vec<String>,_>("permissions"), "status": r.get::<String,_>("status"),
    })))
}
