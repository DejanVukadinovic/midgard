-- Append-only audit log.
CREATE TABLE audit_log (
  id         BIGSERIAL PRIMARY KEY,
  actor      TEXT NOT NULL,
  action     TEXT NOT NULL,
  target     TEXT NOT NULL,
  before     JSONB,
  after      JSONB,
  request_id TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE FUNCTION audit_append_only() RETURNS trigger AS $$
BEGIN
  RAISE EXCEPTION 'audit_log is append-only';
END $$ LANGUAGE plpgsql;
CREATE TRIGGER audit_no_update BEFORE UPDATE OR DELETE ON audit_log
  FOR EACH ROW EXECUTE FUNCTION audit_append_only();
CREATE TRIGGER audit_no_truncate BEFORE TRUNCATE ON audit_log
  FOR EACH STATEMENT EXECUTE FUNCTION audit_append_only();

GRANT SELECT, INSERT ON audit_log TO midgard_runtime;
GRANT USAGE ON SEQUENCE audit_log_id_seq TO midgard_runtime;

-- Minimal tenant-owned fixture proving RLS isolation for the shared persistence helpers.
CREATE TABLE tenant_fixture_notes (
  tenant_id UUID NOT NULL,
  id        UUID NOT NULL DEFAULT gen_random_uuid(),
  body      TEXT NOT NULL,
  PRIMARY KEY (tenant_id, id)
);
ALTER TABLE tenant_fixture_notes ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_fixture_notes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON tenant_fixture_notes
  USING (tenant_id = NULLIF(current_setting('app.tenant_id', true), '')::uuid)
  WITH CHECK (tenant_id = NULLIF(current_setting('app.tenant_id', true), '')::uuid);
GRANT SELECT, INSERT, UPDATE, DELETE ON tenant_fixture_notes TO midgard_runtime;
