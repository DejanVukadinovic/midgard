# Contributing to MIDGARD

Contributions are welcome. Acceptance is discretionary and guided by the project's internal, application-driven roadmap;
there are no support SLAs, no promise of compatibility with arbitrary external integrations, and no hosted offering.
Open an issue before large changes. By contributing you agree your contribution is licensed under the repository's
[Apache-2.0 license](LICENSE).

## Sign-off (DCO)

We use the [Developer Certificate of Origin](https://developercertificate.org/). Sign every commit: `git commit -s`.
No CLA is required.

## Prerequisites

| Tool | Version | Used for |
|---|---|---|
| Rust | per `rust-toolchain.toml` (rustfmt, clippy) | core, crates, services |
| Go | 1.24 | SDK, sample application |
| PostgreSQL | 16, reachable as a **superuser** (tests create throw-away databases and roles) | Rust integration tests, Docker-free acceptance |
| Docker + Compose v2 | recent | Compose quickstart, image builds |
| `curl`, `jq`, `psql` | any | scripts |
| `cargo-deny`, `govulncheck`, `gitleaks` | optional locally (CI runs them) | `make audit`, `make secrets-scan` |

Default test connection: `postgres://midgard:midgard@localhost:5432/postgres` (override with `TEST_DATABASE_ADMIN_URL`).
Quick local setup: `sudo -u postgres psql -c "create role midgard superuser login password 'midgard'"`.

## Quickstart for contributors

```bash
git clone https://github.com/dejanvukadinovic/midgard.git && cd midgard
cp .env.example .env
make dev-up && make bootstrap && make acceptance-compose   # Docker path (what CI runs)
make acceptance                                             # Docker-free path, local PostgreSQL
```

## Daily workflow

```bash
make fmt          # cargo fmt + gofmt
make lint         # clippy -D warnings, go vet
make test         # Rust (needs PostgreSQL) + Go
make contracts-check sdk-check
```

* Keep changes small and reviewable; add tests for behavioural changes. Security-relevant behaviour needs a negative test.
* README claims must be backed by an automated test or labelled planned/unverified.
* Never commit secrets, `.env`, customer data, database dumps or private-application code. `make secrets-scan` before pushing.
* Logs and errors must not contain tokens, secrets or personal data.

## Contracts

Contracts live in `contracts/<namespace>/<name>/v<major>/<version>/` (`manifest.yaml`, `openapi.yaml`, `semantics.md`).

1. **Published versions are immutable.** Never edit a released version directory; add a new version.
2. Additive change ⇒ new minor; breaking change ⇒ new **major** (`v2`). `make contracts-check` and the registry enforce compatibility.
3. Compute the digest with `contract-tool digest <dir>`; regenerate clients with `make sdk-generate` (CI fails on drift).
4. Contract changes need human review of `semantics.md` — OpenAPI alone does not describe behaviour.

Third-party implementations: see [docs/contributing/third-party-implementations.md](docs/contributing/third-party-implementations.md).

## Architecture decisions

Significant decisions need an ADR in `docs/adr/` (copy `ADR-000-template.md`): context, decision, alternatives,
tradeoffs, consequences. Record uncertainties instead of claiming unverified behaviour.

## Pull requests

Use the PR template; CI (`ci`, `security`) must be green. Maintainers may squash. Security issues: see [SECURITY.md](SECURITY.md).
