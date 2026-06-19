# Sketch 01 — The issue-model duality: board ↔ roadmap as co-equal views; Epic/Initiative as type-vs-level (PR-2)

> Exploration note. Weighs the **make-or-break UX bet** (Phase-2 §11 Q1; deep-dive §3.2/§3.5; PR-2). This is
> the decision that, gotten wrong, forces PMs into a parallel reality and re-creates the "engineering tool vs
> management tool" split the platform exists to kill (design-language §2). Status: this sketch *leans*; the
> commit is in `00-findings.md`.

## The question, sharpened

Two coupled forks:

1. **Is "Epic"/"Initiative" a *type* of issue, or a *level* in a containment hierarchy?** Jira makes Epic a
   *type* (an issue with `issuetype=Epic`); Linear makes Project/Initiative *separate objects* with their own
   schema, distinct from issues.
2. **Are the board (engineer lens) and the roadmap (PM lens) two views over one object graph, or two object
   graphs?** VISION §2 + design-language §2 demand they be **co-equal views over one schema** — "the lens is a
   view, not a fork." But co-equal-views is a *requirement*, not yet a *data model*; the model must make it true.

These are coupled: if Epic/Initiative is a *type*, then the roadmap is "a view filtered to `type ∈ {epic,
initiative}` arranged on a time axis," and board↔roadmap co-equality is nearly free. If Epic/Initiative is a
*separate object*, the roadmap reads a different table than the board, and we must engineer a join to keep them
co-equal.

## Candidate A — Everything is an issue; Epic/Initiative is a *type*; level is *derived* from the type's hierarchy rule (Jira-leaning, but fixed)

One `issue` table. `type` is configuration (a row in a `type_scheme`). Each type declares a **hierarchy rank**
(an integer "level": sub-task=0, story=1, epic=2, initiative=3) and which ranks it may parent. Containment is the
`issue_relation` `parent` edge (the TE-7 typed table — REF-1/ISS-1). The roadmap is `view(type_rank ≥ 2)` laid
on a date axis; the board is `view(type_rank ≤ 1)` grouped by state. **One object, one table, one ref-edge for
hierarchy.**

- **For:** Board↔roadmap co-equality is *structural* — both are `myelin-query` AST views (ADR-06/07) over the
  same `issue` rows, differing only in filter + layout (table/board vs timeline). This is exactly the
  design-language §2 "same data, different lens" made literal. One human-key scheme (TE-14), one permission
  object type (`issue` in identity §5 — already seeded), one rollup engine, one import target. An initiative
  spanning teams is just an issue whose `parent` children live in many projects (deep-dive §3.8 — the
  cross-team-portfolio case) — no special object.
- **For:** Matches the seeded ReBAC namespace: identity §5 already declares `definition issue { … }` with no
  separate `epic`/`initiative` type. Candidate A needs **zero** new authz object types.
- **For (agent-native):** rollup-drift, forecast, and health agents (deep-dive §7.3) operate on `issue` rows
  uniformly — an initiative's health is the same rollup math as an epic's, just at a higher rank.
- **Against:** Jira's overloading of "Epic" is a known UX smell (deep-dive §3.6) — Epic-as-type historically
  meant Epic couldn't *also* be in a sprint, needed bespoke "Epic Link" fields, and confused users. We must
  avoid re-importing that confusion: the fix is that **rank is a clean ordering and `parent` is the one
  containment edge**, not a parallel "Epic Link."
- **Against:** a *pure* free-rank model (any type parents any type) re-creates Jira's "infinitely configurable
  therefore slow" trap (deep-dive §0). Needs guardrails (rank-monotonic parenting by default).

## Candidate B — Issues and Planning-objects are *separate* tables (Linear-leaning)

`issue` is the leaf/story object; `project`, `epic`, `initiative` are **distinct objects** with their own
tables, their own (smaller) schema, their own lifecycle. The roadmap reads `initiative`/`project`; the board
reads `issue`. Membership is FK (`issue.project_id`) + a separate `initiative_project` join.

- **For:** Clean separation — a Project/Initiative genuinely *is* a different thing (it has a target date, an
  OKR link, a health, not a "state machine workflow"); modelling it as an issue forces awkward nulls.
- **For:** Linear's model is loved precisely because Projects/Cycles/Initiatives are first-class, not "issues
  wearing a costume."
- **Against (the killer):** board↔roadmap **co-equality is now an engineering burden, not a structural
  guarantee.** Two tables → two human-key schemes (or none for planning objects) → two permission object types
  (identity §5 would need `definition epic`, `definition initiative`) → two rollup paths → two import targets →
  the roadmap and the board can *drift*. Every cross-cutting query ("show me everything blocking this
  initiative across its child issues") becomes a multi-table join. This is the **parallel-reality risk**
  re-introduced at the schema layer.
- **Against:** the shared `myelin-query` view primitive (ADR-06) is built to project *one* collection many
  ways; two collections means the "one component, many projections" promise (design-language §5.6) needs a
  union view — more surface, more drift.

## Candidate C — Hybrid: one `issue` table for the *work* spine (sub-task→story→epic→initiative as ranked types, Candidate A), BUT `cycle` and `project`/`space` are genuinely separate axis-objects (not issues)

The insight from deep-dive §3.6: **Cycle (time axis) and Project/Epic/Initiative (scope axis) are distinct
axes** — an issue can be in cycle *N* and project *X* at once. So:

- **Scope/containment spine = one `issue` table, ranked types** (Candidate A): sub-task, story/bug/task, epic,
  initiative are all `issue` rows; `parent` is the one containment edge; rank governs valid parenting. This is
  what the roadmap and the board both read — **co-equal views over one table, structurally**.
- **Cycle = a separate `cycle` object** (a time-box with start/end/capacity), and `issue ∈ cycle` is a
  *membership edge*, not containment. A cycle is not an issue (it has no workflow state, no assignee); modelling
  it as one would be the awkward-nulls problem of Candidate B. Cycle membership is a typed relation
  (`added_to_cycle`) feeding burndown.
- **Project/Space** is the *org-scope* object that already exists in the **identity** namespace (`definition
  project` in identity §5) — Issues does **not** re-invent it (deep-dive §3.8; Knowledge does the same with its
  `space`→`project` mapping, knowledge 01 §2.2). An issue's `parent_project` is its authz scope.

So: **Initiative/Epic = ranked issue types (one table); Cycle = separate time-axis object; Project = the
identity scope object.** This separates the *three axes deep-dive §3.6 says must stay separate* (containment /
time / org-scope) onto the *right* primitives, while keeping the containment spine (where board↔roadmap
co-equality lives) as **one table**.

- **For:** Keeps Candidate A's structural board↔roadmap co-equality (the whole spine is one `issue` table) AND
  Linear's clean separation of the genuinely-different axis-objects (cycle, project). Best of both.
- **For:** Each axis lands on the primitive that already owns it — containment on `issue_relation` (TE-7),
  org-scope on the identity `project`, time on a small `cycle` table — **no axis is forced onto the wrong
  shape.**
- **For:** matches every seeded contract (identity §5 `issue`, the `issue.epic`/`issue.sprint` ref types in
  event-bus §6.2 — note the token table already lists `epic` and `sprint` as `issue` *types*, confirming
  epic-as-issue-type and sprint/cycle as a sibling type).
- **Against:** "Epic is a type but Cycle is an object" is a subtle distinction to teach — mitigated because
  users never see the schema; they see "an epic contains stories" (containment) and "a cycle holds this
  sprint's issues" (time-box), which are *already* their mental model (deep-dive §3.6).

## The event-bus token table settles part of it

event-bus §6.2 froze the `issue` subsystem's representative `<type>` tokens as: **`issue`, `epic`, `sprint`,
`field`, `comment`, `relation`**. This is a Phase-3 contract. It says:
- `epic` is a **type under the `issue` subsystem** (`myelin://t/issue/epic/…`) → confirms **epic-as-issue-type**
  (Candidate A/C), not a separate subsystem object. ArtifactRef-wise an epic *is* an issue-family artifact.
