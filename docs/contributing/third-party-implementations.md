# Implementing a published contract

Anyone may implement a published MIDGARD contract; that does **not** make them a trusted platform administrator.

1. Read the contract package (`contracts/<ns>/<name>/v<major>/<version>/`): `openapi.yaml` **and** `semantics.md` (auth, tenancy, idempotency, errors).
2. Generate server stubs from the OpenAPI document, or follow `services/example-capability` (Rust/Axum) as a reference.
3. Authenticate every call yourself: verify the workload token with audience = your service, then ask core `POST /v0/authorize` (with your deployment identity) whether the
   calling application is granted and bound, and for tenant-owned data obtain the tenant from core — never from caller-supplied identifiers. Use PostgreSQL RLS for tenant data.
4. Claim exactly the contract digest you built against (`contract-tool digest <dir>`); expose `GET /health/ready`.
5. Run the contract's conformance tests locally (`contracts/<…>/tests`, when provided) and your own authorization tests.
6. Registration in a deployment is **operator-approved**: the deployment owner adds your identity with `POST /v0/deployment-identities` (slug-scoped), you register via `POST /v0/implementations`,
   and the owner decides grants and bindings. **Conformance is not authorization**, and passing tests grants no right to register in anyone else's deployment.

Out of scope: marketplace, automatic download/execution of third-party code, untrusted-code sandboxing.
