#!/bin/sh
# Entrypoint wrapper: if the ZITADEL bootstrap job published its ids to $IDP_DIR (default /idp), export
# them (they override the static defaults from the Compose file), apply IDP_ENV_MAP renames, then exec the service.
#   IDP_ENV_MAP="TARGET=SOURCE ..."   e.g. CAPABILITY_CORE_AUDIENCE=PLATFORM_AUDIENCE (TARGET gets the value of variable SOURCE)
# Without idp.env (dev-IdP mode) this is a no-op.
dir=${IDP_DIR:-/idp}
if [ -f "$dir/idp.env" ]; then
  set -a
  # shellcheck disable=SC1090
  . "$dir/idp.env"
  [ -f "$dir/secrets.env" ] && . "$dir/secrets.env"
  set +a
  for kv in $IDP_ENV_MAP; do
    target=${kv%%=*}; source=${kv#*=}
    export "$target=$(eval "printf %s \"\${$source}\"")"
  done
fi
exec "$@"
