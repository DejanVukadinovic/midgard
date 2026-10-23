mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

struct World {
    env: Env,
    obroklik: String,
    fleet: String,
    t_a_obr: String,
    t_a_fleet: String,
    t_b_fleet: String,
}

async fn world() -> World {
    let env = Env::new().await;
    let (_, a) = env
        .register_app(
            "obroklik",
            "p-obroklik",
            "obroklik-web",
            "obroklik-be",
            "ka",
        )
        .await;
    let (_, f) = env
        .register_app("fleet", "p-fleet", "fleet-web", "fleet-be", "kf")
        .await;
    let obroklik = a["application"]["id"].as_str().unwrap().to_string();
    let fleet = f["application"]["id"].as_str().unwrap().to_string();
    let mk = |org: &str, app: &str| format!("/v0/organizations/{org}/applications/{app}/tenants");
    let (s, t1) = env.op("POST", &mk("orgA", &obroklik), None).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, t2) = env.op("POST", &mk("orgA", &fleet), None).await;
    let (_, t3) = env.op("POST", &mk("orgB", &fleet), None).await;
    World {
        t_a_obr: t1["id"].as_str().unwrap().into(),
        t_a_fleet: t2["id"].as_str().unwrap().into(),
        t_b_fleet: t3["id"].as_str().unwrap().into(),
        env,
        obroklik,
        fleet,
    }
}

impl World {
    async fn resolve(
        &self,
        workload: &str,
        user_token: &str,
        org: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut b = json!({"user_token": user_token});
        if let Some(o) = org {
            b["organization_id"] = json!(o);
        }
        self.env
            .call(
                "POST",
                "/v0/tenants/resolve",
                Some(&self.env.token(workload)),
                Some(b),
                &[],
            )
            .await
    }
}

// IAM-02, IAM-03, IAM-06
#[tokio::test]
async fn one_user_two_products_distinct_tenants() {
    let w = world().await;
    let alice_obr = w.env.user_token("alice", "obroklik-web", &["p-obroklik"]);
    let (s, c) = w.resolve("obroklik-be", &alice_obr, None).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c["tenant_id"], w.t_a_obr);
    assert_eq!(c["organization_id"], "orgA");
    assert_eq!(c["subject_id"], "alice");
    assert_eq!(c["roles"], json!(["member"]));
    assert_eq!(c["actor_type"], "UserDelegated");

    let alice_fleet = w.env.user_token("alice", "fleet-web", &["p-fleet"]);
    let (s, c) = w.resolve("fleet-be", &alice_fleet, None).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c["tenant_id"], w.t_a_fleet);
    assert_ne!(
        w.t_a_fleet, w.t_a_obr,
        "distinct tenants per product for the same organization"
    );

    // IAM-06: explicit extra-organization selection, authorized by verified claims
    let (s, c) = w.resolve("fleet-be", &alice_fleet, Some("orgB")).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c["tenant_id"], w.t_b_fleet);

    // tenants are idempotent
    let (s, again) = w
        .env
        .op(
            "POST",
            &format!("/v0/organizations/orgA/applications/{}/tenants", w.fleet),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["id"], w.t_a_fleet);
    drop_db(w.env).await;
}

