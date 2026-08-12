#!/usr/bin/env bash
# Build the `linux-rust-v1` gVisor rootfs used by the Rust toolchain capability smoke test. This
# asset does not replace `linux-small-v1` and does not cover checkout mounts, dependency vendoring,
# environment propagation, resource sizing, Node/browser jobs, Docker-in-Docker, or database egress.
#
# The script pulls a versioned Debian Rust image and exports its filesystem. Docker is used only for
# image retrieval and export; jobs run the resulting tree with `runsc`.
#
# Usage:
#   ./scripts/build-rust-rootfs.sh
#   FORCE=1 ./scripts/build-rust-rootfs.sh
#   RUST_IMAGE_TAG=rust:1.83-slim-bookworm ./scripts/build-rust-rootfs.sh
#   ALLOW_DIGEST_CHANGE=1 ./scripts/build-rust-rootfs.sh
#   MYELIN_ALLOW_REPLACE_UNMANAGED=1 ./scripts/build-rust-rootfs.sh
#
# `docker export` omits image environment metadata. The script therefore links `rustc` and `cargo`
# directly from the installed toolchain into `/usr/local/bin` instead of relying on rustup proxies.
#
# Promotion rules:
#   1. Build into `<STAGED>.versions/sha256-<digest>`, then atomically repoint `<STAGED>`.
#   2. Refuse a digest that differs from `runner-assets.toml` unless `ALLOW_DIGEST_CHANGE=1`.
#   3. Refuse an unmanaged target directory unless `MYELIN_ALLOW_REPLACE_UNMANAGED=1`; with the
#      override, rename it aside rather than deleting it.
#   4. Recompute the digest of an existing version directory before reusing it.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="${SCRIPT_DIR}/../runner-assets.toml"

ASSETS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets"
STAGED="${MYELIN_GVISOR_RUST_ROOTFS:-${ASSETS_DIR}/rust-rootfs}"
VERSIONS_DIR="${STAGED}.versions"
MANAGED_MARKER="${STAGED}.myelin-managed"
RUST_IMAGE_TAG="${RUST_IMAGE_TAG:-rust:1.95-slim-bookworm}"

die() { echo "build-rust-rootfs: $*" >&2; exit 1; }

command -v docker >/dev/null 2>&1 || die "\`docker\` is required on PATH to pull/export the Rust image"

# This is the same recipe used by `self-host.sh verify_ci_rootfs`. Tar headers include file modes.
canonical_tree_sha256() {
  tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu \
    -C "$1" -cf - . |
    sha256sum |
    awk '{print $1}'
}

# Verify an existing digest-named version directory before reusing it.
verify_version_dir_or_die() {
  local version_dir="$1" expected_digest="$2" context="$3"
  if [[ -L "${version_dir}" ]]; then
    trap - EXIT
    die "integrity violation while ${context}: ${version_dir} exists but is a SYMLINK, not a real directory — a version directory must never itself be a symlink. Refusing to reuse or delete it. Investigate by hand, then rerun."
  fi
  local actual_digest
  actual_digest="$(canonical_tree_sha256 "${version_dir}")"
  if [[ "${actual_digest}" != "${expected_digest}" ]]; then
    trap - EXIT
    die "integrity violation while ${context}: ${version_dir} exists but its content digest sha256:${actual_digest} does NOT match its own directory name (expected sha256:${expected_digest}) — this looks like corruption or tampering, not a legitimate rebuild collision. Refusing to reuse it, refusing to delete it, and refusing to touch the live ${STAGED} symlink. Quarantine ${version_dir} by hand (rename it aside), then rerun. The freshly built, already digest-verified candidate tree is preserved${TMP:+ at ${TMP}} instead of being discarded, for comparison."
  fi
}

