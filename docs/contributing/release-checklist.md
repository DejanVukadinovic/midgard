# Release checklist (OSS-16)

Run before tagging; attach evidence (CI run links) to the release notes.

- [ ] All required CI checks green on the default branch: `ci` (rust, go, contracts, compose-config, acceptance-local, acceptance-compose) and `security`
- [ ] Clean-volume install: `FRESH=1 tests/acceptance/compose.sh` passes; restart/recovery pass; no direct database edits were used
- [ ] `docs` accuracy: README status table matches reality; every README command ran in CI (`acceptance-compose`); `scripts/check-docs.sh` passes; ZITADEL status stated honestly
- [ ] ZITADEL Phase-0 spike items in [operations/zitadel.md](../operations/zitadel.md) completed or the limitation still prominently documented
- [ ] Dependency licenses and advisories: `cargo deny check`, `govulncheck`; NOTICE entries added for dependencies that require them; bundled assets checked
- [ ] Container images: Trivy scan; image **digests** pinned in `.env.example`/compose; non-root, read-only rootfs verified
- [ ] Secrets: gitleaks over **full Git history** clean; no `.env`, keys, dumps in the tree; example credentials clearly labelled non-production
- [ ] Contracts: published versions unchanged (`make contracts-check`, registry immutability); SDK generation drift check clean
- [ ] Threat model reviewed; `SECURITY.md` contact channel works
- [ ] `CHANGELOG.md` updated; tag build reproducible (`Cargo.lock`/`go.sum` committed, toolchains pinned); checksums published
- [ ] GitHub Actions pinned to commit SHAs (Dependabot keeps them current); branch protection requires CI + review
