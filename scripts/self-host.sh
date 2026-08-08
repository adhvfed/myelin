#!/usr/bin/env bash
# Myelin FOUNDER-SELF_HOST helper (R4.0) — bring the real `edge` binary up, mint an operator token,
# point a git remote / the web app at it. The companion runbook is docs/self-host.md.
#
#   ./scripts/self-host.sh env               print the full self-host env contract (eval-able), incl. the
#                                          seal-key handling (generates the key once, reuses it)
#   ./scripts/self-host.sh edge              build + run the edge over the self-host env (serves :8080)
#   ./scripts/self-host.sh ci                build + run the opt-in CI control plane/runner in the
#                                          foreground over the same self-host cell
#   ./scripts/self-host.sh verify-ci-rootfs  prove the staged runner rootfs matches the digest pinned
#                                          by the checked-in founder pipeline
#   ./scripts/self-host.sh publisher         provision the bounded shared JetStream stream, then run
#                                          the elected least-privilege outbox publisher
#   ./scripts/self-host.sh dispatch          build + run the Git-event→CI-run dispatch consumer
#   ./scripts/self-host.sh git-checks        build + run Git's CI-check projection consumer
#   ./scripts/self-host.sh verify-check <repo> <pr> <head-oid> [context]
#                                          read-only proof that an exact PR head surfaced a required
#                                          settled green context through the production edge
#   ./scripts/self-host.sh verify-ci <run> <job> <marker> [evidence-dir]
#                                          read-only proof that the exact successful/settled run has
#                                          one byte-exact archived marker matching its live capture
#   ./scripts/self-host.sh bootstrap -- <flags>
#                                          run `edge bootstrap <flags>` over the self-host env, e.g.
#                                            ./scripts/self-host.sh bootstrap -- --tenant acme --principal founder \
#                                              --issues-project 20aee030-c7fa-4757-8243-700faf528690
#                                          prints the capability token to STDOUT (nothing else)
#   printf '%s' "$SECRET" | ./scripts/self-host.sh secret -- <operation> <flags>
#                                          run authenticated `edge secret ...`; secret material for
#                                          create/update/rotate is read only from STDIN
#   ./scripts/self-host.sh web               print (or, with EXEC=1, run) the frontend start wired to
#                                          MYELIN_EDGE_URL=http://127.0.0.1:8080
#
# The Fed stack must be up first (`fed start`). This script
# reuses that stack's env contract and ADDS the edge-only env (seal key, git root, region, addr).
#
# THE SEAL KEY IS THE ROOT OF TRUST. It unseals BOTH the KMS root AND the capability-token cell root.
# It is generated ONCE into a 0600 file and reused; LOSE IT AND YOU LOSE EVERYTHING (all encrypted
# columns + every minted token stops verifying). Back it up (see docs/self-host.md).
set -euo pipefail
# Never inherit caller xtrace into a script that handles the seal key or an operator credential.
set +x

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
GVISOR_RUST_ROOTFS_DEFAULT="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets/rust-rootfs"
# The git WIRE (clone/fetch/push) runs a real `git` inside a gVisor sandbox, so it needs a git-bearing
# rootfs staged (scripts/stage-git-rootfs.sh). Default location mirrors resolved_gvisor_git_rootfs().
GIT_ROOTFS_DEFAULT="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets/git-rootfs"
SELF_HOST_ISSUES_PROJECT="20aee030-c7fa-4757-8243-700faf528690"
SELF_HOST_ISSUES_TYPE="7d457754-f6a1-4cd8-8738-21751570b627"
SELF_HOST_ISSUES_PREFIX="MYL"

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
  echo "self-host: generated a NEW seal key at ${SEAL_KEY_FILE} (0600) — BACK THIS UP (see docs/self-host.md)" >&2
}

fail() {
  echo "self-host: $*" >&2
  exit 1
}

marker_count() {
  local file="$1" marker="$2" matches
  matches="$(grep -aFo -- "${marker}" "${file}" || true)"
  if [[ -z "${matches}" ]]; then
    echo 0
  else
    printf '%s\n' "${matches}" | wc -l | tr -d ' '
  fi
}

