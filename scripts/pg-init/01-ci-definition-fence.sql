-- ═══════════════════════════════════════════════════════════════════════════════════════════════
-- CI DEFINITION-FENCE PROVISIONING (CT-007 lease/topology reconciliation)
-- ═══════════════════════════════════════════════════════════════════════════════════════════════
--
-- WHAT THIS PROVISIONS, AND WHY IT IS NOT A MIGRATION
--
-- The `ci.pipeline` definition cutover must answer one DATABASE-WIDE question while holding the
-- `wf_definition` row lock: "is any non-terminal run still pinned to the superseded version?".
-- `workflow_run` is FORCE ROW LEVEL SECURITY, and at boot there is no tenant/region scope, so the
-- answer can only be obtained by a role that can see past RLS. If that authority were missing the
-- probe would return FALSE rather than raising — a fail-OPEN cutover that drains the old version
-- while live runs still depend on it.
--
-- `BYPASSRLS` is cluster authority. Granting it is an operator action, deliberately NOT something an
-- application migration performs opportunistically when it happens to be running as a role with
-- CREATEROLE: migration behaviour must not vary with accidental excess privilege, and an
-- existing-volume rollout must be explicit and auditable. So this file provisions, and the
-- `ci_0020h` migration only VERIFIES and names this script in its refusal.
--
-- HOW TO RUN
--
--   psql "$DATABASE_PROVISIONING_URL" \
--     --set=ON_ERROR_STOP=1 \
--     --set=migration_role=myelin_admin \
--     --file scripts/pg-init/01-ci-definition-fence.sql
--
-- `migration_role` MUST be the role behind `DATABASE_MIGRATION_URL` — the role that will run the
-- migrations. It defaults to `myelin_admin` so Docker's `/docker-entrypoint-initdb.d` ordering
-- (00- then 01-) provisions a fresh self-host volume with no operator action.
--
-- Requires a cluster-admin/superuser connection: it creates a role carrying BYPASSRLS.
--
-- ── WHY THIS SCRIPT USES AN EXPLICIT TRANSACTION AND `ci_0020h` DELIBERATELY DOES NOT ────────────
--
-- These two look like the same problem and are not.
--
-- `ci_0020h` is handed to PostgreSQL by `PgMigrator` as ONE Simple Query message, so every statement
-- in it already runs inside a single implicit transaction: a `RAISE` rolls back the whole prefix and
-- ENDS that transaction. Adding an explicit `BEGIN` there would instead leave the pooled migrator
-- connection sitting in an aborted transaction block, so the next user of that connection —
-- including the migrator's own `pg_advisory_unlock` — would fail with `25P02`.
--
-- `psql` is the opposite: it sends each statement as its own message, so each one COMMITS on
-- success. Without an explicit transaction, a refusal partway through this script would leave
-- everything before it permanently applied — e.g. a colliding role already normalized to
-- `BYPASSRLS` before the ownership refusal fires. An aborted `psql` session simply exits and the
-- transaction rolls back, so the explicit `BEGIN`/`COMMIT` below is exactly right HERE and exactly
-- wrong THERE.
--
-- ── ORDERING: VALIDATE EVERYTHING, THEN MUTATE ───────────────────────────────────────────────────
--
-- Every collision refusal runs BEFORE any normalization. An earlier version revoked "known excess"
-- first and then scanned for unexpected grants — which destroyed the very evidence the scan needed,
-- so a colliding role's privileges were silently laundered instead of refused. Validation is
-- therefore a strict phase: it reads, it raises with object identities, and it changes nothing.
--
-- SCOPE OF THE AUTHORITY THIS CREATES
--
-- The fence role is NOLOGIN (never connectable), NOINHERIT, owns exactly one schema
-- (`myelin_ci_security`) and — after `ci_0020h`/`ci_0022c` — exactly two aggregate/boolean-returning
-- functions (the superseded-run backlog probe and the CT-007 5b.3-6e.1 activation-readiness probe). It
-- is deliberately given NO privilege on `public` and NO table grants here: `ci_0020h` grants it SELECT
-- on exactly three non-payload columns of `workflow_run`, and `ci_0022c` on exactly four non-payload
-- columns of `job_queue`, once those tables exist. `BYPASSRLS` does not itself grant access to any
-- table, so the role's reach is the intersection of "can see past RLS" and "has SELECT on those
-- columns" — which is exactly the two questions it must answer.

\set ON_ERROR_STOP on

