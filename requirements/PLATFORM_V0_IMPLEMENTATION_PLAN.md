# Platform v0 — Implementation Plan

**Status:** Architecture baseline / implementation-ready plan  
**Deployment target:** Docker Compose, single environment initially  
**Core language:** Rust (Tokio, Axum, SQLx, Serde)  
**Identity provider:** ZITADEL  
**Database:** PostgreSQL  
**First applications:** Obroklik and Fleet Management  
**Goal:** Secure, dynamically registered applications and contract-based service implementations, with tenant-aware authorization and stable routing, without implementing business capabilities yet.

---

## 1. Executive summary

Platform v0 is an **internal development accelerator**, not a public developer platform or marketplace. All applications and services are built, deployed, and operated by us. v0 delivers:

1. **IAM integration** with ZITADEL for identities, organizations, OAuth/OIDC clients, product projects, project grants, and roles.
2. **Dynamic application registration** with ZITADEL provisioning and safe, repeatable updates.
3. **Versioned contract registry** independent of service implementations and deployments.
4. **Dynamic service/implementation registration** and health-aware logical endpoint publication.
5. **Application-to-contract grants and bindings**, including a deterministic implementation selection policy.
6. **Contract resolution** for authenticated application workloads, without exposing unnecessary implementation details.
7. **Tenant mapping and authorization context** using `(ZITADEL organization, application/project)` → internal `tenant_id`.
8. **Shared Rust middleware/libraries** for token verification, tenant context, persistence, HTTP errors, telemetry, and future capability adoption.
9. **Generated Go client/SDK** for application facades to request contract resolutions and invoke contract-defined APIs.
10. **Automated acceptance tests** covering provisioning, isolation, authorization, contracts, dynamic registration, lifecycle, and restart recovery.

The platform core starts as **one modular Rust executable/container** with separate IAM, application registry, contract registry, service registry, and resolver modules. ZITADEL and PostgreSQL are separate containers. Docker Compose runs the environment. v0 does **not** deploy containers or implement a service mesh.

### Explicit boundaries

- **Contracts** are the stable dependency visible to applications.
- **Implementations** are services that satisfy one or more contracts.
- **Deployments** are running instances behind stable logical endpoints.
- **Registry** chooses an authorized implementation for a contract; infrastructure balances requests across replicas of that same implementation.
- **ZITADEL** owns authentication, credentials, home organizations, project grants, and identity-level role assignments.
- **Platform core** owns contract definitions, service implementations, application requirements, grants/bindings, and internal tenant mapping.
- **Business capabilities** (Ordering, Catalog, Notifications, Files, etc.) are **not implemented in v0**. A minimal example/echo contract implementation is used for verification.

---

## 2. Decisions already made

| Area | Decision | Rationale |
|---|---|---|
| Scope | Internal platform only | No third-party onboarding, public marketplace, developer portal, or metered platform billing |
| Stack | Rust for reusable platform core/capabilities; Go for app facades; TypeScript frontends | Shared performant core, pragmatic app-specific logic |
| Repositories | Rust platform Cargo workspace monorepo; applications in separate repositories | Shared crates and independent application lifecycles |
| Deployment | Docker Compose first; Kubernetes optional later | Avoid premature orchestration complexity |
| IAM | ZITADEL instead of custom identity provider | OIDC/OAuth2, MFA, credentials, organizations, projects, role assignments |
| Customer identity | One primary/home organization per user | Simplifies common-case user experience |
| Additional access | Cross-organization/project grants possible | No duplicate user accounts; preserve future expansion |
| Customer organization | ZITADEL organization | Identity ownership and customer-level administration |
| Product | ZITADEL project | Obroklik, Fleet, etc.; projects are not per customer |
| OAuth client | ZITADEL application | Web, mobile, and confidential backend clients per product |
| Customer product access | ZITADEL project grant + user authorization | Organization-level availability does not imply all users are authorized |
| Internal tenant | Unique `(organization_id, application_id)` mapping to UUID | Business tables need only `tenant_id`; app-specific isolation |
| Data model | Shared tenant tables within each capability DB, PostgreSQL RLS defense in depth | Simpler than DB per tenant; enforce isolation |
| Registry | Dynamic application and service registration | No core redeploy to add an approved application/implementation |
| App dependency | Versioned contract, not concrete service | Implementation/deployment independence |
| Implementation topology | One service can implement multiple contracts; multiple implementations can satisfy one contract | Supports later splits/consolidation and substitutions |
| Binding | Explicit application-to-implementation binding for each required contract | Avoid accidental data/behavior switching |
| Routing | Client-side contract resolution in Go SDK initially | Avoid extra gateway hop |
| Endpoint | Stable logical implementation endpoint, never a pod/container IP | Docker/Kubernetes independence |
| Load balancing | Docker DNS/reverse proxy initially; Kubernetes Service later | Platform registry is not a load balancer |
| Registration security | Operator/deployment-controlled identities | Dynamic does not mean anonymous/untrusted registration |
| Contracts | Independently versioned OpenAPI + event schemas + semantics | Structural and behavioral compatibility |
| Conformance | CI-gated contract tests initially | No arbitrary test execution inside registry |
| Messaging later | NATS JetStream + transactional outbox for future capabilities | Durable integration/event infrastructure, not mandatory to run v0 |
| Persistence later | PostgreSQL authoritative, SQLx, explicit repositories | No universal CRUD handler or event sourcing in v0 |

### Open decisions to settle during the first spike

These are **not silently assumed to be supported by ZITADEL**:

1. Exact claim shape and validation needed for **user authorization within a granted project in an external organization**.
2. Whether the selected ZITADEL version supports the desired **token exchange / delegated user flow** without a custom token issuer.
3. How ZITADEL represents and authorizes **machine clients and service registration identities**.
4. Whether ZITADEL's chosen deployment requires additional persistence/configuration dependencies and what resource footprint it has.
5. Exact behavior of role revocation in already-issued JWTs and acceptable revocation window.

The plan specifies safe fallback behavior: **fail closed** if an asserted organization/project cannot be proven authorized; do not infer authorization from a home-org claim or caller-supplied header.

---

## 3. Terminology and identity mapping

Avoid overloading “application”:

