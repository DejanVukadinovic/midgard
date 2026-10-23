//! A tiny OIDC-style identity provider for tests and the offline demo stack.
//!
//! It serves discovery + JWKS, a `client_credentials` token endpoint and a dev-only endpoint
//! that mints ZITADEL-shaped *user* access tokens. Keys are generated at process start; nothing
//! here is suitable for production, and no secret material is committed to the repository.

use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{Form, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::{
    SecretKey,
    elliptic_curve::{rand_core::OsRng, sec1::ToEncodedPoint},
    pkcs8::{EncodePrivateKey, LineEnding},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const HOME_ORG_CLAIM: &str = "urn:zitadel:iam:user:resourceowner:id";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientCfg {
    pub client_id: String,
    pub client_secret: String,
    /// `sub` of the machine user behind the client.
    pub subject: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Audiences this client may request.
    #[serde(default)]
    pub audiences: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserCfg {
    pub sub: String,
    pub home_org: String,
    /// project_id -> role -> org ids in which the role is held.
    #[serde(default)]
    pub roles: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdpConfig {
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub clients: Vec<ClientCfg>,
    #[serde(default)]
    pub users: Vec<UserCfg>,
    #[serde(default = "default_ttl")]
    pub token_ttl_secs: u64,
}

fn default_ttl() -> u64 {
    300
}

pub struct Idp {
    pub cfg: std::sync::RwLock<IdpConfig>,
    kid: String,
    enc: EncodingKey,
    jwk: Value,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

impl Idp {
    pub fn new(cfg: IdpConfig) -> Arc<Self> {
        let sk = SecretKey::random(&mut OsRng);
        let pem = sk.to_pkcs8_pem(LineEnding::LF).expect("pem");
        let enc = EncodingKey::from_ec_pem(pem.as_bytes()).expect("ec key");
        let pt = sk.public_key().to_encoded_point(false);
        let kid = format!("kid-{}", URL_SAFE_NO_PAD.encode(&pt.x().unwrap()[..6]));
        let jwk = json!({
            "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": kid,
            "x": URL_SAFE_NO_PAD.encode(pt.x().unwrap()),
            "y": URL_SAFE_NO_PAD.encode(pt.y().unwrap()),
        });
        Arc::new(Self {
            cfg: std::sync::RwLock::new(cfg),
            kid,
            enc,
            jwk,
        })
    }

    pub fn issuer(&self) -> String {
        self.cfg.read().unwrap().issuer.clone()
    }

    /// Sign arbitrary claims as an access token (`typ: at+jwt`).
    pub fn sign(&self, claims: &Value) -> String {
        self.sign_with_typ(claims, "at+jwt")
    }

    pub fn sign_with_typ(&self, claims: &Value, typ: &str) -> String {
        let mut h = Header::new(Algorithm::ES256);
        h.kid = Some(self.kid.clone());
        h.typ = Some(typ.into());
        jsonwebtoken::encode(&h, claims, &self.enc).expect("sign")
    }

    pub fn base_claims(
        &self,
        sub: &str,
        client_id: &str,
        audiences: &[String],
        scope: &str,
    ) -> Value {
        let (iss, ttl) = {
            let c = self.cfg.read().unwrap();
            (c.issuer.clone(), c.token_ttl_secs)
        };
        let n = now();
        json!({
            "iss": iss, "sub": sub, "aud": audiences, "client_id": client_id,
            "scope": scope, "iat": n, "nbf": n, "exp": n + ttl, "jti": uuid_like(),
        })
    }

    pub fn user_token(
        &self,
        user: &str,
        client_id: &str,
        audiences: &[String],
    ) -> Result<String, String> {
        let u = self
            .cfg
            .read()
            .unwrap()
            .users
            .iter()
            .find(|u| u.sub == user)
            .cloned()
            .ok_or("unknown user")?;
        let mut claims = self.base_claims(&u.sub, client_id, audiences, "openid profile");
        claims[HOME_ORG_CLAIM] = json!(u.home_org);
        for (project, roles) in &u.roles {
            let mut m = serde_json::Map::new();
            for (role, orgs) in roles {
                let o: serde_json::Map<String, Value> = orgs
                    .iter()
                    .map(|o| (o.clone(), json!(format!("{o}.idp.test"))))
                    .collect();
                m.insert(role.clone(), Value::Object(o));
            }
            claims[format!("urn:zitadel:iam:org:project:{project}:roles")] = Value::Object(m);
        }
        Ok(self.sign(&claims))
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/oauth/v2/keys", get(keys))
            .route("/oauth/v2/token", post(token))
            .route("/dev/user-token", post(dev_user_token))
            .route("/dev/config", post(dev_config))
            .with_state(self.clone())
    }
}

fn uuid_like() -> String {
    let mut b = [0u8; 8];
    use p256::elliptic_curve::rand_core::RngCore;
    OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

async fn discovery(State(i): State<Arc<Idp>>) -> Json<Value> {
    let iss = i.issuer();
    Json(
        json!({"issuer": iss, "jwks_uri": format!("{iss}/oauth/v2/keys"), "token_endpoint": format!("{iss}/oauth/v2/token")}),
    )
}

async fn keys(State(i): State<Arc<Idp>>) -> Json<Value> {
    Json(json!({"keys": [i.jwk]}))
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    scope: Option<String>,
    audience: Option<String>,
}

async fn token(
    State(i): State<Arc<Idp>>,
    headers: HeaderMap,
    Form(f): Form<TokenForm>,
) -> impl IntoResponse {
    let err = |s: StatusCode, e: &str| (s, Json(json!({"error": e}))).into_response();
    if f.grant_type != "client_credentials" {
        return err(StatusCode::BAD_REQUEST, "unsupported_grant_type");
    }
    let (mut id, mut secret) = (f.client_id.clone(), f.client_secret.clone());
    if let Some(b) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        && let Some(dec) = base64::engine::general_purpose::STANDARD
            .decode(b)
            .ok()
            .and_then(|d| String::from_utf8(d).ok())
        && let Some((a, b)) = dec.split_once(':')
    {
        id = Some(a.to_string());
        secret = Some(b.to_string());
    }
    let client = i
        .cfg
        .read()
        .unwrap()
        .clients
        .iter()
        .find(|c| Some(&c.client_id) == id.as_ref())
        .cloned();
    let Some(c) = client.filter(|c| Some(&c.client_secret) == secret.as_ref()) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_client");
    };
    let requested_aud: Vec<String> = f
        .audience
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if requested_aud.iter().any(|a| !c.audiences.contains(a)) {
        return err(StatusCode::BAD_REQUEST, "invalid_target");
    }
    let aud = if requested_aud.is_empty() {
        c.audiences.clone()
    } else {
        requested_aud
    };
    let scopes: Vec<&str> = f
        .scope
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .collect();
    if scopes.iter().any(|s| !c.scopes.iter().any(|x| x == s)) {
        return err(StatusCode::BAD_REQUEST, "invalid_scope");
    }
    let scope = if scopes.is_empty() {
        c.scopes.join(" ")
    } else {
        scopes.join(" ")
    };
    let claims = i.base_claims(&c.subject, &c.client_id, &aud, &scope);
    let ttl = i.cfg.read().unwrap().token_ttl_secs;
    Json(json!({"access_token": i.sign(&claims), "token_type": "Bearer", "expires_in": ttl}))
        .into_response()
}

#[derive(Deserialize)]
struct UserTokenReq {
    user: String,
    client_id: String,
    #[serde(default)]
    audience: Vec<String>,
}

async fn dev_user_token(
    State(i): State<Arc<Idp>>,
    Json(r): Json<UserTokenReq>,
) -> impl IntoResponse {
    match i.user_token(&r.user, &r.client_id, &r.audience) {
        Ok(t) => Json(json!({"access_token": t})).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e}))).into_response(),
    }
}

/// Replace clients/users at runtime (dev stack bootstrap).
async fn dev_config(State(i): State<Arc<Idp>>, Json(c): Json<IdpConfig>) -> StatusCode {
    let mut cur = i.cfg.write().unwrap();
    cur.clients = c.clients;
    cur.users = c.users;
    StatusCode::NO_CONTENT
}

/// Start the IdP on an ephemeral port; returns (idp, base_url). `issuer` is rewritten to the bound URL.
pub async fn spawn(mut cfg: IdpConfig) -> (Arc<Idp>, String) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    cfg.issuer = url.clone();
    let idp = Idp::new(cfg);
    let app = idp.router();
    tokio::spawn(async move { axum::serve(l, app).await.ok() });
    (idp, url)
}

/// Serve the IdP on an existing listener (used by the `dev-idp` binary).
pub async fn serve(l: tokio::net::TcpListener, idp: Arc<Idp>) -> std::io::Result<()> {
    axum::serve(l, idp.router()).await
}
