-- MIDGARD core registry schema. Soft-disable instead of delete; published contracts are immutable.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Group role the runtime login role must be a member of (created by deployment init scripts).
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'midgard_runtime') THEN
    CREATE ROLE midgard_runtime NOLOGIN NOBYPASSRLS;
  END IF;
END $$;

CREATE TABLE applications (
  id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  slug               TEXT NOT NULL UNIQUE CHECK (slug ~ '^[a-z][a-z0-9-]{1,31}$'),
  display_name       TEXT NOT NULL,
  zitadel_project_id TEXT UNIQUE,
  status             TEXT NOT NULL CHECK (status IN ('pending','active','suspended','failed')),
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE oauth_clients (
  id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  application_id   UUID NOT NULL REFERENCES applications(id) ON DELETE RESTRICT,
  zitadel_client_id TEXT NOT NULL UNIQUE,
  kind             TEXT NOT NULL CHECK (kind IN ('web','mobile','backend')),
  status           TEXT NOT NULL CHECK (status IN ('active','disabled')) DEFAULT 'active',
  created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX oauth_clients_app_idx ON oauth_clients(application_id);

CREATE TABLE tenants (
  id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  zitadel_organization_id TEXT NOT NULL,
  application_id          UUID NOT NULL REFERENCES applications(id) ON DELETE RESTRICT,
  status                  TEXT NOT NULL CHECK (status IN ('active','suspended')),
  created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (zitadel_organization_id, application_id)
);

CREATE TABLE contracts (
  id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name          TEXT NOT NULL,
  version       TEXT NOT NULL,
  major         INT  NOT NULL,
  minor         INT  NOT NULL,
  patch         INT  NOT NULL,
  digest        TEXT NOT NULL,
  manifest      JSONB NOT NULL,
  openapi       TEXT NOT NULL,
  semantics     TEXT NOT NULL,
  permissions   TEXT[] NOT NULL DEFAULT '{}',
  status        TEXT NOT NULL CHECK (status IN ('published','deprecated')) DEFAULT 'published',
  published_by  TEXT NOT NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (name, version)
);

-- Published payloads are immutable (only `status` may change).
CREATE FUNCTION contracts_immutable() RETURNS trigger AS $$
BEGIN
  IF TG_OP = 'DELETE' THEN RAISE EXCEPTION 'published contracts cannot be deleted'; END IF;
  IF NEW.name <> OLD.name OR NEW.version <> OLD.version OR NEW.digest <> OLD.digest
     OR NEW.manifest <> OLD.manifest OR NEW.openapi <> OLD.openapi OR NEW.semantics <> OLD.semantics
     OR NEW.permissions <> OLD.permissions THEN
    RAISE EXCEPTION 'published contract payload is immutable';
  END IF;
  RETURN NEW;
END $$ LANGUAGE plpgsql;
CREATE TRIGGER contracts_immutable BEFORE UPDATE OR DELETE ON contracts
  FOR EACH ROW EXECUTE FUNCTION contracts_immutable();

CREATE TABLE implementations (
  id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  slug            TEXT NOT NULL UNIQUE CHECK (slug ~ '^[a-z][a-z0-9-]{1,63}$'),
  release_version TEXT NOT NULL,
  status          TEXT NOT NULL CHECK (status IN ('active','disabled')) DEFAULT 'active',
  owner_identity  TEXT NOT NULL,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE implementation_contracts (
  implementation_id   UUID NOT NULL REFERENCES implementations(id) ON DELETE RESTRICT,
  contract_id         UUID NOT NULL REFERENCES contracts(id) ON DELETE RESTRICT,
  contract_digest     TEXT NOT NULL,
  verification_status TEXT NOT NULL CHECK (verification_status IN ('claimed','verified')) DEFAULT 'claimed',
  PRIMARY KEY (implementation_id, contract_id)
);

CREATE TABLE deployments (
  id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  implementation_id    UUID NOT NULL REFERENCES implementations(id) ON DELETE RESTRICT,
  environment          TEXT NOT NULL CHECK (environment ~ '^[a-z][a-z0-9-]{0,31}$'),
  logical_endpoint     TEXT NOT NULL,
  status               TEXT NOT NULL CHECK (status IN ('registered','healthy','degraded','unavailable','disabled')) DEFAULT 'registered',
  consecutive_failures INT NOT NULL DEFAULT 0,
  last_health_at       TIMESTAMPTZ,
  created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (implementation_id, environment)
);

-- Which identities may register which implementation slugs ("prefix*" patterns allowed).
CREATE TABLE deployment_identities (
  id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  subject      TEXT NOT NULL,
  slug_pattern TEXT NOT NULL CHECK (slug_pattern ~ '^[a-z][a-z0-9-]*\*?$'),
  status       TEXT NOT NULL CHECK (status IN ('active','revoked')) DEFAULT 'active',
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (subject, slug_pattern)
);

CREATE TABLE application_requirements (
  application_id     UUID NOT NULL REFERENCES applications(id) ON DELETE RESTRICT,
  contract_name      TEXT NOT NULL,
  version_constraint TEXT NOT NULL,
  PRIMARY KEY (application_id, contract_name)
);

CREATE TABLE contract_grants (
  id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  application_id UUID NOT NULL REFERENCES applications(id) ON DELETE RESTRICT,
  contract_name  TEXT NOT NULL,
  permissions    TEXT[] NOT NULL,
  status         TEXT NOT NULL CHECK (status IN ('active','revoked')) DEFAULT 'active',
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  revoked_at     TIMESTAMPTZ
);
CREATE UNIQUE INDEX contract_grants_active_uq ON contract_grants(application_id, contract_name) WHERE status = 'active';

CREATE TABLE contract_bindings (
  application_id    UUID NOT NULL REFERENCES applications(id) ON DELETE RESTRICT,
  contract_name     TEXT NOT NULL,
  major_version     INT  NOT NULL,
  environment       TEXT NOT NULL,
  implementation_id UUID NOT NULL REFERENCES implementations(id) ON DELETE RESTRICT,
  updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (application_id, contract_name, major_version, environment)
);

-- Durable, idempotent provisioning operations.
CREATE TABLE provisioning_operations (
  id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  idempotency_key TEXT NOT NULL UNIQUE,
  request_hash    TEXT NOT NULL,
  kind            TEXT NOT NULL,
  desired_state   JSONB NOT NULL,
  step            TEXT NOT NULL DEFAULT 'created',
  status          TEXT NOT NULL CHECK (status IN ('pending','succeeded','failed')) DEFAULT 'pending',
  error           TEXT,
  application_id  UUID REFERENCES applications(id),
  actor           TEXT NOT NULL,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

GRANT SELECT, INSERT, UPDATE ON
  applications, oauth_clients, tenants, implementations, implementation_contracts, deployments,
  deployment_identities, application_requirements, contract_grants, contract_bindings,
  provisioning_operations TO midgard_runtime;
GRANT SELECT, INSERT, UPDATE ON contracts TO midgard_runtime;
GRANT DELETE ON implementation_contracts, deployments, contract_bindings, application_requirements TO midgard_runtime;
