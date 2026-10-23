use std::time::Duration;

use anyhow::Context;
use example_capability::{AppState, CoreCredentials, MIGRATOR, router};
use platform_auth::{TokenVerifier, VerifierConfig};
use platform_persistence::connect;
use serde_json::json;

fn var(n: &str) -> Option<String> {
    std::env::var(n).ok().filter(|s| !s.is_empty())
}

fn req(n: &str) -> anyhow::Result<String> {
    var(n).with_context(|| format!("{n} is required"))
}

fn secret(n: &str) -> anyhow::Result<String> {
    match var(&format!("{n}_FILE")) {
        Some(p) => Ok(std::fs::read_to_string(p)?.trim().to_string()),
        None => req(n),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let url = var("CAPABILITY_MIGRATION_DATABASE_URL")
            .or_else(|| var("CAPABILITY_DATABASE_URL"))
            .context("CAPABILITY_MIGRATION_DATABASE_URL required")?;
        MIGRATOR.run(&connect(&url, 2).await?).await?;
        tracing::info!("migrations applied");
        return Ok(());
    }
    let issuer = req("CAPABILITY_ISSUER")?;
    let jwks = var("CAPABILITY_JWKS_URL")
        .unwrap_or_else(|| format!("{}/oauth/v2/keys", issuer.trim_end_matches('/')));
    let verifier = TokenVerifier::from_jwks_url(
        VerifierConfig {
            issuers: vec![issuer.clone(), issuer.trim_end_matches('/').to_string()],
            audience: var("CAPABILITY_AUDIENCE").unwrap_or_else(|| "platform.example".into()),
            leeway_secs: 30,
            max_lifetime_secs: 3600,
        },
        &jwks,
    );
    let mut pool = None;
    for attempt in 1..=30 {
        match connect(&req("CAPABILITY_DATABASE_URL")?, 8).await {
            Ok(p) => {
                pool = Some(p);
                break;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "database not ready");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    let pool = pool.context("database unavailable")?;
    let creds = CoreCredentials::new(
        var("CAPABILITY_TOKEN_URL")
            .unwrap_or_else(|| format!("{}/oauth/v2/token", issuer.trim_end_matches('/'))),
        req("CAPABILITY_CLIENT_ID")?,
        secret("CAPABILITY_CLIENT_SECRET")?,
        var("CAPABILITY_CORE_AUDIENCE").unwrap_or_else(|| "midgard-core".into()),
    );
    let core_url = req("PLATFORM_CORE_URL")?;
    let st = AppState::new(
        pool,
        verifier,
        core_url.clone(),
        creds,
        Duration::from_secs(15),
    );

    // Optional dynamic self-registration through the approved deployment identity.
    if var("CAPABILITY_SELF_REGISTER").as_deref() == Some("true") {
        let st2 = st.clone();
        let slug = var("CAPABILITY_SLUG").unwrap_or_else(|| "example-capability".into());
        let endpoint = req("CAPABILITY_PUBLIC_ENDPOINT")?;
        let env_name = var("CAPABILITY_ENVIRONMENT").unwrap_or_else(|| "local".into());
        let digests = example_capability::contract_digests();
        tokio::spawn(async move {
            let body = json!({
                "slug": slug, "release_version": env!("CARGO_PKG_VERSION"),
                "implements": digests.iter().map(|(v, d)| json!({"contract": "platform.example", "version": v, "digest": d})).collect::<Vec<_>>(),
                "deployments": [{"environment": env_name, "endpoint": endpoint}],
            });
            loop {
                let res = async {
                    let t = st2.creds.token(&st2.http).await?;
                    let r = st2
                        .http
                        .post(format!("{}/v0/implementations", st2.core_url))
                        .bearer_auth(t)
                        .json(&body)
                        .send()
                        .await
                        .map_err(|e| e.to_string())?;
                    if r.status().is_success() {
                        Ok(())
                    } else {
                        Err(format!("registration returned {}", r.status()))
                    }
                }
                .await;
                match res {
                    Ok(()) => {
                        tracing::info!("registered with core");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "self-registration failed; retrying");
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
        });
    }
    let addr = var("CAPABILITY_BIND_ADDR").unwrap_or_else(|| "0.0.0.0:8080".into());
    let l = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "example-capability listening");
    axum::serve(l, router(st)).await?;
    Ok(())
}
