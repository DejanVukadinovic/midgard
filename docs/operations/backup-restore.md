# Backup and restore

## What to back up
1. MIDGARD core database (`midgard_core`): applications, contracts, implementations, grants, bindings, tenants, audit.
2. ZITADEL's database (`zitadel`) **from the same point in time**: tenant rows reference ZITADEL organization/project ids; restoring only one side can orphan mappings (OPS-02, to be exercised once the ZITADEL path is verified).
3. Sample service databases (capability data) with the owning service.
4. Secrets (outside Git) and the ZITADEL master key — losing it makes the ZITADEL database unreadable.

## Procedure
```bash
scripts/backup.sh /backups/core-$(date +%F).dump                 # PG* env selects the server
scripts/restore.sh /backups/core-2026-10-23.dump midgard_core_new  # refuses an existing database
# run the migration job (owner role) to confirm schema; point a core at the new database; verify with the acceptance checks
```
`tests/acceptance/backup-restore.sh` drills this on the local stack: it restores into a fresh database, starts a second core on it, and verifies applications, tenants, bindings,
audit and resolution.

## Procedures to document per environment (stubs)
* ZITADEL recovery: restore its database + master key, then verify `/.well-known/openid-configuration` and JWKS before starting core.
* Rotating client credentials: create the new credential in the IdP, update the workload's secret, remove the old credential; core stores no client secrets.
* Disabling an application: `POST /v0/applications/{id}/suspend` (immediate); tenants: `POST /v0/tenants/{id}/suspend`.
* Contract rollback: published versions are immutable; point bindings at an implementation of the previous major/minor.
* Registry outage: already-resolved calls continue until the SDK TTL + 60 s stale bound; no new resolutions.
