#!/usr/bin/env bash
# Build the CT-007 lockfile-keyed, read-only Cargo dependency asset. Network is allowed only here,
# on the trusted staging host; the resulting tree is content-addressed and the gVisor workload uses
# it with CARGO_NET_OFFLINE=true under deny-all egress.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
MANIFEST="${REPO_ROOT}/runner-assets.toml"
FIXTURE="${REPO_ROOT}/testing/fixtures/cargo-vendor-smoke"
ASSETS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets"
STAGED="${MYELIN_GVISOR_CARGO_VENDOR:-${ASSETS_DIR}/cargo-vendor-smoke-v1}"
VERSIONS_DIR="${STAGED}.versions"
MANAGED_MARKER="${STAGED}.myelin-managed"

die() { echo "build-cargo-vendor-asset: $*" >&2; exit 1; }

for tool in cargo tar sha256sum awk; do
  command -v "${tool}" >/dev/null 2>&1 || die "${tool} is required"
done

canonical_tree_sha256() {
  tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu \
    -C "$1" -cf - . | sha256sum | awk '{print $1}'
}

committed_digest() {
  awk '
    /^\[\[asset\]\]/ { in_block = 0 }
    /^id = "cargo-vendor-smoke-v1"/ { in_block = 1 }
    in_block && /^canonical_tree_sha256 = / {
      line = $0; sub(/^canonical_tree_sha256 = "/, "", line); sub(/"$/, "", line);
      print line; exit
    }
  ' "${MANIFEST}"
}

[[ -f "${FIXTURE}/Cargo.lock" && ! -L "${FIXTURE}/Cargo.lock" ]] ||
  die "the exact fixture Cargo.lock is absent or linked"
LOCK_SHA256="$(sha256sum "${FIXTURE}/Cargo.lock" | awk '{print $1}')"

if [[ "${FORCE:-0}" != "1" && -d "${STAGED}" ]]; then
  PIN="$(committed_digest)"
  CURRENT="$(canonical_tree_sha256 "${STAGED}")"
  if [[ -n "${PIN}" && "${CURRENT}" == "${PIN}" ]]; then
    echo "build-cargo-vendor-asset: already staged at ${STAGED}, matches sha256:${PIN}" >&2
    echo "${STAGED}"
    exit 0
  fi
fi

if [[ -e "${STAGED}" && ! -L "${STAGED}" && ! -f "${MANAGED_MARKER}" ]]; then
  die "refusing to replace unmanaged path ${STAGED}"
fi

mkdir -p "${VERSIONS_DIR}"
TMP="$(mktemp -d "${VERSIONS_DIR}/.build.XXXXXX")"
chmod 0755 "${TMP}"
cleanup() { rm -rf "${TMP}" 2>/dev/null || true; }
trap cleanup EXIT

echo "build-cargo-vendor-asset: cargo vendor for Cargo.lock sha256:${LOCK_SHA256}" >&2
cargo vendor --locked --versioned-dirs --manifest-path "${FIXTURE}/Cargo.toml" \
  "${TMP}/vendor" >/dev/null
mkdir -p "${TMP}/.cargo"
cp "${FIXTURE}/Cargo.lock" "${TMP}/Cargo.lock"
printf '%s\n' \
  '[source.crates-io]' \
  'replace-with = "vendored-sources"' \
  '' \
  '[source.vendored-sources]' \
  'directory = "/opt/myelin/cargo-vendor/vendor"' \
  '' \
  '[net]' \
  'offline = true' >"${TMP}/.cargo/config.toml"

# The vendor tree is bind-mounted READ-ONLY into the sandbox and read there by a non-owner mapped
# subuid — every entry MUST be world-readable (dirs world-traversable) or the offline build fails
# with a bare EACCES on a vendored source file (some crates ship 0640 files). Guarantee it here so
# the pinned tree is readable by construction; the registry ALSO enforces this fail-closed at verify
# (GvisorAssetRegistry::verify_cargo_vendor_world_readable). `a+rX` only adds x to dirs/already-x
# files, so it is a no-op on an already-world-readable tree (the committed smoke pin is unchanged).
chmod -R a+rX "${TMP}"

DIGEST="$(canonical_tree_sha256 "${TMP}")"
PIN="$(committed_digest)"
if [[ -n "${PIN}" && "${PIN}" != "${DIGEST}" && "${ALLOW_DIGEST_CHANGE:-0}" != "1" ]]; then
  die "built sha256:${DIGEST} differs from committed sha256:${PIN}; set ALLOW_DIGEST_CHANGE=1 only for an intentional repin"
fi

VERSION_DIR="${VERSIONS_DIR}/sha256-${DIGEST}"
if [[ -e "${VERSION_DIR}" ]]; then
  [[ ! -L "${VERSION_DIR}" ]] || die "version directory is a symlink: ${VERSION_DIR}"
  EXISTING="$(canonical_tree_sha256 "${VERSION_DIR}")"
  [[ "${EXISTING}" == "${DIGEST}" ]] || die "existing version directory drifted: ${VERSION_DIR}"
  rm -rf "${TMP}"
else
  mv "${TMP}" "${VERSION_DIR}"
fi
trap - EXIT

NEW_LINK="${STAGED}.new-symlink.$$"
ln -s "${VERSION_DIR}" "${NEW_LINK}"
mv -T "${NEW_LINK}" "${STAGED}"
touch "${MANAGED_MARKER}"
echo "build-cargo-vendor-asset: lock sha256:${LOCK_SHA256}" >&2
echo "build-cargo-vendor-asset: canonical-tree sha256:${DIGEST}" >&2
echo "${STAGED}"
