# Capability contracts v0.2

A v0.2 contract describes one **capability** that can be offered over several **interfaces**
(transports). It is published through `POST /v0/capability-contracts`, validated, normalized and stored
immutably next to the v0 contract registry. v0 contracts (`POST /v0/contracts`) keep working unchanged;
grants, bindings and the resolver use one shared namespace of contract names.

| Concept | Owned by | Not owned |
|---|---|---|
| Capability contract | name `service.capability`, version, digest, authorization requirements, interfaces | hostnames, broker URLs, secrets |
| Interface specification | OpenAPI 3.1 operation (`http`) or AsyncAPI 3.0 operation (`mq`) | grants, deployments |
| Implementation | provider identity, which interfaces it serves, where | the contract's meaning |
| Grant / binding | which application may use / which implementation serves it | payload shape |
| Resolved contract | the authorized view for one caller | long-lived authorization |

## Manifest

Schema: [`contracts/schema/midgard-contract-v0.2.schema.json`](../contracts/schema/midgard-contract-v0.2.schema.json).
Complete, validated example (OpenAPI + AsyncAPI): [`contracts/examples/echo`](../contracts/examples/echo).

```yaml
midgard: "0.2"
service: example.echo          # dotted lowercase segments
capability: submit             # ONE lowercase segment: name = service.capability is unambiguous
version: 1.0.0                 # SemVer, no pre-release/build; immutable once published
execution: {outcome: accepted, idempotency: supported}
authorization:
  permission: example.echo:submit
  callers: [service, user]
  context: {application: required, tenant: required}
  delegation: supported
interfaces:
  - id: rest
    protocol: http             # transport family, never a vendor name
    specification: {type: openapi, document: ./openapi.yaml, operation: submitEcho}
  - id: queue
    protocol: mq
    authorization: {callers: [service]}   # may narrow, never widen
    specification: {type: asyncapi, document: ./asyncapi.yaml, operation: submitEcho}
```

### Validation rules (enforced at publish time)

* Unknown fields are rejected; `protocol` ∈ `http|mq`; `http` requires an `openapi` spec and `mq` an `asyncapi` spec.
* Interface ids are unique (`^[a-z][a-z0-9-]{0,31}$`); 1–16 interfaces.
* `specification.document` is a **relative path supplied in the request** (`documents` map) or `inline`.
  Absolute paths, `..`, backslashes and any `scheme://` are rejected; the server never fetches documents.
* OpenAPI: must be `3.1.x`, the `operationId` must exist exactly once, `responses` non-empty with valid status
  keys, request/response media types well formed. Non-JSON media types (XML, text, binary) are accepted and
  recorded verbatim.
* AsyncAPI: must be `3.0.x`; the operation must exist, `action` is `send` or `receive` (request/reply is **not
  supported in v0.2** and is rejected); `channel.$ref` and message `$ref`s must resolve inside the document;
  every message needs a `payload`. Recorded: action, channel/address, message names, content type, whether
  headers and a correlation id are declared.
* Interface `callers` must be a subset of the capability `callers`.
* **Semantic agreement across transports (partial):** `execution.outcome: accepted` requires the HTTP operation
  to answer `202` and no other `2xx`; `completed` forbids `202`. MQ semantics cannot be checked mechanically;
  the MQ interface description must state them (the example does).
* No credentials belong in manifests; nothing in the format can carry any.

### Identity, digest, immutability

A published version is identified by `(service, capability, version)` and a `sha256:` **digest** over the
normalized manifest, which embeds the digest of each normalized (canonical JSON) interface document. Re-publishing
identical content is a no-op (`200`); different content for the same version is `409`; the database also blocks
updates/deletes of contract payloads and interfaces. Changed interface or IAM declaration ⇒ new version.
Changed deployment location ⇒ no new version.

**Resolution is by exact version (`=1.0.0`) or the same `semver` constraints as v0; nothing is silently upgraded.**
There is no automatic schema-compatibility proof for v0.2 contracts (the v0 OpenAPI breaking-change check
applies only to v0 contracts).

## Registering an implementation

`POST /v0/implementations` (approved deployment identity) — the `implements[]` entry gains `interfaces`:

```json
{"contract": "example.echo.submit", "version": "1.0.0", "digest": "sha256:…",
 "interfaces": [
   {"id": "rest",  "location": "http://example-provider:8080"},
   {"id": "queue", "destination": "example.echo.submit", "broker_binding": "default-mq"}]}
```

* HTTP `location` is a base URL under the same endpoint (SSRF) policy as deployments; method/path come from OpenAPI.
* MQ `destination` is a plain dotted name (no wildcards) and `broker_binding` names the deployment's broker
  configuration — deployment metadata, not contract identity.
* v0.2 contracts need at least one interface; a **subset** (e.g. HTTP only) is valid. Ids and protocols are
  checked against the published contract; v0 contracts must not pass `interfaces`.
* Re-registering replaces the advertised interface set; no restart of Core is needed.
* A deployment (health-probed HTTP endpoint) is still registered per environment, also for MQ-only providers.

## Resolution

`POST /v0/contracts/resolve` (workload token) behaves exactly as in v0 (declared requirement → active grant →
explicit binding → healthy deployment) and, for v0.2 contracts, additionally requires the grant to include the
contract's `authorization.permission`, then returns only interfaces that the **bound implementation advertises**
and that a **service** caller may use:

```json
{"contract": "example.echo.submit", "resolved_version": "1.0.0", "digest": "sha256:…",
 "protocol": "multi", "endpoint": "http://example-provider:8080", "expires_in_seconds": 300,
 "authorization": {"permission": "example.echo:submit", "context": {…}, "delegation": "supported"},
 "execution": {"outcome": "accepted", "idempotency": "supported"},
 "interfaces": [
  {"id": "rest", "protocol": "http", "operation": "submitEcho", "location": "http://example-provider:8080",
   "specification": {"type": "openapi", "digest": "sha256:…", "summary": {"method": "POST", "path": "/submit", …},
                     "document_path": "/v0/contracts/example.echo.submit/versions/1.0.0/interfaces/rest/document"}},
  {"id": "queue", "protocol": "mq", "destination": "example.echo.submit", "broker_binding": "default-mq", …}]}
```

The response carries no credentials and no other party's grants. `expires_in_seconds` is the cache
lifetime: clients must re-resolve afterwards. Revocation is **not** pushed to clients; it is enforced at
invocation (see [iam-enforcement.md](iam-enforcement.md)). A resolved endpoint is not an authorization token.

## Tooling

`contract-tool validate-v02 <dir>` validates a package offline and prints its digest;
`contract-tool request-v02 <dir>` builds the publish request body (`make contracts-check` runs the former).

## Not in v0.2

gRPC; request/reply messaging; compatible-range resolution for v0.2 contracts; automatic compatibility proofs;
generated clients for v0.2 contracts; per-tenant implementation bindings beyond the existing
application/environment/major binding; interface-level grants (restrictions are by caller kind only).
