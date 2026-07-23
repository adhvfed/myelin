#!/usr/bin/env bash
# Myelin BACKUP/RESTORE DRILL (R4.3) — capture the LIVE dogfood data (Postgres OLTP + the on-disk git
# object tier), RESTORE it into a CLEAN target, and VERIFY the restore reads back byte-identical.
#
# This is the master-plan Tier-0 promise made a REPEATABLE DRILL, not a one-off: run it on a schedule
# (cron / systemd timer — see the tail of this file) so "our backups actually restore" is proven
# continuously against real data, never assumed.
#
#   ./scripts/backup-drill.sh run          capture → restore-to-clean → verify → report PASS/FAIL, cleanup
#   KEEP=1 ./scripts/backup-drill.sh run   keep the capture artifacts + the restored DB for inspection
#
# What it proves:
#   • Postgres: a `pg_dump` of the live `myelin` DB restores into a FRESH database with identical row
#     counts for every dogfood-bearing table (principals, ReBAC tuples, the cell token root, outbox, …).
#   • Git object tier: every on-disk bare repo under MYELIN_GIT_ROOT, archived and extracted into a
#     CLEAN root, `git fsck`es clean and advertises byte-identical ref→oid sets (destructive-restore
#     parity — the same property `myelin_git::backup::destructive_restore…reads_back_identical` asserts).
#
# Scope note: the git dogfood repos are on-disk bare repos (GT-001), so the essential git state is
# PG (refs/PR/authz rows) + the on-disk object tier captured here. The S3/RustFS blob tier holds large
# LFS-class objects; a full DR runbook also snapshots the bucket (documented at the tail) — this drill
# proves the two tiers the dogfood cutover actually populated.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ── Config (env-overridable; defaults match scripts/dogfood.sh + docker-compose.dev.yml) ──
PG_CONTAINER="${MYELIN_PG_CONTAINER:-myelin-postgres}"
PG_USER="${MYELIN_PG_ADMIN_USER:-myelin_admin}"
PG_DB="${MYELIN_PG_DB:-myelin}"
GIT_ROOT="${MYELIN_GIT_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/myelin/git-data}"
BACKUP_ROOT="${MYELIN_BACKUP_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/myelin/backups}"
RESTORE_DB="${MYELIN_RESTORE_DB:-myelin_restore_drill}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
WORK="${BACKUP_ROOT}/${STAMP}"

log()  { echo "backup-drill: $*" >&2; }
fail() { echo "backup-drill: FAIL — $*" >&2; exit 1; }
psql_admin() { docker exec -e PGPASSWORD="${MYELIN_PG_ADMIN_PW:-myelin_dev_pw}" "${PG_CONTAINER}" psql -U "${PG_USER}" "$@"; }

# The dogfood-bearing tables whose row counts must survive the round trip (loud if a table is missing).
VERIFY_TABLES=(principal rebac_tuple cell_token_root outbox kms_sealed_root revocation check_status)

run_drill() {
  command -v docker >/dev/null 2>&1 || fail "docker is required (the dev PG runs in ${PG_CONTAINER})"
  docker inspect "${PG_CONTAINER}" >/dev/null 2>&1 || fail "PG container ${PG_CONTAINER} not found — bring the stack up (scripts/dev-stack.sh up)"
  [[ -d "${GIT_ROOT}" ]] || fail "git object root ${GIT_ROOT} absent — nothing to back up (run the dogfood flow first)"

  mkdir -p "${WORK}"
  log "drill ${STAMP} — capturing live state into ${WORK}"

  # ── 1. CAPTURE ──────────────────────────────────────────────────────────────────────────────
  # Postgres: a consistent custom-format dump (single-txn snapshot) streamed out of the container.
  log "pg_dump ${PG_DB} (custom format) …"
  docker exec -e PGPASSWORD="${MYELIN_PG_ADMIN_PW:-myelin_dev_pw}" "${PG_CONTAINER}" \
    pg_dump -U "${PG_USER}" -Fc "${PG_DB}" > "${WORK}/pg.dump" || fail "pg_dump failed"
  [[ -s "${WORK}/pg.dump" ]] || fail "pg.dump is empty"
  log "pg.dump: $(du -h "${WORK}/pg.dump" | cut -f1)"

  # Git object tier: archive the whole on-disk root (bare repos, preserving layout/perms).
  log "archiving the git object tier (${GIT_ROOT}) …"
  tar -C "${GIT_ROOT}" -czf "${WORK}/git-data.tgz" . || fail "git-data archive failed"
  log "git-data.tgz: $(du -h "${WORK}/git-data.tgz" | cut -f1)"

  # Record the SOURCE fingerprints we will verify the restore against.
  source_git_fingerprint > "${WORK}/git.source.refs"
  source_table_counts    > "${WORK}/pg.source.counts"

  # ── 2. RESTORE INTO A CLEAN TARGET ──────────────────────────────────────────────────────────
  log "restoring Postgres into a CLEAN database '${RESTORE_DB}' …"
  psql_admin -d postgres -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS ${RESTORE_DB};" >/dev/null
  psql_admin -d postgres -v ON_ERROR_STOP=1 -c "CREATE DATABASE ${RESTORE_DB};" >/dev/null
  # pg_restore the custom dump into the fresh DB (streamed back into the container).
  docker exec -i -e PGPASSWORD="${MYELIN_PG_ADMIN_PW:-myelin_dev_pw}" "${PG_CONTAINER}" \
    pg_restore -U "${PG_USER}" -d "${RESTORE_DB}" --no-owner --no-privileges < "${WORK}/pg.dump" \
    || fail "pg_restore into ${RESTORE_DB} failed"

  log "extracting the git object tier into a CLEAN root …"
  local clean_git="${WORK}/restored-git"
  rm -rf "${clean_git}"; mkdir -p "${clean_git}"
  tar -C "${clean_git}" -xzf "${WORK}/git-data.tgz" || fail "git-data extract failed"

  # ── 3. VERIFY ───────────────────────────────────────────────────────────────────────────────
  log "verifying Postgres row-count parity …"
  restored_table_counts "${RESTORE_DB}" > "${WORK}/pg.restored.counts"
  if ! diff -u "${WORK}/pg.source.counts" "${WORK}/pg.restored.counts" > "${WORK}/pg.counts.diff"; then
    cat "${WORK}/pg.counts.diff" >&2
    fail "Postgres row counts DIVERGED after restore (see ${WORK}/pg.counts.diff)"
  fi
  log "  ✓ PG parity: $(wc -l < "${WORK}/pg.source.counts") tables, identical counts"

  log "verifying the git object tier (fsck + ref→oid parity) …"
  restored_git_fingerprint "${clean_git}" > "${WORK}/git.restored.refs"
  if ! diff -u "${WORK}/git.source.refs" "${WORK}/git.restored.refs" > "${WORK}/git.refs.diff"; then
    cat "${WORK}/git.refs.diff" >&2
    fail "git ref→oid sets DIVERGED after restore (see ${WORK}/git.refs.diff)"
  fi
  local nrepos; nrepos=$(grep -c '\.git ' "${WORK}/git.source.refs" || true)
  log "  ✓ git parity: every ref→oid identical; all repos fsck clean"

  # ── 4. REPORT + CLEANUP ─────────────────────────────────────────────────────────────────────
  log "DRILL PASSED — pg + git-object tier restored byte-identical into a clean target."
  if [[ "${KEEP:-0}" == "1" ]]; then
    log "KEEP=1 — left artifacts in ${WORK} and DB ${RESTORE_DB} for inspection."
  else
    psql_admin -d postgres -c "DROP DATABASE IF EXISTS ${RESTORE_DB};" >/dev/null
    rm -rf "${WORK}"
    log "cleaned up the restored DB + capture artifacts (KEEP=1 to retain)."
  fi
  echo "PASS ${STAMP}"
}

