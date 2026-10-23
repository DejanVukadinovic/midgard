# ADR-001: Rust core with Go application facades

* Status: accepted
* Date: 2026-10-23

## Context
Reusable platform capabilities need performance, strong typing and a shared library surface (auth, persistence, HTTP conventions). Product-specific logic changes quickly and is owned by application teams.

## Decision
Platform core and reusable capabilities are Rust (Tokio, Axum, SQLx, Serde) in one Cargo workspace. Applications are separate repositories with Go facades using the generated Go SDK; frontends are TypeScript.

## Alternatives considered
* Single-language Go monolith: faster to start, but loses the shared Rust crates and pushes tenancy/auth enforcement into every app.
* All-Rust applications: slows product iteration where domain rules churn.

## Tradeoffs
Two toolchains to maintain, SDK drift risk (mitigated by generating clients from contracts and a CI drift check), and contributors need both ecosystems for the full test suite.

## Consequences
Shared behaviour (token verification, tenant context, RFC 9457 errors) lives once in `crates/`. Application code never reimplements it. Contracts, not language, are the integration boundary.
