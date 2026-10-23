# ADR-013: Root `compose.yaml` over the v0 stack; dev IdP; secrets generation

* Status: accepted
* Date: 2026-10-23

## Decision
The repository root `compose.yaml` `include`s the v0 stack unchanged and adds NATS, the synthetic provider/client,
bootstrap and the test runner. The default IdP remains the offline `dev-idp`; the v0 `zitadel` profile stays a
documented, **unverified** template. Credentials are generated into git-ignored files by
`deployments/compose/gen-dev-secrets.sh` before `up` (Compose file-secrets must exist at start), so the quickstart
has one more command than the build guide's literal three.

## Alternatives
Generating secrets in a one-shot container into a volume (needs every service changed to read from it — rejected as
a gratuitous rewrite of working v0 wiring); building the ZITADEL first-instance bootstrap without being able to run it.

## Consequences
The "ZITADEL in Compose" requirement of the v0.2 guide is **not met** by this change; see compose-quickstart.md.
