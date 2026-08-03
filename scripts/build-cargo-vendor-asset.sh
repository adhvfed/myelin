#!/usr/bin/env bash
# Build a CT-007 lockfile-keyed, read-only Cargo dependency asset. Network is allowed only here,
# on the trusted staging host; the resulting tree is content-addressed and the gVisor workload uses
# it with CARGO_NET_OFFLINE=true under deny-all egress.
#
# Usage: build-cargo-vendor-asset.sh [smoke|workspace]   (default: smoke)
#
#   smoke     — the tiny external-dependency smoke crate under testing/fixtures/cargo-vendor-smoke
#               (registry row `cargo-vendor-smoke-v1`, env override MYELIN_GVISOR_CARGO_VENDOR).
#   workspace — this repo's FULL workspace, keyed to the root Cargo.lock (registry row
#               `cargo-vendor-workspace-v1`, env override MYELIN_GVISOR_CARGO_VENDOR_WORKSPACE).
#
# Both variants share ONE staging recipe — the atomic build-then-promote, the world-readable
# `chmod -R a+rX` guarantee, the committed-digest guard, and the content-addressed .versions layout
# are identical byte-for-byte; only the source manifest/lockfile, the staged path + its env override,
# and the manifest row id the committed digest is read from differ. The default (`smoke`, no args)
# is behaviorally unchanged from the single-asset version this generalizes.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
MANIFEST="${REPO_ROOT}/runner-assets.toml"
ASSETS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets"

die() { echo "build-cargo-vendor-asset: $*" >&2; exit 1; }

ASSET="${1:-smoke}"
case "${ASSET}" in
  smoke)
    ROW_ID="cargo-vendor-smoke-v1"
    MANIFEST_PATH="${REPO_ROOT}/testing/fixtures/cargo-vendor-smoke/Cargo.toml"
    LOCK_SRC="${REPO_ROOT}/testing/fixtures/cargo-vendor-smoke/Cargo.lock"
    STAGED="${MYELIN_GVISOR_CARGO_VENDOR:-${ASSETS_DIR}/${ROW_ID}}"
    ;;
  workspace)
    ROW_ID="cargo-vendor-workspace-v1"
    MANIFEST_PATH="${REPO_ROOT}/Cargo.toml"
    LOCK_SRC="${REPO_ROOT}/Cargo.lock"
    STAGED="${MYELIN_GVISOR_CARGO_VENDOR_WORKSPACE:-${ASSETS_DIR}/${ROW_ID}}"
    ;;
  *)
    die "unknown asset '${ASSET}' (expected: smoke | workspace)"
    ;;
esac

VERSIONS_DIR="${STAGED}.versions"
MANAGED_MARKER="${STAGED}.myelin-managed"

for tool in cargo tar sha256sum awk; do
  command -v "${tool}" >/dev/null 2>&1 || die "${tool} is required"
done

canonical_tree_sha256() {
  tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu \
    -C "$1" -cf - . | sha256sum | awk '{print $1}'
}

committed_digest() {
  awk -v row="${ROW_ID}" '
    /^\[\[asset\]\]/ { in_block = 0 }
    $0 == "id = \"" row "\"" { in_block = 1 }
    in_block && /^canonical_tree_sha256 = / {
      line = $0; sub(/^canonical_tree_sha256 = "/, "", line); sub(/"$/, "", line);
      print line; exit
    }
  ' "${MANIFEST}"
}

[[ -f "${LOCK_SRC}" && ! -L "${LOCK_SRC}" ]] ||
  die "the exact source Cargo.lock is absent or linked: ${LOCK_SRC}"
LOCK_SHA256="$(sha256sum "${LOCK_SRC}" | awk '{print $1}')"

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
cargo vendor --locked --versioned-dirs --manifest-path "${MANIFEST_PATH}" \
  "${TMP}/vendor" >/dev/null
mkdir -p "${TMP}/.cargo"
cp "${LOCK_SRC}" "${TMP}/Cargo.lock"
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
