#!/usr/bin/env bash
# OPS-01 (local-process stack): back up the core DB, restore into a fresh database, start a second core on
# that database and verify registrations, bindings, tenants and the audit trail are restored consistently.
source "$(dirname "$0")/../../scripts/lib.sh"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
: "${PG_ADMIN_URL:=postgres://midgard:midgard@localhost:5432/postgres}"
PGUSER="$(sed -E 's#^postgres://([^:@]*).*#\1#' <<<"$PG_ADMIN_URL")"
PGPASSWORD="$(sed -E 's#^postgres://[^:]*:([^@]*)@.*#\1#' <<<"$PG_ADMIN_URL")"
HP="$(sed -E 's#^postgres://[^@]*@([^/]*)/.*#\1#' <<<"$PG_ADMIN_URL")"
export PGHOST="${HP%%:*}" PGPORT="${HP##*:}" PGUSER PGPASSWORD
set -a; source "$ROOT/.local/env"; set +a
RESTORED=midgard_core_restored_$$
DUMP="$(mktemp)"; trap 'rm -f "$DUMP"; [[ -n "${CORE2:-}" ]] && kill "$CORE2" 2>/dev/null; dropdb --if-exists "$RESTORED" 2>/dev/null || true' EXIT

"$ROOT/scripts/backup.sh" "$DUMP"
"$ROOT/scripts/restore.sh" "$DUMP" "$RESTORED"

PORT=8190
ZITADEL_ISSUER="http://localhost:${PORT_IDP:-9000}" PLATFORM_BIND_ADDR="127.0.0.1:$PORT" PLATFORM_AUDIENCE=midgard-core \
PLATFORM_DATABASE_URL="postgres://midgard_app:$MIDGARD_APP_PASSWORD@$HP/$RESTORED" PLATFORM_OPERATOR_SUBJECTS=operator-1 \
PLATFORM_PUBLISHER_SUBJECTS='publisher-1=platform.' REGISTRY_ENDPOINT_ALLOWED_HOSTS=localhost REGISTRY_HEALTH_CHECK_INTERVAL_SECONDS=2 \
  "$ROOT/target/debug/platform-core" serve >"$ROOT/.local/core-restored.log" 2>&1 &
CORE2=$!
wait_http "http://localhost:$PORT/health/ready" 30

export PLATFORM_URL="http://localhost:$PORT"
OP="$(PLATFORM_URL= token midgard-operator "${OPERATOR_CLIENT_SECRET:-dev-only-operator}")"
apps="$(body_of "$(api GET /v0/applications "$OP" "")")"
[[ "$(jq '.items|length' <<<"$apps")" == 2 ]] || die "restored DB should contain 2 applications"
OBT="$(token obroklik-backend dev-only-obroklik)"
for _ in $(seq 1 20); do   # the restored core re-probes deployment health
  out="$(api POST /v0/contracts/resolve "$OBT" '{"contract":"platform.example","version_constraint":">=1.0.0 <2.0.0"}')"
  [[ "$(status_of "$out")" == 200 ]] && break; sleep 1
done
expect 200 "$out" "resolve from restored registry"
audit="$(body_of "$(api GET '/v0/audit?limit=500' "$OP" "")")"
[[ "$(jq '.items|length' <<<"$audit")" -gt 5 ]] || die "audit trail not restored"
tenants="$(psql -d "$RESTORED" -Atc 'SELECT count(*) FROM tenants')"
[[ "$tenants" == 4 ]] || die "expected 4 tenants after restore, got $tenants"
log "PASS OPS-01: backup restored into a fresh database; apps, tenants, bindings, audit and resolution intact"
