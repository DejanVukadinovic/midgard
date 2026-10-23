use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use platform_auth::TokenVerifier;
use platform_persistence::sqlx::PgPool;

use crate::{config::Config, provisioning::Provisioner, ratelimit::RateLimiter};

#[derive(Default)]
pub struct Metrics {
    pub resolve_ok: AtomicU64,
    pub resolve_denied: AtomicU64,
    pub registrations: AtomicU64,
    pub provisioning_failures: AtomicU64,
    pub probe_failures: AtomicU64,
}

impl Metrics {
    pub fn inc(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub verifier: TokenVerifier,
    pub cfg: Arc<Config>,
    pub provisioner: Arc<Provisioner>,
    pub limiter: Arc<RateLimiter>,
    pub metrics: Arc<Metrics>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(
        cfg: Config,
        pool: PgPool,
        verifier: TokenVerifier,
        provisioner: Provisioner,
    ) -> Self {
        let limiter = Arc::new(RateLimiter::new(cfg.admin_rate_limit_per_minute));
        // Health probes must never follow redirects (SSRF hardening).
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client");
        Self {
            pool,
            verifier,
            cfg: Arc::new(cfg),
            provisioner: Arc::new(provisioner),
            limiter,
            metrics: Arc::new(Metrics::default()),
            http,
        }
    }
}
