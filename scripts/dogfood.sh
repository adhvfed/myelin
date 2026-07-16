#!/usr/bin/env bash
# Myelin FOUNDER-DOGFOOD helper (R4.0) — bring the real `edge` binary up, mint an operator token,
# point a git remote / the web app at it. The companion runbook is docs/dogfood.md.
#
#   ./scripts/dogfood.sh env               print the full dogfood env contract (eval-able), incl. the
#                                          seal-key handling (generates the key once, reuses it)
#   ./scripts/dogfood.sh edge              build + run the edge over the dogfood env (serves :8080)
#   ./scripts/dogfood.sh bootstrap -- <flags>
#                                          run `edge bootstrap <flags>` over the dogfood env, e.g.
#                                            ./scripts/dogfood.sh bootstrap -- --tenant acme --principal founder
#                                          prints the capability token to STDOUT (nothing else)
#   ./scripts/dogfood.sh web               print (or, with EXEC=1, run) the frontend start wired to
#                                          MYELIN_EDGE_URL=http://127.0.0.1:8080
#
# The DATA LAYER (Postgres/Valkey/NATS/S3) must be up first — `./scripts/dev-stack.sh up`. This script
# reuses that stack's env contract and ADDS the edge-only env (seal key, git root, region, addr).
#
# THE SEAL KEY IS THE ROOT OF TRUST. It unseals BOTH the KMS root AND the capability-token cell root.
# It is generated ONCE into a 0600 file and reused; LOSE IT AND YOU LOSE EVERYTHING (all encrypted
# columns + every minted token stops verifying). Back it up (see docs/dogfood.md).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# XDG paths (the operator's own machine): the seal key is STATE (secret, per-host, never in the repo);
# the git data root is DATA.
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/myelin"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/myelin"
SEAL_KEY_FILE="${STATE_DIR}/seal.key"
GIT_ROOT_DEFAULT="${DATA_DIR}/git-data"

# Generate the seal key ONCE (0600), reuse thereafter. openssl if present, else /dev/urandom. NEVER
# regenerated over an existing key (that would orphan the KMS root + every minted token, fail-closed).
ensure_seal_key() {
  if [[ -s "${SEAL_KEY_FILE}" ]]; then
    return
  fi
  mkdir -p "${STATE_DIR}"
  ( umask 077
    if command -v openssl >/dev/null 2>&1; then
      openssl rand -hex 32 > "${SEAL_KEY_FILE}"
    else
      # 32 bytes → 64 hex chars from the kernel CSPRNG.
      head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "${SEAL_KEY_FILE}"
    fi
  )
  chmod 600 "${SEAL_KEY_FILE}"
  echo "dogfood: generated a NEW seal key at ${SEAL_KEY_FILE} (0600) — BACK THIS UP (see docs/dogfood.md)" >&2
}

# Print the eval-able env contract: the dev-stack data-layer env + the edge-only env.
print_env() {
  ensure_seal_key
  local seal git_root region db_app db_admin
  seal="$(cat "${SEAL_KEY_FILE}")"
  git_root="${MYELIN_GIT_ROOT:-${GIT_ROOT_DEFAULT}}"
  region="${MYELIN_REGION:-fr-par}"
  # The data-layer contract (DATABASE_URL / S3_* / REDIS_URL / NATS_URL / MYELIN_REGION).
  "${REPO_ROOT}/scripts/dev-stack.sh" env
  # The edge SELF-MIGRATES at boot (CREATE TABLE …), so it needs a DB role WITH `CREATE` on schema
  # `public`. The dev-stack `myelin_app` role is least-privilege (no CREATE), so for single-founder
  # dogfood — where the operator owns the box AND the DB — we run the edge as the schema-owner
  # (`myelin_admin`). (A real multi-tenant deployment runs migrations under a privileged role and the
  # serving edge under the app role; that split is a deployment concern, not a dogfood one.)
  db_app="$("${REPO_ROOT}/scripts/dev-stack.sh" env | sed -n 's/^export DATABASE_URL="\(.*\)"$/\1/p')"
  db_admin="${DATABASE_URL_ADMIN:-${db_app/myelin_app:myelin_app_pw/myelin_admin:myelin_dev_pw}}"
  cat <<EOF
# ── the edge (R4.0) env ──
export DATABASE_URL="${db_admin}"      # schema-owner role: the edge self-migrates at boot (needs CREATE)
export MYELIN_CELL_ID="\${MYELIN_CELL_ID:-cell-dogfood}"  # a DEDICATED cell (the shared 'cell-dev' root may be sealed under a different key)
export MYELIN_KMS_SEAL_KEY="${seal}"   # the operator seal key (unseals the KMS root AND the token cell root)
export MYELIN_GIT_ROOT="${git_root}"   # on-disk bare-repo root
export MYELIN_REGION="${region}"       # residency region
export MYELIN_EDGE_ADDR="\${MYELIN_EDGE_ADDR:-127.0.0.1:8080}"
export MYELIN_TOKEN_LOGIN="\${MYELIN_TOKEN_LOGIN:-1}"  # surface the operator-token web login in /v1/auth/config
EOF
}

# Load the dogfood env into THIS shell (for `edge`/`bootstrap`).
load_env() {
  ensure_seal_key
  # shellcheck disable=SC1090
  eval "$(print_env)"
  mkdir -p "${MYELIN_GIT_ROOT}"
}

cmd="${1:-env}"; shift || true

case "${cmd}" in
  env)
    print_env
    ;;
  edge)
    load_env
    echo "dogfood: building + serving the edge on ${MYELIN_EDGE_ADDR} (git root ${MYELIN_GIT_ROOT})" >&2
    exec cargo run --quiet -p myelin-edge --bin edge
    ;;
  bootstrap)
    # Everything after an optional `--` is passed straight to `edge bootstrap`.
    if [[ "${1:-}" == "--" ]]; then shift; fi
    load_env
    echo "dogfood: minting an operator token (edge bootstrap $*) — the token prints to STDOUT" >&2
    exec cargo run --quiet -p myelin-edge --bin edge -- bootstrap "$@"
    ;;
  web)
    export MYELIN_EDGE_URL="http://127.0.0.1:8080"
    if [[ "${EXEC:-0}" == "1" ]]; then
      echo "dogfood: starting the frontend (MYELIN_EDGE_URL=${MYELIN_EDGE_URL})" >&2
      cd "${REPO_ROOT}/frontend/apps/web"
      exec pnpm dev
    fi
    cat <<EOF
# The web operator UI is served by pnpm/vinxi in frontend/apps/web. Point it at the edge:
export MYELIN_EDGE_URL="http://127.0.0.1:8080"
cd frontend/apps/web && pnpm install && pnpm dev
# The operator-token login surface is gated by MYELIN_TOKEN_LOGIN=1 on the edge (see \`dogfood.sh env\`);
# the web login form that consumes it is the SEPARATE frontend deliverable (R4.0 frontend half). Until
# then, use the CLI: \`myelin login --token <token> --scheme agent\` or a git remote (see docs/dogfood.md).
# (Re-run with EXEC=1 ./scripts/dogfood.sh web to actually start it.)
EOF
    ;;
  *)
    echo "usage: $0 {env|edge|bootstrap -- <flags>|web}" >&2
    exit 2
    ;;
esac
