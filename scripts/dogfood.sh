#!/usr/bin/env bash
# Myelin FOUNDER-DOGFOOD helper (R4.0) — bring the real `edge` binary up, mint an operator token,
# point a git remote / the web app at it. The companion runbook is docs/dogfood.md.
#
#   ./scripts/dogfood.sh env               print the full dogfood env contract (eval-able), incl. the
#                                          seal-key handling (generates the key once, reuses it)
#   ./scripts/dogfood.sh edge              build + run the edge over the dogfood env (serves :8080)
#   ./scripts/dogfood.sh ci                build + run the opt-in CI control plane/runner in the
#                                          foreground over the same dogfood cell
#   ./scripts/dogfood.sh dispatch          build + run the Git-event→CI-run dispatch consumer
#   ./scripts/dogfood.sh git-checks        build + run Git's CI-check projection consumer
#   ./scripts/dogfood.sh verify-check <repo> <pr> <head-oid> [context]
#                                          read-only proof that an exact PR head surfaced a required
#                                          settled green context through the production edge
#   ./scripts/dogfood.sh bootstrap -- <flags>
#                                          run `edge bootstrap <flags>` over the dogfood env, e.g.
#                                            ./scripts/dogfood.sh bootstrap -- --tenant acme --principal founder \
#                                              --issues-project 20aee030-c7fa-4757-8243-700faf528690
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
# Hosted CI uses the immutable base gVisor rootfs; Git wire layers its separate git-bearing rootfs
# from this same staged asset.
GVISOR_ROOTFS_DEFAULT="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets/rootfs"
# The git WIRE (clone/fetch/push) runs a real `git` inside a gVisor sandbox, so it needs a git-bearing
# rootfs staged (scripts/stage-git-rootfs.sh). Default location mirrors resolved_gvisor_git_rootfs().
GIT_ROOTFS_DEFAULT="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets/git-rootfs"
DOGFOOD_ISSUES_PROJECT="20aee030-c7fa-4757-8243-700faf528690"
DOGFOOD_ISSUES_TYPE="7d457754-f6a1-4cd8-8738-21751570b627"
DOGFOOD_ISSUES_PREFIX="MYL"

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
  local seal git_root rootfs git_rootfs region runsc_bin
  seal="$(cat "${SEAL_KEY_FILE}")"
  git_root="${MYELIN_GIT_ROOT:-${GIT_ROOT_DEFAULT}}"
  rootfs="${MYELIN_GVISOR_ROOTFS:-${GVISOR_ROOTFS_DEFAULT}}"
  git_rootfs="${MYELIN_GVISOR_GIT_ROOTFS:-${GIT_ROOTFS_DEFAULT}}"
  runsc_bin="${MYELIN_RUNSC_BIN:-$(command -v runsc || true)}"
  region="${MYELIN_REGION:-fr-par}"
  # The data-layer contract (DATABASE_URL / S3_* / REDIS_URL / NATS_URL / MYELIN_REGION).
  "${REPO_ROOT}/scripts/dev-stack.sh" env
  # `dev-stack.sh env` exports split credentials: DATABASE_URL stays the constrained `myelin_app`
  # runtime role while DATABASE_MIGRATION_URL is the schema-owning `myelin_admin` bootstrap role.
  # The edge validates the pair, runs DDL only through the latter, then closes it before serving.
  cat <<EOF
