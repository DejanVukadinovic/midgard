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
    /// v0.2: the interface being invoked. When set, the contract's interface and capability
    /// authorization (permission, caller kinds) are enforced for this invocation.
    #[serde(default)]
    pub interface: Option<String>,
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
    if let Some(iface) = &r.interface {
        let row = sqlx::query(
            "SELECT ci.callers, c.manifest FROM contract_interfaces ci JOIN contracts c ON c.id=ci.contract_id
              JOIN implementation_interfaces ii ON ii.contract_id=ci.contract_id AND ii.interface_id=ci.interface_id
              JOIN implementations i ON i.id=ii.implementation_id AND i.slug = ANY($4)
              JOIN contract_bindings b ON b.implementation_id=i.id AND b.application_id=$5 AND b.environment=$3
             WHERE c.name=$1 AND ci.interface_id=$2 AND b.major_version=c.major
             LIMIT 1",
        )
        .bind(&r.contract)
        .bind(iface)
        .bind(&env)
        .bind(&slugs)
        .bind(app.id)
        .fetch_optional(&st.pool)
        .await
        .map_err(db)?
        .ok_or_else(deny)?;
        let callers: Vec<String> = row.get("callers");
        let kind = if r.user_token.is_some() {
            "user"
        } else {
            "service"
        };
        let manifest: Value = row.get("manifest");
        let required = manifest["authorization"]["permission"]
            .as_str()
            .unwrap_or_default();
        if !callers.iter().any(|c| c == kind) || !permissions.iter().any(|p| p == required) {
            return Err(deny());
        }
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
