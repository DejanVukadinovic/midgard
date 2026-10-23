//! Sample implementation of `platform.example`.
//!
//! It authenticates callers itself (workload token, audience = this service), then asks MIDGARD
//! core — authenticated as its approved deployment identity — whether the calling application
//! may use the contract on this implementation and, for tenant-owned data, which tenant the
//! delegated user maps to. Nothing about tenancy is taken from caller-supplied identifiers.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::get,
};
use platform_auth::{TokenVerifier, VerifiedToken};
use platform_http::{ApiResult, Problem, internal_error};
use platform_persistence::{begin_tenant_tx, sqlx::PgPool};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub const CONTRACT: &str = "platform.example";
pub const USER_TOKEN_HEADER: &str = "x-delegated-user-token";

/// The contract bundles this build implements, embedded at compile time so that the digests the
/// service claims at registration are those of the exact files it was built and tested against.
const CONTRACT_FILES: [(&str, &str, &str, &str); 2] = [
    (
        "1.0.0",
        include_str!("../../../contracts/platform/example/v1/1.0.0/manifest.yaml"),
        include_str!("../../../contracts/platform/example/v1/1.0.0/openapi.yaml"),
        include_str!("../../../contracts/platform/example/v1/1.0.0/semantics.md"),
    ),
    (
        "1.1.0",
        include_str!("../../../contracts/platform/example/v1/1.1.0/manifest.yaml"),
        include_str!("../../../contracts/platform/example/v1/1.1.0/openapi.yaml"),
        include_str!("../../../contracts/platform/example/v1/1.1.0/semantics.md"),
    ),
];

/// (version, digest) pairs of the implemented contract versions.
pub fn contract_digests() -> Vec<(String, String)> {
    CONTRACT_FILES
        .iter()
        .map(|(v, m, o, s)| {
            let manifest: platform_contracts::Manifest =
                serde_yaml::from_str(m).expect("embedded manifest");
            let bundle =
                platform_contracts::ContractBundle::parse(manifest, o.to_string(), s.to_string())
                    .expect("embedded contract");
            (v.to_string(), bundle.digest())
        })
        .collect()
}

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Obtains access tokens for this service's own deployment identity (client credentials).
pub struct CoreCredentials {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub audience: String,
    cache: tokio::sync::Mutex<Option<(String, Instant)>>,
}

impl CoreCredentials {
    pub fn new(
        token_url: String,
        client_id: String,
        client_secret: String,
        audience: String,
    ) -> Self {
        Self {
            token_url,
            client_id,
            client_secret,
            audience,
            cache: tokio::sync::Mutex::new(None),
        }
    }

    pub async fn token(&self, http: &reqwest::Client) -> Result<String, String> {
        let mut c = self.cache.lock().await;
        if let Some((t, exp)) = c.as_ref()
            && Instant::now() < *exp
        {
            return Ok(t.clone());
        }
        let resp = http
            .post(&self.token_url)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[
                ("grant_type", "client_credentials"),
                ("audience", self.audience.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("token request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("token endpoint returned {}", resp.status()));
        }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        let tok = v["access_token"]
            .as_str()
            .ok_or("no access_token")?
            .to_string();
        let ttl = v["expires_in"]
            .as_u64()
            .unwrap_or(60)
            .saturating_sub(30)
            .max(5);
        *c = Some((tok.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(tok)
    }
}

/// client_id -> (expiry, application, permissions)
pub type AuthzCache = Arc<Mutex<HashMap<String, (Instant, String, Vec<String>)>>>;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub verifier: TokenVerifier,
    pub core_url: String,
    pub creds: Arc<CoreCredentials>,
    pub http: reqwest::Client,
    /// Short-lived cache of tenant-less authorization decisions: client_id -> (expiry, application, permissions).
    pub authz_cache: AuthzCache,
    pub authz_ttl: Duration,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        verifier: TokenVerifier,
        core_url: String,
        creds: CoreCredentials,
        authz_ttl: Duration,
    ) -> Self {
        Self {
            pool,
            verifier,
            core_url: core_url.trim_end_matches('/').to_string(),
            creds: Arc::new(creds),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("client"),
            authz_cache: Arc::default(),
            authz_ttl,
        }
    }
}

async fn workload(st: &AppState, headers: &HeaderMap) -> ApiResult<VerifiedToken> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| Problem::unauthorized("missing bearer token"))?;
    st.verifier.verify(raw.trim()).await.map_err(|e| {
        tracing::info!(reason = %e, "workload token rejected");
        Problem::unauthorized("invalid access token")
    })
}

struct Decision {
    application: String,
    permissions: Vec<String>,
    tenant: Option<Value>,
}

