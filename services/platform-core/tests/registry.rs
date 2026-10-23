mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

// APP-01..05, APP-07, APP-08, ENV-02 (idempotent re-run)
#[tokio::test]
async fn application_registration_lifecycle() {
    let env = Env::new().await;

    // APP-02 / APP-03
    let (s, _) = env
        .call("POST", "/v0/applications", None, Some(json!({})), &[])
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let fleet_token = env.token("fleet-be");
    let (s, _) = env
        .call(
            "POST",
            "/v0/applications",
            Some(&fleet_token),
            Some(json!({"slug":"x1","display_name":"x"})),
            &[("idempotency-key", "k0")],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // APP-01
    let (s, v) = env
        .register_app(
            "obroklik",
            "p-obroklik",
            "obroklik-web",
            "obroklik-be",
            "key-1",
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["application"]["status"], "active");
    let app_id = v["application"]["id"].as_str().unwrap().to_string();

    // APP-04: same key + payload -> same result, no duplicates
    let (s, v2) = env
        .register_app(
            "obroklik",
            "p-obroklik",
            "obroklik-web",
            "obroklik-be",
            "key-1",
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v2["application"]["id"], app_id);
    assert_eq!(v2["operation_id"], v["operation_id"]);

    // APP-05: same key, different payload
    let (s, v3) = env
        .register_app(
            "obroklik2",
            "p-obroklik",
            "obroklik-web",
            "obroklik-be",
            "key-1",
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v3}");
    assert_eq!(v3["code"], "idempotency_conflict");

    // APP-08: duplicate slug (new key), invalid redirect
    let (s, v4) = env
        .register_app("obroklik", "p-other", "w2", "b2", "key-2")
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v4}");
    assert_eq!(v4["code"], "slug_taken");
    let (s, v5) = env
        .op("POST", "/v0/applications", Some(json!({"slug":"bad","display_name":"Bad","zitadel_project_id":"p","oauth_clients":[{"kind":"web","redirect_uris":["javascript:alert(1)"],"zitadel_client_id":"c"}]})))
        .await;
    // missing idempotency key is checked first
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v5["code"], "idempotency_key_required");
    let (s, v5) = env
        .call("POST", "/v0/applications", Some(&env.token("op-client")), Some(json!({"slug":"bad","display_name":"Bad","zitadel_project_id":"p","oauth_clients":[{"kind":"web","redirect_uris":["javascript:alert(1)"],"zitadel_client_id":"c"}]})), &[("idempotency-key", "k-bad")])
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v5["code"], "invalid_redirect_uri");
    // no partial application left behind
    let (_, list) = env.op("GET", "/v0/applications", None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    // APP-07: suspend blocks workload operations immediately
    let (s, _) = env
        .op("POST", &format!("/v0/applications/{app_id}/suspend"), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = env
        .call(
            "POST",
            "/v0/contracts/resolve",
            Some(&env.token("obroklik-be")),
            Some(json!({"contract":"platform.example","version_constraint":">=1.0.0 <2.0.0"})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = env
        .op("POST", &format!("/v0/applications/{app_id}/activate"), None)
        .await;
    assert_eq!(s, StatusCode::OK);

    // audit trail recorded actor/targets
    let (_, audit) = env
        .op("GET", "/v0/audit?target_prefix=application:", None)
        .await;
    let actions: Vec<_> = audit["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap().to_string())
        .collect();
    assert!(
        actions.contains(&"application.register".to_string())
            && actions.contains(&"application.suspend".to_string())
    );
    drop_db(env).await;
}

// CON-01..04, CON-06, CON-07, CON-09
#[tokio::test]
async fn contract_publication_and_immutability() {
    let env = Env::new().await;
    let pubt = env.token("publisher-client");

    let v1 = env.publish_contract("1.0.0").await;
    assert_eq!(v1["created"], true);
    // CON-02 idempotent
    let again = env.publish_contract("1.0.0").await;
    assert_eq!(again["created"], false);
    assert_eq!(again["digest"], v1["digest"]);

    // CON-03 changed content under the same version
    let mut changed = Env::contract_body("1.0.0", "");
    changed["semantics"] = json!("different");
    let (s, v) = env
        .call("POST", "/v0/contracts", Some(&pubt), Some(changed), &[])
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "contract_version_conflict");
    // the original is unchanged
    let (s, got) = env
        .op("GET", "/v0/contracts/platform.example/versions/1.0.0", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["semantics"], "echo semantics");

    // CON-04 malformed
    let (s, v) = env.call("POST", "/v0/contracts", Some(&pubt), Some(json!({"manifest":{"id":"platform.example","version":"1.1.0"},"openapi":"not: [valid","semantics":""})), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, _) = env.call("POST", "/v0/contracts", Some(&pubt), Some(json!({"manifest":{"id":"platform.example","version":"1.1.0"},"openapi":"openapi: 3.0.0\ninfo: {}\npaths: {}","semantics":""})), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // compatible minor: adds an optional operation
    let extra = "  /v1/ping:\n    get:\n      responses:\n        '200': {description: ok}\n";
    let (s, v) = env
        .call(
            "POST",
            "/v0/contracts",
            Some(&pubt),
            Some(Env::contract_body("1.1.0", extra)),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    // CON-06 breaking change in a minor: removed operation
    let breaking = json!({
        "manifest": {"id": "platform.example", "version": "1.2.0", "permissions": ["example:read"]},
        "openapi": "openapi: 3.1.0\ninfo: {title: e, version: '1'}\npaths:\n  /v1/other:\n    get:\n      responses:\n        '200': {description: ok}\n",
        "semantics": "",
    });
    let (s, v) = env
        .call(
            "POST",
            "/v0/contracts",
            Some(&pubt),
            Some(breaking.clone()),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(v["code"], "breaking_change_within_major");

    // CON-07 the same breaking spec is fine as a new major, and coexists
    let mut v2 = breaking;
    v2["manifest"]["version"] = json!("2.0.0");
    let (s, v) = env
        .call("POST", "/v0/contracts", Some(&pubt), Some(v2), &[])
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (s, _) = env
        .op("GET", "/v0/contracts/platform.example/versions/1.0.0", None)
        .await;
    assert_eq!(s, StatusCode::OK);

    // CON-09 an ordinary application token cannot publish; namespace restrictions apply to publishers
    let (s, _) = env
        .call(
            "POST",
            "/v0/contracts",
            Some(&env.token("fleet-be")),
            Some(Env::contract_body("1.5.0", "")),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let mut other_ns = Env::contract_body("1.0.0", "");
    other_ns["manifest"]["id"] = json!("billing.core");
    let (s, _) = env
        .call("POST", "/v0/contracts", Some(&pubt), Some(other_ns), &[])
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // the database refuses to mutate published payloads even for the owner role
    let r = sqlx::query("UPDATE contracts SET openapi='x' WHERE name='platform.example'")
        .execute(&env.owner_pool)
        .await;
    assert!(r.is_err());
    drop_db(env).await;
}
