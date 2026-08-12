#!/usr/bin/env bash
# Exercise definition-fence provisioning on a new PostgreSQL 16 volume. This covers init ordering
# and a non-superuser migration role, neither of which the long-lived development database can test.
# The script runs `integration_ci_definition_fence_fresh` and prints container logs on failure.

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
  # Remove the anonymous volume so the next run starts fresh.
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
# Use the published port because it opens only after the init scripts finish; `docker exec` can
# connect to the temporary server while initialization is still running.
psql_admin() { PGPASSWORD="$SUPER_PW" psql -v ON_ERROR_STOP=1 -qtA \
  -h 127.0.0.1 -p "$PORT" -U myelin_admin -d myelin "$@" </dev/null; }
psql_admin_stdin() { PGPASSWORD="$SUPER_PW" psql -v ON_ERROR_STOP=1 -qtA \
  -h 127.0.0.1 -p "$PORT" -U myelin_admin -d myelin; }

# Avoid `docker logs | grep -q` under pipefail because the early grep exit causes SIGPIPE.
container_logs() { docker logs "$CONTAINER" 2>&1; }

log "waiting for the fresh volume's init scripts to complete"
ready=0
for _ in $(seq 1 120); do
  # Require both the completion message and a responsive published port.
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

# Check that initialization completed before application migrations.
log "proving the init scripts completed on a fresh volume with no application schema"
FENCE_ROLE="$(psql_admin -c "SELECT count(*) FROM pg_roles WHERE rolname='myelin_ci_definition_fence'")"
[ "$FENCE_ROLE" = "1" ] || { echo "pg-init did not create the fence role"; exit 1; }
SECURITY_SCHEMA="$(psql_admin -c "SELECT count(*) FROM pg_namespace WHERE nspname='myelin_ci_security'")"
[ "$SECURITY_SCHEMA" = "1" ] || { echo "pg-init did not create myelin_ci_security"; exit 1; }
# `myelin_ci_scheduler_region_map` is provisioning state, so it is not counted as an application table.
MIGRATED="$(psql_admin -c "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind='r' AND c.relname IN ('myelin_applied_migration','workflow_run','wf_definition','job_queue')")"
[ "$MIGRATED" = "0" ] || { echo "expected NO migrated tables before migrations, found $MIGRATED"; exit 1; }
PROBE_ABSENT="$(psql_admin -c "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='myelin_ci_security'")"
[ "$PROBE_ABSENT" = "0" ] || { echo "the probe must not exist before ci_0020h runs, found $PROBE_ABSENT"; exit 1; }
log "  fence role + empty security schema present; 0 migrated tables — init ordering proven"

# Create a constrained migration role and the runtime grants.
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

# Provision the fence membership for the constrained role.
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
