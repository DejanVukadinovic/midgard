#!/usr/bin/env bash
# Headless end-user token from ZITADEL for tests: authorization code + PKCE completed through the
# session API (requires the instance's login-v2 flag, set by zitadel-bootstrap.sh). Real users, real tokens;
# no browser. Usage: zitadel-user-token.sh <username> <web-client-id> <project-id> [redirect-uri]
# Env: ZITADEL_URL, ZITADEL_HOST (optional Host header), OUT_DIR (holds secrets.env: login-client PAT and the user's password)
source "$(dirname "$0")/lib.sh"
need curl jq openssl
user="$1" cid="$2" pid="$3"
# Redirect URIs registered by zitadel-bootstrap.sh (only used to complete the code flow; nothing listens there).
if [[ "$cid" == "${FLEET_WEB_CLIENT:-}" ]]; then redirect="${4:-https://fleet.example.test/auth/callback}"; else redirect="${4:-https://obroklik.example.test/auth/callback}"; fi
: "${ZITADEL_URL:=http://localhost:8085}" "${OUT_DIR:=.local/idp}"
HOSTH=(); [[ -n "${ZITADEL_HOST:-}" ]] && HOSTH=(-H "Host: $ZITADEL_HOST")
PAT="$(grep -E "^LOGIN_CLIENT_PAT=" "$OUT_DIR/secrets.env" | cut -d= -f2-)"   # least privilege: IAM_LOGIN_CLIENT only
pw="$(grep -E "^PASSWORD_$(tr 'a-z' 'A-Z' <<<"$user")=" "$OUT_DIR/secrets.env" | cut -d= -f2-)"

verifier="$(head -c 48 /dev/urandom | base64 | tr -d '=+/\n' | head -c 64)"
challenge="$(printf %s "$verifier" | openssl dgst -sha256 -binary | base64 | tr '+/' '-_' | tr -d '=')"
enc() { printf %s "$1" | jq -sRr @uri; }
# resourceowner scope adds the home-organization claims MIDGARD uses as the default tenant selection
scope="openid urn:zitadel:iam:user:resourceowner urn:zitadel:iam:org:project:id:$pid:aud${EXTRA_SCOPES:+ $EXTRA_SCOPES}"
loc="$(curl -sS "${HOSTH[@]}" -o /dev/null -D - "$ZITADEL_URL/oauth/v2/authorize?client_id=$cid&redirect_uri=$(enc "$redirect")&response_type=code&scope=$(enc "$scope")&code_challenge=$challenge&code_challenge_method=S256&state=s" | grep -i '^location:' | tr -d '\r')"
ar="$(sed -nE 's/.*authRequest=([^&]+).*/\1/p' <<<"$loc")"
[[ -n "$ar" ]] || die "no auth request in redirect: $loc"
sess="$(curl -fsS "${HOSTH[@]}" -H "Authorization: Bearer $PAT" -H 'Content-Type: application/json' -d "$(jq -nc --arg u "$user" --arg p "$pw" '{checks:{user:{loginName:$u},password:{password:$p}}}')" "$ZITADEL_URL/v2/sessions")"
cb="$(curl -fsS "${HOSTH[@]}" -H "Authorization: Bearer $PAT" -H 'Content-Type: application/json' -d "$(jq -nc --argjson s "$sess" '{session:{sessionId:$s.sessionId,sessionToken:$s.sessionToken}}')" "$ZITADEL_URL/v2/oidc/auth_requests/$ar" | jq -er .callbackUrl)"
code="$(sed -nE 's/.*[?&]code=([^&]+).*/\1/p' <<<"$cb")"
curl -fsS "${HOSTH[@]}" -d grant_type=authorization_code -d "code=$code" -d "client_id=$cid" -d "code_verifier=$verifier" --data-urlencode "redirect_uri=$redirect" "$ZITADEL_URL/oauth/v2/token" | jq -er ".${TOKEN_FIELD:-access_token}"
