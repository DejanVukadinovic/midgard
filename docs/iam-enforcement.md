# Authorization at resolution and at invocation (v0.2)

MIDGARD Core is the control plane. It decides **who may learn how to reach** a capability; the provider
(HTTP) and the broker (MQ) decide **who may invoke** it, on every invocation.

## Sequence

1. The caller authenticates to Core with a workload token (audience `midgard-core`).
2. `POST /v0/contracts/resolve`: application identity comes from the verified `client_id`; Core checks the
   declared requirement, the active grant (which must include the contract's permission), the explicit
   binding, and a healthy deployment, and returns only the interfaces the bound implementation advertises
   and a service may use. Uniform `403` for every denial (no existence leak).
3. HTTP: the caller sends its own token (audience = provider). The provider verifies signature, issuer,
   audience and expiry **itself** (no MIDGARD library), then calls `POST /v0/authorize` with its approved
   deployment identity: `{client_id, contract, interface, [user_token, organization_id]}`. Core re-checks
   the active grant, the binding to this provider, the interface's caller kinds and the permission, and for
   a delegated user derives the tenant from the verified user token and ZITADEL-mapped roles. Authorization
   happens before the payload is touched.
4. MQ: the broker's ACL decides who can publish (`example.echo.submit.<application>`) and consume. The
   provider derives the application from the authenticated subject and calls `/v0/authorize`
   `{application, contract, interface: "queue"}`. Unauthorized messages are dead-lettered, not processed.

## Service vs user

`callers: [service, user]` states which caller kinds an interface accepts. A user-delegated call carries a
user token **in addition to** the service token; a service identity never inherits a user's permissions, and a
user token alone is not accepted. An interface restricted to `service` (the MQ interface in the example) is
refused by `/v0/authorize` when a user token accompanies the request; the resolver returns an interface to
workload (service) callers only if its caller set includes `service`. Tenant is always taken from verified token claims plus the MIDGARD mapping — never from a header.

## Revocation

Revoking a grant or binding takes effect at the **next** `/v0/authorize` (HTTP: next request; MQ: next
message delivery) and the next resolution. Limits: a provider that caches authorization decisions extends the
window by its cache TTL (the fixture does not cache); access tokens remain valid until they expire; broker ACLs
are static per deployment, so a revoked application can still *publish* to its subject — the provider
refuses to process and dead-letters it. Broker-level revocation needs an ACL change/reload.

## Audit

Publication, implementation registration, grants, bindings and application changes are written to the
append-only `audit_log` without payloads or tokens (`GET /v0/audit`).