- **Platform application:** An internally registered customer-facing product (`obroklik`, `fleet`), identified by platform `application_id`, associated with **one ZITADEL project**.
- **ZITADEL application:** An OAuth/OIDC client within a ZITADEL project (browser, mobile, Go backend).
- **Customer organization:** ZITADEL organization; user's primary employment/home organization.
- **Tenant:** A customer organization's workspace in a specific platform application, keyed by `(organization_id, application_id)`.
- **Contract:** Stable, versioned capability interface (`commerce.orders`, major `v1`).
- **Implementation:** A registered provider of one or more contracts (`standard-ordering`).
- **Deployment:** An active running revision/instance group of an implementation, with a logical network endpoint.
- **Binding:** The implementation selected for an application/contract/major version (optionally extendable to tenant-specific overrides later).
- **Grant:** Permission for an application identity to consume a contract's operations; separate from a human user's roles.

### Example

```text
ZITADEL Instance
├── Our Company (provider organization)
│   ├── Project: Obroklik
│   │   ├── OAuth client: web
│   │   ├── OAuth client: mobile
│   │   └── OAuth client: backend
│   └── Project: Fleet
│       ├── OAuth client: web
│       └── OAuth client: backend
├── Company A (customer org; home users John, Alice)
│   ├── Granted project: Obroklik
│   └── Granted project: Fleet
└── Company B (customer org; home user Mark)
    └── Granted project: Fleet

Platform tenants:
  Company A + Obroklik -> tenant UUID A1
  Company A + Fleet    -> tenant UUID A2
  Company B + Fleet    -> tenant UUID B1
```

**Crucial:** The ZITADEL user's home organization is not always the *active authorized organization*. Default to home organization only after confirming authorization for the current project. For a user with extra grants, require explicit selection and verify the selected target organization.

---

## 4. Logical architecture

```text
                Browser / Mobile
                       |
                OIDC / PKCE
                       v
                   ZITADEL
                       |
                user identity
                       v
            Go application facade
              |               |
        authenticated       SDK resolves
        app workload        required contract
              |               |
              +-------+-------+
                      v
               PLATFORM CORE
             (Rust Axum API)
      +---------------+-----------------+
      | IAM/tenancy   | App registry    |
      | Contracts     | Implementations |
      | Grants        | Bindings        |
      | Resolver      | Audit           |
      +---------------+-----------------+
                      |
                  PostgreSQL
                      |
        resolve authorized stable endpoint
                      v
          Implementation logical endpoint
           (Docker service/proxy today)
                      |
             Example capability service
```

The registry is on the **control plane** (provision, publish, resolve). Normal contract calls flow directly from the Go facade to the selected implementation (**data plane**). Every implementation authenticates and authorizes incoming calls itself; possession of a resolved URL is not authorization.

### Runtime deployment principle

- Core never deploys, restarts, or scales containers.
- Deployment automation registers a **logical endpoint** (e.g. `http://example-capability:8080` on the private Compose network).
- Core may probe readiness of that endpoint; it does not track individual replicas.
- Docker networking/reverse proxy handles instance routing; Kubernetes Service can replace this later.
- A registry failure should not interrupt already-resolved healthy data-plane calls until clients need to refresh discovery; the SDK uses bounded stale-if-error behavior only for **routing**, never for expired authorization.

---

## 5. Repository layout

```text
platform/
├── Cargo.toml                         # workspace
├── Cargo.lock
├── crates/
│   ├── platform-auth/                 # JWT/JWKS, claims, auth context
│   ├── platform-persistence/          # SQLx pool, tx, RLS helpers, migrations
│   ├── platform-http/                 # RFC 9457 errors, pagination, tracing
│   ├── platform-contracts/            # manifest/schema parsing, semver checks
│   └── platform-observability/        # tracing, metrics conventions
├── services/
│   ├── platform-core/                 # one Axum binary, modular internals
│   └── example-capability/           # contract conformance test service
├── contracts/
│   ├── platform/
│   │   └── example/v1/
│   │       ├── openapi.yaml
│   │       ├── semantics.md
│   │       ├── compatibility.yaml
│   │       └── tests/
│   └── registry/v0/openapi.yaml
├── sdk/
│   └── go/                            # generated clients + thin resolver cache
├── migrations/
├── deploy/
│   └── compose/
│       ├── compose.yaml
│       ├── compose.dev.yaml
│       ├── .env.example
│       └── init/
├── scripts/
│   ├── bootstrap.sh
│   ├── register-app.sh
│   ├── register-contract.sh
│   └── register-implementation.sh
├── tests/
│   ├── integration/
│   ├── acceptance/
│   ├── security/
│   └── fixtures/
├── docs/
│   ├── architecture/
│   ├── adr/
│   ├── operations/
│   └── api/
└── .github/workflows/
```

Application repositories (`obroklik`, `fleet`) remain separate and import generated Go SDKs or generated contract clients. Business logic stays in application facades; generic reusable capabilities go into platform services when justified.

---

## 6. Docker Compose deployment

### 6.1 Containers

**Required for v0:**

- `postgres`: persistent PostgreSQL instance; separate logical databases/schemas for ZITADEL and platform core as required by ZITADEL's supported configuration.
- `zitadel`: pinned release, OIDC discovery, admin/management APIs, persistent configuration; run the **officially supported init/start/migration sequence** for the chosen version.
- `platform-core`: Rust API, migrations, background health checks, JWKS cache.
- `example-capability`: lightweight test provider of `platform.example.v1`.
- `edge` (Traefik or another reverse proxy): HTTPS termination in deployed environments, local routing for auth and platform APIs.

**Optional development/test-only:** test runner, mock SMTP if ZITADEL email flows are exercised, Prometheus/Grafana, Postgres admin UI.

**Not required for v0:** NATS JetStream, MinIO/S3, Redis, Kubernetes, full telemetry pipeline. Reserve interfaces for later services, but do not impose operational dependencies before needed.

### 6.2 Network layout

```text
public / localhost
        |
   edge reverse proxy
        |
   +----+-------------+
   |                  |
 ZITADEL          platform-core
                       |
                  private network
                   |         |
                postgres  example-capability
```

Only `edge` exposes application-facing ports. Postgres and capability endpoints stay private. In local development, expose explicit loopback-only ports if necessary for tooling; production should use TLS and restrictive firewall rules.

### 6.3 Compose implementation requirements

- Pin container images by version/digest; never use `latest` in production.
- Use named volumes for Postgres and ZITADEL-required state.
- Define health checks and `depends_on: condition: service_healthy` where applicable; application-level retry remains mandatory.
- Use a `platform` internal Docker network; DNS names are stable endpoints.
- Mount credentials as secrets or read-only secret files where supported; `.env` for nonsecret configuration only.
- Keep operator bootstrap credentials outside Git; rotate after first initialization.
- Add restart policies, bounded resource limits where supported, graceful shutdown, and structured JSON logs.
- Provide `compose.dev.yaml` for local convenience; never silently weaken production authentication.
- Establish database backup/restore procedures and migration ordering.

