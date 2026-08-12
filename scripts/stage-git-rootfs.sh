#!/usr/bin/env bash
# Stage a Git-capable gVisor rootfs for clone, fetch, and push. It layers the host Git binary, shared
# libraries, and helpers onto the base BusyBox rootfs. The guest runs as uid 65534. Deployed assets
# should instead come from an immutable, digest-pinned image.
#
#   ./scripts/stage-git-rootfs.sh          stage into ~/.local/share/gvisor-assets/git-rootfs (idempotent)
#   FORCE=1 ./scripts/stage-git-rootfs.sh  re-stage from scratch (removes the existing staged tree)
#
# Prereqs: a base gVisor rootfs at ~/.local/share/gvisor-assets/rootfs (MYELIN_GVISOR_ROOTFS overrides),
# a host `git` on PATH, and `runsc` if you intend to actually run the wire.
set -euo pipefail

ASSETS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets"
BASE_ROOTFS="${MYELIN_GVISOR_ROOTFS:-${ASSETS_DIR}/rootfs}"
STAGED="${MYELIN_GVISOR_GIT_ROOTFS:-${ASSETS_DIR}/git-rootfs}"

die() { echo "stage-git-rootfs: $*" >&2; exit 1; }

[[ -d "${BASE_ROOTFS}" ]] || die "base rootfs absent at ${BASE_ROOTFS} — build/stage it first (MYELIN_GVISOR_ROOTFS overrides)"
HOST_GIT="$(command -v git)" || die "no host \`git\` on PATH to bake into the guest"

# Every OCI destination this shared tree can receive must exist before the tree is content-pinned.
# `/tmp` is the tmpfs target on both paths; checkout binds `/workspace`; git-wire binds `/repo` and
# optionally `/quarantine`. A pre-existing deployment missing any one of them is deliberately
# re-staged below instead of taking the old `usr/bin/git`-only early return.
REQUIRED_MOUNTPOINTS=(tmp workspace repo quarantine)
mountpoints_ready=1
for mountpoint in "${REQUIRED_MOUNTPOINTS[@]}"; do
  if [[ ! -d "${STAGED}/${mountpoint}" || -L "${STAGED}/${mountpoint}" ]]; then
    mountpoints_ready=0
    break
  fi
done

# Reuse the tree only when Git and every required mountpoint are present.
if [[ "${FORCE:-0}" != "1" && -x "${STAGED}/usr/bin/git" && "${mountpoints_ready}" == "1" ]]; then
  echo "stage-git-rootfs: already staged at ${STAGED} (FORCE=1 to re-stage)" >&2
  echo "${STAGED}"
  exit 0
fi

echo "stage-git-rootfs: baking a git rootfs at ${STAGED} from base ${BASE_ROOTFS} + ${HOST_GIT}" >&2
rm -rf "${STAGED}"
mkdir -p "${STAGED}"
# Copy the whole base rootfs (busybox + its glibc) preserving symlinks/perms.
cp -a "${BASE_ROOTFS}/." "${STAGED}/"

# The host `git` (a single multi-call binary; upload-pack/receive-pack dispatch off argv[0]).
install -Dm755 "${HOST_GIT}" "${STAGED}/usr/bin/git"

# Copy each host library into /usr/lib and /lib with its soname symlink.
stage_lib() {
  local soname="$1" host_path="$2" real real_name libdir dst link
  [[ -e "${host_path}" ]] || die "expected host lib ${host_path} (git's dependency) is absent"
  real="$(readlink -f "${host_path}")"
  real_name="$(basename "${real}")"
  for libdir in usr/lib lib; do
    dst="${STAGED}/${libdir}/${real_name}"
    install -Dm644 "${real}" "${dst}"
    link="${STAGED}/${libdir}/${soname}"
    rm -f "${link}"
    ln -s "${real_name}" "${link}"
  done
}
# The two shared libs `git` needs beyond glibc (discover the exact paths from the host's own git).
resolve_lib() { ldd "${HOST_GIT}" | sed -n "s/.*${1}[^ ]* => \([^ ]*\).*/\1/p" | head -1; }
stage_lib "libpcre2-8.so.0"  "$(resolve_lib libpcre2-8)"
stage_lib "libz-ng.so.2"     "$(resolve_lib libz-ng)"

# The git-core exec dir: helpers are symlinks back to the single `git` binary
# (/usr/lib/git-core/../../bin/git = /usr/bin/git). GIT_EXEC_PATH points the sandboxed git here.
mkdir -p "${STAGED}/usr/lib/git-core"
for helper in git-upload-pack git-receive-pack; do
  ln -sf "../../bin/git" "${STAGED}/usr/lib/git-core/${helper}"
done

# Mount destinations must exist before the rootfs becomes read-only.
mkdir -p \
  "${STAGED}/tmp" \
  "${STAGED}/workspace" \
  "${STAGED}/repo" \
  "${STAGED}/quarantine"

echo "stage-git-rootfs: done — set MYELIN_GVISOR_GIT_ROOTFS=${STAGED}" >&2
echo "${STAGED}"
