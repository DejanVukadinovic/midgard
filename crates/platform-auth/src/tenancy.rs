//! Active-organization derivation from ZITADEL-style project role claims.
//!
//! ZITADEL emits role claims as
//! `"urn:zitadel:iam:org:project:{project_id}:roles": {"role": {"<org_id>": "<domain>"}}`.
//! The organizations listed under a role are the organizations in which the user
//! holds that role for the project (home org or via project grant). The shape is
//! pinned by the Phase-0 spike (see docs/adr/ADR-002); everything here fails closed.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const HOME_ORG_CLAIM: &str = "urn:zitadel:iam:user:resourceowner:id";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorType {
    UserDelegated,
    Workload,
}

/// Produced only by a trusted resolver from verified tokens and registry state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    pub subject_id: String,
    pub application_id: Uuid,
    pub organization_id: String,
    pub tenant_id: Uuid,
    pub roles: Vec<String>,
    pub actor_type: ActorType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveOrg {
    pub organization_id: String,
    pub roles: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OrgSelectionError {
    #[error("user holds no role for this project")]
    NoAccess,
    #[error("selected organization is not authorized for this user and project")]
    NotAuthorized,
    #[error("organization selection required")]
    SelectionRequired,
}

/// role -> set of org ids, for `project_id` (project-specific claim only).
pub fn project_roles(claims: &Value, project_id: &str) -> BTreeMap<String, BTreeSet<String>> {
    let key = format!("urn:zitadel:iam:org:project:{project_id}:roles");
    let mut out = BTreeMap::new();
    if let Some(Value::Object(roles)) = claims.get(&key) {
        for (role, orgs) in roles {
            if let Value::Object(orgs) = orgs {
                out.insert(role.clone(), orgs.keys().cloned().collect());
            }
        }
    }
    out
}

pub fn home_org(claims: &Value) -> Option<String> {
    claims
        .get(HOME_ORG_CLAIM)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Decide the active organization.
///
/// * a requested organization is a *selection* and must be among the verified authorized orgs
/// * without a selection the home org is used only if it is itself authorized for the project
/// * any other multi-org situation requires an explicit selection (fail closed)
pub fn select_active_org(
    claims: &Value,
    project_id: &str,
    requested: Option<&str>,
) -> Result<ActiveOrg, OrgSelectionError> {
    let roles = project_roles(claims, project_id);
    let authorized: BTreeSet<&String> = roles.values().flatten().collect();
    if authorized.is_empty() {
        return Err(OrgSelectionError::NoAccess);
    }
    let chosen: String = match requested {
        Some(org) => {
            if !authorized.iter().any(|o| o.as_str() == org) {
                return Err(OrgSelectionError::NotAuthorized);
            }
            org.to_string()
        }
        None => match home_org(claims) {
            Some(h) if authorized.iter().any(|o| o.as_str() == h) => h,
            _ => return Err(OrgSelectionError::SelectionRequired),
        },
    };
    let roles_in_org = roles
        .iter()
        .filter(|(_, orgs)| orgs.contains(&chosen))
        .map(|(r, _)| r.clone())
        .collect();
    Ok(ActiveOrg {
        organization_id: chosen,
        roles: roles_in_org,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claims() -> Value {
        json!({
            HOME_ORG_CLAIM: "orgA",
            "urn:zitadel:iam:org:project:p1:roles": {
                "member": {"orgA": "a.example"},
                "fleet_viewer": {"orgB": "b.example"}
            }
        })
    }

    #[test]
    fn home_org_default() {
        let o = select_active_org(&claims(), "p1", None).unwrap();
        assert_eq!(o.organization_id, "orgA");
        assert_eq!(o.roles, vec!["member"]);
    }

    #[test]
    fn explicit_extra_grant() {
        let o = select_active_org(&claims(), "p1", Some("orgB")).unwrap();
        assert_eq!(o.organization_id, "orgB");
        assert_eq!(o.roles, vec!["fleet_viewer"]);
    }

    #[test]
    fn spoofed_org_rejected() {
        assert_eq!(
            select_active_org(&claims(), "p1", Some("orgC")),
            Err(OrgSelectionError::NotAuthorized)
        );
    }

    #[test]
    fn no_roles_for_project() {
        assert_eq!(
            select_active_org(&claims(), "p2", None),
            Err(OrgSelectionError::NoAccess)
        );
        assert_eq!(
            select_active_org(&claims(), "p2", Some("orgA")),
            Err(OrgSelectionError::NoAccess)
        );
    }

    #[test]
    fn home_without_role_requires_selection() {
        let c = json!({HOME_ORG_CLAIM: "orgA", "urn:zitadel:iam:org:project:p1:roles": {"r": {"orgB": "b"}}});
        assert_eq!(
            select_active_org(&c, "p1", None),
            Err(OrgSelectionError::SelectionRequired)
        );
        assert!(select_active_org(&c, "p1", Some("orgB")).is_ok());
    }
}
