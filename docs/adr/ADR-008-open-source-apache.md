# ADR-008: Public Apache-2.0 project with an internally driven roadmap

* Status: accepted
* Date: 2026-10-23

## Context
MIDGARD is built in public from the start but exists to accelerate our own products.

## Decision
Apache License 2.0 for MIDGARD-owned code, DCO sign-off (no CLA), public source/ADRs/contracts/tests. Maintainers control scope, releases and compatibility; no support SLAs. Registries are never publicly writable: dynamic registration requires operator-approved identities. Published contract versions are immutable; pre-1.0 platform API stability is documented in `docs/contributing/api-stability.md`.

## Alternatives considered
Source-available or private development; copyleft licensing.

## Tradeoffs
Public scrutiny and contributor expectations versus openness and reviewability; mitigated by explicit governance and "demo defaults are not production defaults".

## Consequences
Hygiene is part of the product: secret scanning including history, dependency/license checks, documented threat model, and CI-backed README claims.
