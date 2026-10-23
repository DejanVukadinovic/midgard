# ADR-013: Root `compose.yaml` over the v0 stack; ZITADEL by default; secrets generation

* Status: accepted
* Date: 2026-10-23

## Decision
The repository root `compose.yaml` `include`s the v0 stack and adds NATS, the synthetic provider/client, bootstrap and the test runner.
**ZITADEL v2.71.8 is the default identity provider** (profile `zitadel`); the offline `dev-idp` remains an opt-in profile. Identity-provider
facts that ZITADEL generates (project ids used as audiences, user ids used as operator/publisher/deployer subjects, client secrets) are produced by an
idempotent `zitadel-bootstrap` job into a named `idp` volume; services adopt them at start through an entrypoint wrapper (`with-idp-env`, Rust images)
or a small loader (Go fixture), overriding static defaults. Without the volume content (dev-IdP mode) the wrapper is a no-op, so one set of service
definitions serves both modes. Credentials for databases and the broker are generated into git-ignored files by
`deployments/compose/gen-dev-secrets.sh` before `up` (Compose file-secrets must exist at start), which is one more command than the build guide's literal three.

## Alternatives
Per-IdP Compose overlays (duplicates service definitions); generating secrets in a one-shot container into a volume for every service (rewrites working v0 wiring);
a fixed, pre-seeded ZITADEL export (not reproducible across releases).

## Tradeoffs
Dev secrets sit in an internal named volume readable by all services. ZITADEL is pinned by tag. The Compose files are validated by `docker compose config` and by running
the same bootstrap and test scripts against the real release as local processes; the containers themselves are exercised first in CI.

## Consequences
See [operations/zitadel.md](../operations/zitadel.md) for the verified ZITADEL behaviour and the code changes it required (JWT `typ`, audience scopes, home-org scope).
