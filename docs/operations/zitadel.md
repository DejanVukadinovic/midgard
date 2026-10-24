# ZITADEL integration: status and evidence

**Pinned release: ZITADEL v2.71.8.** The scenarios in `tests/acceptance` and `tests/integration` run green against the
real release binary (PostgreSQL 16, local processes): `make acceptance-zitadel` (CI job `zitadel-local`).
**Not yet exercised: the ZITADEL *container* in Compose** (no Docker daemon was available when this was written) and the
core's `PLATFORM_PROVISIONER=zitadel` management-API provisioner (all runs use `link` mode with ids from
`scripts/zitadel-bootstrap.sh`).

## Spike results

| # | Question | Result (observed on v2.71.8) |
|---|---|---|
| 1 | Supported init/start flow | `zitadel start-from-init --masterkey <32 chars> --tlsMode disabled` works against a PostgreSQL admin login. `ZITADEL_FIRSTINSTANCE_ORG_MACHINE_*` creates an admin machine user and writes its PAT to `ZITADEL_FIRSTINSTANCE_PATPATH`. Needs a machine identifier: in environments without a private IP set `ZITADEL_MACHINE_IDENTIFICATION_HOSTNAME_ENABLED=true` (otherwise startup panics). `/debug/ready` can answer before the API gateway: wait for an authenticated admin call. v2.66.4 (the previous pin) lacks the `loginV2` feature the headless user tokens below depend on. |
| 2 | Orgs, projects, grants, real claims | Created by `scripts/zitadel-bootstrap.sh` with `projectRoleAssertion=true`: projects `midgard-core`, `platform-example`, `example-echo`, `obroklik`, `fleet`; orgs `org-acme`, `org-globex`; project grants and user grants. Tokens carry `urn:zitadel:iam:org:project:{pid}:roles` (and a project-less `…:project:roles`). |
| 3 | Role claim shape for extra-org grants | **Confirmed** `{"role": {"<orgId>": "<primary domain>"}}`, org ids listed under the role, including a user granted in a second organization through that organization's project grant (a user may be granted in an org other than their own). Matches `platform-auth::tenancy`. |
| 4 | Token exchange / delegation | Not evaluated. The design keeps explicit user-token forwarding (`/v0/authorize` `user_token`). |
| 5 | Machine users | Client-credentials with `accessTokenType=JWT`: `sub` = user id, `client_id` = **user name**, `aud` = requested project ids. Operators/publishers are therefore configured by user id (`PLATFORM_OPERATOR_SUBJECTS`), written by the bootstrap to `idp.env`. Client secrets are generated once and kept in `secrets.env`. |
| 6 | Audiences | The generic `audience` form parameter is ignored. Request `scope=openid urn:zitadel:iam:org:project:id:<projectId>:aud`; `aud` then contains that project id (all-digit audiences are sent as scopes by the Go SDK, the Go fixture, the Rust service and `scripts/lib.sh`). Services verify `aud` = their project id. |
| 7 | Revocation latency | Not measured. Access-token lifetime is set to 1 h by the bootstrap (Core rejects tokens whose `exp-iat` exceeds `PLATFORM_TOKEN_MAX_LIFETIME_SECONDS`, default 3600; ZITADEL's default is 12 h). Grant revocation in MIDGARD is immediate (checked per invocation). |
| 8 | Key rotation / `kid` refresh | Not tested against ZITADEL (covered against the test IdP). |
| 9 | Management API paths | v1 management/admin APIs used by the bootstrap are listed in `scripts/zitadel-bootstrap.sh` (search-then-create; project-grant listing has no org filter, so the script filters client-side). |

## Behaviours that required code changes

* **JWT `typ`.** ZITADEL's JWT access tokens have header `typ: JWT` (not `at+jwt`); ID tokens share it. With
  `PLATFORM_ACCEPT_JWT_TYP=true` (and `CAPABILITY_ACCEPT_JWT_TYP`) the verifier accepts `typ: JWT` but still rejects tokens carrying
  `at_hash`, `nonce`, `amr` or `auth_time`, and the audience check still applies (an ID token's audience is the client, not Core).
  Default stays strict.
* **Home organization.** Without a selected org the core uses the `urn:zitadel:iam:user:resourceowner:id` claim, which ZITADEL emits only
  when the client requests `scope=urn:zitadel:iam:user:resourceowner`. Applications must request it.
* **Headless end-user tokens (tests).** `scripts/zitadel-user-token.sh` completes an authorization-code + PKCE flow without a browser:
  instance feature `loginV2` is enabled (the redirect target need not exist), then a least-privilege `IAM_LOGIN_CLIENT` machine user creates
  a session with the user's password and finalizes the auth request through the v2 API. Real users, real tokens; not for production use.
* **Issuer routing.** ZITADEL resolves the instance from the request `Host`. In Compose the issuer is `http://zitadel:8080`; every client,
  including host tools, must use that name (or send that `Host` header, `ZITADEL_HOST`).

## Compose (default profile `zitadel`)

`zitadel` (init + start), `zitadel-bootstrap` (one-shot, idempotent) and the `idp` volume that carries ids, audiences and secrets to the services
(see [compose-quickstart](../compose-quickstart.md)). The admin PAT lives only in the `zitadel-admin` volume mounted by the bootstrap job.
Dev passwords and client secrets in the `idp` volume are throwaway development credentials.

## Production notes (not done)

TLS in front of ZITADEL and a real external domain; private-key-JWT client authentication instead of client secrets; no login-v2 flag, no
password-based test users, no world-readable `idp` volume; scoped service-user tokens for `PLATFORM_PROVISIONER=zitadel`; image digests.
