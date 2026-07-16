# R3 sketch spec — G-7 PR diff / files-changed (+ G-4 compare reuse)

> Phase R3 · 2026-07-16 · extends the frozen finalist-A direction
> (`06-design-sketches/6c-finalists/finalist-A-instrument/screens/2-pr-diff.html`) under the 08
> design system. Acceptance contract: `05-user-facing-surfaces/git.md` G-7 rows 1–11 (G-4 reuses
> this component). Dissolves `reviews/2026-07-06/ux-ux-git.md` findings **2** (no PR diff surface),
> **7** (raw err.message / undifferentiated no-access-vs-not-found-vs-error), **9** (broken
> check→line chain — this surface is the W4 *landing* end of it).
> Density 0.85 — earned, not a fork: shell, chip, identity, comment thread stay shared.

## Sketch files

| File | State(s) |
|---|---|
| `sketch-1-side-by-side.html` | populated split view · files-changed tree index · collapsed hunk + expand-context · line-anchored thread (German) · **rebase-orphan honest-detach** ("Outdated — was on former line 87", lifted to file level) · agent-suggested-fix (four-channel) · **W4 deep-link entry** from failing check (banner + anchored line) · restricted-count row · viewed/collapsed file · load-remaining-files · j/k + F7 demo script |
| `sketch-2-unified.html` | populated unified (= the mobile spelling; switcher hidden <720px) · **open line-comment popover** (composer) · stale/head-moved banner · erased-user + tombstoned comment · deleted-file tombstoned hunk · binary/LFS row · **"Part of this diff is restricted"** section |
| `sketch-3-skeleton.html` | loading — structure skeleton, **gutters + line-number columns first** (R-13 §A.2), `aria-busy`, one polite live region, no spinner; PR header already real (regions skeleton independently) |
| `sketch-4-terminal-states.html` | no-access (Restricted) · not-found/Absent (German copy) · error (system-blaming, scoped) · empty diff — four DISTINCT dignified cards |

All link `../tokens.css` (semantic vars only, no hex, logical properties, token-layer
`:focus-visible` ring, `100dvh`, no CDN fonts). Theme toggle in every file tests `data-theme`.

---

## 1. IA + routes (SolidStart, `frontend/apps/web/src/routes/`)

```
(app)/git/repos/[repo]/prs/[n]/index.tsx    ← existing prs/[n].tsx MOVES here (Overview tab)
(app)/git/repos/[repo]/prs/[n]/diff.tsx     ← NEW: this surface (Files changed tab)
(app)/git/repos/[repo]/compare/[range].tsx  ← G-4, later wave: same <DiffViewer>, different entry
```

- Tabs on the PR header: **Overview · Files changed (n) · Checks (x/y)** — a shared
  `PrHeader` layout segment across the three PR routes (the PR-overview assignment owns its
  final form; this sketch consumes it). Active tab = `--surface-hover` fill + brighter text (R1).
- Query params (all optional, shareable, this route owns them):
  - `?view=unified|split` — layout (default: split ≥ 960px, unified below; **mobile is
    unified-only**, the switcher is absent, not greyed).
  - `?file=<path>&line=<n>&side=old|new` — the **W4 deep-link anchor** (arriving from a failing
    check/run). On load: scroll the line to center, mark it (`--info-subtle` tint + gutter glyph +
    SR text "deep-link target from failing check …"), show the dismissible entry banner. If the
    file/line no longer exists in the current diff → the banner says so honestly ("the check ran
    against an older head") — **never a silent nearest-line guess** (same honesty rule as
    rebase-orphan).
- The commit diff route `commit/[oid].tsx` migrates onto the same `<DiffViewer>` component in the
  build wave (G-3 "commit detail = diff (reuses G-7)"); its current bespoke `FileDiff`/`DiffRow`
  are retired.
- The G-6 context pane is NOT part of this surface (it belongs to Overview; the diff earns its
  density with the full content region — git.md G-7 §2).

## 2. Full R-21 state enumeration