\if :{?migration_role}
\else
\set migration_role myelin_admin
\endif

\echo 'ci-definition-fence: provisioning for migration_role =' :'migration_role'

BEGIN;

-- psql does NOT interpolate `:'var'` inside dollar-quoted bodies, so the DO blocks below read the
-- configured role through a transaction-local GUC instead.
SELECT set_config('myelin.provision_migration_role', :'migration_role', true);

-- The configured migration role must actually exist, or every membership statement below would
-- fail confusingly at the end instead of clearly at the start.
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_roles
     WHERE rolname = current_setting('myelin.provision_migration_role')
  ) THEN
    RAISE EXCEPTION
      'the configured migration_role % does not exist; pass the role behind DATABASE_MIGRATION_URL',
      current_setting('myelin.provision_migration_role');
  END IF;
END
$$;

-- ═══════════════════════════════════════════════════════════════════════════════════════════════
-- PHASE 1 — VALIDATE. Reads only. Raises with object identities. Mutates NOTHING.
-- ═══════════════════════════════════════════════════════════════════════════════════════════════
--
-- Most of this phase is conditional on the fence role already existing — with no role there is no
-- ownership, no ACL and no membership to collide with. The SCHEMA check is the exception and runs
-- FIRST, unconditionally: a foreign `myelin_ci_security` can exist with no role of our name at all.
DO $$
DECLARE
  fence oid;
  intended_probe constant text :=
    'myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer)';
  intended_probe_oid oid;
  -- CT-007 5b.3-6e.1: the SECOND fence-owned function — the activation-readiness probe (`ci_0022c`).
  intended_readiness_probe constant text :=
    'myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count()';
  intended_readiness_probe_oid oid;
  security_schema_owner text;
  offending text;
