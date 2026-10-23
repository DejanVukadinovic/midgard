# MIDGARD — v0.1 Implementation Notes

**Subtitle:** Modular Infrastructure for Dynamic Governance and Rapid Development  
**Status:** Addendum to `PLATFORM_V0_IMPLEMENTATION_PLAN.md`  
**Scope:** Open-source readiness, reproducible deployment, contributor-facing contracts, and public-project hygiene  
**Baseline:** v0 architecture and functionality remain in force unless explicitly amended below.

## 1. Decision record

MIDGARD will be developed **in a public GitHub repository from the beginning**. It is an open-source, contract-driven application platform used internally to accelerate delivery of products such as Obroklik and Fleet Management. The platform is **not** being commercialized as a hosted developer platform in v0/v0.1; its roadmap remains driven by real application needs.

**Decisions:**

1. **Project identity:** `MIDGARD`; repository name `midgard` (subject to availability). Expansion: **Modular Infrastructure for Dynamic Governance and Rapid Development**.
2. **License:** Apache License 2.0 for MIDGARD-owned code, subject to checking dependency and bundled-asset licenses before publication. Include the full `LICENSE` text and any required notices.
3. **Public by default:** Source, architecture decisions, example configurations, contracts, tests, and deployment instructions are public. No customer data, proprietary application code, production configuration, or credentials enter the repository.
4. **Internal-first governance:** External contributions may be accepted, but maintainers control scope, releases, compatibility commitments, and roadmap. No obligation to support arbitrary external integrations or third-party deployments beyond documented guarantees.
5. **Local-first reproducibility:** A clean checkout must run the complete v0 development/demo stack with Docker Compose and no paid external dependencies.
6. **Demonstrable architecture:** The repository must contain an independent sample application and sample contract implementation exercising registration, grants, resolution, authentication, and tenant isolation.
7. **CI-backed claims:** Security and functional properties advertised in the README must have automated tests or be explicitly labeled as planned/not yet verified.
8. **Versioning:** Until a stable 1.0 release, platform APIs may evolve, but published **contract versions remain immutable**. Breaking contract changes require a new major version. Pre-1.0 platform API compatibility guarantees must be documented separately.

## 2. Relationship to the v0 plan

This is an **incremental implementation addendum**, not a redesign. Preserve the established v0 architecture:

- Rust/Axum platform core with IAM integration, application registry, contract registry, service implementation registry, grants, bindings, and resolver.
- ZITADEL as the external identity provider; one home organization per user, with future cross-organization grants.
- ZITADEL organization + product/project mapped to an internal application-scoped `tenant_id`; PostgreSQL tenant isolation and RLS.
- Applications consume **versioned contracts**, never hard-coded service identities. Multiple implementations may register; authorized bindings determine which implementation is selected.
- Docker Compose first. Resolver returns stable implementation endpoints; orchestration/load balancing are delegated to deployment infrastructure. Kubernetes remains a future option.
- Dynamic registration requires **authorized operator/deployment identities**, not anonymous self-registration. Public source code does not imply publicly writable registries.
- No third-party application marketplace, self-service hosted offering, Kubernetes control plane, or general plugin sandbox in v0.1.

**Terminology change:** In docs and examples, replace generic references to “Platform v0” with **“MIDGARD v0”** where appropriate. Do not rename stable identifiers mechanically once users may depend on them.

## 3. Proposed public repository layout

```text
midgard/
├── README.md
├── LICENSE
├── NOTICE                       # only if required
├── CONTRIBUTING.md
├── CODE_OF_CONDUCT.md            # optional at launch
├── SECURITY.md
├── CHANGELOG.md
├── .gitignore
├── .gitattributes
├── .editorconfig
├── .env.example
├── .github/
│   ├── workflows/
│   │   ├── ci.yml
│   │   ├── security.yml
│   │   └── release.yml           # enable when releases exist
│   ├── ISSUE_TEMPLATE/
│   └── pull_request_template.md
├── docs/
│   ├── getting-started.md
│   ├── architecture/
│   │   ├── overview.md
│   │   ├── auth-and-tenancy.md
│   │   └── contracts-and-resolution.md
│   ├── adr/
│   ├── operations/
│   ├── security/
│   └── contributing/
├── contracts/
│   └── examples/
├── crates/
├── core/
├── sdk/
│   └── go/
├── deployments/
│   └── compose/
├── examples/
│   ├── sample-application/
│   └── sample-service/
├── scripts/
└── tests/
    ├── integration/
    ├── acceptance/
    └── contract-conformance/
```

This is a target structure, not a mandate to create empty placeholder directories. Keep existing v0 module boundaries unless there is a concrete reason to change them.

## 4. Public installation and bootstrap

### 4.1 One documented local path

