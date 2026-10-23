# Security Policy

## Supported versions

MIDGARD is pre-1.0. Only the latest tagged release and the default branch receive security fixes.

## Reporting a vulnerability

**Please do not open a public issue.** Report privately through GitHub's
[private vulnerability reporting](https://github.com/dejanvukadinovic/midgard/security/advisories/new)
for this repository (Security → Report a vulnerability). Include affected version/commit, impact, and a
reproduction (redact any credentials). You can expect an acknowledgement within 5 working days; we will
coordinate a fix and disclosure timeline with you and credit you if you wish. There is no bug bounty and no SLA.

If you cannot use GitHub's reporting, open an issue titled "security contact request" *without details*
and a maintainer will arrange a private channel.

## Scope notes

* The offline `dev-idp`, the `dev-only-*` fixtures and the Compose stack are **development defaults**, not a
  hardened production template. Findings that only apply when running those in production are documentation
  issues, not vulnerabilities.
* Cross-tenant data access, identity/application spoofing, unauthorized registration/grant/binding, SSRF via
  endpoint registration, and token-validation bypasses are in scope. See [`docs/security/threat-model.md`](docs/security/threat-model.md).
