#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
metric="$root/scripts/plan-payoff-metrics.sh"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT

git -C "$fixture" init -q
git -C "$fixture" config user.name "L3 fixture"
git -C "$fixture" config user.email "l3@example.invalid"

printf 'base\n' >"$fixture/a.txt"
git -C "$fixture" add a.txt
git -C "$fixture" commit -q -m "fixture baseline"
base="$(git -C "$fixture" rev-parse HEAD)"

printf 'one\ntwo\n' >"$fixture/a.txt"
git -C "$fixture" commit -qam "R3.1: first increment"
printf 'three\n' >>"$fixture/a.txt"
printf 'new\n' >"$fixture/b.txt"
git -C "$fixture" add a.txt b.txt
git -C "$fixture" commit -q -m "R3.1: second increment"
printf 'changed\n' >"$fixture/b.txt"
git -C "$fixture" commit -qam "P0.2: later phase"
head="$(git -C "$fixture" rev-parse HEAD)"

report="$(cd "$fixture" && "$metric" --json "$base" "$head")"
[[ "$report" == *'"row":"R3.1","commits":2,"lines_changed":5,"additions":4,"deletions":1,"files_touched":2'* ]] || {
  echo "L3 green fixture lost the R3.1 aggregate: $report" >&2
  exit 1
}
[[ "$report" == *'"row":"P0.2","commits":1,"lines_changed":2,"additions":1,"deletions":1,"files_touched":1'* ]] || {
  echo "L3 green fixture lost the P0.2 aggregate: $report" >&2
  exit 1
}

printf 'untracked\n' >"$fixture/c.txt"
git -C "$fixture" add c.txt
git -C "$fixture" commit -q -m "feat(ci): untraceable fixture"
if (cd "$fixture" && "$metric" --json "$base" HEAD >/dev/null 2>&1); then
  echo "L3 red fixture passed: an untraceable commit must fail attribution" >&2
  exit 1
fi

echo "L3: green aggregation and red attribution fixtures pass"
