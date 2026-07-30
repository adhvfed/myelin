#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════════════════════════
# THE FRESH-VOLUME DEFINITION-FENCE DRILL (CT-007 lease/topology reconciliation)
# ═══════════════════════════════════════════════════════════════════════════════════════════════
#
# WHY THIS EXISTS, AND WHY THE PERSISTENT DEV STACK CANNOT REPLACE IT
#
# Every other gate runs against the long-lived dev stack, where `pg-init` ran once long ago and the
# migration role is a SUPERUSER. Two failure classes are therefore structurally invisible there:
#
#   1. INIT ORDERING — whether `scripts/pg-init/01-ci-definition-fence.sql` actually completes on a
#      brand-new volume before any application table exists, so that `ci_0020h` finds its
#      provisioning already in place. On the dev stack the provisioning simply pre-exists.
#   2. THE NON-SUPERUSER MIGRATION POSTURE — whether a `NOSUPERUSER NOBYPASSRLS NOCREATEROLE`
#      migration role can adopt the fence role through its explicit `SET TRUE` membership and create
#      the probe as its final owner. A superuser succeeds even if that membership is missing
#      entirely, so the dev stack cannot make that assertion honestly.
#
# This script builds a disposable `postgres:16`, lets Docker run the real `pg-init` scripts on a
# fresh volume, creates a genuinely constrained migration role, and runs the ignored integration
# target `integration_ci_definition_fence_fresh` through it.
#
# Usage:  scripts/drill-ci-definition-fence-fresh-postgres.sh
# Exit:   0 only if every assertion in that target passed. LOUD on failure (prints container logs).

set -Eeuo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER="myelin-fence-drill-$$-$(date +%s)"
# A random high loopback port so parallel runs (and a busy dev stack on 5433) never collide.
PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
SUPER_PW="fence_drill_super_pw"
MIGRATOR_PW="fence_drill_migrator_pw"
APP_PW="myelin_app_pw"   # set by 00-rls-conventions.sql's CREATE ROLE

log() { printf '\n\033[1m[fence-drill]\033[0m %s\n' "$*"; }

cleanup() {
  local status=$?
  if [ "$status" -ne 0 ]; then
    log "FAILED (exit $status) — PostgreSQL container logs follow:"
    docker logs "$CONTAINER" 2>&1 | tail -80 || true
  fi
  # Always remove the container AND its anonymous volume; a leaked volume would make the next run
  # non-fresh, which is precisely the property this drill depends on.
  docker rm -f -v "$CONTAINER" >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

command -v docker >/dev/null || { echo "docker is required for this drill" >&2; exit 1; }
command -v psql  >/dev/null || { echo "psql is required for this drill" >&2; exit 1; }

log "starting a disposable postgres:16 as $CONTAINER on 127.0.0.1:$PORT"
# `pg-init` is mounted read-only into the entrypoint directory: Docker runs *.sql in filename order
# (00- then 01-) exactly once, on a fresh volume, before accepting external connections.
docker run -d --name "$CONTAINER" \
  -e POSTGRES_USER=myelin_admin \
  -e POSTGRES_PASSWORD="$SUPER_PW" \
  -e POSTGRES_DB=myelin \
  -p "127.0.0.1:$PORT:5432" \
  -v "$REPO_ROOT/scripts/pg-init:/docker-entrypoint-initdb.d:ro" \
  postgres:16 >/dev/null

ADMIN_URL="postgres://myelin_admin:$SUPER_PW@127.0.0.1:$PORT/myelin"
# Everything below talks to the PUBLISHED PORT from the host, never `docker exec`. That is
# load-bearing: the entrypoint runs the init scripts against a temporary server on the container's
# UNIX socket and only starts listening externally once they finish, so host connectivity is what
# actually proves init completed. A `docker exec` readiness probe succeeds mid-init and would let
# the assertions below run before `01-ci-definition-fence.sql` had executed at all.
psql_admin() { PGPASSWORD="$SUPER_PW" psql -v ON_ERROR_STOP=1 -qtA \
  -h 127.0.0.1 -p "$PORT" -U myelin_admin -d myelin "$@" </dev/null; }
psql_admin_stdin() { PGPASSWORD="$SUPER_PW" psql -v ON_ERROR_STOP=1 -qtA \
  -h 127.0.0.1 -p "$PORT" -U myelin_admin -d myelin; }

# `docker logs | grep -q` is deliberately avoided: under `set -o pipefail`, `grep -q` exits on the
# first match, `docker logs` dies of SIGPIPE, and the pipeline reports failure even though the
# pattern WAS found. Capture once into a variable and match that instead.
container_logs() { docker logs "$CONTAINER" 2>&1; }

log "waiting for the fresh volume's init scripts to complete"
ready=0
for _ in $(seq 1 120); do
  # Both conditions: the entrypoint announced init completion AND the published port answers.
  logs="$(container_logs)"
  case "$logs" in
    *"PostgreSQL init process complete"*)
      if psql_admin -c 'SELECT 1' >/dev/null 2>&1; then
        ready=1
        break
      fi
      ;;
  esac
  sleep 1
done
[ "$ready" = "1" ] || { echo "the container never finished init and opened its port"; exit 1; }
# The init scripts must have run, in filename order, before the port ever opened.
logs="$(container_logs)"
case "$logs" in
  *"running /docker-entrypoint-initdb.d/00-rls-conventions.sql"*) ;;
  *) echo "00-rls-conventions.sql did not run"; exit 1 ;;