### 6.4 Example configuration keys

```dotenv
PLATFORM_BIND_ADDR=0.0.0.0:8080
PLATFORM_PUBLIC_URL=https://platform.local.test
ZITADEL_ISSUER=https://auth.local.test
ZITADEL_JWKS_URL=<derived-from-oidc-discovery>
PLATFORM_DATABASE_URL=<secret-file-or-injected-value>
REGISTRY_HEALTH_CHECK_INTERVAL_SECONDS=30
REGISTRY_RESOLUTION_TTL_SECONDS=300
REGISTRY_STALE_ROUTING_MAX_SECONDS=60
```

These are illustrative; final variable names should be stable and documented. Do not handcraft a guessed ZITADEL Compose configuration: pin a tested version and adapt its official Docker Compose deployment, including supported initialization and TLS/issuer requirements.

### 6.5 Startup/bootstrap order

1. Start PostgreSQL and ensure readiness.
2. Run ZITADEL's supported database/init/start flow; verify OIDC discovery and JWKS.
3. Bootstrap provider organization and a least-privilege platform provisioning machine identity.
4. Run platform core migrations and start core.
5. Publish `platform.example.v1` contract and conformance fixture.
6. Start example capability and register its implementation through an authenticated deployment identity.
7. Register Obroklik and Fleet using the platform admin API; create/associate ZITADEL projects and clients.
8. Create test customer organizations and grants; provision tenant mappings.
9. Bind applications to approved contracts/implementations; run end-to-end tests.

Bootstrap must be **idempotent**: re-running it reconciles existing records rather than creating duplicates.

---

## 7. IAM and ZITADEL integration

### 7.1 Source-of-truth boundaries

**ZITADEL owns:** user credentials, user IDs, home organizations, authentication sessions, MFA, OAuth clients, product projects, project grants, project roles, role assignments.

**Platform core owns:** platform application record ↔ ZITADEL project mapping, tenant UUID mapping, contract grants, implementation bindings, service/contract registry, audit records for platform actions.

Never maintain a second independently editable copy of ZITADEL membership or password data. Cached identity/authorization metadata must be derived, time-bounded, and invalidatable.

### 7.2 Identity flows

- Browser/mobile: OIDC authorization code + PKCE; no client secret in public clients.
- Go backend: confidential client authentication, preferably private-key JWT if supported/configured, otherwise securely managed client credentials.
- Core and capabilities: verify access-token **signature, issuer, audience, expiry/not-before, intended client/workload, scopes and required claims**. Reject ID tokens presented as API access tokens.
- JWKS discovery and rotation: cache keys with refresh-on-unknown-key and strict issuer allowlist.
- Token exchange/delegation: **spike and verify against the pinned ZITADEL release**. A machine token alone is insufficient to prove the human's tenant membership. Until delegation is proven, require explicit server-side membership validation for user-initiated tenant operations; never trust arbitrary `X-Tenant-ID` or `organization_id` from the facade.
- Background jobs: separate service-principal permissions from user-delegated permissions, with auditable acting context.

### 7.3 Organization and user model

- Every human user has one **home ZITADEL organization**.
- Home org is the default organization shown in product UI, subject to project authorization.
- Additional project/organization role grants can enable access to other organizations later.
- Each platform application is mapped to exactly one ZITADEL project; each product has its own roles.
- A customer organization may be granted several products; one product may be granted to many organizations.
- A project grant does **not** imply every user of the organization can perform every product action.
- A user with several authorized target organizations must select the active organization; platform verifies the selection.

### 7.4 Tenant model

```sql
CREATE TABLE tenants (
    id UUID PRIMARY KEY,
    zitadel_organization_id TEXT NOT NULL,
    application_id UUID NOT NULL REFERENCES applications(id),
    status TEXT NOT NULL CHECK (status IN ('active','suspended')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (zitadel_organization_id, application_id)
);
```

Business capability data uses `tenant_id UUID` only; tenant uniquely implies application. In future capability databases, add tenant-scoped composite uniqueness/foreign keys and transaction-scoped PostgreSQL RLS context. **Do not use one database connection role that bypasses RLS for ordinary application requests.**

### 7.5 Authorization context

```rust
pub struct AuthContext {
    pub subject_id: String,       // human or workload principal
    pub application_id: Uuid,     // verified from registered OAuth client
    pub organization_id: String, // verified active customer org
    pub tenant_id: Uuid,
    pub roles: Vec<String>,
    pub actor_type: ActorType,    // UserDelegated | Workload
}
```

A tenant context must be produced by a **trusted resolver** from validated token claims and verified grants, never directly deserialized from caller-provided values. Context for platform administration may legitimately be application-independent; do not fabricate a tenant for administrative endpoints.

### 7.6 Provisioning lifecycle

1. Authorized operator calls application registration.
2. Core creates a pending registration record and requests ZITADEL project/client provisioning.
3. Persist external IDs and mark active after verification.
4. If ZITADEL call fails, preserve pending/failed status with safe retry; do not leave an apparently active partial registration.
5. For a new customer, create/identify ZITADEL organization, grant product project, assign authorized users/roles, and create tenant mapping.
6. Disabling a product tenant blocks future tenant resolution and capability access, even if old ZITADEL credentials still exist.

Use an **idempotent reconciliation workflow**, not an unsafe distributed SQL/ZITADEL transaction. Record desired state and retryable steps; compensate only when safe and unambiguous.

### 7.7 Authorization freshness

JWTs may remain valid after a grant is revoked. Document an explicit revocation target (initial proposal: **≤ 5 minutes** for ordinary application grants, faster for platform-admin/service-registration credentials). Use short-lived access tokens plus a bounded grant cache or version/revocation check where needed. Test real ZITADEL behavior before declaring a hard guarantee.

---

## 8. Contract model

### 8.1 Contract identity

- Stable identifier: `namespace.name` (e.g. `commerce.orders`).
- Semantic version: `MAJOR.MINOR.PATCH`.
- Application requirements bind to a **major** and optional minimum compatible minor, e.g. `commerce.orders` `>=1.2.0 <2.0.0`.
- Service release versions are **independent** of contract versions.
- Contract definitions are immutable once published. A changed specification produces a new version and digest.
- Contract identity is independent of hostnames, service names, or deployment topology.

