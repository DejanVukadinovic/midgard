#!/usr/bin/env bash
# Run the full demo stack WITHOUT Docker (processes on localhost) against an existing PostgreSQL.
#   scripts/dev-local.sh up | down | restart-core | env
# Requires: cargo, go, curl, jq, psql, and PG_ADMIN_URL (superuser connection, default below).
# State (pids, logs, generated dev passwords) lives in the git-ignored .local/ directory.
source "$(dirname "$0")/lib.sh"
need cargo go curl jq psql

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOCAL="$ROOT/.local"
mkdir -p "$LOCAL"
: "${PG_ADMIN_URL:=postgres://midgard:midgard@localhost:5432/postgres}"
PG_HOSTPORT="$(sed -E 's#^postgres://[^@]*@([^/]*)/.*#\1#' <<<"$PG_ADMIN_URL")"
PGUSER="$(sed -E 's#^postgres://([^:@]*).*#\1#' <<<"$PG_ADMIN_URL")"
PGPASSWORD="$(sed -E 's#^postgres://[^:]*:([^@]*)@.*#\1#' <<<"$PG_ADMIN_URL")"
export PGHOST="${PG_HOSTPORT%%:*}" PGPORT="${PG_HOSTPORT##*:}" PGUSER PGPASSWORD

PORT_CORE=${PORT_CORE:-8080} PORT_IDP=${PORT_IDP:-9000} PORT_CAP=${PORT_CAP:-8082}
BIN="$ROOT/target/debug"

gen() { openssl rand -hex 16 2>/dev/null || head -c16 /dev/urandom | od -An -tx1 | tr -d ' \n'; }
load_env() {
  if [[ ! -f "$LOCAL/env" ]]; then
    umask 077
    {
      echo "MIDGARD_OWNER_PASSWORD=$(gen)"; echo "MIDGARD_APP_PASSWORD=$(gen)"
      echo "CAPABILITY_OWNER_PASSWORD=$(gen)"; echo "CAPABILITY_APP_PASSWORD=$(gen)"
    } >"$LOCAL/env"
  fi
  set -a; source "$LOCAL/env"; set +a
}

start() { # start <name> <command...>   (env inherited)
  local name="$1"; shift
  "$@" >"$LOCAL/$name.log" 2>&1 &
  echo $! >"$LOCAL/$name.pid"
}
stop() {
  local f="$LOCAL/$1.pid"
  if [[ -f "$f" ]]; then kill "$(cat "$f")" 2>/dev/null || true; rm -f "$f"; fi
}

core_env() {
  export PLATFORM_BIND_ADDR="127.0.0.1:$PORT_CORE" ZITADEL_ISSUER="http://localhost:$PORT_IDP"
  export PLATFORM_DATABASE_URL="postgres://midgard_app:$MIDGARD_APP_PASSWORD@$PG_HOSTPORT/midgard_core"
  export PLATFORM_MIGRATION_DATABASE_URL="postgres://midgard_owner:$MIDGARD_OWNER_PASSWORD@$PG_HOSTPORT/midgard_core"
  export PLATFORM_AUDIENCE=midgard-core PLATFORM_OPERATOR_SUBJECTS=operator-1
  export PLATFORM_PUBLISHER_SUBJECTS='publisher-1=platform.|examples.'
  export REGISTRY_ENDPOINT_ALLOWED_HOSTS=localhost REGISTRY_HEALTH_CHECK_INTERVAL_SECONDS=2
}

up() {
  load_env
  for n in example-capability platform-core dev-idp; do stop "$n"; done   # never orphan a previous stack
  sleep 1
  log "building binaries"
  (cd "$ROOT" && cargo build -q --bins) && (cd "$ROOT/examples/sample-application" && go build -o "$BIN/sample-application" .)
  log "preparing roles/databases"
  PSQL_ADMIN="psql -v ON_ERROR_STOP=1 -q" "$ROOT/deployments/compose/init/postgres/01-roles-and-databases.sh" >/dev/null

  DEV_IDP_ISSUER="http://localhost:$PORT_IDP" DEV_IDP_CONFIG="$ROOT/deployments/compose/dev-idp.json" DEV_IDP_BIND="127.0.0.1:$PORT_IDP" \
    start dev-idp "$BIN/dev-idp"
  wait_http "http://localhost:$PORT_IDP/.well-known/openid-configuration" 30

  ( core_env; "$BIN/platform-core" migrate ) >"$LOCAL/core-migrate.log" 2>&1 || { cat "$LOCAL/core-migrate.log" >&2; die "core migrations failed"; }
  ( CAPABILITY_MIGRATION_DATABASE_URL="postgres://capability_owner:$CAPABILITY_OWNER_PASSWORD@$PG_HOSTPORT/midgard_capability" "$BIN/example-capability" migrate ) >"$LOCAL/cap-migrate.log" 2>&1 || { cat "$LOCAL/cap-migrate.log" >&2; die "capability migrations failed"; }
  (core_env; start platform-core "$BIN/platform-core" serve)
  wait_http "http://localhost:$PORT_CORE/health/ready" 60

  ( export CAPABILITY_BIND_ADDR="127.0.0.1:$PORT_CAP" CAPABILITY_ISSUER="http://localhost:$PORT_IDP" CAPABILITY_AUDIENCE=platform.example \
      CAPABILITY_DATABASE_URL="postgres://capability_app:$CAPABILITY_APP_PASSWORD@$PG_HOSTPORT/midgard_capability" \
      PLATFORM_CORE_URL="http://localhost:$PORT_CORE" CAPABILITY_CLIENT_ID=example-deployer CAPABILITY_CLIENT_SECRET=dev-only-deployer \
      CAPABILITY_SELF_REGISTER=true CAPABILITY_PUBLIC_ENDPOINT="http://localhost:$PORT_CAP"
    start example-capability "$BIN/example-capability" )
  wait_http "http://localhost:$PORT_CAP/health/ready" 60
  log "stack is up: core=:$PORT_CORE idp=:$PORT_IDP capability=:$PORT_CAP (logs in .local/)"
}

case "${1:-}" in
  up) up ;;
  down) for n in example-capability platform-core dev-idp; do stop "$n"; done; log "stopped" ;;
  restart-core)
    load_env; stop platform-core; sleep 1; (core_env; start platform-core "$BIN/platform-core" serve)
    wait_http "http://localhost:$PORT_CORE/health/ready" 60 ;;
  env)
    cat <<ENV
export PLATFORM_URL=http://localhost:$PORT_CORE IDP_URL=http://localhost:$PORT_IDP
export OPERATOR_CLIENT_SECRET=dev-only-operator PUBLISHER_CLIENT_SECRET=dev-only-publisher
export CAPABILITY_URL=http://localhost:$PORT_CAP SAMPLE_APP=$BIN/sample-application CONTRACT_TOOL=$BIN/contract-tool
ENV
    ;;
  *) echo "usage: $0 up|down|restart-core|env" >&2; exit 2 ;;
esac
