//! DEVELOPMENT-ONLY identity provider (keys are generated in memory at start-up).
//!
//! `DEV_IDP_ISSUER`  issuer URL as seen by the other services (e.g. http://dev-idp:9000)
//! `DEV_IDP_CONFIG`  path to a JSON file with `clients` and `users`
//! `DEV_IDP_BIND`    listen address (default 0.0.0.0:9000)

use anyhow::Context;
use platform_testkit::{Idp, IdpConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let mut cfg: IdpConfig = match std::env::var("DEV_IDP_CONFIG") {
        Ok(p) => serde_json::from_str(
            &std::fs::read_to_string(&p).with_context(|| format!("read {p}"))?,
        )?,
        Err(_) => IdpConfig::default(),
    };
    cfg.issuer = std::env::var("DEV_IDP_ISSUER").context("DEV_IDP_ISSUER is required")?;
    let bind = std::env::var("DEV_IDP_BIND").unwrap_or_else(|_| "0.0.0.0:9000".into());
    let idp = Idp::new(cfg);
    let l = tokio::net::TcpListener::bind(&bind).await?;
    tracing::warn!(%bind, "dev-idp listening — DEVELOPMENT ONLY, unauthenticated /dev endpoints are exposed");
    axum_serve(l, idp).await
}

async fn axum_serve(l: tokio::net::TcpListener, idp: std::sync::Arc<Idp>) -> anyhow::Result<()> {
    platform_testkit::serve(l, idp).await?;
    Ok(())
}
