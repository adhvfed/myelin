# R-10 — Shared Interaction Patterns: Views · Editor · Notifications Inbox · Overlays

> **Phase 4 research corpus** · deliverable of prompt **R-10** (workstream
> [`ws-d`](../../02-research-roadmap/ws-d-interaction-patterns.md), Seq #10). **File date: 2026-06-20.**
> Methods: **#11 atomic design** (loose taxonomy over the §5 inventory), the **§8b.1/§8b.2 day-one
> mandates** (carried verbatim as design rules), **#19 heuristics** + **#20 cognitive walkthrough** as the
> self-critique lens.
>
> **This file specs the four biggest reuse boundaries** after the chip/palette: the **views component**
> (table/board/calendar/list/gallery/timeline), the **block editor**, the **notifications inbox**, and the
> **overlay primitives** (Dialog/Confirm/Popover/Dropdown/Tooltip/Toast). It is the substrate R-16
> (dual-audience), R-21 (state-craft), R-17 (a11y), and Phase 6 compose against.
>
> **Builds ON prior `04-research` (does not duplicate):**
> [R-01 teardown-dossier](../north-star/teardown-dossier.md) (Notion views §2.2 / editor §2.1 / slash §2.3;
> Linear board §1.3; Slack/Linear inbox §3.3/§1.3 + §4.3 batching) and
> [R-06 platform-ia](../ia/platform-ia.md) (the one tree these components hang in; §3.2 the views-tree
> reuse seam; §4.3 the Inbox as a global shell surface). Where this file says "the views-tree shape" or
> "the Inbox is shell-owned", that is R-06 §3.2/§4.3 — not re-stated.
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited standard (WCAG/ARIA-APG, ICU), a vendor-doc
> behaviour, or an existing platform contract we *surface* (notifications.md `origin_event`+`reason`,
> ADR-05/06/07, §8b mandates). **HOUSE STYLE** = our design synthesis/taste. No part is user-validated;
> the deferred validation is named in §6 and routed to R-16/R-07/R-17.

---

## 0. How to read this file

Five parts. Each component family (§2 views, §3 editor, §4 inbox, §5 overlays) follows the **same fixed
structure** so the corpus is scannable:

> **What it is + atomic placement → the interaction spec → the binding rules (§5/§8b law) → the full state
> set → keyboard + a11y model → the trap.**

Then **§1** the atomic taxonomy (the reuse map — the acceptance-criterion artifact for Phase-7 coherence
scoring), **§6** the deferred validation, **§7** completeness-critic, **§8** rubric/funnel actionability,
**§9** sources, **§10** self-check.

**The one-line thesis (HOUSE STYLE):** *these four families are the platform's "build-once" layer. The
views component is the dual-audience mechanism AND the issues↔knowledge reuse boundary; the editor is one
render path everywhere content is authored; the inbox is the one read-state truth; the overlays are the
one focus/z-index/portal substrate. If any subsystem ships its own copy of any of these, the product has
fractured (the R-06 §4 global-surface invariant, applied to components).*

---

## 1. The atomic taxonomy (the reuse map — Phase-7 coherence proof)

Method #11: a **loose** atomic taxonomy over the design-language §5 inventory. The point is not purist
classification; it is to **make cross-component reuse visible** so a Phase-7 reviewer can check "is this
the same molecule rendered twice, or two divergent forks?" (rubric D4). *(Placement HOUSE STYLE; the
shared dependencies — chip, identity badge, query AST, content AST — are PROVEN contracts.)*

| Atom | Molecule | Organism | Shared dependency it must reuse |
|---|---|---|---|
| icon, status-glyph, badge, `focus-ring`, density-token | **reference chip** (R-09), **identity/agent badge** (§5.11), cell-editor, filter-pill, **toast**, **tooltip** | — | one icon set §3.7; `agent` token §3.2 |
| text-input, checkbox, kbd-hint | **slash-menu item**, **palette row** (R-08), **inbox-item row**, **field-editor**, **dropdown item** | — | `ToolDef` catalogue (R-08); `origin_event`+`reason` (inbox) |
| portal, backdrop, focus-scope | **Popover · Dropdown · Tooltip** (transient) | **Dialog · Confirm** (modal) | **one overlay substrate** §8b.1 (this file §5) |
| column header, row, swimlane | **table-view · board-view · list-view · calendar-view · gallery-view · timeline-view** | **the views component** (§5.6) — one organism, six projections of one query AST | one query AST (ADR-07); permission-pre-filter (ADR-03); chip (R-09) |
| block-handle, caret, inline-mark | **block** (heading/list/code/callout/table/embed), **mention node**, **artifact_ref node** | **the block editor** (§5.9) — one render path | one content AST + markdown-subset string (ADR-05/§8b.2); chip (R-09) |
| inbox-item row, "why" provenance line, triage-action | **inbox group** (deduped/coalesced) | **the notifications inbox** (§5.8) — one store, many filtered views | `list_inbox`/`reason` (notifications.md 7.1); chip (R-09); editor render-path for humanised strings |

**The reuse invariant (HOUSE STYLE, the D4 test):** every organism above is built **on** the molecules/
atoms above it — and the **reference chip (R-09)** and **identity badge (§5.11)** appear inside *all four*
organisms (a chip in a board cell, in an editor mention, in an inbox-item subject, in an unfurl inside a
dialog). That single fact is the mechanical coherence guarantee: the wedge component is the connective
tissue between the four families. *Cross-component check for Phase 7: open a `#issue` reference in a board
cell, an editor mention, and an inbox row — it must be the identical chip.*

---

## 2. The Views component (table · board · calendar · list · gallery · timeline)

**What it is.** ONE organism (§5.6, ADR-06) that renders the shared structured-collection primitive as
**six projections of one query AST**, used by **both the issue tracker AND knowledge databases** — the
platform's biggest reuse boundary (R-01 §2.2; R-06 §3.2). **Atomic placement:** organism; composed of
header/row/cell/swimlane molecules; consumes the query AST (ADR-07), the permission pre-filter (ADR-03),
the chip (R-09), the editor (§3, for inline rich cells), and density tokens (§3.4).

**This is simultaneously two load-bearing things** (the acceptance criterion):
1. **The issues↔knowledge reuse boundary** — the *same* component renders `issue` items and `db-row`
   items; the engines underneath (issue workflow/SLA vs knowledge formula/collab) differ and surface
   their own field controls, but the **table/board/field UX is one component** (PROVEN — §5.6, ADR-06).
