# Design — Wireframes (Issue Tracker primary screens)

> Phase 4 design sketch (required before architecture). ASCII wireframes of the PRIMARY screens from the
> design-language §7.3 catalogue, each showing **empty / loading / error** (and where relevant
> permission-denied / erased / agent-pending) states — not just the happy path (VISION §3 / §5.10). Applies the
> day-one UX primitives (§8b): overlays **portal to root**; **one editor render path** (read+edit share the
> `myelin-content` parser); **measured tokens** (status by glyph+label+position, never colour alone — §8b.3);
> **layout containment** (shell pinned `100vh`, each region its own scroller with `min-height:0`); **humanised
> strings** at the backend (NOTIF-1 — no raw `issue_state_changed`, no raw ids). Wireframes are structural, not
> pixel-final; concrete token values are the design-system deliverable.

Legend: `▓` skeleton block · `●/◐/○` status glyph (always paired with a label) · `[btn]` · `⟦empty⟧` etc.

---

## S1 — Issue detail

### Happy path
```
┌ ENG-1421  Login 500 on SSO ─────────────────────────────────┬ CONTEXT PANE ───────────┐
│ ●Started · In Progress ▾   ⬆High ▾   @alice ▾   ⌗ENG cycle 14 │ Linked PRs              │
│ Type: Bug ▾   Labels: sso, regression  +                      │  ◐ PR #88 checks running│
│─────────────────────────────────────────────────────────────│  ● PR #90 merged        │
│ [ Description — shared editor §5.9, ONE render path ]         │ Linked docs             │
│  Repro on staging. See `auth/session.rs` …  @bob #ENG-1390    │  ▸ SSO runbook          │
│                                                               │ CI runs                 │
│ ▾ Sub-issues  (3/5 done) ▓▓▓▓▓░░░                             │  ✖ run 4412 failed      │
│   ☑ ENG-1422  ☑ ENG-1423  ☐ ENG-1424 …                        │ Backlinks (referenced)  │
│ ▾ Relations   ⛔ blocked_by ENG-1490 (open)                   │  💬 #incidents thread    │
│   [ Remind me when unblocked ]   ← stateful Trigger (A5)      │  (all per-viewer filtered│
│─────────────────────────────────────────────────────────────│   — confidential never  │
│ ⏱ SLA: first-response 1h12m left  ◐at-risk(80%)               │   leaks; tombstone on   │
│─────────────────────────────────────────────────────────────│   erasure)              │
│ ▾ Activity / comments  (read+edit same parser)               │                         │
│  ⬡ alice moved In Progress · 2h    [agent]Triage set severity│                         │
│  [ comment composer … @ # / ]                                 │                         │
└───────────────────────────────────────────────────────────────┴─────────────────────────┘
```

### Key non-happy states
- **Loading:** properties sidebar + body render as **skeletons matching the final layout** (the title row, the
  property chips, the sub-issue bar) — never a spinner on blank (§8b.6). Header chips show `▓`.
- **Error:** "Couldn't load this issue. [Retry]" — one quiet line, blames the system, a path (§8b.6). Already-
  loaded panes that fail (e.g. CI status) **fail static**: "CI status temporarily unavailable" for *that* pane
  only, the rest of the issue stays usable.
- **Permission-denied (confidential):** the whole screen is the graceful no-access card — "You don't have access
  to this issue" — never a leaked title (§5.3/ADR-03). Reached only if the URL was guessed; normal lists never
  surface it (`list_objects` pre-filter).
- **Transition-blocked:** the state ▾ shows the target greyed with a tooltip "Blocked: CI red on PR #88 /
  blocked_by ENG-1490" (glyph ⛔ + label, §8b.3) — the guard reason, pre-assembled.
- **Agent-pending (HITL):** an inline **agent card** atop activity: "[agent] Triage proposes: transition → Done.
  Risk: governed workflow. Est. cost: 0.4¢  [Approve] [Edit] [Reject]" — the `agent` treatment (§6.1), no
  sparkle iconography (§8b.3).
