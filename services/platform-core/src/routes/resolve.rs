use axum::{Json, extract::State};
use platform_contracts::parse_constraint;
use platform_http::{ApiResult, Problem};
use platform_persistence::sqlx::{self, Row};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    authn::{ApiJson, Caller},
    common::{active_grant, require_active, valid_contract_name, workload_app},
    error::db,
    state::{AppState, Metrics},
};

#[derive(Deserialize)]
pub struct ResolveReq {
    pub contract: String,
    pub version_constraint: String,
    #[serde(default)]
    pub environment: Option<String>,
}

pub async fn resolve(
    State(st): State<AppState>,
    caller: Caller,
    ApiJson(req): ApiJson<ResolveReq>,
) -> ApiResult<Json<Value>> {
    let out = resolve_inner(&st, &caller, req).await;
    match &out {
        Ok(_) => Metrics::inc(&st.metrics.resolve_ok),
        Err(_) => Metrics::inc(&st.metrics.resolve_denied),
    }
    out
}

async fn resolve_inner(st: &AppState, caller: &Caller, req: ResolveReq) -> ApiResult<Json<Value>> {
    let constraint = parse_constraint(&req.version_constraint)
        .map_err(|e| Problem::bad_request("invalid_version_constraint", e.to_string()))?;
    if !valid_contract_name(&req.contract) {
        return Err(Problem::bad_request(
            "invalid_contract",
            "invalid contract name",
        ));
    }
    let env = req
        .environment
        .clone()
        .unwrap_or_else(|| st.cfg.default_environment.clone());

    // 1. identity: from the verified token only.
    let app = workload_app(&st.pool, caller).await?;
    require_active(&app)?;
    // 2. declared requirement + 3. explicit grant. One uniform denial: no contract existence leak.
    let declared = sqlx::query(
        "SELECT 1 FROM application_requirements WHERE application_id=$1 AND contract_name=$2",
    )
    .bind(app.id)
    .bind(&req.contract)
    .fetch_optional(&st.pool)
    .await
    .map_err(db)?;
    let permissions = active_grant(&st.pool, app.id, &req.contract).await?;
    let (Some(_), Some(permissions)) = (declared, permissions) else {
        return Err(Problem::forbidden(
            "access to this contract is not granted to the application",
        ));
    };

    // 4. explicit binding(s) for this app/contract/environment; prefer the highest compatible major.
    let bindings = sqlx::query(
        "SELECT b.major_version, b.implementation_id, i.status AS impl_status
           FROM contract_bindings b JOIN implementations i ON i.id = b.implementation_id
          WHERE b.application_id=$1 AND b.contract_name=$2 AND b.environment=$3
          ORDER BY b.major_version DESC",
    )
    .bind(app.id)
    .bind(&req.contract)
    .bind(&env)
    .fetch_all(&st.pool)
    .await
    .map_err(db)?;
    if bindings.is_empty() {
        return Err(Problem::unprocessable(
            "binding_not_found",
            "no implementation is bound for this contract and environment",
        ));
    }
    let mut chosen: Option<(Uuid, String, String, String)> = None; // impl_id, impl_status, version, protocol
    'outer: for b in &bindings {
        let (major, impl_id): (i32, Uuid) = (b.get("major_version"), b.get("implementation_id"));
        let rows = sqlx::query(
            "SELECT c.version, c.manifest->>'protocol' AS protocol FROM implementation_contracts ic JOIN contracts c ON c.id = ic.contract_id
              WHERE ic.implementation_id=$1 AND c.name=$2 AND c.major=$3 ORDER BY c.minor DESC, c.patch DESC",
        )
        .bind(impl_id)
        .bind(&req.contract)
        .bind(major)
        .fetch_all(&st.pool)
        .await
        .map_err(db)?;
        for r in rows {
            let v: String = r.get("version");
            if semver::Version::parse(&v).is_ok_and(|pv| constraint.matches(&pv)) {
                chosen = Some((
                    impl_id,
                    b.get("impl_status"),
                    v,
                    r.get::<Option<String>, _>("protocol")
                        .unwrap_or_else(|| "http".into()),
                ));
                break 'outer;
            }
        }
    }
    let Some((impl_id, impl_status, version, protocol)) = chosen else {
        return Err(Problem::unprocessable(
            "no_compatible_version",
            "no bound implementation satisfies the requested version constraint",
        ));
    };
    // 5. lifecycle: only active + healthy resolves; never fail over to another implementation.
    if impl_status != "active" {
        return Err(Problem::unavailable(
            "no_healthy_deployment",
            "bound implementation is disabled",
        ));
    }
    let dep = sqlx::query("SELECT logical_endpoint, status FROM deployments WHERE implementation_id=$1 AND environment=$2")
        .bind(impl_id)
        .bind(&env)
        .fetch_optional(&st.pool)
        .await
        .map_err(db)?;
    let endpoint = match dep {
        Some(d) if d.get::<String, _>("status") == "healthy" => {
            d.get::<String, _>("logical_endpoint")
        }
        _ => {
            return Err(Problem::unavailable(
                "no_healthy_deployment",
                "bound implementation has no healthy deployment in this environment",
            ));
        }
    };
    Ok(Json(json!({
        "contract": req.contract,
        "resolved_version": version,
        "protocol": protocol,
        "endpoint": endpoint,
        "permissions": permissions,
        "expires_in_seconds": st.cfg.resolution_ttl_secs,
    })))
}
