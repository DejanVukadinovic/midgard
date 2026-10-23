#!/usr/bin/env bash
# Idempotent ZITADEL provisioning for the MIDGARD demo (safe to re-run; lookup-before-create everywhere).
# Uses the first-instance admin machine user's PAT (written by ZITADEL's FirstInstance setup).
#
# Creates: OIDC token settings, projects (midgard-core, platform-example, example-echo, obroklik, fleet) with roles,
#          two customer organizations + project grants, machine users (operator, publisher, deployers,
#          backend clients), human users with grants, OIDC web apps (PKCE) and enables login v2 so
#          authorization requests can be completed headlessly through the session API (tests only).
# Writes, into $OUT_DIR (a volume shared with the services): idp.env (non-secret ids, audiences, issuer)
#          and secrets.env (client secrets, dev user passwords; mode 0600). Re-runs keep existing secrets.
#
# Env: ZITADEL_URL (default http://localhost:8085)   ZITADEL_HOST (Host header, for in-network access when the
#      issuer's external domain differs from the URL used here)   ZITADEL_PAT_FILE   OUT_DIR
source "$(dirname "$0")/lib.sh"
need curl jq

: "${ZITADEL_URL:=http://localhost:8085}" "${OUT_DIR:=.local/idp}"
: "${ZITADEL_PAT_FILE:?set ZITADEL_PAT_FILE}"
mkdir -p "$OUT_DIR"; umask 077
SECRETS="$OUT_DIR/secrets.env"; touch "$SECRETS"