### 8.2 Contract package

```text
contracts/commerce/orders/v1/
├── manifest.yaml
├── openapi.yaml
├── events/
│   ├── order-created.schema.json
│   └── order-confirmed.schema.json
├── semantics.md
├── compatibility.yaml
└── tests/
    └── conformance.*
```

`manifest.yaml` example:

```yaml
id: commerce.orders
version: 1.0.0
protocol: http
specification: openapi-3.1
openapi: ./openapi.yaml
semantics: ./semantics.md
permissions:
  - orders:read
  - orders:write
compatibility:
  policy: backward-compatible-within-major
```

A contract defines operation semantics, error codes, idempotency, pagination, auth scopes, tenant behavior, and events if applicable. OpenAPI alone is insufficient to describe behavior.

### 8.3 Compatibility policy

- **Patch:** corrections with no behavior or wire break.
- **Minor:** strictly backward-compatible additions; avoid changing required request fields or removing response fields.
- **Major:** breaking API or semantic changes.
- CI checks OpenAPI compatibility and JSON schema compatibility; human review for semantic compatibility.
- Existing published versions are content-addressed (`sha256`) and cannot be silently overwritten.
- One implementation may expose v1 and v2 concurrently.

### 8.4 Contracts versus implementations

A service may implement several contracts; a contract may have multiple implementations. A single implementation may support multiple compatible versions. **Implementation binding is explicit and deterministic**; the resolver does not randomly switch implementations based on health if they own different state.

Example:

```text
commerce.orders.v1
  ├── standard-ordering (bound to Obroklik)
  └── alternate-ordering (bound to another product)
```

A migration between stateful implementations requires an explicit data migration/cutover plan. API compatibility alone does not make databases interchangeable.

### 8.5 Contract conformance

- CI runs reusable contract tests against the implementation build.
- Verify required paths, request/response schemas, standard errors, auth requirements, idempotency, and tenant isolation for tenant-bearing contracts.
- Deployment registration references the tested contract digest and CI attestation/artifact where available.
- v0 registry verifies published manifest/digest and authorized registration; it does **not** execute untrusted test code.
- A “claims compatibility” flag alone is not proof; production rollout policy should require CI evidence.

---

## 9. Registry domain and persistence model

Suggested minimum tables (names illustrative):

| Table | Important columns | Constraints / notes |
|---|---|---|
| `applications` | `id`, `slug`, `display_name`, `zitadel_project_id`, `status`, timestamps | unique slug; unique project ID |
| `oauth_clients` | `id`, `application_id`, `zitadel_client_id`, `kind`, `status` | unique client ID; no secrets stored in plaintext |
| `tenants` | `id`, `zitadel_organization_id`, `application_id`, `status` | unique `(org, app)` |
| `contracts` | `id`, `namespace_name`, `version`, `digest`, `manifest`, `status` | unique `(name, version)`; immutable published payload |
| `implementations` | `id`, `slug`, `release_version`, `status`, `owner_identity` | stable identity distinct from deployment |
| `implementation_contracts` | `implementation_id`, `contract_id`, `contract_digest`, `verification_status` | many-to-many |
| `deployments` | `id`, `implementation_id`, `environment`, `logical_endpoint`, `status`, `last_health_at` | one or more per implementation/environment |
| `application_requirements` | `application_id`, `contract_name`, `version_constraint` | declared dependency |
| `contract_grants` | `application_id`, `contract_name`, `permissions`, `status` | permissions subset of published contract |
| `contract_bindings` | `application_id`, `contract_name`, `major_version`, `implementation_id`, `environment` | unique per app/contract/major/environment |
| `provisioning_operations` | `id`, `idempotency_key`, `kind`, `desired_state`, `step`, `status`, `error` | durable reconciliation |
| `audit_log` | `id`, `actor`, `action`, `target`, `before`, `after`, `timestamp`, `request_id` | immutable append-only audit |

Use foreign keys and `ON DELETE RESTRICT` for referenced published contracts/bindings. Prefer **soft disable/suspend** over destructive deletion. Avoid storing secrets in registry responses, audit records, or structured logs.

### Migration strategy

- SQLx migrations in source control.
- Expand/contract schema changes; backwards-compatible deployment order.
- Transactional migrations where possible.
- Run migrations as a separate one-shot Compose job in production or through an explicit migration command; do not let several replicas race migrations.

---

## 10. Registration and lifecycle

### 10.1 Dynamic application registration

```http
POST /v0/applications
Authorization: Bearer <authorized-platform-operator-token>
Idempotency-Key: <unique-key>
Content-Type: application/json
```

```json
{
  "slug": "obroklik",
  "display_name": "Obroklik",
  "oauth_clients": [
    {"kind": "web", "redirect_uris": ["https://obroklik.example/auth/callback"]},
    {"kind": "backend"}
  ],
  "requirements": [
    {"contract": "platform.example", "version": ">=1.0.0 <2.0.0"}
  ]
}
```

**Behavior:** Validate payload; create pending app; provision or link ZITADEL project and requested clients; persist mapping; approve/attach requested contract grants **only when operator is authorized**; report status. Repeated calls with the same idempotency key and payload return the same operation/result; mismatched payload yields conflict.

Application registration does not automatically grant every service or every customer organization access.

### 10.2 Contract publication

```http
POST /v0/contracts
Authorization: Bearer <contract-publisher-token>
```

Publish manifest, OpenAPI, semantic documentation reference, schemas, and digest. Require namespace ownership/operator approval. Publishing the same version with a different digest returns `409 Conflict`; same digest is idempotent.

### 10.3 Service implementation registration

```http
POST /v0/implementations
Authorization: Bearer <approved-deployment-token>
```

```json
{
  "slug": "example-capability",
  "release_version": "0.1.0",
  "implements": [
    {"contract": "platform.example", "version": "1.0.0", "digest": "sha256:..."}
  ],
  "deployments": [
    {"environment": "local", "endpoint": "http://example-capability:8080"}
  ]
}
```

**Security:** Each approved deployment identity is authorized for particular implementation IDs/namespaces. Registration credentials cannot register arbitrary services, publish contracts, create applications, or assign themselves grants. Reject endpoints outside approved internal networks/schemes to prevent SSRF; avoid fetching arbitrary caller-provided OpenAPI URLs from the registry. Prefer upload of immutable contract bundles/digests.

