//! PostgreSQL helpers: pool construction and transaction-scoped tenant (RLS) context.
//!
//! Tenant-bearing tables must `ENABLE` + `FORCE ROW LEVEL SECURITY` with policies keyed on
//! `current_setting('app.tenant_id', true)`, and the application must connect as a role that
//! does not own the tables and lacks `BYPASSRLS`.

use std::time::Duration;

use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

pub use sqlx;

pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
}

/// Begin a transaction with `app.tenant_id` set via `set_config(..., is_local = true)`.
///
/// `is_local = true` scopes the setting to this transaction, so a pooled connection can never
/// carry one tenant's context into another request.
pub async fn begin_tenant_tx(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant_id.to_string())
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