HOSTH=(); [[ -n "${ZITADEL_HOST:-}" ]] && HOSTH=(-H "Host: $ZITADEL_HOST")
z() { # z <method> <path> [json] [org id]  -> body on stdout; dies on HTTP >= 400 except 409 (returns empty)
  local m="$1" p="$2" body="${3:-}" org="${4:-}" tmp code
  local args=(-sS -X "$m" "${HOSTH[@]}" -H "Authorization: Bearer $(<"$ZITADEL_PAT_FILE")" -H 'Content-Type: application/json' -w '\n%{http_code}')
  [[ -n "$body" ]] && args+=(--data "$body")
  [[ -n "$org" ]] && args+=(-H "x-zitadel-orgid: $org")
  tmp="$(curl "${args[@]}" "$ZITADEL_URL$p")"; code="${tmp##*$'\n'}"
  if [[ "$code" == 409 ]] || grep -q 'already exists\|AlreadyExists\|COMMAND-.*exists\|No changes' <<<"$tmp"; then return 0; fi
  [[ "$code" -lt 400 ]] || die "ZITADEL $m $p -> HTTP $code: ${tmp%$'\n'*}"
  printf '%s' "${tmp%$'\n'*}"
}
secret_get() { grep -E "^$1=" "$SECRETS" | head -1 | cut -d= -f2- || true; }
secret_put() { grep -vE "^$1=" "$SECRETS" >"$SECRETS.tmp" || true; echo "$1=$2" >>"$SECRETS.tmp"; mv "$SECRETS.tmp" "$SECRETS"; }
rand() { head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n'; }

log "waiting for ZITADEL at $ZITADEL_URL"
for _ in $(seq 1 120); do curl -fsS "${HOSTH[@]}" -o /dev/null "$ZITADEL_URL/debug/ready" 2>/dev/null && break; sleep 2; done
curl -fsS "${HOSTH[@]}" -o /dev/null "$ZITADEL_URL/debug/ready" || die "ZITADEL not ready"
for _ in $(seq 1 60); do [[ -s "$ZITADEL_PAT_FILE" ]] && break; sleep 1; done
[[ -s "$ZITADEL_PAT_FILE" ]] || die "admin PAT file missing/empty: $ZITADEL_PAT_FILE"
# /debug/ready can precede the API gateway: wait until an authenticated admin call works.
for _ in $(seq 1 60); do (z GET /admin/v1/healthz) >/dev/null 2>&1 && break; sleep 2; done

# ---- instance settings ------------------------------------------------------------------------
log "1/7 instance settings (1h access tokens, login v2 for headless sessions)"
oidc="$(z GET /admin/v1/settings/oidc | jq -c '.settings|{accessTokenLifetime:"3600s",idTokenLifetime:"3600s",refreshTokenIdleExpiration,refreshTokenExpiration}')"
z PUT /admin/v1/settings/oidc "$oidc" >/dev/null
z PUT /v2/features/instance '{"loginV2":{"required":true,"baseUri":"http://localhost:3000/ui/v2/login"}}' >/dev/null

org_id() { z POST /admin/v1/orgs/_search "$(jq -nc --arg n "$1" '{queries:[{nameQuery:{name:$n,method:"TEXT_QUERY_METHOD_EQUALS"}}]}')" | jq -r '.result[0].id // empty'; }
ensure_org() { local id; id="$(org_id "$1")"; if [[ -z "$id" ]]; then z POST /management/v1/orgs "$(jq -nc --arg n "$1" '{name:$n}')" >/dev/null; id="$(org_id "$1")"; fi; echo "$id"; }
project_id() { z POST /management/v1/projects/_search "$(jq -nc --arg n "$1" '{queries:[{nameQuery:{name:$n,method:"TEXT_QUERY_METHOD_EQUALS"}}]}')" | jq -r '.result[0].id // empty'; }
ensure_project() { # name -> id  (role assertion on so tokens carry role claims)
  local id; id="$(project_id "$1")"
  if [[ -z "$id" ]]; then z POST /management/v1/projects "$(jq -nc --arg n "$1" '{name:$n,projectRoleAssertion:true,projectRoleCheck:false}')" >/dev/null; id="$(project_id "$1")"; fi
  echo "$id"
}

log "2/7 projects, roles, customer organizations, project grants"
CORE_PID="$(ensure_project midgard-core)"; CAP_PID="$(ensure_project platform-example)"; ECHO_PID="$(ensure_project example-echo)"
OBR_PID="$(ensure_project obroklik)"; FLEET_PID="$(ensure_project fleet)"
z POST "/management/v1/projects/$OBR_PID/roles" '{"roleKey":"member","displayName":"member"}' >/dev/null
z POST "/management/v1/projects/$FLEET_PID/roles" '{"roleKey":"viewer","displayName":"viewer"}' >/dev/null
ORG_A="$(ensure_org org-acme)"; ORG_B="$(ensure_org org-globex)"
grant_id() { z POST "/management/v1/projects/$1/grants/_search" '{}' | jq -r --arg o "$2" '[.result[]?|select(.grantedOrgId==$o)][0].grantId // empty'; }
ensure_grant() { # project org role -> grant id
  local g; g="$(grant_id "$1" "$2")"
  if [[ -z "$g" ]]; then z POST "/management/v1/projects/$1/grants" "$(jq -nc --arg o "$2" --arg r "$3" '{grantedOrgId:$o,roleKeys:[$r]}')" >/dev/null; g="$(grant_id "$1" "$2")"; fi
  echo "$g"
}
G_OBR_A="$(ensure_grant "$OBR_PID" "$ORG_A" member)"; G_OBR_B="$(ensure_grant "$OBR_PID" "$ORG_B" member)"
G_FL_A="$(ensure_grant "$FLEET_PID" "$ORG_A" viewer)"; G_FL_B="$(ensure_grant "$FLEET_PID" "$ORG_B" viewer)"

log "3/7 machine users (client credentials, JWT access tokens)"
machine() { # username -> user id; sets/keeps secret
  local id; id="$(z POST /management/v1/users/_search "$(jq -nc --arg n "$1" '{queries:[{userNameQuery:{userName:$n,method:"TEXT_QUERY_METHOD_EQUALS"}}]}')" | jq -r '.result[0].id // empty')"
  if [[ -z "$id" ]]; then
    id="$(z POST /management/v1/users/machine "$(jq -nc --arg n "$1" '{userName:$n,name:$n,accessTokenType:"ACCESS_TOKEN_TYPE_JWT"}')" | jq -r .userId)"
  fi
  local key; key="SECRET_$(tr 'a-z-' 'A-Z_' <<<"$1")"
  if [[ -z "$(secret_get "$key")" ]]; then secret_put "$key" "$(z PUT "/management/v1/users/$id/secret" '{}' | jq -r .clientSecret)"; fi
  echo "$id"
}
OPERATOR_ID="$(machine midgard-operator)"; PUBLISHER_ID="$(machine midgard-publisher)"
DEPLOYER_ID="$(machine example-deployer)"; ECHO_PROVIDER_ID="$(machine echo-provider)"
machine obroklik-backend >/dev/null; machine fleet-backend >/dev/null

log "4/7 human users and grants"
human() { # username org -> user id
  local id; id="$(z POST /v2/users "$(jq -nc --arg n "$1" '{queries:[{userNameQuery:{userName:$n,method:"TEXT_QUERY_METHOD_EQUALS"}}]}')" "$2" | jq -r '.result[0].userId // empty')"
  if [[ -z "$id" ]]; then
    local pw; pw="$(secret_get "PASSWORD_$(tr 'a-z' 'A-Z' <<<"$1")")"; [[ -n "$pw" ]] || { pw="Aa1!$(rand)"; secret_put "PASSWORD_$(tr 'a-z' 'A-Z' <<<"$1")" "$pw"; }
    id="$(z POST /management/v1/users/human/_import "$(jq -nc --arg n "$1" --arg p "$pw" '{userName:$n,profile:{firstName:$n,lastName:"Demo"},email:{email:($n+"@demo.test"),isEmailVerified:true},password:$p,passwordChangeRequired:false}')" "$2" | jq -r .userId)"
  fi
  echo "$id"
}
ensure_user_grant() { # user org project projectGrantId role
  local have; have="$(z POST /management/v1/users/grants/_search "$(jq -nc --arg u "$1" --arg p "$3" '{queries:[{userIdQuery:{userId:$u}},{projectIdQuery:{projectId:$p}}]}')" "$2" | jq -r '.result|length')"
  [[ "$have" != 0 ]] || z POST "/management/v1/users/$1/grants" "$(jq -nc --arg p "$3" --arg g "$4" --arg r "$5" '{projectId:$p,projectGrantId:$g,roleKeys:[$r]}')" "$2" >/dev/null
}
ALICE_ID="$(human alice "$ORG_A")"; GINA_ID="$(human gina "$ORG_B")"
ensure_user_grant "$ALICE_ID" "$ORG_A" "$OBR_PID" "$G_OBR_A" member
ensure_user_grant "$GINA_ID" "$ORG_B" "$OBR_PID" "$G_OBR_B" member
ensure_user_grant "$ALICE_ID" "$ORG_A" "$FLEET_PID" "$G_FL_A" viewer
ensure_user_grant "$GINA_ID" "$ORG_B" "$FLEET_PID" "$G_FL_B" viewer
# Extra-organization grant: a home-org-acme user who may also act in globex's Fleet tenant (explicit selection required).
ensure_user_grant "$ALICE_ID" "$ORG_B" "$FLEET_PID" "$G_FL_B" viewer

# Test-only "login client" (what ZITADEL's login v2 UI uses): may create sessions and finalize auth requests.
LOGIN_ID="$(machine login-client)"
z POST /admin/v1/members "$(jq -nc --arg u "$LOGIN_ID" '{userId:$u,roles:["IAM_LOGIN_CLIENT"]}')" >/dev/null
if [[ -z "$(secret_get LOGIN_CLIENT_PAT)" ]]; then
  secret_put LOGIN_CLIENT_PAT "$(z POST "/management/v1/users/$LOGIN_ID/pats" '{"expirationDate":"2099-01-01T00:00:00Z"}' | jq -r .token)"
fi

log "5/7 OIDC web applications (authorization code + PKCE, JWT access tokens, role assertion)"
oidc_app() { # project name redirect -> client id
  local cid; cid="$(z POST "/management/v1/projects/$1/apps/_search" "$(jq -nc --arg n "$2" '{queries:[{nameQuery:{name:$n,method:"TEXT_QUERY_METHOD_EQUALS"}}]}')" | jq -r '.result[0].oidcConfig.clientId // empty')"
  if [[ -z "$cid" ]]; then
    cid="$(z POST "/management/v1/projects/$1/apps/oidc" "$(jq -nc --arg n "$2" --arg r "$3" '{name:$n,redirectUris:[$r],responseTypes:["OIDC_RESPONSE_TYPE_CODE"],grantTypes:["OIDC_GRANT_TYPE_AUTHORIZATION_CODE"],appType:"OIDC_APP_TYPE_WEB",authMethodType:"OIDC_AUTH_METHOD_TYPE_NONE",accessTokenType:"OIDC_TOKEN_TYPE_JWT",accessTokenRoleAssertion:true,idTokenRoleAssertion:true,devMode:true}')" | jq -r .clientId)"
  fi
  echo "$cid"
}
OBR_WEB="$(oidc_app "$OBR_PID" obroklik-web https://obroklik.example.test/auth/callback)"
FLEET_WEB="$(oidc_app "$FLEET_PID" fleet-web https://fleet.example.test/auth/callback)"

log "6/7 writing $OUT_DIR/idp.env"
ISSUER="$(curl -fsS "${HOSTH[@]}" "$ZITADEL_URL/.well-known/openid-configuration" | jq -r .issuer)"
cat >"$OUT_DIR/idp.env.tmp" <<ENV
IDP_KIND=zitadel
IDP_ISSUER=$ISSUER
IDP_TOKEN_URL=$ISSUER/oauth/v2/token
IDP_JWKS_URL=$ISSUER/oauth/v2/keys
PLATFORM_AUDIENCE=$CORE_PID
CAPABILITY_AUDIENCE=$CAP_PID
ECHO_AUDIENCE=$ECHO_PID
PLATFORM_OPERATOR_SUBJECTS=$OPERATOR_ID
PLATFORM_PUBLISHER_SUBJECTS='$PUBLISHER_ID=platform.|example.|examples.'
OPERATOR_CLIENT_ID=midgard-operator
PUBLISHER_CLIENT_ID=midgard-publisher
DEPLOYER_SUBJECT=$DEPLOYER_ID
ECHO_PROVIDER_SUBJECT=$ECHO_PROVIDER_ID
OBROKLIK_PROJECT=$OBR_PID
OBROKLIK_WEB_CLIENT=$OBR_WEB
OBROKLIK_BACKEND_CLIENT=obroklik-backend
FLEET_PROJECT=$FLEET_PID
FLEET_WEB_CLIENT=$FLEET_WEB
FLEET_BACKEND_CLIENT=fleet-backend
ORG_A=$ORG_A
ORG_B=$ORG_B
ALICE_ID=$ALICE_ID
GINA_ID=$GINA_ID
ENV
mv "$OUT_DIR/idp.env.tmp" "$OUT_DIR/idp.env"

log "7/7 verifying: operator token carries the core audience"
tok="$(curl -fsS -u "midgard-operator:$(secret_get SECRET_MIDGARD_OPERATOR)" "${HOSTH[@]}" -d grant_type=client_credentials \
  --data-urlencode "scope=openid urn:zitadel:iam:org:project:id:$CORE_PID:aud" "$ZITADEL_URL/oauth/v2/token" | jq -er .access_token)"
jq -e --arg a "$CORE_PID" --arg s "$OPERATOR_ID" '.aud|index($a)!=null' <<<"$(cut -d. -f2 <<<"$tok" | tr '_-' '/+' | base64 -d 2>/dev/null)" >/dev/null || die "operator token lacks the core audience"
log "ZITADEL bootstrap complete (ids in $OUT_DIR/idp.env, secrets in $SECRETS)"
