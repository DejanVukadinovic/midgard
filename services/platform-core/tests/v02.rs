mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

const CONTRACT: &str = include_str!("../../../contracts/examples/echo/contract.yaml");
const OPENAPI: &str = include_str!("../../../contracts/examples/echo/openapi.yaml");
const ASYNCAPI: &str = include_str!("../../../contracts/examples/echo/asyncapi.yaml");
const NAME: &str = "example.echo.submit";
const RESOLVE: &str = "/v0/contracts/resolve";

fn publish_body(contract: &str, openapi: &str) -> Value {
    json!({
        "contract": serde_yaml::from_str::<Value>(contract).unwrap(),
        "documents": {"openapi.yaml": openapi, "asyncapi.yaml": ASYNCAPI},
    })
}

async fn publish(env: &Env, body: Value) -> (StatusCode, Value) {
    env.op("POST", "/v0/capability-contracts", Some(body)).await
}

async fn register_impl(
    env: &Env,
    slug: &str,
    digest: &str,
    interfaces: Value,
    endpoint: &str,
) -> (StatusCode, Value) {
    env.call(
        "POST",
        "/v0/implementations",
        Some(&env.token("deployer-client")),
        Some(json!({
            "slug": slug, "release_version": "0.1.0",
            "implements": [{"contract": NAME, "version": "1.0.0", "digest": digest, "interfaces": interfaces}],
            "deployments": [{"environment": "local", "endpoint": endpoint}],
        })),
        &[],
    )
    .await
}

