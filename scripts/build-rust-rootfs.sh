#!/usr/bin/env bash
# Stage a Rust-capable gVisor rootfs — a Rust TOOLCHAIN SMOKE asset toward the CT-007 gate-2
# (planning/system-reviews/2026-06-26/12-ci-track-ledger.md, "Pre-registered CT-007 cutover floor",
# item 2/4) `build-test-clippy` job. It proves the Rust toolchain executes under the hardened
# sandbox on the exact production hardening path — it is NOT yet proof of that job's full
# capability (no checkout mount, no vendored/locked deps, no env propagation, no resource sizing
# proven against a real build — see ci-workload-inventory.toml's `migration_state =
# "capability-smoke"` on that row, and the ledger's 2026-07-25 correction entry). Node/browser/
# Docker-in-Docker/advisory-DB-egress capabilities are OUT OF SCOPE here — separate runner assets,
# separate scripts, later slices of the same gate.
#
# THIS IS NOT THE PRODUCTION `linux-small-v1` profile (`.myelin/ci.toml`'s pinned
# `myelin.local/linux-small-v1-rootfs`) and does not touch it, its base busybox rootfs
# (~/.local/share/gvisor-assets/rootfs), or the git-capable rootfs
# (~/.local/share/gvisor-assets/git-rootfs) staged by scripts/stage-git-rootfs.sh. It stages a
# SEPARATE, independent asset used only to prove the Rust capability runs under the SAME
# GvisorBackend::launch/launch_streaming hardened path (see
# crates/myelin-ci-sandbox/tests/rust_capable_rootfs_prod_exec_test.rs).
#
# UNLIKE stage-git-rootfs.sh (which hand-copies a single host binary + its 2 shared libs onto the
# base busybox rootfs), the host's system `cargo`/`rustc` pull in DOZENS of transitive shared libs
# (libgit2, libssl, libcurl, icu, ...) — hand-staging all of those would be fragile AND not a real
# "digest-pinned reproducible artifact" (it would just be whatever pacman happens to have installed
# today). Instead this script pulls an OFFICIAL, versioned `rust:<version>-slim-bookworm` Debian
# image, pins the exact digest it resolves, and exports its REAL filesystem tree with `docker
# export` — a full, coherent Debian userland + toolchain, not a hand-chased lib list.
#
#   ./scripts/build-rust-rootfs.sh          stage into ~/.local/share/gvisor-assets/rust-rootfs
#                                           (idempotent — a no-op if the CURRENT tree's own
#                                           canonical-tree digest still matches the committed pin in
#                                           runner-assets.toml; a mismatch triggers a rebuild even
#                                           without FORCE=1. If no committed pin exists yet at all —
#                                           only possible before this asset's first-ever manifest
#                                           row is written — the existing tree is left exactly as-is
#                                           with no rebuild, since there is nothing to compare it
#                                           against; FORCE=1 to rebuild regardless)
#   FORCE=1 ./scripts/build-rust-rootfs.sh  re-stage unconditionally (re-pulls the image, re-exports)
#   RUST_IMAGE_TAG=rust:1.83-slim-bookworm ./scripts/build-rust-rootfs.sh
#                                           override the source image tag (must resolve to a REAL
#                                           existing tag — this script does not invent one)
#   ALLOW_DIGEST_CHANGE=1                  required if the freshly built tree's digest does NOT
#                                           match the committed pin — see "DIGEST-CHANGE SAFETY" below
#   MYELIN_ALLOW_REPLACE_UNMANAGED=1       required if MYELIN_GVISOR_RUST_ROOTFS names an existing
#                                           real directory this script did not create — see
#                                           "OVERRIDE-TARGET SAFETY" below
#
# Prereqs: `docker` on PATH (used ONLY to pull + export a filesystem; the sandbox that later RUNS
# this rootfs is `runsc`, never docker — docker here is strictly an image-fetch/export tool, exactly
# as it is already used for the `web-container` CI job's Dockerfile builds elsewhere in this repo).
#
# PATH GOTCHA (real, not hypothetical): `docker export` exports ONLY the filesystem, not the image's
# `ENV`/`CMD` metadata, so the official image's `PATH=/usr/local/cargo/bin:...` is LOST on export.
# This repo's gVisor guest PATH is hardcoded to
# `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`
# (crates/myelin-ci-sandbox/src/gvisor.rs, OciConfig::to_json) and does NOT read the source image's
# env — so `rustc`/`cargo` must be made reachable on THAT path by pure filesystem content, not by
# touching the hardening/OCI-config code. Symlinking `/usr/local/bin/{rustc,cargo}` is deliberately
# NOT a symlink to the `/usr/local/cargo/bin/*` rustup PROXY binaries (those dispatch via
# rustup's own $CARGO_HOME/$RUSTUP_HOME resolution, which depends on env/HOME state this sandbox
# does not set up) but directly to the REAL toolchain binaries under
# `/usr/local/rustup/toolchains/<host-triple>/bin/{rustc,cargo}` — confirmed by hand to run cleanly
# under `env -i` (no ambient env) via their own relative-to-binary ($ORIGIN) rpath, so they work
# regardless of what env vars the sandbox sets.
#
# STAGING SAFETY (2026-07-25, three findings from adversarial review by gpt-5.6-sol, fixed here):
#
# 1. ATOMIC VERSIONED PROMOTION. Content is staged into an immutable, digest-named directory under
#    `<STAGED>.versions/sha256-<digest>/`, never in place. `<STAGED>` itself is a SYMLINK this script
#    manages, atomically repointed (`mv -T` a freshly created symlink onto the old one — a rename,
#    not a delete-then-create) to the new version directory only after that directory is fully built
#    and verified. A failure at ANY point before the final `mv -T` leaves the previous live symlink
#    (and everything it points to) completely untouched — there is no window where `<STAGED>` is
#    absent, and no in-place deletion of the tree currently in use.
# 2. DIGEST-CHANGE SAFETY. Before promotion, if `runner-assets.toml` already carries a committed pin
#    for this asset, the freshly built tree's digest MUST match it, or the script refuses to promote
#    (fails closed) — a mutable source-image tag drift, an export anomaly, or a regression in this
#    script's own fixup logic must never silently replace a known-good asset with a DIFFERENT one.
#    `ALLOW_DIGEST_CHANGE=1` is the explicit, conscious override for a genuinely intentional change
#    (e.g. a new `RUST_IMAGE_TAG`) — the operator must then update the committed pin themselves.
# 3. OVERRIDE-TARGET SAFETY. `MYELIN_GVISOR_RUST_ROOTFS` can point anywhere, so this script never
#    deletes or renames an EXISTING REAL (non-symlink) directory at that path unless a sidecar marker
#    file (`<STAGED>.myelin-managed`, sitting NEXT TO the directory, never inside the hashed tree
#    content) proves this script created it. Without that marker, it refuses outright unless
#    `MYELIN_ALLOW_REPLACE_UNMANAGED=1` is set, and even then only renames the unrecognized directory
#    aside (to `<STAGED>.unmanaged.<timestamp>`) — never deletes it.
#
# INTEGRITY-CHECKED REUSE (2026-07-25, a reproduced bug from a second adversarial round by
# gpt-5.6-sol, fixed here). Both the legacy-migration path and the versioned-promotion path can
# encounter an ALREADY-EXISTING directory named after the digest they're about to place there
# (`sha256-<digest>/`). The original code trusted the directory's NAME alone and discarded the
# freshly-built (and already digest-verified) replacement — so if that existing directory had ever
# drifted from its own name (e.g. something wrote an extra file into it after the fact), this script
# would silently keep serving the CORRUPTED tree under a digest that no longer describes it, having
# just deleted the correct one. `verify_version_dir_or_die` closes this: before ANY code path treats
# an existing `sha256-<digest>/` directory as reusable, it recomputes that directory's OWN canonical
# digest and requires it to still equal its name. A mismatch refuses loudly, preserves BOTH the
# corrupt existing directory and the freshly-built correct candidate (for operator comparison/
# quarantine), and never touches the live symlink — reuse must never mean "trust the pathname."
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="${SCRIPT_DIR}/../runner-assets.toml"

