# Deployment notes

The Compose stack in `deployments/compose/` is a **local development and single-host starting point**, not a hardened production template.

## What the stack does
* PostgreSQL 16 with a named volume; `db-init` reconciles roles/databases on every `up`; migrations are one-shot jobs using owner roles; services use non-owner runtime roles.
* Only `edge` (Traefik) publishes a port, bound to `127.0.0.1:8088`; `/metrics` is not routed. The `platform` network is `internal` (no outbound internet).
* Containers: read-only root filesystem, all capabilities dropped, `no-new-privileges`, memory limits, restart policy, bounded JSON logs. Health checks gate `depends_on`; services additionally retry (Compose ordering is not readiness).
* Credentials: generated into `deployments/compose/.secrets/` (git-ignored), mounted as Docker secrets or `*_FILE`.

## Before using anything like this beyond localhost
1. Terminate **TLS** at the edge (or a cloud load balancer) and set issuer/public URLs to HTTPS; the shipped edge is plain HTTP on loopback.
2. Replace the dev IdP with a real IdP; remove the `dev-only-*` fixtures and the `compose.dev.yaml` override.
3. Pin image **digests** (tags are pinned in `.env.example`; digest pinning is a release-checklist item) and scan images.
4. Rotate/replace generated secrets with your secret manager; rotate the bootstrap/operator credentials after first initialization.
5. Restrict the host firewall; keep PostgreSQL private; schedule backups ([backup-restore.md](backup-restore.md)).
6. Set `PLATFORM_TOKEN_MAX_LIFETIME_SECONDS` / IdP access-token lifetime to your revocation target (≤ 5 minutes is the project goal).

## Resource requirements (measured guidance, local)
Core ≈ 30–60 MB RSS, example service ≈ 20 MB, PostgreSQL ≈ 100 MB idle. ZITADEL adds its own footprint (to be recorded by the spike). Ports: `8088` (edge); dev only: `9000`, `8082`.

## Upgrades
Pre-1.0: read the changelog; take a backup; pull new images; run the one-shot migration job; restart. Migrations are forward-only and written expand/contract.
Downgrades are restores from backup.
