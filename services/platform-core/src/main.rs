use std::sync::Arc;

use anyhow::Context;
use platform_auth::{TokenVerifier, VerifierConfig};
use platform_core::{
    MIGRATOR,
    config::{Config, ProvisionerKind},
    health,
    provisioning::{Provisioner, ZitadelClient},
    router,
    state::AppState,
};
use platform_persistence::connect;

fn init_tracing() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    let cfg = Config::from_env()?;
    match cmd.as_str() {
        "migrate" => {
            // Migrations run as a separate one-shot job with the owner role.
            let url = cfg
                .migration_database_url
                .as_deref()
                .unwrap_or(&cfg.database_url);
            let pool = connect(url, 2).await.context("connect for migration")?;
            MIGRATOR.run(&pool).await.context("run migrations")?;
            tracing::info!("migrations applied");
            Ok(())
        }
        "serve" => serve(cfg).await,
        other => anyhow::bail!("unknown command `{other}` (expected `serve` or `migrate`)"),
    }
}

async fn serve(cfg: Config) -> anyhow::Result<()> {
    // Bounded retries so startup ordering relative to PostgreSQL is not load-bearing.
    let mut pool = None;
    for attempt in 1..=30 {
        match connect(&cfg.database_url, 10).await {
            Ok(p) => {
                pool = Some(p);
                break;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "database not ready");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
    let pool = pool.context("database unavailable after retries")?;
    let verifier = TokenVerifier::from_jwks_url(
        VerifierConfig {
            issuers: vec![
                cfg.issuer.trim_end_matches('/').to_string(),
                cfg.issuer.clone(),
            ],
            audience: cfg.audience.clone(),
            leeway_secs: 30,
            max_lifetime_secs: cfg.token_max_lifetime_secs,
            accept_plain_jwt_typ: cfg.accept_plain_jwt_typ,
        },
        &cfg.jwks_url,
    );
    let provisioner = match cfg.provisioner {
        ProvisionerKind::Link => Provisioner::Link,
        ProvisionerKind::Zitadel => Provisioner::Zitadel(ZitadelClient::new(
            cfg.zitadel_api_url.as_deref().unwrap(),
            cfg.zitadel_api_token.clone().unwrap(),
            cfg.zitadel_org_id.clone(),
        )),
    };
    let addr = cfg.bind_addr.clone();
    let st = AppState::new(cfg, pool, verifier, provisioner);
    health::spawn_prober(st.clone());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "platform-core listening");
    axum::serve(listener, router(st))
        .with_graceful_shutdown(shutdown())
        .await?;
    let _ = Arc::new(());
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
