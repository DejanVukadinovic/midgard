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
| `zitadel`, `zitadel-bootstrap` | **ZITADEL v2.71.8** (`start-from-init`, issuer `http://zitadel:8080`) and the idempotent job that creates projects, orgs, grants, machine/human users and OIDC apps, publishing ids and secrets to the `idp` volume. Default profile `zitadel` |
| `dev-idp` | optional offline development IdP (profile `dev-idp`; see below) |
| `nats`, `nats-init` | NATS JetStream with a file-storage volume; idempotent stream/consumer provisioning |
| `example-capability` | v0 sample service (`platform.example`) |
| `example-provider` | v0.2 synthetic provider (HTTP + queue), Go, no MIDGARD library |
| `bootstrap` | `scripts/bootstrap.sh` + `scripts/bootstrap-v02.sh`; re-runnable, creates nothing twice |
| `edge` | Traefik, loopback-only |
| `example-client` (profile `demo`) | one-shot synthetic client call |
| `integration-tests` (profile `test`) | `tests/integration/v02.sh`; non-zero exit on any failure |

Only the dev-IdP (if used), the sample service and the edge publish host ports (loopback). ZITADEL is not published: its issuer
`http://zitadel:8080` exists only on the Compose network (ZITADEL routes by `Host`). PostgreSQL, NATS and the
provider are reachable only on the internal Compose network; internal names like `http://example-provider:8080`
mean nothing outside it. `docker compose down` keeps volumes; **`docker compose down -v` deletes all state**
(database, JetStream data).

## Identity provider modes

* **ZITADEL (default, `COMPOSE_PROFILES=zitadel`).** `zitadel-bootstrap` writes `idp.env` / `secrets.env` into the `idp` volume; Core, the
  example service (via the `with-idp-env` entrypoint wrapper), the Go fixture (`idpenv.go`) and the test scripts read audiences (ZITADEL project
  ids), operator/publisher/deployer subjects (user ids), client ids and secrets from there. Nothing ZITADEL-specific is committed or typed by hand.
* **Offline dev IdP** (no ZITADEL; fastest, least realistic): in `.env` set `COMPOSE_PROFILES=dev-idp`, `IDP_ISSUER=http://dev-idp:9000`,
  `IDP_TOKEN_URL=http://dev-idp:9000/oauth/v2/token`, `PLATFORM_ACCEPT_JWT_TYP=false`; run the tests with
  `docker compose --profile dev-idp run --rm integration-tests` (do not pass `--profile test` alone: that selects the ZITADEL services).

## Docker-free equivalents (these *have* been run)

```bash
ZITADEL_BIN=/path/to/zitadel NATS_SERVER_BIN=/path/to/nats-server make acceptance-zitadel   # real ZITADEL release binary
NATS_SERVER_BIN=/path/to/nats-server make acceptance-interfaces                              # offline dev IdP
```

(PostgreSQL superuser, cargo, go, jq and curl required.)

## Known gaps — read before relying on this

* **The Compose path has not been run in the authoring environment** (no Docker daemon). `docker compose config` validates the files; the image
  builds, the ZITADEL container, the `with-idp-env` wrapper inside the images, and the `v02-compose` CI job are the first real execution.
  The identical scenarios *were* executed against the real ZITADEL v2.71.8 binary, PostgreSQL, `nats-server` 2.10.22 and the Go fixture as local
  processes. ZITADEL findings and unverified items: [operations/zitadel.md](operations/zitadel.md).
* The demo `sample-application` service (profile `demo`) still targets the dev IdP only; the v0.2 fixture works with both.
* `PLATFORM_PROVISIONER=zitadel` (Core creating projects through the management API) is unverified; the stack uses link mode.
* Images are pinned by tag, not digest (see [operations/deployment.md](operations/deployment.md)).
* Dev credentials (`dev-only-*`, the throwaway client secrets and user passwords in the `idp` volume) are for local use only.
