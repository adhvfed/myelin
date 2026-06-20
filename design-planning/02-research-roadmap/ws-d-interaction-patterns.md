# WS-D — The Interaction-Pattern Library

> Workstream D (see [`README.md`](./README.md)). Turns the shared components of design-language §5 + §8b
> into reusable, specced interaction patterns — the substrate every later phase composes from, and the
> mechanical guarantee of one-product coherence (P1). Build ON §5/§8b; do NOT re-derive them. Phase-1
> methods #2 (teardown bar), #9 (job-flow/states), #11 (atomic taxonomy), #19/#20 (heuristic/walkthrough
> self-critique), #8b (the day-one primitive mandates).

These three items deliberately split the §5 component list by *risk and reuse*: R-08 the
palette/search nerve-centre, R-09 the wedge component (the single most important component, §5.3), R-10
the rest of the shared library + the day-one overlay/editor primitives.

---

## R-08 — Command palette + search-find interaction spec

**Questions answered.** How does `Cmd/Ctrl-K` unify navigation + actions + search on *every* screen at
Linear/Notion grade? How do structured filters typed in the palette build the *same* query AST as saved
views and agent triggers (ADR-07)? How is search permission-pre-filtered (you can only find what you may
see, ADR-03)? Keyboard + pointer paths; all states.

**Phase-1 methodology.** #2 teardown (Linear/Notion palette bar); #20 cognitive walkthrough (can a new PM
discover what an engineer reaches by muscle memory?); #19 heuristics.

**Inputs.** R-01 (Linear/Notion teardown); design-language §5.2 (palette), §5.7 (search), §2.5
agent-tool symmetry (ADR-08 `ToolDef`s), ADR-07 (query AST), ADR-03 (`list-objects`); R-06 (IA the
palette navigates).

**Deliverable.** `design-planning/04-research/interaction/command-palette.md`. The full interaction spec:
modes (navigate / act / search / build-query), the query-AST surfacing (humanly), keyboard model,
result ranking, the permission-pre-filter guarantee as a UX behaviour, the human↔agent tool-catalogue
symmetry, and the state set (empty/loading/no-results/no-access/error). Plus the search *view* (facets,
type/subsystem scoping, multilingual) as the palette's heavyweight sibling.

**Sequencing & dependencies.** Seq #8. Depends on R-01, R-06. Feeds Phase 6 (every finalist's wedge
moment) and the rubric D1.

**User-dependency.** none.

**Effort.** M.

**Acceptance criteria.** Palette unifies nav+actions+search with one query AST; permission-pre-filter is
specified as UX (graceful, never leaks); keyboard model is complete and the new-user discoverability path
is walked (#20); all states enumerated; human/agent tool symmetry shown.

---

## R-09 — Reference chip + artifact unfurl interaction spec (the wedge component)

**Questions answered.** How do the inline **reference chip** and the rich **unfurl card** render an
`ArtifactRef` everywhere content lives — live (not snapshot), permission-aware per viewer, tombstoning
gracefully — with inline actions where permitted? This is "the most important shared component in the
platform" (§5.3) and the literal embodiment of the wedge.

**Phase-1 methodology.** #2 teardown (Slack unfurl bar); #9 job-flow (the full state set is the point);
#8b (live-projection, humanised strings).

**Inputs.** R-01 (Slack/GitHub unfurl teardown); design-language §5.3 (the hard rules: live /
permission-aware / tombstones), §5.5 (mentions as ref chips), P6 (wedge); ADR-13/ADR-03/ADR-12 (the
platform-law behind the component); reference-graph architecture (`05-refined/.../reference-graph.md` —
the projection cache, the 4-step tombstone ladder, content-anchored line-ranges); R-04 (the flows the
chip threads).

**Deliverable.** `design-planning/04-research/interaction/reference-unfurl.md`. The chip + unfurl spec:
both forms (compact chip; rich card per artifact type — PR/issue/doc/run/thread); the inline-action
surface (re-run job, transition issue, approve PR — where permitted); and **every state**: live, peeking
(hover), no-access (graceful card, never a leaked title), moved/outdated, **tombstoned/erased**,
cross-cell-resolves-to-projection-or-tombstone, and the diff-line-anchored chip that relocates/orphans
after rebase. Map to the existing reference-graph resolver behaviour (do not redesign the backend; surface
it).

**Sequencing & dependencies.** Seq #9. Depends on R-01, R-06. Feeds R-22 (wedge moments), Phase 6, and
the completeness-critic (this component owns several unglamorous states).

**User-dependency.** none.

**Effort.** L.

**Acceptance criteria.** Both forms specced per artifact type; inline actions specified with permission
behaviour; **all** states present incl. no-access, tombstoned, moved/outdated, cross-cell, and
rebase-orphaned; live-not-snapshot default shown; humanised strings (no raw ids); maps onto the existing
reference-graph resolver rather than inventing a new one.

---

## R-10 — Shared interaction patterns: views, editor, notifications inbox, overlays

**Questions answered.** How do the remaining shared components behave as one library: the
tables/boards/views component (one component, many projections — the §5.6 issues↔knowledge reuse
boundary); the block editor (one render path, one AST — §8b.2); the notifications inbox ("what needs
*me*", deduped, "why it fired"); and the day-one **overlay primitives** (Dialog/Confirm/Popover/Dropdown/
Tooltip/Toast — §8b.1, build-first)? What is the atomic taxonomy that makes reuse visible?

**Phase-1 methodology.** #11 atomic design (loose taxonomy over the §5 inventory); #8b.1/§8b.2 (the
day-one overlay + editor mandates); #19 heuristics.

**Inputs.** design-language §5.6 (views), §5.9 (editor), §5.8 (inbox), §8b.1 (overlays), §8b.2 (editor
render path); R-01 (Notion views/editor + Slack/Linear inbox teardown); ADR-05/06/07; notifications
architecture (`05-refined/.../notifications.md` — dedup, `origin_event`+`reason` "why fired",
read-state); R-06 (IA).

**Deliverable.** `design-planning/04-research/interaction/shared-patterns.md`. Per component: the
interaction spec + state set + the atomic-taxonomy placement (atom/molecule/organism, anchored to the §5
list). Must cover: **views component** (table/board/calendar/list/gallery/timeline as projections of one
query AST; persona-adaptive per §2; keyboard nav; inline-edit; drag); **editor** (block model, slash
menu, mention/ref nodes, the *one render path* + round-trip gate as a design constraint, controlled
contenteditable); **notifications inbox** (prioritised, deduped, "why am I getting this" provenance from
`origin_event`+`reason`, one-action triage, one read-state truth across views, calm-by-default, agent
volume out of the main stream); **overlay primitives** (portal-always, one z-index scale, focus-trap/
return-focus/scroll-lock/Escape/ARIA centralised, single-purpose-by-shape).

**Sequencing & dependencies.** Seq #10. Depends on R-01, R-06. Feeds R-16 (views component is the
dual-audience mechanism), R-21 (states), and Phase 6.

**User-dependency.** none.

**Effort.** L.

**Acceptance criteria.** All four component families specced with state sets; the views component shown
as the issues↔knowledge reuse boundary and the dual-audience mechanism; the editor's one-render-path +
round-trip constraint stated as binding; the inbox surfaces "why it fired" from the existing
`origin_event`+`reason` (not a new mechanism); the overlay primitives carry the §8b.1 mandates verbatim
as design rules; atomic taxonomy makes cross-component reuse visible for Phase-7 coherence scoring.
