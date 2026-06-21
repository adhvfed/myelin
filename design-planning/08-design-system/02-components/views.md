# Component spec — Views (table · board · calendar · list · gallery · timeline)

> **Phase 8b · `02-components/` · Tier-2 shared component.** Direction = finalist **A "Instrument"**
> (consumes [`../01-tokens/tokens.css`](../01-tokens/tokens.css)). **File date: 2026-06-20.**
> Stack: TS + React (function components) + **React Aria Components**. **Not committed.**
>
> **Implements:** design-language **§5.6** (the tables/boards/views component — the biggest reuse boundary;
> one component, multiple projections; persona-adaptive) + **§5.10** (states). Research it renders:
> [`shared-patterns.md`](../../04-research/interaction/shared-patterns.md) (R-10 §2 — the six projections,
> config-delta lens model, keyboard-drag, the trap) · [`state-craft.md`](../../04-research/craft/state-craft.md)
> (R-21 §2 matrix) · [`reference-unfurl.md`](../../04-research/interaction/reference-unfurl.md) (R-09 — chips
> as cell content).
>
> **Tagging:** **PROVEN** = a cited standard (ARIA-APG Grid), or an existing contract surfaced (one query AST
> ADR-07; permission-pre-filter ADR-03; the shared field model ADR-06). **HOUSE STYLE** = synthesis.
> `[DEFERRED-UNTIL-USERS]` = the dual-audience hypothesis (R-16 owns).
>
> **Reuse:** ref-typed cells render **[`<ReferenceChip>`](./reference-chip-and-unfurl.md)**; the cell-editor
> molecule is shared with the [`<BlockEditor>`](./block-editor.md)'s `/database` embed; a knowledge page can
> **embed a live board** over an `ArtifactRef` (the editor hosts this organism inline).

---

## 1. Name + purpose

**`<Views>`** — ONE organism that renders the shared structured-collection primitive (ADR-06) as **six
projections of one query AST**, used by **both the issue tracker AND knowledge databases**. It is
**doubly load-bearing**:
1. **The issues↔knowledge reuse boundary** — the *same* component renders `issue` items and `db-row` items;
   the engines underneath differ (workflow/SLA vs formula/collab) and surface their own field controls, but
   the **table/board/field UX is one component** (PROVEN — §5.6, ADR-06).
2. **The dual-audience mechanism** — the *same records* render as a keyboard-dense engineer **board**, a
   spacious PM **timeline/roadmap**, and an exec **portfolio rollup**. **The lens is a *view* (a saved query
   AST + grouping + visible-fields + density), not a fork** (PROVEN — §5.6; R-16 owns the per-lens critique).

**A view = `query AST` (filter) + grouping + sort + visible fields + projection-type + density.** Switching
projection re-renders the *same rows*; it is **free** (not a navigation). Saved views are first-class
permissioned objects. *(PROVEN — ADR-06/07; interaction HOUSE STYLE.)*

---

## 2. Anatomy

```
┌──────────────────────────────────────────────────────────────────┐
│ {view tabs / projection switcher}   {filter} {group} {sort} {⊞}   │  ← view bar (projection + query-AST controls)
├──────────────────────────────────────────────────────────────────┤
│ {projection body — table/board/list/calendar/gallery/timeline}    │  ← one item model, six molecule renderings
└──────────────────────────────────────────────────────────────────┘
```
- **View bar** — projection switcher; filter/group/sort builders that **compose the query AST** (same AST as
  the palette and agent triggers — one query language); add/visible fields.
- **Body molecules** (different renderings of the same item model): **table-row + gridcell**, **board-card +
  column/swimlane**, **list-row**, **calendar-day-cell**, **gallery-card**, **timeline-bar**. Ref-typed cells
  are `<ReferenceChip>`s.

---

## 3. The six projections (one component, switchable for free)

