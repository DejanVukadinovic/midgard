# Authentication, authorization and tenancy

## Token verification (`platform-auth`)

Every service verifies bearer JWTs itself:

* signature with **RS256 or ES256 only** (no HMAC, no `none`), key by `kid` from the issuer's JWKS (cached 10 min, refreshed on unknown `kid`, refresh rate-limited);
* `iss` in an exact allowlist, `aud` must contain **this service's audience**, `exp`/`nbf` with 30 s leeway, `iat` required and
  `exp − iat` ≤ policy (bounds the revocation window);
* **ID tokens are rejected**: `typ` other than `at+jwt`, or `nonce`/`at_hash` present;
* if the IdP's keys are unreachable the request fails closed (`503 idp_unavailable` in core; `401` in services).

## Principals

| Principal | How it is recognized | May |
|---|---|---|
| Operator | token `sub` in `PLATFORM_OPERATOR_SUBJECTS` | everything administrative |
| Publisher | `sub` in `PLATFORM_PUBLISHER_SUBJECTS`, optionally restricted to namespace prefixes | publish contracts |
| Deployment identity | row in `deployment_identities` (`sub` + slug pattern), managed by operators | register/update only matching implementation slugs; call `/v0/authorize` |
| Application workload | token `client_id` ∈ `oauth_clients` of an **active** application | resolve contracts, resolve tenants |
| User (delegated) | user access token for one of the application's clients | only as input to tenant resolution |

Identity always comes from the verified token. `application_id`, `organization_id` and tenant ids in request bodies/headers are never trusted:
an organization is a *selection* that must be present in the user's verified role claims.

## Tenant resolution (`POST /v0/tenants/resolve`, `POST /v0/authorize`)

1. caller's workload token → active application;
2. user token verified (audience = the app's project id or one of its client ids); its `client_id` must be one of the application's clients (so a token minted for
   Fleet cannot be used for Obroklik);
3. authorized organizations = organizations listed in the user's `urn:zitadel:iam:org:project:{project_id}:roles` claim; none ⇒ `403`;
4. active organization = the requested one if authorized (else `403`); without a request, the user's home organization **only if** authorized for the project,
   otherwise `422 organization_selection_required`;
5. the `(organization, application)` tenant must exist and be `active` (suspended ⇒ `403` regardless of token validity);
6. response is an `AuthContext {subject_id, application_id, organization_id, tenant_id, roles, actor_type: UserDelegated}`.

A machine (client-credentials) token carries no project roles, so it can never obtain a user tenant context.

**Open point (see ADR-002):** the claim shape above is ZITADEL's documented shape; whether the pinned release emits roles for granted
organizations exactly this way, and how revocation propagates into already-issued JWTs, must be confirmed by the Phase-0 spike. Until then the
behaviour is verified against the test IdP only.

## Revocation

* grants, bindings, application/tenant suspension and deployment-identity revocation are read from PostgreSQL on every request ⇒ effective immediately
  (implementations cache a tenant-less decision for ≤ 15 s);
* IdP-side role/grant revocation reaches us only when a new token is issued: effective within the token lifetime (`PLATFORM_TOKEN_MAX_LIFETIME_SECONDS`, default ≤ 1 h; the dev IdP issues 5-minute tokens). The target of ≤ 5 minutes requires ≤ 5-minute access tokens in the IdP. Not yet measured against ZITADEL.

## Row-level security (`platform-persistence`)

Tenant-owned tables `ENABLE` + `FORCE ROW LEVEL SECURITY` with policy
`tenant_id = NULLIF(current_setting('app.tenant_id', true), '')::uuid` (both `USING` and `WITH CHECK`).
`begin_tenant_tx` runs `set_config('app.tenant_id', $1, true)` — transaction-local, so pooled connections cannot carry context between requests.
The runtime role is a non-owner with `NOBYPASSRLS`. Tests prove: no context ⇒ no rows; cross-tenant read/insert/update/delete blocked;
context does not leak across pooled connections (`services/platform-core/tests/tenancy.rs`, `services/example-capability/tests/service.rs`).

## Audit

Administrative changes append to `audit_log` (actor, action, target, before/after, request id) in the same transaction as the change.
A trigger rejects `UPDATE`/`DELETE`/`TRUNCATE` for every role. Normal resolutions are counted in metrics, not logged individually.
