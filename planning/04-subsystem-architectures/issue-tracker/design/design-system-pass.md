# Issue Tracker — Design-system pass (the pre-frontend visual/token-level pass)

> The **ISS-P16 / P-382** design-system pass: a visual/token-level pass over the preserved structural
> sketch (`information-architecture.md`, `user-flows.md`, `wireframes.md`) for the **co-equal views**
> (board / roadmap / backlog / list / table / calendar / cycle), **including the empty / loading /
> error / permission / tombstone states**. Per VISION §3 ("no frontend code without a reviewed design
> sketch behind it") this is a **design sketch, not frontend code** — no contract, no Rust UI. The
> frontend lands in the M6 surface prompts (ISS-P33+).
>
> **Date: 2026-06-23.** Conforms to the frozen design system
> (`design-planning/08-design-system/`): the **`<Views>` component spec**
> ([`02-components/views.md`](../../../../design-planning/08-design-system/02-components/views.md)) is
> the ONE shared visual primitive these seven views render through; the semantic tokens are the finalist-A
> set ([`01-tokens/tokens.css`](../../../../design-planning/08-design-system/01-tokens/)); the glyphs are
> the 42-icon library ([`04-icons/`](../../../../design-planning/08-design-system/04-icons/)).
>
> **Keyed to the live contract:** the views are co-equal `myelin-query` `ViewSpec` projections over the
> ONE `issue` table — the SHAPE already declared in `myelin_issues::views` (this prompt). The eventual
> frontend renders the **real projection** (the seven `IssueView::spec()` `ViewSpec`s + the leak-free
> `IssueView::plan()` executor seam), not a parallel visual vocabulary.

---

## 0. The structural bet this pass makes visible (the falsifiable rule)

The make-or-break UX bet (hard-problems §1): **the board and the roadmap are not two tools — they are two
`ViewSpec`s over the SAME rows** (the denormalised `type_rank` split: board `≤ 1`, roadmap `≥ 2`).
Editing an issue on the board patches the roadmap live because there is **one `issue` table, no parallel
reality** (proven by row id in the ISS-D1 drill, `integration_iss_p16_coequal_views.rs`).

**The visual pass MUST NOT betray this structurally**: there is **ONE `<Views>` organism** rendering all
seven projections — board/roadmap/backlog/list/table/calendar/cycle are **chrome-invariant renderings of
one item model** (views-spec §4). A reviewer can switch projection on live data and see the **same rows**;
if the roadmap and the board can't, it forked (the "two components" trap). **No `switch(direction)`** — the
projections read `density`/`tone` + semantic tokens, never a per-tool theme.

---

## 1. The token map per view (semantic tokens, never inline colour)

Every surface binds **semantic tokens** (views-spec §7), never a literal colour. Status is **glyph +
label**, never colour-alone (P-craft; WCAG 1.4.1).

| Surface | Tokens |
|---|---|
| Grid / board / list surface | `--surface`; header + frozen first column `--surface-raised`; hairline row/cell/column lines `--border` |
| Row/card hover · selected | `--surface-hover`; selected `--accent-weak` |
| Cell / card text · muted meta | `--text-primary`; secondary `--text-muted`; subtle `--text-subtle` |
| Status (state_category / SLA / WIP) | `--success` / `--warning` / `--danger` / `--info` + subtle board-column tints — **always glyph + label** (`check-pass`/`check-fail`/`check-pending`/`priority`) |
| Ref-typed cells (assignee, linked PR, parent) | `<ReferenceChip>` tokens (`--c-chip-*`) — never a raw id |
| Agent-pending row/card (triage suggestion, agent-moved card) | `--agent` / `--agent-subtle` / `--c-agent-mark` — the `agent` treatment, **no magic-wand** |
| Inline-edit active cell | `--focus-ring` (the ONE focus token — distinct from `--accent`/identity) |
| Drag pending · drop target | drop zone `--accent-weak`; the pending card `--text-subtle` |
| Timeline (roadmap) lanes · now-marker | lanes `--border-strong`; now-marker `--accent` |
| Density | `--row-h` / `--control-h` / `--space-*` (the `comfortable`↔`compact` flag) |

Counts/dates use `font-variant-numeric: tabular-nums`. **Focus-token ≠ identity-token** (the focus ring is
`--focus-ring`, never the brand `--accent`).

---

## 2. Type / spacing / density (the dual-audience mechanism — config, not code)

The engineer board and the PM roadmap differ ONLY by **four config values** (views-spec §3): (a) default
projection, (b) `density` token, (c) vocabulary label (`issue`↔`work item` — bounded), (d) default visible
fields. **Same component, four config values** — the D5 mechanism. Concretely:

- **Engineer (board / backlog / list / cycle):** `density = compact` → tight `--row-h` / `--space-*`,
  keyboard-forward (`j/k`, single-key transition). Default fields: title + assignee + priority.
- **PM/exec (roadmap / calendar / table):** `density = comfortable` → spacious `--row-h`, chart-forward.
  Default fields per lens (roadmap: earliest_start + latest_due; calendar: due).

Type scale + the 4px spacing ramp come from the one token scale; **no per-view font/spacing literals**.
German +35% / 200% zoom / 320px reflow survive via **logical properties** (RTL mirrors; frozen columns +
timeline bars reflow) — the hardest case (views-spec §6).

---

## 3. The icon → meaning map (the 42-icon library; glyph + label)

- **Projection switcher:** `roadmap` (timeline) · `cycle` (calendar). **Gap (named floor):**
  board/table/list/gallery have **no core glyph** yet — interim text labels until the `view-*` mini-family
  lands (`USAGE-MAP` §C).
- **View-bar query controls:** `search` (filter) · `priority` (group/sort field) · `settings` (visible
  fields ⊞) — these compose the **ONE query AST** (same AST as ⌘K and agent triggers, ADR-07).
- **Status cells:** `check-pass` / `check-fail` / `check-pending` · `priority` — **glyph + label**.
- **Disclosure / group collapse:** `chevron` (CSS-rotated). **Row overflow:** `kebab`. **Agent-pending
  row:** `agent`.
- **Ref-typed cells:** `<ReferenceChip>` type-icons (assignee = person, linked PR = git, parent = issue).

---

## 4. The seven views — the visual pass per screen (keyed to `IssueView`)

Each view is `IssueView::spec()` (a frozen `ViewSpec`) rendered by `<Views>`; each conjoins the leak-free
ACL `Filter` via `IssueView::plan()` (forbidden rows **absent**, never greyed — §5 permission state).

| View | Projection (`kind`) | Filter slice (the `ViewSpec` over one table) | Group / sort | Density default |
|---|---|---|---|---|
| **Board** (S3) | `board` | `type_rank ≤ 1` | group `state_category`, sort `order_key` | compact |
| **Roadmap** (S5) | `timeline` | `type_rank ≥ 2` (the CO-EQUAL lens) | date axis (`earliest_start`), dependency overlay | comfortable |
| **Backlog** (S6) | `list` | `state_category == unstarted` | drag-rank (`order_key` + CAS) | compact |
| **List** (S2) | `list` | any `QueryAst` | sort + `order_key` tiebreak | compact |
| **Table** (S4) | `table` | any `QueryAst` | inline-edit, visible/hidden fields | comfortable |
| **Calendar** (S7) | `calendar` | cycle/date membership (bound `cycle_id`) | group by a date field | comfortable |
| **Cycle** (S8) | `board` | cycle membership (bound `cycle_id`) | group `state_category` | compact |

**Board↔roadmap co-equality made visible:** the board renders a card per row in a `state_category` column;
the roadmap renders the SAME rows (those with `type_rank ≥ 2`) as timeline bars on a date axis. A
`type_rank` edit on the board (promote a story → epic) **moves the same card off the board and onto the
roadmap** with a `--dur-fast` reposition — *not* a delete-and-create (it is the SAME row id). This is the
visual proof of the structural bet (the falsifiable-rule reviewer test).

---

## 5. ALL states (the §5.10 / craft-R21 matrix — the pre-frontend gate's required coverage)

Per the prompt's DoD, the pass covers **empty / loading / error / permission / tombstone** (and the
`<Views>` "owns" stress states: conflict + optimistic-rollback). Each is a **designed state, never a blank
or a spinner**.

