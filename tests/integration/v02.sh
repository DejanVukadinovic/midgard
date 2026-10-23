#!/usr/bin/env bash
# MIDGARD v0.2 end-to-end acceptance against a RUNNING stack (Compose `integration-tests` service
# or scripts/dev-local.sh with WITH_V02=1). Exercises the multi-protocol contract path:
# publish -> register interfaces -> resolve -> HTTP + queue invocation -> authorization at the
# provider and at the broker -> idempotency -> poison isolation -> revocation -> bootstrap reruns.
#
# Env: PLATFORM_URL IDP_URL PROVIDER_URL ECHO_BIN CONTRACT_TOOL OPERATOR_CLIENT_SECRET PUBLISHER_CLIENT_SECRET
#      ECHO_NATS_URL, NATS_CLIENT_PASSWORD (obroklik's broker credentials), NATS_FLEET_PASSWORD optional
source "$(dirname "$0")/../../scripts/lib.sh"
need curl jq
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
: "${PROVIDER_URL:?}" "${ECHO_BIN:?}" "${CONTRACT_TOOL:?}" "${OPERATOR_CLIENT_SECRET:?}" "${PUBLISHER_CLIENT_SECRET:?}"
export CONTRACT_TOOL REPO_ROOT="$ROOT"
PASS=0
ok() { PASS=$((PASS + 1)); log "PASS  $*"; }
RUN="$(date +%s)-$$"

export ECHO_CORE_URL="$PLATFORM_URL" ECHO_TOKEN_URL="$IDP_TOKEN_URL" ECHO_NATS_URL="${ECHO_NATS_URL:?}"
obroklik() { ECHO_CLIENT_ID=obroklik-backend ECHO_CLIENT_SECRET=dev-only-obroklik ECHO_APP=obroklik ECHO_NATS_USER=client-obroklik ECHO_NATS_PASSWORD="${NATS_PW_OVERRIDE:-${NATS_CLIENT_PASSWORD:?}}" "$ECHO_BIN" client "$@"; }
fleet()    { ECHO_CLIENT_ID=fleet-backend ECHO_CLIENT_SECRET=dev-only-fleet ECHO_APP=fleet ECHO_NATS_USER=client-obroklik ECHO_NATS_PASSWORD="${NATS_CLIENT_PASSWORD:?}" "$ECHO_BIN" client "$@"; }
rc() { set +e; "$@" >/tmp/v02-out.$$ 2>&1; local r=$?; set -e; cat /tmp/v02-out.$$ >&2; echo "$r"; }
processed() { curl -fsS "$PROVIDER_URL/debug/processed"; }
wait_processed() { # wait_processed <desc> [jq args...] <jq filter>
  local desc="$1"; shift
  for _ in $(seq 1 40); do processed | jq -e "$@" >/dev/null 2>&1 && return 0; sleep 0.5; done
  die "$desc (state: $(processed))"
}
user_token() { curl -fsS -H 'Content-Type: application/json' -d "$(jq -nc --arg u "$1" --arg c "$2" --arg a "$3" '{user:$u,client_id:$c,audience:[$a]}')" "$IDP_URL/dev/user-token" | jq -er .access_token; }

log "== CON/COMPOSE: v0.2 bootstrap twice; reruns must not duplicate records"
"$ROOT/scripts/bootstrap-v02.sh"
"$ROOT/scripts/bootstrap-v02.sh"
OP="$(token midgard-operator "$OPERATOR_CLIENT_SECRET")"
n="$(body_of "$(api GET /v0/implementations "$OP" "")" | jq '[.items[]|select(.slug=="example-echo")]|length')"
[[ "$n" == 1 ]] || die "expected one example-echo implementation, got $n"
ok "bootstrap is idempotent"

log "== CON: published contract is retrievable with both interface documents; mutation is refused"
V="$(body_of "$(api GET /v0/contracts/example.echo.submit/versions/1.0.0 "$OP" "")")"
[[ "$(jq '.manifest.interfaces|map(.protocol)|sort' -c <<<"$V")" == '["http","mq"]' ]] || die "interfaces missing: $V"
body="$("$CONTRACT_TOOL" request-v02 "$ROOT/contracts/examples/echo" | jq -c '.documents["openapi.yaml"] |= sub("Synthetic Echo API";"Tampered") | del(.digest)')"
out="$(api POST /v0/capability-contracts "$(token midgard-publisher "$PUBLISHER_CLIENT_SECRET")" "$body")"
expect 409 "$out" "mutated published contract"
ok "published contract immutable"

log "== REG: one capability resolves to HTTP and MQ interfaces for the bound application"
R="$(obroklik resolve)"
[[ "$(jq -c '[.interfaces[].id]|sort' <<<"$R")" == '["queue","rest"]' ]] || die "expected rest+queue: $R"
[[ "$(jq -r '.interfaces[]|select(.id=="queue").destination' <<<"$R")" == example.echo.submit ]] || die "queue destination: $R"
! grep -qiE 'password|secret|token' <<<"$R" || die "resolved contract leaks credentials"
ok "multi-interface resolution without credentials"

log "== HTTP: resolve -> invoke (202 accepted, idempotent), wrong media type / payload rejected"
obroklik submit-http --id "h-$RUN" --value v1 >/dev/null
obroklik submit-http --id "h-$RUN" --value v1 >/dev/null     # retry with same id
wait_processed "http submission not recorded exactly once" --arg i "h-$RUN" '[.items[]|select(.id==$i and .via=="rest" and .application=="obroklik")]|length==1'
[[ "$(rc obroklik submit-http --id x --content-type text/plain)" != 0 ]] || die "wrong media type accepted"
[[ "$(rc obroklik submit-http --body '{"id":""}')" != 0 ]] || die "invalid payload accepted"
ok "HTTP path"

