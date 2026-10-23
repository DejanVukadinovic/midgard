# ADR-006: Client-side resolution and stable logical endpoints

* Status: accepted
* Date: 2026-10-23

## Context
An extra gateway hop adds latency and a failure domain; infrastructure already balances replicas.

## Decision
The Go SDK resolves a contract via core and then calls the implementation directly. The registry returns a **stable logical endpoint** (Docker DNS name / reverse proxy / future Kubernetes Service), never a container or pod IP, with a TTL. Implementations authenticate and authorize every call themselves.

## Alternatives considered
Central API gateway/proxy (single choke point, more latency, becomes a data-plane dependency); service mesh (out of scope).

## Tradeoffs
Clients carry resolution logic (kept thin and generated where possible); the SDK holds a bounded routing cache with stale-if-error limited to registry outages.

## Consequences
Registry outages do not stop already-routed healthy traffic beyond the TTL + stale bound; authorization is never served stale.