| Projection | Primary interactions | Persona-default (lens) |
|---|---|---|
| **Table** | inline-edit cell, resize/reorder/add column, multi-select, group-collapse, frozen first column | engineer + PM data work |
| **Board / kanban** | drag card between status columns (optimistic), WIP limits, swimlanes, `j/k`+arrow card focus, single-key transition | **engineer default** |
| **List** | grouped/sorted/filtered rows, keyboard-nav, inline peek, row-action surface | engineer triage; mixed |
| **Calendar** | drag to reschedule (optimistic), range display, create-on-day | PM/delivery scheduling |
| **Gallery** | card grid, cover field, hover-peek | knowledge/asset browsing |
| **Timeline / Gantt / roadmap** | drag bar to move/resize range, dependency links, now-next-later lanes, zoom | **PM/exec default** |

- **Inline-edit (table/list power surface):** a cell enters edit on `Enter`/double-click/typing; commits on
  `Enter`/`Tab` (optimistic), reverts on `Escape`; `Tab`/`Shift-Tab` move to the next cell *while editing* (the
  spreadsheet contract). Field-type-aware editors (text/number/select/date/person/relation/formula) are the
  **same field-editor molecules** the editor's `/database` embed reuses.
- **Drag (board/calendar/timeline):** **optimistic** (the card moves immediately, settles on ack, honest-
  rollback on reject); **must have a keyboard equivalent** (§7) — a pointer-only board fails P3 *and* G1.

**Persona-adaptive deltas as configuration, not code (the D5 mechanism):** the engineer board and the PM
roadmap differ only by **(a)** default projection, **(b)** `density` token, **(c)** vocabulary label
(`issue`↔`work item` — bounded), **(d)** default visible fields. *Same component, four config values.*

---

## 4. Variants + parameterization variant flags

- **`density` flag (`comfortable`↔`compact`)** — **the most visible flag in this component**: sets default
  row-height / card padding / cell line-height via the token set (`--row-h`, `--space-*`); per-surface override
  stays. Engineer board = compact, PM roadmap = comfortable — *the same component*.
- **`tone` flag** — empty-state voice (§5) + reading-surface type config where a view embeds reading content;
  token-and-copy, not chrome.
- **vocabulary label** + **default fields** + **default projection** are the per-lens config deltas (§3) — a
  saved-view config, not a code branch.
- **NOT affected:** `nav`, `agentPresence`, `surfaceUnification`, `sovereigntyVisibility` (the projections are
  chrome-invariant; a cross-cell *row* carries its residency tag via the chip, not a view-level flag).
- **The falsifiable rule:** a reviewer can switch projection on the live data and see the **same rows**. If
  the roadmap and the board can't, it forked (the "two components" trap). **No `switch(direction)`.**

---

## 5. ALL states

| State | Behaviour |
|---|---|
| **Empty** | onboarding-forward per projection ("No issues in this cycle — create one `C`"; "No initiatives — add one"); **filtered-to-nothing is distinct** ("No results for these filters — clear filters"), never read as "you have no data". Voice per `tone`. Never a blank grid. |
| **Loading** | **structure-skeleton matching the projection** — ghost rows (table/list), ghost cards in columns (board), ghost bars (timeline), ghost tiles (gallery), ghost day-cells (calendar). `aria-busy` + polite live region; never a blank spinner; suppress flash <~1s; partial-load fills per-section. |
| **Error** | one quiet **system-blaming** line + retry, **scoped to the view** ("Couldn't load this view — retry"); the surrounding shell stays live (fails static). |
| **Permission-denied rows** | **never rendered** — the view is permission-pre-filtered by construction (ADR-03); a forbidden row is **absent**, not greyed; you cannot infer it exists. A whole-view no-access → the no-access card. |
| **Erased / tombstoned cell** | a cell whose value is a ref to an erased artifact renders the **tombstone chip**, never a dangling id (R-09 owns the chip). |
| **Agent-pending** | a row/cell an agent is acting on shows the agent-pending treatment (e.g. an agent-suggested transition pending approval); the proposal opens the `<AgentHitlCard>`. |
| **Degraded** | a view whose data source is unreachable shows last-known rows + "couldn't refresh" (fails static), never blanking loaded data. |
| **Stale / live-update** | a bus-pushed row change (a PR going green, an issue moving) transitions in **subtly without scroll-jump or losing the user's selection/in-progress edit**. |
| **Optimistic-pending** | the dragged card / edited cell shows a subtle pending affordance; settles or **honest-rollback** (reverse the move; one quiet line). |
| **Conflict** | two near-simultaneous edits surface the CAS resolution legibly ("this changed while you were editing — keep yours / take theirs"); the component **must not silently lose an edit**. |
| **Moved / outdated** | a ref-typed cell whose target moved/changed carries the chip's "moved"/"outdated" pill. |
| **Cross-cell** | a ref-typed cell to another residency cell → chip + residency tag, else no-access (no leak). |
| **No-results (filtered)** | distinct from empty (§ Empty above); permission-honest ("no results" never reveals hidden matches exist). |

