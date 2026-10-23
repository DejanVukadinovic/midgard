# MIDGARD v0.2 — Build Guide

**Milestone:** Contract Specification & Reproducible Deployment  
**Status:** Implementation plan  
**Prerequisite:** v0 has passed its acceptance tests. Preserve existing v0 behavior and APIs unless a documented migration is necessary.

## 1. Objective

Deliver a production-oriented, language-independent, multi-protocol capability contract system and a **single Docker Compose deployment** that boots the MIDGARD infrastructure and runs end-to-end tests from a fresh clone.

MIDGARD Core is **infrastructure only**. Notifications, files, ordering, and every other business capability are outside Core. Example services and clients in this milestone are **synthetic test fixtures**, not business functionality.

### Non-negotiable principles

- **B required, A recommended:** Any language can implement a MIDGARD capability without importing MIDGARD libraries. Rust is the recommended SDK/runtime, never a protocol requirement.
- Prefer mature, commercially usable open-source tools over custom infrastructure. Record licensing, maintenance, operational cost, and rationale when selecting dependencies.
- PostgreSQL is the Core persistence standard; independent services may use any datastore.
- NATS JetStream is the **initial** messaging backend, not a permanent contract dependency. Keep the broker adapter replaceable; don't claim broker migration is configuration-only.
- No data-plane proxy through MIDGARD Core for every HTTP request or message. Core is the control plane; providers and brokers enforce authorization at invocation time.
- JSON is recommended, **not mandatory**. Use standard interface-description formats where possible.
- Contract definition, deployment binding, and resolved endpoint are separate concepts.

## 2. Scope

### In scope

1. Versioned MIDGARD capability manifest with IAM requirements and multiple protocol interfaces.
2. OpenAPI 3.1 HTTP operations and AsyncAPI 3.0 messaging operations, referenced or inline.
3. Validation, normalization, immutable publication, implementation registration, grants, and authorized resolution.
4. Initial HTTP and NATS JetStream integrations with clear transport-specific configuration.
5. Broker-independent messaging semantics and an initial NATS adapter; contract tests for adapter behavior.
6. End-to-end Docker Compose stack with ZITADEL, PostgreSQL, MIDGARD Core, NATS, synthetic provider/client, and test runner.
7. Repeatable bootstrap, migrations, health checks, smoke tests, security tests, and documentation.

### Out of scope

Business capabilities; Kubernetes; service mesh; a custom gateway; a new IAM product; a universal ORM; general workflow orchestration; automatic schema compatibility proofs; production-grade client generation; second production broker; a UI administration console.

## 3. Conceptual model

| Object | Owns | Does not own |
|---|---|---|
| **Capability contract** | Logical service/capability/version, IAM requirements, supported interfaces, operation semantics | Hostnames, broker URLs, runtime secrets |
| **Interface specification** | Protocol operation, input/output media types, schema, response/error behavior | Application grants, live deployments |
| **Implementation** | Provider identity, supported contract versions/interfaces, deployment endpoints, health | Canonical contract meaning |
| **Grant** | Which application/service/tenant may invoke a capability/interface | Payload shape |
| **Binding** | Which implementation serves an authorized application/tenant/contract | Interface schema |
| **Resolved contract** | Authorized view combining canonical contract and selected implementation connection metadata | Long-lived invocation authorization |

Suggested identity: `(service, capability, version)` with a stable identifier and a content digest. Published versions are immutable. A changed interface or IAM declaration requires a new version; changed deployment locations do not.

**Important:** One capability can have HTTP and MQ interfaces. Their externally observable semantics must agree. For example, both can mean *accepted for processing*, but not silently mean *completed* in one transport and *queued* in another.

## 4. Contract specification v0.2

The following is the **target manifest shape**, not a claim that the current v0 API already supports it. Use a JSON Schema to validate the MIDGARD-specific manifest; use existing OpenAPI/AsyncAPI parsers for referenced interface specifications.

```yaml
midgard: "0.2"
service: example.echo
capability: submit
version: 1.0.0
description: Accept a synthetic payload and return or acknowledge its identifier.

execution:
  outcome: accepted       # accepted | completed
  idempotency: supported  # required | supported | none

authorization:
  permission: example.echo:submit
  callers: [service, user]
  context:
    application: required
    tenant: required
  delegation: supported

interfaces:
  - id: rest
    protocol: http
    specification:
      type: openapi
      document: ./openapi.yaml
      operation: submitEcho

  - id: queue
    protocol: mq
    authorization:
      callers: [service]  # narrows, never widens, capability-level callers
    specification:
      type: asyncapi
      document: ./asyncapi.yaml
      operation: submitEcho
```