2. **The dual-audience mechanism** (§2) — the *same records* render as a keyboard-dense engineer **board**
   *and* a spacious PM **timeline/roadmap** *and* an exec **portfolio rollup**. The lens is a *view (a
   saved query AST + grouping + visible-fields + density), not a fork* (PROVEN — §5.6/§2; R-16 owns the
   per-lens critique). This is the literal §2 "one schema, many views" wedge.

### 2.1 Interaction spec — the six projections (one component, switchable for free)

A **view = `query AST` (filter) + grouping + sort + visible fields + projection-type + density** (PROVEN —
ADR-06/07). Switching projection re-renders the *same rows*; it is not a navigation (R-01 §1.3 — "switching
is free"). Saved views are first-class permissioned objects (§5.6).

| Projection | Primary interactions | Persona-default (§2) | Notion/Linear baseline (R-01) |
|---|---|---|---|
| **Table** | inline-edit cell, resize/reorder/add column, multi-select, group-collapse, frozen first column | engineer + PM data work; the spreadsheet mental model (R-01 §2.2) | Notion table; the "zero new concepts" win |
| **Board / kanban** | drag card between status columns (optimistic), WIP limits, swimlanes, `j/k`+arrow card focus, single-key transition on focused card | **engineer default** (cycle board, R-06 §7) | Linear board §1.3 (`j/k`, single-key actions) |
| **List** | grouped/sorted/filtered rows, keyboard-navigable, inline peek, row-action surface | engineer triage; mixed | Linear list §1.3 |
| **Calendar** | drag to reschedule (optimistic), range display, create-on-day | PM/delivery scheduling | Notion calendar §2.2 |
| **Gallery** | card grid, cover field, hover-peek | knowledge/asset browsing | Notion gallery §2.2 |
| **Timeline / Gantt / roadmap** | drag bar to move/resize range, dependency links, now-next-later lanes, zoom | **PM/exec default** (roadmap, R-06 §7) | Notion timeline; Linear cycles |

**Inline-edit (the table/list power surface).** A cell enters edit mode on `Enter`/double-click/typing;
commits on `Enter`/`Tab` (optimistic, R-01 §1.2), reverts on `Escape`; `Tab`/`Shift-Tab` move to the next
cell *while editing* (the spreadsheet contract). Field-type-aware editors (text/number/select/date/person/
relation/formula) are the **same field-editor molecules** the editor's `/database` embed reuses (§5.6
field-definition UI). *(Interaction HOUSE STYLE over the PROVEN §5.6 field model.)*

**Drag (board/calendar/timeline).** Pointer drag is **optimistic** (the card moves immediately, settles
on server-ack, honest-rollback on reject — R-01 §1.2; motion §3.6 "card-moves-column"). Drag **must have a
keyboard equivalent** (see §2.4) — a pointer-only board fails P3 *and* G1 (the §2.4 trap).

**Persona-adaptive deltas as configuration, not code (HOUSE STYLE, the D5 mechanism):** the engineer board
and the PM roadmap differ only by **(a)** default projection, **(b)** density token (`compact` vs
`comfortable`, §3.4/P5), **(c)** vocabulary label (`issue`↔`work item`, R-06 §6.2 — bounded), **(d)** which
fields are visible by default. *Same component, four config values.* R-16 critiques each lens against its
persona to prove neither is a degraded compromise (the §2 "serves neither" trap).

### 2.2 The full state set

| State | Behaviour | Source |
|---|---|---|
| **Empty** | onboarding-forward per projection ("No issues in this cycle — create one `C`"); never a blank grid (§5.10; R-21 owns depth) | §5.10 |
| **Loading** | **structure-skeleton** matching the projection (ghost rows for table/list, ghost cards for board, ghost bars for timeline) — never a blank spinner (§8b.6 PROVEN) | §8b.6 |
| **Error** | one quiet system-blaming line + retry, scoped to the view, not the shell ("Couldn't load this view — retry"); the surrounding shell stays live (§8b.6 fails-static) | §8b.6 |
| **Permission-denied rows** | **never rendered** — the view is permission-pre-filtered by construction (ADR-03), so a forbidden row is *absent*, not greyed; you cannot infer its existence (PROVEN — §5.7, the search/views correctness invariant) | ADR-03 |
| **Erased/tombstoned cell** | a cell whose value is a ref to an erased artifact renders the tombstone chip, never a dangling id (R-09 owns the chip; ADR-12) | ADR-12 |
| **Optimistic-pending** | the dragged card / edited cell shows a subtle pending affordance; settles or rolls back honestly (R-01 §1.2; R-13/R-21 own the rollback craft) | §5.10 |
| **Live-update** | a bus-pushed row change (a PR going green, an issue moving) transitions in subtly without scroll-jump or losing the user's selection (§3.6; firehose resume §7 notifications) | §3.6 |
| **Conflict** | two near-simultaneous edits surface the CAS→CRDT resolution legibly (routed to R-21; the views component must not silently lose an edit) | R-21 |

### 2.3 Atomic placement & reuse callout