### 10.4 Deployment and implementation lifecycle

```text
PENDING -> REGISTERED -> HEALTHY -> ACTIVE
                            |          |
                            v          v
                         DEGRADED   DISABLED
                            |
                            v
                        UNAVAILABLE
```

- Health probe only checks readiness; health is not proof of authorization or contract conformance.
- Registration can occur before readiness; only active + healthy bound implementations resolve.
- Service can be disabled independently of the app or contract.
- Stop accepting new resolutions for unhealthy deployments; do not automatically switch to another **stateful implementation**.
- On shutdown, deployment tooling can deregister a deployment; also detect crashes with health probes/leases.
- Unregistering a bound implementation requires explicit unbind/migration or fails with `409 Conflict`.

### 10.5 Grants and bindings

Two distinct administrative operations:

1. **Grant:** app may call contract `platform.example` with permissions `example:read`.
2. **Bind:** app's `platform.example.v1` requests resolve to `example-capability` in `local` environment.

Grants are **authorization policy**; bindings are **routing policy**. A valid binding never bypasses a missing grant.

---

## 11. Resolver behavior and SDK

### 11.1 API

Prefer a `POST` endpoint for complex resolution and to avoid leaking selectors in URLs:

```http
POST /v0/contracts/resolve
Authorization: Bearer <application-workload-access-token>
Content-Type: application/json
```

```json
{
  "contract": "platform.example",
  "version_constraint": ">=1.0.0 <2.0.0",
  "environment": "local"
}
```

Response:

```json
{
  "contract": "platform.example",
  "resolved_version": "1.0.0",
  "protocol": "http",
  "endpoint": "http://example-capability:8080",
  "expires_in_seconds": 300
}
```

The resolver must:

1. Validate workload token and map its verified client identity to a registered application.
2. Confirm app is active, contract requirement exists, and contract grant allows access.
3. Find a compatible **published** contract version.
4. Select the explicit binding for application + contract major + environment.
5. Confirm implementation declares that contract/version and deployment is active/healthy.
6. Return its stable logical endpoint and bounded TTL.
7. Audit authorization-sensitive administrative changes; sample normal resolutions for telemetry.

**Tenant context:** Basic routing resolution generally does not need a tenant. Tenant resolution is a separate operation and must verify user-delegated authorization. Future tenant-specific bindings can be added without changing contract identity.

### 11.2 Go SDK behavior

```go
client, err := platform.New(platform.Config{
    RegistryURL: "https://platform.example",
    TokenSource: tokenSource,
})

endpoint, err := client.Resolve(ctx, "platform.example", ">=1.0.0 <2.0.0")
```

- Cache by `(application identity, environment, contract, version constraint)`; never share across credentials/authorization contexts.
- Honor TTL; cap stale routing to 60 seconds on registry outage only if implementation remains reachable and credentials valid.
- Retry transient resolver failures with jitter and bounded backoff.
- Avoid automatic retries of non-idempotent contract calls unless idempotency keys or contract semantics permit.
- Generated contract clients come from published OpenAPI, not implementation-specific schemas.
- A direct contract call always carries an audience-correct access token; registry discovery tokens are not necessarily valid for capability APIs.

### 11.3 Error model

Use `application/problem+json` (RFC 9457), stable machine-readable error codes, trace/request IDs, and appropriate HTTP statuses:

- `400` invalid manifest or version constraint.
- `401` missing/invalid token.
- `403` valid caller lacking registration/grant/tenant authorization.
- `404` contract/application not visible to caller.
- `409` duplicate version with changed digest, binding conflict, invalid lifecycle transition.
- `422` contract incompatible or unmet prerequisite.
- `503` no healthy deployment for valid binding; retry guidance.

Avoid revealing internal endpoints or existence of private implementations to unauthorized callers.

---

## 12. API surface for v0

All admin APIs require explicit platform operator/publisher/deployment scopes; normal application clients receive only permitted discovery and context APIs.

| Method | Endpoint | Role |
|---|---|---|
| `POST` | `/v0/applications` | operator |
| `GET` | `/v0/applications` | operator |
| `GET` | `/v0/applications/{id}` | operator |
| `PATCH` | `/v0/applications/{id}` | operator |
| `POST` | `/v0/applications/{id}/suspend` | operator |
| `POST` | `/v0/organizations/{org_id}/applications/{app_id}/tenants` | provisioning operator |
| `GET` | `/v0/tenants/{id}` | authorized operator |
| `POST` | `/v0/tenants/resolve` | trusted authenticated delegated context |
| `POST` | `/v0/contracts` | publisher |
| `GET` | `/v0/contracts/{name}/versions/{version}` | authorized reader |
| `POST` | `/v0/implementations` | approved deployment identity |
| `PATCH` | `/v0/implementations/{id}` | authorized owner/operator |
| `POST` | `/v0/implementations/{id}/deployments` | approved deployment identity |
| `DELETE` | `/v0/deployments/{id}` | approved deployment identity |
| `POST` | `/v0/applications/{id}/contract-grants` | operator |
| `DELETE` | `/v0/applications/{id}/contract-grants/{grant_id}` | operator |
| `PUT` | `/v0/applications/{id}/bindings/{contract}` | operator |
| `POST` | `/v0/contracts/resolve` | application workload |
| `GET` | `/health/live` | infrastructure |
| `GET` | `/health/ready` | infrastructure |
| `GET` | `/metrics` | internal monitoring only |

**Note:** These are the planned canonical API shapes, not an existing implemented API. Publish the v0 OpenAPI spec before implementation and generate typed clients from it. API version `v0` signals pre-stable administrative interfaces; contract versioning is independent.

---

## 13. Security design

### 13.1 Trust boundaries

- Browser cannot hold backend client secrets or impersonate registered platform applications.
- Application workload identity is verified from the OAuth token, never a caller-supplied `application_id`.
- A workload credential does not establish a human user's permission to act for a tenant.
- An organization identifier supplied in a request is a **selection**, not authorization evidence.
- Service registration requires identity scoped to that service/namespace.
- Contract publication and grant/binding modification require separate privileged scopes.
- Service discovery never grants access by itself; capability services verify JWTs independently.

### 13.2 Recommended controls