### 4.1 Schema rules

- Required: `midgard`, `service`, `capability`, `version`, `authorization`, and at least one `interfaces` entry.
- `service` and `capability` use documented, stable naming rules; reject collisions and ambiguous identifiers.
- Versions use SemVer. **v0.2 resolves exact versions by default**; do not silently select a newer version. Compatible-major ranges may be added only with explicit tests and policy.
- Interface `id` values are unique within the contract. `protocol` is a transport family (`http`, `mq`, later `grpc`); it is not a vendor name (`nats`).
- `specification` may contain an inline document **or** a relative/registered reference. Resolve relative paths at registration, store normalized immutable bytes and a digest, and prohibit arbitrary server-side URL fetching (SSRF risk).
- For HTTP, validate the OpenAPI operation ID exists and request/response media types and schemas are well formed. OpenAPI can describe JSON, XML, text, binary, and other content types.
- For MQ, validate the AsyncAPI operation exists and record the interaction model (`send`, `receive`, or request/reply where explicitly supported), message schema, headers, and acknowledgment expectations.
- Use standard schema languages as applicable; **do not invent a parallel payload type system**.
- Interface-level authorization can **restrict** capability-level authorization, never relax it.
- Contract authorization describes required permission and context, **not** individual users or grants. ZITADEL and MIDGARD's existing IAM/grant model remain authoritative.
- Do not place credentials, access tokens, tenant secrets, or broker passwords in manifests or resolved contracts.

### 4.2 HTTP operation example (`openapi.yaml`)

```yaml
openapi: 3.1.0
info:
  title: Synthetic Echo API
  version: 1.0.0
paths:
  /submit:
    post:
      operationId: submitEcho
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [id, value]
              properties:
                id: { type: string }
                value: { type: string }
      responses:
        '202':
          description: Accepted
          content:
            application/json:
              schema:
                type: object
                required: [id, status]
                properties:
                  id: { type: string }
                  status: { type: string, const: accepted }
        '401': { description: Unauthenticated }
        '403': { description: Unauthorized }
```

The synthetic MQ operation should use an AsyncAPI 3.0 document with the **same logical payload and acceptance semantics**. Include it as a real, validated fixture in the repository; don't hand-roll an AsyncAPI-like JSON format.

### 4.3 Implementation registration

```yaml
implementation: echo-provider-primary
provides:
  - service: example.echo
    capability: submit
    version: 1.0.0
    interfaces:
      - id: rest
        location: http://example-provider:8080
      - id: queue
        destination: example.echo.submit
        broker_binding: default-mq
```

Registration must use an authenticated, approved deployment/service identity. `location` is the **HTTP base URL**; method/path come from OpenAPI. MQ destinations are deployment metadata, not canonical contract identity. Validate advertised interfaces against the published contract. Maintain explicit application/tenant implementation bindings where stateful routing requires them.

### 4.4 Resolved contract

The authenticated resolver returns only authorized, supported interfaces of the bound implementation. Its response includes:

- Service, capability, immutable version and digest.
- Required authorization metadata **without leaking other parties' grants**.
- Protocol, endpoint/destination, invocation operation, and a reference or embedded snapshot of the interface specification.
- Enough connection metadata for the chosen protocol adapter; never raw infrastructure credentials.
- A bounded cache lifetime / revalidation policy.

A resolved endpoint is **not** a bearer authorization token. The target service validates identity, capability permission, active application/tenant context, and delegation on every invocation. MQ access must be enforced by broker credentials/ACLs or a trusted enforcement component; merely hiding a subject in the resolver is not security.

## 5. Authorization and tenancy

Reuse v0's ZITADEL-backed identities and application-scoped tenancy. Do not redefine the IAM data model in this milestone.

1. Authenticate the caller and identify its application/service identity.
2. Validate the requested active organization/project against ZITADEL grants and MIDGARD mapping.
3. Resolve the authoritative tenant ID; never trust an arbitrary client-supplied tenant header.
4. Evaluate the capability grant and interface-level restrictions.
5. Select only a bound, authorized implementation.
6. Return connection metadata; the provider/broker independently enforces invocation permissions.

