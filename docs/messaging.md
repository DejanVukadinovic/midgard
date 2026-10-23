# Messaging boundary (`protocol: mq`)

Contracts name a **logical destination**; the broker is an adapter choice. NATS JetStream is the initial
adapter. Rust code uses the `platform-messaging` crate (`MessageBus`/`Consumer` traits); providers and
clients in any language only need the contract, the broker's client library and their broker credentials —
the Go fixture in [`examples/echo`](../examples/echo) uses `nats.go` and no MIDGARD code.

## Who guarantees what

| Property | MIDGARD contract (trait) | Adapter capability | Application logic |
|---|---|---|---|
| `publish` returns success only after durable broker acknowledgment; failure is an error | ✔ | NATS: JetStream publish ack | |
| At-least-once delivery, explicit ack, redelivery on `nak` / ack timeout | ✔ | NATS: durable pull consumer | |
| Bounded redelivery (`max_deliver`) + dead-letter isolation of poison messages (`settle`) | ✔ | NATS: `max_deliver`, back-off | provider decides *what* is poison |
| Publish de-duplication by message id | | NATS: `Nats-Msg-Id`, bounded `duplicate_window` (2 min in the fixture) | idempotent handling beyond the window |
| Durability across broker restart | | NATS: file storage on a named volume (in-memory adapter: none) | |
| Authorization to publish/consume | | broker ACLs (NATS users/permissions) | |
| Correlation / W3C `traceparent` propagation | carried in `Message` | NATS headers | continuing the trace |
| Tenant/application identity | | derived from the authenticated broker identity (see below) | tenant-scoped data handling |
| Ordering, exactly-once | **not claimed** | | |

Messages that exhaust `max_deliver` *without* an ack or term (e.g. a crashing consumer) are not redelivered;
NATS emits a `MAX_DELIVERIES` advisory and the in-memory adapter reports them via `exhausted()`. `settle()` avoids this
for handled failures by publishing to the dead-letter destination first.

## Identity on the wire

Message headers are not proof of identity. The fixture binds the caller to the **subject**: the broker lets
application `obroklik` publish only to `example.echo.submit.obroklik`; the provider derives the application
from the subject suffix, then asks Core `POST /v0/authorize {application, interface: "queue"}`. Binding a
*tenant* the same way (a subject per tenant, or signed envelopes) is **not implemented**; the fixture's queue
interface is application-scoped and service-only.

## NATS mapping and provisioning

* Stream `EXAMPLE` (file storage, subjects `example.>`), durable pull consumer `provider-submit`
  (explicit ack, `max_deliver` 5, back-off 1s/2s/4s/8s). Provisioned idempotently by `echo nats-init` with the
  admin identity (the `nats-init` Compose job). Runtime identities cannot create streams or consumers.
* [`deployments/compose/nats/nats.conf`](../deployments/compose/nats/nats.conf): one account, users `admin`
  (provisioning), `provider` (consume + dead-letter publish), `client-obroklik` (publish to its own subject), no anonymous
  access. Passwords come from environment variables substituted into the file, so they must start with a letter.
* Tested against real `nats-server` 2.10.22: adapter conformance, persistence across broker kill/restart, and
  permission violations ([`crates/platform-messaging/tests/nats.rs`](../crates/platform-messaging/tests/nats.rs)).

## Replacing the broker

Adapter interchangeability does not make migration automatic. A migration needs a plan for in-flight messages
(drain or dual-publish), consumer positions (JetStream sequence numbers do not transfer), the ACL model
(re-expressed in the new broker), and downtime. The registry already separates `destination` from
`broker_binding`, so contracts do not change; deployments and providers do.
