# v0.2 acceptance matrix and verification status

"Verified" means executed with the real component named. Nothing in the "Not run" rows should be read as passing.

| Area | Test | Where | Status |
|---|---|---|---|
| Regression | v0 suites unchanged | `cargo test --workspace` (Rust, real PostgreSQL), `go test` for SDK/sample | Verified locally (all pass) |
| Contract | Valid OpenAPI + AsyncAPI registered, retrievable | `crates/platform-contracts` unit tests, `services/platform-core/tests/v02.rs`, `tests/integration/v02.sh` | Verified |
| Contract | Invalid operation / reference / protocol / widening / bad doc rejected with actionable error | same | Verified |
| Contract | Published version mutated | `409` (`v02.rs`, `v02.sh`); DB triggers | Verified |
| Contract | Non-JSON media type | `non_json_media_types_are_represented` | Verified (parsing/representation only) |
| Contract | JSON Schema validates fixtures, rejects vendor protocol | `example_contract_validates_against_json_schema` | Verified |
| Registry | One capability, HTTP + MQ | `v02.rs`, `v02.sh` | Verified |
| Registry | Partial implementation (HTTP only) | `v02.rs` | Verified |
| Registry | Dynamic registration without Core restart | `v02.rs` (second registration), provider self-registration in `v02.sh` | Verified |
| Binding | Explicit application binding selects implementation | v0 resolver tests + `v02.rs` | Verified (no per-tenant bindings) |
| IAM | Missing grant / grant lacking the permission | `v02.rs`, `v02.sh` | Verified |
| IAM | User attempts service-only MQ interface | `/v0/authorize` refusal in `v02.rs`. A user has no broker credentials; broker-side denial is covered by the next row | Verified at Core; no end-to-end user→broker path exists |
| IAM | Cross-tenant | v0 tenancy tests (resolver); provider refusal of `org-globex` for alice in `v02.sh` | Verified |
| IAM | Revoked grant after resolution | `v02.sh`: provider answers 403, queue message dead-lettered | Verified; no decision cache (documented limits) |
| HTTP | Resolve → invoke, wrong media type, invalid payload, unauthenticated, wrong audience | `v02.sh` | Verified. "Unavailable endpoint" is covered by the v0 health/resolver tests, not re-tested for v0.2 |
| MQ | Publish → consume → ack, de-duplication | conformance + `v02.sh` | Verified (real NATS) |
| MQ | Consumer crash before ack → redelivery | conformance `unacked_message_is_redelivered` | Verified (real NATS) |
| MQ | Poison message isolation, bounded redelivery | conformance `poison_message_is_isolated`; `v02.sh` | Verified |
| MQ | Broker kill + restart keeps messages | `messages_survive_broker_restart` (kill → restart on same store) | Verified (process kill, not `docker restart`) |
| MQ | Direct unauthorized broker access | `broker_enforces_publish_and_consume_permissions`, `v02.sh` | Verified (real NATS ACLs) |
| Language | Independent provider, no MIDGARD library | `examples/echo` (Go; imports only `nats.go`) | Verified |
| Compose | Fresh checkout `docker compose up -d --build` healthy | CI job `v02-compose` | **Not run** (no Docker daemon in the authoring environment); `docker compose config` validates |
| Compose | Bootstrap repeated, no duplicates | `v02.sh` runs bootstrap twice | Verified as local processes; **not run in Compose** |
| Compose | Restart keeps state | v0 restart acceptance; Compose restart in CI job | **Not run** for v0.2 in Compose |
| Compose | Test profile deterministic exit code | `integration-tests` service | **Not run** (the script itself was run natively) |
| Compose | ZITADEL | — | **Not implemented** (dev IdP only) |
| Hardening | Secret scan, license/advisory, image scan | `make audit`, `make secrets-scan`, CI security workflow | **Not run** here (tools unavailable); new dependencies: `async-nats` (Apache-2.0), `jsonschema` (MIT), `async-trait` (MIT/Apache-2.0), `nats.go` (Apache-2.0) |
