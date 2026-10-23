#!/usr/bin/env bash
# Logical backup of the MIDGARD core database (registry, grants, bindings, tenants, audit).
#   scripts/backup.sh <output.dump>          env: PGHOST PGPORT PGUSER PGPASSWORD, DB (default midgard_core)
# For ZITADEL, take its database snapshot at the SAME point in time (see docs/operations/backup-restore.md):
# tenant mappings reference ZITADEL organization/project ids.
set -euo pipefail
out="${1:?usage: backup.sh <output.dump>}"
pg_dump --format=custom --no-owner --file "$out" "${DB:-midgard_core}"
echo "backup written to $out ($(wc -c <"$out") bytes)"
