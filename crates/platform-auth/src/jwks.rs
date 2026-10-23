use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use jsonwebtoken::{DecodingKey, jwk::JwkSet};
use tokio::sync::RwLock;

use crate::AuthError;

struct State {
    keys: HashMap<String, Arc<DecodingKey>>,
    fetched_at: Option<Instant>,
    last_attempt: Option<Instant>,
}

/// JWKS cache: refresh on TTL expiry or unknown `kid` (rate limited).
pub struct JwksCache {
    url: String,
    http: reqwest::Client,
    ttl: Duration,
    min_refresh_interval: Duration,
    state: RwLock<State>,
}

impl JwksCache {
    pub fn new(url: impl Into<String>, ttl: Duration, min_refresh_interval: Duration) -> Self {
        Self {
            url: url.into(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client"),
            ttl,
            min_refresh_interval,
            state: RwLock::new(State {
                keys: HashMap::new(),
                fetched_at: None,
                last_attempt: None,
            }),
        }
    }

    pub async fn key(&self, kid: &str) -> Result<Arc<DecodingKey>, AuthError> {
        {
            let st = self.state.read().await;
            let fresh = st.fetched_at.is_some_and(|t| t.elapsed() < self.ttl);
            if fresh && let Some(k) = st.keys.get(kid) {
                return Ok(k.clone());
            }
        }
        self.refresh().await?;
        let st = self.state.read().await;
        st.keys.get(kid).cloned().ok_or(AuthError::UnknownKey)
    }

    async fn refresh(&self) -> Result<(), AuthError> {
        let mut st = self.state.write().await;
        // Another task may have refreshed while we waited for the lock, and unknown-kid
        // floods must not hammer the IdP.
        if st
            .last_attempt
            .is_some_and(|t| t.elapsed() < self.min_refresh_interval)
        {
            return Ok(());
        }
        st.last_attempt = Some(Instant::now());
        let resp = self.http.get(&self.url).send().await.map_err(|e| {
            tracing::warn!(error = %e, "jwks fetch failed");
            AuthError::JwksUnavailable
        })?;
        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "jwks fetch non-200");
            return Err(AuthError::JwksUnavailable);
        }
        let set: JwkSet = resp.json().await.map_err(|_| AuthError::JwksUnavailable)?;
        let mut keys = HashMap::new();
        for jwk in set.keys {
            if let (Some(kid), Ok(key)) = (jwk.common.key_id.clone(), DecodingKey::from_jwk(&jwk)) {
                keys.insert(kid, Arc::new(key));
            }
        }
        st.keys = keys;
        st.fetched_at = Some(Instant::now());
        Ok(())
    }
}
