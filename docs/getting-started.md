# Getting started

Two supported local paths run the same acceptance scenario (`tests/acceptance/acceptance.sh`).

## A. Docker Compose (canonical)

```bash
cp .env.example .env
make dev-up                # secrets → images → postgres, core, dev IdP, example service, edge
make bootstrap             # idempotent: contracts, deployment identity, apps, tenants, grant + binding
make acceptance-compose    # clean volumes → full scenario → whole-stack restart → scenario again
make dev-down
```

Published ports (all loopback): `8088` edge (`platform.localhost` → core `/v0`, `/health`), and for development only
`9000` dev IdP and `8082` example service ([`compose.dev.yaml`](../deployments/compose/compose.dev.yaml)). PostgreSQL and
service ports are never published (`tests/acceptance/ports.sh`). Generated credentials live in
`deployments/compose/.secrets/` (git-ignored); `dev-only-*` values are documented demo fixtures.

## B. Docker-free (processes on localhost)

Needs a local PostgreSQL with a superuser (`PG_ADMIN_URL`, default `postgres://midgard:midgard@localhost:5432/postgres`).

```bash
make acceptance      # builds, starts dev IdP + core + example service, runs acceptance + backup/restore drill
make local-down
```

Logs are written to `.local/` (git-ignored). `scripts/dev-local.sh env` prints the environment used by the scripts.

## What the scenario proves

Two synthetic organizations (`org-acme`, `org-globex`), two applications (`obroklik`, `fleet`), one sample implementation:
idempotent bootstrap → authorized resolution + invocation from the Go sample app → ungranted app denied at the registry and
at the service → wrong-audience token rejected → distinct tenants per `(organization, application)` → spoofed organization or tenant
headers rejected → tenant-isolated data → unauthorized registration refused → core restart with state intact.

## Using a real identity provider

See [operations/zitadel.md](operations/zitadel.md). The ZITADEL path is a template that has **not** been verified here.
