mod common;

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

const RESOLVE: &str = "/v0/contracts/resolve";

fn rq() -> Value {
    json!({"contract": "platform.example", "version_constraint": ">=1.0.0 <2.0.0"})
}

async fn setup_apps(env: &Env) -> (String, String) {
    let (s, a) = env
        .register_app(
            "obroklik",
            "p-obroklik",
            "obroklik-web",
            "obroklik-be",
            "ka",
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let (s, f) = env
        .register_app("fleet", "p-fleet", "fleet-web", "fleet-be", "kf")
        .await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    (
        a["application"]["id"].as_str().unwrap().into(),
        f["application"]["id"].as_str().unwrap().into(),
    )
}

// SVC-01..06, SEC-04 (endpoint policy), RES-01..07, SVC-07..09, grant revocation (RES-04)
#[tokio::test]
async fn implementations_grants_bindings_and_resolution() {
    let env = Env::new().await;
    let (obroklik, fleet) = setup_apps(&env).await;
    let c = env.publish_contract("1.0.0").await;
    let digest = c["digest"].as_str().unwrap().to_string();
    let svc = fake_service().await;

    // SVC-02 unregistered workload / SVC-03 identity scoped to other slug
    let (s, _) = env
        .register_impl("example-capability", &digest, "1.0.0", &svc.url)
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "deployer not yet approved");
    env.allow_deployer("deployer-1", "example-*").await;
    let (s, _) = env
        .register_impl("other-service", &digest, "1.0.0", &svc.url)
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "identity limited to example-*");
    let (s, _) = env
        .call(
            "POST",
            "/v0/implementations",
            Some(&env.token("fleet-be")),
            Some(json!({"slug":"example-x","release_version":"1.0.0","implements":[]})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // SVC-04 SSRF: loopback IP, metadata, other schemes are rejected
    for bad in [
        "http://169.254.169.254/latest",
        "http://127.0.0.1:9",
        "file:///etc/passwd",
        "http://evil.example:80",
    ] {
        let (s, v) = env
            .register_impl("example-capability", &digest, "1.0.0", bad)
            .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {v}");
        assert_eq!(v["code"], "endpoint_rejected");
    }
    // CON-08 wrong digest
    let (s, v) = env
        .register_impl("example-capability", "sha256:deadbeef", "1.0.0", &svc.url)
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["code"], "contract_digest_mismatch");

    // SVC-01 registers at runtime
    let (s, v) = env
        .register_impl("example-capability", &digest, "1.0.0", &svc.url)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let impl_id = v["id"].as_str().unwrap().to_string();
    env.wait_deployment(&impl_id, "healthy").await;

    // another identity cannot take over the slug
    env.allow_deployer("deployer-2", "example-*").await;
    let (s, _) = env
        .call("POST", "/v0/implementations", Some(&env.token("deployer2-client")), Some(json!({"slug":"example-capability","release_version":"9.9.9","implements":[{"contract":"platform.example","version":"1.0.0","digest":digest}]})), &[])
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // RES-03: declared + not granted -> denied; granted but not bound -> explicit unresolved binding
    let obt = env.token("obroklik-be");
    let (s, _) = env.call("POST", RESOLVE, Some(&obt), Some(rq()), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, g) = env
        .op(
            "POST",
            &format!("/v0/applications/{obroklik}/contract-grants"),
            Some(json!({"contract":"platform.example","permissions":["example:read"]})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    let grant_id = g["id"].as_str().unwrap().to_string();
    let (s, v) = env.call("POST", RESOLVE, Some(&obt), Some(rq()), &[]).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(v["code"], "binding_not_found");
    // unknown permission cannot be granted
    let (s, _) = env
        .op(
            "POST",
            &format!("/v0/applications/{fleet}/contract-grants"),
            Some(json!({"contract":"platform.example","permissions":["admin:everything"]})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // RES-01
    let (s, b) = env
        .op(
            "PUT",
            &format!("/v0/applications/{obroklik}/bindings/platform.example"),
            Some(json!({"major":1,"environment":"local","implementation":"example-capability"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let (s, r) = env.call("POST", RESOLVE, Some(&obt), Some(rq()), &[]).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["endpoint"], svc.url);
    assert_eq!(r["resolved_version"], "1.0.0");
    assert_eq!(r["permissions"], json!(["example:read"]));
    assert_eq!(r["expires_in_seconds"], 300);

    // RES-02 Fleet has no grant even though it knows the contract name
    let (s, _) = env
        .call(
            "POST",
            RESOLVE,
            Some(&env.token("fleet-be")),
            Some(rq()),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // a registered-nothing client also gets nothing
    let (s, _) = env
        .call(
            "POST",
            RESOLVE,
            Some(&env.token("stranger-client")),
            Some(rq()),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // RES-05 incompatible major
    let (s, v) = env
        .call(
            "POST",
            RESOLVE,
            Some(&obt),
            Some(json!({"contract":"platform.example","version_constraint":">=2.0.0 <3.0.0"})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["code"], "no_compatible_version");
    let (s, v) = env
        .call(
            "POST",
            RESOLVE,
            Some(&obt),
            Some(json!({"contract":"platform.example","version_constraint":"not a version"})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");

    // RES-06 a published compatible minor is selected once the implementation claims it
    let extra = "  /v1/ping:\n    get:\n      responses:\n        '200': {description: ok}\n";
    let (s, m) = env
        .call(
            "POST",
            "/v0/contracts",
            Some(&env.token("publisher-client")),
            Some(Env::contract_body("1.1.0", extra)),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) = env
        .call("POST", "/v0/implementations", Some(&env.token("deployer-client")), Some(json!({"slug":"example-capability","release_version":"0.2.0","implements":[{"contract":"platform.example","version":"1.0.0","digest":digest},{"contract":"platform.example","version":"1.1.0","digest":m["digest"]}]})), &[])
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, r) = env.call("POST", RESOLVE, Some(&obt), Some(rq()), &[]).await;
    assert_eq!(r["resolved_version"], "1.1.0");
    let (_, r) = env
        .call(
            "POST",
            RESOLVE,
            Some(&obt),
            Some(json!({"contract":"platform.example","version_constraint":">=1.0.0 <1.1.0"})),
            &[],
        )
        .await;
    assert_eq!(r["resolved_version"], "1.0.0");

    // RES-07 never a container/pod IP
    assert!(!r["endpoint"].as_str().unwrap().contains("172.") || r["endpoint"] == svc.url);

    // SVC-06 / SVC-08: a second implementation of the same contract does not change the binding,
    // and failure of the bound one does not fail over.
    let svc2 = fake_service().await;
    let (s, _) = env
        .register_impl("alt-implementation", &digest, "1.0.0", &svc2.url)
        .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "alt-* not yet approved for deployer-1"
    );
    env.allow_deployer("deployer-1", "alt-*").await;
    let (s, v2) = env
        .register_impl("alt-implementation", &digest, "1.0.0", &svc2.url)
        .await;
    assert_eq!(s, StatusCode::OK, "{v2}");
    env.wait_deployment(v2["id"].as_str().unwrap(), "healthy")
        .await;
    let (_, r) = env.call("POST", RESOLVE, Some(&obt), Some(rq()), &[]).await;
    assert_eq!(
        r["endpoint"], svc.url,
        "binding unchanged by a second implementation"
    );

    // SVC-07: stop the bound implementation -> controlled 503, no failover to the healthy alternate
    svc.healthy.store(false, Ordering::SeqCst);
    env.wait_deployment(&impl_id, "unavailable").await;
    let (s, v) = env.call("POST", RESOLVE, Some(&obt), Some(rq()), &[]).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    assert_eq!(v["code"], "no_healthy_deployment");
    // SVC-09 recovers without restarting core
    svc.healthy.store(true, Ordering::SeqCst);
    env.wait_deployment(&impl_id, "healthy").await;
    let (s, _) = env.call("POST", RESOLVE, Some(&obt), Some(rq()), &[]).await;
    assert_eq!(s, StatusCode::OK);

    // SVC-10 deleting a bound deployment is refused
    let dep_id = {
        let (_, v) = env
            .op("GET", &format!("/v0/implementations/{impl_id}"), None)
            .await;
        v["deployments"][0]["id"].as_str().unwrap().to_string()
    };
    let (s, v) = env
        .call(
            "DELETE",
            &format!("/v0/deployments/{dep_id}"),
            Some(&env.token("deployer-client")),
            None,
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");

    // RES-04: grant revoked while binding remains
    let (s, _) = env
        .op(
            "DELETE",
            &format!("/v0/applications/{obroklik}/contract-grants/{grant_id}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = env.call("POST", RESOLVE, Some(&obt), Some(rq()), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // SEC-05 audit recorded before/after for binding changes
    let (_, audit) = env
        .op("GET", "/v0/audit?target_prefix=application:", None)
        .await;
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["action"] == "binding.put"
                && a["after"]["implementation"] == "example-capability")
    );
    drop_db(env).await;
}

// RES-08: re-registration at a new endpoint is picked up
#[tokio::test]
async fn endpoint_change_is_visible_after_reregistration() {
    let env = Env::new().await;
    let (obroklik, _) = setup_apps(&env).await;
    let c = env.publish_contract("1.0.0").await;
    let digest = c["digest"].as_str().unwrap().to_string();
    env.allow_deployer("deployer-1", "example-*").await;
    let a = fake_service().await;
    let (_, v) = env
        .register_impl("example-capability", &digest, "1.0.0", &a.url)
        .await;
    let id = v["id"].as_str().unwrap().to_string();
    env.wait_deployment(&id, "healthy").await;
    env.op(
        "POST",
        &format!("/v0/applications/{obroklik}/contract-grants"),
        Some(json!({"contract":"platform.example","permissions":["example:read"]})),
    )
    .await;
    env.op(
        "PUT",
        &format!("/v0/applications/{obroklik}/bindings/platform.example"),
        Some(json!({"major":1,"environment":"local","implementation":"example-capability"})),
    )
    .await;
    let t = env.token("obroklik-be");
    let (_, r) = env.call("POST", RESOLVE, Some(&t), Some(rq()), &[]).await;
    assert_eq!(r["endpoint"], a.url);
    let b = fake_service().await;
    env.register_impl("example-capability", &digest, "1.0.0", &b.url)
        .await;
    env.wait_deployment(&id, "healthy").await;
    let (_, r) = env.call("POST", RESOLVE, Some(&t), Some(rq()), &[]).await;
    assert_eq!(r["endpoint"], b.url);
    drop_db(env).await;
}
