-- Development PostgreSQL initialization. It creates non-superuser runtime roles, tenant/region
-- session settings, and the helper used by migrations to enable and force RLS. Domain tables remain
-- owned by service migrations. myelin_admin owns migrations; myelin_app handles runtime queries.

\set ON_ERROR_STOP on

-- Runtime queries use a non-superuser role without BYPASSRLS.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_app') THEN
    CREATE ROLE myelin_app LOGIN PASSWORD 'myelin_app_pw' NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE;
  END IF;
END
$$;

GRANT CONNECT ON DATABASE myelin TO myelin_app;
GRANT USAGE ON SCHEMA public TO myelin_app;

-- The cross-tenant CI scheduler is a distinct least-privilege capability. The NOLOGIN role is the
-- policy/grant target; the dev LOGIN inherits it but cannot SET ROLE to it, preserving session_user
-- as the connection identity the scheduler-region mapping keys on.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_ci_region_scheduler') THEN
    CREATE ROLE myelin_ci_region_scheduler NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_ci_scheduler_fr_par') THEN
    CREATE ROLE myelin_ci_scheduler_fr_par LOGIN PASSWORD 'myelin_ci_scheduler_dev_pw'
      NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE INHERIT;
  END IF;
END
$$;

ALTER ROLE myelin_ci_region_scheduler
  NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT;
ALTER ROLE myelin_ci_scheduler_fr_par
  LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE INHERIT;
GRANT myelin_ci_region_scheduler TO myelin_ci_scheduler_fr_par
  WITH INHERIT TRUE, SET FALSE;
REVOKE myelin_ci_region_scheduler FROM myelin_app;
GRANT CONNECT ON DATABASE myelin TO myelin_ci_scheduler_fr_par;
GRANT USAGE ON SCHEMA public TO myelin_ci_region_scheduler;

-- The elected outbox publisher is a distinct cross-tenant capability. The NOLOGIN role is the
-- fixed migration grant target; the constrained regional dev login inherits it but cannot SET
-- ROLE, preserving the authenticated session identity for provider verification.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_outbox_publisher') THEN
    CREATE ROLE myelin_outbox_publisher NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_outbox_publisher_fr_par') THEN
    CREATE ROLE myelin_outbox_publisher_fr_par LOGIN PASSWORD 'myelin_outbox_publisher_dev_pw'
      NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE INHERIT;
  END IF;
END
$$;

ALTER ROLE myelin_outbox_publisher
  NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT;
ALTER ROLE myelin_outbox_publisher_fr_par
  LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE INHERIT;
GRANT myelin_outbox_publisher TO myelin_outbox_publisher_fr_par
  WITH INHERIT TRUE, SET FALSE;
REVOKE myelin_outbox_publisher FROM myelin_app;
REVOKE myelin_outbox_publisher FROM myelin_ci_region_scheduler;
REVOKE myelin_outbox_publisher FROM myelin_ci_scheduler_fr_par;
GRANT CONNECT ON DATABASE myelin TO myelin_outbox_publisher_fr_par;
GRANT USAGE ON SCHEMA public TO myelin_outbox_publisher;

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

-- A scheduler's residency authority is server-owned, not a client-selected GUC. The private mapping
-- is keyed by session_user (the authenticated LOGIN remains stable across SECURITY DEFINER/SET ROLE)
-- and can be read only through the fixed-search-path function. `myelin.region` remains a required
-- transaction-local corroborating pin; it is never sufficient on its own.
BEGIN;
CREATE TABLE IF NOT EXISTS public.myelin_ci_scheduler_region_map (
  session_role name PRIMARY KEY,
  region       text NOT NULL CHECK (region <> '')
);
ALTER TABLE public.myelin_ci_scheduler_region_map OWNER TO myelin_admin;
REVOKE ALL ON TABLE public.myelin_ci_scheduler_region_map FROM PUBLIC;
REVOKE ALL ON TABLE public.myelin_ci_scheduler_region_map FROM myelin_app;
REVOKE ALL ON TABLE public.myelin_ci_scheduler_region_map FROM myelin_ci_region_scheduler;
REVOKE ALL ON TABLE public.myelin_ci_scheduler_region_map FROM myelin_ci_scheduler_fr_par;

INSERT INTO public.myelin_ci_scheduler_region_map (session_role, region)
VALUES ('myelin_ci_scheduler_fr_par', 'fr-par')
ON CONFLICT (session_role) DO UPDATE SET region = EXCLUDED.region;

CREATE OR REPLACE FUNCTION public.myelin_ci_scheduler_region()
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog
AS $$
  SELECT mapping.region
    FROM public.myelin_ci_scheduler_region_map AS mapping
   WHERE mapping.session_role = session_user::name
$$;
ALTER FUNCTION public.myelin_ci_scheduler_region() OWNER TO myelin_admin;
REVOKE ALL ON FUNCTION public.myelin_ci_scheduler_region() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.myelin_ci_scheduler_region() TO myelin_ci_region_scheduler;
COMMIT;

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
