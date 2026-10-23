#!/usr/bin/env bash
# Generate random LOCAL-ONLY database credentials into the git-ignored .secrets/ directory.
# Idempotent: existing files are kept, so re-running never rotates working credentials.
set -euo pipefail
cd "$(dirname "$0")"
umask 077
mkdir -p .secrets
gen() { head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n'; }
put() { [[ -f ".secrets/$1" ]] || printf '%s' "$2" >".secrets/$1"; }
val() { cat ".secrets/$1"; }

put pg_superuser_password "$(gen)"
for k in midgard_owner midgard_app capability_owner capability_app zitadel_db zitadel_masterkey; do put "pw_$k" "$(gen | head -c 32)"; done
# ZITADEL requires a 32-byte master key
printf '%s' "$(val pw_zitadel_masterkey | head -c 32)" >.secrets/zitadel_masterkey
put core_db_url                 "postgres://midgard_app:$(val pw_midgard_app)@postgres:5432/midgard_core"
put core_migration_db_url       "postgres://midgard_owner:$(val pw_midgard_owner)@postgres:5432/midgard_core"
put capability_db_url           "postgres://capability_app:$(val pw_capability_app)@postgres:5432/midgard_capability"
put capability_migration_db_url "postgres://capability_owner:$(val pw_capability_owner)@postgres:5432/midgard_capability"
# env file consumed by the db-init job
cat >.secrets/db-init.env <<ENV
MIDGARD_OWNER_PASSWORD=$(val pw_midgard_owner)
MIDGARD_APP_PASSWORD=$(val pw_midgard_app)
CAPABILITY_OWNER_PASSWORD=$(val pw_capability_owner)
CAPABILITY_APP_PASSWORD=$(val pw_capability_app)
ZITADEL_DB_PASSWORD=$(val pw_zitadel_db)
ENV
cat >.secrets/zitadel.env <<ENV
ZITADEL_DATABASE_POSTGRES_USER_PASSWORD=$(val pw_zitadel_db)
ZITADEL_DATABASE_POSTGRES_ADMIN_PASSWORD=$(val pg_superuser_password)
ENV
echo "secrets ready in deployments/compose/.secrets (ignored by git; local development only)"