BEGIN
  -- (1a) An existing security schema must ALREADY belong to the fence role. Silently transferring
  -- someone else's schema would hand this role authority over objects it did not create.
  --
  -- This deliberately precedes the fresh-database early return below. With the check placed after
  -- it, a database carrying a foreign-owned `myelin_ci_security` but NO `myelin_ci_definition_fence`
  -- sailed straight through: phase 2 created the role, `CREATE SCHEMA IF NOT EXISTS` silently
  -- skipped the existing foreign schema (its `AUTHORIZATION` clause is not applied to a schema that
  -- already exists), and provisioning COMMITTED — leaving the postcondition query at the bottom
  -- merely PRINTING the wrong owner while `ci_0020h` failed later, after the commit.
  SELECT pg_catalog.pg_get_userbyid(n.nspowner) INTO security_schema_owner
    FROM pg_catalog.pg_namespace n WHERE n.nspname = 'myelin_ci_security';
  IF security_schema_owner IS NOT NULL
     AND security_schema_owner <> 'myelin_ci_definition_fence' THEN
    RAISE EXCEPTION
      'schema myelin_ci_security already exists and is owned by %, not myelin_ci_definition_fence. '
      'Refusing to take ownership of another role''s schema — resolve this deliberately.',
      security_schema_owner;
  END IF;

  SELECT oid INTO fence FROM pg_roles WHERE rolname = 'myelin_ci_definition_fence';
  IF fence IS NULL THEN
    RETURN;  -- fresh database: no role means no ownership, no grants and no membership edges
  END IF;
  intended_probe_oid := pg_catalog.to_regprocedure(intended_probe);
  intended_readiness_probe_oid := pg_catalog.to_regprocedure(intended_readiness_probe);

  -- (1b) OWNERSHIP. Identity is the exact `regprocedure` (name AND argument types), never `proname`:
  -- a colliding `...has_nonterminal_runs(text)` overload would otherwise be accepted here, never
  -- examined by `ci_0020h`, and — once the role is normalized to BYPASSRLS — become a
  -- BYPASSRLS-powered SECURITY DEFINER function that PUBLIC may be able to execute.
  SELECT string_agg(identity, ', ' ORDER BY identity)
    INTO offending
    FROM (
      SELECT 'relation ' || n.nspname || '.' || c.relname AS identity
        FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
       WHERE c.relowner = fence
      UNION ALL
      SELECT 'schema ' || n.nspname
        FROM pg_namespace n
       WHERE n.nspowner = fence AND n.nspname <> 'myelin_ci_security'
      UNION ALL
      SELECT 'routine ' || p.oid::regprocedure::text
        FROM pg_proc p
       WHERE p.proowner = fence
         AND p.oid IS DISTINCT FROM intended_probe_oid
         AND p.oid IS DISTINCT FROM intended_readiness_probe_oid
      UNION ALL
      SELECT 'type ' || n.nspname || '.' || t.typname
        FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace
       WHERE t.typowner = fence AND t.typtype <> 'c'
      UNION ALL
      SELECT 'database ' || d.datname
        FROM pg_database d
       WHERE d.datdba = fence
    ) AS owned;
  IF offending IS NOT NULL THEN
    RAISE EXCEPTION
      'myelin_ci_definition_fence already owns objects outside its dedicated scope: %. A role with '
      'this name appears to exist for another purpose. Resolve the collision deliberately (rename '
      'or reassign those objects); this script will not mass-revoke unknown ownership.', offending;
  END IF;

  -- (1c) PRIVILEGES GRANTED **TO** the fence role. Read with `aclexplode` filtered to this grantee:
  -- `has_*_privilege` would also report privileges held via PUBLIC (every function is
  -- PUBLIC-executable by default), which would make this refuse on any ordinary database.
  -- `public.workflow_run` (`ci_0020h`) and `public.job_queue` (`ci_0022c`) are excluded because those
  -- migrations own those grants; `myelin_ci_security` and the two intended probes are excluded because
  -- they are this provisioning surface.
  SELECT string_agg(identity, ', ' ORDER BY identity)
    INTO offending
    FROM (
      SELECT 'relation ' || n.nspname || '.' || c.relname AS identity
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        CROSS JOIN LATERAL aclexplode(c.relacl) AS acl
       WHERE c.relacl IS NOT NULL AND acl.grantee = fence
         AND NOT (n.nspname = 'public' AND c.relname IN ('workflow_run', 'job_queue'))
      UNION ALL
      SELECT 'column ' || n.nspname || '.' || c.relname || '.' || a.attname
        FROM pg_attribute a
        JOIN pg_class c ON c.oid = a.attrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        CROSS JOIN LATERAL aclexplode(a.attacl) AS acl
       WHERE a.attacl IS NOT NULL AND acl.grantee = fence
         AND NOT (n.nspname = 'public' AND c.relname IN ('workflow_run', 'job_queue'))
      UNION ALL
      SELECT 'schema ' || n.nspname
        FROM pg_namespace n
        CROSS JOIN LATERAL aclexplode(n.nspacl) AS acl
       WHERE n.nspacl IS NOT NULL AND acl.grantee = fence
         AND n.nspname NOT IN ('myelin_ci_security', 'public')
      UNION ALL
      SELECT 'routine ' || p.oid::regprocedure::text
        FROM pg_proc p
        CROSS JOIN LATERAL aclexplode(p.proacl) AS acl
       WHERE p.proacl IS NOT NULL AND acl.grantee = fence
         AND p.oid IS DISTINCT FROM intended_probe_oid
         AND p.oid IS DISTINCT FROM intended_readiness_probe_oid
    ) AS held;
  IF offending IS NOT NULL THEN
    RAISE EXCEPTION
      'myelin_ci_definition_fence holds privileges outside its dedicated scope: %. Resolve the '
      'collision deliberately; this script will not mass-revoke unknown grants.', offending;
  END IF;

  -- (1d) PRIVILEGES GRANTED **ON** fence-owned objects — the other direction, and the one that
  -- actually matters for escalation. A pre-existing fence-owned object that some other role (or
  -- PUBLIC) may execute or read is how a BYPASSRLS normalization turns into someone else's
  -- authority. Only `myelin_app`'s intended access to the security schema and the probe is allowed.
  SELECT string_agg(identity, ', ' ORDER BY identity)
    INTO offending
    FROM (
      SELECT 'schema myelin_ci_security granted to '
             || COALESCE(pg_catalog.pg_get_userbyid(NULLIF(acl.grantee, 0)), 'PUBLIC')
             || ' (' || acl.privilege_type || ')' AS identity
        FROM pg_namespace n
        CROSS JOIN LATERAL aclexplode(n.nspacl) AS acl
       WHERE n.nspname = 'myelin_ci_security'
         AND n.nspacl IS NOT NULL
         AND acl.grantee <> fence
         AND NOT (acl.grantee = 'myelin_app'::regrole::oid AND acl.privilege_type = 'USAGE')
      UNION ALL
      SELECT 'routine ' || p.oid::regprocedure::text || ' granted to '
             || COALESCE(pg_catalog.pg_get_userbyid(NULLIF(acl.grantee, 0)), 'PUBLIC')
             || ' (' || acl.privilege_type || ')'
        FROM pg_proc p
        CROSS JOIN LATERAL aclexplode(p.proacl) AS acl
       WHERE p.proowner = fence
         AND p.proacl IS NOT NULL
         AND acl.grantee <> fence
         AND NOT (acl.grantee = 'myelin_app'::regrole::oid AND acl.privilege_type = 'EXECUTE')
    ) AS exposed;
  IF offending IS NOT NULL THEN
    RAISE EXCEPTION
      'objects owned by myelin_ci_definition_fence are exposed to other roles: %. Normalizing this '
      'role to BYPASSRLS would hand that authority to them. Resolve the collision deliberately.',
      offending;
  END IF;

  -- (1e) MEMBERSHIP, BOTH DIRECTIONS. Neither direction is visible to the direct-ACL scans above: a
  -- same-named role can own nothing and hold no direct grant while still being wired into other
  -- roles, and privileges reached through such an edge are INHERITED, so (1c) never sees them.
  -- Phase 2 revokes both directions unconditionally, so without this check a membership-only
  -- collision was destroyed rather than reported — the same class as the public-grant laundering.
  --
  -- Direction A — the fence role as a MEMBER of something else. This script never creates such an
  -- edge in any shape, so any edge at all is foreign and is refused.
  SELECT string_agg('member of ' || quote_ident(g.rolname), ', ' ORDER BY g.rolname)
    INTO offending
    FROM pg_catalog.pg_auth_members a
    JOIN pg_catalog.pg_roles g ON g.oid = a.roleid
   WHERE a.member = fence;
  IF offending IS NOT NULL THEN
    RAISE EXCEPTION
      'myelin_ci_definition_fence is a member of other roles: %. It must be a member of NOTHING — '
      'inherited privileges would silently widen a BYPASSRLS role beyond the one question it exists '
      'to answer. Resolve the collision deliberately; this script will not revoke unknown '
      'membership.', offending;
  END IF;

  -- Direction B — other roles as MEMBERS OF the fence role. Re-targeting which role may adopt the
  -- fence IS supported (phase 2c revokes the previous adopter), so the identity of the member is not
  -- what is validated here — the OPTIONS are. This script only ever creates
  -- `ADMIN FALSE, INHERIT FALSE, SET TRUE`; any other shape was granted by something else. `ADMIN`
  -- is the authority to re-grant BYPASSRLS onward and `INHERIT` makes it passive (no explicit
  -- `SET ROLE` needed at all) — and because pg_auth_members is keyed by GRANTOR, re-granting from
  -- this session would leave such an edge standing alongside ours rather than replacing it.
  SELECT string_agg(
           quote_ident(m.rolname) || ' (admin=' || a.admin_option
             || ', inherit=' || a.inherit_option || ', set=' || a.set_option
             || ', granted by ' || quote_ident(pg_catalog.pg_get_userbyid(a.grantor)) || ')',
           ', ' ORDER BY m.rolname)
    INTO offending
    FROM pg_catalog.pg_auth_members a
    JOIN pg_catalog.pg_roles m ON m.oid = a.member
   WHERE a.roleid = fence
     AND NOT (a.admin_option IS FALSE AND a.inherit_option IS FALSE AND a.set_option IS TRUE);
  IF offending IS NOT NULL THEN
    RAISE EXCEPTION
      'membership of myelin_ci_definition_fence was granted with unexpected options: %. This script '
      'grants only ADMIN FALSE, INHERIT FALSE, SET TRUE; ADMIN would let the member re-grant '
      'BYPASSRLS onward and INHERIT would confer it passively. Revoke those grants deliberately '
      'before re-running.', offending;
  END IF;