log "== IAM: ungranted app denied at resolution and by the provider even with a valid audience token"
[[ "$(rc fleet resolve)" == 3 ]] || die "fleet resolve must be denied"
FT="$(ECHO_CLIENT_ID=fleet-backend; token fleet-backend dev-only-fleet example.echo)"
code="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $FT" -H 'Content-Type: application/json' -d '{"id":"f1","value":"x"}' "$PROVIDER_URL/submit")"
[[ "$code" == 403 ]] || die "provider must refuse the ungranted app (403), got $code"
code="$(curl -sS -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -d '{"id":"f1","value":"x"}' "$PROVIDER_URL/submit")"
[[ "$code" == 401 ]] || die "unauthenticated call must be 401, got $code"
OBT_CORE="$(token obroklik-backend dev-only-obroklik)"   # wrong audience for the provider
code="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $OBT_CORE" -H 'Content-Type: application/json' -d '{"id":"f1","value":"x"}' "$PROVIDER_URL/submit")"
[[ "$code" == 401 ]] || die "wrong-audience token must be 401, got $code"
ok "provider-side enforcement"

log "== IAM: tenant context only from verified user tokens; cross-tenant denied at the provider"
ALICE="$(user_token alice obroklik-web prj-obroklik)"
obroklik submit-http --id "t-$RUN" --user-token "$ALICE" --organization org-acme >/dev/null
wait_processed "delegated submission has no tenant" --arg i "t-$RUN" '[.items[]|select(.id==$i and (.tenant|length)>0)]|length==1'
[[ "$(rc obroklik submit-http --id "x-$RUN" --user-token "$ALICE" --organization org-globex)" == 3 ]] || die "alice must not act in org-globex"
ok "delegated user + tenant enforcement"

log "== MQ: publish -> durable consume -> idempotent (dedupe by message id)"
out="$(obroklik submit-queue --id "q-$RUN" --value v2)"
wait_processed "queue message not processed" --arg i "q-$RUN" '[.items[]|select(.id==$i and .via=="queue" and .application=="obroklik")]|length==1'
out="$(obroklik submit-queue --id "q-$RUN" --value v2)"
grep -q 'duplicate=true' <<<"$out" || die "broker must de-duplicate by message id: $out"
ok "MQ delivery + de-duplication"

log "== MQ: direct unauthorized broker access is denied by the broker itself"
[[ "$(rc obroklik publish-raw --subject example.echo.submit.fleet --id spoof-$RUN)" == 3 ]] || die "client published as another application"
[[ "$(rc obroklik publish-raw --subject example.echo.dead.submit --id dl-$RUN)" == 3 ]] || die "client published to the dead-letter subject"
[[ "$(NATS_PW_OVERRIDE=wrong rc obroklik publish-raw --subject example.echo.submit.obroklik --id anon-$RUN)" == 3 ]] || die "wrong broker password accepted"
sleep 1
processed | jq -e --arg i "spoof-$RUN" '[.items[]|select(.id==$i)]|length==0' >/dev/null || die "spoofed message was processed"
ok "broker ACLs enforced"

log "== MQ: poison message is isolated (bounded redelivery, dead-letter), valid traffic continues"
obroklik publish-raw --subject example.echo.submit.obroklik --id "poison-$RUN" --body 'not json' >/dev/null
obroklik submit-queue --id "after-poison-$RUN" >/dev/null
wait_processed "valid message blocked behind poison" --arg i "after-poison-$RUN" '[.items[]|select(.id==$i)]|length==1'
processed | jq -e '.dead|length>=1' >/dev/null || die "poison message was not dead-lettered"
ok "poison isolation"

log "== IAM: revoked grant -> resolution and provider invocation fail"
OBR="$(body_of "$(api GET /v0/applications "$OP" "")" | jq -r '.items[]|select(.slug=="obroklik").id')"
gid="$(body_of "$(api GET "/v0/applications/$OBR/contract-grants" "$OP" "")" | jq -r '.items[]|select(.contract=="example.echo.submit" and .status=="active").id')"
expect '200|204' "$(api DELETE "/v0/applications/$OBR/contract-grants/$gid" "$OP" "")" "revoke"
[[ "$(rc obroklik resolve)" == 3 ]] || die "resolution must fail after revoke"
OBT_PROV="$(token obroklik-backend dev-only-obroklik example.echo)"
code="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $OBT_PROV" -H 'Content-Type: application/json' -d "{\"id\":\"late-$RUN\",\"value\":\"x\"}" "$PROVIDER_URL/submit")"
[[ "$code" == 403 ]] || die "provider must refuse a revoked app even with a previously resolved endpoint, got $code"
obroklik publish-raw --subject example.echo.submit.obroklik --id "late-q-$RUN" >/dev/null   # broker still accepts (ACL is identity based) ...
sleep 2
processed | jq -e --arg i "late-q-$RUN" '[.items[]|select(.id==$i)]|length==0' >/dev/null || die "... but the provider must not process it after revoke"
ok "revocation takes effect at the provider"
log "restoring grant"; "$ROOT/scripts/bootstrap-v02.sh"

log "V0.2 INTEGRATION PASSED ($PASS checks)"
