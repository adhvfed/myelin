#!/usr/bin/env bash
# L3 — derive the compounding-payoff signal from immutable, L4-tagged Git history.
set -euo pipefail

traceability_start="51e7841d"
format="markdown"

usage() {
  echo "usage: $0 [--json] [<base-sha> <head-sha>]" >&2
  exit 2
}

if [[ "${1:-}" == "--json" ]]; then
  format="json"
  shift
fi

if [[ "$#" -eq 0 ]]; then
  base="${traceability_start}^"
  head="HEAD"
elif [[ "$#" -eq 2 ]]; then
  base="$1"
  head="$2"
else
  usage
fi

git rev-parse --verify "${base}^{commit}" >/dev/null
git rev-parse --verify "${head}^{commit}" >/dev/null
if ! git merge-base --is-ancestor "$base" "$head"; then
  echo "L3: base $base is not an ancestor of head $head" >&2
  exit 1
fi

declare -A row_commits=()
declare -A row_lines=()
declare -A row_additions=()
declare -A row_deletions=()
declare -A row_files=()
declare -A row_binary_files=()
declare -A seen_file=()
declare -A seen_binary_file=()

while IFS= read -r commit; do
  [[ -n "$commit" ]] || continue
  subject="$(git show -s --format=%s "$commit")"
  if [[ "$subject" =~ ^(L[1-5]|R[0-9]+(\.[0-9]+)*|P([0-9]+(\.[0-9]+)*|-[A-Za-z0-9][A-Za-z0-9.-]*)):\  ]]; then
    row="${BASH_REMATCH[1]}"
  elif [[ "$subject" =~ ^Merge\ (pull\ request\ \#[0-9]+|branch\ ) ]]; then
    continue
  else
    echo "L3: cannot attribute commit $commit to a plan row: $subject" >&2
    exit 1
  fi

  row_commits["$row"]=$(( ${row_commits["$row"]:-0} + 1 ))
  while IFS= read -r -d '' record; do
    added="${record%%$'\t'*}"
    remainder="${record#*$'\t'}"
    deleted="${remainder%%$'\t'*}"
    if [[ "$added" == "-" || "$deleted" == "-" ]]; then
      continue
    fi
    row_additions["$row"]=$(( ${row_additions["$row"]:-0} + added ))
    row_deletions["$row"]=$(( ${row_deletions["$row"]:-0} + deleted ))
    row_lines["$row"]=$(( ${row_lines["$row"]:-0} + added + deleted ))
  done < <(git diff-tree --root --no-commit-id --numstat -z -r --no-renames "$commit")

  while IFS= read -r -d '' file_name; do
    key="${row}"$'\x1f'"${file_name}"
    if [[ -z "${seen_file["$key"]+present}" ]]; then
      seen_file["$key"]=1
      row_files["$row"]=$(( ${row_files["$row"]:-0} + 1 ))
    fi
  done < <(git diff-tree --root --no-commit-id --name-only -z -r --no-renames "$commit")

  while IFS= read -r -d '' record; do
    added="${record%%$'\t'*}"
    remainder="${record#*$'\t'}"
    deleted="${remainder%%$'\t'*}"
    file_name="${remainder#*$'\t'}"
    if [[ "$added" != "-" && "$deleted" != "-" ]]; then
      continue
    fi
    key="${row}"$'\x1f'"${file_name}"
    if [[ -z "${seen_binary_file["$key"]+present}" ]]; then
      seen_binary_file["$key"]=1
      row_binary_files["$row"]=$(( ${row_binary_files["$row"]:-0} + 1 ))
    fi
  done < <(git diff-tree --root --no-commit-id --numstat -z -r --no-renames "$commit")
done < <(git rev-list --reverse "$base..$head")

if [[ "${#row_commits[@]}" -eq 0 ]]; then
  echo "L3: no plan-row commits found in $base..$head" >&2
  exit 1
fi
mapfile -t rows < <(printf '%s\n' "${!row_commits[@]}" | LC_ALL=C sort -V)

if [[ "$format" == "json" ]]; then
  printf '{"schema_version":1,"base":"%s","head":"%s","rows":[' \
    "$(git rev-parse "$base")" "$(git rev-parse "$head")"
  separator=""
  for row in "${rows[@]}"; do
    printf '%s{"row":"%s","commits":%d,"lines_changed":%d,"additions":%d,"deletions":%d,"files_touched":%d,"binary_files_touched":%d}' \
      "$separator" "$row" "${row_commits["$row"]}" "${row_lines["$row"]:-0}" \
      "${row_additions["$row"]:-0}" "${row_deletions["$row"]:-0}" \
      "${row_files["$row"]:-0}" "${row_binary_files["$row"]:-0}"
    separator=","
  done
  printf ']}\n'
  exit 0
fi

printf '## L3 compounding-payoff metric\n\n'
printf 'Range: `%s..%s`. Lines changed are additions + deletions; files are unique within each plan row. Binary files count as touched but contribute no lines.\n\n' \
  "$(git rev-parse --short "$base")" "$(git rev-parse --short "$head")"
printf '| Plan row | Commits | Lines changed | Files touched | Binary files |\n'
printf '|---|---:|---:|---:|---:|\n'
for row in "${rows[@]}"; do
  printf '| %s | %d | %d | %d | %d |\n' "$row" "${row_commits["$row"]}" \
    "${row_lines["$row"]:-0}" "${row_files["$row"]:-0}" "${row_binary_files["$row"]:-0}"
done
