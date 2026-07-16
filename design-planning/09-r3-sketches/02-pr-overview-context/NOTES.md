# R3 sketch spec — 02 · PR overview + context pane (G-6) · review verdicts (G-8) · checks panel (G-9)

> Date 2026-07-16 · per `../_brief.md`. Direction FROZEN: 08 design system + finalist-A screen
> `1-shell-pr-context.html` — this pack extends it onto the generated tokens and the real SolidStart app.
> Files here: `sketch-1-populated.html` · `sketch-2-skeleton-assembling.html` ·
> `sketch-3-restricted-and-edge.html` · `sketch-4-review-in-progress.html` ·
> `sketch-5-merge-and-terminal.html` (each links `../tokens.css`, all three themes, RTL-safe logical
> properties, one German string per file).

---

## 1. IA + routes (SolidStart, under `frontend/apps/web/src/routes/`)

| Route | Surface | Status |
|---|---|---|
| `(app)/git/repos/[repo]/prs/index.tsx` | PR list (review finding 1's front door — owned by the PR-list assignment, named here because the sidebar reuses its rows) | NEW route |
| `(app)/git/repos/[repo]/prs/[n].tsx` | **G-6 Overview** (this spec) — description · checks · reviews · commits · discussion · merge card; context pane populated | EXISTS, rebuilt |
| `(app)/git/repos/[repo]/prs/[n]/commits.tsx` | full commits-in-PR list (same rows as the overview section) | NEW route |
| `(app)/git/repos/[repo]/prs/[n]/diff.tsx` | G-7 diff (separate assignment; tab target only here) | NEW route (other pack) |
| `(app)/git/repos/[repo]/prs/[n]/checks.tsx` | **G-9 full checks panel** (same component as the overview's embedded panel, full-height) | NEW route |
| `…/prs/[n]#discussion` | Discussion tab = anchor into the overview thread (NOT a separate route — one `<Thread>`, no fork; the tab scrolls/focuses the section) | anchor |

Deep links: every check row → `/ci/runs/[run]#step-<i>` (W4 warm chain); every commit row →
`…/commit/[oid]`; pane cards → their artifact routes (or open **in** the pane later — see open Q7).

### 1b. The shell grid change (the pane is SHELL-OWNED — binding for the builder)

`src/components/AppShell.tsx` grows a fourth region. Today: `grid-template-columns: auto 1fr`
(rail | secondaryNav+main). Target:

```tsx
export interface AppShellProps {
  viewer: Viewer;
  secondaryNav?: JSX.Element;   // existing — contextual sidebar slot
  contextPane?: JSX.Element;    // NEW — the shell-owned 4th region (G-6 supplies it)
  children?: JSX.Element;
}
```

- Grid: `grid-template-rows: auto minmax(0,1fr)`; body columns
  `auto 224px minmax(0,1fr) 332px` **when `contextPane` is present**, `auto 224px minmax(0,1fr)`
  when absent (the column *drops*; content never renders beside an empty gutter).
- The pane is `<aside aria-label="…">`, `overflow:auto; min-height:0; overscroll-behavior:contain`
  — it owns its scroller like every region (shell-and-nav §2). `height:100dvh` on the shell root
  (current AppShell uses `100vh` — fix while touching it).
- **MOB-2**: ≤1280px the pane column drops and the pane becomes a **toggled end-side drawer** on the
  overlay substrate (portal, scrim, focus-trap, `Esc`, route-change auto-close); the trigger is a
  "Context" button the shell renders in the surface header area (sketched in sketch-1, functional).
  ≤900px the contextual sidebar also folds (existing shell behaviour, out of scope here).
- No per-screen pane hack: the PR screen passes `contextPane={<PrContextPane …/>}` — the *frame,
  breakpoints, drawer, and landmark* live in AppShell once, for every future surface (issue pane,
  doc backlinks pane).

---

## 2. Full R-21 state enumeration (G-6 §2b set + G-8 + G-9)

| # | State | Where sketched | Notes |
|---|---|---|---|
| 1 | Populated (gate blocked) | sketch-1 | the realistic default; blocked reasons each link to their fix |
| 2 | Populated (gate admitted) + merge confirm | sketch-5 | one primary CTA; ConfirmDialog, safe-action focus |
| 3 | Loading / **W1 assembling** | sketch-2 | scaffold instant; labelled slots fill independently; per-slot `aria-busy`; ONE debounced polite live region; type-true icons pre-resolution |
| 4 | Empty discussion | sketch-3 | composed with read-only (composer ABSENT, not greyed) |
| 5 | Permission — linked ref no-access | sketch-3 pane | `Restricted` card, **no title** (resolver-collapsed; non-leak by construction), request-access path |
| 6 | Permission — whole PR no-access | not sketched | the route-level `NotAvailable` card (exists); indistinguishable from not-found (anti-oracle). No layout novelty — copy per R-21 |
| 7 | Cross-cell linked issue | sketch-3 pane | normal card + mono residency tag + T3 provenance footnote |
| 8 | Moved chip | sketch-3 appendix | label pill "moved"; card banner "Relocated — showing the current version" |
| 9 | Outdated chip | sketch-3 description | label pill; opens surviving content; never silent re-anchor |
| 10 | Erased/tombstoned — run crypto-shredded | sketch-3 pane | dignified dated tombstone; verdict record retained note |
| 11 | Erased — comment/author | sketch-3 appendix | "[erased user]", thread integrity preserved |
| 12 | Error — checks projection fails | sketch-3 | **LOCAL boundary** (review finding 5): "Checks unavailable", scoped retry, PR stays live; merge card degrades to "Gate state unavailable" — never fabricates gate state |
| 13 | Error — pane slot fails | not sketched | same failcard pattern per-slot (fails static, frozen last-known if cached); identical anatomy to #12 |
| 14 | Stale / reconnecting checks | sketch-3 appendix | "Reconnecting… last updated 12s ago" + last-known rows stay; auto-resume on `ci.*` |
| 15 | Agent-pending review | sketch-4 + sketch-3 appendix | "Reviewing… advisory — never counts toward the gate"; four-channel treatment |
| 16 | Review-in-progress (batched) | sketch-4 | pending = dashed border + "Pending · only you" pill; verdict radiogroup; ONE event on submit |
| 17 | Merged terminal | sketch-5 appendix | merged pill (`merge` glyph + label); merged-by record; pane stays live; delete-branch secondary |
| 18 | Closed terminal | sketch-5 appendix | closed pill + required reason; reopen quiet secondary; checks read-only last projection |
| 19 | Optimistic send / rollback (comment) | not sketched | comments spec §5 owns it (settle `--dur-micro`, honest rollback, text never lost) — no layout change |
| 20 | Conflict (comment edit) | not sketched | comments spec owns (CRDT/keep-yours); PR overview adds nothing |
| 21 | Draft PR | not sketched | state pill variant only (`edit` glyph + "Draft", `--text-muted`); merge card renders "Draft — not mergeable" with a "Mark ready" primary. No other layout change |
| 22 | Fork-trust (un-endorsed fork run) | not sketched | KEEP the existing `fork-trust` note row + `gate` glyph from the current page verbatim inside the new checks panel — the R2 semantics (a fork's green never reads as gating-green) must survive the reskin |

---

## 3. Data contract — every rendered field tagged

### EXISTING (from `frontend/apps/web/src/lib/api.ts`)

- `PrVM`: `number` · `pr_state` · `base_ref` · `head_ref` · `head_oid` · `author` · `reviews`(count) · `durable`
- `PrChecksVM`: `required_contexts[]` · `required_approvals` · `green_contexts[]` · `endorsed_contexts[]`
  · `fork_unendorsed_contexts[]` · **`gate_admitted` (AUTHORITATIVE — the merge card renders it verbatim;
  the UI never recomputes policy; blocked "why" strings are display-only)** · `durable`
- `CommitRowVM` (reused for commits-in-PR rows): `oid` · `short_oid` · `summary` · `author` · `committed_at` · `parents`

### NEW (the backend work order — proposed shapes, edge ViewModels)

**N1 — PR record extension** (extend `PrVM`; same endpoint `GET /v1/git/repos/{repo}/prs/{n}`):

```ts
interface PrVM {
  // …existing…
  title: string;                    // NEW — header h1
  body_md: string | null;          // NEW — description; rendered via the ONE BlockEditor read path
  created_at: number;               // NEW — "opened 14.07.2026" (Intl-formatted client-side)
  commits_count: number;            // NEW — tab badge without fetching the list
  visibility_label: string | null; // NEW — humanised visibility chip ("Internal · platform"); see open Q4
}
```

**N2 — commits-in-PR** `GET …/prs/{n}/commits` → `{ items: CommitRowVM[], page }` (MR-014 envelope;
reuses `CommitRowVM` — no new row shape).

**N3 — checks→run refs** (extend `PrChecksVM` additively; parallel string arrays stay for the gate,
rows carry the W4 chain):

```ts
interface PrCheckRowVM {
  context: string;                  // "test · integration · edge-authz" (humanised at the backend)
  required: boolean;
  status: "pass" | "fail" | "pending" | "tombstoned";  // glyph+label, never colour alone
  run_number: number | null;       // "run #4117"
  run_href: string | null;         // check → run (W4); null = viewer can't read the run → row renders
                                    //   status but NO link (no leaked run name — G-9 §6)
  duration_s: number | null;
  failing_step: { index: number; total: number; name: string; line: number | null } | null; // fail only
  tombstone_note: string | null;   // "erased under retention on 12.06.2026" (humanised, dated)
}
interface PrChecksVM { /* …existing… */ check_rows: PrCheckRowVM[]; as_of: number; }
// as_of drives "projection as of 12:04:31" and the stale state's "last updated Ns ago"
```

**N4 — linked refs (the pane's food)** `GET …/prs/{n}/refs` — the edge surface over the existing
refs graph; items are **resolver projections, per viewer** (the UI renders exactly the state returned,
invents none — reference-chip spec §5):

```ts
interface LinkedRefVM {
  ref: string;                      // opaque ArtifactRef URN; type parsed for the pre-resolution icon
  type: "issue" | "run" | "doc" | "message" | "commit";
  slot: "issue" | "ci" | "doc" | "other";      // which labelled pane slot it fills
  state: "live" | "no_access" | "moved" | "outdated" | "tombstoned" | "cross_cell" | "degraded";
  title: string | null;            // ABSENT (null) for no_access/tombstoned — nothing to leak
  status_label: string | null;     // "failed" / "In progress" — glyph+label pairs
  fields: { label: string; value: string }[];  // humanised body lines (SLA, priority, step) — backend-
                                    //   humanised + viewer-locale (never a frontend id→name map)
  region: string | null;           // cross_cell: "eu-central-1 · Frankfurt"
  href: string | null;
  tombstone_note: string | null;
}
```

**N5 — comments store (NO store exists — R3 scopes it; this is the design)**
`GET …/prs/{n}/comments` → `{ items: PrCommentVM[], page }`;
`POST …/comments` (single) · `POST …/reviews` (start batch) · `POST …/reviews/{id}/comments` (pending)
· `POST …/reviews/{id}/submit { verdict, summary_md }` · `DELETE …/reviews/{id}` (discard).

```ts
interface PrincipalVM {            // shared atom — the identity/agent badge renders exactly this
  kind: "human" | "agent" | "service";
  display: string;                  // humanised; "[erased user]" / "Restricted" arrive pre-collapsed
  on_behalf_of: string | null;     // agents only — attribution channel
  trigger: string | null;          // agents only — "pr.updated"
}
interface PrCommentVM {
  id: string;
  author: PrincipalVM;
  body_md: string;                  // ONE render path (BlockEditor); mentions/refs are structured nodes
  created_at: number; edited_at: number | null;
  state: "visible" | "removed";   // removed → "Comment removed", tree preserved
  anchor: { path: string; line: number | null;
            anchor_state: "live" | "moved" | "outdated" } | null;   // diff-line anchored comments
  review_id: string | null;        // batch membership
  pending: boolean;                 // true ONLY in the author's own view of an unsubmitted batch
}
interface PrReviewVM {
  id: string; reviewer: PrincipalVM;
  verdict: "approved" | "changes_requested" | "commented" | "in_progress" | "pending";
  advisory: boolean;                // agent reviews: true — NEVER counts toward required_approvals
  submitted_at: number | null; summary_md: string | null; comment_count: number;
}
```

Submit emits **one** notification event carrying the batch (R-BATCH-1) — server-side contract, not UI.

**N6 — merge action** `POST …/prs/{n}/merge` → `200 { merged_oid, merged_at, merged_by }` |
`409 { checks: PrChecksVM }` (gate flipped mid-dialog → re-render the blocked card; never merge on
stale state) | `403` → route-level not-available. Merged/closed terminal fields on `PrVM`:
`merged_by: string|null`, `merged_at: number|null`, `merge_oid: string|null`,
`closed_by: string|null`, `closed_reason: string|null`.

**N7 — live updates**: the page subscribes the existing SSE to `ci.*` + `pr.*` for this PR; a flip
patches `PrChecksVM`/`PrReviewVM` in place (`motion.liveUpdate`, no scroll-jump); drop → state #14.

---

## 4. Keyboard map + SR behaviour

### Landmarks (per shell-and-nav §5 + the pane addition)

`banner` (topbar) → `nav[aria-label="Surfaces"]` → `aside[aria-label="Pull requests in {repo}"]`
→ `main#main[aria-labelledby=pr-h1]` → **`aside[aria-labelledby=ctx-h]` — the pane is a
`complementary` landmark named "Pull request context"**; inside it each slot is a
`<section aria-labelledby>` with a visible h3 label (Linked issue / CI run / Linked doc / Agent) so an
SR user can jump slot-to-slot by heading. Skip-link first-focusable → `#main`. One `lang` on `<html>`.

### Keys (single-key, palette-consistent; documented in the `?` cheat-sheet)

| Key | Action |
|---|---|
| `⌘K` | palette (shell) |
| `g d` / `g c` / `g k` / `g o` | go diff / commits / checks / overview (tab nav) |
| `x` | toggle the context pane (the drawer on ≤1280px; focus moves into the pane, `Esc` returns) |
| `.` | focus the comment composer |
| `r` | reply to the focused comment; `e` edit own |
| `v` | open the verdict panel while a review is in progress |
| `m` | open the merge ConfirmDialog — only bound when `gate_admitted` (otherwise inert, no tease) |
| `F7`/`Shift-F7` | next/prev failing check row (mirrors the diff's change-nav convention) |
| `Esc` | close popover/drawer/dialog, return focus to trigger — never a trap |

Verdict panel: `role=dialog` non-modal popover; radiogroup with arrow-key movement + roving tabindex;
`Enter` on Submit; closing keeps the batch (durable draft). Merge dialog: `role=alertdialog`,
focus-trapped, **default focus on Cancel** (safe action), `Esc` cancels.

### SR / live announcements

- Every skeleton slot: `aria-busy="true"` + structure-matching ghosts; the shell's **one debounced
  polite `role=status` region** announces per settle-burst ("Checks loaded. Remaining context is
  loading." → "Pull request context loaded.") — never one announcement per slot, never per background
  tick.
- Check rows: status is text ("passed/failed/running") inside the row's accessible name, with the
  failing row naming run + step + line ("failed — open run 4117 at step 5, cross_tenant_read_denied,
  line 214"). A watched flip (red→green) announces once, politely.
- Sidebar status glyphs carry `sr-only` labels ("checks passed") — position+glyph+label, never colour.
- Agent rows: the accessible name includes the literal word "Agent" + the attribution string.
- Pending review comments: accessible name includes "Pending review comment, visible only to you".
- Stale: the reconnect bar is `role=status` (announced once); the "as of" clock does NOT re-announce.

---

## 5. Component reuse + new primitives

**Reused (08 `02-components/` + existing DS `frontend/packages/design-system`):**

- `shell-and-nav` — the whole frame; this pack ADDS the `contextPane` slot (§1b) to the existing
  `AppShell.tsx`, not a new shell.
- `reference-chip-and-unfurl` — every pane card IS `<ReferenceUnfurl>`; inline description/comment
  refs are `<ReferenceChip>`; moved/outdated/no-access/tombstoned/cross-cell renders come from its §5
  table verbatim. The pane invents no new ref rendering.
- `comments-mentions` — `<Thread surface="review">` for the discussion incl. batching, pending pills,
  anchored-comment chips, tombstones; composer = the BlockEditor comment tuning.
- `identity-and-agent-badge` — every author/reviewer row; the four-channel agent treatment is
  inherited, never redefined here.
- `block-editor` read path — PR `body_md` + comment bodies (one render path; no PR-local markdown).
- Overlays (existing `Dialog`/`Menu`/`Toast` in the DS) — merge **ConfirmDialog** (alertdialog +
  safe-action focus is a needed variant of the existing Dialog), verdict **Popover**, pane **drawer**
  (portal + scrim + trap), comment-action menus, undo toast for optimistic sends.
- Icons: all from the 42 registry — `pull-request, merge, gate, check-pass/fail/pending, commit, run,
  issue, doc, file, link, edit, message, approve, reject, kebab, rerun, close, human, agent, inbox,
  search, settings, nav-*`. **No new icon needed by this surface.** (Sketch SVGs are placeholders
  tagged `data-icon` with registry names.)

**NEW shared primitives (names for the DS backlog — REFINEMENTS checked, none exist yet):**

1. `Skeleton` — bar/row/card ghosts + the `aria-busy` + debounced-live-region wiring (expected by the
   brief; every R3 surface needs it).
2. `StatusPill` — glyph+label pill (PR state, check counts); one component so state pills render
   identically in header, sidebar, chips.
3. `Chip` — the `<ReferenceChip>` DS implementation (spec exists, component doesn't).
4. `PaneSection` — the labelled context-pane slot: h3 label + per-slot skeleton + per-slot fail-static
   error boundary. Justified: the W1 "assembles itself" contract (label visible before content, slot
   fails alone) must be mechanical, not re-implemented per surface; issues/docs panes will reuse it.
5. `LocalBoundary` — the scoped quiet-failure card ("Checks unavailable" + retry). Justified: finding 5
   is a *class* of bug (one shared boundary component kills it everywhere), and the copy pattern
   (system-blaming one-liner + scoped retry) should be uniform.

**Explicitly NOT new:** the merge readiness card, review bar, verdict panel — surface organisms built
from Button/StatusPill/overlays, live in the route, not the DS.

---

## 6. Motion (token-bound; reduced-motion = instant flip, information preserved)

- **W1 assemble:** slots fill in place, `--dur-fast` + `--ease-enter`, no layout shift (skeleton
  reserves final geometry). The pane never "animates in" — it renders.
- **Live check flip** red→green in place: `--dur-deliberate` (240ms) cross-fade, no scroll-jump (B5).
- Verdict submit / merge settle: `--dur-micro` settle; rollback reverses (failure ≠ success motion).
- Drawer: `--dur-base` + `--ease-enter`; reduced-motion instant (tokens.css zeroes durations).

---

## 7. Open questions (for the orchestrator gate)

1. **Comments store scope:** N5 is sketched as a PR-scoped edge API, but ADR-05/§5.5 want ONE
   conversation primitive across PR/issue/doc/chat. Does R3 build the store PR-shaped-but-generic
   (`subject_ref` instead of `{repo,n}`) so issues/docs mount the same store later? I propose yes —
   the VM shapes above don't change, only the keying. Needs a backend owner decision.
2. **Discussion tab vs inline:** the frozen finalist has a Discussion tab; the G-6 switch test wants
   discussion visible without a tab dance. Sketched compromise: thread inline on Overview, tab = anchor
   (`#discussion`). Confirm this reading of "extend, don't re-diverge".
3. **`gate_admitted` vs review verdicts:** does the server's gate already ingest `changes_requested`
   as blocking (sketch-1 assumes yes — it lists the missing approval, and G-8 says merge is gated)?
   If review-blocking is NOT in the R2 gate, the blocked-reasons list must not imply it. Backend truth
   needed before copy freezes.
4. **`visibility_label` (sovereignty chip on the header):** no existing VM carries visibility; is the
   humanised label cheap to project from the R2 authz model, or does the chip wait? Sketches show it;
   it can drop without layout change. (Floor, honestly named.)
5. **Diff stat + Diff tab badge (`+412 −286`)** depend on the head-vs-base diff endpoint owned by the
   G-7 pack. Until it lands the tab renders without the badge (sketch-2 shows the skeleton spelling).
6. **Merge methods:** sketch-5 assumes merge-commit only (matches the current backend). Squash/rebase
   options are a later ruleset concern — the ConfirmDialog summary line is where they'd surface.
7. **Chips opening IN the pane:** R-06 §3.4 hints the pane could host opened refs (click issue chip →
   issue unfurl in pane instead of navigation). Deferred: sketched behaviour is navigate-on-click,
   peek-on-hover/focus. Wedge-stronger but needs a pane-stack interaction model — propose R3.5 spike.
8. **Agent slot for out-of-scope viewers** (sketch-3): rendered ABSENT. Is absence right, or should a
   permitted-but-empty tenant see "no agent activity"? Current call: absent for no-permission, empty
   state only when permitted-and-none — mirrors the composer rule.
9. **`x` / `F7` key choices** are proposals consistent with the diff pack's conventions — the two packs
   must land one shared cheat-sheet (coordinate at the gate).