// IAM-04, IAM-05, IAM-09, IAM-10, IAM-11
#[tokio::test]
async fn spoofing_and_unauthorized_access_is_denied() {
    let w = world().await;
    let alice_obr = w.env.user_token("alice", "obroklik-web", &["p-obroklik"]);
    let alice_fleet = w.env.user_token("alice", "fleet-web", &["p-fleet"]);

    // IAM-04: org has the project but the user holds no role
    let carol = w.env.user_token("carol", "fleet-web", &["p-fleet"]);
    let (s, _) = w.resolve("fleet-be", &carol, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // IAM-05: caller-supplied organization is only a selection and is verified
    let (s, _) = w.resolve("obroklik-be", &alice_obr, Some("orgB")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = w.resolve("obroklik-be", &alice_obr, Some("orgZ")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // spoofed tenant header is ignored: result is derived from the token
    let (s, c) = w
        .env
        .call(
            "POST",
            "/v0/tenants/resolve",
            Some(&w.env.token("obroklik-be")),
            Some(json!({"user_token": alice_obr})),
            &[("x-tenant-id", &w.t_b_fleet), ("x-organization-id", "orgB")],
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c["tenant_id"], w.t_a_obr);

    // a user whose only Fleet grant is in orgB must select it explicitly
    let bob = w.env.user_token("bob", "fleet-web", &["p-fleet"]);
    let (s, v) = w.resolve("fleet-be", &bob, None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(v["code"], "organization_selection_required");
    let (s, c) = w.resolve("fleet-be", &bob, Some("orgB")).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c["tenant_id"], w.t_b_fleet);
    let (s, _) = w.resolve("fleet-be", &bob, Some("orgA")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // IAM-09: a machine token is not user authorization
    let machine = w.env.token("obroklik-be");
    let (s, _) = w.resolve("obroklik-be", &machine, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // ... and no workload token at all -> 401
    let (s, _) = w
        .env
        .call(
            "POST",
            "/v0/tenants/resolve",
            None,
            Some(json!({"user_token": alice_obr})),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // IAM-10: app A cannot adopt a user token minted for app B, nor impersonate via fields
    let (s, _) = w.resolve("obroklik-be", &alice_fleet, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = w.resolve("fleet-be", &alice_obr, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // a stranger client has no application at all
    let (s, _) = w.resolve("stranger-client", &alice_obr, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // IAM-11: suspended tenant is denied regardless of valid tokens
    let (s, _) = w
        .env
        .op("POST", &format!("/v0/tenants/{}/suspend", w.t_a_obr), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = w.resolve("obroklik-be", &alice_obr, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = w
        .env
        .op("POST", &format!("/v0/tenants/{}/activate", w.t_a_obr), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = w.resolve("obroklik-be", &alice_obr, None).await;
    assert_eq!(s, StatusCode::OK);

    // tenants cannot be created for unknown/ suspended apps or malformed orgs
    let (s, _) = w
        .env
        .op(
            "POST",
            &format!(
                "/v0/organizations/o%27%3B--/applications/{}/tenants",
                w.obroklik
            ),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    drop_db(w.env).await;
}

// IAM-14, IAM-15: PostgreSQL RLS with the non-owner runtime role
#[tokio::test]
async fn rls_isolates_tenants_and_pooled_connections_do_not_leak() {
    let env = Env::new().await;
    // a single-connection pool makes any context leak between "requests" deterministic
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&env.cfg.database_url)
        .await
        .unwrap();
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());

    for (t, body) in [(a, "a-secret"), (b, "b-secret")] {
        let mut tx = platform_persistence::begin_tenant_tx(&pool, t)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tenant_fixture_notes (tenant_id, body) VALUES ($1,$2)")
            .bind(t)
            .bind(body)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    // no context on a reused connection -> nothing visible
    let n: i64 = sqlx::query("SELECT count(*) AS n FROM tenant_fixture_notes")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(n, 0, "no tenant context must see no rows");

    // tenant A sees only A
    let mut tx = platform_persistence::begin_tenant_tx(&pool, a)
        .await
        .unwrap();
    let rows = sqlx::query("SELECT body FROM tenant_fixture_notes")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>("body"), "a-secret");
    // cannot read B explicitly
    let n: i64 = sqlx::query("SELECT count(*) AS n FROM tenant_fixture_notes WHERE tenant_id=$1")
        .bind(b)
        .fetch_one(&mut *tx)
        .await
        .unwrap()
        .get("n");
    assert_eq!(n, 0);
    // cannot write rows for B nor update/delete B's rows
    assert!(
        sqlx::query("INSERT INTO tenant_fixture_notes (tenant_id, body) VALUES ($1,'evil')")
            .bind(b)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = platform_persistence::begin_tenant_tx(&pool, a)
        .await
        .unwrap();
    let upd = sqlx::query("UPDATE tenant_fixture_notes SET body='pwned' WHERE tenant_id=$1")
        .bind(b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(upd.rows_affected(), 0);
    let del = sqlx::query("DELETE FROM tenant_fixture_notes WHERE tenant_id=$1")
        .bind(b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(del.rows_affected(), 0);
    tx.commit().await.unwrap();

    // after A's transaction, the same pooled connection holds no context again
    let n: i64 = sqlx::query("SELECT count(*) AS n FROM tenant_fixture_notes")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(n, 0);

    // the runtime role neither owns the table nor bypasses RLS
    let r = sqlx::query("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!r.get::<bool, _>("rolsuper") && !r.get::<bool, _>("rolbypassrls"));
    pool.close().await;
    drop_db(env).await;
}
