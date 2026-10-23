# ADR-009: v0.2 contract identity, naming and versioning

* Status: accepted
* Date: 2026-10-23

## Context
v0 contracts are named `namespace.name` with one OpenAPI document. v0.2 contracts are capabilities with several
interfaces. Grants, bindings, the resolver and the audit log already key on a single contract-name string.

## Decision
A v0.2 contract is `(service, capability, version)`; its **contract name** is `service.capability`, where
`capability` is exactly one segment so the split is unambiguous (`a.b` + `c` can never collide with `a` + `b.c`).
It is stored in the existing `contracts` table (manifest `midgard: "0.2"`, `protocol: "multi"`) with interface
rows in `contract_interfaces`. The digest covers the normalized manifest, which embeds each normalized
interface document's digest. Versions are exact-match by default; publishing is idempotent for identical
content, `409` otherwise, and immutability is also enforced by database triggers.

## Alternatives
A separate table/namespace for v0.2 contracts (would duplicate grants/bindings/resolver); folding interfaces into
the v0 `openapi` column (cannot represent AsyncAPI); content-defined names (unreadable).

## Tradeoffs
v0 and v0.2 contracts share a namespace, so a name cannot be both. The legacy `openapi` column holds the first HTTP
document (or empty) for v0 readers. No automatic compatibility proof exists for v0.2 contracts.

## Consequences
Existing v0 data, APIs and tests are untouched; migration `0003` is additive.
