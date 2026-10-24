use platform_auth::{ActorType, AuthContext, OrgSelectionError, tenancy::select_active_org};
use platform_http::Problem;
use platform_persistence::sqlx::{self, PgPool, Row};
use uuid::Uuid;

use crate::{authn::Caller, error::db, state::AppState};

/// The registered application a workload token belongs to (derived from the *verified*
/// `client_id`, never from request fields).
#[derive(Debug, Clone)]
pub struct WorkloadApp {
    pub id: Uuid,
    pub slug: String,
    pub project_id: Option<String>,
    pub status: String,
}

pub async fn workload_app(pool: &PgPool, caller: &Caller) -> Result<WorkloadApp, Problem> {
    let client_id = caller
        .token
        .client_id
        .as_deref()
        .ok_or_else(|| Problem::forbidden("token carries no client identity"))?;
    app_for_client(pool, client_id)
        .await?
        .ok_or_else(|| Problem::forbidden("caller is not a registered application"))
}

pub async fn app_for_client(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<WorkloadApp>, Problem> {
    let row = sqlx::query(
        "SELECT a.id, a.slug, a.zitadel_project_id, a.status
           FROM oauth_clients c JOIN applications a ON a.id = c.application_id
          WHERE c.zitadel_client_id = $1 AND c.status = 'active'",
    )
    .bind(client_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    Ok(row.map(|r| WorkloadApp {
        id: r.get("id"),
        slug: r.get("slug"),
        project_id: r.get("zitadel_project_id"),
        status: r.get("status"),
    }))
}

pub async fn app_for_slug(pool: &PgPool, slug: &str) -> Result<Option<WorkloadApp>, Problem> {
    let row = sqlx::query(
        "SELECT id, slug, zitadel_project_id, status FROM applications WHERE slug = $1",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    Ok(row.map(|r| WorkloadApp {
        id: r.get("id"),
        slug: r.get("slug"),
        project_id: r.get("zitadel_project_id"),
        status: r.get("status"),
    }))
}

pub fn require_active(app: &WorkloadApp) -> Result<(), Problem> {
    if app.status == "active" {
        Ok(())
    } else {
        Err(Problem::forbidden("application is not active"))
    }
}

pub async fn active_grant(
    pool: &PgPool,
    app_id: Uuid,
    contract: &str,
) -> Result<Option<Vec<String>>, Problem> {
    let r = sqlx::query("SELECT permissions FROM contract_grants WHERE application_id=$1 AND contract_name=$2 AND status='active'")
        .bind(app_id)
        .bind(contract)
        .fetch_optional(pool)
        .await
        .map_err(db)?;
    Ok(r.map(|r| r.get("permissions")))
}

/// Verify a *user-delegated* access token for `app` and derive the tenant context.
///
/// * the token must be a valid access token whose audience is the app's project or one of its clients
/// * its `client_id` must be one of the app's registered clients (an app cannot adopt tokens
///   minted for another product)
/// * the active organization comes only from verified role claims; `requested_org` is merely a
///   selection that must be among them
/// * the `(organization, application)` tenant must exist and be active
pub async fn resolve_user_context(
    st: &AppState,
    app: &WorkloadApp,
    user_token: &str,
    requested_org: Option<&str>,
) -> Result<AuthContext, Problem> {
    let project_id = app
        .project_id
        .as_deref()
        .ok_or_else(|| Problem::forbidden("application has no identity project"))?;
    let client_ids: Vec<String> = sqlx::query(
        "SELECT zitadel_client_id FROM oauth_clients WHERE application_id=$1 AND status='active'",
    )
    .bind(app.id)
    .fetch_all(&st.pool)
    .await
    .map_err(db)?
    .into_iter()
    .map(|r| r.get("zitadel_client_id"))
    .collect();
    let mut audiences = client_ids.clone();
    audiences.push(project_id.to_string());
    let tok = st
        .verifier
        .verify_for_audiences(user_token, &audiences)
        .await
        .map_err(|e| {
            tracing::info!(reason = %e, "user token rejected");
            Problem::forbidden("invalid user token")
        })?;
    if !tok
        .client_id
        .as_ref()
        .is_some_and(|c| client_ids.contains(c))
    {
        return Err(Problem::forbidden(
            "user token was not issued to this application",
        ));
    }
    let active =
        select_active_org(&tok.claims, project_id, requested_org).map_err(|e| match e {
            OrgSelectionError::NoAccess | OrgSelectionError::NotAuthorized => {
                Problem::forbidden(e.to_string())
            }
            OrgSelectionError::SelectionRequired => {
                Problem::unprocessable("organization_selection_required", e.to_string())
            }
        })?;
    let row = sqlx::query(
        "SELECT id, status FROM tenants WHERE zitadel_organization_id=$1 AND application_id=$2",
    )
    .bind(&active.organization_id)
    .bind(app.id)
    .fetch_optional(&st.pool)
    .await
    .map_err(db)?;
    let tenant_id = match row {
        Some(r) if r.get::<String, _>("status") == "active" => r.get::<Uuid, _>("id"),
        _ => {
            return Err(Problem::forbidden(
                "no active tenant for this organization and application",
            ));
        }
    };
    Ok(AuthContext {
        subject_id: tok.subject,
        application_id: app.id,
        organization_id: active.organization_id,
        tenant_id,
        roles: active.roles,
        actor_type: ActorType::UserDelegated,
    })
}

pub fn valid_contract_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && n.split('.').count() >= 2
        && n.split('.').all(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

pub fn valid_org_id(o: &str) -> bool {
    !o.is_empty()
        && o.len() <= 64
        && o.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
