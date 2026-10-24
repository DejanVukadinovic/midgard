use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use platform_contracts::{
    ContractBundle, Manifest, compat,
    v02::{BundleV02, ManifestV02},
};
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, Row};
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

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

#[derive(Deserialize)]
pub struct PublishV02Req {
    /// The v0.2 manifest (see `contracts/schema/midgard-contract-v0.2.schema.json`).
    pub contract: ManifestV02,
    /// Interface documents referenced by `specification.document`, keyed by relative path.
    /// The server never fetches documents from URLs.
    #[serde(default)]
    pub documents: BTreeMap<String, String>,
    #[serde(default)]
    pub digest: Option<String>,
}

/// Publish a v0.2 capability contract: validate the manifest and its OpenAPI/AsyncAPI documents,
/// then store the normalized manifest and interfaces immutably. Idempotent for identical content.
pub async fn publish_v02(
    State(st): State<AppState>,
    caller: Caller,
    ApiJson(req): ApiJson<PublishV02Req>,
) -> ApiResult<Response> {
    let name = req.contract.contract_name();
    caller.require_publisher(&st, &name)?;
    let bundle = BundleV02::build(req.contract, &req.documents)
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
    let manifest_json = bundle.normalized_manifest();
    // `contracts.openapi` is NOT NULL (v0 column): keep the first HTTP document for v0 readers.
    let first_http = bundle
        .interfaces
        .iter()
        .find(|i| i.protocol == platform_contracts::v02::Protocol::Http)
        .map(|i| i.document.clone())
        .unwrap_or_default();

    let mut tx = st.pool.begin().await.map_err(db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&name)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    if let Some(r) = sqlx::query("SELECT digest FROM contracts WHERE name=$1 AND version=$2")
        .bind(&name)
        .bind(&version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
    {
        return if r.get::<String, _>("digest") == digest {
            Ok((StatusCode::OK, Json(json!({"contract": name, "version": version, "digest": digest, "created": false}))).into_response())
        } else {
            Err(Problem::conflict(
                "contract_version_conflict",
                "this contract version is already published with different content; publish a new version",
            ))
        };
    }
    let id: uuid::Uuid = sqlx::query(
        "INSERT INTO contracts (name, version, major, minor, patch, digest, manifest, openapi, semantics, permissions, published_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id",
    )
    .bind(&name)
    .bind(&version)
    .bind(m.version.major as i32)
    .bind(m.version.minor as i32)
    .bind(m.version.patch as i32)
    .bind(&digest)
    .bind(&manifest_json)
    .bind(&first_http)
    .bind(m.description.clone().unwrap_or_default())
    .bind(vec![m.authorization.permission.clone()])
    .bind(caller.subject())
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?
    .get("id");
    for i in &bundle.interfaces {
        let callers: Vec<String> = i
            .callers
            .iter()
            .map(|c| {
                serde_json::to_value(c)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default()
            })
            .collect();
        sqlx::query(
            "INSERT INTO contract_interfaces (contract_id, interface_id, protocol, spec_type, operation, document, document_digest, callers, summary)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(id)
        .bind(&i.id)
        .bind(i.protocol.as_str())
        .bind(i.spec_type.as_str())
        .bind(&i.operation)
        .bind(&i.document)
        .bind(&i.document_digest)
        .bind(&callers)
        .bind(&i.summary)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    }
    audit::record(
        &mut tx,
        &caller,
        "contract.publish",
        &format!("contract:{name}@{version}"),
        None,
        Some(json!({"digest": digest, "midgard": "0.2", "interfaces": bundle.interfaces.iter().map(|i| i.id.clone()).collect::<Vec<_>>()})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"contract": name, "version": version, "digest": digest, "created": true})),
    )
        .into_response())
}

/// The normalized interface document of a published v0.2 contract. Same readers as `get_version`.
pub async fn get_interface_document(
    State(st): State<AppState>,
    caller: Caller,
    Path((name, version, interface)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let Json(_) = get_version(
        State(st.clone()),
        caller,
        Path((name.clone(), version.clone())),
    )
    .await?;
    let r = sqlx::query(
        "SELECT i.document, i.document_digest FROM contract_interfaces i JOIN contracts c ON c.id=i.contract_id
          WHERE c.name=$1 AND c.version=$2 AND i.interface_id=$3",
    )
    .bind(&name)
    .bind(&version)
    .bind(&interface)
    .fetch_optional(&st.pool)
    .await
    .map_err(db)?
    .ok_or_else(|| Problem::not_found("interface not found"))?;
    Ok((
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/json".to_string(),
            ),
            (
                axum::http::header::ETAG,
                format!("\"{}\"", r.get::<String, _>("document_digest")),
            ),
        ],
        r.get::<String, _>("document"),
    )
        .into_response())
}
