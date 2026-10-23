//! JWKS rotation: a token signed by a new key (unknown `kid`) triggers a refresh; forged `kid`s do not.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{Json, Router, extract::State, routing::get};
use platform_auth::{JwksCache, TokenVerifier, VerifierConfig};
use platform_testkit::{ClientCfg, IdpConfig};
use serde_json::Value;

#[tokio::test]
async fn verifier_picks_up_rotated_keys() {
    let cfg = || IdpConfig {
        clients: vec![ClientCfg {
            client_id: "c".into(),
            client_secret: "s".into(),
            subject: "sub".into(),
            scopes: vec![],
            audiences: vec!["aud".into()],
        }],
        ..Default::default()
    };
    let (old, old_url) = platform_testkit::spawn(cfg()).await;
    let (new, new_url) = platform_testkit::spawn(cfg()).await;
    let http = reqwest::Client::new();
    let old_jwks: Value = http
        .get(format!("{old_url}/oauth/v2/keys"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let new_jwks: Value = http
        .get(format!("{new_url}/oauth/v2/keys"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // a JWKS endpoint whose content we can "rotate"
    let current = Arc::new(Mutex::new(old_jwks));
    let app = Router::new()
        .route(
            "/keys",
            get(
                |State(c): State<Arc<Mutex<Value>>>| async move { Json(c.lock().unwrap().clone()) },
            ),
        )
        .with_state(current.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/keys", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.ok() });

    let issuer = old.issuer();
    let v = TokenVerifier::new(
        VerifierConfig {
            issuers: vec![issuer.clone()],
            audience: "aud".into(),
            leeway_secs: 5,
            max_lifetime_secs: 3600,
        },
        Arc::new(JwksCache::new(
            url,
            Duration::from_secs(600),
            Duration::from_millis(0),
        )),
    );
    // issuer claim must match: re-sign with the verifier's issuer for the "new" key too
    let t_old = old.sign(&old.base_claims("sub", "c", &["aud".to_string()], ""));
    let mut claims_new = new.base_claims("sub", "c", &["aud".to_string()], "");
    claims_new["iss"] = Value::String(issuer);
    let t_new = new.sign(&claims_new);

    assert!(v.verify(&t_old).await.is_ok());
    assert!(
        v.verify(&t_new).await.is_err(),
        "unknown kid before rotation"
    );
    *current.lock().unwrap() = new_jwks; // IdP rotates keys
    assert!(
        v.verify(&t_new).await.is_ok(),
        "unknown kid triggers a JWKS refresh"
    );
}
