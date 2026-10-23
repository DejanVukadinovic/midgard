#![allow(dead_code)]
//! Shared integration-test harness: a real PostgreSQL database per test, migrations applied as
//! the owner, and the core running as a non-owner, non-superuser role (so RLS really applies).

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{Router, http::StatusCode, routing::get};
use platform_auth::{TokenVerifier, VerifierConfig};
use platform_core::{
    MIGRATOR,
    config::{Config, ProvisionerKind},
    health,
    provisioning::Provisioner,
    router,
    state::AppState,
};
use platform_testkit::{ClientCfg, Idp, IdpConfig, UserCfg};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

pub const CORE_AUD: &str = "midgard-core";

fn admin_url() -> String {
    std::env::var("TEST_DATABASE_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://midgard:midgard@localhost:5432/postgres".into())
}

fn with_db(url: &str, db: &str, user: Option<(&str, &str)>) -> String {
    let (scheme_rest, _) = url.rsplit_once('/').unwrap();
    let base = match user {
        Some((u, p)) => {
            let host = scheme_rest.rsplit_once('@').unwrap().1;
            format!("postgres://{u}:{p}@{host}")
        }
        None => scheme_rest.to_string(),
    };
    format!("{base}/{db}")
}

pub struct Fake {
    pub url: String,
    pub healthy: Arc<AtomicBool>,
}