ASSETS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets"
STAGED="${MYELIN_GVISOR_RUST_ROOTFS:-${ASSETS_DIR}/rust-rootfs}"
VERSIONS_DIR="${STAGED}.versions"
MANAGED_MARKER="${STAGED}.myelin-managed"
RUST_IMAGE_TAG="${RUST_IMAGE_TAG:-rust:1.82-slim-bookworm}"

die() { echo "build-rust-rootfs: $*" >&2; exit 1; }

command -v docker >/dev/null 2>&1 || die "\`docker\` is required on PATH to pull/export the Rust image"

# The exact recipe scripts/dogfood.sh's verify_ci_rootfs() uses, reused for content pins here too.
# NOTE: this hashes file MODE bits too (via the tar header), not just content — a directory promoted
# with the wrong permissions (e.g. mktemp -d's default 0700) produces a DIFFERENT digest, which is
# exactly how a real permission regression was caught here on 2026-07-25 (see runner-assets.toml).
canonical_tree_sha256() {
  tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu \
    -C "$1" -cf - . |
    sha256sum |
    awk '{print $1}'
}

# Require an existing `${VERSIONS_DIR}/sha256-<digest>` directory to actually BE what its name
# claims before any caller treats it as reusable — see "INTEGRITY-CHECKED REUSE" above. Takes the
# version dir path, the digest its name claims, and a human label for the "what were we about to do"
# error message. Dies loudly (preserving the version dir AND, per the caller's own trap state, the
# freshly-built candidate) on a symlinked version dir or a content/name mismatch; otherwise returns
# silently (0) meaning "safe to reuse, discard the redundant freshly-built candidate instead."
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

# --- override-target safety: never delete/rename an existing real directory we didn't create ------
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

# --- idempotent no-op check: compare the CURRENT tree's OWN digest against the committed pin -------
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

CONTAINER_ID="$(docker create "${RUST_IMAGE_TAG}" true)"

echo "build-rust-rootfs: exporting ${RUST_IMAGE_TAG} filesystem (container ${CONTAINER_ID}) to ${TMP} ..." >&2
docker export "${CONTAINER_ID}" | tar -x -C "${TMP}"

docker rm "${CONTAINER_ID}" >/dev/null 2>&1 || true
CONTAINER_ID=""

# The enabled checkout runner binds each job's external workspace over this fixed destination.
# It is part of the content-addressed asset (and therefore the digest below), never something
# runsc may create in the shared verified tree after registry construction.
mkdir -p "${TMP}/workspace"

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

echo "build-rust-rootfs: verifying rustc/cargo run standalone (no ambient env) ..." >&2
if ! env -i "${TMP}/usr/local/bin/rustc" --version >&2; then
  die "staged rustc does not run standalone under env -i — the toolchain fixup is broken"
fi
if ! env -i "${TMP}/usr/local/bin/cargo" --version >&2; then
  die "staged cargo does not run standalone under env -i — the toolchain fixup is broken"
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
