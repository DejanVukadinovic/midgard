-- Tenant-owned data for the sample service, isolated by PostgreSQL row-level security.
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'capability_runtime') THEN
    CREATE ROLE capability_runtime NOLOGIN NOBYPASSRLS;
  END IF;
END $$;

CREATE TABLE notes (
  tenant_id  UUID NOT NULL,
  id         UUID NOT NULL DEFAULT gen_random_uuid(),
  body       TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, id)
);
ALTER TABLE notes ENABLE ROW LEVEL SECURITY;
ALTER TABLE notes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON notes
  USING (tenant_id = NULLIF(current_setting('app.tenant_id', true), '')::uuid)
  WITH CHECK (tenant_id = NULLIF(current_setting('app.tenant_id', true), '')::uuid);
GRANT SELECT, INSERT ON notes TO capability_runtime;
