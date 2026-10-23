#!/usr/bin/env bash
# Restore a core backup into a (new, empty) database. Never restores over a populated database.
#   scripts/restore.sh <input.dump> <target-db>      env: PGHOST PGPORT PGUSER PGPASSWORD (CREATEDB-capable role)
# After restoring, grant the runtime role access by running the migration job (idempotent) with the owner role.
set -euo pipefail
in="${1:?usage: restore.sh <input.dump> <target-db>}"; db="${2:?target database}"
if psql -d postgres -Atc "SELECT 1 FROM pg_database WHERE datname='$db'" | grep -q 1; then
  echo "refusing to restore into existing database $db" >&2; exit 1
fi
createdb "$db"
pg_restore --no-owner --dbname "$db" "$in"
# The runtime role needs the same privileges as in the source database.
psql -d "$db" -v ON_ERROR_STOP=1 -q <<'SQL'
GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA public TO midgard_runtime;
GRANT DELETE ON implementation_contracts, deployments, contract_bindings, application_requirements TO midgard_runtime;
REVOKE UPDATE ON audit_log FROM midgard_runtime;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO midgard_runtime;
GRANT DELETE ON tenant_fixture_notes TO midgard_runtime;
SQL
echo "restored into $db"