A contributor with Docker Compose and the language toolchains documented in `CONTRIBUTING.md` should be able to:

```bash
git clone <repository-url>
cd midgard
cp .env.example .env
# Run the documented bootstrap command, e.g. make dev-up
# Run the documented smoke/acceptance command, e.g. make acceptance
```

The precise command names may differ; **there must be one canonical, tested quickstart**. Do not include fabricated URLs, example secrets that also work in production, or undocumented manual steps.

### 4.2 Compose stack

Keep the v0 Compose deployment and add what is needed for reproducible demos:

- PostgreSQL with persistent named volume and deterministic migrations.
- ZITADEL with documented local configuration, health checks, and automated/bootstrap provisioning.
- MIDGARD Core and any supporting services already required by the v0 plan.
- A sample application and sample service, optionally enabled through a Compose profile to keep the default footprint modest.
- Stable Compose DNS names and health-aware startup; no dependency on fixed container IPs.
- A local development environment requiring **no commercial SaaS accounts**.

Pin image versions/digests for repeatability; document resource requirements and ports. Use local-only defaults, non-production credentials explicitly labeled as such, and bind sensitive administration endpoints to localhost where practical. Do not imply Compose's startup ordering alone proves readiness; use health checks and retry-safe initialization.

### 4.3 Idempotent bootstrap

Provide a bootstrap command that can be safely rerun. It must:

1. Initialize the ZITADEL instance using supported administrative/bootstrap mechanisms.
2. Provision or reconcile the MIDGARD operator/deployment identity.
3. Provision demo product projects, OAuth clients, organizations, and grants required for the sample.
4. Initialize the platform registry and tenant mappings through supported APIs/migrations.
5. Register the example contract, example service implementation, and example application.
6. Grant the example application the minimum required contract permissions and bind an implementation.
7. Emit non-sensitive identifiers and local instructions; store secrets only in ignored local files or an appropriate secrets mechanism.

Handle partial failures and reruns without duplicate organizations, clients, tenants, or bindings. **Validate actual ZITADEL APIs, project grants, token claims, and delegated-authorization behavior in a prototype** before treating this as complete.

### 4.4 Fresh-install verification

Run a fresh-start acceptance job using empty volumes and a clean checkout. Separately test restart/recovery with populated volumes. Both should pass without direct database edits.

## 5. Public example: contract → implementation → application

Provide a synthetic, vendor-neutral example unrelated to Obroklik or Fleet business logic.

Suggested contract: `examples.echo.v1` or `examples.tasks.v1`. Prefer a simple contract for bootstrapping; a second tenant-owned resource example can demonstrate RLS.

**Required flow:**

1. Register/publish a versioned contract package (OpenAPI schema, semantic notes, compatibility metadata).
2. Deploy a sample Rust service implementing that contract.
3. Register its implementation and stable endpoint using an authorized deployment identity.
4. Register a sample Go application/facade with a ZITADEL-backed workload identity.
5. Grant the application access to the contract and bind the chosen implementation.
6. Resolve the contract using the Go SDK; invoke it without embedding the service name or endpoint in application code.
7. Deny resolution for an ungranted application.
8. Demonstrate separate tenant contexts and verify cross-tenant reads/writes are denied for tenant-owned data.
9. Restart the core and sample service; demonstrate persistence and successful resolution afterward.

The sample should show **both** application identity and human/tenant authorization where relevant. Do not treat a client-credentials token as proof of a user's organization membership. Tests must exercise the real delegated authorization/tenant-resolution path rather than trusting caller-supplied headers.

## 6. Contracts as public extension points

External contributors may implement published MIDGARD contracts without becoming trusted platform administrators.

- Contract packages are public and independently versioned; include OpenAPI 3.1, event schemas if applicable, semantics, auth scopes, idempotency/error behavior, and compatibility rules.
- Published versions are immutable. New versions are additive where possible; breaking changes create a new major version.
- Provide generated clients and/or generation instructions for the Go SDK; avoid handwritten client behavior diverging from the contract.
- Provide a reusable **contract conformance test suite** runnable against a local implementation. CI verifies schema compatibility and essential behavior.
- **Conformance is not authorization:** passing tests does not grant a service permission to register in someone else's MIDGARD deployment.
- Registration still requires an operator-approved workload identity; each deployment owner controls trusted implementations and application-to-contract bindings.
- Keep v0's explicit binding policy: never randomly route tenant data between independently stateful implementations just because they implement the same contract.

For v0.1, document how to develop a third-party implementation locally. Do **not** build a marketplace, automatic remote-code installer, or untrusted execution sandbox.

## 7. Documentation requirements

### 7.1 README (first screen)

Include:

- **MIDGARD** name and acronym expansion.
- One-sentence technical purpose: *contract-driven infrastructure for reusable, multi-tenant application capabilities*.
- Clear **current status** and working features versus roadmap.
- Compact architecture diagram: application → contract resolver → authorized implementation → deployment endpoint; ZITADEL and tenant isolation alongside.
- Tested quickstart with Docker Compose.
- Reproducible example of registering a contract, application, and service.
- Links to architecture, API contracts, acceptance tests, security policy, license, and contribution guide.

Do not describe future Kubernetes, Yggdrasil, Heimdall, or Bifröst ideas as implemented functionality. Those names may appear only in a clearly labeled future-ideas section, if at all.

### 7.2 Architecture Decision Records

At minimum:

| ADR | Decision to document |
|---|---|
| ADR-001 | Rust core + Go application facades; why not a single-language monolith |
| ADR-002 | ZITADEL-backed IAM; one home organization per user |
| ADR-003 | Organization + product/project → internal tenant ID; PostgreSQL RLS |
| ADR-004 | Contract-first dependencies; contracts distinct from services/deployments |
| ADR-005 | Explicit application/contract grants and implementation bindings |
| ADR-006 | Client-side resolution and stable logical endpoints |
| ADR-007 | Docker Compose first; Kubernetes-ready separation of concerns |
| ADR-008 | Public Apache-2.0 project with internally driven roadmap |

Each ADR should include context, decision, alternatives, tradeoffs, and consequences. Document uncertainties rather than claiming unsupported ZITADEL behavior.

### 7.3 Operations and security docs

Document configuration, secret handling, backup/restore, migrations, token verification, tenant context, RLS, health checks, logging, and supported upgrade paths. Include a threat model covering spoofed application identity, forged tenant selection, compromised registration credentials, malicious endpoint registration, and unauthorized contract binding.

## 8. Public repository security and hygiene

- Never commit `.env`, OAuth client secrets, private keys, customer records, database dumps, production URLs with credentials, or internal infrastructure inventory.
- Add `.gitignore`, example configuration, and automated secret scanning. Scan **Git history** before initial publication; deleting a secret in a later commit does not remove its exposure.
- Use least-privilege CI credentials; prefer ephemeral/OIDC-based publishing credentials when available.
- Protect the default branch with required tests and review checks; use CODEOWNERS if multiple maintainers emerge.
- Add `SECURITY.md` with a private vulnerability-reporting channel and supported versions. Do not ask reporters to publish exploitable issues immediately.
- Add dependency and license checks for Rust, Go, container images, and generated artifacts. Track and patch vulnerabilities according to severity.
- Produce reproducible tagged releases and checksums/artifact provenance when publishing binaries or images; no requirement to build a release pipeline before the first usable v0 demo.
- Clearly distinguish **development defaults** from production guidance. The local demo is not a secure production deployment template without additional configuration.
- Ensure logging and tracing redact credentials and personally identifying data. Demo fixtures must be synthetic.
- Include attribution and NOTICE material where required by dependencies and bundled resources.

## 9. Contribution model

`CONTRIBUTING.md` should cover development prerequisites, quickstart, formatting/linting, test commands, contract contribution workflow, ADR process, and pull request expectations. Prefer small, reviewable changes and tests for behavioral modifications.

**Maintainer policy:** Contributions are welcome but acceptance is discretionary and guided by the internal roadmap. Do not promise support SLAs, universal compatibility, or external hosting. State whether a Developer Certificate of Origin (DCO) sign-off is required; a CLA is unnecessary unless a concrete legal reason emerges. Clarify that contributions are submitted under the repository's license.

## 10. CI pipeline

**Pull-request required checks:**

1. Rust formatting (`cargo fmt --check`), lint (`cargo clippy`), unit tests, and dependency/license checks.
2. Go formatting, static analysis, unit tests, and SDK generation-drift checks.
3. Contract schema validation and backward-compatibility checks against published versions.
4. Docker Compose configuration validation and container build checks.
5. Integration tests against PostgreSQL and ZITADEL.
6. End-to-end example acceptance tests for registration, grants, resolution, invocation, tenant isolation, and negative authorization cases.
7. Secret scanning and baseline dependency vulnerability scanning.
8. Documentation quickstart checks (at minimum, automated command validation; ideally clean-install smoke tests).

Use pinned toolchain versions and caching to keep PR feedback reasonable. Full clean-volume installation, restart/recovery, and heavier security tests can run in a dedicated CI job or scheduled workflow if PR runtime becomes excessive, but must be run before tagging a release.

## 11. Additional acceptance tests (OSS-01 through OSS-16)

These supplement, rather than replace, the functional/security tests in the v0 plan.

