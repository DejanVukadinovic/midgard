mod common;

use axum::http::StatusCode;
use base64::Engine as _;
use common::*;
use serde_json::json;

fn b64(v: &serde_json::Value) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
}

// SEC-01, IAM-07, IAM-08: every malformed/forged token is a 401 and performs no operation
#[tokio::test]
async fn forged_and_misissued_tokens_are_rejected() {
    let env = Env::new().await;
    let good = env.token("op-client");
    let (s, _) = env
        .call("GET", "/v0/applications", Some(&good), None, &[])
        .await;
    assert_eq!(s, StatusCode::OK);

    let base = |f: &dyn Fn(&mut serde_json::Value)| {
        let mut c = env
            .idp
            .base_claims("operator-1", "op-client", &[CORE_AUD.to_string()], "");
        f(&mut c);
        c
    };
    let mut bad: Vec<(&str, String)> = vec![
        (
            "wrong audience",
            env.idp.sign(&base(&|c| c["aud"] = json!(["someone-else"]))),
        ),
        (
            "wrong issuer",
            env.idp
                .sign(&base(&|c| c["iss"] = json!("https://evil.example"))),
        ),
        (
            "expired",
            env.idp.sign(&base(&|c| {
                c["exp"] = json!(platform_testkit::now() - 3600);
                c["iat"] = json!(platform_testkit::now() - 7200);
            })),
        ),
        (
            "not yet valid",
            env.idp
                .sign(&base(&|c| c["nbf"] = json!(platform_testkit::now() + 3600))),
        ),
        (
            "id token (nonce)",
            env.idp.sign(&base(&|c| c["nonce"] = json!("n"))),
        ),
        (
            "id token (at_hash)",
            env.idp.sign(&base(&|c| c["at_hash"] = json!("h"))),
        ),
        ("id token typ", env.idp.sign_with_typ(&base(&|_| {}), "JWT")),
        (
            "lifetime too long",
            env.idp.sign(&base(&|c| {
                c["exp"] = json!(platform_testkit::now() + 86_400)
            })),
        ),
        (
            "missing iat",
            env.idp.sign(&base(&|c| {
                c.as_object_mut().unwrap().remove("iat");
            })),
        ),
        ("garbage", "not.a.jwt".to_string()),
        ("empty", String::new()),
    ];
    // tampered signature / payload
    let mut parts: Vec<&str> = good.split('.').collect();
    let sig = parts.pop().unwrap();
    let flipped = format!(
        "{}{}",
        &sig[..sig.len() - 2],
        if sig.ends_with("AA") { "BB" } else { "AA" }
    );
    bad.push(("bad signature", format!("{}.{}", parts.join("."), flipped)));
    let forged_payload = b64(&base(&|c| c["sub"] = json!("someone")));
    bad.push((
        "tampered payload",
        format!("{}.{}.{}", parts[0], forged_payload, sig),
    ));
    // alg confusion: HS256 / none
    let hs = format!(
        "{}.{}.{}",
        b64(&json!({"alg":"HS256","typ":"at+jwt","kid":"x"})),
        b64(&base(&|_| {})),
        "c2ln"
    );
    bad.push(("hs256", hs));
    let none = format!(
        "{}.{}.",
        b64(&json!({"alg":"none","typ":"at+jwt"})),
        b64(&base(&|_| {}))
    );
    bad.push(("alg none", none));
    // token signed by a different IdP key but same claims
    let (other, _) = platform_testkit::spawn(test_idp_config()).await;
    bad.push(("foreign key", other.sign(&base(&|_| {}))));

    for (what, tok) in bad {
        let (s, _) = env
            .call(
                "GET",
                "/v0/applications",
                if tok.is_empty() { None } else { Some(&tok) },
                None,
                &[],
            )
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{what} must be 401");
    }
    // no business operation happened
    let (_, apps) = env.op("GET", "/v0/applications", None).await;
    assert!(apps["items"].as_array().unwrap().is_empty());
    drop_db(env).await;
}

