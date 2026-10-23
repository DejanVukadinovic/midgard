//! MIDGARD core: application registry, contract registry, implementation registry, grants,
//! bindings and the contract resolver.

pub mod audit;
pub mod authn;
pub mod common;
pub mod config;
pub mod endpoint_policy;
pub mod error;
pub mod health;
pub mod provisioning;
pub mod ratelimit;
pub mod routes;
pub mod state;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::HeaderName,
    middleware,
    routing::{delete, get, post, put},
};
use platform_http::REQUEST_ID_HEADER;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use routes::{
    applications as app, authorize, contracts, grants, identities, implementations as imp, ops,
    resolve, tenants,
};
use state::AppState;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub fn router(st: AppState) -> Router {
    let admin = Router::new()
        .route("/v0/applications", post(app::register).get(app::list))
        .route("/v0/applications/{id}", get(app::get).patch(app::patch))
        .route("/v0/applications/{id}/suspend", post(app::suspend))
        .route("/v0/applications/{id}/activate", post(app::activate))
        .route(
            "/v0/applications/{id}/contract-grants",
            post(grants::create_grant).get(grants::list_grants),
        )
        .route(
            "/v0/applications/{id}/contract-grants/{grant_id}",
            delete(grants::revoke_grant),
        )
        .route(
            "/v0/applications/{id}/bindings/{contract}",
            put(grants::put_binding).delete(grants::delete_binding),
        )
        .route(
            "/v0/organizations/{org_id}/applications/{app_id}/tenants",
            post(tenants::create),
        )
        .route("/v0/tenants/{id}", get(tenants::get))
        .route("/v0/tenants/{id}/suspend", post(tenants::suspend))
        .route("/v0/tenants/{id}/activate", post(tenants::activate))
        .route(
            "/v0/contracts",
            post(contracts::publish).layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .route(
            "/v0/capability-contracts",
            post(contracts::publish_v02).layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route("/v0/implementations", post(imp::register).get(imp::list))
        .route("/v0/implementations/{id}", get(imp::get).patch(imp::patch))
        .route(
            "/v0/implementations/{id}/deployments",
            post(imp::add_deployment),
        )
        .route("/v0/deployments/{id}", delete(imp::delete_deployment))
        .route(
            "/v0/deployment-identities",
            post(identities::create).get(identities::list),
        )
        .route("/v0/deployment-identities/{id}", delete(identities::revoke))
        .route("/v0/operations/{id}", get(ops::get_operation))
        .route("/v0/audit", get(ops::audit_list))
        .layer(middleware::from_fn_with_state(
            st.clone(),
            ratelimit::limit_admin,
        ));

    let rid = HeaderName::from_static(REQUEST_ID_HEADER);
    Router::new()
        .merge(admin)
        .route(
            "/v0/contracts/{name}/versions/{version}",
            get(contracts::get_version),
        )
        .route(
            "/v0/contracts/{name}/versions/{version}/interfaces/{interface}/document",
            get(contracts::get_interface_document),
        )
        .route("/v0/contracts/resolve", post(resolve::resolve))
        .route("/v0/tenants/resolve", post(tenants::resolve))
        .route("/v0/authorize", post(authorize::authorize))
        .route("/health/live", get(ops::live))
        .route("/health/ready", get(ops::ready))
        .route("/metrics", get(ops::metrics))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(15),
        ))
        .layer(PropagateRequestIdLayer::new(rid.clone()))
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::new(rid, MakeRequestUuid))
        .with_state(st)
}