END
$$;

-- ═══════════════════════════════════════════════════════════════════════════════════════════════
-- PHASE 2 — MUTATE. Only reached when phase 1 proved the state is either fresh or already ours.
-- ═══════════════════════════════════════════════════════════════════════════════════════════════

-- (2a) The role, normalized to an exact attribute set.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_ci_definition_fence') THEN
    CREATE ROLE myelin_ci_definition_fence
      NOLOGIN NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT;
  END IF;
END
$$;

ALTER ROLE myelin_ci_definition_fence
  NOLOGIN NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT;

-- (2b) The fence role is a member of NOTHING: being a member of another role would silently widen
-- its reach beyond the one question it exists to answer. Phase 1e already REFUSED every such edge,
-- so this loop is now unreachable in practice and kept only as a normalization backstop — it can no
-- longer be the thing that quietly erases a collision before anyone is told about it.
DO $$
DECLARE
  granted text;
BEGIN
  FOR granted IN
    SELECT g.rolname
      FROM pg_auth_members a
      JOIN pg_roles g ON g.oid = a.roleid
     WHERE a.member = 'myelin_ci_definition_fence'::regrole::oid
  LOOP
    EXECUTE format('REVOKE %I FROM myelin_ci_definition_fence', granted);
  END LOOP;
END
$$;

