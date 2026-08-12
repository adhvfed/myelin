#!/usr/bin/env bash
# Provision the service account, directories, and pinned `runsc` binary for a gVisor CI runner.
# This script is idempotent and does not start services or grant system-wide capabilities. Runner
# startup rechecks paths, ownership, modes, and user-namespace configuration.
#
# Usage:
#   sudo ./scripts/install-ci-runner-host.sh /path/to/pinned/runsc
#
# The `runsc` argument must be the EXACT pinned build
# `crates/myelin-ci-sandbox/src/gvisor.rs`'s `PINNED_EXPLICIT_USERNS_RUNSC_VERSION`/
# `_SHA256_HEX` constants name — this script only places it correctly; it does not itself verify the
# digest (the running service refuses to boot against a mismatched one, which is the actual
# enforcement point and avoids duplicating the pinned digest in two places that could drift apart).
#
# Idempotent by design: re-running against an already-correct host only re-asserts ownership/mode
# (via `install`/`chown`/`chmod`, all no-ops when already correct) — it never deletes existing
# `runsc-root` or `userns-leases` state, since those hold durable crash-recovery markers.
set -euo pipefail

die() { echo "install-ci-runner-host: $*" >&2; exit 1; }

[[ $EUID -eq 0 ]] || die "must run as root (via sudo) — it creates root-owned directories and a system service account"
[[ $# -eq 1 ]] || die "usage: $0 /path/to/pinned/runsc"
RUNSC_SRC="$1"
[[ -f "${RUNSC_SRC}" ]] || die "${RUNSC_SRC}: no such file"

SERVICE_USER="${MYELIN_CI_RUNNER_USER:-myelin-runner}"
SERVICE_GROUP="${MYELIN_CI_RUNNER_GROUP:-myelin-runner}"
# Mirrors docs/edge-deployment.md's established convention: `/opt/myelin/...` for pinned
# binaries/immutable assets, `/var/lib/myelin/...` for mutable persistent state.
OPT_ROOT="/opt/myelin"
VAR_ROOT="/var/lib/myelin"
RUNSC_DST="${OPT_ROOT}/bin/runsc"
RUNSC_STATE_ROOT="${OPT_ROOT}/gvisor-runsc-root"
USERNS_LEASES_DIR="${VAR_ROOT}/userns-leases"
CI_WORKSPACES_DIR="${VAR_ROOT}/ci-workspaces"

echo "install-ci-runner-host: service account '${SERVICE_USER}:${SERVICE_GROUP}'" >&2
if ! getent group "${SERVICE_GROUP}" >/dev/null; then
  groupadd --system "${SERVICE_GROUP}"
fi
if ! getent passwd "${SERVICE_USER}" >/dev/null; then
  useradd --system --gid "${SERVICE_GROUP}" --no-create-home --shell /usr/sbin/nologin \
    --comment "Myelin CI sandbox runner (unprivileged; CAP_SYS_ADMIN via systemd AmbientCapabilities only)" \
    "${SERVICE_USER}"
fi

echo "install-ci-runner-host: pinned runsc -> ${RUNSC_DST}" >&2
install -d -m 0755 -o root -g root "${OPT_ROOT}"
install -d -m 0755 -o root -g root "${OPT_ROOT}/bin"
# `install` (not `cp`) so the destination is always rewritten with the exact requested
# owner/group/mode in one atomic step, regardless of what previously existed there.
install -m 0755 -o root -g root "${RUNSC_SRC}" "${RUNSC_DST}"

echo "install-ci-runner-host: explicit-userns runsc state root -> ${RUNSC_STATE_ROOT}" >&2
# `harden_explicit_userns_runsc_root` requires: ancestor chain not writable by the SERVICE ACCOUNT
# (satisfied — OPT_ROOT is root-owned 0755), and the LEAF owned by the service account itself, mode
# 0700 exactly (checked bit-for-bit: no group/other bits, full owner rwx). Only created if absent —
# an existing state root holds live container-state lookups a re-run must never disturb.
if [[ ! -d "${RUNSC_STATE_ROOT}" ]]; then
  install -d -m 0700 -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" "${RUNSC_STATE_ROOT}"
else
  chown "${SERVICE_USER}:${SERVICE_GROUP}" "${RUNSC_STATE_ROOT}"
  chmod 0700 "${RUNSC_STATE_ROOT}"
fi

echo "install-ci-runner-host: persistent state under ${VAR_ROOT}" >&2
install -d -m 0755 -o root -g root "${VAR_ROOT}"
# User-namespace lease state is service-owned, mode 0700, and durable across restarts.
if [[ ! -d "${USERNS_LEASES_DIR}" ]]; then
  install -d -m 0700 -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" "${USERNS_LEASES_DIR}"
else
  chown "${SERVICE_USER}:${SERVICE_GROUP}" "${USERNS_LEASES_DIR}"
  chmod 0700 "${USERNS_LEASES_DIR}"
fi
# This creates the workspace directory but not its Btrfs filesystem. Verify the filesystem and quota
# support before enabling `EphemeralDisk`.
if [[ ! -d "${CI_WORKSPACES_DIR}" ]]; then
  install -d -m 0700 -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" "${CI_WORKSPACES_DIR}"
else
  chown "${SERVICE_USER}:${SERVICE_GROUP}" "${CI_WORKSPACES_DIR}"
  chmod 0700 "${CI_WORKSPACES_DIR}"
fi
fstype="$(stat -f -c %T "${CI_WORKSPACES_DIR}" 2>/dev/null || echo unknown)"
if [[ "${fstype}" != "btrfs" ]]; then
  echo "install-ci-runner-host: WARNING: ${CI_WORKSPACES_DIR} is on '${fstype}', not btrfs — the" \
    "EphemeralDisk workspace manager will refuse to start against it until this path is a real," \
    "quota-enforcing Btrfs mount" >&2
fi

# `preflight_explicit_userns_helpers` needs `/usr/bin/newuidmap`/`newgidmap` present, root-owned,
# setuid, non-group/other-writable — this is a standard `uidmap` package install on virtually every
# distro, not something this script provisions; it only checks and warns.
for helper in newuidmap newgidmap; do
  path="/usr/bin/${helper}"
  if [[ ! -u "${path}" ]] || [[ "$(stat -c %U "${path}" 2>/dev/null)" != "root" ]]; then
    echo "install-ci-runner-host: WARNING: ${path} is missing, not root-owned, or not setuid —" \
      "install the 'uidmap' package (or equivalent) before enabling ExplicitUserNamespace mode" >&2
  fi
done

echo "install-ci-runner-host: done. Summary:" >&2
stat -c '  %A %U:%G %n' "${OPT_ROOT}" "${OPT_ROOT}/bin" "${RUNSC_DST}" "${RUNSC_STATE_ROOT}" \
  "${VAR_ROOT}" "${USERNS_LEASES_DIR}" "${CI_WORKSPACES_DIR}" >&2
echo "install-ci-runner-host: pinned runsc digest: $(sha256sum "${RUNSC_DST}" | cut -d' ' -f1)" >&2
echo "install-ci-runner-host: install deploy/systemd/myelin-ci-controlplane.service next, then" \
  "'systemctl daemon-reload && systemctl enable --now myelin-ci-controlplane'" >&2
