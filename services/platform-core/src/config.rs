use std::{collections::BTreeMap, env, time::Duration};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub migration_database_url: Option<String>,
    pub issuer: String,
    pub jwks_url: String,
    pub audience: String,
    pub token_max_lifetime_secs: u64,
    pub operator_subjects: Vec<String>,
    /// subject -> allowed namespace prefixes (empty = any namespace)
    pub publishers: BTreeMap<String, Vec<String>>,
    pub provisioner: ProvisionerKind,
    pub zitadel_api_url: Option<String>,
    pub zitadel_api_token: Option<String>,
    pub zitadel_org_id: Option<String>,
    pub health_check_interval: Duration,
    pub unavailable_after_failures: i32,
    pub resolution_ttl_secs: u64,
    pub default_environment: String,
    /// Explicit endpoint host allowlist (exact or `*.suffix`). Empty = default safe policy.
    pub endpoint_allowed_hosts: Vec<String>,
    pub admin_rate_limit_per_minute: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvisionerKind {
    /// Link pre-existing IdP projects/clients supplied by the operator.
    Link,
    /// Create projects/clients through the ZITADEL management API.
    Zitadel,
}

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|s| !s.is_empty())
}

fn file_or_var(name: &str) -> Option<String> {
    if let Some(p) = var(&format!("{name}_FILE")) {
        return std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string());
    }
    var(name)
}

fn list(name: &str) -> Vec<String> {
    var(name)
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn num<T: std::str::FromStr>(name: &str, default: T) -> anyhow::Result<T> {
    match var(name) {
        None => Ok(default),
        Some(v) => v
            .parse()
            .map_err(|_| anyhow::anyhow!("{name} is not a valid number")),
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let issuer =
            var("ZITADEL_ISSUER").ok_or_else(|| anyhow::anyhow!("ZITADEL_ISSUER is required"))?;
        let jwks_url = var("ZITADEL_JWKS_URL")
            .unwrap_or_else(|| format!("{}/oauth/v2/keys", issuer.trim_end_matches('/')));
        let mut publishers = BTreeMap::new();
        for entry in list("PLATFORM_PUBLISHER_SUBJECTS") {
            match entry.split_once('=') {
                Some((s, prefixes)) => publishers.insert(
                    s.to_string(),
                    prefixes.split('|').map(str::to_string).collect(),
                ),
                None => publishers.insert(entry, vec![]),
            };
        }
        let provisioner = match var("PLATFORM_PROVISIONER").as_deref() {
            None | Some("link") => ProvisionerKind::Link,
            Some("zitadel") => ProvisionerKind::Zitadel,
            Some(o) => anyhow::bail!("PLATFORM_PROVISIONER must be `link` or `zitadel`, got `{o}`"),
        };
        let cfg = Self {
            bind_addr: var("PLATFORM_BIND_ADDR").unwrap_or_else(|| "0.0.0.0:8080".into()),
            database_url: file_or_var("PLATFORM_DATABASE_URL")
                .ok_or_else(|| anyhow::anyhow!("PLATFORM_DATABASE_URL is required"))?,
            migration_database_url: file_or_var("PLATFORM_MIGRATION_DATABASE_URL"),
            issuer,
            jwks_url,
            audience: var("PLATFORM_AUDIENCE").unwrap_or_else(|| "midgard-core".into()),
            token_max_lifetime_secs: num("PLATFORM_TOKEN_MAX_LIFETIME_SECONDS", 3600)?,
            operator_subjects: list("PLATFORM_OPERATOR_SUBJECTS"),
            publishers,
            provisioner,
            zitadel_api_url: var("ZITADEL_API_URL"),
            zitadel_api_token: file_or_var("ZITADEL_API_TOKEN"),
            zitadel_org_id: var("ZITADEL_ORG_ID"),
            health_check_interval: Duration::from_secs(num(
                "REGISTRY_HEALTH_CHECK_INTERVAL_SECONDS",
                30,
            )?),
            unavailable_after_failures: num("REGISTRY_UNAVAILABLE_AFTER_FAILURES", 3)?,
            resolution_ttl_secs: num("REGISTRY_RESOLUTION_TTL_SECONDS", 300)?,
            default_environment: var("REGISTRY_DEFAULT_ENVIRONMENT")
                .unwrap_or_else(|| "local".into()),
            endpoint_allowed_hosts: list("REGISTRY_ENDPOINT_ALLOWED_HOSTS"),
            admin_rate_limit_per_minute: num("PLATFORM_ADMIN_RATE_LIMIT_PER_MINUTE", 600)?,
        };
        if cfg.operator_subjects.is_empty() {
            anyhow::bail!("PLATFORM_OPERATOR_SUBJECTS must list at least one operator identity");
        }
        if cfg.provisioner == ProvisionerKind::Zitadel
            && (cfg.zitadel_api_url.is_none() || cfg.zitadel_api_token.is_none())
        {
            anyhow::bail!(
                "ZITADEL_API_URL and ZITADEL_API_TOKEN(_FILE) are required for the zitadel provisioner"
            );
        }
        Ok(cfg)
    }
}