| State | Treatment (per the `<Views>` spec §5 + the wireframes' non-happy states) |
|---|---|
| **Empty (no data)** | onboarding-forward, per projection + voice (`tone`): board "No issues in this cycle yet — [Create issue] or [Plan from backlog]"; roadmap "No initiatives or epics yet — [Create initiative]" + a hint "same data as the board, one lens"; backlog "Backlog is empty — [Create issue]"; triage "Triage is clear ✦" (calm). **Never a blank grid.** |
| **Empty (filtered-to-nothing)** | **DISTINCT from no-data**: "No results for these filters — [Clear filters]" — never read as "you have no data", and **permission-honest** (never reveals hidden matches exist). |
| **Loading** | **structure-skeleton matching the projection**: ghost cards in columns (board/cycle), ghost bars at row positions (roadmap), ghost rows (backlog/list/table), ghost day-cells (calendar). `aria-busy` + polite live region; **suppress flash < ~1s**; partial-load fills per-section (a single failing board column fails static, the rest render). **Never a blank spinner.** |
| **Error** | one quiet **system-blaming** line + retry, **scoped to the view**: "Couldn't load this view — [Retry]"; the surrounding shell stays live (fails static). Per-section: "Couldn't load 'Overdue' — [Retry]" while the rest shows. Roadmap rollup lag shows "● progress updating…" on the bar rather than a wrong number. |
| **Permission-denied rows** | **NEVER rendered** — the view is permission-pre-filtered by construction (`IssueView::plan` conjoins the leak-free `Filter`, 4.3/ADR-03). A forbidden row is **absent**, not greyed; **no "N hidden" leak** — you cannot infer it exists. A whole-view no-access → the no-access card (not a partial leak). |
| **Erased / tombstoned cell** | a ref-typed cell pointing at an erased artifact renders the **tombstone chip** ("This content was erased" — structure preserved, identity gone; ADR-12), **never a dangling id**. The row's own structure survives the tombstone (the issue stays; the erased ref is the chip). |
| **Agent-pending** | a row/card an agent is acting on (a triage suggestion, an agent-proposed transition) shows the `agent` treatment (glyph + label, no magic-wand); the proposal opens the `<AgentHitlCard>` — **proposed, never auto-applied** (P-agent §6.2). |
| **Optimistic-pending** | a dragged card / edited cell shows a subtle pending affordance (`--text-subtle`); settles on ack or **honest-rollback** (`--dur-fast` reverse of the move + one quiet line "reordered by someone else — your change was re-applied below"). A failed drag looks **different** from a success (OPT-1). |
| **Conflict (CAS)** | two near-simultaneous edits surface the CAS resolution legibly ("this changed while you were editing — keep yours / take theirs"); the component **must not silently lose an edit** (the board reorder + the inline-edit CAS, ISS-P09). |
| **Stale / live-update** | a bus-pushed change (a PR going green, an issue moving lenses) transitions in **subtly, no scroll-jump, no selection/in-progress-edit loss** (`--dur-deliberate`, in place). |
| **Cross-cell** | a ref-typed cell to another residency cell → chip + residency tag (or no-access — no leak). The cross-cell **portfolio rollup view is a named floor**, ISS-P32. |
| **WIP-limit / degraded** | board column header shows "●limit 2/2" (glyph + count, not colour-only); a degraded source shows last-known rows + "couldn't refresh" (fails static, never blanks loaded data). |

---

## 6. The a11y constraints the eventual value-table must clear (the floors the frontend proves)

- **Composite-grid focus** — roving tabindex OR `aria-activedescendant`; the grid is ONE Tab stop;
  arrows/Home/End move within (React Aria `Table`/`GridList`). **No keyboard trap** (`Escape` exits edit;
  `Tab` exits the grid).
- **Keyboard drag equivalent (the load-bearing G1)** — focus card → `Space` pick up → arrows move → `Space`
  drop, `Escape` cancel, with a **live-region announcement** ("Moved to In Progress, position 2"). A
  drag-only board is a **G1 failure** — the board/roadmap/calendar drag MUST have the keyboard equivalent.
- **Status never colour-alone** in cells (glyph + label); **live updates** use a polite live region (no
  spam); inline-edit announces the field name + type on entry.
- **WCAG 2.2 AA measured** contrast on the concrete token values (the value-table is the frontend floor);
  EU-multilingual + RTL via logical properties; 200% zoom / 320px reflow.

---

## 7. Named floors (this is a pass + sign-off, not built UI)

- **The concrete token-value table + the live styleguide + the measured-contrast / round-trip-editor /
  keyboard-drag-parity gates** land with the **frontend foundation (ISS-P33+)** — this pass is the reviewed
  build-to, not the built UI; the frontend done-bar (the switch test) applies there.
- **The `view-*` icon mini-family** (board/table/list/gallery glyphs) is the largest concrete icon gap
  (`USAGE-MAP` §C) — interim text labels until it lands.
- **The cross-cell portfolio rollup view** (an exec portfolio spanning residency cells) is the M5 follow-on
  **ISS-P32** (the `CrossCellPointer` bridge) — a view's rows stay within one residency cell here.
- **The real-time board sync** (the firehose resume-cursor protocol; the live-update / presence state above
  is the visual surface) is **ISS-P30 (P-397)**.

*End. A visual/token-level pass over the PRESERVED structural sketch, conforming to the frozen `<Views>`
component spec + the finalist-A token set + the 42-icon library. No frontend code; the frontend lands in
ISS-P33+. Sign-off: [`signoff.md`](./signoff.md).*