Cover user-delegated versus service-only access explicitly. A service identity must not automatically inherit a user's permissions. Preserve v0 transaction-scoped PostgreSQL tenant RLS for Core-owned tenant data. Audit changes to contracts, grants, and bindings without logging sensitive payloads.

## 6. Messaging boundary

Keep the **logical messaging interface** independent of NATS-specific stream/subject names. Initial adapter: NATS JetStream.

Required baseline semantics:

- At-least-once delivery; consumers must be idempotent.
- Durable publish acknowledgment or a reported failure.
- Durable consumers, explicit acknowledgment, bounded redelivery/backoff, poison-message isolation, and observable failures.
- Correlation/request IDs and trace-context propagation where applicable.
- Tenant/application context is authenticated or cryptographically bound to trusted identity; message headers alone are not proof.
- No blanket global ordering or exactly-once claims.

Document which semantics are **guaranteed by MIDGARD**, which are **adapter capabilities**, and which require application logic. Test a deterministic in-memory adapter for API behavior and real NATS for persistence/recovery/security. A future broker migration requires a plan for in-flight messages, offsets, ACLs, and downtime; adapter interchangeability alone does not solve it.

For v0.2, use **one concrete, documented MQ interaction** in the synthetic fixture (for example, durable command submission with acknowledgment). Avoid building a generic event router or workflow engine. Do not force all transports to support identical delivery mechanics.

## 7. Persistence and migration

Use PostgreSQL for Core state. Extend existing v0 tables and APIs rather than replacing them gratuitously. Suggested logical records (map to current schema after inspection):

- `capability_contracts`: service, capability, version, normalized manifest, digest, publication status.
- `contract_interfaces`: interface ID, protocol, specification type, immutable document/digest, operation ID.
- `implementations`: authenticated provider identity, lifecycle/health metadata.
- `implementation_interfaces`: contract/interface reference, endpoint or destination, broker binding.
- Existing application/tenant grants and implementation bindings, extended only as necessary for interface restrictions.

Constraints: unique `(service, capability, version)`; immutable published contract content; FK integrity; explicit allowed transitions; idempotent registration; atomic publication. Migrations must preserve existing v0 data and pass v0 regression tests.

## 8. Docker Compose — full infrastructure deployment

**Acceptance goal:** from a clean checkout with Docker and Compose installed:

```bash
cp .env.example .env
docker compose up -d --build
docker compose ps
docker compose --profile test run --rm integration-tests
```

The commands are **target behavior to implement and verify**, not commands asserted to work before this milestone is built.

### 8.1 Services

| Compose service | Responsibility | Startup requirement |
|---|---|---|
| `postgres` | Persistent Core/ZITADEL PostgreSQL data, isolated DBs/users | Healthy database |
| `zitadel` | OIDC/IAM and initial bootstrap | Healthy HTTP/OIDC endpoint |
| `midgard-core` | Registry, authorization, contract resolution | DB migrated; ZITADEL reachable |
| `nats` | JetStream-enabled initial MQ backend | Healthy broker and persistence volume |
| `example-provider` | Synthetic HTTP and MQ capability implementation | Dependencies reachable |
| `example-client` | Synthetic application exercising resolution and invocation | Seeded identity/grants |
| `bootstrap` | Idempotent operator/project/client/grant/contract/implementation provisioning | Runs after required services are ready |
| `integration-tests` | End-to-end acceptance tests | Compose `test` profile only |

ZITADEL setup must follow the **actual supported bootstrap mechanism of the selected pinned version**, including any required initialization step. Avoid assuming one health check or a generic `depends_on` is sufficient for readiness.

### 8.2 Compose requirements

