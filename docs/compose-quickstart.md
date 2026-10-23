# Compose quickstart (v0.2)

Prerequisites: Docker with Compose v2, ≈2 GB RAM, ports `8088`, `9000`, `8082` free (all bound to `127.0.0.1`).
Images are built from source (Rust 1.97, Go 1.26); no paid services.

```bash
cp .env.example .env
deployments/compose/gen-dev-secrets.sh            # random LOCAL-ONLY credentials into deployments/compose/.secrets (git-ignored)
docker compose up -d --build --wait
docker compose ps
docker compose --profile test run --rm integration-tests
```

`make up`, `make test-compose` and `make down` wrap the same commands. The extra `gen-dev-secrets.sh` step is
the one deviation from a literal "three commands": credentials are mounted as files (Compose `secrets:`),
which must exist before `up`; `make up` runs it for you.

## What starts

| Service | Role |
|---|---|
| `postgres`, `db-init` | PostgreSQL 16 with a named volume; idempotent role/database reconciliation |
| `core-migrate`, `platform-core` | migrations (one-shot), then Core on the internal network |
| `dev-idp` | **offline development IdP** (profile `dev-idp`, enabled via `COMPOSE_PROFILES` in `.env`) |
| `nats`, `nats-init` | NATS JetStream with a file-storage volume; idempotent stream/consumer provisioning |
| `example-capability` | v0 sample service (`platform.example`) |
| `example-provider` | v0.2 synthetic provider (HTTP + queue), Go, no MIDGARD library |
| `bootstrap` | `scripts/bootstrap.sh` + `scripts/bootstrap-v02.sh`; re-runnable, creates nothing twice |
| `edge` | Traefik, loopback-only |
| `example-client` (profile `demo`) | one-shot synthetic client call |
| `integration-tests` (profile `test`) | `tests/integration/v02.sh`; non-zero exit on any failure |

Only the dev-IdP, the sample service and the edge publish host ports (loopback). PostgreSQL, NATS and the
provider are reachable only on the internal Compose network; internal names like `http://example-provider:8080`
mean nothing outside it. `docker compose down` keeps volumes; **`docker compose down -v` deletes all state**
(database, JetStream data).

## Docker-free equivalent

```bash
NATS_SERVER_BIN=/path/to/nats-server make acceptance-interfaces   # PostgreSQL superuser + cargo + go + jq required
```

## Known gaps — read before relying on this

* **ZITADEL is not part of this stack.** The default IdP is the offline `dev-idp`. A `zitadel` profile template
  from v0 remains in `deployments/compose/compose.yaml` but its first-instance bootstrap has **not** been
  built or verified; see [operations/zitadel.md](operations/zitadel.md). v0.2's IAM enforcement is tested against `dev-idp`.
* **The Compose path has not been run in the authoring environment** (no Docker daemon was available).
  `docker compose config` validates; the image builds and the `v02-compose` CI job are the first real execution.
  The same scenario *was* executed with real PostgreSQL, Core, dev-idp, `nats-server` 2.10.22 and the Go
  fixture as local processes (`make acceptance-interfaces`).
* Images are pinned by tag, not digest (see [operations/deployment.md](operations/deployment.md) for the upgrade policy).
* Dev credentials in `compose.yaml` (`dev-only-*`) belong to the dev IdP fixtures only.
