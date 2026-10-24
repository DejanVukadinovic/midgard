#!/usr/bin/env bash
# Idempotent MIDGARD demo bootstrap (safe to re-run). Registers, through the public API only:
#   contracts -> deployment identity -> applications (obroklik, fleet) -> tenants
#   -> grant + binding for obroklik ONLY (fleet is deliberately left ungranted).
# Environment: PLATFORM_URL, IDP_TOKEN_URL, OPERATOR_CLIENT_ID/SECRET, PUBLISHER_CLIENT_ID/SECRET,
#              CONTRACT_TOOL (path to the contract-tool binary), REPO_ROOT.
# No secrets are printed; secrets come from the environment / ignored local files only.
source "$(dirname "$0")/lib.sh"
need curl jq

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CONTRACT_TOOL="${CONTRACT_TOOL:-$REPO_ROOT/target/debug/contract-tool}"
: "${OPERATOR_CLIENT_ID:=midgard-operator}"
: "${OPERATOR_CLIENT_SECRET:=$(csecret "$OPERATOR_CLIENT_ID")}"
: "${PUBLISHER_CLIENT_ID:=midgard-publisher}"
: "${PUBLISHER_CLIENT_SECRET:=$(csecret "$PUBLISHER_CLIENT_ID")}"
: "${DEPLOYER_SUBJECT:=example-deployer}"
: "${ENVIRONMENT:=local}"
# Identity-provider objects the demo applications are linked to (link provisioner).
: "${OBROKLIK_PROJECT:=prj-obroklik}" "${OBROKLIK_WEB_CLIENT:=obroklik-web}" "${OBROKLIK_BACKEND_CLIENT:=obroklik-backend}"
: "${FLEET_PROJECT:=prj-fleet}" "${FLEET_WEB_CLIENT:=fleet-web}" "${FLEET_BACKEND_CLIENT:=fleet-backend}"
: "${ORG_A:=org-acme}" "${ORG_B:=org-globex}"

wait_http "${PLATFORM_URL}/health/ready" 120
OP="$(token "$OPERATOR_CLIENT_ID" "$OPERATOR_CLIENT_SECRET")"
PUB="$(token "$PUBLISHER_CLIENT_ID" "$PUBLISHER_CLIENT_SECRET")"

log "1/6 publishing contracts (immutable; same digest is a no-op)"
for v in 1.0.0 1.1.0; do
  out="$(api POST /v0/contracts "$PUB" "$("$CONTRACT_TOOL" request "$REPO_ROOT/contracts/platform/example/v1/$v")")"
  expect '200|201' "$out" "publish platform.example@$v"
done

log "2/6 approving the example service's deployment identity"
out="$(api POST /v0/deployment-identities "$OP" "$(jq -nc --arg s "$DEPLOYER_SUBJECT" '{subject:$s, slug_pattern:"example-*"}')")"
expect '201' "$out" "deployment identity"

register_app() { # slug project web backend
  local body out
  body="$(jq -nc --arg slug "$1" --arg p "$2" --arg web "$3" --arg be "$4" '{
    slug:$slug, display_name:($slug|ascii_upcase), zitadel_project_id:$p,
    oauth_clients:[{kind:"web",redirect_uris:["https://\($slug).example.test/auth/callback"],zitadel_client_id:$web},{kind:"backend",zitadel_client_id:$be}],
    requirements:[{contract:"platform.example",version:">=1.0.0 <2.0.0"}]}')"
  out="$(api POST /v0/applications "$OP" "$body" -H "Idempotency-Key: bootstrap-app-$1-v1")"
  expect '200|201' "$out" "register $1"
  body_of "$out" | jq -r .application.id
}
log "3/6 registering applications"
OBROKLIK_ID="$(register_app obroklik "$OBROKLIK_PROJECT" "$OBROKLIK_WEB_CLIENT" "$OBROKLIK_BACKEND_CLIENT")"
FLEET_ID="$(register_app fleet "$FLEET_PROJECT" "$FLEET_WEB_CLIENT" "$FLEET_BACKEND_CLIENT")"

log "4/6 provisioning tenants (organization x application)"
for pair in "$ORG_A:$OBROKLIK_ID" "$ORG_B:$OBROKLIK_ID" "$ORG_A:$FLEET_ID" "$ORG_B:$FLEET_ID"; do
  out="$(api POST "/v0/organizations/${pair%%:*}/applications/${pair##*:}/tenants" "$OP" "")"
  expect '200|201' "$out" "tenant $pair"
done

log "5/6 waiting for the example implementation to self-register"
for _ in $(seq 1 120); do
  list="$(body_of "$(api GET /v0/implementations "$OP" "")")"
  [[ "$(jq -r '[.items[]|select(.slug=="example-capability")]|length' <<<"$list")" == 1 ]] && break
  sleep 1
done
[[ "$(jq -r '[.items[]|select(.slug=="example-capability")]|length' <<<"$list")" == 1 ]] || die "example-capability never registered"

log "6/6 granting + binding obroklik (fleet stays ungranted)"
out="$(api POST "/v0/applications/$OBROKLIK_ID/contract-grants" "$OP" '{"contract":"platform.example","permissions":["example:read","example:write"]}')"
expect '200|201' "$out" "grant"
out="$(api PUT "/v0/applications/$OBROKLIK_ID/bindings/platform.example" "$OP" "$(jq -nc --arg e "$ENVIRONMENT" '{major:1,environment:$e,implementation:"example-capability"}')")"
expect '200' "$out" "bind"
log "bootstrap complete: obroklik=$OBROKLIK_ID fleet=$FLEET_ID"
