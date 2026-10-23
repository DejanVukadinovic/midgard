-- MIDGARD v0.2: multi-protocol capability contracts. Additive; v0 contracts/implementations are untouched.
-- A v0.2 contract is a row in `contracts` (manifest->>'midgard' = '0.2', protocol 'multi') plus one
-- immutable row per interface here. Deployment-specific locations live in `implementation_interfaces`.

CREATE TABLE contract_interfaces (
  contract_id     UUID NOT NULL REFERENCES contracts(id) ON DELETE RESTRICT,
  interface_id    TEXT NOT NULL CHECK (interface_id ~ '^[a-z][a-z0-9-]{0,31}$'),
  protocol        TEXT NOT NULL CHECK (protocol IN ('http','mq')),
  spec_type       TEXT NOT NULL CHECK (spec_type IN ('openapi','asyncapi')),
  operation       TEXT NOT NULL,
  document        TEXT NOT NULL,          -- normalized (canonical JSON) interface description
  document_digest TEXT NOT NULL,
  callers         TEXT[] NOT NULL CHECK (cardinality(callers) > 0 AND callers <@ ARRAY['service','user']),
  summary         JSONB NOT NULL,
  PRIMARY KEY (contract_id, interface_id)
);

CREATE FUNCTION contract_interfaces_immutable() RETURNS trigger AS $$
BEGIN
  RAISE EXCEPTION 'published contract interfaces are immutable';
END $$ LANGUAGE plpgsql;
CREATE TRIGGER contract_interfaces_immutable BEFORE UPDATE OR DELETE ON contract_interfaces
  FOR EACH ROW EXECUTE FUNCTION contract_interfaces_immutable();

CREATE TABLE implementation_interfaces (
  implementation_id UUID NOT NULL REFERENCES implementations(id) ON DELETE RESTRICT,
  contract_id       UUID NOT NULL,
  interface_id      TEXT NOT NULL,
  location          TEXT,                 -- http: base URL (method/path come from OpenAPI)
  destination       TEXT,                 -- mq: logical destination; broker mapping is deployment metadata
  broker_binding    TEXT,                 -- mq: name of the deployment's broker binding
  updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (implementation_id, contract_id, interface_id),
  FOREIGN KEY (contract_id, interface_id) REFERENCES contract_interfaces(contract_id, interface_id) ON DELETE RESTRICT,
  FOREIGN KEY (implementation_id, contract_id) REFERENCES implementation_contracts(implementation_id, contract_id) ON DELETE CASCADE,
  CHECK ((location IS NOT NULL) <> (destination IS NOT NULL)),
  CHECK (destination IS NULL OR broker_binding IS NOT NULL)
);

GRANT SELECT, INSERT, UPDATE, DELETE ON implementation_interfaces TO midgard_runtime;
GRANT SELECT, INSERT ON contract_interfaces TO midgard_runtime;
