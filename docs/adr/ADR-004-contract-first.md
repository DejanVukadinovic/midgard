# ADR-004: Contract-first dependencies

* Status: accepted
* Date: 2026-10-23

## Context
Applications must not hard-code service identities, and implementations must be replaceable.

## Decision
Applications depend on `contract@version-range`. Contracts (OpenAPI 3.1 + semantics + permissions) are published independently, immutable and content-addressed; services are separate registered entities that claim contract digests.

## Alternatives considered
Service-name discovery (couples apps to topology); shared client libraries per service (version lockstep).

## Tradeoffs
Contract authoring overhead; structural compatibility checks cannot prove behavioural compatibility (semantics are reviewed by humans, conformance runs in CI).

## Consequences
A service may implement many contracts and a contract may have many implementations; breaking changes require a new major which coexists with the old one.