> Storm (col 13) is N/A here — the inbox owns it. The "owns" cells for `<Views>`: **conflict** (CAS cell) and
> **optimistic-rollback** (drag/inline-edit) are its defining stress states (R-21 §2h).

---

## 6. Keyboard + ARIA model (a named G1 "hard component")

- **Composite-grid focus** — **roving tabindex** (one cell/row/card `tabindex=0`, the rest `-1`; arrows
  re-rove) OR `aria-activedescendant` on the grid container (the two PROVEN APG models). The grid is **one Tab
  stop**; arrows/Home/End/`Ctrl+Home`/`Ctrl+End` move within. React Aria primitives: **`Table`** (table/list,
  `role=grid`/`row`/`gridcell`, `treegrid` when rows expand), **`GridList`** (board cards / list),
  **`ListBox`**/**`GridList`** (gallery). The view-bar controls use **`Select`/`ComboBox`/`Menu`**.
- **No keyboard trap** — `Escape` exits cell-edit back to cell-nav; `Tab` exits the grid to the next shell
  region (the AG-Grid keyboard-trap class is the anti-pattern to avoid).
- **Keyboard drag equivalent (the load-bearing one):** focus a card → `Space`/`Enter` to **pick up** → arrows
  to move between columns/positions → `Space`/`Enter` to **drop**, `Escape` to cancel — with a **live-region
  announcement** of each move ("Moved to In Progress, position 2"). React Aria's drag-and-drop hooks provide
  the keyboard + pointer parity. A drag-only board is a G1 failure.
- **Inline-edit announces** the field name + type to AT on entry; **status never colour-alone** in cells
  (glyph + label); **live updates** use a polite live region (don't spam).
- **Cognitive walkthrough:** a PM who never learned `j/k` can still operate the board — every keyboard action
  has a visible pointer affordance (drag with mouse, click-menu to transition, click-to-edit). Keyboard-first
  is never keyboard-only (P3).
- **200% zoom / reflow + RTL** — the hardest case; the component uses **logical properties** so RTL mirrors;
  columns/cells reflow at 200%/320px; frozen columns + timeline bars survive German +35%.

---

## 7. Semantic tokens consumed

| Purpose | Token(s) |
|---|---|
| Grid surface / row dividers | `--surface`, `--surface-raised` (header/frozen col), `--border` (hairline cell/row lines) |
| Row hover / selected | `--surface-hover`; selected `--accent-weak` |
| Cell text / muted meta | `--text-primary`, `--text-muted`, `--text-subtle` |
| Status cells | `--success`/`--warning`/`--danger`/`--info` (+ subtle fills for board column tints) — **glyph + label** |
| Ref-typed cells | `<ReferenceChip>` tokens (`--c-chip-*`) |
| **Agent** (agent-suggested/pending row) | **`--agent`** / `--agent-subtle` / `--c-agent-mark` |
| Inline-edit active cell | `--focus-ring` outline (the one focus token) |
| Drag pending / drop target | `--accent-weak` drop zone; pending `--text-subtle` |
| Timeline lanes / now-marker | `--border-strong` lanes; now-marker `--accent` (identity) |
| Density | `--row-h`, `--control-h`, `--space-*` |

Compact tabular numbers (`font-variant-numeric: tabular-nums`) for counts/dates. Binds only to semantics.

---

## 7b. Icons (canonical glyphs — registry names)

From the 42-icon library ([`../04-icons/ICONS-README.md`](../04-icons/ICONS-README.md) §2;
[USAGE-MAP](../04-icons/USAGE-MAP.md) §A).

- **Projection switcher:** `roadmap` (timeline/Gantt) · `cycle` (calendar). **Gap:** board/table/list/gallery
  have **no core glyph yet** — interim text labels; a `view-*` mini-family is the largest concrete gap
  ([USAGE-MAP](../04-icons/USAGE-MAP.md) §C).
- **View-bar query controls:** `search` (filter) · `priority` (group/sort field) · `settings` (visible fields ⊞).
- **Ref-typed cells:** `<ReferenceChip>` type-icons.
- **Status cells:** `check-pass` / `check-fail` / `check-pending` · `priority` — **glyph + label**.
- **Disclosure / group collapse:** `chevron` (CSS-rotated). **Row overflow:** `kebab`. **Agent-pending row:** `agent`.

## 8. Motion (token-based, reduced-motion first-class)

- **Card-moves-column / drag-settle** — `--dur-fast` `--ease-standard`; **rollback reverses the move** so a
  failed drag looks different from a successful one (OPT-1).
- **Live row update** — `--dur-deliberate` (240) `--ease-standard`, in place, **no scroll-jump, no selection
  loss**.
- **Projection switch** — instant (`--dur-instant`) or a `--dur-fast` cross-fade; **the rows don't fly**
  (pages render, they don't animate in).
- **Group collapse/expand** — `--dur-fast`.
- **No bounce/sparkle.** **`prefers-reduced-motion`** → 0; rows/cards reposition instantly + announce.

---

## 9. Usage do / don't

**Do**
- Keep the engineer board and PM roadmap **the same component, four config values apart** (projection,
  density, vocabulary, fields). Prove it: switch projection on live data → same rows.
- Make drag **optimistic + keyboard-equivalent + announced**.
- Render forbidden rows as **absent** (pre-filtered), ref cells to gone artifacts as **tombstone chips**.
- Span subsystems via the reference graph (a board embedded in a runbook) — don't inherit Notion's one-source
  limit.

**Don't**
- Don't fork a separate "roadmap tool" (the exact Jira-for-eng / Productboard-for-PM split Myelin kills).
- Don't ship a drag-only board (G1 fail).
- Don't let a live update clobber an in-progress inline edit (conflict state).
- Don't post-filter rows (leak); don't colour-code status without a glyph + label.
- Don't `switch(direction)`; read `density`/`tone` + semantic tokens.

---

## 10. Honesty — PROVEN vs HOUSE STYLE vs deferred

- **PROVEN:** one query AST / one item model across issues + knowledge (ADR-06/07); permission-pre-filter so
  forbidden rows are absent (ADR-03); the APG grid focus models + keyboard-drag parity (G1); the shared field
  model; the chip as ref-cell content.
- **HOUSE STYLE:** the projection-switcher choreography; the keyboard pick-up grammar; the per-lens config
  deltas as the dual-audience mechanism; the drag-settle/rollback motion.
- **`[DEFERRED-UNTIL-USERS]`** (R-16 / R-10 §6 own): the **dual-audience** claim — "one component, both lenses,
  *neither degraded*" — is the largest hypothesis: does each segment find its lens first-class, or a starved
  compromise, or believe they're looking at *different objects* (the dual-product split returns)? The
  **dual-audience vocabulary** (`issue`↔`work item`) is the specific comprehension risk. Method: per-segment
  RITE on the **same** surface in **both** lenses (engineers P1–P5 vs PMs P6–P10); Phase 6 must sketch both
  lenses over the same data.

*End. Component spec HOUSE STYLE over the PROVEN §5.6 + ADR-06/07/03 + the APG grid pattern; renders R-10 §2;
ref cells are `<ReferenceChip>`s; cell-editor shared with `<BlockEditor>`. Consumes the finalist-A token set.
Not committed.*
