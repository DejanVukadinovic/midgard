#!/usr/bin/env bash
# MIDGARD end-to-end acceptance scenario against a RUNNING stack (Compose or scripts/dev-local.sh).
#
# Two synthetic organizations, two applications, one sample implementation:
#   bootstrap (twice) -> authorized resolution + invocation -> denied ungranted app -> wrong audience
#   -> distinct tenants per (org, application) -> spoofing denied -> cross-tenant isolation -> restart.
#
# Required env (see `scripts/dev-local.sh env`): PLATFORM_URL IDP_URL CAPABILITY_URL SAMPLE_APP CONTRACT_TOOL
#   OPERATOR_CLIENT_SECRET PUBLISHER_CLIENT_SECRET
# Optional: RESTART_CMD (command that restarts platform-core; default scripts/dev-local.sh restart-core)
source "$(dirname "$0")/../../scripts/lib.sh"
need curl jq
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
: "${CAPABILITY_URL:?}" "${SAMPLE_APP:?}" "${CONTRACT_TOOL:?}" "${OPERATOR_CLIENT_SECRET:?}" "${PUBLISHER_CLIENT_SECRET:?}"
: "${RESTART_CMD:=$ROOT/scripts/dev-local.sh restart-core}"
export CONTRACT_TOOL REPO_ROOT="$ROOT"

PASS=0
ok() { PASS=$((PASS + 1)); log "PASS  $*"; }

user_token() { # user_token <user> <client_id> <audience>
  curl -fsS -H 'Content-Type: application/json' -d "$(jq -nc --arg u "$1" --arg c "$2" --arg a "$3" '{user:$u,client_id:$c,audience:[$a]}')" "$IDP_URL/dev/user-token" | jq -er .access_token
}
app() { # app <client> <secret> args... -> runs the sample application facade once
  local id="$1" secret="$2"; shift 2
  MIDGARD_REGISTRY_URL="$PLATFORM_URL" MIDGARD_TOKEN_URL="$IDP_TOKEN_URL" MIDGARD_CLIENT_ID="$id" MIDGARD_CLIENT_SECRET="$secret" \
    "$SAMPLE_APP" check "$@"
}

log "== ENV-02/OSS-09: bootstrap twice; second run must not create duplicates"
"$ROOT/scripts/bootstrap.sh"
"$ROOT/scripts/bootstrap.sh"
OP="$(token midgard-operator "$OPERATOR_CLIENT_SECRET")"
apps="$(body_of "$(api GET /v0/applications "$OP" "")")"
[[ "$(jq '.items|length' <<<"$apps")" == 2 ]] || die "expected exactly 2 applications after two bootstraps"
OBR="$(jq -r '.items[]|select(.slug=="obroklik").id' <<<"$apps")"; FLEET="$(jq -r '.items[]|select(.slug=="fleet").id' <<<"$apps")"
ok "bootstrap is idempotent"

log "== wait for the bound deployment to be healthy"
for _ in $(seq 1 60); do
  [[ "$(body_of "$(api GET /v0/implementations "$OP" "")" | jq -r '.items[0].deployments[0].status')" == healthy ]] && break; sleep 1
done
OBT="$(token obroklik-backend dev-only-obroklik)"
FLT="$(token fleet-backend dev-only-fleet)"

log "== RES-01/OSS-05: granted app resolves by contract (no hard-coded URL) and invokes"
out="$(app obroklik-backend dev-only-obroklik)"
[[ "$(jq -r .echo.application <<<"$out")" == obroklik ]] || die "echo failed: $out"
ok "obroklik resolved + invoked platform.example"

log "== RES-02/OSS-12: ungranted app cannot resolve, even knowing the endpoint"
set +e; app fleet-backend dev-only-fleet >/dev/null 2>&1; rc=$?; set -e
[[ $rc == 3 ]] || die "fleet should be denied (exit 3), got $rc"
out="$(api POST /v0/contracts/resolve "$FLT" '{"contract":"platform.example","version_constraint":">=1.0.0 <2.0.0"}')"
expect 403 "$out" "fleet resolve"
FLT_CAP="$(token fleet-backend dev-only-fleet platform.example)"   # valid token for the service's own audience
code="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $FLT_CAP" -H 'Content-Type: application/json' -d '{"message":"x"}' "$CAPABILITY_URL/v1/echo")"
[[ "$code" == 403 ]] || die "direct call by ungranted app must be 403, got $code"
ok "ungranted application denied at registry and at the service"

log "== RES-10: wrong-audience token rejected by the implementation; no token rejected"
code="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $OBT" -H 'Content-Type: application/json' -d '{"message":"x"}' "$CAPABILITY_URL/v1/echo")"
[[ "$code" == 401 ]] || die "core-audience token must be rejected by the capability (401), got $code"
code="$(curl -sS -o /dev/null -w '%{http_code}' -d '{}' -H 'Content-Type: application/json' "$CAPABILITY_URL/v1/echo")"
[[ "$code" == 401 ]] || die "unauthenticated capability call must be 401, got $code"
ok "audience separation enforced"

