#!/usr/bin/env bash
# Idempotent role/database setup. Runs from /docker-entrypoint-initdb.d on first start, and is
# also used by scripts/dev-local.sh. Passwords come from the environment (never committed).
#   PSQL_ADMIN   command that connects as a superuser (default: psql -U "$POSTGRES_USER")
set -euo pipefail
PSQL_ADMIN="${PSQL_ADMIN:-psql -v ON_ERROR_STOP=1 -U ${POSTGRES_USER:-postgres}}"
: "${MIDGARD_OWNER_PASSWORD:?}" "${MIDGARD_APP_PASSWORD:?}" "${CAPABILITY_OWNER_PASSWORD:?}" "${CAPABILITY_APP_PASSWORD:?}"

# role <name> <login-options...>: create if missing, always (re)set the password.
$PSQL_ADMIN -d postgres <<SQL
DO \$\$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='midgard_runtime')   THEN CREATE ROLE midgard_runtime NOLOGIN; END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='capability_runtime') THEN CREATE ROLE capability_runtime NOLOGIN; END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='midgard_owner')     THEN CREATE ROLE midgard_owner LOGIN; END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='midgard_app')       THEN CREATE ROLE midgard_app LOGIN NOSUPERUSER NOBYPASSRLS; END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='capability_owner')  THEN CREATE ROLE capability_owner LOGIN; END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='capability_app')    THEN CREATE ROLE capability_app LOGIN NOSUPERUSER NOBYPASSRLS; END IF;
END \$\$;
ALTER ROLE midgard_owner    PASSWORD '${MIDGARD_OWNER_PASSWORD}';
ALTER ROLE midgard_app      PASSWORD '${MIDGARD_APP_PASSWORD}';
ALTER ROLE capability_owner PASSWORD '${CAPABILITY_OWNER_PASSWORD}';
ALTER ROLE capability_app   PASSWORD '${CAPABILITY_APP_PASSWORD}';
GRANT midgard_runtime   TO midgard_app;
GRANT capability_runtime TO capability_app;
SQL

create_db() { # create_db <db> <owner>
  if ! $PSQL_ADMIN -d postgres -Atc "SELECT 1 FROM pg_database WHERE datname='$1'" | grep -q 1; then
    $PSQL_ADMIN -d postgres -c "CREATE DATABASE $1 OWNER $2"
  fi
  # Runtime roles may connect but own nothing; PUBLIC loses default access.
  $PSQL_ADMIN -d postgres -c "REVOKE ALL ON DATABASE $1 FROM PUBLIC"
}
create_db midgard_core midgard_owner
create_db midgard_capability capability_owner
$PSQL_ADMIN -d postgres -c "GRANT CONNECT ON DATABASE midgard_core TO midgard_app, midgard_owner"
$PSQL_ADMIN -d postgres -c "GRANT CONNECT ON DATABASE midgard_capability TO capability_app, capability_owner"

# ZITADEL keeps its own database (only used by the `zitadel` Compose profile).
if [[ -n "${ZITADEL_DB_PASSWORD:-}" ]]; then
  $PSQL_ADMIN -d postgres <<SQL
DO \$\$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='zitadel') THEN CREATE ROLE zitadel LOGIN; END IF; END \$\$;
ALTER ROLE zitadel PASSWORD '${ZITADEL_DB_PASSWORD}';
SQL
  $PSQL_ADMIN -d postgres -Atc "SELECT 1 FROM pg_database WHERE datname='zitadel'" | grep -q 1 || $PSQL_ADMIN -d postgres -c "CREATE DATABASE zitadel OWNER zitadel"
fi
