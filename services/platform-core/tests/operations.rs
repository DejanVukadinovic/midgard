mod common;

use axum::http::StatusCode;
use axum::{Json, Router, extract::State, routing::post};
use common::*;
use platform_core::provisioning::{Provisioner, ZitadelClient};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

// OSS-15 / ENV-03: a fresh core process (new pool, new state) sees all registrations
#[tokio::test]
async fn state_survives_core_restart() {
    let env = Env::new().await;
    let (s, a) = env
        .register_app(
            "obroklik",
            "p-obroklik",
            "obroklik-web",
            "obroklik-be",
            "ka",
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let c = env.publish_contract("1.0.0").await;
    env.allow_deployer("deployer-1", "example-*").await;
    let svc = fake_service().await;
    let (_, v) = env
        .register_impl(
            "example-capability",
            c["digest"].as_str().unwrap(),
            "1.0.0",
            &svc.url,
        )
        .await;
    env.wait_deployment(v["id"].as_str().unwrap(), "healthy")
        .await;
    let app_id = a["application"]["id"].as_str().unwrap();
    env.op(
        "POST",
        &format!("/v0/applications/{app_id}/contract-grants"),
        Some(json!({"contract":"platform.example","permissions":["example:read"]})),
    )
    .await;
    env.op(
        "PUT",
        &format!("/v0/applications/{app_id}/bindings/platform.example"),
        Some(json!({"major":1,"environment":"local","implementation":"example-capability"})),
    )
    .await;

    // "restart": brand-new pool + state + server on the same database
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&env.cfg.database_url)
        .await
        .unwrap();
    let st2 = Env::state(&env.cfg, pool, &env.idp, Provisioner::Link);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base2 = format!("http://{}", l.local_addr().unwrap());
    let app = platform_core::router(st2);
    tokio::spawn(async move { axum::serve(l, app).await.ok() });

    let r = env
        .http
        .post(format!("{base2}/v0/contracts/resolve"))
        .bearer_auth(env.token("obroklik-be"))
        .json(&json!({"contract":"platform.example","version_constraint":">=1.0.0 <2.0.0"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!(
        body["endpoint"], svc.url,
        "healthy status and bindings persisted across restart"
    );
    drop_db(env).await;
}

// APP-06 / ENV-06: IdP outage leaves a visible failed operation; retry with the same key completes
#[tokio::test]
async fn provisioning_failure_is_visible_and_retry_safe() {
    // reserve a port, leave it closed (outage), then bring the fake ZITADEL up on it
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let url = format!("http://{addr}");

    let env = Env::with(
        |_| {},
        Provisioner::Zitadel(ZitadelClient::new(&url, "t".into(), None)),
    )
    .await;
    let body =
        json!({"slug":"obroklik","display_name":"Obroklik","oauth_clients":[{"kind":"backend"}]});
    let t = env.token("op-client");
    let (s, v) = env
        .call(
            "POST",
            "/v0/applications",
            Some(&t),
            Some(body.clone()),
            &[("idempotency-key", "retry-1")],
        )
        .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "{v}");
    assert_eq!(v["code"], "provisioning_failed");
    // no apparently-active partial registration
    let (_, apps) = env.op("GET", "/v0/applications", None).await;
    assert_eq!(apps["items"][0]["status"], "pending");
    let op: String = sqlx::query_scalar(
        "SELECT id::text FROM provisioning_operations WHERE idempotency_key='retry-1'",
    )
    .fetch_one(&env.owner_pool)
    .await
    .unwrap();
    let (_, opv) = env.op("GET", &format!("/v0/operations/{op}"), None).await;
    assert_eq!(opv["status"], "failed");

    // IdP recovers
    #[derive(Default)]
    struct F {
        creates: u32,
    }
    let st = Arc::new(Mutex::new(F::default()));
    let app = Router::new()
        .route(
            "/management/v1/projects/_search",
            post(|| async { Json(json!({"result": []})) }),
        )
        .route(
            "/management/v1/projects",
            post(|State(s): State<Arc<Mutex<F>>>| async move {
                s.lock().unwrap().creates += 1;
                Json(json!({"id":"p-obroklik"}))
            }),
        )
        .route(
            "/management/v1/projects/{p}/apps/_search",
            post(|| async { Json(json!({"result": []})) }),
        )
        .route(
            "/management/v1/projects/{p}/apps/api",
            post(|State(s): State<Arc<Mutex<F>>>| async move {
                s.lock().unwrap().creates += 1;
                Json(json!({"clientId":"obroklik-be"}))
            }),
        )
        .with_state(st.clone());
    let l = tokio::net::TcpListener::bind(addr).await.unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.ok() });

    let (s, v) = env
        .call(
            "POST",
            "/v0/applications",
            Some(&t),
            Some(body),
            &[("idempotency-key", "retry-1")],
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["application"]["status"], "active");
    assert_eq!(v["application"]["zitadel_project_id"], "p-obroklik");
    let (_, apps) = env.op("GET", "/v0/applications", None).await;
    assert_eq!(
        apps["items"].as_array().unwrap().len(),
        1,
        "no duplicate application on retry"
    );
    drop_db(env).await;
}