| State | Where | Render |
|---|---|---|
| **loading** | sketch-3 | structure skeleton: file frames + real gutter/line-number columns first, ragged code bars; `aria-busy`; one debounced polite live region ("Diff loaded, 4 files"); suppress flash <~1s; header/tabs render as soon as `PrVM` resolves (independent regions) |
| **populated** | sketch-1/2 | split + unified; per-file sticky headers; tree index |
| **empty** | sketch-4 D | base == head → "No changes to review" + explains why + paths (Overview / compare) — a real state, not an error |
| **error** | sketch-4 C | system-blaming quiet line + scoped retry; **only the diff region** — overview/checks unaffected; draft comments preserved; NEVER a raw `err.message`, NEVER the gate icon |
| **permission-denied (whole diff)** | sketch-4 A | Restricted: PR number only (it's in the URL); title/files/diffstat withheld; request-access path. Restricted-vs-Absent is **policy's choice** — Absent renders as B |
| **not-found** | sketch-4 B | absent PR → dignified not-found (German in sketch); deliberately identical for policy-Absent (anti-oracle) |
| **permission-denied (part of diff)** | sketch-2 §4 + sketch-1 tree | "Part of this diff is restricted — N changed files aren't shown." **Count only — no path, no diffstat per file** (non-leaking by construction: the edge never serialises them). Totals in the toolbar say "4 of 6 files · 1 restricted" |
| **erased / tombstoned** | sketch-2 thread | comment author erased → "[erased user]"; deleted comment → "Comment removed", thread tree intact; an erased *file body* is not a diff state (git content is the Art-17 `[OPEN — LEGAL]` residual, out of scope here) |
| **moved / outdated (rebase-orphan)** | sketch-1 | the diff **owns** this: content-anchor re-resolves (BLAKE3 + 3-way, `myelin-refs` §3.5); relocated → thread follows with a "moved" pill; content gone → **detach to "Outdated — was on former line N" pill and lift to file level. Never a silent wrong-line move.** SR announces "outdated, was on former line 87" |
| **stale / head-moved** | sketch-2 banner | head advanced mid-review → banner names both oids, "Show the updated diff" is a *user action*; drafts kept; never auto-reloads under the reviewer |
| **agent-pending** | sketch-1 | agent-suggested fix awaiting human action (four-channel treatment); "Apply suggestion…" routes through the confirm/HITL shape (plan-then-apply) — advisory scope stated inline, merge stays gated (`gate_admitted` authoritative) |
| **optimistic-rollback** | not sketched (interaction) | comment send: appears instantly + quiet "sending…"; on reject → visible rollback + system-blaming line, **typed text preserved** (comments-mentions §5 owns the render) |
| **conflict** | not sketched | concurrent edit of the same comment → comments-mentions owns (keep-yours/take-theirs); no diff-specific conflict exists (read surface) |
| **degraded** | not sketched (chip-level) | a chip inside a comment that can't refresh freezes last-known + "can't refresh" dot (reference-chip #10); the diff itself fails static per file section |
| **cross-cell** | not sketched | refs in comment bodies get the residency tag (reference-chip #8); no diff-level cross-cell state |
| **storm / no-results** | n/a / not sketched | storm is the inbox's; no-results = tree-index filter matching nothing ("No files match — clear filter"), distinct from restricted; filter box is a fast-follow (open Q7) |
| **binary / LFS** | sketch-2 §3 | "Binary file — no text diff" + pointer change + size + Download; never garbled text (dissolves the finding-10 class for diffs) |
| **file deleted** | sketch-2 §2 | tombstoned hunk: one line naming the deletion + −N + "Show deleted contents" on demand — never a dumped 214-line red wall |
| **large diff** | sketch-1 | per-hunk: unchanged runs collapsed with "⋯ 96 unchanged lines" + Expand 20 ↑ / all / 20 ↓; per-file: `truncated` cap with "Expand all"; per-diff: MR-014 cursor pages files — "2 more files weren't rendered · Load remaining files" (names + counts visible; contents lazy) |

## 3. Data contract

### EXISTING (`frontend/apps/web/src/lib/api.ts`)
- `EXISTING: PrVM.number / pr_state / base_ref / head_ref / head_oid / author` — header strip.
- `EXISTING: PrChecksVM.gate_admitted / required_contexts / green_contexts` — the "Merge blocked"
  / "1 check failing" pills (authoritative; UI never recomputes policy).
- `EXISTING: DiffFileVM.path / old_path / status` and `DiffLineVM.origin / content` — reused
  *shapes*, but both need additive extension (below). CommitDiffVM untouched.

### NEW — the backend work order

**N1 · PR diff endpoint** — `GET /v1/git/repos/{repo}/prs/{n}/diff?cursor=&view=` → `PrDiffVM`.
Diff is `merge-base(base_ref, head_oid) … head_oid` (three-dot; the reviewer reviews *the PR's*
changes, not drift in base). Cursor pages **files** (MR-014 envelope).

```ts
/** NEW: one diff line — origin plus BOTH line numbers (anchors, SR, deep-links need them). */
export interface DiffLineVM {
  origin: "+" | "-" | " ";
  content: string;
  old_no: number | null;   // NEW additive field (null on "+")
  new_no: number | null;   // NEW additive field (null on "-")
}

/** NEW: hunk boundaries — collapsed-run + expand-context need them (flat lines[] can't). */
export interface DiffHunkVM {
  header: string;                      // "@@ -104,7 +104,9 @@ impl DurableGitEdge {"
  old_start: number; old_lines: number;
  new_start: number; new_lines: number;
  lines: DiffLineVM[];
}

/** NEW: per-file entry. Restricted files are NEVER in this list (no-leak by construction). */
export interface PrDiffFileVM {
  path: string;
  old_path: string | null;             // renames
  status: "A" | "M" | "D" | "R" | "C";
  kind: "text" | "binary" | "lfs" | "submodule";
  additions: number; deletions: number;
  size_bytes: number | null;           // binary/lfs
  hunks: DiffHunkVM[];                 // empty for binary/lfs/deleted-collapsed
  deleted_body_available: boolean;     // "Show deleted contents" affordance
  truncated: boolean;                  // per-file line cap hit → "Expand all" refetches
}

export interface PrDiffVM {
  number: number;
  base_ref: string;
  base_oid: string;                    // the merge-base actually diffed
  head_oid: string;                    // what this diff snapshot renders
  files: PrDiffFileVM[];
  restricted_files: number;            // COUNT only — no paths cross the wire
  total_files: number; total_additions: number; total_deletions: number;
  page: { next_cursor: string | null; limit: number };   // "Load remaining files"
}
```

**N2 · Expand-context** — `GET /v1/git/repos/{repo}/file-lines/{oid}?path=&start=&end=` →
`{ lines: DiffLineVM[] }` (context lines at a blob oid; serves Expand ↑/↓/all and "Show deleted
contents" via the old oid). Authz: same object check as the blob route.

**N3 · PR review threads (the R3 comment store — no store exists yet).**
`GET /v1/git/repos/{repo}/prs/{n}/threads` → `{ threads: PrThreadVM[] }` ·
`POST …/threads` (new thread) · `POST …/threads/{id}/comments` (reply) ·
`POST …/threads/{id}/resolve`.

```ts
/** NEW: the content anchor — resolved server-side by myelin-refs (BLAKE3 + 3-way match). */
export interface PrAnchorVM {
  path: string;
  side: "old" | "new";
  line: number | null;                 // CURRENT resolved line; null = detached
  state: "live" | "moved" | "outdated";
  former_line: number;                 // authored-time line → "was on former line N"
  anchored_oid: string;                // head oid when authored
}
export interface PrCommentVM {
  id: string;
  author: { name: string; kind: "human" | "agent" | "service"; erased: boolean };
  body_md: string;                     // rendered via the ONE BlockEditor render path
  created_at: number;
  tombstoned: boolean;                 // "Comment removed"
  suggestion: {                        // agent (or human) suggested fix, optional
    replaces: { start: number; end: number; side: "new" };
    proposed: string;
    scope: string;                     // "review-comment · advisory" — displayed verbatim
    apply_effect_id: string | null;    // present iff viewer may apply → confirm/HITL shape
  } | null;
}
export interface PrThreadVM {
  id: string;
  anchor: PrAnchorVM | null;           // null = PR-level conversation (Overview renders those)
  resolved: boolean;
  pending: boolean;                    // my un-submitted review batch (G-8 owns submit/verdict)
  comments: PrCommentVM[];
}
```

**N4 · Viewed-file marks** — `PUT /v1/git/repos/{repo}/prs/{n}/viewed` `{ path, viewed }` +
`viewed_paths: string[]` piggybacked on N1 (per-reviewer, server-side — survives devices).
Cheap table; could slip to a fast-follow with client-local storage as the stopgap (open Q6).

**Consumed, owned elsewhere:** the failing-check → `?file=&line=` deep-link is *minted* by the
Checks surface (G-9 needs check→run→line refs — a censused gap); this route only honours the
params. PR `title` for the header strip is the PR-overview assignment's NEW field (`PrVM.title`);
sketches assume it exists.

## 4. Keyboard map + SR behaviour

**Landmarks:** `banner` (topbar) · `nav` "Subsystems" (rail) · `nav` "Pull request sections"
(tabs) · `toolbar` "Diff controls" · `nav` "Files changed index" (tree) · the diff column is the
`main` content; each file is a `section` with `aria-label` "Diff for {path}".

| Key | Action |
|---|---|
| `j` / `k` | next / previous line (roving focus over code rows; the scroller follows) |
| `F7` / `Shift-F7` | next / previous **change** (first row of each add/del run; scrolls to center) — the G-7 DoD binding; `]` / `[` are synonyms (finalist continuity) |
| `n` / `p` | next / previous **file** (focus its header) |
| `c` | comment on the focused line → opens the composer popover, focus moves into the textarea; `Esc` closes and **returns focus to the line** (no trap); `⌘⏎` submits |
| `e` | expand/collapse the focused collapsed-run (nearest expander); on a file header: fold/unfold the file |
| `v` | toggle Viewed on the file containing focus (announces "list_filter.rs marked viewed, collapsed") |
| `Enter` on a collapsed-run expander | expand all hidden lines |
| `?` | keyboard cheat-sheet dialog (shared shell primitive) |
| `Tab` | never enters the line grid row-by-row — the grid is one tab stop (roving `tabindex`); inline widgets (threads, suggestions, expanders) are normal tab order after their row |

**SR contract (R-17 §5.1, the hard component):**
- Change kind is **text, never colour**: each row's code cell carries a visually-hidden prefix —
  "added, new line 210: …" / "removed, old line 105: …" / "unchanged, line 104" — and the `+`/`−`
  sign is a visible TEXT glyph (aria-hidden, since the prefix already says it).
- **Line numbers are announced** (prefix includes them; gutter cells are plain content, not
  aria-hidden).
- **SR-linear mode** (toolbar toggle, persisted): re-renders the current file unified,
  single-column, wrap on, one `<article>` per hunk with the hunk header as its accessible name —
  a linear read-through for continuous SR reading, same data, no separate route.
- Rebase-orphan announces its pill text: "Outdated — was on former line 87".
- Deep-link target row appends "deep-link target from failing check authz-integration" to its
  prefix; the arrival banner is a `role="note"` before the first file.
- One debounced polite live region for: diff loaded, N lines expanded, file marked viewed,
  comment posted / rollback. Skeletons set `aria-busy` on the busy region only.
- Comment marker `●` in the gutter is aria-hidden; the row prefix says "has 1 comment thread".
- 200% / 320px reflow: tree stacks above the diff (<960px); tables scroll inside `.tblwrap`
  (`overflow-x:auto`) — the page never scrolls horizontally; "Wrap lines" is the dense-code
  relief valve. RTL: all logical properties; code + oids + refs stay LTR via `<bdi>`/`pre`
  (bidi-isolated); prose (comments) mirrors.

## 5. Component reuse + new primitives

**Reused (02-components):**
- `comments-mentions.md` — `<Thread>`/`<Comment>` in the `review` variant: inline threads,
  composer, erased/tombstoned renders, optimistic send, resolve. Bodies via the BlockEditor
  render path; mentions/refs as `<ReferenceChip>`.
- `reference-chip-and-unfurl.md` — chips in comment bodies (`ISS-482`, run refs), the W4 banner's
  check chip, anchor-state pills (moved/outdated vocabulary comes from its state table #9).
- `overlays.md` — Popover (comment composer; portal, `--z-popover`, focus-trap + return-focus,
  Esc), Menu (file-header kebab: collapse all · copy path · view file @head · open blame),
  Tooltip (kbd hints), ConfirmDialog (Apply-suggestion effect confirm).
- `identity-and-agent-badge.md` — comment authors; the agent four-channel treatment on FixAgent.
- `agent-hitl-card.md` — the shape "Apply suggestion…" routes into (proposed effect + scope).
- `forms-and-controls.md` — Button, Checkbox (Viewed), the composer textarea/validation.
- Shell/nav + tabs from `shell-and-nav.md` (R1 active treatment).

**NEW shared primitives (named, justified):**
- **`<DiffViewer>`** — the R-17 §5.1 hard component, contributed down to the shared library
  (manual §4 seam: "the diff / files-changed viewer … is owned by the rich-components track").
  Sub-parts: `DiffFileSection` (sticky header, fold, viewed), `DiffHunk`, `DiffLineRow` (roving
  focus, SR prefix), `ExpandContextControl`, `DiffToolbar`. Consumers: PR diff (G-7), compare
  (G-4), commit detail (G-3) — three surfaces, zero forks.
- **`<FileTreeIndex>`** — files-changed tree (status glyph+letter, counts, comment badge, viewed,
  restricted-count row). Later shared with G-2 repo tree.
- **`Skeleton`**, **`StatusPill`** — as REFINEMENTS/brief anticipate (pill = glyph+label, used
  for Open / failing / Merge blocked / Outdated).
- **Icons:** all from the 42 registry (`pull-request, gate, check-fail, check-pending, merge,
  commit, file, folder, message, chevron, kebab, link, external-link, agent, human, rerun`).
  One genuine gap: an **unfold/expand-context glyph** — proposed registry name `expand-lines`
  (USAGE-MAP §C backlog); sketches use the `chevron` pair meanwhile, which is acceptable.

## 6. Open questions (orchestrator gate)

1. **Intra-line (word-level) change emphasis** — no semantic token exists for "emphasised region
   inside an added/removed line" (`--success-subtle` is already the row fill). Ship R3 without
   word-level emphasis (sketches do), or add a `--diff-emph-add/-del` semantic pair to
   tokens.json? Recommend deferring; the +/− text channel is intact either way.
2. **Syntax highlighting** — the generated token set has no `--tok-*` ramp (the finalist sketch
   ad-hoc'd them). R3 ships monochrome mono (as sketched)? If syntax colour is wanted it is a
   token-tier addition + the WASM diff-render path, not a per-surface hack.
3. **Comment-store scope** — is N3 the *git-local* thread store, or the first face of the one
   conversation primitive's store (issues/docs/chat next)? The VM is designed to survive either,
   but the endpoint namespace (`/git/repos/…/prs/…/threads` vs a `/threads?subject=` graph) is a
   backend architecture call.
4. **Merge-base cost** — N1 assumes the edge can compute `merge-base(base, head)` cheaply on
   durable repos (libgit2/gix). If not, first ship two-dot (`base_oid = base_ref@load`) and say
   so in the UI ("compared against main @ 2f4c1e9")? Honesty either way; needs a backend answer.
5. **Review batching (G-8 seam)** — the composer offers "Start review" (batch) vs "Add single
   comment". Verdict submit + batch management live on the Review surface. Does R3 ship
   single-comment-only first (composer loses one button), or batch-without-verdict? `pending` on
   `PrThreadVM` supports both.
6. **Viewed-marks persistence** — server-side per-reviewer (N4) vs client-local for R3?
   Server-side is small but is a new write path + store.
7. **Tree-index filter box** — sketched without one (4-file dogfood diffs don't need it; 200-file
   diffs do). Fast-follow or in-scope? (Adds the no-results state, already enumerated.)
8. **Restricted representation is count-only** — deliberately no tree position, no per-file
   diffstat (both leak). Confirm policy is happy that totals ("6 files") minus visible ("4")
   plus the count row is the *intended* disclosure level for "Restricted"; for "Absent"-classed
   files the totals would exclude them entirely (policy input, not frontend guess).
9. **Deep-link freshness** — when `?line=` was minted against an older head, the sketch's answer
   is an honest banner ("the check ran against an older head") with no auto-guess. Alternative:
   re-anchor via the same 3-way content match the comments use. Nicer, but needs N2-style
   server help — decide with the G-9 assignment.