- HTTPS externally; internal private Docker network; no public DB/implementation ports.
- Short-lived tokens, JWKS rotation, issuer/audience allowlist, strict clock-skew limits.
- Least-privilege DB roles and separate migration role.
- SQLx parameterized queries; no dynamic SQL built from unchecked user input.
- Per-operation audit events with actor, target, result, correlation ID.
- Redact Authorization headers, tokens, credentials, secret paths, and PII in logs.
- Request body limits, timeouts, rate limits on admin/registration APIs.
- Validate registration endpoints against allowlisted schemes/hosts/network ranges to prevent SSRF.
- Store immutable contract digests; reject manifest substitution.
- Explicit suspend/revoke operations; bounded cache TTL and revocation expectations.
- Secure secret injection and rotation; no credentials committed to Git.

### 13.3 Future RLS integration contract

A shared Rust persistence crate will expose `with_tenant_transaction(tenant_id, ...)` that sets tenant context using `SET LOCAL`/`set_config(..., true)` **inside a transaction**, ensuring pooled connections do not leak prior tenant state. Tenant-bearing tables must have RLS policies, forced RLS where appropriate, and a non-bypass DB role. v0 tests should include a minimal fixture table proving isolation even though business services arrive later.

---

## 14. Observability and operations

- JSON structured logs with `request_id`, `trace_id`, `actor_type`, `application_id`, `contract_id`, and safe error codes.
- OpenTelemetry-compatible tracing instrumentation; exporting to a collector can be deferred.
- Metrics: registration attempts/success/failures, provisioning retries, resolver latency and error rates, active bindings, unhealthy deployments, JWKS refresh failures.
- Health endpoints: liveness for process, readiness for required DB connectivity and critical initialization; avoid making every ZITADEL transient outage immediately kill core process.
- Database backups: nightly snapshots for deployed environments; rehearse restore before production.
- Document procedures for ZITADEL recovery, rotating client credentials, disabling an app, contract rollback, and registry outage.
- Run load tests on resolver (target to establish via benchmark; v0 should not assume a speculative QPS number).

---

## 15. Implementation phases and deliverables

### Phase 0 — Verify external assumptions and write ADRs

**Tasks**

1. Pin ZITADEL release and verify official Compose startup/init and resource requirements.
2. Prototype ZITADEL projects, applications, customer organizations, project grants, user roles, machine identities.
3. Verify claims for home-organization and external organization access; test selected-org authorization.
4. Test token exchange/delegation support; choose documented secure fallback if unsuitable.
5. Decide v0 operator auth mechanism and token audience strategy for platform core and capability services.
6. Write ADRs for IAM ownership, contract resolution, app/implementation binding, tenant mapping, Docker-first deployment.

**Exit:** Recorded test evidence for ZITADEL assumptions; no security-critical unknown left as an implicit assumption.

### Phase 1 — Repository and local infrastructure

**Tasks**

1. Create Cargo workspace, lint/format/test pipelines, Go SDK module.
2. Create Compose stack for Postgres, ZITADEL, platform core, example service, reverse proxy.
3. Add pinned images, volumes, health checks, `.env.example`, secret handling.
4. Implement SQLx migrations, config validation, health endpoints, RFC 9457 error layer, structured logs.
5. Write bootstrap and teardown scripts.

**Exit:** Fresh checkout + documented commands produce a healthy local stack; restarts preserve data.

### Phase 2 — IAM, app provisioning, tenant mapping

**Tasks**

1. Implement JWT/JWKS validation and workload-to-app mapping.
2. Implement operator authentication and scoped registration credentials.
3. Implement dynamic application registration with durable idempotent provisioning operations.
4. Integrate ZITADEL project/client creation and status reconciliation.
5. Implement tenant provisioning, organization/project mapping, and authorization resolution.
6. Add suspend, grant revocation, audit events, and tenant RLS fixture.

**Exit:** Obroklik and Fleet register without core restart; Company A maps to two distinct tenants; unauthorized tenant selection fails.

### Phase 3 — Contract publication and versioning

**Tasks**

1. Define contract manifest format, OpenAPI 3.1 publishing, content digests.
2. Implement immutable version publication and semantic-version constraints.
3. Add schema compatibility checks in CI and behavioral conformance fixture.
4. Publish `platform.example.v1` and a backward-compatible minor release.

**Exit:** Conflicting contract overwrite rejected; compatible versions resolve; breaking changes require a major version.

### Phase 4 — Implementations, grants, bindings, resolution

**Tasks**

1. Implement approved implementation/deployment registration and lifecycle.
2. Implement health checks, active/inactive status, stable endpoint validation.
3. Implement application requirements, contract grants, explicit bindings.
4. Implement deterministic contract resolver and permissions filtering.
5. Generate Go registry client and implement bounded resolution cache.
6. Run example capability as one service implementing one or two example contracts.

**Exit:** Approved app resolves and calls implementation by contract; unapproved app denied; unhealthy binding fails safely; no core restart needed.

### Phase 5 — Hardening and release readiness

**Tasks**

1. Security test suite, revocation-window measurement, credential rotation.
2. Crash/restart and database restore drills.
3. CI contract conformance gate and dependency scanning.
4. Basic performance and fault-injection testing.
5. Operator runbooks, API docs, architecture diagram, upgrade instructions.
6. Tag v0 release and pin Docker images/contract digests.

**Exit:** All P0/P1 acceptance tests pass; known limitations documented; v0 deployable to a single VPS without manual DB edits.

---

## 16. Acceptance test plan

Run tests against a **real Compose stack** with ZITADEL, PostgreSQL, platform core, and example implementation. Use mock ZITADEL responses only for unit tests; security-critical end-to-end claims require a real provider. Tests should be automated and deterministic, resetting test fixtures between runs.

### 16.1 Environment and bootstrap

| ID | Priority | Scenario | Expected result |
|---|---|---|---|
| ENV-01 | P0 | Fresh checkout, configure secrets, `docker compose up` + bootstrap | All required services healthy; no manual SQL edits |
| ENV-02 | P0 | Run bootstrap twice | No duplicate projects, apps, tenants, or contracts |
| ENV-03 | P0 | Restart core, ZITADEL, and Postgres containers | Registered state persists; service recovers |
| ENV-04 | P1 | Core starts before ZITADEL ready | Bounded retries; no corrupt partial state |
| ENV-05 | P0 | Inspect published ports | DB and capability ports not publicly exposed |
| ENV-06 | P1 | Kill core mid-provisioning, then restart | Pending operation reconciles or clearly reports failure |

### 16.2 IAM and tenant authorization

