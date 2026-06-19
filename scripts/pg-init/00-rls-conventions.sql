-- Myelin dev Postgres init (runs once on first cluster init, in /docker-entrypoint-initdb.d).
--
-- Establishes the (tenant, region) RLS-ready conventions the architecture pins
-- (multi-tenant isolation by Row-Level Security; the no-cross-tenant-read invariant
-- is enforced in Postgres, not just in app code). This script does NOT create the
-- domain tables — each service owns its own schema/migrations (the no-cross-db rule).
-- It creates:
--   1. a non-superuser application role that does NOT bypass RLS;
--   2. a session GUC convention (myelin.tenant_id / myelin.region) the app sets per
--      transaction so RLS policies can reference current_setting(...);
--   3. a helper that every tenant-scoped migration calls to make a table RLS-ready
--      (ENABLE + FORCE row level security + the standard tenant/region policy).
--
-- POSTGRES_USER (myelin_admin) is the migration/owner role; the app connects as
-- myelin_app at runtime so RLS is actually enforced (the owner would otherwise be
-- exempt unless FORCE is set — we FORCE, and additionally use a non-owner app role).

\set ON_ERROR_STOP on

-- 1. The runtime application role. NOSUPERUSER + NOBYPASSRLS is the load-bearing part:
--    a superuser (or a BYPASSRLS role) silently ignores every policy. The app MUST NOT.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_app') THEN
    CREATE ROLE myelin_app LOGIN PASSWORD 'myelin_app_pw' NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE;
  END IF;
END
$$;

GRANT CONNECT ON DATABASE myelin TO myelin_app;
GRANT USAGE ON SCHEMA public TO myelin_app;

-- Future tables/sequences created by the owner (myelin_admin) are usable by the app role.
ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;
ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO myelin_app;

-- 2. The session-GUC convention. The app sets these per transaction:
--      SELECT set_config('myelin.tenant_id', $1, true);
--      SELECT set_config('myelin.region',    $2, true);
--    RLS policies then reference current_setting('myelin.tenant_id', true).
--    We register the custom GUC namespace so a fresh session reads '' (not an error).
ALTER DATABASE myelin SET myelin.tenant_id TO '';
ALTER DATABASE myelin SET myelin.region TO '';

-- 3. The convention helper a tenant-scoped migration calls once per table. It makes the
--    table RLS-ready: ENABLE + FORCE (so even the table owner is subject to policies),
--    and installs the standard (tenant_id, region) isolation policy. A table is expected
--    to carry a tenant_id text column and a region text column.
CREATE OR REPLACE FUNCTION myelin_make_tenant_scoped(target regclass)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
  tname text := target::text;
BEGIN
  EXECUTE format('ALTER TABLE %s ENABLE ROW LEVEL SECURITY', tname);
  EXECUTE format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', tname);
  EXECUTE format(
    'CREATE POLICY myelin_tenant_isolation ON %s
       USING (tenant_id = current_setting(''myelin.tenant_id'', true)
              AND region = current_setting(''myelin.region'', true))
       WITH CHECK (tenant_id = current_setting(''myelin.tenant_id'', true)
                   AND region = current_setting(''myelin.region'', true))',
    tname);
END
$$;

COMMENT ON FUNCTION myelin_make_tenant_scoped(regclass) IS
  'Myelin RLS convention: ENABLE+FORCE RLS and install the (tenant_id, region) isolation policy on a tenant-scoped table.';
