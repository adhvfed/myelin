#!/usr/bin/env bash
# L4 — every authored commit names the plan row it advances. Platform-generated merge commits are
# the sole exception; ordinary feat/fix/docs subjects are not traceability.
set -euo pipefail

# The founder plan made L4 binding at this already-tagged commit. Three older in-flight commits were
# rebased above the plan document before that instruction was read; they are pre-ratchet history.
traceability_start="51e7841d"

usage() {
  echo "usage: $0 --stdin | <base-sha> <head-sha>" >&2
  exit 2
}

subjects=""
if [[ "${1:-}" == "--stdin" ]]; then
  [[ "$#" -eq 1 ]] || usage
  subjects="$(cat)"
elif [[ "$#" -eq 2 ]]; then
  base="$1"
  head="$2"
  if [[ "$base" =~ ^0+$ ]]; then
    base="${head}^"
  fi
  if git merge-base --is-ancestor "$traceability_start" "$head" &&
      git merge-base --is-ancestor "$base" "$traceability_start"; then
    base="$traceability_start"
  fi
  subjects="$(git log --format=%s "$base..$head")"
else
  usage
fi

failed=0
while IFS= read -r subject; do
  [[ -n "$subject" ]] || continue
  if [[ "$subject" =~ ^(L[1-5]|R[0-9]+(\.[0-9]+)*|P([0-9]+(\.[0-9]+)*|-[A-Za-z0-9][A-Za-z0-9.-]*)):\  ]]; then
    continue
  fi
  if [[ "$subject" =~ ^Merge\ (pull\ request\ \#[0-9]+|branch\ ) ]]; then
    continue
  fi
  echo "L4: commit subject lacks a plan-row prefix: $subject" >&2
  failed=1
done <<< "$subjects"

if [[ "$failed" -ne 0 ]]; then
  echo "L4: expected subjects such as 'R3.4: ...', 'P0.2: ...', or 'L2: ...'" >&2
  exit 1
fi

echo "L4: commit subjects are plan-row traceable"