log "== OSS-13: tenant isolation through real delegated auth (alice@org-acme vs gina@org-globex)"
ALICE="$(user_token alice obroklik-web prj-obroklik)"; GINA="$(user_token gina obroklik-web prj-obroklik)"
app obroklik-backend dev-only-obroklik --user-token "$ALICE" >/dev/null
out_g="$(app obroklik-backend dev-only-obroklik --user-token "$GINA")"
notes_g="$(jq -c '[.notes[].body]' <<<"$out_g")"
out_a="$(app obroklik-backend dev-only-obroklik --user-token "$ALICE")"
tenant_a="$(body_of "$(api POST /v0/tenants/resolve "$OBT" "$(jq -nc --arg t "$ALICE" '{user_token:$t}')")" | jq -r .tenant_id)"
tenant_g="$(body_of "$(api POST /v0/tenants/resolve "$OBT" "$(jq -nc --arg t "$GINA" '{user_token:$t}')")" | jq -r .tenant_id)"
[[ -n "$tenant_a" && "$tenant_a" != null && "$tenant_a" != "$tenant_g" ]] || die "organizations must map to distinct tenants"
[[ "$(jq '.notes|length' <<<"$out_a")" -ge 2 ]] || die "alice should see her notes: $out_a"
[[ "$(jq '.notes|length' <<<"$out_g")" == 1 ]] || die "gina must see only her own tenant's notes: $notes_g"
ok "same application, two organizations -> separate tenants and separate data"

log "== IAM-05/OSS-13: spoofed organization / tenant header is ignored or rejected"
out="$(api POST /v0/tenants/resolve "$OBT" "$(jq -nc --arg t "$GINA" '{user_token:$t,organization_id:"org-acme"}')")"
expect 403 "$out" "gina selecting org-acme"
out="$(api POST /v0/tenants/resolve "$OBT" "$(jq -nc --arg t "$ALICE" '{user_token:$t,organization_id:"org-globex"}')")"
expect 403 "$out" "alice selecting org-globex for obroklik"
code="$(curl -sS -o /tmp/acc-spoof.json -w '%{http_code}' -H "Authorization: Bearer $(token obroklik-backend dev-only-obroklik platform.example)" \
  -H "X-Delegated-User-Token: $GINA" -H "X-Tenant-Id: $tenant_a" -H "X-Organization-Id: org-acme" "$CAPABILITY_URL/v1/notes")"
[[ "$code" == 403 ]] || die "spoofed org header must be rejected (403), got $code"
ok "spoofed organization/tenant selection denied"

log "== IAM-03: one organization, two applications -> distinct tenants"
ALICE_FLEET="$(user_token alice fleet-web prj-fleet)"
tf="$(body_of "$(api POST /v0/tenants/resolve "$FLT" "$(jq -nc --arg t "$ALICE_FLEET" '{user_token:$t}')")" | jq -r .tenant_id)"
[[ "$tf" != null && "$tf" != "$tenant_a" ]] || die "org-acme must have distinct tenants per application"
tfb="$(body_of "$(api POST /v0/tenants/resolve "$FLT" "$(jq -nc --arg t "$ALICE_FLEET" '{user_token:$t,organization_id:"org-globex"}')")" | jq -r .tenant_id)"
[[ "$tfb" != null && "$tfb" != "$tf" ]] || die "explicit extra-org grant must select a different tenant"
ok "distinct tenants per (organization, application)"

log "== OSS-11: unauthorized registration"
for who in "$OBT" "$FLT"; do
  out="$(api POST /v0/applications "$who" '{"slug":"evil","display_name":"E"}' -H 'Idempotency-Key: evil-1')"; expect 403 "$out" "app registering app"
  out="$(api POST "/v0/applications/$FLEET/contract-grants" "$who" '{"contract":"platform.example","permissions":["example:read"]}')"; expect 403 "$out" "self-grant"
done
out="$(api POST /v0/applications "" '{}' )"; expect 401 "$out" "anonymous registration"
ok "registration/grant/binding APIs refuse non-operators"

log "== OSS-15/ENV-03: restart core; registrations, bindings and data survive"
before="$(app obroklik-backend dev-only-obroklik --user-token "$ALICE" | jq '.notes|length')"
eval "$RESTART_CMD" >/dev/null
sleep 3
after="$(app obroklik-backend dev-only-obroklik --user-token "$ALICE" | jq '.notes|length')"
[[ "$after" == $((before + 1)) ]] || die "after restart expected $((before + 1)) notes, got $after (before=$before)"
"$ROOT/scripts/bootstrap.sh"   # rerun after restart is still a no-op
[[ "$(jq '.items|length' <<<"$(body_of "$(api GET /v0/applications "$(token midgard-operator "$OPERATOR_CLIENT_SECRET")" "")")")" == 2 ]] || die "duplicate apps after rerun"
ok "core restart + bootstrap rerun preserve state"

log "== SEC: resolver output contains logical names only; secrets not leaked in core logs"
res="$(body_of "$(api POST /v0/contracts/resolve "$OBT" '{"contract":"platform.example","version_constraint":">=1.0.0 <2.0.0"}')")"
[[ "$(jq -r .endpoint <<<"$res")" != *"172."* ]] || die "container IP in resolver output"
if [[ -d "$ROOT/.local" ]]; then
  ! grep -rqE 'dev-only-|eyJ[A-Za-z0-9_-]{20,}' "$ROOT/.local"/platform-core.log || die "credentials/tokens found in core logs"
fi
ok "no container IPs or credentials leaked"

log "ACCEPTANCE PASSED ($PASS checks)"
