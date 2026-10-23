# Acceptance matrix

Maps the v0 acceptance tests (ENV/IAM/APP/CON/SVC/RES/SEC/OPS) and v0.1 tests (OSS) to automated checks and says what has actually been run.

Legend — **A**: automated, run in the development sandbox against real PostgreSQL and real processes (no Docker daemon was available there).
**CI**: automated, runs in GitHub Actions (including the Docker Compose path), not run in the sandbox. **P**: partially covered. **U**: unverified / not implemented.
"Test IdP" = `platform-testkit`/`dev-idp`, which emits ZITADEL-shaped tokens; it does **not** prove ZITADEL behaviour.

## v0

| ID | Status | Where |
|---|---|---|
| ENV-01 fresh checkout + compose + bootstrap | CI | `acceptance-compose` job (README commands verbatim); sandbox ran the Docker-free equivalent (`make acceptance`) |
| ENV-02 bootstrap twice | A + CI | `acceptance.sh` runs bootstrap twice and again after restart |
| ENV-03 restart core / Postgres / IdP | A (core) + CI (whole stack) | `operations.rs::state_survives_core_restart`, `acceptance.sh`, `compose.sh` restarts every container incl. PostgreSQL and the IdP |
| ENV-04 core before IdP | A | `security.rs::idp_outage_fails_closed…` (core stays ready, 503/fail-closed), DB connect retries in `main.rs` (not unit-tested) |
| ENV-05 published ports | A + CI | `tests/acceptance/ports.sh` |
| ENV-06 kill core mid-provisioning | P | durable operation + resume tested via IdP failure (`operations.rs::provisioning_failure…`); a hard `SIGKILL` between steps is not automated |
| IAM-01 correct ZITADEL org/project/client relationships | **U** | requires live ZITADEL (spike) |
| IAM-02/03 one user, two products, distinct tenants | A | `tenancy.rs::one_user_two_products…`, `acceptance.sh` (test IdP) |
| IAM-04 role missing though org has project | A | `tenancy.rs` (carol) |
| IAM-05 org id in header/body | A | `tenancy.rs`, `acceptance.sh` |
| IAM-06 additional org grant | A | `tenancy.rs` (alice/bob) — claim shape from ADR-002 is **U** against ZITADEL |
| IAM-07 bad issuer/audience/expired → 401 | A | `security.rs::forged_and_misissued_tokens…` |
| IAM-08 ID token as access token | A | same |
| IAM-09 machine token claims a user | A | `tenancy.rs` |
| IAM-10 app impersonation by field | A | `tenancy.rs` |
| IAM-11 suspended tenant | A | `tenancy.rs` |
| IAM-12 revocation window | P | immediate for DB-backed state; IdP-side revocation window **U** (documented as token lifetime) |
| IAM-13 key rotation | P | JWKS unknown-`kid` refresh `crates/platform-auth/tests/rotation.rs`; client-credential rotation **U** |
| IAM-14/15 RLS and pooled connection | A | `tenancy.rs::rls_…`, `service.rs` |
| APP-01…05, 07, 08 | A | `registry.rs`, `operations.rs` |
| APP-06 IdP provisioning failure | A | `operations.rs::provisioning_failure_is_visible_and_retry_safe` (fake IdP API) |
| CON-01…04, 06, 07, 09 | A | `registry.rs`; compat unit tests |
| CON-05 compatible minor passes | A + CI | `compat` unit test, `registry.rs` (1.1.0), `make contracts-check` |
| CON-08 digest mismatch | A | `resolver.rs` |
| SVC-01…10 | A | `resolver.rs` (SVC-05 `implementation_may_serve_two_contracts…`) |
| RES-01…07 | A | `resolver.rs`, `acceptance.sh` |
| RES-08 re-registration/TTL | A | `resolver.rs::endpoint_change…`, Go `TestResolveCachesUntilTTL…` |
| RES-09 stale routing | A | Go `TestStaleIfErrorIsBoundedAndRoutingOnly` |
| RES-10 wrong audience at implementation | A | `service.rs`, `acceptance.sh` |
| RES-11 no replay of non-idempotent calls | A | Go `TestCallUsesContractAudience…` |
| SEC-01 token fuzz | A | `security.rs` (manual corpus, not a fuzzer) |
| SEC-02 no secrets in logs | P | acceptance scans core/service logs for tokens and dev secrets; not exhaustive |
| SEC-03 injection | A | `security.rs` |
| SEC-04 SSRF | A | `endpoint_policy` unit tests, `resolver.rs`; no URL-fetching code exists to test |
| SEC-05 audit | A | `resolver.rs`, `registry.rs` |
| SEC-06 rate/body limits | A | `security.rs` |
| OPS-01 backup/restore | A | `tests/acceptance/backup-restore.sh` |
| OPS-02 restore with ZITADEL snapshot | **U** | needs live ZITADEL |
| OPS-03 migrations on populated DB | P | migrate job re-run on populated DB in acceptance; no data-transforming migration yet |
| OPS-04/05 outage behaviour | P | timeouts configured; fault injection not automated |

## v0.1 (OSS)

| ID | Status | Notes |
|---|---|---|
| OSS-01 clean checkout + Compose | CI | `acceptance-compose` (`FRESH=1`) |
| OSS-02 no paid dependencies | A | nothing in the stack needs an account; dev IdP is local |
| OSS-03 secret-free repository | CI | gitleaks over full history + allowlisted, labelled fixtures |
| OSS-04 CI coverage | CI | `ci.yml`, `security.yml`; "passes on default branch" is verified only once the workflow has run |
| OSS-05 Go sample resolves without hard-coded URL | A | `acceptance.sh` |
| OSS-06 sample service dynamically registered | A | self-registration with an approved deployment identity |
| OSS-07 README commands succeed | CI | `acceptance-compose` + `scripts/check-docs.sh` |
| OSS-08 license + CONTRIBUTING + SECURITY | A | files present; `LICENSE` is Apache-2.0 |
| OSS-09 idempotent bootstrap | A (dev IdP) / **U** (ZITADEL) | |
| OSS-10 no private dependencies | A | repo builds and tests standalone |
| OSS-11 unauthorized registration | A | `security.rs`, `acceptance.sh` |
| OSS-12 unauthorized discovery/invocation | A | `resolver.rs`, `service.rs`, `acceptance.sh` |
| OSS-13 tenant isolation, spoofed headers | A | `tenancy.rs`, `service.rs`, `acceptance.sh` |
| OSS-14 contract immutability | A | `registry.rs` |
| OSS-15 recovery & repeatability | A (core restart, bootstrap repeat) + CI (whole stack) | |
| OSS-16 release hygiene | manual | [release-checklist.md](release-checklist.md) |

## Known gaps (do not claim these as done)
Live ZITADEL verification (IAM-01, OPS-02, claim shapes, revocation, key/credential rotation), real Docker runs in the sandbox, image-digest pinning, GitHub Action SHA pinning,
dependency advisory scan (advisory DB unreachable in the sandbox; license/ban/source checks passed), load testing of the resolver, kill -9 provisioning fault injection.
