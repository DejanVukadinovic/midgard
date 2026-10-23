//! Deployment readiness probing. Health is *readiness only*; it proves nothing about
//! authorization or contract conformance.

use std::net::SocketAddr;

use platform_persistence::sqlx::{self, Row};
use uuid::Uuid;

use crate::{
    endpoint_policy::{forbidden_ip, host_allowed},
    state::{AppState, Metrics},
};

pub fn spawn_prober(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(st.cfg.health_check_interval);
        loop {
            tick.tick().await;
            probe_all(&st).await;
        }
    });
}

pub async fn probe_all(st: &AppState) {
    let rows = sqlx::query("SELECT id FROM deployments WHERE status <> 'disabled'")
        .fetch_all(&st.pool)
        .await;
    match rows {
        Ok(rows) => {
            let ids: Vec<Uuid> = rows.iter().map(|r| r.get("id")).collect();
            futures_join(st, ids).await;
        }
        Err(e) => tracing::warn!(error = %e, "probe: listing deployments failed"),
    }
}

async fn futures_join(st: &AppState, ids: Vec<Uuid>) {
    let mut set = tokio::task::JoinSet::new();
    for id in ids {
        let st = st.clone();
        set.spawn(async move { probe_one(&st, id).await });
    }
    while set.join_next().await.is_some() {}
}

pub async fn probe_one(st: &AppState, id: Uuid) {
    let Ok(Some(row)) = sqlx::query(
        "SELECT logical_endpoint FROM deployments WHERE id=$1 AND status <> 'disabled'",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    else {
        return;
    };
    let endpoint: String = row.get("logical_endpoint");
    let ok = check_ready(st, &endpoint).await;
    let res = if ok {
        sqlx::query("UPDATE deployments SET status='healthy', consecutive_failures=0, last_health_at=now(), updated_at=now() WHERE id=$1 AND status <> 'disabled'")
            .bind(id)
            .execute(&st.pool)
            .await
    } else {
        Metrics::inc(&st.metrics.probe_failures);
        sqlx::query(
            "UPDATE deployments SET consecutive_failures = consecutive_failures + 1,
               status = CASE WHEN consecutive_failures + 1 >= $2 THEN 'unavailable' ELSE 'degraded' END,
               updated_at=now()
             WHERE id=$1 AND status <> 'disabled'",
        )
        .bind(id)
        .bind(st.cfg.unavailable_after_failures)
        .execute(&st.pool)
        .await
    };
    if let Err(e) = res {
        tracing::warn!(error = %e, "probe: status update failed");
    }
}

async fn check_ready(st: &AppState, endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let allowlisted = host_allowed(host, &st.cfg.endpoint_allowed_hosts);
    if !allowlisted {
        // Resolve first and refuse loopback/link-local/metadata targets.
        match tokio::net::lookup_host((host, port)).await {
            Ok(addrs) => {
                let addrs: Vec<SocketAddr> = addrs.collect();
                if addrs.is_empty() || addrs.iter().any(|a| forbidden_ip(a.ip())) {
                    tracing::warn!(%host, "probe refused: endpoint resolves to a forbidden address");
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    let probe = format!("{}/health/ready", endpoint.trim_end_matches('/'));
    matches!(st.http.get(probe).send().await, Ok(r) if r.status().is_success())
}
