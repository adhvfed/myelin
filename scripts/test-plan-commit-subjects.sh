#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
gate="$root/scripts/check-plan-commit-subjects.sh"

printf '%s\n' \
  'L2: split the edge module' \
  'R3.4: harden tree pagination' \
  'P0.2: promote the supply-chain floor' \
  'P-S30: re-green shim conformance' \
  'Merge pull request #42 from acme/topic' \
  | "$gate" --stdin >/dev/null

if printf '%s\n' 'feat(git): untracked work' | "$gate" --stdin >/dev/null 2>&1; then
  echo "red fixture passed: an untagged subject must fail" >&2
  exit 1
fi

if printf '%s\n' 'R3.4 harden without delimiter' | "$gate" --stdin >/dev/null 2>&1; then
  echo "red fixture passed: a tag without the canonical colon-space delimiter must fail" >&2
  exit 1
fi

echo "L4: green and red commit-subject fixtures pass"
