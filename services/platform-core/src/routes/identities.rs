use axum::{
    Json,
    extract::{Path, State},
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
    error::db,
    state::AppState,
};

#[derive(Deserialize)]
pub struct CreateReq {
    pub subject: String,
    pub slug_pattern: String,
}

pub async fn create(
    State(st): State<AppState>,
    caller: Caller,
    ApiJson(r): ApiJson<CreateReq>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    caller.require_operator(&st)?;
    let pat = r.slug_pattern.as_str();
    let stem = pat.strip_suffix('*').unwrap_or(pat);
    let ok = !r.subject.is_empty()
        && r.subject.len() <= 128
        && !stem.is_empty()
        && stem.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && stem
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(Problem::bad_request(
            "invalid_identity",
            "subject required; slug_pattern must be a slug or `prefix*`",
        ));
    }
    let mut tx = st.pool.begin().await.map_err(db)?;
    let row = sqlx::query(
        "INSERT INTO deployment_identities (subject, slug_pattern) VALUES ($1,$2)
         ON CONFLICT (subject, slug_pattern) DO UPDATE SET status='active' RETURNING id",
    )
    .bind(&r.subject)
    .bind(&r.slug_pattern)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;
    let id: Uuid = row.get("id");
    audit::record(
        &mut tx,
        &caller,
        "deployment_identity.grant",
        &format!("deployment_identity:{id}"),
        None,
        Some(json!({"subject": r.subject, "slug_pattern": r.slug_pattern})),
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id": id, "subject": r.subject, "slug_pattern": r.slug_pattern, "status": "active"}),
        ),
    ))
}

pub async fn list(State(st): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    caller.require_operator(&st)?;
    let items: Vec<Value> = sqlx::query("SELECT id, subject, slug_pattern, status FROM deployment_identities ORDER BY subject, slug_pattern")
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|r| json!({"id": r.get::<Uuid,_>("id"), "subject": r.get::<String,_>("subject"), "slug_pattern": r.get::<String,_>("slug_pattern"), "status": r.get::<String,_>("status")}))
        .collect();
    Ok(Json(json!({"items": items})))
}

pub async fn revoke(
    State(st): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    caller.require_operator(&st)?;
    let mut tx = st.pool.begin().await.map_err(db)?;
    let r = sqlx::query(
        "UPDATE deployment_identities SET status='revoked' WHERE id=$1 AND status='active'",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    if r.rows_affected() == 0 {
        return Err(Problem::not_found("active deployment identity not found"));
    }
    audit::record(
        &mut tx,
        &caller,
        "deployment_identity.revoke",
        &format!("deployment_identity:{id}"),
        None,
        None,
    )
    .await?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Does any active identity row for `subject` authorize `slug`?
pub async fn authorized_for_slug(
    st: &AppState,
    subject: &str,
    slug: &str,
) -> Result<bool, Problem> {
    let rows = sqlx::query(
        "SELECT slug_pattern FROM deployment_identities WHERE subject=$1 AND status='active'",
    )
    .bind(subject)
    .fetch_all(&st.pool)
    .await
    .map_err(db)?;
    Ok(rows
        .iter()
        .any(|r| matches_pattern(&r.get::<String, _>("slug_pattern"), slug)))
}

pub fn matches_pattern(pattern: &str, slug: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => slug.starts_with(prefix),
        None => pattern == slug,
    }
}

pub async fn patterns_for(st: &AppState, subject: &str) -> Result<Vec<String>, Problem> {
    Ok(sqlx::query(
        "SELECT slug_pattern FROM deployment_identities WHERE subject=$1 AND status='active'",
    )
    .bind(subject)
    .fetch_all(&st.pool)
    .await
    .map_err(db)?
    .into_iter()
    .map(|r| r.get("slug_pattern"))
    .collect())
}
