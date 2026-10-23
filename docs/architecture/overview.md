# Architecture overview

MIDGARD v0 is an internal development accelerator: a small control plane that lets applications depend on
**contracts** while operators decide which **implementation** serves them.

## Vocabulary

* **Platform application** – a registered product (`obroklik`), associated with exactly one IdP project.
* **Contract** – stable, versioned interface (`platform.example`, `1.1.0`), immutable once published, content-addressed (`sha256`).
* **Implementation** – a registered provider of one or more contract versions (`example-capability`), identified by slug.
* **Deployment** – a logical endpoint of an implementation in an environment (`http://example-capability:8080`).
* **Grant** – authorization policy: an application may use a contract with specific permissions.
* **Binding** – routing policy: an application's contract *major* in an environment resolves to one implementation.
* **Tenant** – `(IdP organization, application)` → internal UUID; business tables carry only `tenant_id`.

A binding never bypasses a missing grant, and a grant never selects an implementation.

## Components

| Component | Responsibility |
|---|---|
| `platform-core` (Rust/Axum, one binary) | registry modules: applications, contracts, implementations, grants/bindings, tenants, resolver, audit, readiness prober |
| PostgreSQL | registry state; row-level security for tenant-owned data; append-only audit |
| Identity provider | authentication, credentials, organizations, projects, roles (ZITADEL; offline dev IdP for the demo) |
| Implementation (e.g. `example-capability`) | data plane; verifies tokens itself; asks core for authorization/tenant context |
| Go SDK | client-side resolution with a private, bounded routing cache; generated contract clients |

Core never deploys, restarts or scales anything; it stores logical endpoints and probes `/health/ready`.
Docker DNS / reverse proxy (later: Kubernetes Service) balances replicas of *one* implementation. Registry choice between
*different* implementations is explicit (bindings) because they may hold different state; there is **no automatic failover** between them.

## Control plane vs data plane

1. Facade → core: `POST /v0/contracts/resolve` with its **workload token** (audience `midgard-core`).
2. Core: identity from the verified token's `client_id` → active application → declared requirement → active grant → binding → implementation
   declares a compatible published version → deployment `healthy` → `{endpoint, resolved_version, permissions, ttl}`.
3. Facade → implementation: direct call with a token whose **audience is the implementation**. The implementation verifies it and
   calls core's `POST /v0/authorize` (as its approved deployment identity) to confirm grant + binding and, for user data, resolve the tenant.

A resolver outage does not interrupt calls already routed; the SDK may reuse an expired *routing* answer for at most `MaxStale` (60 s)
and only on registry unavailability — never after a denial.

## Lifecycles

Application: `pending → active ⇄ suspended` (a failed IdP provisioning keeps the application `pending` with a `failed` operation; retry with the
same `Idempotency-Key`). Deployment: `registered → healthy ⇄ degraded → unavailable`, or `disabled`. Only `active` implementations with a
`healthy` deployment resolve. Deleting a bound deployment, or dropping a bound contract major from an implementation, is `409`.
Contract versions are never deleted.

## Persistence

Migrations are SQL files in `migrations/`, applied by a separate one-shot job (`platform-core migrate`) with the owner role. The runtime connects as
a non-owner role without `BYPASSRLS`. See [auth-and-tenancy](auth-and-tenancy.md) for RLS, and
[operations](../operations/configuration.md) for configuration.