// audit_log is append-only for every role
#[tokio::test]
async fn audit_log_is_append_only() {
    let env = Env::new().await;
    env.register_app("fleet", "p-fleet", "fleet-web", "fleet-be", "kf")
        .await;
    for role_pool in [&env.owner_pool, &env.st.pool] {
        assert!(
            sqlx::query("UPDATE audit_log SET actor='x'")
                .execute(role_pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM audit_log")
                .execute(role_pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("TRUNCATE audit_log")
                .execute(role_pool)
                .await
                .is_err()
        );
    }
    drop_db(env).await;
}

// /v0/authorize: an implementation learns whether a caller may use it (RES-02, tenant path)
#[tokio::test]
async fn implementation_side_authorization() {
    let env = Env::new().await;
    let (_, a) = env
        .register_app(
            "obroklik",
            "p-obroklik",
            "obroklik-web",
            "obroklik-be",
            "ka",
        )
        .await;
    env.register_app("fleet", "p-fleet", "fleet-web", "fleet-be", "kf")
        .await;
    let app_id = a["application"]["id"].as_str().unwrap();
    let c = env.publish_contract("1.0.0").await;
    env.allow_deployer("deployer-1", "example-*").await;
    let svc = fake_service().await;
    env.register_impl(
        "example-capability",
        c["digest"].as_str().unwrap(),
        "1.0.0",
        &svc.url,
    )
    .await;
    env.op(
        "POST",
        &format!("/v0/applications/{app_id}/contract-grants"),
        Some(json!({"contract":"platform.example","permissions":["example:read","example:write"]})),
    )
    .await;
    env.op(
        "PUT",
        &format!("/v0/applications/{app_id}/bindings/platform.example"),
        Some(json!({"major":1,"environment":"local","implementation":"example-capability"})),
    )
    .await;
    env.op(
        "POST",
        &format!("/v0/organizations/orgA/applications/{app_id}/tenants"),
        None,
    )
    .await;

    let dt = env.token("deployer-client");
    let (s, v) = env
        .call(
            "POST",
            "/v0/authorize",
            Some(&dt),
            Some(json!({"client_id":"obroklik-be","contract":"platform.example"})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["permissions"], json!(["example:read", "example:write"]));
    assert!(v["tenant"].is_null());

    // ungranted client and unrelated identities are denied
    let (s, _) = env
        .call(
            "POST",
            "/v0/authorize",
            Some(&dt),
            Some(json!({"client_id":"fleet-be","contract":"platform.example"})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = env
        .call(
            "POST",
            "/v0/authorize",
            Some(&env.token("fleet-be")),
            Some(json!({"client_id":"obroklik-be","contract":"platform.example"})),
            &[],
        )
        .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "only approved deployment identities may ask"
    );
    let (s, _) = env
        .call(
            "POST",
            "/v0/authorize",
            Some(&env.token("deployer2-client")),
            Some(json!({"client_id":"obroklik-be","contract":"platform.example"})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "identity without approved slugs");

    // delegated tenant through the same endpoint
    let user = env.user_token("alice", "obroklik-web", &["p-obroklik"]);
    let (s, v) = env
        .call(
            "POST",
            "/v0/authorize",
            Some(&dt),
            Some(
                json!({"client_id":"obroklik-be","contract":"platform.example","user_token":user}),
            ),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["tenant"]["organization_id"], "orgA");
    let (s, _) = env.call("POST", "/v0/authorize", Some(&dt), Some(json!({"client_id":"obroklik-be","contract":"platform.example","user_token":user,"organization_id":"orgB"})), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    drop_db(env).await;
}
