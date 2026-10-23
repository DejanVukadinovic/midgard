//! The sample service against a stub of the core's `/v0/authorize`, a test IdP and real PostgreSQL.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use example_capability::{AppState, CoreCredentials, MIGRATOR, router};
use platform_auth::{TokenVerifier, VerifierConfig};
use platform_testkit::{ClientCfg, Idp, IdpConfig};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

const TENANT_A: &str = "00000000-0000-0000-0000-00000000000a";
const TENANT_B: &str = "00000000-0000-0000-0000-00000000000b";
const AUD: &str = "platform-example";

struct FakeCore {
    calls: AtomicU32,
    grants: Mutex<HashMap<String, Vec<String>>>,
}

async fn authorize(
    State(c): State<Arc<FakeCore>>,
    headers: axum::http::HeaderMap,
    Json(b): Json<Value>,
) -> (StatusCode, Json<Value>) {
    c.calls.fetch_add(1, Ordering::SeqCst);
    if headers.get("authorization").is_none() {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    let Some(perms) = c
        .grants
        .lock()
        .unwrap()
        .get(b["client_id"].as_str().unwrap_or_default())
        .cloned()
    else {
        return (StatusCode::FORBIDDEN, Json(json!({"code": "forbidden"})));
    };
    let tenant = b["user_token"]
        .as_str()
        .map(|t| json!({"tenant_id": if t == "user-a" { TENANT_A } else { TENANT_B }}));
    (
        StatusCode::OK,
        Json(json!({"application": b["client_id"], "permissions": perms, "tenant": tenant})),
    )
}

struct Harness {
    base: String,
    core: Arc<FakeCore>,
    idp: Arc<Idp>,
    pool: PgPool,
    admin: PgPool,
    db: String,
    http: reqwest::Client,
}

impl Harness {
    async fn new() -> Self {
        let admin_url = std::env::var("TEST_DATABASE_ADMIN_URL")
            .unwrap_or_else(|_| "postgres://midgard:midgard@localhost:5432/postgres".into());
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .expect("PostgreSQL required");
        let db = format!("cap_it_{}", Uuid::new_v4().simple());
        admin
            .execute(format!("CREATE DATABASE {db}").as_str())
            .await
            .unwrap();
        let _ = admin.execute("DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='capability_it_app') THEN CREATE ROLE capability_it_app LOGIN PASSWORD 'cap_app_test' NOSUPERUSER NOBYPASSRLS; END IF; EXCEPTION WHEN duplicate_object THEN NULL; END $$").await;
        let _ = admin.execute("DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='capability_runtime') THEN CREATE ROLE capability_runtime NOLOGIN; END IF; EXCEPTION WHEN duplicate_object THEN NULL; END $$").await;
        let _ = admin
            .execute("GRANT capability_runtime TO capability_it_app")
            .await;
        let (head, _) = admin_url.rsplit_once('/').unwrap();
        let host = head.rsplit_once('@').unwrap().1;
        let owner = PgPoolOptions::new()
            .max_connections(2)
            .connect(&format!("{head}/{db}"))
            .await
            .unwrap();
        MIGRATOR.run(&owner).await.unwrap();
        owner.close().await;
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&format!(
                "postgres://capability_it_app:cap_app_test@{host}/{db}"
            ))
            .await
            .unwrap();

        let (idp, issuer) = platform_testkit::spawn(IdpConfig {
            clients: vec![
                ClientCfg {
                    client_id: "cap-deployer".into(),
                    client_secret: "s".into(),
                    subject: "deployer".into(),
                    scopes: vec![],
                    audiences: vec!["midgard-core".into()],
                },
                ClientCfg {
                    client_id: "app-rw".into(),
                    client_secret: "s".into(),
                    subject: "svc-rw".into(),
                    scopes: vec![],
                    audiences: vec![AUD.into(), "other".into()],
                },
                ClientCfg {
                    client_id: "app-ro".into(),
                    client_secret: "s".into(),
                    subject: "svc-ro".into(),
                    scopes: vec![],
                    audiences: vec![AUD.into()],
                },
                ClientCfg {
                    client_id: "app-none".into(),
                    client_secret: "s".into(),
                    subject: "svc-none".into(),
                    scopes: vec![],
                    audiences: vec![AUD.into()],
                },
            ],
            ..Default::default()
        })
        .await;
        let core = Arc::new(FakeCore {
            calls: AtomicU32::new(0),
            grants: Mutex::new(HashMap::from([
                (
                    "app-rw".to_string(),
                    vec!["example:read".to_string(), "example:write".to_string()],
                ),
                ("app-ro".to_string(), vec!["example:read".to_string()]),
            ])),
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let core_url = format!("http://{}", l.local_addr().unwrap());
        let capp = Router::new()
            .route("/v0/authorize", post(authorize))
            .with_state(core.clone());
        tokio::spawn(async move { axum::serve(l, capp).await.ok() });

        let verifier = TokenVerifier::from_jwks_url(
            VerifierConfig {
                issuers: vec![issuer.clone()],
                audience: AUD.into(),
                leeway_secs: 5,
                max_lifetime_secs: 3600,
                accept_plain_jwt_typ: false,
            },
            &format!("{issuer}/oauth/v2/keys"),
        );
        let creds = CoreCredentials::new(
            format!("{issuer}/oauth/v2/token"),
            "cap-deployer".into(),
            "s".into(),
            "midgard-core".into(),
        );
        let st = AppState::new(
            pool.clone(),
            verifier,
            core_url,
            creds,
            Duration::from_secs(30),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let app = router(st);
        tokio::spawn(async move { axum::serve(l, app).await.ok() });
        Harness {
            base,
            core,
            idp,
            pool,
            admin,
            db,
            http: reqwest::Client::new(),
        }
    }

    fn tok(&self, client: &str, aud: &str) -> String {
        let sub = format!("svc-{}", client.trim_start_matches("app-"));
        self.idp
            .sign(&self.idp.base_claims(&sub, client, &[aud.to_string()], ""))
    }

    async fn finish(self) {
        self.pool.close().await;
        let _ = self
            .admin
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.db).as_str())
            .await;
    }
}

#[tokio::test]
async fn echo_requires_valid_token_grant_and_permission() {
    let h = Harness::new().await;
    let url = format!("{}/v1/echo", h.base);
    let body = json!({"message": "hi"});

    // no token / wrong audience / forged
    assert_eq!(
        h.http.post(&url).json(&body).send().await.unwrap().status(),
        401
    );
    let wrong_aud = h.tok("app-rw", "other");
    assert_eq!(
        h.http
            .post(&url)
            .bearer_auth(wrong_aud)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        h.http
            .post(&url)
            .bearer_auth("a.b.c")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        h.core.calls.load(Ordering::SeqCst),
        0,
        "unauthenticated requests never reach core"
    );

    // granted
    let r = h
        .http
        .post(&url)
        .bearer_auth(h.tok("app-rw", AUD))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<Value>().await.unwrap(),
        json!({"message": "hi", "application": "app-rw"})
    );
    // decision cached briefly
    let before = h.core.calls.load(Ordering::SeqCst);
    h.http
        .post(&url)
        .bearer_auth(h.tok("app-rw", AUD))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(h.core.calls.load(Ordering::SeqCst), before);

    // valid token, but no grant for this contract on this implementation: knowing the endpoint is not enough
    let r = h
        .http
        .post(&url)
        .bearer_auth(h.tok("app-none", AUD))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    h.finish().await;
}

