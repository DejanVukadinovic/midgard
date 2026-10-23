# API stability (pre-1.0)

* **Contract versions** (`contracts/…`) are immutable once published; breaking changes require a new major. This is enforced by the registry, a DB trigger and CI.
* **Platform API (`/v0`)**: may change before 1.0. Changes are listed in the changelog; additive changes are preferred; removed or renamed fields get at least one minor release of notice where practical. `v0` in the path signals "pre-stable administrative interface".
* **Go SDK**: module `github.com/dejanvukadinovic/midgard/sdk/go` follows the platform's pre-1.0 policy; the generated contract clients follow their contract's versioning.
* **Database schema**: forward-only migrations; downgrades are restores.
* **Configuration keys**: documented in [operations/configuration.md](../operations/configuration.md); renames are announced in the changelog.
* No stability, support or compatibility guarantees are given to third-party deployments beyond what is documented here.