| ID | Priority | Scenario | Expected result |
|---|---|---|---|
| IAM-01 | P0 | Create Company A and B; register Obroklik/Fleet projects | Correct ZITADEL org/project/client relationships |
| IAM-02 | P0 | Same user accesses Obroklik and Fleet at Company A | One identity; independent product roles and tenant IDs |
| IAM-03 | P0 | Company A has both products | Distinct internal tenants per product |
| IAM-04 | P0 | User lacks Fleet role but org has Fleet project grant | Fleet user operation denied |
| IAM-05 | P0 | User passes Company B org ID in header/body | Rejected unless independently authorized |
| IAM-06 | P0 | User has valid additional Company B grant | Can select Company B only for authorized project/actions |
| IAM-07 | P0 | Access token has wrong issuer/audience or is expired | `401`; no business operation |
| IAM-08 | P0 | Browser ID token sent as capability API access token | Rejected |
| IAM-09 | P0 | Backend machine token claims arbitrary user ID | Cannot gain user-delegated tenant rights |
| IAM-10 | P0 | Registered app A attempts to impersonate app B via request field | Authorization uses token identity; spoof rejected |
| IAM-11 | P0 | Suspended tenant attempts a new operation | Denied regardless of existing token |
| IAM-12 | P1 | Revoke user/project grant | Access denied within measured/documented window |
| IAM-13 | P1 | Rotate signing key/client credential | Valid new credentials work; invalid old credentials fail as configured |
| IAM-14 | P0 | Tenant A queries tenant B's RLS fixture rows | No cross-tenant data returned or modified |
| IAM-15 | P0 | Reuse pooled DB connection after tenant A request for tenant B | No leaked tenant context |

### 16.3 Dynamic application registration

| ID | Priority | Scenario | Expected result |
|---|---|---|---|
| APP-01 | P0 | Operator registers new application while core is running | App becomes active; no restart |
| APP-02 | P0 | Unauthenticated caller registers app | `401` |
| APP-03 | P0 | Ordinary app token attempts app registration | `403` |
| APP-04 | P0 | Retry registration with same idempotency key and payload | Same operation/result, no duplicate ZITADEL project |
| APP-05 | P0 | Same idempotency key, different payload | `409` |
| APP-06 | P1 | ZITADEL provisioning fails temporarily | Pending/failed operation visible; retry safe |
| APP-07 | P0 | Suspend registered app | New resolutions and tenant operations denied |
| APP-08 | P1 | Invalid redirect URI or duplicate slug | Validation/conflict error; no active partial app |

### 16.4 Contract publishing and compatibility

| ID | Priority | Scenario | Expected result |
|---|---|---|---|
| CON-01 | P0 | Publish valid OpenAPI contract and manifest | Immutable version available |
| CON-02 | P0 | Publish same version and digest twice | Idempotent success |
| CON-03 | P0 | Publish same version with different digest | `409`, original unchanged |
| CON-04 | P0 | Publish malformed OpenAPI or invalid schema | Rejected with actionable error |
| CON-05 | P0 | Compatible minor version adds optional field | CI compatibility passes |
| CON-06 | P0 | Minor version removes required operation/field | CI compatibility fails |
| CON-07 | P0 | Breaking contract published as new major | Coexists with previous major |
| CON-08 | P1 | Service claims a contract digest that differs from published version | Registration rejected |
| CON-09 | P1 | Unprivileged app tries to publish contract | `403` |

### 16.5 Service registration and lifecycle

| ID | Priority | Scenario | Expected result |
|---|---|---|---|
| SVC-01 | P0 | Approved deployment identity registers implementation at runtime | Discoverable without core restart |
| SVC-02 | P0 | Unregistered workload attempts registration | `401/403` |
| SVC-03 | P0 | Service A credential attempts to register Service B | `403` |
| SVC-04 | P0 | Register arbitrary public/loopback/metadata endpoint | Rejected by endpoint policy |
| SVC-05 | P0 | Implementation supports two contracts | Both can be registered and independently granted/bound |
| SVC-06 | P0 | Two implementations support same contract | Both stored; resolver uses explicit binding |
| SVC-07 | P0 | Stop bound implementation | Health becomes unavailable; resolver returns controlled `503` |
| SVC-08 | P0 | Alternate implementation healthy while bound one fails | No silent cross-implementation failover |
| SVC-09 | P1 | Deployment comes back healthy | Resolution recovers without core restart |
| SVC-10 | P1 | Attempt delete of actively bound implementation | `409` or safe disable workflow |

### 16.6 Grants, binding, and resolver

| ID | Priority | Scenario | Expected result |
|---|---|---|---|
| RES-01 | P0 | Obroklik granted `platform.example` and bound to example implementation | Resolves endpoint and can invoke contract |
| RES-02 | P0 | Fleet lacks grant but knows contract name/endpoint | Registry denies resolution; service denies unauthorized direct call |
| RES-03 | P0 | Grant exists but binding absent | Explicit unresolved-binding error, no arbitrary implementation chosen |
| RES-04 | P0 | Binding exists but grant revoked | Resolution denied |
| RES-05 | P0 | App requests incompatible major | No compatible implementation returned |
| RES-06 | P0 | App requests compatible minor range | Highest allowed/pinned version selected per documented policy |
| RES-07 | P0 | Resolver returns logical Docker DNS/proxy endpoint | Never container/pod IP |
| RES-08 | P1 | Resolver endpoint changes after re-registration | SDK refreshes on TTL expiry |
| RES-09 | P1 | Registry temporarily unavailable, cached endpoint still reachable | Bounded routing-only stale cache works; auth expiry still enforced |
| RES-10 | P0 | Wrong-audience token used for direct capability call | Rejected by implementation |
| RES-11 | P1 | Non-idempotent operation fails mid-flight | SDK does not blindly replay it |

### 16.7 Security, reliability, and operations

| ID | Priority | Scenario | Expected result |
|---|---|---|---|
| SEC-01 | P0 | Fuzz/alter JWT signature, issuer, audience, claims | Invalid tokens rejected |
| SEC-02 | P0 | Inspect logs during provisioning and failed auth | No tokens, client secrets, passwords, or sensitive headers |
| SEC-03 | P0 | Attempt SQL injection in names/IDs/version constraints | Safe parameterized handling |
| SEC-04 | P0 | Attempt SSRF via registration endpoint or OpenAPI URL | Rejected; no internal metadata access |
| SEC-05 | P1 | Operator updates binding/grant | Audit event records actor, target, before/after, timestamp |
| SEC-06 | P1 | Burst of registration requests | Rate limits/body limits prevent resource exhaustion |
| OPS-01 | P0 | Backup and restore platform DB into fresh stack | Contracts, apps, bindings, tenants restored consistently |
| OPS-02 | P1 | Restore with ZITADEL DB snapshot from same backup point | External mappings remain consistent |
| OPS-03 | P1 | Run DB migrations against populated environment | No data loss; service recovers |
| OPS-04 | P1 | Kill a container during in-flight requests | Timeouts and errors bounded; no indefinite hang |
| OPS-05 | P1 | Observe service outage | Logs, metrics, readiness, and diagnostic errors identify fault |