#[tokio::test]
async fn notes_are_tenant_isolated_by_rls() {
    let h = Harness::new().await;
    let notes = format!("{}/v1/notes", h.base);
    let rw = h.tok("app-rw", AUD);

    let create = |user: &'static str, text: &'static str| {
        let (h, rw, notes) = (&h, rw.clone(), notes.clone());
        async move {
            h.http
                .post(&notes)
                .bearer_auth(rw)
                .header("x-delegated-user-token", user)
                .json(&json!({"body": text}))
                .send()
                .await
                .unwrap()
        }
    };
    assert_eq!(create("user-a", "a-secret").await.status(), 201);
    assert_eq!(create("user-b", "b-secret").await.status(), 201);

    let list = |user: &'static str| {
        let (h, rw, notes) = (&h, rw.clone(), notes.clone());
        async move {
            h.http
                .get(&notes)
                .bearer_auth(rw)
                .header("x-delegated-user-token", user)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let a = list("user-a").await;
    assert_eq!(a["items"].as_array().unwrap().len(), 1);
    assert_eq!(a["items"][0]["body"], "a-secret");
    assert_eq!(list("user-b").await["items"][0]["body"], "b-secret");

    // caller-supplied tenant identifiers are ignored
    let r = h
        .http
        .get(&notes)
        .bearer_auth(&rw)
        .header("x-delegated-user-token", "user-a")
        .header("x-tenant-id", TENANT_B)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.json::<Value>().await.unwrap()["items"][0]["body"],
        "a-secret"
    );

    // missing user token -> 400; read-only app cannot write; ungranted app denied
    assert_eq!(
        h.http
            .get(&notes)
            .bearer_auth(&rw)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let ro = h.tok("app-ro", AUD);
    let r = h
        .http
        .post(&notes)
        .bearer_auth(&ro)
        .header("x-delegated-user-token", "user-a")
        .json(&json!({"body": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(
        h.http
            .get(&notes)
            .bearer_auth(&ro)
            .header("x-delegated-user-token", "user-a")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        h.http
            .get(&notes)
            .bearer_auth(h.tok("app-none", AUD))
            .header("x-delegated-user-token", "user-a")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );

    // defense in depth: even a bug that skipped tenant context sees nothing through the runtime role
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    h.finish().await;
}

#[tokio::test]
async fn ping_and_health() {
    let h = Harness::new().await;
    assert_eq!(
        h.http
            .get(format!("{}/health/ready", h.base))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        h.http
            .get(format!("{}/v1/ping", h.base))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let r = h
        .http
        .get(format!("{}/v1/ping", h.base))
        .bearer_auth(h.tok("app-ro", AUD))
        .send()
        .await
        .unwrap();
    assert_eq!(r.json::<Value>().await.unwrap(), json!({"pong": true}));
    h.finish().await;
}