- **SLA breaching/breached:** ⏱ turns to "● Breached 12m ago" (glyph+label+position, not red-only).
- **Soft-deleted/restorable:** a banner "This issue was deleted · [Restore] (undo window)" (reversibility, §8b.6).
- **Erased:** a tombstone — "This content was erased" — structure preserved, identity gone (ADR-12).

---

## S3 — Board / Kanban (the shared views component §5.6, board projection)

### Happy path
```
┌ Active cycle · Board ──────────  group: Status ▾   filter ⌗   ⊕ density ──────────────────┐
│ Unstarted (4)      │ Started (3)        │ In Review (2)     │ Done (6)                     │
│ ┌ENG-1430 ●Todo ┐  │ ┌ENG-1421 ●Started┐│ ┌ENG-1418 ◐Review┐│ ┌ENG-1402 ●Done ┐            │
│ │Cache miss 500 │  │ │Login 500 SSO  │  ││ │API ratelimit  ││ │…              │            │
│ │@bob  ⬆High    │  │ │@alice ⬆High   │  ││ │@cara          ││ │               │            │
│ └───────────────┘  │ │[agent]moved → │  ││ └───────────────┘│ └───────────────┘            │
│ ┌ENG-1431 …     ┐  │ └───────────────┘  │ (WIP 2/2 ●limit)  │                              │
│  drag ⇕ rank (CAS-arbitrated; agent & human one path)        │                              │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

### Non-happy states
- **Empty (no issues / fresh team):** onboarding-forward — "No issues in this cycle yet. [Create issue]  or
  [Plan from backlog]" (§5.10 empty explains+offers).
- **Loading:** each column renders 2–3 **card skeletons** at the right size (structure, not spinner).
- **Error:** "Couldn't load the board. [Retry]" + fail-static per column if only one group fails.
- **Permission-filtered:** some issues simply aren't present (pre-filtered via `list_objects`); **no "N hidden"
  leak** — the absence is silent and correct (deep-dive §8.4).
- **Live multi-user drag (presence):** a faint avatar on a card another user is dragging (firehose presence,
  §5.11); **agent-moved card** animates in with the `agent` badge (§6.1).
- **WIP-limit-exceeded:** the column header shows "●limit 2/2" (glyph+count, not colour-only); a drop over limit
  warns inline.
- **Drag-reorder conflict:** the optimistic move that loses CAS **snaps back** to the authoritative position with
  a one-line "reordered by someone else — your change was re-applied below" (honest rollback, §8b.6 / sketch 06).
- **Touch:** card row-actions (assign/transition) are behind a `⋯` affordance, not hover-only (§8b.4).

---

## S5 — Timeline / Roadmap (co-equal PM lens over the SAME issue rows; sketch 01)

### Happy path
```
┌ Roadmap · ENG ─────────────  Q2 ──────── Q3 ──────── Q4 ──────────────────────────────────┐
│ ▾ Initiative: Sovereign auth   ▓▓▓▓▓▓░░░░ 62%                                              │
│    ▾ Epic: SSO hardening       ▓▓▓▓▓▓▓▓░░ 80%   ●on-track                                  │
│        bars span dates · ⤳ depends_on overlay → Epic: Session store ◐ date-at-risk         │
│    ▾ Epic: MFA rollout         ▓▓░░░░░░░░ 18%   ◐ date-at-risk (forecast)                  │
│         └ contributing: ⛔ ENG-1490 blocked, ENG-1502 unestimated  ← context pre-assembled │
│ ▾ Initiative: Billing v2 …                                                                 │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```
Rollup % bars come from the incremental rollup (sketch 05); date-at-risk from the forecast agent (sketch 05).
Editing dates/scope here patches the engineer board live (one `issue` table — no parallel reality).

### Non-happy states
- **Empty:** "No initiatives or epics yet. [Create initiative]" + a hint that the roadmap is the same data as the
  board, one lens.
- **Loading:** skeleton bars at row positions (structure).
- **Error:** "Couldn't load the roadmap. [Retry]"; rollup-compute lag shows "● progress updating…" on the bar
  rather than a wrong number.
- **Rollup progress / dependency-blocked / date-at-risk** are first-class states (glyph+label).
- **Cross-team initiative:** an initiative whose child epics live in several teams renders with team chips per
  bar; permission-filtered (a viewer sees only the children they may read).

---

## S9 — Triage inbox (agent-assisted; design-language §6)

```
┌ Triage · ENG  (3) ───────────────────────────────────────────────────────────────────────┐
│ ☐ ENG-1455  Payment webhook 502         [agent] suggests: ⬆High · label:billing · team:PAY │
│             "suspected dup of ENG-1390"   [Accept] [Edit] [Dismiss]                         │
│ ☐ ENG-1456  Typo in onboarding copy      [agent] suggests: ⬇Low · label:docs               │
│ ▾ Duplicate-suspected cluster (2)         ENG-1457 ≈ ENG-1390   [Merge] [Not a dup]         │
│ [ Bulk: select all → apply ]                                                               │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```
- **Empty:** "Triage is clear ✦" — a calm, designed empty (not an error).
- **Loading:** row skeletons + a placeholder agent strip.
- **Error:** "Couldn't load triage. [Retry]."
- **Agent strip** uses the `agent` treatment, glyph+label, **no magic-wand icon** (§8b.3); suggestions are
  **proposed, never auto-applied** (§6.2). Mock-runtime today, real later — same UI.

---

## S10 — My Work hub (a scoped VIEW of the ONE Notif inbox — C-9, NOT a second store)

```
┌ My Work ──────────────────────────────────────────────────────────────────────────────────┐
│ ▾ Assigned to me        ● ENG-1421 In Progress · SLA 1h left                                │
│ ▾ Blocked / waiting     ⛔ ENG-1410 blocked_by ENG-1490   [Remind me when unblocked]        │
│ ▾ Needs my approval     [agent] transition ENG-1430 → Done   [Approve] [Edit] [Reject]      │
│ ▾ Overdue               ● ENG-1399 due 2d ago                                               │
│  (each item carries "why it fired" provenance — NOTIF-2; humanised — NOTIF-1)              │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```
- **Empty:** "Nothing needs you right now." (calm-by-default, P8).
- **Loading:** grouped skeletons.
- **Error:** fail-static per group; "Couldn't load 'Overdue'. [Retry]" — the rest still shows.
- It is a **`filter` over the one inbox** (`list_inbox` with a reason/subject filter — Notif §1.3), never a
  separate inbox; read-state is shared with the global inbox (mark once, consistent everywhere).

---

## S6 — Backlog (drag-to-rank; sketch 06)
```
┌ Backlog · ENG ────────────────────────────────  filter ⌗  ─────────────────────────────────┐
│ ⇕ ENG-1440  Refactor auth guards     ⬆High   @—      est 5                                  │
│ ⇕ ENG-1441  Flaky SSO test           ◯Med    @alice  est 2     [→ add to cycle]             │
│   … drag to reorder (LexoRank, CAS-arbitrated) · bulk-move-to-cycle                          │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```
- **Empty:** "Backlog is empty. [Create issue]."  **Loading:** row skeletons.  **Error:** "[Retry]."
- **Rank-rebalancing:** invisible to the user; a background region rebalance never reorders the *displayed*
  order. **Concurrent-reorder conflict:** the losing optimistic move snaps to authoritative + one-line notice.

---

## S13 — Workflow / scheme editor (governance; admin P15; progressive disclosure P4)
```
┌ Workflow scheme: Support ─────────────────────────────────────────────────────────────────┐
│  ┌Todo┐ →[guard]→ ┌In Progress┐ →[guard: CI green]→ ┌In Review┐ → ┌Done┐   ┌Cancelled┐     │
│   unstarted        started                            started      completed  cancelled    │
│  (every state maps to a fixed CATEGORY — the mandatory invariant, sketch 02)               │
│  Selected transition → [ guard builder: query-AST predicate ]  [ post-action ]             │
│  ⚠ Validation: state "Blocked" is unreachable                                              │
│  Assign scheme to:  Type ▾  ×  Team/Project ▾                                               │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```
- **Empty (new scheme):** starts from the Linear-simple default (Todo→In Progress→Done + Cancelled) — the
  no-config baseline, editable (sketch 02).
- **Validation state:** unreachable-state / missing-category-mapping flagged inline (glyph+label) before save.
- **Error:** save failure → one quiet line, no lost edits (the editor holds local state).
- The guard builder is the **shared safe query-AST** builder (sketch 02/03) — no free-form scripting. Overlays
  (the assign dropdown, the guard popover) **portal to root** and **flip when off-screen** (§8b.1/§8b.4).

---

## S17 — Import wizard (PR-8; sketch 09)
```
┌ Import from Jira ────────────────────────────────────────────────────────────────────────┐
│ ① Connect → ② Map (types/statuses/fields/links/users) → ③ Dry-run → ④ Run                  │
│ ── ③ Dry-run reconciliation report ──                                                      │
│   ✓ 1,240 issues mapped     ◐ 38 lossy (ADF rich-text nodes, JQL filters)                  │
│   ✖ 4 dropped (unmappable permission scheme — review)   [Download report]                  │
│   [ Back to mapping ]   [ Run import (resumable) ]                                          │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```
- **Lossy/dropped are named explicitly** (never silent — VISION §5.4 / sketch 09). **Running:** a resumable
  progress state ("4,120 / 12,000 · resumable"). **Partial-failure:** "paused at 6,300 — [Resume]" (idempotent,
  no duplicates). **Error:** rate-limit/credential errors blame the system + a path.

---

## S19 — Command palette / quick-create (shell-global; §5.2)
```
┌ ⌘K ──────────────────────────────────────────────────────────────────────────────────────┐
│ > state:open assignee:me cycle:current                          (query-AST autocomplete)   │
│   Create issue…    Go to ENG-1421…    View: My cycle…    Transition ENG-1421 → In Review…  │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```
- **Loading:** results stream in under the input (no blank flash < 1s — §8b.6 latency budget).
- **Empty query:** recent issues/views + create actions. **No results:** "No matches — [Create issue 'x']."
- Actions map to the **same `ToolDef`s agents use** (§5.2/ADR-08) — humans and agents act through one catalogue.
- Keyboard response < ~100ms (§8b.6 hard latency budget); the palette is a **portalled overlay** (§8b.1).

---

## Cross-cutting wireframe rules applied (the §8b checklist, proven where cited)
- **Overlays** (quick-create, guard popover, assign dropdown, palette, approval card) **portal to root**, share
  one z-index scale, centralise focus-trap/Escape/ARIA, and **flip when off-screen** *(PROVEN — §8b.1/§8b.4)*.
- **One editor render path** for issue body + comments: read mode and edit mode run the **same `myelin-content`
  parser** (ADR-05/KN-4); markdown-subset inline, `mention`/`artifact_ref` as structured nodes *(PROVEN gate —
  round-trip)*.
- **Status by glyph+label+position, never colour alone**; no saturated status fills; **no inline colour on
  interactive elements** *(PROVEN — §8b.3)*.
- **Skeletons match final layout; error blames the system in one line; degraded panes fail static** *(§8b.6)*.
- **Humanised strings at the backend** (NOTIF-1): the activity feed shows "alice moved In Progress," never
  `issue.state_changed`; every inbox item carries "why it fired" *(NOTIF-2)*.