- Root-level `compose.yaml` is the documented entry point; split into included files only if it reduces complexity.
- Pin production-relevant container versions (prefer digests for reproducibility); document upgrade policy.
- Use named persistent volumes for PostgreSQL and JetStream.
- Use an internal Compose network; publish only developer-required ports to the host, preferably bound to `127.0.0.1`.
- Use health checks and dependency readiness; bootstrap retries must be bounded and idempotent.
- Put nonsecret defaults in `.env.example`; `.env` is gitignored. Never commit real credentials, ZITADEL machine tokens, or private keys.
- Prefer Docker secrets or mounted local secret files for sensitive configuration. Development-only bootstrap credentials must be clearly marked and never reused in production.
- Configure JetStream storage explicitly and verify persistence after container recreation.
- Support Apple Silicon and mainstream x86_64 environments or document any unsupported image architecture.
- Provide a `make up`, `make test`, `make down` convenience layer only if useful; **raw Docker Compose commands must work**.
- `docker compose down` must preserve volumes; `down -v` must be documented as destructive.

### 8.3 Idempotent bootstrap

Bootstrap should:

1. Initialize required databases/users and run schema migrations.
2. Complete ZITADEL first-instance setup, if not already complete.
3. Create or locate operator identity, synthetic application project, test organization, and service identity.
4. Create or locate appropriate ZITADEL grants and MIDGARD tenant mapping.
5. Publish the synthetic contract and its OpenAPI/AsyncAPI documents.
6. Register the provider's supported interfaces and stable Compose endpoints.
7. Grant the synthetic consumer the correct capability and bind its implementation.
8. Verify readiness and emit a concise nonsecret deployment summary.

Every operation must use a stable external identifier or lookup-before-create/upsert approach. Re-running bootstrap must not duplicate organizations, projects, clients, grants, or contract versions.

### 8.4 Network and address semantics

Internal service URLs such as `http://example-provider:8080` work **inside** the Compose network, not from browsers or mobile clients. Register internal endpoints for container-based test consumers; explicitly distinguish externally reachable endpoints if needed. Do not expose internal broker credentials or Docker DNS hostnames as if they were public URLs.

## 9. Suggested repository structure

```text
midgard/
├── compose.yaml
├── .env.example
├── core/                         # existing v0 Core
├── contracts/
│   ├── schema/
│   │   └── midgard-contract-v0.2.schema.json
│   └── examples/echo/
│       ├── contract.yaml
│       ├── openapi.yaml
│       └── asyncapi.yaml
├── adapters/
│   └── messaging/nats/          # only if it fits current repo structure
├── examples/
│   ├── provider/                 # synthetic service, no mandatory MIDGARD SDK
│   └── client/
├── deploy/
│   └── bootstrap/
├── tests/
│   ├── contract-validation/
│   ├── messaging-conformance/
│   ├── authorization/
│   └── integration/
├── docs/
│   ├── contracts-v0.2.md
│   ├── compose-quickstart.md
│   ├── iam-enforcement.md
│   └── adr/
└── README.md
```

Adapt directories to the existing repository rather than reorganizing working v0 code solely to match this illustration.

## 10. Implementation sequence

### Phase 0 — Baseline and ADRs

- [ ] Run and record the existing v0 suite; inventory APIs, schema, auth flows, and Compose assets.
- [ ] Write ADRs for contract identity/versioning, multi-protocol interfaces, broker boundary, and provider-side authorization.
- [ ] Confirm chosen OpenAPI/AsyncAPI validators and licenses.

**Exit:** Existing behavior is understood; no regressions introduced.

### Phase 1 — Contract specification

- [ ] Write MIDGARD manifest JSON Schema and explanatory examples.
- [ ] Implement YAML/JSON parsing and normalization.
- [ ] Validate referenced OpenAPI/AsyncAPI documents and operation IDs.
- [ ] Reject invalid references, unknown protocols, duplicate interface IDs, and privilege-widening overrides.
- [ ] Implement immutable publication and digest storage.

**Exit:** Valid fixtures publish; malformed fixtures fail with actionable errors.

### Phase 2 — Registry and resolution

- [ ] Extend implementation registration to advertise supported interface IDs and deployment locations.
- [ ] Preserve v0 grants and bindings; add interface-specific restrictions.
- [ ] Resolve by service/capability/exact version and active authorized context.
- [ ] Return only interfaces supported by the selected implementation and allowed for the caller.
- [ ] Define cache TTL and invalidation/revalidation behavior.

**Exit:** No restart needed for new contracts or implementations; unauthorized resolution fails.

### Phase 3 — HTTP path

