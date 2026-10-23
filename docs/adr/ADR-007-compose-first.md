# ADR-007: Docker Compose first; Kubernetes-ready separation

* Status: accepted
* Date: 2026-10-23

## Context
A single environment on one host is enough for v0; Kubernetes would be premature complexity.

## Decision
Docker Compose is the deployment target (pinned image tags, health checks, secrets from files, internal network, only the edge publishes a loopback port). Core never deploys or scales containers. Registered endpoints are orchestration-neutral.

## Alternatives considered
Kubernetes from day one; Nomad.

## Tradeoffs
No built-in rolling deploys or autoscaling; Compose ordering is not readiness (we use health checks and retry-safe initialization everywhere).

## Consequences
Moving to Kubernetes means registering Service DNS names instead of Compose names; the registry, contracts and bindings do not change.
