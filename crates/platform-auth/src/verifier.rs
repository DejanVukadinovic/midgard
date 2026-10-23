use std::{collections::BTreeSet, sync::Arc, time::Duration};

use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use serde_json::Value;

use crate::JwksCache;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing or malformed bearer token")]
    Malformed,
    #[error("token rejected: {0}")]
    Invalid(&'static str),
    #[error("signing key unknown")]
    UnknownKey,
    #[error("identity provider keys unavailable")]
    JwksUnavailable,
}

#[derive(Clone)]
pub struct VerifierConfig {
    /// Exact-match issuer allowlist.
    pub issuers: Vec<String>,
    /// The audience *this* service requires in `aud`.
    pub audience: String,
    pub leeway_secs: u64,
    /// Reject tokens whose `exp - iat` exceeds this (bounds the revocation window).
    pub max_lifetime_secs: u64,
}

/// A cryptographically verified access token.
#[derive(Debug, Clone)]
pub struct VerifiedToken {
    pub subject: String,
    pub client_id: Option<String>,
    pub scopes: BTreeSet<String>,
    pub audiences: Vec<String>,
    pub claims: Value,
}

impl VerifiedToken {
    pub fn has_scope(&self, s: &str) -> bool {
        self.scopes.contains(s)
    }
}

#[derive(Clone)]
pub struct TokenVerifier {
    cfg: Arc<VerifierConfig>,
    jwks: Arc<JwksCache>,
}

impl TokenVerifier {
    pub fn new(cfg: VerifierConfig, jwks: Arc<JwksCache>) -> Self {
        Self {
            cfg: Arc::new(cfg),
            jwks,
        }
    }

    pub fn from_jwks_url(cfg: VerifierConfig, jwks_url: &str) -> Self {
        let jwks = Arc::new(JwksCache::new(
            jwks_url,
            Duration::from_secs(600),
            Duration::from_secs(10),
        ));
        Self::new(cfg, jwks)
    }

    pub fn audience(&self) -> &str {
        &self.cfg.audience
    }

    /// Verify `token` for this service's configured audience.
    pub async fn verify(&self, token: &str) -> Result<VerifiedToken, AuthError> {
        self.verify_for_audiences(token, std::slice::from_ref(&self.cfg.audience))
            .await
    }

    /// Verify `token` against explicit audiences (any one may match); used when a service
    /// validates a user token minted for one of its clients rather than for itself.
    pub async fn verify_for_audiences(
        &self,
        token: &str,
        audiences: &[String],
    ) -> Result<VerifiedToken, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::Malformed)?;
        let alg = header.alg;
        if !matches!(alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(AuthError::Invalid("unsupported algorithm"));
        }
        if let Some(typ) = &header.typ
            && !typ.eq_ignore_ascii_case("at+jwt")
        {
            return Err(AuthError::Invalid("not an access token"));
        }
        let kid = header.kid.ok_or(AuthError::Invalid("missing kid"))?;
        let key = self.jwks.key(&kid).await?;

        let mut v = Validation::new(alg);
        v.set_issuer(&self.cfg.issuers);
        v.set_audience(audiences);
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        v.validate_nbf = true;
        v.leeway = self.cfg.leeway_secs;
        let data = decode::<Value>(token, &key, &v).map_err(|e| {
            use jsonwebtoken::errors::ErrorKind::*;
            AuthError::Invalid(match e.kind() {
                ExpiredSignature => "expired",
                InvalidAudience => "wrong audience",
                InvalidIssuer => "wrong issuer",
                ImmatureSignature => "not yet valid",
                InvalidSignature => "bad signature",
                _ => "invalid token",
            })
        })?;
        let claims = data.claims;

        // ID tokens must never be accepted as API access tokens.
        if claims.get("nonce").is_some()
            || claims.get("at_hash").is_some()
            || claims.get("token_use").and_then(Value::as_str) == Some("id")
        {
            return Err(AuthError::Invalid("id token presented as access token"));
        }
        if let (Some(iat), Some(exp)) = (
            claims.get("iat").and_then(Value::as_u64),
            claims.get("exp").and_then(Value::as_u64),
        ) {
            if exp.saturating_sub(iat) > self.cfg.max_lifetime_secs {
                return Err(AuthError::Invalid("token lifetime exceeds policy"));
            }
        } else {
            return Err(AuthError::Invalid("missing iat"));
        }

        let subject = claims
            .get("sub")
            .and_then(Value::as_str)
            .ok_or(AuthError::Invalid("missing sub"))?
            .to_string();
        let client_id = claims
            .get("client_id")
            .or_else(|| claims.get("azp"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let scopes = claims
            .get("scope")
            .and_then(Value::as_str)
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        let audiences = match claims.get("aud") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            _ => vec![],
        };
        Ok(VerifiedToken {
            subject,
            client_id,
            scopes,
            audiences,
            claims,
        })
    }
}