validate_edge_url() {
  local url="$1" port=""
  if [[ "${url}" =~ ^https://([A-Za-z0-9.-]+|\[[0-9A-Fa-f:]+\])(:([0-9]{1,5}))?$ ]]; then
    port="${BASH_REMATCH[3]:-}"
  elif [[ "${url}" =~ ^http://(127\.0\.0\.1|\[::1\])(:([0-9]{1,5}))?$ ]]; then
    port="${BASH_REMATCH[3]:-}"
  else
    fail "MYELIN_EDGE_URL must be an HTTPS origin or an HTTP loopback origin without path, userinfo, query, or fragment"
  fi
  if [[ -n "${port}" && ( "${port}" -lt 1 || "${port}" -gt 65535 ) ]]; then
    fail "MYELIN_EDGE_URL has an invalid port"
  fi
}

authenticated_get_to_file() {
  local url="$1" output="$2"
  local token_scheme="${MYELIN_TOKEN_SCHEME:-agent}"
  # --disable must be curl's first argument: operator curlrc settings may enable a trace that leaks
  # the credential. Direct connections also prevent proxy environment variables from receiving it.
  curl --disable \
    --fail --silent --show-error \
    --connect-timeout 10 --max-time 30 --max-filesize 524288 \
    --proto '=http,https' --proto-redir '=https' --noproxy '*' \
    --output "${output}" --config - "${url}" <<EOF
header = "authorization: Bearer ${MYELIN_TOKEN}"
header = "x-myelin-token-scheme: ${token_scheme}"
EOF
  local response_bytes
  response_bytes="$(wc -c <"${output}" | tr -d ' ')"
  [[ "${response_bytes}" -le 524288 ]] || fail "Edge response exceeds the 524288-byte acceptance bound"
}

verify_ci_rootfs() {
  for tool in sed tar sha256sum awk find sort readlink; do
    command -v "${tool}" >/dev/null 2>&1 || fail "CI rootfs verification requires ${tool}"
  done
  local config="${REPO_ROOT}/.myelin/ci.toml"
  [[ -f "${config}" && ! -L "${config}" ]] ||
    fail "checked-in pipeline is absent or linked: ${config}"

  local images
  images="$(
    sed -nE \
      's#^image = "(myelin\.local/linux-(small|rust)-v1-rootfs@sha256:[0-9a-f]{64})"$#\1#p' \
      "${config}" | sort -u
  )"
  [[ -n "${images}" ]] ||
    fail "checked-in pipeline does not declare a supported pinned runner image"

  local image image_id expected_digest rootfs resolved_rootfs actual_digest
  while IFS= read -r image; do
    image_id="${image%%@sha256:*}"
    expected_digest="${image##*@sha256:}"
    case "${image_id}" in
      myelin.local/linux-small-v1-rootfs)
        rootfs="${MYELIN_GVISOR_ROOTFS:-${GVISOR_ROOTFS_DEFAULT}}"
        ;;
      myelin.local/linux-rust-v1-rootfs)
        rootfs="${MYELIN_GVISOR_RUST_ROOTFS:-${GVISOR_RUST_ROOTFS_DEFAULT}}"
        ;;
      *)
        fail "unsupported runner image in checked-in pipeline: ${image_id}"
        ;;
    esac

    resolved_rootfs="$(readlink -f -- "${rootfs}" 2>/dev/null || true)"
    [[ -n "${resolved_rootfs}" && "${resolved_rootfs}" != "/" && -d "${resolved_rootfs}" ]] ||
      fail "staged CI rootfs is absent or invalid: ${rootfs}"
    [[ -d "${resolved_rootfs}/workspace" && ! -L "${resolved_rootfs}/workspace" ]] ||
      fail "staged CI rootfs must contain a real, precreated /workspace mountpoint: ${resolved_rootfs}/workspace"
    [[ -z "$(find "${resolved_rootfs}/workspace" -mindepth 1 -print -quit)" ]] ||
      fail "staged CI rootfs /workspace mountpoint must be empty before it is pinned"
    actual_digest="$(
      tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu \
        -C "${resolved_rootfs}" -cf - . |
        sha256sum |
        awk '{print $1}'
    )"
    [[ "${actual_digest}" == "${expected_digest}" ]] ||
      fail "staged CI rootfs digest ${actual_digest} does not match ${image}"
    printf '%s  %s\n' "${actual_digest}" "${image_id}"
  done <<< "${images}"
}

