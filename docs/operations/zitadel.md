# ZITADEL integration status and Phase-0 spike

**Honest status:** the platform's token verification, claim parsing and provisioning flow are implemented and tested against a *test IdP* that emits ZITADEL-shaped tokens
and against a fake ZITADEL management API. They have **not been run against a live, pinned ZITADEL release** in this repository's CI. Treat every item below as unverified until the
evidence column is filled in.

## Spike checklist (do this before relying on ZITADEL)

| # | Question | Needed for | Evidence |
|---|---|---|---|
| 1 | Pin a release; does the official Compose init/start flow (`start-from-init`, master key, external domain/port, TLS mode) work behind our edge? Resource footprint? | ENV-01, IAM-01 | — |
| 2 | Create provider org, projects `obroklik`/`fleet`, customer orgs A/B, project grants; capture real **access-token claims** (JWT access-token type enabled, role assertion on) | IAM-01..03 | — |
| 3 | A home-org user with an extra Fleet grant in org B: does the role claim list org B under the project roles claim (shape assumed in ADR-002)? | IAM-06 | — |
| 4 | Is token exchange/delegation available and sufficient? If not, keep explicit user-token forwarding (current design) | IAM-09 | — |
| 5 | Machine users/clients for operator, publisher, deployment identities: which claims (`sub`, `client_id`) do their tokens carry? | APP-02/03, SVC-02/03 | — |
| 6 | Audiences: how to request `midgard-core` vs the implementation audience (`urn:zitadel:iam:org:project:id:{id}:aud` scope) | RES-10 | — |
| 7 | Revocation: after removing a user grant/role, how long until new tokens lack it; access-token lifetime setting | IAM-12 | — |
| 8 | Key rotation and `kid` refresh behaviour (we refresh JWKS on unknown `kid`) | IAM-13 | — |
| 9 | Management API paths and payloads used by `ZitadelClient` (`/management/v1/projects/_search`, `/projects`, `/projects/{id}/apps/oidc`, `/apps/api`) for the pinned version | APP-04, IAM-01 | — |

Record outcomes here (claims samples with identifiers redacted), update ADR-002, and add automated tests that run against the real container in a dedicated CI job.

## Using ZITADEL with the Compose stack (template)
`docker compose --profile zitadel` starts a ZITADEL service configured from `.secrets/zitadel.env`. It is a template: complete the spike, then set
`IDP_ISSUER`, `IDP_TOKEN_URL`, `PLATFORM_OPERATOR_SUBJECTS` etc. to the real values, set `PLATFORM_PROVISIONER=zitadel` with a least-privilege service-user token in `ZITADEL_API_TOKEN_FILE`,
and use `scripts/bootstrap.sh` with the operator/publisher client credentials issued by ZITADEL (link mode ids come from the provisioned projects/clients).