- `sprint` is a sibling type token → a cycle/sprint is addressable but is its own type.
- There is **no `initiative` token** in the seed — but §6.2 says "each subsystem owns its complete list under
  this grammar," so Issues adds `initiative` as another ranked `issue`-family type in P4. (Findings will add it.)

This is decisive: the **frozen ArtifactRef grammar already commits us to epic-as-type**, which rules out pure
Candidate B and points at **C** (ranked issue types for the containment spine + separate cycle object).

## Leaning

**Candidate C.** One `issue` table carrying the ranked-type containment spine (sub-task → story/bug/task/chore/
spike → epic → initiative), `parent` as the single containment edge in `issue_relation` (TE-7 source of truth),
**board and roadmap as co-equal `myelin-query` AST views over that one table** (board = rank≤1 grouped by state;
roadmap = rank≥2 on a date axis); `cycle`/`sprint` as a separate small time-axis object with a membership edge;
`project`/`space` reused from the identity namespace, never re-invented. Rank is configuration (per
`type_scheme`) with **rank-monotonic parenting as the default guardrail** (an epic may parent stories, not vice
versa) to avoid Jira's free-config sprawl, overridable per scheme for the orgs that need DAG-ish portfolios.

This makes the platform-defining bet — board↔roadmap co-equality — **a property of the schema**, not a feature
we maintain. The roadmap cannot drift from the board because they read the same rows.

## What this hands forward (to findings / architecture)

- The exact `type_scheme` shape and the rank/parenting-rule encoding (governance — sketch 02).
- Whether parenting is a strict tree or a constrained DAG (cross-team initiatives spanning projects: deep-dive
  §3.5 — leaning **tree for `parent`, DAG for lateral `relates`/`depends_on`**, with rollup cycle-detection).
- Adding the `initiative` type token to the Issues taxonomy (event-bus §6.2 extension).
