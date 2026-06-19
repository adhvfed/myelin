# Sketch 07 — Diff/comment anchoring across rewrites (TE-22)

> Exploration note. The primary correctness/UX battleground (Phase-1 §4.3, Phase-2 §9): keep an inline
> review comment anchored to the right line as the PR head moves (force-push, rebase, amend, new
> commits) and the base branch advances. Where competitors visibly differ in quality. Date: 2026-06-19.

## The problem, precisely

A review comment is placed on "file X, line 42, the `new` side, in diff of commit C". Then the author
force-pushes a rebased branch: commit C no longer exists; line 42 may have moved, changed, or vanished.
The comment must either **relocate to the equivalent line** or be marked **outdated** (with "show in
original context") — never silently point at the wrong line, never crash, never leak.

## What to store as the anchor (the data-model call)

Naive `(file, line_number)` is fragile (Phase-1 §4.3). The durable anchor records enough to **recompute
position by diffing**, not a bare line number. The committed anchor:

```
CommentAnchor {
  path:        "src/auth/sso.rs",        // file path on the anchored side
  side:        Old | New,                // which side of the diff (deletion vs addition context)
  blob_sha:    <sha of the file blob at anchor time>,   // content-addressed: survives commit rewrites
  commit_sha:  <head commit at anchor time>,            // for "show in original context"
  line:        42,                       // line within that blob
  hunk_context: <N lines of surrounding text>,          // fuzzy fallback if line mapping is ambiguous
  diff_hunk:   <the hunk header the comment was made in>
}
```

**Key insight (the prior art):** anchor to the **blob SHA + line**, not the commit. A blob's content is
content-addressed, so "line 42 of *this blob*" is stable even when the commit that contained it is
rewritten — as long as that blob still appears somewhere reachable. GitHub/GitLab both key on blob SHA
+ position and remap. (Verified directionally against GitLab's `Gitlab::Git::Compare` diff-refresh
model; GitHub's review API exposes `original_commit_id`/`commit_id` + position + `diff_hunk`.)

## The remap algorithm (on every PR head/base move)

```
on git.pr.synchronized | base advanced:                  # head moved or base changed
  new_head_blob = blob of path at new head
  for each open comment on (path):
     if anchor.blob_sha == new_head_blob_sha:
        position unchanged → keep                          # fast path: file didn't change
     else:
        mapping = diff(anchor.blob_sha, new_head_blob)     # gix in-process diff (sketch 02)
        new_line = map_line(anchor.line, mapping)          # line-tracking through the diff hunks
        if new_line exists and content matches (or hunk_context matches fuzzily):
           relocate the comment to new_line on new_head_blob
        else:
           mark thread OUTDATED (keep anchor.commit_sha for "show in original context")
```

- **Line mapping** uses the **per-line diff** between the two blobs (Myers diff / git's diff machinery):
  unchanged lines map directly; the comment moves with its line; if the line was deleted, the thread
  goes outdated. **`hunk_context`** is the fuzzy fallback when the line mapping is ambiguous (e.g. a
  block moved) — match on surrounding content, à la `patch`'s fuzz.
- **"Changes since you last reviewed" / incremental review** is the same machinery: store, per
  reviewer, the head sha they last reviewed; the diff between that sha and the current head is the
  "what's new" view (Phase-1 §4.2 — depends on diff-position tracking).

## Candidate approaches weighed

- **A. Store `(commit, path, line, side)`, recompute by re-diffing on every view.** Simple to store but
  loses the anchor the moment the commit is rewritten (outdated immediately on rebase). *Too brittle —
  rebases are common; everything goes outdated.*
- **B. Store `(blob_sha, path, line, side) + hunk_context`, remap by blob-diff on head/base move.**
  Survives commit rewrites (blob-addressed); relocates across rebase/amend when the line still exists;
  outdated only when the line truly changed/vanished. **This is the leaning.**
- **C. Track positions live via an interval/op-transform structure per file.** Most precise (CRDT-like
  position tracking) but heavy, and overkill — review comments are coarse-grained and the blob-diff
  remap is sufficient + matches user mental model ("outdated" is an accepted, legible state). Reserve as
  a follow-on if measured precision is insufficient.

## State surfaced in the UI (ties to wireframes)

- **Current** — anchored, relocates silently as the head moves.
- **Outdated** — the anchored line no longer exists at head; the thread collapses to an "Outdated"
  badge with **"show in original context"** (renders the original blob@commit_sha). Never leaks, never
  mis-points.
- **Resolved** — orthogonal review state (resolvable threads), independent of outdated.

These are the `§5.10` cross-cutting states applied to the comment thread (the wireframes show
current/outdated/resolved as first-class).

## `#sub` anchor stability (Refs obligation)

The comment's `ArtifactRef` sub-anchor (`myelin://…/git/pr/88#comment-12`, and the diff-line anchor
`…/git/blob/<repo>/<path>#L42` per contract 5.7) must be **stable across edits** so embeds in chat/docs
don't dangle. The **comment id is stable** (an opaque mint, never the line number); the *position*
remaps but the *id* is forever. A line-range ref (`#L42-L88`) resolves to its blob@commit at resolution
time and tombstones gracefully if the blob is erased (Refs tombstoning).

## Leaning (committed in findings)

**Candidate B**: anchor on **blob SHA + path + line + side + hunk_context + commit_sha**; **remap by
in-process blob-diff (gix) on every PR head/base move**; relocate when the line survives, mark
**outdated** (with "show in original context") when it doesn't. Comment **ids are stable opaque mints**
(the `#sub` ref never changes); positions remap. Incremental "changes since you last reviewed" reuses
the same diff machinery against a per-reviewer last-seen head sha. **Floor named:** fuzzy `hunk_context`
fallback covers most moves; precise op-transform position tracking (Candidate C) is the follow-on if
measured precision is insufficient.

## Prior art / sources

- GitHub review API anchoring (`commit_id`/`original_commit_id` + `position`/`original_position` +
  `diff_hunk`); GitLab diff-refresh via `Gitlab::Git::Compare` (verified directionally, 2026-06).
- Myers, *An O(ND) Difference Algorithm* (1986) — the diff machinery line mapping rides.
- `patch` fuzz / context matching — the `hunk_context` fuzzy fallback.
- Phase-1 git-hosting §4.3 (the callout); Phase-2 git-hosting §4.2 (outdated state), §9 (TE-22);
  Refs contract 5.7 (`#sub` stable across edits).

[Sources: docs.gitlab.com/user/project/merge_requests/changes; docs.gitlab.com working-with-diffs]