// OSS-11 / SVC-02 / CON-09 / APP-03: privileged routes refuse every non-operator identity
#[tokio::test]
async fn non_operators_cannot_use_admin_apis() {
    let env = Env::new().await;
    env.register_app("fleet", "p-fleet", "fleet-web", "fleet-be", "kf")
        .await;
    let id = "00000000-0000-0000-0000-000000000001";
    let cases: Vec<(&str, String, serde_json::Value)> = vec![
        ("GET", "/v0/applications".into(), json!(null)),
        (
            "POST",
            format!("/v0/applications/{id}/suspend"),
            json!(null),
        ),
        (
            "POST",
            format!("/v0/applications/{id}/contract-grants"),
            json!({"contract":"platform.example","permissions":["example:read"]}),
        ),
        (
            "PUT",
            format!("/v0/applications/{id}/bindings/platform.example"),
            json!({"major":1,"environment":"local","implementation":"x"}),
        ),
        (
            "POST",
            format!("/v0/organizations/orgA/applications/{id}/tenants"),
            json!(null),
        ),
        (
            "POST",
            "/v0/deployment-identities".into(),
            json!({"subject":"svc-fleet","slug_pattern":"x*"}),
        ),
        ("GET", "/v0/audit".into(), json!(null)),
    ];
    for client in [
        "fleet-be",
        "deployer-client",
        "publisher-client",
        "stranger-client",
    ] {
        let t = env.token(client);
        for (m, p, b) in &cases {
            let (s, _) = env
                .call(
                    m,
                    p,
                    Some(&t),
                    if b.is_null() { None } else { Some(b.clone()) },
                    &[("idempotency-key", "k")],
                )
                .await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{client} {m} {p}");
        }
    }
    // an application cannot self-grant even with its own workload token
    let (_, apps) = env.op("GET", "/v0/applications", None).await;
    let fleet = apps["items"][0]["id"].as_str().unwrap();
    let (s, _) = env
        .call(
            "POST",
            &format!("/v0/applications/{fleet}/contract-grants"),
            Some(&env.token("fleet-be")),
            Some(json!({"contract":"platform.example","permissions":["example:read"]})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    drop_db(env).await;
}

// SEC-03: hostile strings are bound as parameters; schema is intact afterwards
#[tokio::test]
async fn sql_injection_attempts_are_inert() {
    let env = Env::new().await;
    let evil = "x'; DROP TABLE applications; --";
    let t = env.token("op-client");
    for body in [
        json!({"slug": evil, "display_name": "e", "zitadel_project_id": evil}),
        json!({"slug": "okslug", "display_name": evil, "zitadel_project_id": evil, "oauth_clients": [{"kind":"backend","zitadel_client_id": evil}], "requirements":[{"contract": evil, "version": evil}]}),
    ] {
        let (s, _) = env
            .call(
                "POST",
                "/v0/applications",
                Some(&t),
                Some(body),
                &[("idempotency-key", evil)],
            )
            .await;
        assert!(s.is_client_error() || s.is_success(), "{s}");
    }
    let bt = env.token("obroklik-be");
    for c in [evil, "a.b' OR '1'='1"] {
        let (s, _) = env
            .call(
                "POST",
                "/v0/contracts/resolve",
                Some(&bt),
                Some(json!({"contract": c, "version_constraint": evil})),
                &[],
            )
            .await;
        assert!(
            s == StatusCode::BAD_REQUEST || s == StatusCode::FORBIDDEN,
            "{s}"
        );
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM applications")
        .fetch_one(&env.owner_pool)
        .await
        .unwrap();
    assert!(n <= 1);
    drop_db(env).await;
}

// SEC-06: rate and body limits
#[tokio::test]
async fn rate_and_body_limits() {
    let env = Env::with(
        |c| c.admin_rate_limit_per_minute = 5,
        platform_core::provisioning::Provisioner::Link,
    )
    .await;
    let t = env.token("op-client");
    let mut limited = false;
    for _ in 0..12 {
        let (s, _) = env
            .call("GET", "/v0/applications", Some(&t), None, &[])
            .await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
        }
    }
    assert!(limited, "admin routes must be rate limited");
    drop_db(env).await;

    let env = Env::new().await;
    let big = "a".repeat(400 * 1024);
    let (s, _) = env
        .call(
            "POST",
            "/v0/applications",
            Some(&env.token("op-client")),
            Some(json!({"slug":"x1","display_name": big})),
            &[("idempotency-key", "k")],
        )
        .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    // RFC 9457 content type and request id on errors
    let r = env
        .http
        .get(format!("{}/v0/applications", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(r.headers()["content-type"], "application/problem+json");
    assert!(r.headers().contains_key("x-request-id"));
    drop_db(env).await;
}