- [ ] Build a synthetic HTTP provider from the OpenAPI fixture.
- [ ] Implement service-side JWT/tenant/grant enforcement without requiring MIDGARD libraries.
- [ ] Resolve the contract and invoke the provider directly.
- [ ] Test wrong media type, invalid payload, wrong tenant, revoked grant, and unavailable endpoint.

**Exit:** Authenticated, tenant-scoped HTTP invocation succeeds end-to-end.

### Phase 4 — MQ path

- [ ] Define broker-neutral messaging operations and adapter conformance tests.
- [ ] Implement NATS JetStream adapter and documented subject/consumer provisioning.
- [ ] Publish/consume the synthetic AsyncAPI-described message.
- [ ] Enforce publish/subscribe permissions **at the broker or trusted enforcement layer**.
- [ ] Test acknowledgment, redelivery, deduplication, poison messages, and broker restart recovery.

**Exit:** MQ invocation and isolation work without baking NATS details into canonical contracts.

### Phase 5 — One-command deployment

- [ ] Build root Compose stack and pin image versions.
- [ ] Automate ZITADEL/database/Core/bootstrap lifecycle.
- [ ] Add readiness checks, persistent volumes, and restart recovery.
- [ ] Add test profile and developer quickstart.
- [ ] Test clean deployment on a blank volume and re-run bootstrap.

**Exit:** A new developer can bring up and test the full system without manual database or IAM configuration.

### Phase 6 — Hardening and release

- [ ] Run full v0 regression suite plus v0.2 acceptance suite.
- [ ] Check secret scanning, dependency licensing, container image scanning, and reproducibility.
- [ ] Document trust boundaries, limitations, operational procedures, and known broker assumptions.
- [ ] Tag v0.2 only after a fresh-clone validation run.

## 11. Acceptance test matrix

| Area | Test | Pass condition |
|---|---|---|
| Regression | v0 tests | All pass unchanged or with documented migration |
| Contract | Valid OpenAPI + AsyncAPI | Registered and retrievable |
| Contract | Invalid operation/schema/reference | Rejected with actionable error |
| Contract | Published version mutated | Rejected; new version required |
| Contract | Non-JSON media type | Accepted and represented accurately |
| Registry | Multiple interfaces | One capability advertises HTTP and MQ |
| Registry | Partial implementation | HTTP-only implementation valid |
| Registry | Dynamic registration | New provider resolves without Core restart |
| Binding | Explicit tenant/application binding | Deterministic implementation selected |
| IAM | Missing capability grant | Resolution denied |
| IAM | User attempts service-only MQ | Resolution and actual publish denied |
| IAM | Cross-tenant request | Denied at resolver and provider |
| IAM | Revoked grant after resolution | Subsequent invocation denied, subject to documented token/ACL propagation limits |
| HTTP | Resolve then invoke | Correct response with documented semantics |
| MQ | Publish then consume | Durable delivery and acknowledgment |
| MQ | Consumer crash before ack | Redelivery and safe idempotent handling |
| MQ | Broker/container restart | Persisted messages survive |
| MQ | Direct unauthorized broker access | Denied by real broker enforcement |
| Language | Independent provider | Runs without MIDGARD SDK dependency |
| Compose | Fresh checkout | `docker compose up -d --build` reaches healthy state |
| Compose | Bootstrap repeated | No duplicated IAM or registry records |
| Compose | Restart | Persistent state and registrations remain intact |
| Compose | Test profile | Deterministic exit code and useful failure logs |

## 12. Definition of done

v0.2 is complete only when:

- The contract format is documented, schema-validated, versioned, and published with immutable artifacts.
- A logical capability can advertise **multiple communication interfaces**, each described by a standard specification.
- A separate implementation can register one or more of those interfaces and be resolved dynamically.
- IAM and tenant restrictions are enforced both during resolution **and at actual invocation**.
- A language-independent synthetic provider/client proves HTTP and MQ paths.
- NATS is an adapter choice, not part of contract identity.
- The full stack boots through Docker Compose from a clean checkout, with automated, idempotent bootstrap.
- All existing v0 tests and new v0.2 acceptance tests pass.
- Documentation accurately distinguishes guaranteed behavior from deferred features.

---

### Engineering note

**Do not interpret this guide as permission to rewrite v0.** Implement the smallest incremental changes necessary. If an existing v0 interface or database model differs from the examples here, preserve the working design where possible and document any intentional deviation with an ADR.