-- (2c) Exactly one role may adopt this authority. Every other member loses it. Re-targeting the
-- adopter between runs is supported and is exactly what this revokes; phase 1e proved every
-- surviving edge was granted in the shape this script itself creates, so nothing foreign is erased
-- here.
DO $$
DECLARE
  member text;
BEGIN
  FOR member IN
    SELECT m.rolname
      FROM pg_auth_members a
      JOIN pg_roles m ON m.oid = a.member
     WHERE a.roleid = 'myelin_ci_definition_fence'::regrole::oid
       AND m.rolname <> current_setting('myelin.provision_migration_role')
  LOOP
    EXECUTE format('REVOKE myelin_ci_definition_fence FROM %I', member);
  END LOOP;
END
$$;

-- The one allowed edge, with every option explicit. `SET TRUE` is what lets `ci_0020h` run
-- `SET LOCAL ROLE myelin_ci_definition_fence` and create the probe function already owned by the
-- fence role — so no ownership transfer is ever needed. `INHERIT FALSE` means the migration role
-- does NOT passively acquire BYPASSRLS: it must adopt the role explicitly, inside one transaction,
-- and reset immediately after. `ADMIN FALSE` means it cannot re-grant this authority onward. The
-- migration credential is offline and destroyed before serving.
GRANT myelin_ci_definition_fence TO :"migration_role"
  WITH ADMIN FALSE, INHERIT FALSE, SET TRUE;

-- (2d) The dedicated schema the fence role owns. Ownership of this one schema supplies the narrowly
-- scoped CREATE the probe function needs; the alternative — granting a BYPASSRLS role CREATE on
-- `public` — would let it create objects alongside every application table. Phase 1 already proved
-- any existing schema of this name is ours, so `AUTHORIZATION` here can only ever be a no-op or a
-- fresh create.
CREATE SCHEMA IF NOT EXISTS myelin_ci_security AUTHORIZATION myelin_ci_definition_fence;
REVOKE ALL ON SCHEMA myelin_ci_security FROM PUBLIC;

-- (2e) The fence role never connects and needs no privilege on `public`. Phase 1 proved there is
-- nothing here but our own, so these revocations cannot destroy foreign state.
DO $$
BEGIN
  EXECUTE format(
    'REVOKE ALL ON DATABASE %I FROM myelin_ci_definition_fence', current_database());
END
$$;
REVOKE ALL ON SCHEMA public FROM myelin_ci_definition_fence;

COMMIT;

-- ═══════════════════════════════════════════════════════════════════════════════════════════════
-- Postconditions an operator can read off the output.
-- ═══════════════════════════════════════════════════════════════════════════════════════════════
\echo 'ci-definition-fence: postconditions'
SELECT rolname               AS fence_role,
       rolcanlogin           AS can_login_must_be_false,
       rolsuper              AS superuser_must_be_false,
       rolbypassrls          AS bypassrls_must_be_true,
       rolinherit            AS inherit_must_be_false,
       rolcreaterole         AS createrole_must_be_false
  FROM pg_roles
 WHERE rolname = 'myelin_ci_definition_fence';

SELECT n.nspname             AS security_schema,
       pg_get_userbyid(n.nspowner) AS owner_must_be_fence_role
  FROM pg_namespace n
 WHERE n.nspname = 'myelin_ci_security';

SELECT m.rolname             AS may_adopt_the_fence_role,
       a.admin_option        AS admin_must_be_false,
       a.inherit_option      AS inherit_must_be_false,
       a.set_option          AS set_must_be_true
  FROM pg_auth_members a
  JOIN pg_roles m ON m.oid = a.member
 WHERE a.roleid = 'myelin_ci_definition_fence'::regrole::oid;

\echo 'ci-definition-fence: provisioning complete — now deploy the binary so ci_0020h can run'