async fn authorize(
    st: &AppState,
    tok: &VerifiedToken,
    user_token: Option<&str>,
    org: Option<&str>,
) -> ApiResult<Decision> {
    let client_id = tok
        .client_id
        .clone()
        .ok_or_else(|| Problem::forbidden("token has no client identity"))?;
    if user_token.is_none()
        && let Some((exp, app, perms)) = st.authz_cache.lock().unwrap().get(&client_id)
        && Instant::now() < *exp
    {
        return Ok(Decision {
            application: app.clone(),
            permissions: perms.clone(),
            tenant: None,
        });
    }
    let core_token = st.creds.token(&st.http).await.map_err(|e| {
        tracing::error!(error = %e, "cannot obtain deployment identity token");
        Problem::unavailable(
            "authorization_unavailable",
            "cannot reach authorization service",
        )
    })?;
    let resp = st
        .http
        .post(format!("{}/v0/authorize", st.core_url))
        .bearer_auth(core_token)
        .json(&json!({"client_id": client_id, "contract": CONTRACT, "user_token": user_token, "organization_id": org}))
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "core unreachable");
            Problem::unavailable("authorization_unavailable", "cannot reach authorization service")
        })?;
    match resp.status() {
        s if s.is_success() => {}
        StatusCode::FORBIDDEN | StatusCode::UNPROCESSABLE_ENTITY => {
            let code = if resp.status() == StatusCode::UNPROCESSABLE_ENTITY {
                "organization_selection_required"
            } else {
                "forbidden"
            };
            let mut p = Problem::new(resp.status(), code, "not authorized");
            if code == "forbidden" {
                p = Problem::forbidden("not authorized");
            }
            return Err(p);
        }
        s => {
            tracing::error!(status = %s, "unexpected authorize response");
            return Err(Problem::unavailable(
                "authorization_unavailable",
                "authorization service error",
            ));
        }
    }
    let v: Value = resp.json().await.map_err(internal_error)?;
    let permissions: Vec<String> = v["permissions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let application = v["application"].as_str().unwrap_or_default().to_string();
    if user_token.is_none() {
        st.authz_cache.lock().unwrap().insert(
            client_id.clone(),
            (
                Instant::now() + st.authz_ttl,
                application.clone(),
                permissions.clone(),
            ),
        );
    }
    Ok(Decision {
        application,
        permissions,
        tenant: v.get("tenant").filter(|t| !t.is_null()).cloned(),
    })
}

fn need(d: &Decision, perm: &str) -> ApiResult<()> {
    if d.permissions.iter().any(|p| p == perm) {
        Ok(())
    } else {
        Err(Problem::forbidden(format!("missing permission {perm}")))
    }
}

fn user_token(headers: &HeaderMap) -> ApiResult<&str> {
    headers
        .get(USER_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            Problem::bad_request(
                "user_token_required",
                "X-Delegated-User-Token header is required",
            )
        })
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> ApiResult<T> {
    serde_json::from_slice(body)
        .map_err(|_| Problem::bad_request("invalid_body", "malformed JSON body"))
}

#[derive(Deserialize)]
struct Echo {
    message: String,
}

async fn echo(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    // Authenticate and authorize before touching the body.
    let tok = workload(&st, &headers).await?;
    let d = authorize(&st, &tok, None, None).await?;
    need(&d, "example:read")?;
    let b: Echo = parse(&body)?;
    if b.message.len() > 1024 {
        return Err(Problem::bad_request(
            "message_too_long",
            "message exceeds 1024 bytes",
        ));
    }
    Ok(Json(
        json!({"message": b.message, "application": d.application}),
    ))
}

async fn ping(State(st): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let tok = workload(&st, &headers).await?;
    let d = authorize(&st, &tok, None, None).await?;
    need(&d, "example:read")?;
    Ok(Json(json!({"pong": true})))
}

fn tenant_id(d: &Decision) -> ApiResult<Uuid> {
    d.tenant
        .as_ref()
        .and_then(|t| t["tenant_id"].as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Problem::forbidden("no tenant context"))
}

async fn list_notes(State(st): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let tok = workload(&st, &headers).await?;
    let org = headers
        .get("x-organization-id")
        .and_then(|v| v.to_str().ok());
    let d = authorize(&st, &tok, Some(user_token(&headers)?), org).await?;
    need(&d, "example:read")?;
    let mut tx = begin_tenant_tx(&st.pool, tenant_id(&d)?)
        .await
        .map_err(internal_error)?;
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, body FROM notes ORDER BY created_at, id")
            .fetch_all(&mut *tx)
            .await
            .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;
    Ok(Json(
        json!({"items": rows.into_iter().map(|(id, body)| json!({"id": id, "body": body})).collect::<Vec<_>>()}),
    ))
}

#[derive(Deserialize)]
struct NewNote {
    body: String,
}

async fn create_note(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let tok = workload(&st, &headers).await?;
    let org = headers
        .get("x-organization-id")
        .and_then(|v| v.to_str().ok());
    let d = authorize(&st, &tok, Some(user_token(&headers)?), org).await?;
    need(&d, "example:write")?;
    let b: NewNote = parse(&body)?;
    if b.body.is_empty() || b.body.len() > 4096 {
        return Err(Problem::bad_request(
            "invalid_note",
            "body must be 1-4096 bytes",
        ));
    }
    let tenant = tenant_id(&d)?;
    let mut tx = begin_tenant_tx(&st.pool, tenant)
        .await
        .map_err(internal_error)?;
    let (id,): (Uuid,) =
        sqlx::query_as("INSERT INTO notes (tenant_id, body) VALUES ($1, $2) RETURNING id")
            .bind(tenant)
            .bind(&b.body)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;
    Ok((StatusCode::CREATED, Json(json!({"id": id, "body": b.body}))))
}

async fn live() -> StatusCode {
    StatusCode::OK
}

async fn ready(State(st): State<AppState>) -> StatusCode {
    match sqlx::query("SELECT 1").execute(&st.pool).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub fn router(st: AppState) -> Router {
    Router::new()
        .route("/v1/echo", axum::routing::post(echo))
        .route("/v1/ping", get(ping))
        .route("/v1/notes", get(list_notes).post(create_note))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
        .with_state(st)
}
