#!/usr/bin/env bash
# Run the acceptance scenario against the Docker Compose stack (clean start, then restart check).
#   tests/acceptance/compose.sh            # uses the existing stack, or starts it
#   FRESH=1 tests/acceptance/compose.sh    # first destroys volumes (clean-volume install, OSS-01)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
[[ -f .env ]] || cp .env.example .env
COMPOSE=(docker compose --env-file .env -f deployments/compose/compose.yaml -f deployments/compose/compose.dev.yaml --profile dev-idp --profile demo)

[[ "${FRESH:-0}" == 1 ]] && "${COMPOSE[@]}" down -v --remove-orphans
deployments/compose/gen-dev-secrets.sh
"${COMPOSE[@]}" up -d --build --wait platform-core dev-idp edge example-capability
# Build the (profile-gated) sample application image now so `compose run` never builds/pulls on stdout later.
"${COMPOSE[@]}" build sample-application >&2
cargo build -q -p platform-contracts --bin contract-tool

# The sample application must run INSIDE the Compose network to reach stable logical endpoints.
WRAP="$(mktemp)"; trap 'rm -f "$WRAP"' EXIT
cat >"$WRAP" <<WRAPPER
#!/usr/bin/env bash
exec ${COMPOSE[*]} run --rm -T -e MIDGARD_CLIENT_ID -e MIDGARD_CLIENT_SECRET \\
  -e MIDGARD_REGISTRY_URL=http://platform-core:8080 sample-application check "\$@" 2>>"${WRAP}.err"
WRAPPER
chmod +x "$WRAP"

export PLATFORM_URL=http://platform.localhost:8088
export IDP_URL=http://localhost:9000
export CAPABILITY_URL=http://localhost:8082
export OPERATOR_CLIENT_SECRET=dev-only-operator PUBLISHER_CLIENT_SECRET=dev-only-publisher
export SAMPLE_APP="$WRAP" CONTRACT_TOOL="$ROOT/target/debug/contract-tool"
export RESTART_CMD="${COMPOSE[*]} restart platform-core && ${COMPOSE[*]} up -d --wait platform-core"
export CORE_LOGS_CMD="${COMPOSE[*]} logs --no-color platform-core example-capability"
tests/acceptance/ports.sh
tests/acceptance/acceptance.sh

echo "== restart the whole stack (not only core) and re-verify"
"${COMPOSE[@]}" restart
"${COMPOSE[@]}" up -d --wait platform-core dev-idp edge example-capability
sleep 5
tests/acceptance/acceptance.sh
