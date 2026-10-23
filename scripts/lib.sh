#!/usr/bin/env bash
# Shared helpers for MIDGARD scripts. Requires: curl, jq.
set -euo pipefail

: "${PLATFORM_URL:=http://localhost:8080}"
: "${IDP_URL:=http://localhost:9000}"
: "${IDP_TOKEN_URL:=${IDP_URL}/oauth/v2/token}"

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

need() { for c in "$@"; do command -v "$c" >/dev/null || die "missing required tool: $c"; done; }

# Identity provider settings. With ZITADEL, `scripts/zitadel-bootstrap.sh` writes $IDP_DIR/idp.env (ids,
# audiences = project ids) and $IDP_DIR/secrets.env (client secrets); both are loaded when present.
IDP_DIR="${IDP_DIR:-}"
if [[ -n "$IDP_DIR" && -f "$IDP_DIR/idp.env" ]]; then set -a; source "$IDP_DIR/idp.env"; [[ -f "$IDP_DIR/secrets.env" ]] && source "$IDP_DIR/secrets.env"; set +a; fi
: "${IDP_KIND:=dev}"
: "${PLATFORM_AUDIENCE:=midgard-core}" "${CAPABILITY_AUDIENCE:=platform.example}" "${ECHO_AUDIENCE:=example.echo}"
: "${ORG_A:=org-acme}" "${ORG_B:=org-globex}"
: "${OBROKLIK_PROJECT:=prj-obroklik}" "${OBROKLIK_WEB_CLIENT:=obroklik-web}" "${FLEET_PROJECT:=prj-fleet}" "${FLEET_WEB_CLIENT:=fleet-web}"

# csecret <client_id>: dev-idp fixture secrets, or the ZITADEL-issued secret from secrets.env
csecret() {
  local v="SECRET_$(tr 'a-z-' 'A-Z_' <<<"$1")"
  if [[ -n "${!v:-}" ]]; then printf '%s' "${!v}"; return; fi
  case "$1" in
    midgard-operator) echo dev-only-operator ;; midgard-publisher) echo dev-only-publisher ;;
    example-deployer) echo dev-only-deployer ;; echo-provider) echo dev-only-echo-provider ;;
    obroklik-backend) echo dev-only-obroklik ;; fleet-backend) echo dev-only-fleet ;;
    *) die "no secret known for client $1" ;;
  esac
}

# token <client_id> <client_secret> [audience]   (ZITADEL project-id audiences are requested as scopes)
token() {
  local aud="${3:-$PLATFORM_AUDIENCE}"
  if [[ "$aud" =~ ^[0-9]+$ ]]; then
    curl -fsS -u "$1:$2" -d grant_type=client_credentials --data-urlencode "scope=openid urn:zitadel:iam:org:project:id:$aud:aud" "$IDP_TOKEN_URL" | jq -er .access_token
  else
    curl -fsS -u "$1:$2" -d grant_type=client_credentials -d "audience=${aud}" "$IDP_TOKEN_URL" | jq -er .access_token
  fi
}

# user_token <user> <web client> <project>  — a real end-user access token
# (dev-idp: /dev/user-token; ZITADEL: authorization code + PKCE through the session API)
user_token() {
  if [[ "$IDP_KIND" == zitadel ]]; then
    local cid="$2" pid="$3"
    case "$2" in obroklik-web) cid="$OBROKLIK_WEB_CLIENT" ;; fleet-web) cid="$FLEET_WEB_CLIENT" ;; esac
    case "$3" in prj-obroklik) pid="$OBROKLIK_PROJECT" ;; prj-fleet) pid="$FLEET_PROJECT" ;; esac
    "$(dirname "${BASH_SOURCE[0]}")/zitadel-user-token.sh" "$1" "$cid" "$pid"
  else
    curl -fsS -H 'Content-Type: application/json' -d "$(jq -nc --arg u "$1" --arg c "$2" --arg a "$3" '{user:$u,client_id:$c,audience:[$a]}')" "$IDP_URL/dev/user-token" | jq -er .access_token
  fi
}

# api <method> <path> <token> [json-body] [extra curl args...]; prints "<status>\n<body>"
api() {
  local method="$1" path="$2" tok="$3" body="${4:-}"; shift 4 || shift $#
  local args=(-sS -X "$method" -H "Authorization: Bearer $tok" -w '\n%{http_code}')
  [[ -n "$body" ]] && args+=(-H 'Content-Type: application/json' --data "$body")
  curl "${args[@]}" "$@" "${PLATFORM_URL}${path}"
}

# status_of <api output> / body_of <api output>
status_of() { printf '%s' "$1" | tail -n1; }
body_of() { printf '%s' "$1" | sed '$d'; }

# expect <expected status(es), pipe separated> <api output> <description>
expect() {
  local st; st="$(status_of "$2")"
  if [[ "|$1|" != *"|$st|"* ]]; then
    die "$3: expected HTTP $1, got $st: $(body_of "$2")"
  fi
}

wait_http() { # wait_http <url> <seconds>
  local i
  for ((i = 0; i < $2; i++)); do curl -fsS -o /dev/null "$1" 2>/dev/null && return 0; sleep 1; done
  die "timed out waiting for $1"
}
