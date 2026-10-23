# platform.example 1.0.0 — semantics

Synthetic contract for verification; it carries no business meaning.

* **Authentication.** Every call carries an application workload token (`Authorization: Bearer`) whose
  audience is the implementation. Implementations verify it themselves; a resolved endpoint is not authorization.
* **Authorization.** The calling application must hold an active grant for `platform.example` and be bound to the
  serving implementation. `echo` and `listNotes` need `example:read`; `createNote` needs `example:write`.
* **Tenancy.** `/v1/notes` is tenant-owned. The caller forwards the end user's access token in
  `X-Delegated-User-Token`; the tenant is derived from verified claims. Tenant identifiers supplied by the caller are ignored.
* **Errors.** RFC 9457 `application/problem+json` with stable `code`s. 401 = bad/missing token, 403 = not authorized.
* **Idempotency.** `echo` and `listNotes` are safe to retry. `createNote` is **not** idempotent; clients must not replay it blindly.
* **Compatibility.** Backward compatible within a major version. Breaking changes require `v2`.
