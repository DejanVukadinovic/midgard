use platform_http::Problem;
use platform_persistence::sqlx::{self, PgConnection};
use serde_json::Value;

use crate::{authn::Caller, error::db};

pub async fn record(
    conn: &mut PgConnection,
    caller: &Caller,
    action: &str,
    target: &str,
    before: Option<Value>,
    after: Option<Value>,
) -> Result<(), Problem> {
    sqlx::query("INSERT INTO audit_log (actor, action, target, before, after, request_id) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(caller.subject())
        .bind(action)
        .bind(target)
        .bind(before)
        .bind(after)
        .bind(caller.request_id.as_deref())
        .execute(conn)
        .await
        .map_err(db)?;
    Ok(())
}