# ── the edge (R4.0) env ──
export MYELIN_CELL_ID="\${MYELIN_CELL_ID:-cell-dogfood}"  # a DEDICATED cell (the shared 'cell-dev' root may be sealed under a different key)
export MYELIN_KMS_SEAL_KEY="${seal}"   # the operator seal key (unseals the KMS root AND the token cell root)
export MYELIN_GIT_ROOT="${git_root}"   # on-disk bare-repo root
export MYELIN_REGION="${region}"       # residency region
export MYELIN_ISSUES_RECONCILE_TENANTS="\${MYELIN_ISSUES_RECONCILE_TENANTS:-acme}"  # explicit FORCE-RLS partitions; defaults to the runbook's canonical dogfood tenant
export MYELIN_DOGFOOD_ISSUES_PROJECT="\${MYELIN_DOGFOOD_ISSUES_PROJECT:-${DOGFOOD_ISSUES_PROJECT}}"  # canonical founder project UUID (bootstrap reader grant)
export MYELIN_DOGFOOD_ISSUES_TYPE="\${MYELIN_DOGFOOD_ISSUES_TYPE:-${DOGFOOD_ISSUES_TYPE}}"        # explicit v1 type UUID (no type catalogue/FK yet)
export MYELIN_DOGFOOD_ISSUES_PREFIX="\${MYELIN_DOGFOOD_ISSUES_PREFIX:-${DOGFOOD_ISSUES_PREFIX}}"                                     # canonical founder issue-key prefix
export MYELIN_EDGE_ADDR="\${MYELIN_EDGE_ADDR:-127.0.0.1:8080}"
export MYELIN_TOKEN_LOGIN="\${MYELIN_TOKEN_LOGIN:-1}"  # surface the operator-token web login in /v1/auth/config
export MYELIN_GVISOR_ROOTFS="\${MYELIN_GVISOR_ROOTFS:-${rootfs}}"  # immutable hosted-CI base rootfs
export MYELIN_GVISOR_GIT_ROOTFS="\${MYELIN_GVISOR_GIT_ROOTFS:-${git_rootfs}}"  # the sandboxed git-wire rootfs (stage-git-rootfs.sh)
export MYELIN_RUNSC_BIN="\${MYELIN_RUNSC_BIN:-${runsc_bin}}"  # absolute gVisor runtime path; edge startup validates it
export MYELIN_PUBLIC_BASE_URL="\${MYELIN_PUBLIC_BASE_URL:-http://\${MYELIN_EDGE_ADDR:-127.0.0.1:8080}}"  # advertised clone-URL base (F3)
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
    # Stage the sandboxed git-wire rootfs once (idempotent) so clone/fetch/push work out of the box.
    if [[ ! -x "${MYELIN_GVISOR_GIT_ROOTFS}/usr/bin/git" ]]; then
      echo "dogfood: staging the git-wire rootfs (${MYELIN_GVISOR_GIT_ROOTFS}) …" >&2
      "${REPO_ROOT}/scripts/stage-git-rootfs.sh" >/dev/null
    fi
    echo "dogfood: building + serving the edge on ${MYELIN_EDGE_ADDR} (git root ${MYELIN_GIT_ROOT})" >&2
    exec cargo run --quiet -p myelin-edge --bin edge
    ;;
  ci)
    load_env
    export MYELIN_CI_RUNNER=1
    echo "dogfood: building + serving the CI control plane with the runner enabled" >&2
    exec cargo run --quiet -p myelin-ci-controlplane --bin ci-controlplane
    ;;
  dispatch)
    load_env
    echo "dogfood: building + serving the Git-event → CI-run dispatch consumer" >&2
    exec cargo run --quiet -p myelin-ci-dispatch --bin ci-dispatch
    ;;
  git-checks)
    load_env
    echo "dogfood: building + serving Git's durable CI-check projection consumer" >&2
    exec cargo run --quiet -p myelin-git --bin git-check-projection
    ;;
  verify-check)
    if [[ "$#" -lt 3 || "$#" -gt 4 ]]; then
      echo "usage: $0 verify-check <repo> <pr-number> <head-oid> [context]" >&2
      exit 2
    fi
    if ! command -v curl >/dev/null 2>&1 || ! command -v jq >/dev/null 2>&1; then
      echo "dogfood: verify-check requires curl and jq" >&2
      exit 2
    fi
    if [[ -z "${MYELIN_TOKEN:-}" ]]; then
      echo "dogfood: MYELIN_TOKEN is required for the read-only verification" >&2
      exit 2
    fi
    repo="$1"
    pr_number="$2"
    expected_head="$3"
    context="${4:-build}"
    if [[ ! "${pr_number}" =~ ^[1-9][0-9]*$ || -z "${repo}" || -z "${expected_head}" || -z "${context}" ]]; then
      echo "dogfood: repo, positive PR number, head OID, and context must be non-empty" >&2
      exit 2
    fi
    edge_url="${MYELIN_EDGE_URL:-http://127.0.0.1:8080}"
    repo_segment="$(jq -rn --arg value "${repo}" '$value | @uri')"
    pr_json="$(curl --fail --silent --show-error \
      -H "authorization: Bearer ${MYELIN_TOKEN}" \
      "${edge_url}/v1/git/repos/${repo_segment}/prs/${pr_number}")"
    checks_json="$(curl --fail --silent --show-error \
      -H "authorization: Bearer ${MYELIN_TOKEN}" \
      "${edge_url}/v1/git/repos/${repo_segment}/prs/${pr_number}/checks")"
    jq -e --arg expected_head "${expected_head}" \
      '.durable == true and .head_oid == $expected_head' \
      <<<"${pr_json}" >/dev/null || {
        echo "dogfood: PR head does not match the expected pushed commit" >&2
        exit 1
      }
    jq -e --arg context "${context}" '
      .durable == true
      and (.required_contexts | index($context) != null)
      and (.green_contexts | index($context) != null)
      and (.fork_unendorsed_contexts | index($context) == null)
    ' <<<"${checks_json}" >/dev/null || {
      echo "dogfood: the required context is not a surfaced settled trusted success" >&2
      exit 1
    }
    jq -n \
      --arg repo "${repo}" \
      --argjson pr "${pr_number}" \
      --arg head_oid "${expected_head}" \
      --arg context "${context}" \
      --argjson gate_admitted "$(jq '.gate_admitted' <<<"${checks_json}")" \
      '{verified:true, repo:$repo, pr:$pr, head_oid:$head_oid, context:$context, gate_admitted:$gate_admitted}'
    ;;
  bootstrap)
    # Everything after an optional `--` is passed straight to `edge bootstrap`.
    if [[ "${1:-}" == "--" ]]; then shift; fi
    load_env
    has_issues_project=0
    for arg in "$@"; do
      if [[ "${arg}" == "--issues-project" || "${arg}" == --issues-project=* ]]; then
        has_issues_project=1
        break
      fi
    done
    if [[ "${has_issues_project}" == "0" ]]; then
      set -- "$@" --issues-project "${MYELIN_DOGFOOD_ISSUES_PROJECT}"
    fi
    echo "dogfood: minting an operator token (edge bootstrap $*) — the token prints to STDOUT" >&2
    exec cargo run --quiet -p myelin-edge --bin edge -- bootstrap "$@"
    ;;
  web)
    export MYELIN_EDGE_URL="http://127.0.0.1:8080"
    # The web create action injects this one server-side target. Export only these non-secret
    # canonical identifiers here (not the wider edge env / seal key).
    export MYELIN_DOGFOOD_ISSUES_PROJECT="${MYELIN_DOGFOOD_ISSUES_PROJECT:-${DOGFOOD_ISSUES_PROJECT}}"
    export MYELIN_DOGFOOD_ISSUES_TYPE="${MYELIN_DOGFOOD_ISSUES_TYPE:-${DOGFOOD_ISSUES_TYPE}}"
    export MYELIN_DOGFOOD_ISSUES_PREFIX="${MYELIN_DOGFOOD_ISSUES_PREFIX:-${DOGFOOD_ISSUES_PREFIX}}"
    if [[ "${EXEC:-0}" == "1" ]]; then
      echo "dogfood: starting the frontend (MYELIN_EDGE_URL=${MYELIN_EDGE_URL})" >&2
      cd "${REPO_ROOT}/frontend/apps/web"
      exec pnpm dev
    fi
    cat <<EOF