esac
case "$logs" in
  *"running /docker-entrypoint-initdb.d/01-ci-definition-fence.sql"*) ;;
  *) echo "01-ci-definition-fence.sql did not run"; exit 1 ;;
esac
first_init="${logs%%running /docker-entrypoint-initdb.d/01-ci-definition-fence.sql*}"
case "$first_init" in
  *"running /docker-entrypoint-initdb.d/00-rls-conventions.sql"*) ;;
  *) echo "the init scripts did not run in filename order"; exit 1 ;;
esac

# ── ASSERTION: the init scripts ran, and they ran BEFORE any application table exists ────────────
log "proving the init scripts completed on a fresh volume with no application schema"
FENCE_ROLE="$(psql_admin -c "SELECT count(*) FROM pg_roles WHERE rolname='myelin_ci_definition_fence'")"
[ "$FENCE_ROLE" = "1" ] || { echo "pg-init did not create the fence role"; exit 1; }
SECURITY_SCHEMA="$(psql_admin -c "SELECT count(*) FROM pg_namespace WHERE nspname='myelin_ci_security'")"
[ "$SECURITY_SCHEMA" = "1" ] || { echo "pg-init did not create myelin_ci_security"; exit 1; }
# "No application table" means no MIGRATION has run: the ledger itself, and the two tables the
# fence interacts with, must all be absent. `myelin_ci_scheduler_region_map` is deliberately NOT
# counted — it is part of provisioning (00-rls-conventions.sql), not the application schema.
MIGRATED="$(psql_admin -c "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind='r' AND c.relname IN ('myelin_applied_migration','workflow_run','wf_definition','job_queue')")"
[ "$MIGRATED" = "0" ] || { echo "expected NO migrated tables before migrations, found $MIGRATED"; exit 1; }
PROBE_ABSENT="$(psql_admin -c "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='myelin_ci_security'")"
[ "$PROBE_ABSENT" = "0" ] || { echo "the probe must not exist before ci_0020h runs, found $PROBE_ABSENT"; exit 1; }
log "  fence role + empty security schema present; 0 migrated tables — init ordering proven"

# ── A genuinely constrained migration role, and the runtime role ─────────────────────────────────
log "creating the NON-SUPERUSER migration role myelin_fresh_migrator"
psql_admin_stdin <<SQL
CREATE ROLE myelin_fresh_migrator LOGIN PASSWORD '$MIGRATOR_PW'
  NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE INHERIT;
GRANT CONNECT ON DATABASE myelin TO myelin_fresh_migrator;
-- It owns the application schema, the posture PgBootstrap documents for production.
ALTER SCHEMA public OWNER TO myelin_fresh_migrator;
GRANT ALL ON SCHEMA public TO myelin_fresh_migrator;
-- Its future tables must be usable by the runtime role, mirroring 00-rls-conventions.sql's
-- default privileges for myelin_admin.
ALTER DEFAULT PRIVILEGES FOR ROLE myelin_fresh_migrator IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;
ALTER DEFAULT PRIVILEGES FOR ROLE myelin_fresh_migrator IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO myelin_app;
SQL

# ── Re-run the provisioning for THIS migration role (the documented operator step) ───────────────
log "re-running 01-ci-definition-fence.sql with migration_role=myelin_fresh_migrator"
PGPASSWORD="$SUPER_PW" psql -v ON_ERROR_STOP=1 -v migration_role=myelin_fresh_migrator \
  -h 127.0.0.1 -p "$PORT" -U myelin_admin -d myelin \
  -f "$REPO_ROOT/scripts/pg-init/01-ci-definition-fence.sql" >/dev/null </dev/null

EDGE="$(psql_admin -c "SELECT m.rolname||':'||a.admin_option||':'||a.inherit_option||':'||a.set_option FROM pg_auth_members a JOIN pg_roles m ON m.oid=a.member WHERE a.roleid='myelin_ci_definition_fence'::regrole::oid")"
[ "$EDGE" = "myelin_fresh_migrator:false:false:true" ] || {
  echo "expected exactly one membership edge myelin_fresh_migrator:false:false:true, found: $EDGE"; exit 1; }
log "  exactly one membership edge, with admin=false inherit=false set=true"

# ── Run the ignored integration target through the constrained migration URL ─────────────────────
log "running integration_ci_definition_fence_fresh through the non-superuser migrator"
cd "$REPO_ROOT"
MYELIN_FRESH_MIGRATION_URL="postgres://myelin_fresh_migrator:$MIGRATOR_PW@127.0.0.1:$PORT/myelin" \
MYELIN_FRESH_APP_URL="postgres://myelin_app:$APP_PW@127.0.0.1:$PORT/myelin" \
MYELIN_FRESH_ADMIN_URL="$ADMIN_URL" \
DATABASE_URL="postgres://myelin_app:$APP_PW@127.0.0.1:$PORT/myelin" \
MYELIN_REGION=fr-par \
  cargo test -p myelin-ci-controlplane --features integration \
    --test integration_ci_definition_fence_fresh -- --ignored --nocapture

log "PASS — fresh-volume provisioning, non-superuser migration, and the complete cutover all hold"
