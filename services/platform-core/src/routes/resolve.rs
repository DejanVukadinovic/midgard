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
    let mut chosen: Option<(Uuid, String, String, String, Uuid, String, Value)> = None; // impl_id, impl_status, version, protocol, contract_id, digest, manifest
    'outer: for b in &bindings {
        let (major, impl_id): (i32, Uuid) = (b.get("major_version"), b.get("implementation_id"));
        let rows = sqlx::query(
            "SELECT c.id, c.version, c.digest, c.manifest, c.manifest->>'protocol' AS protocol FROM implementation_contracts ic JOIN contracts c ON c.id = ic.contract_id
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
                    r.get("id"),
                    r.get("digest"),
                    r.get("manifest"),
                ));
                break 'outer;
            }
        }
    }
    let Some((impl_id, impl_status, version, protocol, contract_id, digest, manifest)) = chosen
    else {
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
    let mut out = json!({
        "contract": req.contract,
        "resolved_version": version,
        "protocol": protocol,
        "endpoint": endpoint,
        "permissions": permissions,
        "expires_in_seconds": st.cfg.resolution_ttl_secs,
    });
    if manifest.get("midgard").and_then(Value::as_str) == Some("0.2") {
        v02_details(
            st,
            &mut out,
            &req.contract,
            &version,
            impl_id,
            contract_id,
            &digest,
            &manifest,
            &permissions,
        )
        .await?;
    }
    Ok(Json(out))
}

/// v0.2 resolved contract: immutable identity + digest, the permission the provider will
/// enforce, and only the interfaces that the bound implementation advertises and that a
/// service-identity caller may use. Resolution is not authorization: the provider/broker
/// re-checks on every invocation.
#[allow(clippy::too_many_arguments)]
async fn v02_details(
    st: &AppState,
    out: &mut Value,
    contract: &str,
    version: &str,
    impl_id: Uuid,
    contract_id: Uuid,
    digest: &str,
    manifest: &Value,
    granted: &[String],
) -> ApiResult<()> {
    let required = manifest["authorization"]["permission"]
        .as_str()
        .unwrap_or_default();
    if !granted.iter().any(|p| p == required) {
        return Err(Problem::forbidden(
            "access to this contract is not granted to the application",
        ));
    }
    let rows = sqlx::query(
        "SELECT ci.interface_id, ci.protocol, ci.spec_type, ci.operation, ci.document_digest, ci.callers, ci.summary,
                ii.location, ii.destination, ii.broker_binding
           FROM contract_interfaces ci
           JOIN implementation_interfaces ii ON ii.contract_id = ci.contract_id AND ii.interface_id = ci.interface_id
          WHERE ci.contract_id=$1 AND ii.implementation_id=$2
          ORDER BY ci.interface_id",
    )
    .bind(contract_id)
    .bind(impl_id)
    .fetch_all(&st.pool)
    .await
    .map_err(db)?;
    // Workload tokens identify a service; user-delegated use is checked by the provider via /v0/authorize.
    let interfaces: Vec<Value> = rows
        .into_iter()
        .filter(|r| r.get::<Vec<String>, _>("callers").iter().any(|c| c == "service"))
        .map(|r| {
            let id: String = r.get("interface_id");
            let mut v = json!({
                "id": id,
                "protocol": r.get::<String,_>("protocol"),
                "operation": r.get::<String,_>("operation"),
                "callers": r.get::<Vec<String>,_>("callers"),
                "specification": {
                    "type": r.get::<String,_>("spec_type"),
                    "digest": r.get::<String,_>("document_digest"),
                    "summary": r.get::<Value,_>("summary"),
                    "document_path": format!("/v0/contracts/{contract}/versions/{version}/interfaces/{id}/document"),
                },
            });
            if let Some(l) = r.get::<Option<String>, _>("location") {
                v["location"] = json!(l);
            } else {
                v["destination"] = json!(r.get::<Option<String>, _>("destination"));
                v["broker_binding"] = json!(r.get::<Option<String>, _>("broker_binding"));
            }
            v
        })
        .collect();
    if interfaces.is_empty() {
        return Err(Problem::unprocessable(
            "no_authorized_interface",
            "the bound implementation advertises no interface this caller may use",
        ));
    }
    out["digest"] = json!(digest);
    out["interfaces"] = Value::Array(interfaces);
    out["authorization"] = json!({
        "permission": required,
        "context": manifest["authorization"]["context"],
        "delegation": manifest["authorization"]["delegation"],
    });
    out["execution"] = manifest["execution"].clone();
    Ok(())
}