# Print the eval-able env contract: the dev-stack data-layer env + the edge-only env.
print_env() {
  ensure_seal_key
  local seal git_root rootfs rust_rootfs git_rootfs region runsc_bin
  seal="$(cat "${SEAL_KEY_FILE}")"
  git_root="${MYELIN_GIT_ROOT:-${GIT_ROOT_DEFAULT}}"
  rootfs="${MYELIN_GVISOR_ROOTFS:-${GVISOR_ROOTFS_DEFAULT}}"
  rust_rootfs="${MYELIN_GVISOR_RUST_ROOTFS:-${GVISOR_RUST_ROOTFS_DEFAULT}}"
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
export MYELIN_CELL_ID="\${MYELIN_CELL_ID:-cell-self-host}"  # a DEDICATED cell (the shared 'cell-dev' root may be sealed under a different key)
export MYELIN_KMS_SEAL_KEY="${seal}"   # the operator seal key (unseals the KMS root AND the token cell root)
export MYELIN_GIT_ROOT="${git_root}"   # on-disk bare-repo root
export MYELIN_CI_CHECKOUT_REPO_ROOT="${git_root}"  # checkout runner's boot-validated bare-repo root
export MYELIN_REGION="${region}"       # residency region
export MYELIN_ISSUES_RECONCILE_TENANTS="\${MYELIN_ISSUES_RECONCILE_TENANTS:-acme}"  # explicit FORCE-RLS partitions; defaults to the runbook's canonical self-host tenant
export MYELIN_SELF_HOST_ISSUES_PROJECT="\${MYELIN_SELF_HOST_ISSUES_PROJECT:-${SELF_HOST_ISSUES_PROJECT}}"  # canonical founder project UUID (bootstrap reader grant)
export MYELIN_SELF_HOST_ISSUES_TYPE="\${MYELIN_SELF_HOST_ISSUES_TYPE:-${SELF_HOST_ISSUES_TYPE}}"        # explicit v1 type UUID (no type catalogue/FK yet)
export MYELIN_SELF_HOST_ISSUES_PREFIX="\${MYELIN_SELF_HOST_ISSUES_PREFIX:-${SELF_HOST_ISSUES_PREFIX}}"                                     # canonical founder issue-key prefix
export MYELIN_EDGE_ADDR="\${MYELIN_EDGE_ADDR:-127.0.0.1:8080}"
export MYELIN_TOKEN_LOGIN="\${MYELIN_TOKEN_LOGIN:-1}"  # surface the operator-token web login in /v1/auth/config
export MYELIN_GVISOR_ROOTFS="\${MYELIN_GVISOR_ROOTFS:-${rootfs}}"  # immutable hosted-CI base rootfs
export MYELIN_GVISOR_RUST_ROOTFS="\${MYELIN_GVISOR_RUST_ROOTFS:-${rust_rootfs}}"  # immutable Rust build rootfs
export MYELIN_GVISOR_GIT_ROOTFS="\${MYELIN_GVISOR_GIT_ROOTFS:-${git_rootfs}}"  # the sandboxed git-wire rootfs (stage-git-rootfs.sh)
export MYELIN_RUNSC_BIN="\${MYELIN_RUNSC_BIN:-${runsc_bin}}"  # absolute gVisor runtime path; edge startup validates it
export MYELIN_PUBLIC_BASE_URL="\${MYELIN_PUBLIC_BASE_URL:-http://\${MYELIN_EDGE_ADDR:-127.0.0.1:8080}}"  # advertised clone-URL base (F3)
EOF
}

