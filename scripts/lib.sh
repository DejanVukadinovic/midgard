#!/usr/bin/env bash
# Shared helpers for MIDGARD scripts. Requires: curl, jq.
set -euo pipefail

: "${PLATFORM_URL:=http://localhost:8080}"
: "${IDP_URL:=http://localhost:9000}"
: "${IDP_TOKEN_URL:=${IDP_URL}/oauth/v2/token}"

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

need() { for c in "$@"; do command -v "$c" >/dev/null || die "missing required tool: $c"; done; }

# token <client_id> <client_secret> [audience]
token() {
  local aud="${3:-midgard-core}"
  curl -fsS -u "$1:$2" -d grant_type=client_credentials -d "audience=${aud}" "$IDP_TOKEN_URL" | jq -er .access_token
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
