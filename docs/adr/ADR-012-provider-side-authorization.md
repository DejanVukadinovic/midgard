# ADR-012: Providers and brokers authorize every invocation

* Status: accepted
* Date: 2026-10-23

## Decision
Core is control plane only. Resolution returns connection metadata; the provider verifies tokens itself and asks
Core `POST /v0/authorize` (with `interface`) on each invocation, and the broker enforces publish/consume ACLs.
For MQ, the caller identity is bound to the authenticated subject (`<destination>.<application>`), and Core
accepts an `application` slug in addition to `client_id` for that case. No data-plane proxy through Core.

## Tradeoffs
Each invocation costs a Core round-trip unless the provider caches decisions (the fixture does not; a cache TTL
widens the revocation window). Tenant binding for MQ is not implemented (application-scoped only). Broker ACLs are
static per deployment, so revoked applications can still publish; providers must refuse to process.

## Consequences
Revocation is effective at the next invocation/delivery. See [iam-enforcement.md](../iam-enforcement.md).
