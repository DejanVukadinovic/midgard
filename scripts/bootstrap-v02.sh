#!/usr/bin/env bash
# Idempotent v0.2 bootstrap (safe to re-run), on top of scripts/bootstrap.sh. Through the public API only:
#   publish example.echo.submit@1.0.0 (OpenAPI + AsyncAPI) -> approve the echo provider's deployment
#   identity -> declare the requirement for obroklik -> wait for the provider to register its
#   interfaces -> grant + bind obroklik only (fleet stays ungranted).
# Environment: as bootstrap.sh, plus ECHO_PROVIDER_SUBJECT (default example-echo-provider).
source "$(dirname "$0")/lib.sh"
need curl jq

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CONTRACT_TOOL="${CONTRACT_TOOL:-$REPO_ROOT/target/debug/contract-tool}"
: "${OPERATOR_CLIENT_ID:=midgard-operator}" "${PUBLISHER_CLIENT_ID:=midgard-publisher}"
: "${OPERATOR_CLIENT_SECRET:?set OPERATOR_CLIENT_SECRET}" "${PUBLISHER_CLIENT_SECRET:?set PUBLISHER_CLIENT_SECRET}"
: "${ECHO_PROVIDER_SUBJECT:=example-echo-provider}" "${ENVIRONMENT:=local}"
NAME=example.echo.submit

wait_http "${PLATFORM_URL}/health/ready" 120
OP="$(token "$OPERATOR_CLIENT_ID" "$OPERATOR_CLIENT_SECRET")"
PUB="$(token "$PUBLISHER_CLIENT_ID" "$PUBLISHER_CLIENT_SECRET")"

log "v0.2 1/5 publishing $NAME@1.0.0 (immutable; same content is a no-op)"
out="$(api POST /v0/capability-contracts "$PUB" "$("$CONTRACT_TOOL" request-v02 "$REPO_ROOT/contracts/examples/echo")")"
expect '200|201' "$out" "publish $NAME"

log "v0.2 2/5 approving the echo provider's deployment identity"
out="$(api POST /v0/deployment-identities "$OP" "$(jq -nc --arg s "$ECHO_PROVIDER_SUBJECT" '{subject:$s, slug_pattern:"example-echo*"}')")"
expect '201' "$out" "deployment identity"

log "v0.2 3/5 declaring the requirement for obroklik and fleet"
for slug in obroklik fleet; do
  app="$(body_of "$(api GET /v0/applications "$OP" "")" | jq -r --arg s "$slug" '.items[]|select(.slug==$s)|.id')"
  [[ -n "$app" ]] || die "application $slug missing: run scripts/bootstrap.sh first"
  reqs="$(body_of "$(api GET "/v0/applications/$app" "$OP" "")" | jq -c --arg n "$NAME" '[.requirements[]|{contract,version}] | map(select(.contract!=$n)) + [{contract:$n,version:">=1.0.0 <2.0.0"}]')"
  out="$(api PATCH "/v0/applications/$app" "$OP" "$(jq -nc --argjson r "$reqs" '{requirements:$r}')")"
  expect '200' "$out" "requirements $slug"
  [[ "$slug" == obroklik ]] && OBROKLIK_ID="$app"
done

log "v0.2 4/5 waiting for the echo provider to register its interfaces"
for _ in $(seq 1 120); do
  list="$(body_of "$(api GET /v0/implementations "$OP" "")")"
  [[ "$(jq -r '[.items[]|select(.slug=="example-echo")]|length' <<<"$list")" == 1 ]] && break
  sleep 1
done
[[ "$(jq -r '[.items[]|select(.slug=="example-echo")]|length' <<<"$list")" == 1 ]] || die "example-echo never registered"

log "v0.2 5/5 granting + binding obroklik (fleet stays ungranted)"
out="$(api POST "/v0/applications/$OBROKLIK_ID/contract-grants" "$OP" "$(jq -nc --arg n "$NAME" '{contract:$n,permissions:["example.echo:submit"]}')")"
expect '200|201' "$out" "grant"
out="$(api PUT "/v0/applications/$OBROKLIK_ID/bindings/$NAME" "$OP" "$(jq -nc --arg e "$ENVIRONMENT" '{major:1,environment:$e,implementation:"example-echo"}')")"
expect '200' "$out" "bind"
log "v0.2 bootstrap complete"
