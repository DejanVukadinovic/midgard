# MIDGARD

**M**odular **I**nfrastructure for **D**ynamic **G**overnance and **R**apid **D**evelopment

*Contract-driven infrastructure for reusable, multi-tenant application capabilities.*

MIDGARD lets applications depend on **versioned contracts** instead of concrete services. Operators
register applications and service implementations at runtime; explicit **grants** and **bindings**
decide which implementation an application resolves, and every `(organization, application)` pair maps
to an isolated internal **tenant**. It is open source (Apache-2.0), driven by the real needs of the
products it accelerates, and is **not** a hosted platform or marketplace.

> **Status: pre-1.0 (v0.1).** APIs under `/v0` may change; published *contract versions* are immutable.
> The table below separates what is implemented and tested from what is not yet verified.

## Current status

| Capability | State |
|---|---|
| Application registry with idempotent, resumable provisioning (`Idempotency-Key`) | Implemented, tested |
| Versioned, content-addressed, immutable contract registry + OpenAPI compatibility check | Implemented, tested |
| Implementation/deployment registry, endpoint (SSRF) policy, readiness probing | Implemented, tested |
| Application↔contract **grants** and explicit **bindings**; deterministic resolver; no cross-implementation failover | Implemented, tested |
| Tenant mapping `(org, application) → tenant_id`, delegated-user tenant resolution, fail-closed org selection | Implemented, tested against a **test IdP** |
| PostgreSQL row-level security helper + fixture, non-owner runtime role | Implemented, tested |
| Rust token verification (JWKS, issuer/audience, ID-token rejection), shared libraries | Implemented, tested |
| Go SDK (resolver cache, bounded stale-if-error, no replay of non-idempotent calls) + generated contract client | Implemented, tested |
| Sample contract `platform.example`, sample Rust service, sample Go application | Implemented, end-to-end tested |
| Docker Compose stack, idempotent bootstrap, acceptance suite | Implemented; Compose path runs in CI (see [acceptance matrix](docs/contributing/acceptance-matrix.md)) |
| **ZITADEL integration** (project/client provisioning, real claim shapes, delegation, revocation) | **Written against documented APIs, NOT yet verified against a live release** — see [ZITADEL status](docs/operations/zitadel.md) |
| Kubernetes, NATS/outbox, business capabilities | Not implemented (non-goals for v0.x) |

## Architecture

```text
 Browser / mobile ──OIDC+PKCE──▶  Identity provider (ZITADEL; offline dev IdP for the demo)
        │                                   │ tokens
        ▼                                   ▼
 Go application facade ──(1) resolve contract──▶ ┌─────────────── MIDGARD core (Rust) ─────────────┐
        │   (workload token)                     │ applications · contracts · implementations      │
        │ ◀──── stable logical endpoint ──────── │ grants · bindings · tenants · resolver · audit  │
        │                                        └──────────────────────┬───────────────────────────┘
        │ (2) call directly, own token (data plane)                     │ PostgreSQL (RLS, audit)
        ▼
 Authorized implementation  ──asks core "may this app call me? which tenant?"──▶ (deployment identity)
 (stable Compose DNS name / reverse proxy; replicas are infrastructure's concern)
```

Control plane (register, grant, bind, resolve) is separate from the data plane (facade → implementation).
Possession of a resolved URL is never authorization: implementations verify tokens themselves and
confirm grants with core. See [architecture overview](docs/architecture/overview.md),
[auth and tenancy](docs/architecture/auth-and-tenancy.md) and
[contracts and resolution](docs/architecture/contracts-and-resolution.md).

## Quickstart (Docker Compose, no paid services)

Prerequisites: Docker with Compose v2, `curl`, `jq`, Rust (see `rust-toolchain.toml`) and Go 1.24 for the
host-side tools. Resource needs: ≈1.5 GB RAM, ports `8088` (edge), `9000`/`8082` (dev only), all loopback.

```bash
git clone https://github.com/dejanvukadinovic/midgard.git
cd midgard
cp .env.example .env
make dev-up                # generate local-only secrets, build images, start the stack
make bootstrap             # register contracts, two apps, tenants, grant + binding (idempotent)
make acceptance-compose    # full end-to-end acceptance scenario on clean volumes
```

No Docker? `make acceptance` runs the same scenario with local processes against a local PostgreSQL
(see [getting started](docs/getting-started.md)). The demo uses an **offline development IdP**
(`tools/dev-idp`); it is explicitly not for production. The CI job runs exactly the commands above.

## Register a contract, a service and an application

`scripts/bootstrap.sh` does this through the public API only; the essential calls
(full reference: [`contracts/registry/v0/openapi.yaml`](contracts/registry/v0/openapi.yaml)):

```bash
# publisher: publish an immutable contract version (same content again is a no-op, changed content is 409)
curl -H "Authorization: Bearer $PUBLISHER" -d "$(contract-tool request contracts/platform/example/v1/1.0.0)" $PLATFORM/v0/contracts
# operator: approve the service's deployment identity; the service then self-registers its implementation
curl -H "Authorization: Bearer $OPERATOR" -d '{"subject":"example-deployer","slug_pattern":"example-*"}' $PLATFORM/v0/deployment-identities
# operator: register an application (idempotent), create tenants, then grant + bind
curl -H "Authorization: Bearer $OPERATOR" -H 'Idempotency-Key: k1' -d @app.json $PLATFORM/v0/applications
curl -H "Authorization: Bearer $OPERATOR" -d '{"contract":"platform.example","permissions":["example:read"]}' $PLATFORM/v0/applications/$APP/contract-grants
curl -X PUT -H "Authorization: Bearer $OPERATOR" -d '{"major":1,"environment":"local","implementation":"example-capability"}' $PLATFORM/v0/applications/$APP/bindings/platform.example
```

The Go sample application then resolves `platform.example` without knowing any service name or URL
([`examples/sample-application`](examples/sample-application)); an ungranted application is denied.

## Repository map

| Path | Purpose |
|---|---|
| `crates/` | Shared Rust libraries: `platform-auth`, `platform-http`, `platform-persistence`, `platform-contracts`, `platform-testkit` |
| `services/platform-core` | The core executable (registry, grants, bindings, resolver) |
| `services/example-capability` | Sample Rust service implementing `platform.example` |
| `tools/dev-idp` | Offline development identity provider (never deploy) |
| `contracts/` | Public contract packages and the registry API description |
| `sdk/go` | Go SDK + generated contract client |
| `examples/sample-application` | Sample Go application facade |
| `deployments/` | Compose stack, Dockerfiles, edge config |
| `migrations/` | SQL migrations (run as a separate one-shot job) |
| `tests/acceptance` | End-to-end scenarios; [acceptance matrix](docs/contributing/acceptance-matrix.md) |
| `docs/` | Architecture, ADRs, operations, security |

## More

[Getting started](docs/getting-started.md) · [ADRs](docs/adr/) · [Threat model](docs/security/threat-model.md) ·
[Operations](docs/operations/) · [Acceptance matrix](docs/contributing/acceptance-matrix.md) ·
[Contributing](CONTRIBUTING.md) · [Security policy](SECURITY.md) · [License](LICENSE) (Apache-2.0) ·
[Changelog](CHANGELOG.md)

## Future ideas (not implemented)

Possible separate projects, mentioned only as ideas and not part of MIDGARD: an external-integration
layer (*Yggdrasil*), a standalone IAM (*Heimdall*), an API gateway (*Bifröst*). Kubernetes support is a
future option; the registry deliberately returns orchestration-neutral logical endpoints so it can be added.
