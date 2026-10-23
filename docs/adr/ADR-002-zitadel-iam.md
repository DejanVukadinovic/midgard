# ADR-002: ZITADEL-backed IAM; one home organization per user

* Status: accepted (integration unverified)
* Date: 2026-10-23

## Context
We need OIDC/OAuth2, MFA, organizations, per-product projects, project grants and roles without writing an identity provider.

## Decision
ZITADEL owns credentials, users, home organizations, OAuth clients, projects, project grants and role assignments. A user has one home organization; additional access is an explicit project grant. MIDGARD owns the application↔project mapping, tenants, contract grants and bindings and never keeps a second editable copy of membership.
Core verifies JWT access tokens (JWKS) and derives authorization only from verified claims plus registry state; it fails closed.

## Alternatives considered
Keycloak/Ory (comparable, heavier or less aligned with the organization/project model); a custom IdP (rejected: security surface, no differentiation).

## Tradeoffs
Coupling to ZITADEL's claim shapes. Mitigated by isolating claim parsing in `platform-auth::tenancy` and testing it against a test IdP that emits the same shapes.

## Consequences
The `link` provisioner attaches pre-existing projects/clients; the `zitadel` provisioner creates them through the management API (idempotent search-then-create).

## Open uncertainties (must be settled by the Phase-0 spike — not assumed)
1. Exact claim shape for roles held in an *external* organization via project grant, and whether the pinned release emits it as assumed (`urn:zitadel:iam:org:project:{id}:roles` with org ids as keys).
2. Whether token exchange / delegation is supported without a custom issuer. Until proven, delegation is **explicit**: the facade passes the user's own access token, core verifies it, and nothing is inferred from a machine token or a caller-supplied header.
3. How machine users/clients map to operator, publisher and deployment identities (today: `sub` matching via configuration and the `deployment_identities` table).
4. ZITADEL's resource footprint and required extra configuration for the pinned version.
5. Behaviour of role revocation on already-issued JWTs (we document a revocation window equal to the access-token lifetime).
The checklist and evidence log live in [operations/zitadel.md](../operations/zitadel.md).

## Update 2026-10-23
Verified against ZITADEL v2.71.8: role-claim shape, audiences-as-project-ids, machine-user claims (`sub` = user id, `client_id` = user name) and the `resourceowner` scope requirement are recorded in [operations/zitadel.md](../operations/zitadel.md).
