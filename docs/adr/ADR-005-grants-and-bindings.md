# ADR-005: Explicit application/contract grants and implementation bindings

* Status: accepted
* Date: 2026-10-23

## Context
Two implementations of one contract may hold different state; silently switching between them would split or corrupt data. Discovery must not equal authorization.

## Decision
Two separate operator actions: a **grant** (authorization: which permissions) and a **binding** (routing: which implementation per application/major/environment). Resolution requires both. The resolver never fails over between implementations.

## Alternatives considered
Health-based automatic selection (unsafe for stateful implementations); implicit grants from bindings (a routing change would alter authorization).

## Tradeoffs
More operator steps; a failed bound implementation yields `503` rather than a degraded alternative.

## Consequences
Migrating between stateful implementations is an explicit data-migration plus rebinding. Every grant/binding change is audited with before/after.
