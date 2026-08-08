#!/usr/bin/env bash
# Run the Rust integration suite against the real local data services.
#
# Usage:
#   scripts/integration-test.sh            # test; leave services running
#   scripts/integration-test.sh --down     # test; stop services afterward
#   scripts/integration-test.sh --nuke     # test; stop services and remove managed state
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

TEARDOWN="${1:-none}"

cleanup() {
  case "${TEARDOWN}" in
    --down) echo "==> stopping services"; fed stop ;;
    --nuke) echo "==> stopping services and removing managed state"; fed stop; fed clean ;;
    *)      echo "==> leaving services running (pass --down or --nuke to tear them down)" ;;
  esac
}
trap cleanup EXIT

echo "==> running backend integration tests"
fed test:backend -- --nocapture
