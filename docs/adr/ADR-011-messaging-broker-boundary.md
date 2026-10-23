# ADR-011: Broker-neutral messaging boundary, NATS JetStream first

* Status: accepted
* Date: 2026-10-23

## Decision
`platform-messaging` defines `MessageBus`/`Consumer`/`Delivery` plus a conformance suite every adapter must pass.
Adapters: deterministic in-memory (API behaviour only) and NATS JetStream (file storage, durable explicit-ack pull
consumers, `Nats-Msg-Id` de-duplication). Contracts carry logical destinations; `broker_binding` and ACLs are
deployment metadata. Guarantees are split into MIDGARD contract / adapter capability / application logic in
[messaging.md](../messaging.md). Authorization is broker ACLs, not message headers.

## Alternatives
Kafka/RabbitMQ first (heavier to operate locally); an outbox/event router (out of scope).

## Tradeoffs
At-least-once only; de-duplication is windowed; broker replacement needs a migration plan, not just a config
change. JetStream ACLs are written against specific `$JS.API.*` subjects and must be revisited on NATS upgrades.

## Evidence
Real-broker tests: conformance, restart persistence, permission violations (`crates/platform-messaging/tests/nats.rs`).
