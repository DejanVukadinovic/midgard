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

### Known limitations
- ZITADEL integration is not yet verified against a live release (see docs/operations/zitadel.md).