# The web operator UI is served by pnpm/vinxi in frontend/apps/web. Point it at the edge:
export MYELIN_EDGE_URL="http://127.0.0.1:8080"
export MYELIN_DOGFOOD_ISSUES_PROJECT="${MYELIN_DOGFOOD_ISSUES_PROJECT}"
export MYELIN_DOGFOOD_ISSUES_TYPE="${MYELIN_DOGFOOD_ISSUES_TYPE}"
export MYELIN_DOGFOOD_ISSUES_PREFIX="${MYELIN_DOGFOOD_ISSUES_PREFIX}"
cd frontend/apps/web && pnpm install && pnpm dev
# The operator-token login surface is gated by MYELIN_TOKEN_LOGIN=1 on the edge (see \`dogfood.sh env\`);
# the web login form that consumes it landed in R4.0 (paste the \`edge bootstrap\` token on /login). The
# CLI (\`myelin login --token <token> --scheme agent\`) and a git remote (see docs/dogfood.md) also work.
# (Re-run with EXEC=1 ./scripts/dogfood.sh web to actually start it.)
EOF
    ;;
  *)
    echo "usage: $0 {env|edge|ci|dispatch|git-checks|verify-check <repo> <pr> <head-oid> [context]|bootstrap -- <flags>|web}" >&2
    exit 2
    ;;
esac
