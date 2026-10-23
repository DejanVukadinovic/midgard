use axum::{Json, extract::State};
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, Row};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    authn::{ApiJson, Caller},
    common::{
        active_grant, app_for_client, require_active, resolve_user_context, valid_contract_name,
        valid_org_id,
    },
    error::db,
    routes::identities::{matches_pattern, patterns_for},
    state::AppState,
};

#[derive(Deserialize)]
pub struct AuthorizeReq {
    /// Client id taken from the *verified* workload token by the implementation.
    pub client_id: String,
    pub contract: String,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub user_token: Option<String>,
    #[serde(default)]
    pub organization_id: Option<String>,
}

/// Called by an implementation (authenticated as its approved deployment identity) to learn
/// whether a caller application may use this contract on it, and optionally the tenant context
/// of a delegated user. Possession of a resolved endpoint is never authorization by itself.
pub async fn authorize(
    State(st): State<AppState>,
    caller: Caller,
    ApiJson(r): ApiJson<AuthorizeReq>,
) -> ApiResult<Json<Value>> {
    let patterns = patterns_for(&st, caller.subject()).await?;
    if patterns.is_empty() {
        return Err(Problem::forbidden("not an approved deployment identity"));
    }
    if !valid_contract_name(&r.contract)
        || r.organization_id
            .as_deref()
            .is_some_and(|o| !valid_org_id(o))
    {
        return Err(Problem::bad_request(
            "invalid_request",
            "invalid contract or organization",
        ));
    }
    let deny =
        || Problem::forbidden("caller is not authorized for this contract on this implementation");
    let app = app_for_client(&st.pool, &r.client_id)
        .await?
        .ok_or_else(deny)?;
    require_active(&app).map_err(|_| deny())?;
    let permissions = active_grant(&st.pool, app.id, &r.contract)
        .await?
        .ok_or_else(deny)?;
    let env = r
        .environment
        .clone()
        .unwrap_or_else(|| st.cfg.default_environment.clone());
    let slugs: Vec<String> = sqlx::query("SELECT i.slug FROM contract_bindings b JOIN implementations i ON i.id=b.implementation_id WHERE b.application_id=$1 AND b.contract_name=$2 AND b.environment=$3 AND i.status='active'")
        .bind(app.id)
        .bind(&r.contract)
        .bind(&env)
        .fetch_all(&st.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(|x| x.get("slug"))
        .collect();
    if !slugs
        .iter()
        .any(|s| patterns.iter().any(|p| matches_pattern(p, s)))
    {
        return Err(deny());
    }
    let tenant = match &r.user_token {
        Some(t) => Some(resolve_user_context(&st, &app, t, r.organization_id.as_deref()).await?),
        None => None,
    };
    Ok(Json(json!({
        "application_id": app.id,
        "application": app.slug,
        "permissions": permissions,
        "tenant": tenant,
    })))
}