| ID | Test | Pass condition |
|---|---|---|
| **OSS-01** | Clean checkout + Compose startup | Documented quickstart brings up the v0 stack on a fresh machine/runner with no manual database edits. |
| **OSS-02** | No paid dependencies | Demo and acceptance suite work without paid services or proprietary accounts. |
| **OSS-03** | Secret-free repository | Automated secret scan passes; example files contain only clearly marked non-production values. |
| **OSS-04** | CI coverage | Required PR checks execute and pass on the default branch. |
| **OSS-05** | Example application | A Go sample application resolves a contract and invokes it without hard-coded implementation identity/URL. |
| **OSS-06** | Example service | A sample service implements a published contract and is dynamically registered through an authorized deployment path. |
| **OSS-07** | Documentation accuracy | Every README quickstart command succeeds in CI or is explicitly verified in the release checklist. |
| **OSS-08** | License and contribution docs | `LICENSE` contains Apache-2.0; `CONTRIBUTING.md` and `SECURITY.md` exist and are actionable. |
| **OSS-09** | ZITADEL bootstrap | Clean bootstrap provisions necessary identity resources via supported APIs; rerunning is idempotent. |
| **OSS-10** | Platform independence | No private Obroklik/Fleet repositories, customer data, or proprietary business modules are required to build/test/run MIDGARD. |
| **OSS-11** | Unauthorized registration | Unknown/underprivileged identities cannot register or overwrite contracts, implementations, applications, grants, or bindings. |
| **OSS-12** | Unauthorized discovery/invocation | An ungranted application cannot resolve or invoke a protected contract even if it knows the service endpoint. |
| **OSS-13** | Tenant isolation | Cross-tenant read/write attempts fail using real auth context and PostgreSQL RLS; spoofed tenant headers are ignored/rejected. |
| **OSS-14** | Contract immutability | Republishing a released contract version with changed contents fails; incompatible changes require a new major version. |
| **OSS-15** | Recovery and repeatability | Core/IdP restart preserves registrations and access mappings; clean bootstrap and repeated bootstrap both pass. |
| **OSS-16** | Public release hygiene | Release checklist verifies dependency licenses, image/config safety, no secrets in Git history, documentation/status accuracy, and tagged build/test results. |

### Required end-to-end acceptance scenario

Use **two synthetic organizations**, **two registered applications**, and **one or more sample contract implementations**. Test: identity bootstrap → application registration → contract publication → implementation registration → grants and explicit bindings → authorized resolution and invocation → denied ungranted access → distinct internal tenants for the same organization across applications → cross-tenant denial → restart → successful recovery. Record test artifacts/logs with secrets redacted.

## 12. Implementation sequence (v0.1 delta)

These tasks should be interleaved with the existing v0 phases, not postponed until all platform features are complete.

| Stage | Work | Exit criterion |
|---|---|---|
| **A. Repository foundation** | Rename project to MIDGARD, add Apache-2.0 license, README skeleton, security/contribution policies, ignore rules, ADR template, CI baseline. | Public repo is safe to publish and builds in CI. |
| **B. Reproducible environment** | Compose stack, pinned versions, `.env.example`, health checks, migration/bootstrap scripts, clean-volume test. | OSS-01, OSS-02, OSS-03, OSS-09 pass. |
| **C. Contracts and samples** | Publish sample contract, implement sample service and Go facade, generate SDK, add registration/grant/resolution walkthrough. | OSS-05, OSS-06, OSS-10, OSS-14 pass. |
| **D. Security and conformance** | Automated auth negative tests, RLS tests, conformance suite, secrets/dependency scans. | OSS-11, OSS-12, OSS-13 pass. |
| **E. Release candidate** | Run full clean-install/restart suite, review docs and threat model, audit dependencies/licenses/history, tag release. | OSS-04, OSS-07, OSS-08, OSS-15, OSS-16 pass; existing v0 acceptance suite remains green. |

## 13. Explicitly deferred

- Public hosted MIDGARD service or SaaS billing.
- Unrestricted third-party app/service self-registration.
- Plugin marketplace, automated downloading/execution of third-party service code, or extension sandbox.
- Generalized external application integration layer (**Yggdrasil** is a possible future separate project).
- Standalone IAM replacement (**Heimdall** is a possible future separate project); ZITADEL remains v0's IdP.
- API gateway branding/project (**Bifröst**) unless a gateway becomes technically necessary.
- Kubernetes orchestration and load balancing (retain orchestration-neutral endpoints).
- External contributor SLAs, formal standards certification, or guarantees of long-term API stability before 1.0.

## 14. Definition of done

**MIDGARD v0.1 is complete when:** the existing v0 functional acceptance criteria pass; OSS-01–OSS-16 pass; a stranger can clone the public repository and reproduce the sample contract-to-service-to-application flow; no proprietary assets or secrets are required; and the documentation accurately distinguishes implemented features from future architecture.

The central standard is simple: **MIDGARD should be inspectable, runnable, testable, and understandable without access to our private applications.**