Organism. The board-card, table-row, and timeline-bar are **different molecules over the same item model**;
the **cell-editor** molecule is shared with the editor's `/database` embed; the **chip** (R-09) is the
universal cell content for ref-typed fields. A **knowledge page can embed a live board** over an
`ArtifactRef` — the editor (§3) hosts the views organism as an inline embed (PROVEN — §5.9 embeds, beats
Notion's silo boundary, R-01 §2.2 trap).

### 2.4 Keyboard + a11y model (the hard-component obligations — feeds G1/R-17)

The views component is a **named "hard component"** for G1 (rubric G1; R-17). The PROVEN patterns:

- **Roving tabindex** (one cell/row/card has `tabindex=0`, the rest `-1`; arrows move focus by re-roving),
  OR `aria-activedescendant` on the grid container — the two PROVEN composite-grid focus models (PROVEN —
  [W3C ARIA-APG Data Grid](https://www.w3.org/WAI/ARIA/apg/patterns/grid/); roving-tabindex per
  [UXPin ARIA guide, 2026](https://www.uxpin.com/studio/blog/keyboard-navigation-patterns-complex-widgets/)).
  The grid is **one Tab stop**; arrows/Home/End/`Ctrl+Home`/`Ctrl+End` move within (PROVEN — APG grid).
- **`role=grid`/`row`/`gridcell`** (or `treegrid` when rows expand) for table/list; the board is a set of
  labelled columns with focusable cards (a single-Tab-stop composite). **No keyboard trap** — `Escape`
  exits cell-edit back to cell nav; `Tab` exits the grid to the next shell region (PROVEN — G1 2.1.2;
  the AG-Grid keyboard-trap class is the anti-pattern to avoid,
  [ag-grid #12547](https://github.com/ag-grid/ag-grid/issues/12547)).
- **Keyboard drag equivalent** (the load-bearing one): focus a card → `Space`/`Enter` to "pick up" →
  arrows to move between columns/positions → `Space`/`Enter` to drop, `Escape` to cancel — with a
  **live-region announcement** of each move ("Moved to In Progress, position 2"). A drag-only board is a
  G1 failure (PROVEN — keyboard operability 2.1.1; HOUSE STYLE for the exact pick-up grammar).
- **Status never by colour alone** in cells (glyph + label, §8b.3 PROVEN); **inline-edit announces** the
  field name + type to AT on entry; **live updates** use a polite live region (don't spam — §4 PROVEN).
- **200% zoom / reflow + RTL** are the views component's hardest case (R-01 §4.2 flagged it for the diff;
  the table is the parallel here) — routed to R-17/R-18; the component uses logical properties (R-18 G2).

**Cognitive walkthrough (#20):** *can a PM who never learned `j/k` still operate the board?* Yes — every
keyboard action has a visible pointer affordance (drag with mouse, click-menu to transition, click-to-edit
a cell). Keyboard-first is never keyboard-only (P3). *Can an engineer move a card without the mouse?* Yes —
the pick-up grammar above. Both pass.

### 2.5 The trap

**(a) The "two components" temptation.** Under deadline pressure the PM roadmap gets forked into a separate
"roadmap tool" — re-creating the exact Jira-for-engineers / Productboard-for-PMs split Myelin exists to
kill (§2; R-02). **Design rule (falsifiable):** the roadmap and the board MUST be the same component
differing only by the four config values (§2.1); a reviewer can switch projection on the live data and see
the *same rows*. If they can't, it forked. **(b) Drag-only boards** (G1 fail, §2.4). **(c) Inline-edit
that loses an edit on a live update** — the live-update transition must never clobber an in-progress edit
(conflict state, §2.2). **(d) Notion's per-silo view boundary** — a Myelin view/embed should span
subsystems via the reference graph (a board embedded in a runbook); inheriting Notion's "one view can't
span two data sources" limit as if it were law is the trap (R-01 §2.2).

---

## 3. The Block Editor (one render path, one AST)

**What it is.** ONE editor organism (§5.9, ADR-05) over the shared content model — the writing surface for
knowledge pages, issue descriptions/comments, PR descriptions, and chat composition, each with the *same*
node taxonomy (concurrency differs per subsystem). **Atomic placement:** organism; composed of block,
mention-node, artifact_ref-node, slash-menu, caret/selection molecules; consumes the chip (R-09) for ref/
mention rendering and the content AST (ADR-05). Notion is the North Star (R-01 §2.1/§2.3/§2.4); §8b.2
sharpens *past* it.

### 3.1 The binding constraints (the §8b.2 day-one mandates — carried verbatim as design law)

These are not aspirations; they are CI-gateable correctness bars. Each tagged at source:

1. **ONE render path: read and edit run the SAME inline parser** *(HOUSE STYLE → PROVEN via the gate
   below)*. There is no separate "viewer" that diverges from the editor — a binding design constraint
   (§8b.2). The renderer for an issue comment and the editor for it are the same code path.
2. **`render(parse(md)) === md` round-trip over a corpus is a HARD CI gate** *(PROVEN by the corpus)*
   (§8b.2). This is the correctness bar whatever concurrency engine Knowledge picks (TE-15). A sketch that
   shows the editor must treat this round-trip as a binding constraint, not a nicety.
3. **Inline content stored as a markdown-subset STRING** (not an inline-range JSON model) *(HOUSE STYLE
   with a structural reason)* — survives copy/paste, export, diff, reference-extraction; needs no server
   sanitisation pass; zero-migration through an editor rewrite (§8b.2). **Reconciliation with ADR-05: AST
   for block structure, markdown-subset string for inline runs**, with `mention`/`artifact_ref`/`embed`
   kept as **structured nodes — never collapsed into the string** — so reference-extraction (the wedge,
   P6) stays reliable (PROVEN — §8b.2; notifications.md X-2/C10 freezes the `mention(Principal)` node
   identical across Chat/Issues/Knowledge).
4. **Controlled `contenteditable`, NOT `<textarea>`** *(PROVEN — a textarea fundamentally cannot show
   formatting as you type)*; **the caret is a char offset into the serialised markdown**, bridged to/from
   the DOM (§8b.2). Browser variance (Enter/IME/paste) is the **top Knowledge-P4 risk** (§8b.2). This is
   the well-known hard problem: `contenteditable` ignores model state and Chrome/Firefox diverge on caret
   behaviour, so a controlled editor must own its document model and reconcile the DOM, not read from it
   (PROVEN — the ProseMirror-class lesson:
   [HN "taming contenteditable"](https://news.ycombinator.com/item?id=35459610);
   [ProseMirror caret/non-editable-node issues #991](https://github.com/ProseMirror/prosemirror/issues/991)).
5. **Editor primitives ship + unit-test STANDALONE before the integrated editor** *(HOUSE STYLE)* — the
   serializer, the offset model, and the DOM-surgery for Enter-splits-block / caret-after-split are
   independently tested. **"Enter just inserts a newline" is the #1 'not a real editor' tell** (§8b.2;
   corroborates R-01 §2.1 — Notion's polish is years of `contenteditable` engineering).

### 3.2 Interaction spec

- **Block model** (PROVEN — R-01 §2.1): every paragraph/heading/list/table/code/callout/image/embed is a
  block; blocks nest; drag-to-reorder via a handle that also exposes delete/duplicate/convert (the
  six-dot-handle molecule). **Hover-is-not-touch** (§8b.4): the handle has an explicit mobile affordance.
- **Slash menu (`/`)** (PROVEN — R-01 §2.3): `/` opens a ranked, type-to-filter menu of insertable blocks
  *and* **reference/embed nodes** (`/issue`, `/database` live board over an `ArtifactRef`) — the wedge (P6)
  reaches into the editor. **Anti-bloat rule (P4/P8):** a short frequency-ranked default set, depth behind
  search — not a 60-item wall (R-01 §2.3 trap, the Jira-config-maze in miniature).
- **Mention / ref nodes (`@` / `#`)** (PROVEN — R-01 §2.4): `@person`/`@agent`/`#artifact` are **first-class
  inline structured nodes** rendered as §5.3 chips (R-09). `@agent` is a **trigger into the agent fabric**
  (ADR-08); `#artifact` spans all five subsystems (the wedge). These nodes are **never collapsed into the
  markdown string** (§3.1 rule 3) — they carry the `ArtifactRef`.
- **One editor, many concurrency models** (PROVEN — §5.9): knowledge = full collaborative (CRDT/OT,
  TE-15); chat messages = small/mostly-immutable; issue descriptions = single-author-at-a-time. They share
  the **editor component + AST**, not the engine ("share the AST, not the editor engine", ADR-05). The user
  experiences one writing surface.
- **Embeds:** a knowledge page embeds a live issue board (the §2 views organism over an `ArtifactRef`),
  a runbook references a CI run — embeds are reference nodes rendered inline (PROVEN — §5.9).
- **Humanised strings render through THIS render path** — the inbox's humanised notification strings and
  CI status summaries render via `myelin-content`, never leaked raw (PROVEN — notifications.md §3.3, C2:
  one templating surface → one render path; this couples §3 and §4).

### 3.3 The full state set

| State | Behaviour |
|---|---|
| **Empty** | a calm placeholder + slash-hint ("Type `/` for blocks, `@` to mention"); onboarding-forward (§5.10) |
| **Loading** | block-skeleton matching final structure (headings/paras as ghost bars), never blank spinner (§8b.6) |
| **Saving / pending** | optimistic; a quiet "saving…/saved" affordance; never a blocking modal (P2) |
| **Error (save failed)** | one quiet line + retry; **the typed content is never lost** (local buffer) — error blames the system (§8b.6) |
| **Permission-denied** | read-only render with a graceful "you can view but not edit" cue, or a no-access card for the whole doc (ADR-03); never a silent swallow of edits |
| **Erased/tombstoned ref** | a mention/ref node to an erased artifact renders the tombstone chip inline (R-09; ADR-12) |
| **Conflict (collab)** | concurrent edits surface presence + the CRDT/OT merge; conflict is shown legibly, not silently dropped (TE-15; routed to R-21) — `[OPEN → P4]` per §9 of design-language |
| **Offline / reconnecting** | buffered locally; re-syncs on reconnect (offline scope is `[OPEN → P4]`, design-language §9; R-21 owns the reconnecting state) |

### 3.4 Atomic placement & reuse callout

Organism. The **mention/artifact_ref node molecule = the R-09 chip** (the single most important reuse:
the chip you see in chat is the chip rendered inside the editor). The **slash-menu-item molecule** shares
shape with the **palette row** (R-08) and the **dropdown item** (§5) — type-to-filter, kbd-hint, icon.
The **`/database` embed** hosts the §2 views organism. One editor, consumed by four subsystems = the P1
coherence guarantee made mechanical.

### 3.5 Keyboard + a11y model (hard component — feeds G1/R-17)

- **Full keyboard operability** (PROVEN — §4/G1): all block ops reachable by keyboard (slash insert,
  Enter-split, Backspace-merge, block-move via shortcut, mark toggles `Cmd-B`/`Cmd-I`); **no trap** — the
  embedded non-editable nodes (chips) must allow caret to pass and `Tab` to exit (the PROVEN
  contenteditable-island caret pitfall, [ProseMirror #991](https://github.com/ProseMirror/prosemirror/issues/991)).
- **Screen-reader correctness is a component contract** (PROVEN — §4): the editor ships correct semantics
  once; AT announces block type on entry; the round-trip markdown is the accessible text fallback.
- **IME / composition events** handled (CJK + accented EU input) — part of the §8b.2 controlled-caret model
  and a G2 (i18n) obligation (R-18).
- **Sanitisation + safe rendering** is a component responsibility inherited by all consumers (PROVEN —
  ADR-05).

### 3.6 The trap

**(a) The viewer/editor divergence** — shipping a fast read-only renderer that drifts from the editor is
how `render(parse(md)) !== md` and the "preview looks different from edit" bug is born (§8b.2 rule 1/2).
**(b) "Enter inserts a newline"** — the #1 fake-editor tell; the standalone-tested DOM-surgery primitive
is the counter (§8b.2 rule 5). **(c) Proprietary block lock-in / weak export** (Notion's trap, R-01 §2.1)
— the markdown-subset-string + open-format export is the deliberate counter (P14/§8b.5). **(d) Collapsing
ref/mention nodes into the string** — destroys reference-extraction and the wedge (§3.1 rule 3); they stay
structured nodes. **(e) Slash-menu bloat** (P4, §3.2).

---

## 4. The Notifications Inbox ("what needs *me*")

**What it is.** The ONE prioritised cross-subsystem inbox (§5.8) — the antidote to notification overload
(P8; the universal incumbent failure). **Atomic placement:** organism (a shell-owned global surface, R-06
§4.3); composed of inbox-item-row + provenance-line + triage-action + deduped-group molecules; consumes
`list_inbox`/`reason` (notifications.md 7.1), the chip (R-09) for subject rendering, and the editor render
path for humanised strings (§3.2). **This file surfaces the existing notifications architecture as UX — it
invents no new mechanism** (the acceptance criterion).

### 4.1 The binding rules surfaced from notifications.md (PROVEN contracts, not new)

- **One store, one read-state truth** (PROVEN — notifications.md §1.3, C-9): there is exactly **one** inbox.
  Issues "My Work", Chat "Activity/Mentions", Git "Review requests" are **scoped `filter`s over one store**
  — never separate inboxes. *Read it in chat → it's read in the unified inbox* (one `state` column across
  every view, notifications.md §2.1). **Design-language one-liner (carried verbatim):** *"there is one
  inbox; everything else is a saved filter on it."*
- **"Why am I getting this" provenance on every item** (PROVEN — notifications.md §2.1, NOTIF-2): every
  item carries `origin_event` + structured `reason` (`mentioned`/`assigned`/`review_requested`/`sla`/
  `approval_requested`/`watched`/`replied`/`agent_proposal`/`state_changed`/`fyi`). The UI renders this as
  a one-line, humanised provenance ("Review requested by @ana on PR #88") — answerable inline, **not a new
  mechanism**. This matches and beats the GitHub/Linear baseline (GitHub shows a `reason` label —
  `mention`/`review_requested`/`subscribed`; Linear restricts to @mention/assignment/overdue —
  [GitHub notifications docs](https://docs.github.com/en/subscriptions-and-notifications/concepts/about-notifications),
  [Linear notifications docs](https://linear.app/docs/notifications)).
- **Deterministic, explainable ranking** (PROVEN — notifications.md §3.1): `priority 0..100` from
  `reason → base → class` (approval/escalated/sla = 90/critical; review/assigned/mentioned = 70/direct;
  replied/agent_proposal = 55/participating; watched/state_changed = 35/watching; fyi = 15). **Why
  deterministic-first:** an unpredictable ranking erodes trust faster than no ranking; "why ranked here?"
  must be answerable (the explain-trace, D-N1 drill). ML-tuned ranking is a named follow-on behind the
  same interface — the UI must not assume it.
- **Dedup / storm-control is write-time** (PROVEN — notifications.md §3.2): N identical events collapse to
  one item with a "+N more" coalesce-count (`UNIQUE(tenant, recipient, dedup_key)`); thread/subject
  coalescing digests the participating and breaks out the direct; self-notifications dropped. The UI shows
  **groups, not a firehose** — grouped by artifact/thread, the deduped item carrying the "+N more".
- **Humanised strings, not raw ids** (PROVEN — notifications.md §3.3 / §8b.5): the human string is rendered
  **per-viewer at read time** by resolving each `ArtifactRef` through Refs `resolve(Display)` — a
  confidential subject humanises to "Alice updated a restricted issue" (title never leaks); an erased actor
  → "[erased user]". The frontend **never owns a humanisation lookup** (§8b.5). This is the same
  permission-safe / erasure-safe / always-current property the chip has (R-09).

### 4.2 Interaction spec

- **One-action triage** (PROVEN — §5.8): each item has done / snooze / mute / go (open) — single-action,
  keyboard-first (`E` done, `H` snooze, `Enter`/`G` go, à la Linear; pointer-complete for PMs). `mark_all_read`
  on a filter (notifications.md 7.2).
- **Prioritised + grouped, calm by default** (PROVEN — §5.8/P8): default view is "what needs me" ranked;
  deduped groups collapse with "+N more"; **agent-generated volume routed OUT of the main stream**
  (threads/collapsible summaries — §6.5; notifications.md §5.2 the agent-mention lane is shed-budgeted
  separately so humans never queue behind agent runs). Quiet is the default; the user opts *into* more.
- **HITL approvals appear here** (PROVEN — §5.8/§6.3): an agent approval card is an inbox item with
  `reason=approval_requested` at critical priority (notifications.md §1.4) — a human gate is never missed.
  R-14 owns the card; the inbox is its second home.
- **Live updates** (PROVEN — notifications.md §7, C4): new items stream over the firehose resume-cursor
  protocol; a reconnect loses zero items (D-N11). The UI transitions a new item in subtly (§3.6), never a
  jarring jump.
- **Tunable** (PROVEN — §5.8): per-type/per-scope preferences over the frozen `QueryAst` matcher
  (notifications.md C9); quiet-hours in the recipient tz, **critical pierces** (you cannot silence an
  on-call page, D-N8). The default is quiet.

### 4.3 The full state set

| State | Behaviour | Source |
|---|---|---|
| **Empty (inbox zero)** | a calm, *rewarding* empty ("You're all caught up") — not an onboarding nag; the calm-by-default payoff (P8) | §5.10/HOUSE STYLE |
| **Loading** | item-row skeletons matching the final list, never a spinner (§8b.6) | §8b.6 |
| **Error** | one quiet line + retry; **fails static** — already-materialised items still render on an Id hiccup (PROVEN — notifications.md §5.3) | notifications.md §5.3 |
| **Permission-changed subject** | an item whose subject the viewer just lost access to humanises to a tombstone/"restricted", never a leaked title (PROVEN — notifications.md §3.3, D-N4) | ADR-03/§3.3 |
| **Erased actor/subject** | "[erased user]" / restricted tombstone; references-not-payloads makes this free (PROVEN — notifications.md §3.9, D-N6) | ADR-12 |
| **Deduped group** | "+N more" coalesce; expandable to the N underlying events (notifications.md §3.2) | §3.2 |
| **Storm / 30×-agent-surge** | the agent lane sheds first (`429+Retry-After`); the **human inbox-read stays in budget**; humans never queue behind agent runs (PROVEN — notifications.md §5.2/D-N5). The UI shows agent volume collapsed/threaded, the human-direct items unburied. **This is the storm state R-21 owns; surfaced here as the inbox's defining stress case.** | notifications.md §5.2 |
| **Reconnecting (firehose drop)** | resume from `last_seq` (backfill then live); `resync_required` → full `list_inbox` reload, named not silent (PROVEN — notifications.md §7, D-N11) | notifications.md §7 |
| **Agent-pending** | "an agent is working / awaiting your approval" (the §6 agent-pending state; R-14) | §5.10/§6 |

### 4.4 Atomic placement & reuse callout

Organism, shell-owned (R-06 §4.3 — binding it to one subsystem would re-fracture the product). The
**inbox-item-row molecule** shares the row+kbd-hint shape with the **palette row** (R-08) and **list-view
row** (§2); the **subject is rendered as the R-09 chip**; the humanised string is rendered via the
**editor render path** (§3.2). The HITL approval card (R-14) docks here as a row.

### 4.5 Keyboard + a11y model

- **Keyboard-first triage** (P3): `j/k` move, single-key actions on focus, `Enter` to open; pointer-complete.
- **Live-region announcements** of new high-priority items **without spamming** (PROVEN — §4: announce
  critical/direct, not every fyi; the polite-region discipline).
- **Status not by colour alone** (priority/class shown by glyph+label+position, §8b.3).
- **Cognitive walkthrough (#20):** *can a new PM tell why an item is here and what to do?* Yes — the
  provenance line ("why") + the explicit triage actions are visible, not memorised.

### 4.6 The trap

**(a) Three inbox-like surfaces** (the exact failure the platform exists to fix) — defeated by the
one-store / filtered-views rule (§4.1; notifications.md §1.3). A design that gives Chat its own message
store has fractured. **(b) The firehose** (notification overload, P8) — defeated by dedup + ranking +
agent-volume-out-of-stream + quiet-by-default; the storm state (§4.3) is the proof. **(c) Unexplainable
ranking** — an ML black box that buries a critical item under fyi (D-N1); the deterministic explain-trace
is the counter. **(d) Read-state drift** — reading in one view not marking read in another (the one-store
rule kills it). **(e) Quiet-hours over-suppression** — silencing an on-call page (D-N8); critical pierces.

---

## 5. The Overlay Primitives (Dialog · Confirm · Popover · Dropdown · Tooltip · Toast)

**What it is.** A shared, single set of overlay primitives, built **before any feature consumes them** —
the most expensive UX retrofit, so it is a §8b.1 day-one / Phase-6 sequencing prerequisite. **Atomic
placement:** the **portal/backdrop/focus-scope** is an atom-level substrate; **Popover/Dropdown/Tooltip**
are transient molecules; **Dialog/Confirm** are modal organisms; **Toast** is a transient molecule in a
dedicated region. **Every consumer inherits the centralised behaviour free.** This is the §8b.1 mandate —
**carried verbatim as design rules** (the acceptance criterion).

### 5.1 The §8b.1 mandates — VERBATIM as binding design rules

1. **Portal-always to the document root** *(PROVEN — transformed/overflow-clipped ancestors otherwise clip
   the overlay)*: the "create dialog renders inside the 240px sidebar" bug class is **forbidden by
   construction**. Every overlay portals to root, never renders in the triggering subtree.
2. **One documented z-index scale** *(PROVEN)*: `chrome < popover < modal < toast` as a **single token**;
   per-component magic z-index numbers are **banned**. (A toast must always clear a modal; a modal must
   always clear a popover.)
3. **Centralised behaviour lives in the primitive, inherited free by every consumer** *(PROVEN —
   WCAG/ARIA)*: **focus-trap + return-focus, scroll-lock with scrollbar-width compensation, Escape +
   backdrop dismiss, and correct ARIA**. Consumers **never re-implement these**. This is the §4
   screen-reader-correctness-once contract for overlays.
4. **Single-purpose by shape** *(HOUSE STYLE)*: split overlays by shape — **viewport-pinned popover /
   inline-flow dropdown / externally-positioned grid** — "nine menus are three shapes"; don't force one
   component to do all three. A design-review heuristic.

These are PROVEN-grounded by the ARIA-APG modal-dialog pattern: **move focus in on open; trap focus
(Tab/Shift-Tab cycle, no escape to background); Escape closes; return focus to the trigger on close;
`role=dialog` + `aria-modal=true` + `aria-labelledby`/`aria-describedby`; background made inert** (PROVEN —
[W3C ARIA-APG Modal Dialog](https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/examples/dialog/);
[UXPin Accessible Modals with Focus Traps, 2026](https://www.uxpin.com/studio/blog/how-to-build-accessible-modals-with-focus-traps/);
[TestParty modal a11y](https://testparty.ai/blog/modal-dialog-accessibility)).

### 5.2 The six primitives — shape, behaviour, ARIA, dismissal

| Primitive | Shape | Modal? | Focus | Dismiss | ARIA role | Used by |
|---|---|---|---|---|---|---|
| **Dialog** | viewport-centred modal | yes | trap + return | Escape, backdrop, explicit close | `dialog` + `aria-modal` + labelledby | create/edit forms, settings, branch-protection editor |
| **Confirm** | small modal | yes | trap + return; **default focus the SAFE action** | Escape = cancel | `alertdialog` + describedby | irreversible/consequential + GDPR/HITL actions (§8b.6 carve-out) |
| **Popover** | viewport-pinned, anchored, flips off-screen | no (non-modal) | focus moves in; returns on close; **no background trap** | Escape, click-outside | `dialog` (non-modal) or labelled region | reference hovercard/unfurl (R-09), filter builder, date picker |
| **Dropdown / Menu** | inline-flow anchored list | no | roving within; returns on close | Escape, click-outside, select | `menu`/`menuitem` (or `listbox`) | row actions, block convert, palette overflow |
| **Tooltip** | tiny anchored label | no | **never takes focus**; shows on hover AND focus | blur, Escape, pointer-leave | `tooltip` + `aria-describedby` | icon-button labels, truncated text |
| **Toast** | corner region, transient | no | **never steals focus**; AT via live region | auto-timeout + manual + pause-on-hover | `status`/`alert` live region | optimistic-settle confirms, async results, undo (§8b.6) |

**Reversibility over confirmation** (PROVEN-routed, §8b.6 HOUSE STYLE): prefer an **undo toast** over a
Confirm dialog — *with the carve-out that irreversible/consequential + GDPR/agent-HITL actions still
Confirm* (§6.3). So most "delete" → optimistic + undo-toast; a *GDPR erase* or an *agent merge approval* →
Confirm/HITL. The Confirm's default focus is the **safe** action (Cancel), not the destructive one.

### 5.3 The full state set (overlays have states too)

| State | Behaviour |
|---|---|
| **Opening / closing** | fast, interruptible motion (≈120–200ms, §3.6); **pages render, they don't animate in** (§8b.6) — the overlay appears, it doesn't slide a paragraph |
| **Anchored-overflow** | popover/dropdown **flips above + caps max-height** when it would go off-screen; **tested against the REAL anchor** (a picker under a bottom-pinned composer renders off-screen otherwise — PROVEN §8b.4) |
| **Mobile** | the dropdown/popover may become a bottom-sheet drawer (backdrop + Escape + route-change auto-close, §8b.4); contextual sidebar / context pane become drawers (R-06 §3.5) |
| **Loading (async content)** | a popover/dialog loading remote content shows structure-skeleton, not a spinner (§8b.6) |
| **Error (async action in dialog)** | inline error in the dialog, one quiet line; the dialog stays open with the user's input intact (§8b.6) |
| **Nested** | a Confirm over a Dialog; a dropdown inside a popover — the z-index scale (§5.1.2) and a focus-trap **stack** keep order correct; Escape closes the top-most only (HOUSE STYLE over PROVEN APG) |
| **Reduced-motion** | the open/close transition collapses to an instant show/hide — a **first-class path, not a degraded one** (PROVEN — §4 `prefers-reduced-motion`) |

### 5.4 Atomic placement & reuse callout

The **portal + backdrop + focus-scope** is the shared substrate atom every overlay composes from — this is
where the focus-trap/return/scroll-lock/Escape/ARIA logic lives **once**. The **dropdown-item molecule** is
the same as the slash-menu item (§3) and palette row (R-08). The **popover** hosts the R-09 unfurl
hovercard and the §2 filter builder. **One substrate = the mechanical a11y guarantee for every transient/
modal surface in the platform.**

### 5.5 Keyboard + a11y model (this IS the a11y substrate — feeds G1/R-17)

The overlay substrate is **where most of G1's overlay obligations are discharged once** (R-17 audits it as
a hard component). PROVEN requirements (§4/G1; ARIA-APG):
- Focus moves into the overlay on open; **trapped** in modals (Tab/Shift-Tab cycle, no background escape);
  **returned to the trigger** on close (PROVEN — APG; the "point of regard" rule).
- **Escape closes** (modal: closes + returns; nested: top-most only); **no keyboard trap** that can't be
  escaped (G1 2.1.2 — a trap you *can't* Escape is a failure; a *deliberate* modal trap you *can* Escape is
  correct).
- Background **inert** via `aria-modal=true` (replaces `aria-hidden` juggling — PROVEN APG note) so AT
  can't wander into background content.
- **Visible focus** on the overlay's focusable elements in every theme (one `focus-ring` token, §4/G1).
- **Tooltip/Toast never steal focus**; tooltip shows on **focus as well as hover** (PROVEN — keyboard
  users; G1 1.4.13 content-on-hover-or-focus dismissible/persistent).
- **Scroll-lock compensates scrollbar width** so the page doesn't shift on open (PROVEN §8b.1).

### 5.6 The trap

**(a) Per-feature overlays** — every team rolling its own dropdown re-implements (and breaks) focus-trap/
Escape/ARIA; the single substrate is the only defence (§5.1.3). **(b) Magic z-index** — the `z-index: 9999`
arms race where a toast hides behind a modal; the single scale token bans it (§5.1.2). **(c) The clipped
overlay** — a dialog rendered in a `transform`ed/`overflow:hidden` sidebar (§5.1.1). **(d) Confirm fatigue**
— a Confirm on every action trains users to click through; reversibility-over-confirmation (§5.2) reserves
Confirm for the consequential/GDPR/HITL set. **(e) Off-screen popover** under a bottom-pinned anchor
(§8b.4) — the flip+real-anchor-test rule. **(f) The focus-not-returned bug** — closing a dialog drops focus
to `<body>`, stranding keyboard/AT users (PROVEN APG return-focus rule).

---

## 6. `[DEFERRED-UNTIL-USERS]` — what these specs have NOT earned

R-10 is `user-dep: none`, so the deliverable IS the no-user substitute (the specced patterns + heuristic/
walkthrough self-critique). But three claims are **HYPOTHESES** that only users settle; recorded as
executable plans, **not faked as validated**:

1. **The views component serves both audiences from one component without degrading either** (the D5 bet).
   - *No-user substitute done now:* the config-delta model (§2.1) + the per-lens critique is **R-16's owned
     deliverable** (engineer board as P1, PM roadmap as P6, exec rollup as P11).
   - *`[DEFERRED-UNTIL-USERS]` plan:* per-segment usability/RITE on the **same** surface in **both** lenses
     (engineers P1–P5 vs PMs P6–P10). *Falsifier:* either segment finds its lens a starved compromise, or
     they believe they're looking at *different objects* (the §2 dual-product split returns). **Phase 6 must
     sketch dual-audience surfaces in BOTH lenses over the same data** (R-16 mandate).
2. **The inbox ranking surfaces the right things (not "important buried")** — only real notification mixes
   prove it. *No-user substitute:* deterministic + explainable v1 (the D-N1 replay drill is the
   architecture's own gate). *`[DEFERRED-UNTIL-USERS]` plan:* measure `important_buried_rate` on real
   inbox traffic; *falsifier:* a critical item ranked below an fyi in practice. (ML-tuned ranking is gated
   behind this measured signal — notifications.md §10.4.)
3. **The editor `contenteditable` model survives real input** (IME, paste-from-Word, EU-language accents,
   mobile keyboards). *No-user substitute:* the §8b.2 standalone-primitive unit tests + the round-trip CI
   gate. *`[DEFERRED-UNTIL-USERS]` plan:* AT + real-input testing per R-17's deferred AT-user plan;
   *falsifier:* a caret/Enter/IME class of bug the round-trip gate didn't catch.

Overlay a11y is **not** deferred-as-unknowable — it is PROVEN-conformant by construction (§5.5, ARIA-APG);
the deferred piece is **AT user testing** (R-17's plan: AA ≠ usable-with-AT).

---

## 7. Completeness-critic (README §9) — gloss-risks this item touches

R-10 **owns** the keyboard ops of the hard components and many unglamorous states; it covers them here and
routes depth to the owners (honest scoping):

- **Keyboard operability of hard components** — **owned/covered**: views grid (§2.4 roving-tabindex +
  keyboard-drag), editor (§3.5 + the contenteditable-island caret pitfall), overlays (§5.5 trap/return),
  inbox (§4.5). Full a11y audit → R-17; these are the keyboard *interaction* specs R-17 audits against.
- **All unglamorous states** — **covered per component** (§2.2/§3.3/§4.3/§5.3): empty/loading/error/
  permission/erased/optimistic/conflict/reconnecting/storm. The **cross-component state catalogue** (every
  component × every state, §8b.6 specifics applied uniformly) is **R-21's owned deliverable** — this file
  gives R-21 its per-component starting set, not the full matrix.
- **Storm / 30×-agent-surge** — **placed** as the inbox's defining stress case (§4.3, surfaced from
  notifications.md §5.2/D-N5); the inbox *experience* of the storm → R-21.
- **Optimistic-rollback** — **placed** (views drag/edit §2.2; editor save §3.3); the rollback *craft* →
  R-13/R-21.
- **Conflict (CAS→CRDT)** — **placed** (views §2.2; editor §3.3); legible surfacing → R-21; the collab
  concurrency UX is `[OPEN → P4]` (design-language §9, TE-15).
- **Consciously deferred (with reason):** the full per-surface state matrix (R-21), the per-lens dual-
  audience critique (R-16), the measured a11y audit (R-17), motion tokens (R-12) — duplicating them breaks
  the cumulative-corpus rule. Named-and-routed, not specced.

---

## 8. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric D5** (dual-/tri-audience, 10%) | The **mechanism**: views component = one organism, four config deltas (projection/density/vocabulary/fields); the "switch projection on the same rows" reviewer test; the two-components trap as a falsifiable rule. R-16 critiques the lenses; this file gives the component. | §2.1, §2.5 |
| **rubric D7** (density-made-calm, 8%) | The inbox patterns: one store, dedup, deterministic ranking, agent-volume-out-of-stream, quiet-by-default, the storm state staying in human budget. Concrete, not "be calm." | §4 |
| **rubric G1** (a11y gate) | The hard-component keyboard models (views roving-tabindex + keyboard-drag §2.4; editor §3.5; **overlay substrate §5.5 — where most overlay G1 obligations discharge once**); status-not-colour; visible focus. Makes G1 *checkable* per component for R-17. | §2.4, §3.5, §4.5, §5.5 |
| **rubric D4** (coherence) | The atomic taxonomy (§1) makes reuse *visible*: the chip + identity badge inside all four organisms; the cross-component "same chip everywhere" test. | §1 |
| **sketch-funnel** | Density Axis 1 (views density modes §2.1; inbox calm §4); Surface-unification Axis 3 (views as the issues↔knowledge reuse boundary §2); Agent-presence Axis 5 (agent volume routing §4.2). Phase 6 builds finalists *on* these four organisms. | §2, §4 |
| **R-16 / R-21 / R-17 / R-12** | R-16 ← views config-delta model (§2.1); R-21 ← per-component state sets (§2.2/§3.3/§4.3/§5.3); R-17 ← hard-component keyboard models (§2.4/§3.5/§4.5/§5.5); R-12 ← the motion hooks (drag-settle, live-update, overlay open). | §2–§5 |

---

## 9. Sources (web-verified 2024–2026 + cited platform contracts)

**Standards / patterns (PROVEN):**
- W3C ARIA-APG Modal Dialog pattern (focus trap, return-focus, Escape, `aria-modal`, inert background):
  https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/examples/dialog/
- W3C ARIA-APG Data Grid pattern (roving tabindex / `aria-activedescendant`, arrow/Home/End nav):
  https://www.w3.org/WAI/ARIA/apg/patterns/grid/
- Accessible modals with focus traps (corroboration, 2026): https://www.uxpin.com/studio/blog/how-to-build-accessible-modals-with-focus-traps/
- Keyboard navigation patterns for complex widgets / roving tabindex (2026): https://www.uxpin.com/studio/blog/keyboard-navigation-patterns-complex-widgets/
- Modal dialog a11y (focus trapping + SR support): https://testparty.ai/blog/modal-dialog-accessibility
- `contenteditable` hard problems (controlled editor, caret/DOM divergence): https://news.ycombinator.com/item?id=35459610
- ProseMirror caret around non-editable inline nodes (the chip-island pitfall): https://github.com/ProseMirror/prosemirror/issues/991
- Data-grid keyboard-trap anti-pattern: https://github.com/ag-grid/ag-grid/issues/12547

**Vendor baselines (PROVEN-as-reported; the bar to meet/beat):**
- GitHub notifications — `reason` label provenance, grouping, read-state: https://docs.github.com/en/subscriptions-and-notifications/concepts/about-notifications
- Linear notifications — narrow alert types (mention/assignment/overdue), grouped: https://linear.app/docs/notifications

**Platform contracts surfaced (PROVEN, internal — not re-derived):**
- notifications.md (one inbox §1.3/C-9; `origin_event`+`reason` §2.1; deterministic ranking §3.1; dedup
  §3.2; humanise §3.3; storm shed-budget §5.2/D-N5; firehose resume §7/D-N11; permission/erasure D-N4/D-N6).
- design-language §5.6 (views), §5.8 (inbox), §5.9 (editor), §8b.1 (overlays), §8b.2 (editor render path),
  §8b.4 (layout-containment bugs), §8b.6 (states/budgets/reversibility); ADR-05/06/07.

**Honest limitation (not deferred-until-users per se):** the standards claims are grounded in cited
2024–2026 sources and the W3C APG; vendor inbox specifics carry the same "expert-teardown-not-hands-on"
caveat as R-01 §9. The internal-contract claims are PROVEN against the frozen notifications.md / ADRs.

---

## 10. Self-check against R-10 acceptance criteria

| Criterion (prompt R-10 / ws-d) | Status | Evidence |
|---|---|---|
| **All four component families specced with state sets** | ✅ Met | §2 views, §3 editor, §4 inbox, §5 overlays — each with interaction spec + full state set (§2.2/§3.3/§4.3/§5.3) |
| **Views shown as the issues↔knowledge reuse boundary AND the dual-audience mechanism** | ✅ Met | §2 opener (the two load-bearing things); §2.1 (config-delta lens model); §2.3 (db-row vs issue, embeds) |
| **Editor's one-render-path + round-trip constraint stated as binding** | ✅ Met | §3.1 rules 1–2 (one render path; `render(parse(md))===md` hard CI gate) carried verbatim from §8b.2 as design law |
| **Inbox surfaces "why it fired" from existing `origin_event`+`reason` (not a new mechanism)** | ✅ Met | §4.1 (PROVEN from notifications.md §2.1/NOTIF-2; one store §1.3/C-9); §4 invents nothing |
| **Overlay primitives carry §8b.1 mandates verbatim as design rules** | ✅ Met | §5.1 (portal-always / one z-scale / centralised focus-trap+return+scroll-lock+Escape+ARIA / single-purpose-by-shape) verbatim |
| **Atomic taxonomy makes cross-component reuse visible for Phase-7 coherence** | ✅ Met | §1 (atom/molecule/organism map + the chip-in-all-four reuse invariant + the D4 reviewer test) |
| **Methods #11 / §8b.1 / §8b.2 / #19 applied** | ✅ Met | #11 §1; §8b.1 §5; §8b.2 §3.1; #19 traps per family; #20 walkthroughs §2.4/§4.5 |
| **Builds ON R-01 + R-06, doesn't duplicate** | ✅ Met | cites R-01 §1.3/§2.1–§2.4/§3.3/§4.3 and R-06 §3.2/§4.3 by section; not re-stated |
| **PROVEN/HOUSE-STYLE tags + date + cited web sources** | ✅ Met | tagged throughout; dated 2026-06-20; ARIA-APG + vendor + internal contracts cited (§9) |
| **§9 gloss-risks addressed (keyboard ops of hard components; unglamorous states)** | ✅ Met | §7 (owned: keyboard ops §2.4/§3.5/§4.5/§5.5; states per component; storm/optimistic/conflict placed, routed to R-21/R-17/R-13) |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §6 (`[DEFERRED-UNTIL-USERS]`: dual-audience both-lens, inbox important-buried, editor real-input/AT — each with falsifier) |
| **Actionable toward rubric D5/D7/G1 + funnel; feeds R-16/R-21** | ✅ Met | §8 mapping |

**Top uncertainties (honest):**
1. **The dual-audience "one component, both lenses, neither degraded" claim (§2)** is a HYPOTHESIS — the
   config-delta model is our HOUSE-STYLE bet; only R-16's per-lens critique + the deferred per-segment
   test (§6.1) settle it. *Largest uncertainty in the file.*
2. **Controlled-`contenteditable` real-input robustness (§3.1 rule 4)** — the §8b.2 mandates are the right
   defence (PROVEN class of bug), but IME/paste/mobile edge cases are notoriously where editors fail; the
   standalone-test discipline is the bet, not a guarantee (§6.3).
3. **Inbox ranking trust (§4.1)** — deterministic-first is the trust-preserving choice, but whether the v1
   weights bury anything is measured-not-known (§6.2; D-N1).
4. **Overlay nested-stack correctness (§5.3)** — the z-scale + focus-trap-stack is the model; deeply nested
   overlays (Confirm-over-Dialog-over-Popover) are a known foot-gun the substrate must be tested against.

---

*End of R-10 deliverable. Date: 2026-06-20. Interaction specs HOUSE STYLE over the PROVEN §5.6/§5.8/§5.9/
§8b.1/§8b.2 mandates, ADR-05/06/07, and the frozen notifications.md contracts; not user-validated — see §6.
Builds on R-01, R-06. Feeds R-16, R-21, R-17, R-12, Phase 6.*
