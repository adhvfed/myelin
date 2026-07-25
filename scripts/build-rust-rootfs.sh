#!/usr/bin/env bash
# Stage a Rust-capable gVisor rootfs — the CT-007 gate-2 (planning/system-reviews/2026-06-26/
# 12-ci-track-ledger.md, "Pre-registered CT-007 cutover floor", item 2/4) RUNNER ASSET for the
# `build-test-clippy` job's need (ci-workload-inventory.toml: "The base Rust workload every other
# job's crates depend on existing"). Node/browser/Docker-in-Docker/advisory-DB-egress capabilities
# are OUT OF SCOPE here — separate runner assets, separate scripts, later slices of the same gate.
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
#                                           (idempotent — a no-op if already staged)
#   FORCE=1 ./scripts/build-rust-rootfs.sh  re-stage from scratch (removes the existing staged tree,
#                                           re-pulls the image, re-exports)
#   RUST_IMAGE_TAG=rust:1.83-slim-bookworm ./scripts/build-rust-rootfs.sh
#                                           override the source image tag (must resolve to a REAL
#                                           existing tag — this script does not invent one)
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
set -euo pipefail

ASSETS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gvisor-assets"
STAGED="${MYELIN_GVISOR_RUST_ROOTFS:-${ASSETS_DIR}/rust-rootfs}"
RUST_IMAGE_TAG="${RUST_IMAGE_TAG:-rust:1.82-slim-bookworm}"

die() { echo "build-rust-rootfs: $*" >&2; exit 1; }

command -v docker >/dev/null 2>&1 || die "\`docker\` is required on PATH to pull/export the Rust image"

# Idempotent: a staged tree with a guest rustc already present is a no-op unless FORCE=1.
if [[ "${FORCE:-0}" != "1" && -x "${STAGED}/usr/local/bin/rustc" && -x "${STAGED}/usr/local/bin/cargo" ]]; then
  echo "build-rust-rootfs: already staged at ${STAGED} (FORCE=1 to re-stage)" >&2
  echo "${STAGED}"
  exit 0
fi

echo "build-rust-rootfs: pulling ${RUST_IMAGE_TAG} ..." >&2
docker pull "${RUST_IMAGE_TAG}" >&2

SOURCE_DIGEST="$(docker inspect --format '{{index .RepoDigests 0}}' "${RUST_IMAGE_TAG}")"
[[ -n "${SOURCE_DIGEST}" ]] || die "could not resolve a RepoDigest for ${RUST_IMAGE_TAG}"
echo "build-rust-rootfs: source image = ${RUST_IMAGE_TAG} (${SOURCE_DIGEST})" >&2

rm -rf "${STAGED}"
mkdir -p "${STAGED}"

CONTAINER_ID="$(docker create "${RUST_IMAGE_TAG}" true)"
trap 'docker rm "${CONTAINER_ID}" >/dev/null 2>&1 || true' EXIT

echo "build-rust-rootfs: exporting ${RUST_IMAGE_TAG} filesystem (container ${CONTAINER_ID}) to ${STAGED} ..." >&2
docker export "${CONTAINER_ID}" | tar -x -C "${STAGED}"

docker rm "${CONTAINER_ID}" >/dev/null 2>&1 || true
trap - EXIT

# --- PATH-reachability fixup (pure filesystem content — no OciConfig/hardening code touched) ------
# Locate the real (non-proxy) toolchain binaries under rustup's toolchain dir (NOT the
# usr/local/cargo/bin/* rustup-proxy hardlinks, which depend on env/HOME resolution this sandbox
# does not set up).
TOOLCHAIN_DIR="$(find "${STAGED}/usr/local/rustup/toolchains" -mindepth 1 -maxdepth 1 -type d | head -1)"
[[ -n "${TOOLCHAIN_DIR}" ]] || die "no rustup toolchain directory found under ${STAGED}/usr/local/rustup/toolchains — unexpected image layout"
[[ -x "${TOOLCHAIN_DIR}/bin/rustc" && -x "${TOOLCHAIN_DIR}/bin/cargo" ]] ||
  die "expected real rustc/cargo binaries under ${TOOLCHAIN_DIR}/bin — unexpected image layout"

mkdir -p "${STAGED}/usr/local/bin"
for tool in rustc cargo cargo-clippy clippy-driver rustfmt rustdoc; do
  target="${TOOLCHAIN_DIR}/bin/${tool}"
  if [[ -e "${target}" ]]; then
    ln -sf "$(realpath --relative-to="${STAGED}/usr/local/bin" "${target}")" "${STAGED}/usr/local/bin/${tool}"
  fi
done

# --- sanity checks -----------------------------------------------------------------------------
for relative in bin/sh bin/false; do
  [[ -x "${STAGED}/${relative}" ]] || die "staged rootfs is missing executable ${relative} (gvisor.rs's validator requires it)"
done
[[ -x "${STAGED}/usr/local/bin/rustc" ]] || die "post-fixup sanity check failed: usr/local/bin/rustc is not executable"
[[ -x "${STAGED}/usr/local/bin/cargo" ]] || die "post-fixup sanity check failed: usr/local/bin/cargo is not executable"

echo "build-rust-rootfs: verifying rustc/cargo run standalone (no ambient env) ..." >&2
if ! env -i "${STAGED}/usr/local/bin/rustc" --version >&2; then
  die "staged rustc does not run standalone under env -i — the toolchain fixup is broken"
fi
if ! env -i "${STAGED}/usr/local/bin/cargo" --version >&2; then
  die "staged cargo does not run standalone under env -i — the toolchain fixup is broken"
fi

# Note: unlike stage-git-rootfs.sh's git-wire mounts (/repo, /quarantine — required because that
# path bind-mounts a bare repo + quarantine dir into the sandbox), a bare `rustc --version`/`cargo
# build` smoke needs no pre-created bind-mount target: this rootfs carries no extra_mounts, so no
# mount-point directories need to pre-exist in the read-only root.

echo "build-rust-rootfs: computing canonical-tree sha256 digest ..." >&2
DIGEST="$(
  tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu \
    -C "${STAGED}" -cf - . |
    sha256sum |
    awk '{print $1}'
)"

echo "build-rust-rootfs: done — set MYELIN_GVISOR_RUST_ROOTFS=${STAGED}" >&2
echo "build-rust-rootfs: source image digest  = ${SOURCE_DIGEST}" >&2
echo "build-rust-rootfs: canonical-tree digest = sha256:${DIGEST}" >&2
echo "${STAGED}"
