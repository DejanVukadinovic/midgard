# MIDGARD developer entry points. `make help` lists targets.
SHELL := /bin/bash
COMPOSE := docker compose --env-file .env -f deployments/compose/compose.yaml -f deployments/compose/compose.dev.yaml --profile dev-idp --profile demo
OAPI_CODEGEN := go run github.com/oapi-codegen/oapi-codegen/v2/cmd/oapi-codegen@v2.4.1
export TEST_DATABASE_ADMIN_URL ?= postgres://midgard:midgard@localhost:5432/postgres

.PHONY: help fmt fmt-check lint test test-rust test-go contracts-check sdk-generate sdk-check \
        dev-up dev-down bootstrap acceptance acceptance-compose local-up local-down audit secrets-scan \
        up down test-compose acceptance-interfaces test-messaging

help: ## list targets
	@grep -hE '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-20s %s\n", $$1, $$2}'

fmt: ## format Rust and Go
	cargo fmt --all
	gofmt -w sdk/go examples/sample-application examples/echo

fmt-check: ## verify formatting
	cargo fmt --all -- --check
	test -z "$$(gofmt -l sdk/go examples/sample-application examples/echo)"

lint: ## clippy (deny warnings) + go vet
	cargo clippy --workspace --all-targets --locked -- -D warnings
	cd sdk/go && go vet ./...
	cd examples/sample-application && go vet ./...
	cd examples/echo && go vet ./...

test-rust: ## Rust tests (needs PostgreSQL: set TEST_DATABASE_ADMIN_URL, superuser)
	cargo test --workspace --locked

test-go: ## Go SDK + sample app tests
	cd sdk/go && go test ./...
	cd examples/sample-application && go test ./...
	cd examples/echo && go test ./...

test: test-rust test-go ## all unit/integration tests

contracts-check: ## schema compatibility of published contract versions + digest sanity
	cargo run -q -p platform-contracts --bin contract-tool -- check contracts/platform/example/v1/1.0.0/openapi.yaml contracts/platform/example/v1/1.1.0/openapi.yaml
	cargo run -q -p platform-contracts --bin contract-tool -- digest contracts/platform/example/v1/1.0.0 >/dev/null
	cargo run -q -p platform-contracts --bin contract-tool -- digest contracts/platform/example/v1/1.1.0 >/dev/null
	cargo run -q -p platform-contracts --bin contract-tool -- validate-v02 contracts/examples/echo >/dev/null

sdk-generate: ## regenerate the Go contract client from the contract package
	cd sdk/go/contracts/platformexample && $(OAPI_CODEGEN) -config oapi-codegen.yaml ../../../../contracts/platform/example/v1/1.1.0/openapi.yaml

sdk-check: ## fail if the committed generated client drifts from the contract
	@tmp=$$(mktemp -d) && cp sdk/go/contracts/platformexample/oapi-codegen.yaml $$tmp/ && \
	 (cd $$tmp && $(OAPI_CODEGEN) -config oapi-codegen.yaml $(CURDIR)/contracts/platform/example/v1/1.1.0/openapi.yaml 2>/dev/null) && \
	 diff -u sdk/go/contracts/platformexample/client.gen.go $$tmp/client.gen.go && echo "sdk: no drift"

# ---- Docker Compose quickstart -------------------------------------------------------------
dev-up: ## start the full stack with Docker Compose (dev IdP)
	@test -f .env || cp .env.example .env
	deployments/compose/gen-dev-secrets.sh
	$(COMPOSE) up -d --build --wait platform-core dev-idp edge example-capability

dev-down: ## stop the stack (keep volumes)
	$(COMPOSE) down

bootstrap: ## register contracts, apps, tenants, grant + binding (idempotent)
	cargo build -q -p platform-contracts --bin contract-tool
	PLATFORM_URL=http://platform.localhost:8088 IDP_URL=http://localhost:9000 \
	OPERATOR_CLIENT_SECRET=dev-only-operator PUBLISHER_CLIENT_SECRET=dev-only-publisher scripts/bootstrap.sh

acceptance-compose: ## end-to-end acceptance on a clean Compose stack
	FRESH=1 tests/acceptance/compose.sh

# ---- v0.2: single root Compose entry point (see docs/compose-quickstart.md) -----------------
up: ## v0.2 stack: secrets + `docker compose up -d --build` (root compose.yaml)
	@test -f .env || cp .env.example .env
	deployments/compose/gen-dev-secrets.sh
	docker compose up -d --build --wait

test-compose: ## v0.2 end-to-end tests inside the Compose network (needs `make up`)
	docker compose --profile test run --rm integration-tests

down: ## stop the v0.2 stack, KEEPING volumes (`docker compose down -v` deletes all state)
	docker compose down

# ---- Docker-free quickstart (processes on localhost; needs a local PostgreSQL superuser) ----
local-up: ## start dev IdP, core and example service as local processes
	scripts/dev-local.sh up

local-down:
	scripts/dev-local.sh down

acceptance: ## end-to-end acceptance against the local-process stack
	scripts/dev-local.sh up
	eval "$$(scripts/dev-local.sh env)" && tests/acceptance/acceptance.sh && tests/acceptance/backup-restore.sh

acceptance-interfaces: ## v0.2 end-to-end (processes on localhost; needs nats-server on PATH or NATS_SERVER_BIN)
	WITH_V02=1 scripts/dev-local.sh up
	eval "$$(scripts/dev-local.sh env)" && OPERATOR_CLIENT_SECRET=dev-only-operator PUBLISHER_CLIENT_SECRET=dev-only-publisher scripts/bootstrap.sh \
	  && OPERATOR_CLIENT_SECRET=dev-only-operator PUBLISHER_CLIENT_SECRET=dev-only-publisher tests/integration/v02.sh

test-messaging: ## messaging conformance incl. real NATS JetStream (needs nats-server; set NATS_SERVER_BIN)
	MIDGARD_REQUIRE_NATS=1 cargo test -p platform-messaging --locked

audit: ## dependency vulnerability + license checks (needs cargo-deny, govulncheck)
	cargo deny check
	cd sdk/go && govulncheck ./...
	cd examples/sample-application && govulncheck ./...
	cd examples/echo && govulncheck ./...

secrets-scan: ## scan the working tree and full history (needs gitleaks)
	gitleaks detect --redact --config .gitleaks.toml