async fn app(env: &Env) -> String {
    let (s, a) = env
        .call(
            "POST",
            "/v0/applications",
            Some(&env.token("op-client")),
            Some(json!({
                "slug": "obroklik", "display_name": "Obroklik", "zitadel_project_id": "p-obroklik",
                "oauth_clients": [
                    {"kind": "web", "redirect_uris": ["https://obroklik.example/cb"], "zitadel_client_id": "obroklik-web"},
                    {"kind": "backend", "zitadel_client_id": "obroklik-be"},
                ],
                "requirements": [{"contract": NAME, "version": ">=1.0.0 <2.0.0"}],
            })),
            &[("idempotency-key", "echo-app")],
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    a["application"]["id"].as_str().unwrap().into()
}

#[tokio::test]
async fn publish_validates_and_is_immutable() {
    let env = Env::new().await;
    let (s, v) = publish(&env, publish_body(CONTRACT, OPENAPI)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let digest = v["digest"].as_str().unwrap().to_string();
    assert_eq!(v["contract"], NAME);

    // idempotent republish
    let (s, v) = publish(&env, publish_body(CONTRACT, OPENAPI)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["digest"], digest);
    assert_eq!(v["created"], false);

    // same version, changed interface -> immutable
    let (s, v) = publish(
        &env,
        publish_body(CONTRACT, &OPENAPI.replace("Synthetic Echo API", "Changed")),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");

    // malformed: unknown operation, URL reference, missing document, wrong spec for protocol
    for (c, needle) in [
        (
            CONTRACT.replace("operation: submitEcho", "operation: nope"),
            "not found",
        ),
        (
            CONTRACT.replace("./openapi.yaml", "http://evil.example/o.yaml"),
            "relative path",
        ),
        (
            CONTRACT.replace("./openapi.yaml", "./absent.yaml"),
            "not supplied",
        ),
        (
            CONTRACT.replace("protocol: mq", "protocol: nats"),
            "unknown variant",
        ),
        (
            CONTRACT
                .replace("callers: [service]   #", "callers: [service, user]   #")
                .replace(
                    "callers: [service, user]\n  context",
                    "callers: [service]\n  context",
                ),
            "never widen",
        ),
    ] {
        let (s, v) = publish(&env, publish_body(&c, OPENAPI)).await;
        assert!(
            s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
            "{needle}: {s} {v}"
        );
        assert!(v.to_string().contains(needle), "{needle}: {v}");
    }

    // retrievable, including the normalized interface document
    let (s, v) = env
        .op("GET", &format!("/v0/contracts/{NAME}/versions/1.0.0"), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["manifest"]["interfaces"].as_array().unwrap().len(), 2);
    let (s, v) = env
        .op(
            "GET",
            &format!("/v0/contracts/{NAME}/versions/1.0.0/interfaces/queue/document"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["asyncapi"].as_str().unwrap().starts_with("3.0"));
    drop_db(env).await;
}

#[tokio::test]
async fn multi_interface_resolution_and_provider_authorization() {
    let env = Env::new().await;
    let (_, p) = publish(&env, publish_body(CONTRACT, OPENAPI)).await;
    let digest = p["digest"].as_str().unwrap().to_string();
    let app_id = app(&env).await;
    env.allow_deployer("deployer-1", "example-*").await;
    let svc = fake_service().await;

    // registration is validated against the published contract
    for (ifaces, code) in [
        (json!([]), "invalid_interface"),
        (
            json!([{"id": "nope", "location": svc.url}]),
            "invalid_interface",
        ),
        (
            json!([{"id": "queue", "destination": "example.echo.submit"}]),
            "invalid_interface",
        ),
        (
            json!([{"id": "queue", "destination": "a.>", "broker_binding": "default-mq"}]),
            "invalid_interface",
        ),
        (
            json!([{"id": "rest", "location": "http://evil.example"}]),
            "endpoint_rejected",
        ),
    ] {
        let (s, v) = register_impl(&env, "example-echo", &digest, ifaces, &svc.url).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        assert_eq!(v["code"], code, "{v}");
    }

    // partial implementation (HTTP only) is valid
    let (s, v) = register_impl(
        &env,
        "example-echo",
        &digest,
        json!([{"id": "rest", "location": svc.url}]),
        &svc.url,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let impl_id = v["id"].as_str().unwrap().to_string();
    env.wait_deployment(&impl_id, "healthy").await;

    let rq = json!({"contract": NAME, "version_constraint": ">=1.0.0 <2.0.0"});
    let be = env.token("obroklik-be");
    let (s, _) = env
        .call("POST", RESOLVE, Some(&be), Some(rq.clone()), &[])
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "no grant");

    // a grant without the contract's permission cannot be created at all
    let (s, _) = env
        .op(
            "POST",
            &format!("/v0/applications/{app_id}/contract-grants"),
            Some(json!({"contract": NAME, "permissions": ["other:perm"]})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, g) = env
        .op(
            "POST",
            &format!("/v0/applications/{app_id}/contract-grants"),
            Some(json!({"contract": NAME, "permissions": ["example.echo:submit"]})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    let grant_id = g["id"].as_str().unwrap().to_string();
    let (s, b) = env
        .op(
            "PUT",
            &format!("/v0/applications/{app_id}/bindings/{NAME}"),
            Some(json!({"major": 1, "environment": "local", "implementation": "example-echo"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    let (s, r) = env
        .call("POST", RESOLVE, Some(&be), Some(rq.clone()), &[])
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["digest"], digest);
    assert_eq!(r["authorization"]["permission"], "example.echo:submit");
    let ifaces = r["interfaces"].as_array().unwrap();
    assert_eq!(
        ifaces.len(),
        1,
        "only the advertised interface is returned: {r}"
    );
    assert_eq!(ifaces[0]["id"], "rest");
    assert_eq!(ifaces[0]["location"], svc.url);
    assert_eq!(ifaces[0]["specification"]["summary"]["method"], "POST");
    assert!(r.to_string().contains("example.echo:submit") && !r.to_string().contains("secret"));

    // dynamic registration of a second interface: no Core restart
    let (s, v) = register_impl(
        &env, "example-echo", &digest,
        json!([{"id": "rest", "location": svc.url}, {"id": "queue", "destination": "example.echo.submit", "broker_binding": "default-mq"}]),
        &svc.url,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, r) = env
        .call("POST", RESOLVE, Some(&be), Some(rq.clone()), &[])
        .await;
    let ids: Vec<&str> = r["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["queue", "rest"], "{r}");
    let q = &r["interfaces"][0];
    assert_eq!(q["destination"], "example.echo.submit");
    assert_eq!(q["broker_binding"], "default-mq");
    assert!(q.get("location").is_none());

    // provider-side authorize: service may use both, a user may not use the service-only queue
    let dep = env.token("deployer-client");
    let auth = |iface: &'static str, user: Option<String>| {
        let mut b = json!({"client_id": "obroklik-be", "contract": NAME, "interface": iface});
        if let Some(u) = user {
            b["user_token"] = json!(u);
        }
        let env = &env;
        let dep = dep.clone();
        async move {
            env.call("POST", "/v0/authorize", Some(&dep), Some(b), &[])
                .await
        }
    };
    assert_eq!(auth("queue", None).await.0, StatusCode::OK);
    assert_eq!(auth("rest", None).await.0, StatusCode::OK);
    assert_eq!(auth("missing", None).await.0, StatusCode::FORBIDDEN);
    let alice = env.user_token("alice", "obroklik-web", &["p-obroklik"]);
    assert_eq!(
        auth("queue", Some(alice)).await.0,
        StatusCode::FORBIDDEN,
        "user must not reach the service-only interface"
    );

    // broker-authenticated callers are identified by application slug (never by message content)
    let by_app = |app: &'static str, iface: &'static str| {
        let (env, dep) = (&env, dep.clone());
        async move {
            env.call(
                "POST",
                "/v0/authorize",
                Some(&dep),
                Some(json!({"application": app, "contract": NAME, "interface": iface})),
                &[],
            )
            .await
            .0
        }
    };
    assert_eq!(by_app("obroklik", "queue").await, StatusCode::OK);
    assert_eq!(by_app("nobody", "queue").await, StatusCode::FORBIDDEN);
    let (s, _) = env
        .call(
            "POST",
            "/v0/authorize",
            Some(&dep),
            Some(json!({"application": "obroklik", "client_id": "obroklik-be", "contract": NAME})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "exactly one identity selector");

    // revoked grant: resolution and provider authorization both fail afterwards
    let (s, _) = env
        .op(
            "DELETE",
            &format!("/v0/applications/{app_id}/contract-grants/{grant_id}"),
            None,
        )
        .await;
    assert!(s.is_success());
    let (s, _) = env.call("POST", RESOLVE, Some(&be), Some(rq), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(auth("rest", None).await.0, StatusCode::FORBIDDEN);
    drop_db(env).await;
}