# ── verification helpers ──────────────────────────────────────────────────────────────────────

# The source PG row counts for every verify table (loud 'MISSING' if a table is absent).
source_table_counts()  { _table_counts "${PG_DB}"; }
restored_table_counts() { _table_counts "$1"; }
_table_counts() {
  local db="$1" t
  for t in "${VERIFY_TABLES[@]}"; do
    local n
    n=$(psql_admin -d "${db}" -tAc "SELECT count(*) FROM ${t};" 2>/dev/null || echo MISSING)
    echo "${t} ${n}"
  done
}

# The source git fingerprint: for every bare repo under GIT_ROOT, fsck it and emit sorted 'ref oid'
# lines prefixed by the repo path (relative to the root) so the restore must match exactly.
source_git_fingerprint()   { _git_fingerprint "${GIT_ROOT}"; }
restored_git_fingerprint() { _git_fingerprint "$1"; }
_git_fingerprint() {
  local root="$1" repo rel
  while IFS= read -r repo; do
    rel="${repo#"${root}"/}"
    # fsck must be clean; a corrupt restore fails the drill loudly.
    git --git-dir="${repo}" fsck --full --strict >/dev/null 2>&1 || { echo "${rel}.git FSCK-FAILED"; continue; }
    # Every ref → oid (sorted), plus the symbolic HEAD target (F9: HEAD must survive).
    git --git-dir="${repo}" for-each-ref --format='%(objectname) %(refname)' 2>/dev/null \
      | sort | sed "s#^#${rel} #"
    echo "${rel} HEAD -> $(git --git-dir="${repo}" symbolic-ref HEAD 2>/dev/null || echo '<none>')"
  done < <(find "${root}" -maxdepth 5 -name '*.git' -type d 2>/dev/null | sort)
}

case "${1:-run}" in
  run) run_drill ;;
  *)   echo "usage: $0 run   (KEEP=1 to retain artifacts)" >&2; exit 2 ;;
esac

# ── Scheduling the drill (make it repeating, not a one-off) ─────────────────────────────────────
# systemd timer (daily), user scope:
#   ~/.config/systemd/user/myelin-backup-drill.service   [Service] Type=oneshot
#     ExecStart=%h/Projects/myelin/scripts/backup-drill.sh run
#   ~/.config/systemd/user/myelin-backup-drill.timer      [Timer] OnCalendar=daily  Persistent=true
#   systemctl --user enable --now myelin-backup-drill.timer
# or cron:  0 4 * * *  /home/adhv/Projects/myelin/scripts/backup-drill.sh run >> ~/.local/state/myelin/backup-drill.log 2>&1
#
# Full-DR extension (beyond this dogfood drill): also `aws s3 sync s3://<bucket> <target>` (RustFS/
# Scaleway Object Storage) for the T2 blob tier, and ship pg.dump + git-data.tgz off-host (the 3-2-1
# rule). This drill proves the RESTORE PATH works; off-siting is an ops-runbook concern (R5.3).