# The committed pin for the `linux-rust-v1` row (empty if the manifest or row doesn't exist yet —
# callers of this function must treat that as "no pin to compare against", not as a match).
committed_digest() {
  [[ -f "${MANIFEST}" ]] || return 0
  awk '
    /^\[\[asset\]\]/ { in_block = 0 }
    /^id = "linux-rust-v1"/ { in_block = 1 }
    in_block && /^canonical_tree_sha256 = /{
      line = $0
      sub(/^canonical_tree_sha256 = "/, "", line)
      sub(/"$/, "", line)
      print line
      exit
    }
  ' "${MANIFEST}"
}

# Handle a pre-existing target directory.
if [[ -e "${STAGED}" && ! -L "${STAGED}" ]]; then
  if [[ -f "${MANAGED_MARKER}" ]]; then
    echo "build-rust-rootfs: ${STAGED} is a legacy (pre-versioning) tree this script previously built directly — migrating it under ${VERSIONS_DIR}" >&2
    mkdir -p "${VERSIONS_DIR}"
    legacy_digest="$(canonical_tree_sha256 "${STAGED}")"
    legacy_version_dir="${VERSIONS_DIR}/sha256-${legacy_digest}"
    if [[ -e "${legacy_version_dir}" ]]; then
      verify_version_dir_or_die "${legacy_version_dir}" "${legacy_digest}" "migrating the legacy tree"
      rm -rf "${STAGED}"
    else
      mv "${STAGED}" "${legacy_version_dir}"
    fi
    ln -s "${legacy_version_dir}" "${STAGED}"
  elif [[ "${MYELIN_ALLOW_REPLACE_UNMANAGED:-0}" == "1" ]]; then
    aside="${STAGED}.unmanaged.$(date +%s 2>/dev/null || echo "$$")"
    echo "build-rust-rootfs: ${STAGED} exists, is not a symlink, and carries no managed marker — MYELIN_ALLOW_REPLACE_UNMANAGED=1 set, renaming it aside to ${aside} (NOT deleting)" >&2
    mv "${STAGED}" "${aside}"
  else
    die "refusing to touch ${STAGED}: it exists, is not a symlink this script manages, and has no ${MANAGED_MARKER} marker — this looks like an unrelated directory, not a prior build of this script. Move it aside by hand, or set MYELIN_ALLOW_REPLACE_UNMANAGED=1 to have this script rename it aside automatically (it will never be deleted)."
  fi
fi

# Reuse a staged tree only when its digest matches the committed pin.
if [[ "${FORCE:-0}" != "1" && -x "${STAGED}/usr/local/bin/rustc" && -x "${STAGED}/usr/local/bin/cargo" ]]; then
  PIN="$(committed_digest)"
  if [[ -z "${PIN}" ]]; then
    echo "build-rust-rootfs: already staged at ${STAGED} (no committed pin to check against; FORCE=1 to re-stage)" >&2
    echo "${STAGED}"
    exit 0
  fi
  CURRENT="$(canonical_tree_sha256 "${STAGED}")"
  if [[ "${CURRENT}" == "${PIN}" ]]; then
    echo "build-rust-rootfs: already staged at ${STAGED}, matches committed pin sha256:${PIN} (FORCE=1 to re-stage)" >&2
    echo "${STAGED}"
    exit 0
  fi
  echo "build-rust-rootfs: staged tree at ${STAGED} has drifted (sha256:${CURRENT} != committed sha256:${PIN}) — rebuilding" >&2
fi

echo "build-rust-rootfs: pulling ${RUST_IMAGE_TAG} ..." >&2
docker pull "${RUST_IMAGE_TAG}" >&2

SOURCE_DIGEST="$(docker inspect --format '{{index .RepoDigests 0}}' "${RUST_IMAGE_TAG}")"
[[ -n "${SOURCE_DIGEST}" ]] || die "could not resolve a RepoDigest for ${RUST_IMAGE_TAG}"
echo "build-rust-rootfs: source image = ${RUST_IMAGE_TAG} (${SOURCE_DIGEST})" >&2

mkdir -p "${VERSIONS_DIR}"
TMP="$(mktemp -d "${VERSIONS_DIR}/.build.XXXXXX")"
# mktemp -d defaults to mode 0700 (owner-only) — fine for a scratch dir, but this directory (or its
# renamed-in-place descendant) becomes the rootfs ROOT, which the sandboxed process (non-root uid
# 65534) must be able to traverse to reach /bin/sh at all. A 0700 root silently produces "permission
# denied" finding the shell — caught by actually running the prod-exec test, not assumed. Restore the
# ordinary traversable mode plain `mkdir -p` would have used.
chmod 0755 "${TMP}"
CONTAINER_ID=""
cleanup() {
  [[ -n "${CONTAINER_ID}" ]] && docker rm "${CONTAINER_ID}" >/dev/null 2>&1 || true
  # A no-op once TMP has been promoted (renamed to its digest-named version dir) — rm -rf on an
  # already-gone path is harmless.
  rm -rf "${TMP}" 2>/dev/null || true
}
trap cleanup EXIT

CONTAINER_ID="$(docker create "${RUST_IMAGE_TAG}" sh -c 'rustup component add clippy')"

echo "build-rust-rootfs: installing the pinned toolchain's clippy component ..." >&2
docker start -a "${CONTAINER_ID}" >&2

echo "build-rust-rootfs: exporting ${RUST_IMAGE_TAG} filesystem (container ${CONTAINER_ID}) to ${TMP} ..." >&2
docker export "${CONTAINER_ID}" | tar -x -C "${TMP}"

docker rm "${CONTAINER_ID}" >/dev/null 2>&1 || true
CONTAINER_ID=""

# The enabled checkout runner binds each job's external workspace over this fixed destination.
# It is part of the content-addressed asset (and therefore the digest below), never something
# runsc may create in the shared verified tree after registry construction.
mkdir -p "${TMP}/workspace"
# The Cargo dependency asset is a separate digest-pinned tree mounted read-only here. This fixed
# destination is part of the rootfs digest and must remain empty; runsc may never create it after
# the rootfs has been verified.
mkdir -p "${TMP}/opt/myelin/cargo-vendor"
# The structured Cargo build mounts a tmpfs Cargo home at /tmp/cargo-home and binds the server
# .cargo config file at /tmp/cargo-home/config.toml (nested inside it). BOTH mount targets must be
# part of the rootfs digest: otherwise runsc's gofer creates them in the shared verified tree at
# launch (owned by the runner host uid), drifting the pinned digest so the NEXT runner startup fails
# asset re-verification with DigestMismatch (the CT-007 #26/#27 restart-integrity bug — proven by a
# 2-job+restart drill). Precreate the directory AND the nested config-file target so no launch-time
# host write to the base ever occurs. Same rationale as /workspace and /opt/myelin/cargo-vendor above.
mkdir -p "${TMP}/tmp/cargo-home"
touch "${TMP}/tmp/cargo-home/config.toml"

# --- PATH-reachability fixup (pure filesystem content — no OciConfig/hardening code touched) ------
TOOLCHAIN_DIR="$(find "${TMP}/usr/local/rustup/toolchains" -mindepth 1 -maxdepth 1 -type d | head -1)"
[[ -n "${TOOLCHAIN_DIR}" ]] || die "no rustup toolchain directory found under ${TMP}/usr/local/rustup/toolchains — unexpected image layout"
[[ -x "${TOOLCHAIN_DIR}/bin/rustc" && -x "${TOOLCHAIN_DIR}/bin/cargo" ]] ||
  die "expected real rustc/cargo binaries under ${TOOLCHAIN_DIR}/bin — unexpected image layout"

mkdir -p "${TMP}/usr/local/bin"
for tool in rustc cargo cargo-clippy clippy-driver rustfmt rustdoc; do
  target="${TOOLCHAIN_DIR}/bin/${tool}"
  if [[ -e "${target}" ]]; then
    ln -sf "$(realpath --relative-to="${TMP}/usr/local/bin" "${target}")" "${TMP}/usr/local/bin/${tool}"
  fi
done

# --- sanity checks -----------------------------------------------------------------------------
for relative in bin/sh bin/false; do
  [[ -x "${TMP}/${relative}" ]] || die "staged rootfs is missing executable ${relative} (gvisor.rs's validator requires it)"
done
[[ -x "${TMP}/usr/local/bin/rustc" ]] || die "post-fixup sanity check failed: usr/local/bin/rustc is not executable"
[[ -x "${TMP}/usr/local/bin/cargo" ]] || die "post-fixup sanity check failed: usr/local/bin/cargo is not executable"
[[ -x "${TMP}/usr/local/bin/cargo-clippy" ]] || die "post-fixup sanity check failed: usr/local/bin/cargo-clippy is not executable"
[[ -x "${TMP}/usr/local/bin/clippy-driver" ]] || die "post-fixup sanity check failed: usr/local/bin/clippy-driver is not executable"

echo "build-rust-rootfs: verifying rustc/cargo run standalone (no ambient env) ..." >&2
if ! env -i "${TMP}/usr/local/bin/rustc" --version >&2; then
  die "staged rustc does not run standalone under env -i — the toolchain fixup is broken"
fi
if ! env -i "${TMP}/usr/local/bin/cargo" --version >&2; then
  die "staged cargo does not run standalone under env -i — the toolchain fixup is broken"
fi
if ! env -i "${TMP}/usr/local/bin/cargo-clippy" --version >&2; then
  die "staged cargo-clippy does not run standalone under env -i — the toolchain fixup is broken"
fi

echo "build-rust-rootfs: computing canonical-tree sha256 digest ..." >&2
DIGEST="$(canonical_tree_sha256 "${TMP}")"

# --- digest-change safety: refuse to promote a tree that disagrees with the committed pin ----------
PIN="$(committed_digest)"
if [[ -n "${PIN}" && "${DIGEST}" != "${PIN}" ]]; then
  if [[ "${ALLOW_DIGEST_CHANGE:-0}" != "1" ]]; then
    die "built tree's digest sha256:${DIGEST} does NOT match the committed pin sha256:${PIN} in ${MANIFEST}. Refusing to promote. If this is an INTENTIONAL change (e.g. a new RUST_IMAGE_TAG), rerun with ALLOW_DIGEST_CHANGE=1 and then update runner-assets.toml's canonical_tree_sha256 to sha256:${DIGEST} yourself. If unexpected, investigate before promoting — the build produced content different from the version already trusted."
  fi
  echo "build-rust-rootfs: ALLOW_DIGEST_CHANGE=1 set — promoting a tree whose digest (sha256:${DIGEST}) differs from the committed pin (sha256:${PIN}). Remember to update runner-assets.toml." >&2
fi

# --- atomic versioned promotion: build a new immutable version dir, then atomically repoint the ----
# --- managed symlink onto it — never delete-then-recreate the live path -------------------------
VERSION_DIR="${VERSIONS_DIR}/sha256-${DIGEST}"
if [[ -e "${VERSION_DIR}" ]]; then
  verify_version_dir_or_die "${VERSION_DIR}" "${DIGEST}" "promoting the freshly built tree"
  rm -rf "${TMP}"
else
  mv "${TMP}" "${VERSION_DIR}"
fi
trap - EXIT
[[ -n "${CONTAINER_ID}" ]] && docker rm "${CONTAINER_ID}" >/dev/null 2>&1 || true

NEW_LINK="${STAGED}.new-symlink.$$"
ln -s "${VERSION_DIR}" "${NEW_LINK}"
mv -T "${NEW_LINK}" "${STAGED}"
touch "${MANAGED_MARKER}"

echo "build-rust-rootfs: done — ${STAGED} -> ${VERSION_DIR}" >&2
echo "build-rust-rootfs: set MYELIN_GVISOR_RUST_ROOTFS=${STAGED}" >&2
echo "build-rust-rootfs: source image digest  = ${SOURCE_DIGEST}" >&2
echo "build-rust-rootfs: canonical-tree digest = sha256:${DIGEST}" >&2
echo "${STAGED}"