# Load the self-host env into THIS shell (for `edge`/`bootstrap`).
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
      echo "self-host: staging the git-wire rootfs (${MYELIN_GVISOR_GIT_ROOTFS}) …" >&2
      "${REPO_ROOT}/scripts/stage-git-rootfs.sh" >/dev/null
    fi
    echo "self-host: building + serving the edge on ${MYELIN_EDGE_ADDR} (git root ${MYELIN_GIT_ROOT})" >&2
    exec cargo run --quiet -p myelin-edge --bin edge
    ;;
  ci)
    load_env
    verify_ci_rootfs >/dev/null
    export MYELIN_CI_RUNNER=1
    echo "self-host: building + serving the CI control plane with the runner enabled" >&2
    exec cargo run --quiet -p myelin-ci-controlplane --bin ci-controlplane
    ;;
  publisher)
    load_env
    echo "self-host: provisioning the bounded shared event stream" >&2
    cargo run --quiet -p myelin-outbox-publisher -- provision
    echo "self-host: serving the elected least-privilege outbox publisher" >&2
    exec cargo run --quiet -p myelin-outbox-publisher -- serve
    ;;
  verify-ci-rootfs)
    if [[ "$#" -ne 0 ]]; then
      echo "usage: $0 verify-ci-rootfs" >&2
      exit 2
    fi
    verify_ci_rootfs
    ;;
  dispatch)
    load_env
    echo "self-host: building + serving the Git-event → CI-run dispatch consumer" >&2
    exec cargo run --quiet -p myelin-ci-dispatch --bin ci-dispatch
    ;;
  git-checks)
    load_env
    echo "self-host: building + serving Git's durable CI-check projection consumer" >&2
    exec cargo run --quiet -p myelin-git --bin git-check-projection
    ;;
  verify-check)
    if [[ "$#" -lt 3 || "$#" -gt 4 ]]; then
      echo "usage: $0 verify-check <repo> <pr-number> <head-oid> [context]" >&2
      exit 2
    fi
    if ! command -v curl >/dev/null 2>&1 || ! command -v jq >/dev/null 2>&1; then
      echo "self-host: verify-check requires curl and jq" >&2
      exit 2
    fi
    if [[ -z "${MYELIN_TOKEN:-}" ]]; then
      echo "self-host: MYELIN_TOKEN is required for the read-only verification" >&2
      exit 2
    fi
    token_re='^v4\.public\.[A-Za-z0-9_-]+\|[A-Za-z0-9_-]+\|[A-Za-z0-9_-]+(\|[A-Za-z0-9_-]+)?$'
    if [[ ! "${MYELIN_TOKEN}" =~ ${token_re} || ! "${MYELIN_TOKEN_SCHEME:-agent}" =~ ^[a-z0-9._-]+$ ]]; then
      echo "self-host: token or token scheme has a noncanonical transport shape" >&2
      exit 2
    fi
    repo="$1"
    pr_number="$2"
    expected_head="$3"
    context_arg="${4:-build}"
    if [[ "${context_arg}" == */* ]]; then
      context="${context_arg}"
    else
      context="ci/${context_arg}"
    fi
    if [[ ! "${pr_number}" =~ ^[1-9][0-9]*$ || -z "${repo}" || -z "${expected_head}" ||
          ! "${context}" =~ ^(ci|external)/[^[:space:]]+$ ]]; then
      echo "self-host: repo, positive PR number, head OID, and context must be non-empty" >&2
      exit 2
    fi
    edge_url="${MYELIN_EDGE_URL:-http://127.0.0.1:8080}"
    validate_edge_url "${edge_url}"
    repo_segment="$(jq -rn --arg value "${repo}" '$value | @uri')"
    response_tmp="$(mktemp)"
    trap 'rm -f "${response_tmp:-}"' EXIT
    authenticated_get_to_file \
      "${edge_url}/v1/git/repos/${repo_segment}/prs/${pr_number}" "${response_tmp}"
    pr_json="$(command cat "${response_tmp}")"
    authenticated_get_to_file \
      "${edge_url}/v1/git/repos/${repo_segment}/prs/${pr_number}/checks" "${response_tmp}"
    checks_json="$(command cat "${response_tmp}")"
    rm -f "${response_tmp}"
    response_tmp=""
    trap - EXIT
    jq -e --arg expected_head "${expected_head}" \
      '.durable == true and .head_oid == $expected_head' \
      <<<"${pr_json}" >/dev/null || {
        echo "self-host: PR head does not match the expected pushed commit" >&2
        exit 1
      }
    jq -e --arg context "${context}" '
      .durable == true
      and (.required_contexts | index($context) != null)
      and (.green_contexts | index($context) != null)
      and (.fork_unendorsed_contexts | index($context) == null)
    ' <<<"${checks_json}" >/dev/null || {
      echo "self-host: the required context is not a surfaced settled trusted success" >&2
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
  verify-ci)
    if [[ "$#" -lt 3 || "$#" -gt 4 ]]; then
      echo "usage: $0 verify-ci <run> <job> <marker> [evidence-dir]" >&2
      exit 2
    fi
    for tool in curl jq base64 sha256sum grep wc mktemp awk cat chmod mkdir rmdir; do
      command -v "${tool}" >/dev/null 2>&1 || {
        echo "self-host: verify-ci requires ${tool}" >&2
        exit 2
      }
    done
    if [[ -z "${MYELIN_TOKEN:-}" ]]; then
      echo "self-host: MYELIN_TOKEN is required for the read-only verification" >&2
      exit 2
    fi
    token_re='^v4\.public\.[A-Za-z0-9_-]+\|[A-Za-z0-9_-]+\|[A-Za-z0-9_-]+(\|[A-Za-z0-9_-]+)?$'
    if [[ ! "${MYELIN_TOKEN}" =~ ${token_re} || ! "${MYELIN_TOKEN_SCHEME:-agent}" =~ ^[a-z0-9._-]+$ ]]; then
      echo "self-host: token or token scheme has a noncanonical transport shape" >&2
      exit 2
    fi
    run="$1"
    job="$2"
    marker="$3"
    evidence_dir="${4:-${XDG_STATE_HOME:-$HOME/.local/state}/myelin/acceptance}"
    uuid_re='^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    if [[ ! "${run}" =~ ${uuid_re} || ! "${job}" =~ ${uuid_re} ]]; then
      echo "self-host: run and job must be canonical lowercase UUIDs" >&2
      exit 2
    fi
    if [[ ! "${marker}" =~ ^MYELIN-CI-[0-9a-f]{32}$ ]]; then
      echo "self-host: marker must have the unambiguous form MYELIN-CI-<32 lowercase hex characters>" >&2
      exit 2
    fi
    mkdir -p "${evidence_dir}"
    [[ ! -L "${evidence_dir}" ]] || fail "evidence directory must not be a symbolic link"
    chmod 700 "${evidence_dir}"
    umask 077
    live_file="${evidence_dir}/myelin-ci-live-${run}-${job}.log"
    detail_file="${evidence_dir}/myelin-ci-run-${run}.json"
    archive_file="${evidence_dir}/myelin-ci-archive-${run}-${job}.log"
    summary_file="${evidence_dir}/myelin-ci-acceptance-${run}-${job}.json"
    [[ -f "${live_file}" && ! -L "${live_file}" ]] ||
      fail "live capture ${live_file} is absent or is a symbolic link; attach ci watch during the run"
    for output in "${detail_file}" "${archive_file}" "${summary_file}"; do
      [[ ! -e "${output}" && ! -L "${output}" ]] ||
        fail "refusing to overwrite existing or linked evidence ${output}"
    done

    edge_url="${MYELIN_EDGE_URL:-http://127.0.0.1:8080}"
    validate_edge_url "${edge_url}"
    acceptance_page_bytes=262144
    acceptance_max_archive_bytes=67108864
    acceptance_max_pages=256
    temp_dir="$(mktemp -d "${evidence_dir}/.verify-ci.XXXXXX")"
    chmod 700 "${temp_dir}"
    response_tmp="${temp_dir}/response.json"
    page_tmp="${temp_dir}/page.bin"
    trap 'rm -f "${response_tmp:-}" "${page_tmp:-}"; rmdir "${temp_dir:-}" 2>/dev/null || true' EXIT
    authenticated_get_to_file "${edge_url}/v1/ci/runs/${run}" "${response_tmp}"
    detail_json="$(command cat "${response_tmp}")"
    jq -e --arg run "${run}" --arg job "${job}" '
      .run.run_id == $run
      and .run.state == "succeeded"
      and .run.cost_settled == true
      and (
        .run.finished_at
        | type == "string"
          and test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]{1,6})?Z$")
      )
      and ([.jobs[] | select(.job_id == $job and .state == "succeeded")] | length) == 1
    ' <<<"${detail_json}" >/dev/null ||
      fail "run/job is not the exact successful, cost-settled terminal pair"
    set -o noclobber
    printf '%s\n' "${detail_json}" | jq '.' >"${detail_file}"

    : >"${archive_file}"
    start=0
    total_end=""
    page_count=0
    while true; do
      page_count=$((page_count + 1))
      [[ "${page_count}" -le "${acceptance_max_pages}" ]] ||
        fail "archive exceeds the ${acceptance_max_pages}-page acceptance bound"
      authenticated_get_to_file \
        "${edge_url}/v1/ci/runs/${run}/jobs/${job}/log?start=${start}&limit=${acceptance_page_bytes}" \
        "${response_tmp}"
      page_json="$(command cat "${response_tmp}")"
      jq -e \
        --arg run "${run}" \
        --arg job "${job}" \
        --argjson start "${start}" \
        --argjson page_limit "${acceptance_page_bytes}" \
        --argjson max_total "${acceptance_max_archive_bytes}" '
        .run_id == $run
        and .job_id == $job
        and .byte_start == $start
        and (.byte_end | type == "number" and floor == .)
        and .byte_end >= $start
        and .byte_end <= 9007199254740991
        and (.total_end | type == "number" and floor == .)
        and .total_end >= .byte_end
        and .total_end <= $max_total
        and .byte_end == (
          if ($start + $page_limit) < .total_end
          then ($start + $page_limit)
          else .total_end
          end
        )
        and .encoding == "base64"
        and (.data | type == "string")
        and (
          if .byte_end < .total_end
          then .next_offset == .byte_end
          else .next_offset == null
          end
        )
      ' <<<"${page_json}" >/dev/null || fail "archive page is malformed or cross-scope"
      page_total="$(jq -r '.total_end' <<<"${page_json}")"
      if [[ -z "${total_end}" ]]; then
        total_end="${page_total}"
      elif [[ "${page_total}" != "${total_end}" ]]; then
        fail "terminal archive total changed during verification"
      fi
      byte_end="$(jq -r '.byte_end' <<<"${page_json}")"
      encoded="$(jq -r '.data' <<<"${page_json}")"
      printf '%s' "${encoded}" | base64 --decode >| "${page_tmp}" ||
        fail "archive page is not canonical base64"
      canonical_encoded="$(base64 --wrap=0 "${page_tmp}")"
      [[ "${canonical_encoded}" == "${encoded}" ]] ||
        fail "archive page is not canonical base64"
      expected_bytes=$((byte_end - start))
      actual_bytes="$(wc -c <"${page_tmp}" | tr -d ' ')"
      [[ "${actual_bytes}" == "${expected_bytes}" ]] ||
        fail "archive page byte count disagrees with its coordinates"
      command cat "${page_tmp}" >>"${archive_file}"
      rm -f "${page_tmp}"
      next_offset="$(jq -r '.next_offset // empty' <<<"${page_json}")"
      if [[ -z "${next_offset}" ]]; then
        [[ "${byte_end}" == "${total_end}" ]] ||
          fail "archive ended before its declared durable total"
        break
      fi
      [[ "${next_offset}" == "${byte_end}" && "${next_offset}" -gt "${start}" ]] ||
        fail "archive continuation is noncontiguous or non-progressing"
      start="${next_offset}"
    done

    live_markers="$(marker_count "${live_file}" "${marker}")"
    archive_markers="$(marker_count "${archive_file}" "${marker}")"
    [[ "${live_markers}" == "1" ]] ||
      fail "live CLI capture must contain the marker exactly once (found ${live_markers})"
    [[ "${archive_markers}" == "1" ]] ||
      fail "durable archive must contain the marker exactly once (found ${archive_markers})"
    archive_bytes="$(wc -c <"${archive_file}" | tr -d ' ')"
    [[ "${archive_bytes}" == "${total_end}" ]] ||
      fail "assembled archive length disagrees with the durable total"
    live_sha="$(sha256sum "${live_file}" | awk '{print $1}')"
    archive_sha="$(sha256sum "${archive_file}" | awk '{print $1}')"
    receipt="$(jq -n \
      --arg run "${run}" \
      --arg job "${job}" \
      --arg marker "${marker}" \
      --argjson archive_bytes "${archive_bytes}" \
      --arg live_sha256 "${live_sha}" \
      --arg archive_sha256 "${archive_sha}" \
      '{
        verified: true,
        run_id: $run,
        job_id: $job,
        marker: $marker,
        marker_count: {live: 1, archive: 1},
        archive_bytes: $archive_bytes,
        live_sha256: $live_sha256,
        archive_sha256: $archive_sha256
      }')"
    printf '%s\n' "${receipt}" >"${summary_file}"
    rm -f "${response_tmp}" "${page_tmp}"
    rmdir "${temp_dir}"
    response_tmp=""
    page_tmp=""
    temp_dir=""
    trap - EXIT
    printf '%s\n' "${receipt}"
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
      set -- "$@" --issues-project "${MYELIN_SELF_HOST_ISSUES_PROJECT}"
    fi
    echo "self-host: minting an operator token (edge bootstrap $*) — the token prints to STDOUT" >&2
    exec cargo run --quiet -p myelin-edge --bin edge -- bootstrap "$@"
    ;;
  secret)
    # Everything after an optional `--` is passed straight to `edge secret`. STDIN remains attached
    # so create/update/rotate material never enters argv or the environment.
    if [[ "${1:-}" == "--" ]]; then shift; fi
    load_env
    if [[ -z "${MYELIN_TOKEN:-}" ]]; then
      echo "self-host: MYELIN_TOKEN is required for secret administration" >&2
      exit 2
    fi
    token_re='^v4\.public\.[A-Za-z0-9_-]+\|[A-Za-z0-9_-]+\|[A-Za-z0-9_-]+(\|[A-Za-z0-9_-]+)?$'
    if [[ ! "${MYELIN_TOKEN}" =~ ${token_re} || ! "${MYELIN_TOKEN_SCHEME:-agent}" =~ ^[a-z0-9._-]+$ ]]; then
      echo "self-host: token or token scheme has a noncanonical transport shape" >&2
      exit 2
    fi
    echo "self-host: running authenticated edge secret operator command" >&2
    exec cargo run --quiet -p myelin-edge --bin edge -- secret "$@"
    ;;
  web)
    export MYELIN_EDGE_URL="http://127.0.0.1:8080"
    # The web create action injects this one server-side target. Export only these non-secret
    # canonical identifiers here (not the wider edge env / seal key).
    export MYELIN_SELF_HOST_ISSUES_PROJECT="${MYELIN_SELF_HOST_ISSUES_PROJECT:-${SELF_HOST_ISSUES_PROJECT}}"
    export MYELIN_SELF_HOST_ISSUES_TYPE="${MYELIN_SELF_HOST_ISSUES_TYPE:-${SELF_HOST_ISSUES_TYPE}}"
    export MYELIN_SELF_HOST_ISSUES_PREFIX="${MYELIN_SELF_HOST_ISSUES_PREFIX:-${SELF_HOST_ISSUES_PREFIX}}"
    if [[ "${EXEC:-0}" == "1" ]]; then
      echo "self-host: starting the frontend (MYELIN_EDGE_URL=${MYELIN_EDGE_URL})" >&2
      cd "${REPO_ROOT}/frontend/apps/web"
      exec pnpm dev
    fi
    cat <<EOF
# The web operator UI is served by pnpm/vinxi in frontend/apps/web. Point it at the edge:
export MYELIN_EDGE_URL="http://127.0.0.1:8080"
export MYELIN_SELF_HOST_ISSUES_PROJECT="${MYELIN_SELF_HOST_ISSUES_PROJECT}"
export MYELIN_SELF_HOST_ISSUES_TYPE="${MYELIN_SELF_HOST_ISSUES_TYPE}"
export MYELIN_SELF_HOST_ISSUES_PREFIX="${MYELIN_SELF_HOST_ISSUES_PREFIX}"
cd frontend/apps/web && pnpm install && pnpm dev
# The operator-token login surface is gated by MYELIN_TOKEN_LOGIN=1 on the edge (see \`self-host.sh env\`);
# the web login form that consumes it landed in R4.0 (paste the \`edge bootstrap\` token on /login). The
# CLI (\`myelin login --token <token> --scheme agent\`) and a git remote (see docs/self-host.md) also work.
# (Re-run with EXEC=1 ./scripts/self-host.sh web to actually start it.)
EOF
    ;;
  *)
    echo "usage: $0 {env|edge|ci|publisher|verify-ci-rootfs|dispatch|git-checks|verify-check <repo> <pr> <head-oid> [context]|verify-ci <run> <job> <marker> [evidence-dir]|bootstrap -- <flags>|secret -- <operation> <flags>|web}" >&2
    exit 2
    ;;
esac
