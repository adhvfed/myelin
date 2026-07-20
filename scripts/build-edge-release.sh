#!/usr/bin/env bash
# Build a native edge release bundle. The edge intentionally runs host-native because its Git wire
# launches rootless gVisor and must own delegated cgroup-v2 memory controls; nesting that contract in
# an ordinary application container would be misleading and usually non-functional.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

revision="${MYELIN_RELEASE_REVISION:-$(git rev-parse --verify HEAD)}"
if [[ ! "${revision}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "build-edge-release: revision must be a full 40-character Git object id" >&2
  exit 2
fi
dirty_suffix=""
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  if [[ "${MYELIN_ALLOW_DIRTY:-0}" != "1" ]]; then
    echo "build-edge-release: refusing to label a dirty checkout as revision ${revision}" >&2
    exit 1
  fi
  dirty_suffix="-dirty"
fi
target="$(rustc -vV | sed -n 's/^host: //p')"
if [[ -z "${target}" ]]; then
  echo "build-edge-release: rustc did not report a host target" >&2
  exit 1
fi

bundle_name="myelin-edge-${revision}${dirty_suffix}-${target}"
bundle_root="${REPO_ROOT}/target/release-bundles"
stage="${bundle_root}/${bundle_name}"
archive="${bundle_root}/${bundle_name}.tar.gz"
mkdir -p "${bundle_root}"
rm -rf "${stage}"
rm -f "${archive}"

cargo build --release --locked -p myelin-edge --bin edge
install -Dm755 target/release/edge "${stage}/edge"
install -Dm644 docs/edge-deployment.md "${stage}/edge-deployment.md"

(
  cd "${stage}"
  sha256sum edge edge-deployment.md > SHA256SUMS
  sha256sum --check SHA256SUMS
)

tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
  -C "${bundle_root}" -cf - "${bundle_name}" | gzip -n > "${archive}"
(
  cd "${bundle_root}"
  sha256sum "${bundle_name}.tar.gz" > "${bundle_name}.tar.gz.sha256"
)
echo "${archive}"
