# Changelog

All notable changes are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow SemVer for the platform; **contract versions are versioned independently and are immutable once published**.
Pre-1.0 platform API stability is described in [docs/contributing/api-stability.md](docs/contributing/api-stability.md).

## [Unreleased]

### Added
- MIDGARD core: application registry (idempotent provisioning), contract registry (immutable, compatibility-checked),
  implementation/deployment registry with readiness probing and endpoint policy, grants, bindings, resolver, tenants,
  delegated tenant resolution, audit log, rate and body limits.
- Shared Rust crates (auth, http, persistence/RLS, contracts); sample service and contract `platform.example` 1.0.0/1.1.0.
- Go SDK, generated contract client, sample application.
- Docker Compose stack, idempotent bootstrap, acceptance suite, CI, security scanning configuration.
- Documentation: architecture, ADR-001…008, threat model, operations guides.

### Added (v0.2, unreleased)
- Multi-protocol capability contracts: manifest + JSON Schema, OpenAPI 3.1 / AsyncAPI 3.0 validation, immutable
  publication (`POST /v0/capability-contracts`), interface registration, interface-aware resolution,
  `/v0/authorize` with `interface` and `application`. Migration `0003` is additive.
- `platform-messaging` (traits, conformance suite, in-memory and NATS JetStream adapters).
- Go provider/client fixture (`examples/echo`), v0.2 bootstrap and end-to-end script, root `compose.yaml`, NATS config with ACLs.
- Docs: contracts v0.2, messaging, IAM enforcement, compose quickstart, ADR-009…013, v0.2 acceptance status.
- ZITADEL v2.71.8 as the default Compose IdP: idempotent `scripts/zitadel-bootstrap.sh`, headless end-user tokens for tests, `PLATFORM_ACCEPT_JWT_TYP` (ZITADEL `typ: JWT` access tokens; ID-token markers still rejected), project-id audiences requested as scopes, `make acceptance-zitadel`.

### Known limitations
- ZITADEL integration is not yet verified against a live release (see docs/operations/zitadel.md).
- v0.2: the Compose deployment has not been executed in an environment with Docker (ZITADEL was verified as a release binary); MQ tenant binding is not implemented; OpenAPI/AsyncAPI validation is structural, not a full conformance check.
