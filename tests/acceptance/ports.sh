#!/usr/bin/env bash
# ENV-05: only the edge publishes a port in the base stack; the dev override binds loopback only.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
[[ -f "$ROOT/.env" ]] || cp "$ROOT/.env.example" "$ROOT/.env"
mkdir -p "$ROOT/deployments/compose/.secrets"; "$ROOT/deployments/compose/gen-dev-secrets.sh" >/dev/null
cfg() { docker compose --env-file "$ROOT/.env" -f "$ROOT/deployments/compose/compose.yaml" "$@" --profile dev-idp --profile demo --profile zitadel config --format json; }
base="$(cfg)"
bad="$(jq -r '.services|to_entries[]|select(.key!="edge" and ((.value.ports//[])|length>0))|.key' <<<"$base")"
[[ -z "$bad" ]] || { echo "services publishing ports in the base stack: $bad" >&2; exit 1; }
jq -e '.services.edge.ports|all(.[]; .host_ip=="127.0.0.1")' <<<"$base" >/dev/null || { echo "edge must bind loopback" >&2; exit 1; }
dev="$(docker compose --env-file "$ROOT/.env" -f "$ROOT/deployments/compose/compose.yaml" -f "$ROOT/deployments/compose/compose.dev.yaml" --profile dev-idp --profile demo --profile zitadel config --format json)"
jq -e '[.services[]|(.ports//[])[]|select(.host_ip!="127.0.0.1")]|length==0' <<<"$dev" >/dev/null || { echo "non-loopback published port" >&2; exit 1; }
! jq -e '.services.postgres.ports' <<<"$dev" >/dev/null || { echo "postgres must not publish ports" >&2; exit 1; }
echo "PASS ENV-05: no database/service ports exposed; published ports are loopback-only"
