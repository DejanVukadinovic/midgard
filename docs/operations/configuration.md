# Configuration reference

Configuration is environment-driven. Secrets may be supplied as `NAME_FILE=/path` (file contents, trimmed) where noted; mount them as Docker secrets.
Never put credentials in `.env`, images or Git.

## platform-core

| Variable | Default | Notes |
|---|---|---|
| `PLATFORM_BIND_ADDR` | `0.0.0.0:8080` | |
| `PLATFORM_DATABASE_URL`(`_FILE`) | required | **runtime** role: non-owner, `NOBYPASSRLS` |
| `PLATFORM_MIGRATION_DATABASE_URL`(`_FILE`) | — | owner role; used only by `platform-core migrate` |
| `ZITADEL_ISSUER` | required | exact `iss` accepted; any OIDC issuer works |
| `ZITADEL_JWKS_URL` | `<issuer>/oauth/v2/keys` | |
| `PLATFORM_AUDIENCE` | `midgard-core` | required `aud` for registry calls |
| `PLATFORM_TOKEN_MAX_LIFETIME_SECONDS` | `3600` | rejects longer-lived tokens; bounds the revocation window |
| `PLATFORM_OPERATOR_SUBJECTS` | required | comma-separated token `sub`s with administrative rights |
| `PLATFORM_PUBLISHER_SUBJECTS` | empty | `sub` or `sub=prefix1\|prefix2` (namespace restriction) |
| `PLATFORM_PROVISIONER` | `link` | `link` or `zitadel` |
| `ZITADEL_API_URL`, `ZITADEL_API_TOKEN`(`_FILE`), `ZITADEL_ORG_ID` | — | for the `zitadel` provisioner (unverified) |
| `REGISTRY_HEALTH_CHECK_INTERVAL_SECONDS` | `30` | |
| `REGISTRY_UNAVAILABLE_AFTER_FAILURES` | `3` | consecutive failed probes before `unavailable` |
| `REGISTRY_RESOLUTION_TTL_SECONDS` | `300` | returned to clients |
| `REGISTRY_DEFAULT_ENVIRONMENT` | `local` | |
| `REGISTRY_ENDPOINT_ALLOWED_HOSTS` | empty | non-empty ⇒ **only** these hosts (`name` or `*.suffix`); empty ⇒ safe default (no IPs/localhost/metadata) |
| `PLATFORM_ADMIN_RATE_LIMIT_PER_MINUTE` | `600` | process-wide, administrative routes |
| `RUST_LOG` | `info` | JSON logs; authorization headers are never logged |

Commands: `platform-core migrate` (one-shot, owner role) and `platform-core serve`. Migrations never run implicitly at serve time, so replicas cannot race.

## example-capability

`CAPABILITY_ISSUER`, `CAPABILITY_JWKS_URL`, `CAPABILITY_AUDIENCE` (default `platform.example`), `CAPABILITY_DATABASE_URL`(`_FILE`) (runtime role),
`CAPABILITY_MIGRATION_DATABASE_URL`(`_FILE`), `PLATFORM_CORE_URL`, `CAPABILITY_TOKEN_URL`, `CAPABILITY_CLIENT_ID`, `CAPABILITY_CLIENT_SECRET`(`_FILE`),
`CAPABILITY_CORE_AUDIENCE`, `CAPABILITY_SELF_REGISTER=true`, `CAPABILITY_PUBLIC_ENDPOINT`, `CAPABILITY_SLUG`, `CAPABILITY_ENVIRONMENT`.

## Health, logging, metrics

* `GET /health/live` process; `GET /health/ready` database reachable and migrated (IdP outages do not make core unready; token checks fail closed per request).
* JSON structured logs; request ids (`x-request-id`) propagate into audit records. Tokens, secrets and Authorization headers are not logged; the acceptance run asserts this.
* `GET /metrics` (Prometheus text): resolve ok/denied, registrations, provisioning failures, probe failures, unhealthy deployments, active bindings. **Do not route `/metrics` through a public edge** (the shipped Traefik config does not).