### 16.8 Required end-to-end demo

Run this as a single automated acceptance workflow:

1. Bring up a clean Compose stack.
2. Bootstrap ZITADEL and platform core.
3. Register `obroklik` and `fleet` dynamically.
4. Provision Company A and Company B, with one user home in Company A.
5. Grant Company A both projects and Company B Fleet only; assign distinct user roles.
6. Publish `platform.example.v1` and register an implementation that supports it.
7. Grant/bind the example contract to Obroklik but not Fleet.
8. Use the Go SDK to resolve and invoke the contract successfully from Obroklik.
9. Verify Fleet cannot resolve or directly invoke it.
10. Register a second implementation of the same contract and prove the original binding remains stable.
11. Suspend Obroklik or revoke its contract grant; prove further access is blocked within the documented window.
12. Restart core and database; prove registrations and tenant mappings survive.
13. Attempt cross-organization spoofing and confirm denial.
14. Verify no public database port, no leaked secrets, and no container-instance IP in resolver output.

**Release gate:** All P0 tests must pass; P1 failures require documented disposition and explicit approval before production use. No known cross-tenant authorization bypass or identity spoofing is acceptable.

---

## 17. Non-goals and later roadmap

### Explicitly out of v0

- Catalog, Ordering, Notifications, Files, Inventory, Billing, Customers, Scheduling business APIs.
- Fleet vehicle/mileage/GPS ingestion, maintenance workflows, or Obroklik meal rules.
- Automatic container deployment, autoscaling, Kubernetes, service mesh.
- Public application developer onboarding, self-service marketplace, platform metering/billing.
- Universal CRUD abstraction, event sourcing, global distributed transactions.
- Arbitrary contract execution or dynamic code/plugin loading in registry.
- Automatic failover between stateful implementations with different data stores.
- Cross-application tenant data sharing by default.

### Planned evolution

**v1 capabilities:** Rust Notifications, Files, Catalog, Ordering, scheduling support as needed; Go app facades own domain-specific rules. Shared Rust persistence/auth/HTTP/messaging crates and generated Go contract clients. PostgreSQL service-owned databases, tenant tables, RLS. NATS JetStream and transactional outbox introduced with asynchronous workflows.

**Obroklik first slice:** login → menu → order → confirmation; menus/deadlines/eligibility/workplace/kitchen reports/QR confirmation in its Go facade, generic catalog/order storage in shared capabilities.

**Fleet first slice:** real GT06 GPS observation → Go gateway → NATS → normalized mileage processing → maintenance threshold alert; maintenance-first v1, route/live tracking later. GPS mileage estimated, with audited manual odometer corrections; notifications on state transitions to avoid spam.

**Kubernetes migration:** Keep registry and contract definitions unchanged. Register stable Kubernetes Service DNS names instead of Docker endpoints. Kubernetes handles pod readiness, rolling deploys, replica balancing, and scaling; the registry still chooses the implementation for each app/contract binding. No routing to individual pod IPs.

---

## 18. Design risks and mitigations

| Risk | Mitigation |
|---|---|
| ZITADEL project-grant claims differ from assumptions | Early real-provider spike; fail-closed active-org authorization |
| OAuth machine credentials misused as user authorization | Separate workload and user-delegated context; explicit verification |
| Cross-system provisioning partially succeeds | Durable idempotent reconciliation operations |
| Revoked grants remain usable in cached JWTs | Short token TTL, bounded authorization cache, measured revocation window |
| Dynamic service registration permits SSRF | Restricted registration identities, endpoint allowlist, no arbitrary URL fetching |
| Contract semver masks semantic incompatibility | Semantics docs + conformance CI + human review |
| Switching implementations silently splits state | Explicit bindings, no automatic cross-implementation failover |
| Resolver outage cascades to apps | Bounded SDK routing cache; direct data plane; health monitoring |
| Compose-to-K8s migration couples registry to runtime | Stable logical endpoints, orchestration-agnostic deployment model |
| Excessive v0 scope delays real products | One core binary, one sample capability, no business services, no NATS requirement |

---

## 19. Definition of done

v0 is complete when:

- [ ] A new product can be registered at runtime through an authenticated API, with its ZITADEL project and clients provisioned or associated.
- [ ] A new contract version can be published without redeploying platform core.
- [ ] An approved service implementation can register dynamically and claim one or multiple published contracts.
- [ ] An application can declare a contract requirement, receive an explicit grant and binding, and resolve a stable implementation endpoint without knowing its service name.
- [ ] Two implementations of one contract can coexist without unintended routing changes.
- [ ] A single user can access two products with distinct roles and distinct per-product tenant UUIDs.
- [ ] Additional cross-organization access can be authorized explicitly; arbitrary organization selection is rejected.
- [ ] Direct capability calls enforce application/workload authorization independently of discovery.
- [ ] Tenant RLS isolation is demonstrated with an integration fixture.
- [ ] Registrations and mappings survive restarts and a tested backup/restore.
- [ ] All P0 acceptance tests pass in Docker Compose.
- [ ] CI publishes contract conformance results and builds version-pinned images.
- [ ] OpenAPI, operational runbooks, security assumptions, and architectural decisions are documented.

---

## 20. Recommended first implementation ticket

**Ticket: ZITADEL identity and delegation spike**

Set up a pinned ZITADEL release in Docker Compose with PostgreSQL; create provider org, Obroklik/Fleet projects, Company A/B customer organizations, one home-org user with cross-org Fleet access, backend OAuth clients, and platform operator/deployment identities. Capture actual access-token claims and prove or disprove the intended delegated-user and active-organization authorization flows. Document token exchange support and revocation behavior. Do not begin tenant-aware capability middleware until this is validated.

**Why first:** The contract registry is straightforward CRUD plus versioning; the difficult and security-sensitive uncertainty is whether ZITADEL's real authorization and token semantics map cleanly onto our application-scoped tenant model.
