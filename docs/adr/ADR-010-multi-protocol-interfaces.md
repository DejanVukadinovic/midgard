# ADR-010: One capability, several interfaces, standard specifications

* Status: accepted
* Date: 2026-10-23

## Decision
A contract lists interfaces with a transport family (`http`, `mq`; later `grpc`) and a reference to an
OpenAPI 3.1 or AsyncAPI 3.0 operation. MIDGARD validates structure and operation existence but does not invent
a payload type system and does not fetch documents from URLs (SSRF): documents arrive in the publish request or
inline, are normalized to canonical JSON and stored. Interface authorization may only narrow capability
authorization. `execution.outcome` is checked against HTTP status codes (`accepted` ⇒ 202) so transports do not
silently disagree; MQ semantics are documentation. An implementation may serve a subset of interfaces.

## Alternatives
A MIDGARD-specific schema language; validating with full OpenAPI/AsyncAPI validators (no mature Rust AsyncAPI 3
validator was found — structural checks plus the example fixtures are used and the gap is documented).

## Consequences
The structural validators are intentionally shallow: schemas inside documents are not type-checked beyond
being well-formed JSON/YAML. Strengthening them is future work.
