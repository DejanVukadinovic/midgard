# Contracts and resolution

## Contract package

```text
contracts/platform/example/v1/1.1.0/
├── manifest.yaml     # id, version, protocol, permissions
├── openapi.yaml      # OpenAPI 3.1
└── semantics.md      # behaviour OpenAPI cannot express: auth, tenancy, idempotency, errors
```

The registry stores manifest + OpenAPI + semantics and computes `digest = sha256(length-prefixed canonical manifest, openapi, semantics)`.
Rules enforced by `POST /v0/contracts`:

* same name+version, same digest ⇒ `200` (idempotent); different digest ⇒ `409` (a database trigger also forbids mutation);
* backward compatibility within a major against the closest lower published version (`platform-contracts::compat`: removed operations/responses/properties,
  retyped properties, newly required parameters or request properties) ⇒ else `422 breaking_change_within_major`; a breaking spec is accepted only as a new major;
* the structural check does **not** prove semantic compatibility; `semantics.md` still needs human review.

`contract-tool check|digest|request` exposes the same logic to CI and scripts.

## Implementations claim digests

An implementation registers `{contract, version, digest}`; the registry rejects unpublished versions and digest mismatches (`422`). The sample service embeds its contract files at
build time and computes the digests it claims. A claim is **not** proof of conformance (`verification_status = claimed`); production rollout policy should require CI conformance evidence.
The registry never executes test code.

## Resolution algorithm (`POST /v0/contracts/resolve`)

Input: `contract`, `version_constraint` (e.g. `>=1.0.0 <2.0.0`), optional `environment`.

1. identity ← verified `client_id`; application must be `active` (suspension denies immediately);
2. the application must have declared the requirement **and** hold an active grant, else one uniform `403` (no existence leak);
3. bindings for (application, contract, environment), highest major first; for each, the highest published version in that major that the bound implementation implements **and**
   that satisfies the constraint; none bound ⇒ `422 binding_not_found`; none compatible ⇒ `422 no_compatible_version`;
4. implementation `active` and deployment `healthy`, else `503 no_healthy_deployment` with `Retry-After` — never another implementation;
5. response: `{contract, resolved_version, protocol, endpoint, permissions, expires_in_seconds}`.

Endpoints are stable logical names. Registration rejects IP literals, `localhost`, metadata names, non-http(s) schemes, embedded credentials and queries
(unless an explicit host allowlist is configured); probing resolves DNS first and refuses loopback/link-local/multicast targets for non-allowlisted hosts and never follows redirects.
The registry never fetches caller-supplied OpenAPI URLs.

## SDK semantics (`sdk/go`)

* cache key `(contract, constraint, environment)` per `Client` (one credential); never shared across credentials;
* TTL from the response; bounded retry with jitter on transient registry failures (502/504/503 except `no_healthy_deployment`);
* bounded stale-if-error (default 60 s) only on registry outage; **never** after `403`/`422`/`no_healthy_deployment`;
* registry tokens (audience `midgard-core`) and implementation tokens (audience = contract) are different; the SDK requests each per audience;
* `Call` never replays non-idempotent requests; `DoOptions.Idempotent` permits one retry after a transport error.