/// A minimal service exposing `/health/ready` whose health can be toggled.
pub async fn fake_service() -> Fake {
    let healthy = Arc::new(AtomicBool::new(true));
    let h = healthy.clone();
    let app = Router::new().route(
        "/health/ready",
        get(move || {
            let h = h.clone();
            async move {
                if h.load(Ordering::SeqCst) {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://localhost:{}", l.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(l, app).await.ok() });
    Fake { url, healthy }
}

pub struct Env {
    pub st: AppState,
    pub base: String,
    pub idp: Arc<Idp>,
    pub http: reqwest::Client,
    pub db_name: String,
    pub owner_pool: PgPool,
    pub cfg: Config,
}

pub fn test_idp_config() -> IdpConfig {
    let client = |id: &str, sub: &str| ClientCfg {
        client_id: id.into(),
        client_secret: format!("{id}-secret"),
        subject: sub.into(),
        scopes: vec![],
        audiences: vec![CORE_AUD.into(), "capability".into()],
    };
    let mut roles: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    roles.insert(
        "p-obroklik".into(),
        BTreeMap::from([("member".into(), vec!["orgA".into()])]),
    );
    roles.insert(
        "p-fleet".into(),
        BTreeMap::from([("viewer".into(), vec!["orgA".into(), "orgB".into()])]),
    );
    IdpConfig {
        issuer: String::new(),
        clients: vec![
            client("op-client", "operator-1"),
            client("publisher-client", "publisher-1"),
            client("deployer-client", "deployer-1"),
            client("deployer2-client", "deployer-2"),
            client("obroklik-be", "svc-obroklik"),
            client("fleet-be", "svc-fleet"),
            client("stranger-client", "svc-stranger"),
        ],
        users: vec![
            UserCfg {
                sub: "alice".into(),
                home_org: "orgA".into(),
                roles,
            },
            // Holds a Fleet role only in orgB (extra grant), home orgA has nothing for Fleet.
            UserCfg {
                sub: "bob".into(),
                home_org: "orgA".into(),
                roles: BTreeMap::from([(
                    "p-fleet".into(),
                    BTreeMap::from([("viewer".into(), vec!["orgB".into()])]),
                )]),
            },
            // No Obroklik/Fleet roles at all (org has a project grant but user was not authorized).
            UserCfg {
                sub: "carol".into(),
                home_org: "orgA".into(),
                roles: BTreeMap::new(),
            },
        ],
        token_ttl_secs: 300,
    }
}

impl Env {
    pub async fn new() -> Env {
        Self::with(|_| {}, Provisioner::Link).await
    }

    pub async fn with(tweak: impl FnOnce(&mut Config), provisioner: Provisioner) -> Env {
        let (idp, issuer) = platform_testkit::spawn(test_idp_config()).await;
        let admin = admin_url();
        let admin_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin)
            .await
            .expect("admin db (is PostgreSQL running? see CONTRIBUTING.md)");
        let db_name = format!("midgard_it_{}", Uuid::new_v4().simple());
        admin_pool
            .execute(format!("CREATE DATABASE {db_name}").as_str())
            .await
            .unwrap();
        // Cluster-wide login role used by the core at runtime: not superuser, not owner, no BYPASSRLS.
        let _ = admin_pool
            .execute("DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='midgard_it_app') THEN CREATE ROLE midgard_it_app LOGIN PASSWORD 'midgard_it_app_test' NOSUPERUSER NOBYPASSRLS; END IF; EXCEPTION WHEN duplicate_object THEN NULL; END $$")
            .await;
        let _ = admin_pool.execute("DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='midgard_runtime') THEN CREATE ROLE midgard_runtime NOLOGIN; END IF; EXCEPTION WHEN duplicate_object THEN NULL; END $$").await;
        let _ = admin_pool
            .execute("GRANT midgard_runtime TO midgard_it_app")
            .await;

        let owner_pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(&with_db(&admin, &db_name, None))
            .await
            .unwrap();
        MIGRATOR.run(&owner_pool).await.expect("migrations");
        let app_url = with_db(
            &admin,
            &db_name,
            Some(("midgard_it_app", "midgard_it_app_test")),
        );
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&app_url)
            .await
            .unwrap();

        let mut cfg = Config {
            bind_addr: "127.0.0.1:0".into(),
            database_url: app_url,
            migration_database_url: None,
            issuer: issuer.clone(),
            jwks_url: format!("{issuer}/oauth/v2/keys"),
            audience: CORE_AUD.into(),
            token_max_lifetime_secs: 3600,
            accept_plain_jwt_typ: false,
            operator_subjects: vec!["operator-1".into()],
            publishers: BTreeMap::from([(
                "publisher-1".to_string(),
                vec!["platform.".to_string(), "examples.".to_string()],
            )]),
            provisioner: ProvisionerKind::Link,
            zitadel_api_url: None,
            zitadel_api_token: None,
            zitadel_org_id: None,
            health_check_interval: Duration::from_millis(300),
            unavailable_after_failures: 2,
            resolution_ttl_secs: 300,
            default_environment: "local".into(),
            endpoint_allowed_hosts: vec!["localhost".into(), "*.svc.test".into()],
            admin_rate_limit_per_minute: 10_000,
        };
        tweak(&mut cfg);
        let st = Self::state(&cfg, pool, &idp, provisioner);
        health::spawn_prober(st.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let app = router(st.clone());
        tokio::spawn(async move { axum::serve(l, app).await.ok() });
        Env {
            st,
            base,
            idp,
            http: reqwest::Client::new(),
            db_name,
            owner_pool,
            cfg,
        }
    }

    pub fn state(cfg: &Config, pool: PgPool, idp: &Arc<Idp>, provisioner: Provisioner) -> AppState {
        let verifier = TokenVerifier::from_jwks_url(
            VerifierConfig {
                issuers: vec![idp.issuer()],
                audience: cfg.audience.clone(),
                leeway_secs: 5,
                max_lifetime_secs: cfg.token_max_lifetime_secs,
                accept_plain_jwt_typ: false,
            },
            &cfg.jwks_url,
        );
        AppState::new(cfg.clone(), pool, verifier, provisioner)
    }

    /// Workload access token for a configured client.
    pub fn token(&self, client_id: &str) -> String {
        self.token_aud(client_id, &[CORE_AUD])
    }

    pub fn token_aud(&self, client_id: &str, aud: &[&str]) -> String {
        let c = self
            .idp
            .cfg
            .read()
            .unwrap()
            .clients
            .iter()
            .find(|c| c.client_id == client_id)
            .cloned()
            .expect("client");
        let aud: Vec<String> = aud.iter().map(|s| s.to_string()).collect();
        self.idp
            .sign(&self.idp.base_claims(&c.subject, client_id, &aud, ""))
    }

    pub fn user_token(&self, user: &str, client_id: &str, aud: &[&str]) -> String {
        let aud: Vec<String> = aud.iter().map(|s| s.to_string()).collect();
        self.idp.user_token(user, client_id, &aud).unwrap()
    }

    pub async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut rb = self.http.request(m, format!("{}{}", self.base, path));
        if let Some(t) = token {
            rb = rb.bearer_auth(t);
        }
        for (k, v) in headers {
            rb = rb.header(*k, *v);
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let resp = rb.send().await.unwrap();
        let status = StatusCode::from_u16(resp.status().as_u16()).unwrap();
        let text = resp.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    pub async fn op(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let t = self.token("op-client");
        self.call(method, path, Some(&t), body, &[]).await
    }

    pub async fn register_app(
        &self,
        slug: &str,
        project: &str,
        web: &str,
        backend: &str,
        key: &str,
    ) -> (StatusCode, Value) {
        let t = self.token("op-client");
        self.call(
            "POST",
            "/v0/applications",
            Some(&t),
            Some(json!({
                "slug": slug, "display_name": slug.to_uppercase(), "zitadel_project_id": project,
                "oauth_clients": [
                    {"kind": "web", "redirect_uris": [format!("https://{slug}.example/auth/callback")], "zitadel_client_id": web},
                    {"kind": "backend", "zitadel_client_id": backend},
                ],
                "requirements": [{"contract": "platform.example", "version": ">=1.0.0 <2.0.0"}],
            })),
            &[("idempotency-key", key)],
        )
        .await
    }

    pub fn contract_body(version: &str, openapi_extra: &str) -> Value {
        json!({
            "manifest": {"id": "platform.example", "version": version, "protocol": "http", "permissions": ["example:read", "example:write"]},
            "openapi": format!(
                "openapi: 3.1.0\ninfo: {{title: example, version: '{version}'}}\npaths:\n  /v1/echo:\n    post:\n      requestBody:\n        required: true\n        content:\n          application/json:\n            schema: {{type: object, required: [message], properties: {{message: {{type: string}}}}}}\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {{type: object, properties: {{message: {{type: string}}}}}}\n{openapi_extra}"
            ),
            "semantics": "echo semantics",
        })
    }

    pub async fn publish_contract(&self, version: &str) -> Value {
        let t = self.token("publisher-client");
        let (s, v) = self
            .call(
                "POST",
                "/v0/contracts",
                Some(&t),
                Some(Self::contract_body(version, "")),
                &[],
            )
            .await;
        assert!(
            s == StatusCode::CREATED || s == StatusCode::OK,
            "publish failed: {s} {v}"
        );
        v
    }

    pub async fn allow_deployer(&self, subject: &str, pattern: &str) {
        let (s, v) = self
            .op(
                "POST",
                "/v0/deployment-identities",
                Some(json!({"subject": subject, "slug_pattern": pattern})),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
    }

    pub async fn register_impl(
        &self,
        slug: &str,
        contract_digest: &str,
        version: &str,
        endpoint: &str,
    ) -> (StatusCode, Value) {
        let t = self.token("deployer-client");
        self.call(
            "POST",
            "/v0/implementations",
            Some(&t),
            Some(json!({
                "slug": slug, "release_version": "0.1.0",
                "implements": [{"contract": "platform.example", "version": version, "digest": contract_digest}],
                "deployments": [{"environment": "local", "endpoint": endpoint}],
            })),
            &[],
        )
        .await
    }

    pub async fn wait_deployment(&self, impl_id: &str, want: &str) {
        for _ in 0..60 {
            let (_, v) = self
                .op("GET", &format!("/v0/implementations/{impl_id}"), None)
                .await;
            if v["deployments"][0]["status"] == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        panic!("deployment never became {want}");
    }
}

impl Drop for Env {
    fn drop(&mut self) {}
}

pub async fn drop_db(env: Env) {
    let admin = admin_url();
    env.st.pool.close().await;
    env.owner_pool.close().await;
    if let Ok(p) = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin)
        .await
    {
        let _ = p
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", env.db_name).as_str())
            .await;
    }
}
