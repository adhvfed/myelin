# R-16 — Dual-/Tri-Audience Persona-Adaptive Design Study ("one component, many lenses")

> **Phase 4 research corpus** · deliverable of prompt **R-16** (workstream
> [`ws-g-dual-audience.md`](../../02-research-roadmap/ws-g-dual-audience.md), Seq #12, parallel with R-15).
> **File date: 2026-06-20.** No real users exist; personas P1–P15 are HYPOTHESES
> ([`personas.md`](../../../planning/01-research/personas.md) §0). Method **#18 dual-audience /
> persona-adaptive** ("one component, many lenses") with **#1 JTBD** supplying the jobs, and **#19
> heuristics + #20 cognitive walkthrough** as the per-lens critique lens.
>
> **What this file is.** The deepening of the corpus's central design problem — *"the single hardest UX
> mandate in Myelin"* (design-language §2). For each dual-audience surface it: (a) names the two/three
> jobs over the **same data**, (b) fixes the **one component**, (c) expresses the role/density/vocabulary
> deltas as **configuration, not separate code**, and (d) **critiques each lens against its persona** to
> prove neither is a degraded compromise (the §2 "serves-neither" trap). It bounds the persona-adaptive
> vocabulary fracturing-risk (design-language §9 open question) and records the deferred both-audience
> validation as an executable plan.
>
> **Builds ON prior `04-research` (does NOT duplicate):**
> - [R-03 jtbd-catalogue](../jtbd-flows/jtbd-catalogue.md) §5 — the named same-data pairs **D1–D5**. This
>   file *deepens each pair*; it does not re-name them.
> - [R-05 persona-pressure-test](../jtbd-flows/persona-pressure-test.md) §2 (conflict matrix **K1–K7**),
>   §4 (validation rank **1** = K1), §3 (the **archetype-cut risk** carried to R-16). The pressure-test
>   tells this file *which persona to critique each lens against* (the least-confident assumption owner).
> - [R-10 shared-patterns](../interaction/shared-patterns.md) §2 — the **views component** spec (the
>   literal mechanism: one organism, six projections of one query AST; §2.1 the four config deltas; §2.5
>   the two-components trap). This file *applies* that component to three surfaces; it does not re-spec it.
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited standard/source, an existing platform
> contract we *surface* (design-language §2/§5.6/§3.4, ADR-06/07/03), or a measured external finding.
> **HOUSE STYLE** = our design synthesis/taste. **HYPOTHESIS** = a specific instantiation no user has
> validated (inherited from R-03/R-05). No part is user-validated; the deferred validation is §6.

---

## 0. How to read this file

The discipline is **method #18**, four steps, applied per surface (§2 issue views, §3 knowledge
databases, §4 dashboards):

> **(a) the jobs over the same data → (b) the one component → (c) the deltas as configuration → (d) the
> per-lens critique against the persona.**

Then: **§1** the model (one component, many lenses — stated once, reused per surface); **§5** the
vocabulary-mapping proposal with fracturing-risk bounded; **§6** the `[DEFERRED-UNTIL-USERS]`
both-audience validation plan + the Phase-6 both-lens mandate; **§7** completeness-critic; **§8** rubric/
funnel actionability; **§9** sources; **§10** self-check.

**The one-line thesis (HOUSE STYLE).** *A lens is a small set of declared configuration values over one
component and one record set — never a fork. The dual-audience product succeeds exactly when a reviewer
can switch the lens on live data and watch the **same rows** re-present, and fails the moment either
audience would be better served by a second tool. Each lens must be **excellent for its persona, not an
average of all personas** — averaging is the §2 "serves-neither" trap.*

---

## 1. The model — "one component, many lenses" (stated once)

### 1.1 What a "lens" is (the config-not-fork contract)

A **lens** is a *named bundle of configuration* applied to **one component over one record set**. It is
**not** new code, not a new data model, not a new component. The bundle has a **fixed, finite shape** so
it stays auditable and CI-checkable (the falsifiable D5 rule, R-10 §2.5):

| Config axis | Values | Source / token |
|---|---|---|
| **1. Projection** | table / board / list / calendar / gallery / timeline (views); chart-type (dashboards); database-view (knowledge) | one query AST → projection-type (PROVEN — ADR-06/07; R-10 §2.1) |
| **2. Density** | `comfortable` (default) ↔ `compact` | density-token set (PROVEN — design-language §3.4; P5) |
| **3. Vocabulary** | label map per term ("issue"↔"work item"↔"deliverable") | a presentation token map, **not** a schema fork (PROVEN — §2; §5 of this file bounds it) |
| **4. Visible fields + grouping + sort + filter** | which columns/lanes/series show by default | a saved query AST (PROVEN — ADR-06; first-class permissioned object, §5.6) |
| **5. Default-landing + chrome** | which lens a role lands on; pointer-affordance prominence | per-role default (PROVEN — §2 "default lens by role, switchable by anyone"; R-06 default-landing map) |

**The invariant (HOUSE STYLE, the D5 mechanism):** *two lenses of the same surface differ ONLY by values
on axes 1–5. If a difference cannot be expressed as a value on one of these five axes, it is a fork — and
a fork is the failure.* This is the mechanical, falsifiable form of design-language §2's "build one
component over shared primitives and adapt presentation by role/density — never split the product."

### 1.2 Why configuration-not-fork is the right bet (grounded)

- **PROVEN (external):** role-based UI personalization — *system-driven adaptation* of density and
  default tool-access by role — improves usability and reduces confusion for multi-role single-interface
  products; the literature distinguishes **adaptation** (system-driven, our role-default) from
  **customization** (user-driven, our "switchable by anyone") and recommends both
  ([Okoone, "How adaptive UI can make every user experience smarter", 2025](https://www.okoone.com/spark/product-design-research/how-adaptive-ui-can-make-every-user-experience-smarter/);
  [UXmatters, "UX Design for Personalization"](https://www.uxmatters.com/mt/archives/2018/07/ux-design-for-personalization.php)).
- **PROVEN (external):** **progressive disclosure** lets one interface serve novice and expert at once —
  deferring advanced/role-gated features to secondary layers makes initial tasks **30–50% faster** while
  preserving the full feature set; NN/g's three-layer model (core / advanced-on-request / role-or-intent-
  gated expert) is exactly how a PM lens can be calm without starving the engineer lens of depth
  ([NN/g, "Progressive Disclosure"](https://www.nngroup.com/articles/progressive-disclosure/)).
- **PROVEN (internal contract):** the views component already *is* one organism, six projections of one
  query AST, used by issues AND knowledge (design-language §5.6; ADR-06; R-10 §2). The mechanism exists;
  this file proves it serves the *audiences*, not just the *subsystems*.
- **HOUSE STYLE (the bet):** the five-axis config bundle is *enough* expressiveness to make each lens
  excellent. **This is the load-bearing hypothesis** (R-05 B-b: "the PM-friendly lens can be a
  configuration of the shared views component, not a separate product"; confidence **L**). §6 is its test.

> **Carried risk from R-05 §3 (the archetype-cut).** R-16 frames lenses by **role** (engineer / PM /
> exec). R-05 flags that the more salient cut may be **archetype** (startup-simple ↔ enterprise-deep). The
> five-axis bundle is **cut-agnostic**: axis-5 (default-landing/chrome) + axis-2 (density) already express
> a startup↔enterprise lens (sensible-invisible defaults vs. configured depth, K6/P4). So if the deferred
> study shows archetype dominates, the model holds — only the *default-assignment table* changes, not the
> component. **(HOUSE STYLE; flagged honestly.)**

### 1.3 The two failure modes this model must avoid (both falsifiable)

1. **The fork** ("two components") — under deadline the PM roadmap becomes a separate "roadmap tool",
   re-creating the Jira-for-engineers / Productboard-for-PMs split Myelin exists to kill (§2; R-02; R-10
   §2.5a). **Test:** switch projection on live data → same rows, or it forked.
2. **The averaged compromise** ("serves neither") — one component tuned to a *middle* that is dense enough
   to scare the PM and soft enough to slow the engineer (§2; R-05 K1; personas.md line 709: "presents as
   both a PM roadmap surface and an engineering board without satisfying neither"). **Test:** §2.4/§3.4/
   §4.4 per-lens critique — each lens must be *excellent for its persona*, not an average. **A lens that
   is only "acceptable" to its persona has already failed.**

---

## 2. Surface 1 — Issue views (the **D1** pair; the central case)

The make-or-break surface (R-03 D1; R-05 **rank 1** K1; the most-cited open question). Below is the full
#18 four-step treatment.

### 2.1 (a) The jobs over the same data

**Shared data = the issue model (one schema, one record set; ADR-06).** Three jobs, three audiences:

| Lens | Persona | Job (from R-03) | Felt job (emotional/social) |
|---|---|---|---|
| **L1 Engineer board** | **P1** backend (also P2/P3) | **D1-eng** ≈ E3/E4: *"track this cycle on a keyboard-driven board so I can ship the sprint"* | flow / competence; *don't make me leave the page or touch the mouse* |
| **L2 PM roadmap** | **P6** PM | **M1**: *"show a roadmap/now-next-later that reflects real delivery from the same data so I can report honestly"* | trust the picture / no parallel reality; approachable, narrative |
| **L3 Exec rollup** | **P11** CTO/VP | **G1**: *"see a portfolio rollup so I can judge org throughput & cost"* | defensible at-a-glance; ground-truth, no vanity metric |

### 2.2 (b) The one component

**The views component** (design-language §5.6; R-10 §2). L1 = **board** projection; L2 = **timeline/
roadmap** projection; L3 = **a grouped table/timeline rolled up by team/initiative** (a saved view with
aggregate fields). All three are projections of **one query AST over the same issue rows** (PROVEN —
ADR-06/07). Switching is free and non-navigational (R-10 §2.1; R-01 §1.3 "switching is free").

### 2.3 (c) The deltas as configuration (the five axes, instantiated)

| Axis | **L1 Engineer board** | **L2 PM roadmap** | **L3 Exec rollup** |
|---|---|---|---|
| **1 Projection** | board/kanban (status columns) | timeline / now-next-later lanes | grouped table or initiative-timeline w/ rollup |
| **2 Density** | `compact` (P3/P5) | `comfortable` | `comfortable` |
| **3 Vocabulary** | "issue", "cycle", "PR", "estimate" | "work item"/"deliverable", "milestone", "outcome" | "initiative", "throughput", "delivery health" |
| **4 Fields/group/sort** | status × assignee; estimate, labels, PR-link visible; grouped by status | grouped by quarter/initiative; outcome/owner/target-date visible | aggregated by team/initiative; counts, % complete, cycle-time rollup |
| **5 Default-landing/chrome** | engineer lands here (R-06); keyboard-forward, `j/k`, single-key transition | PM lands here; pointer-friendly, chart-forward | exec lands here; read-mostly, drill-down to L2/L1 |

*Same component. Same rows. Fifteen config values across three lenses — zero forked code.* The progress a
PM sees roll up on L2/L3 is the **same event stream** an engineer moves a card on in L1 — so the numbers
**cannot disagree** (the D5 anti-gaming rule; R-05 K4/K5).

### 2.4 (d) Per-lens critique — is each lens excellent *for its persona*?

**L1 critiqued as P1 (backend engineer; heuristics #19 + walkthrough #20).** P1 values keyboard speed,
density, context-without-leaving-flow (personas.md P1). *Is the board starved by being "shared"?*
- ✅ **Density:** `compact` token gives the dense board P1 expects (P5); not softened toward the PM mean.
- ✅ **Keyboard:** `j/k` + arrow focus + single-key transition + keyboard-drag (R-10 §2.4) — full muscle-
  memory path; **never keyboard-only-but-also-never-mouse-required** (P3).
- ✅ **Context:** ref chips in cells (PR-link, blocked-by) thread the wedge into the board (R-10 §1).
- ⚠️ **Risk:** if the component's *default* vocabulary/fields lean PM (axis-3/4 mis-defaulted), P1 sees
  "work items" and outcome columns and reads it as "an engineer tool with PM paint" (R-05 B-b). **Rule:**
  the engineer lens defaults to engineer vocabulary/fields — the *lens owns its defaults* (axis-5), the
  shared component does not impose a cross-audience default. **Verdict: excellent, not a compromise —
  conditional on per-lens defaults.**

**L2 critiqued as P6 (PM; the rank-1, lowest-confidence persona — R-05 B-a).** P6's success is "roadmap
views reflect real delivery from the same data — no second system" (personas.md P6). *Does the same-data
roadmap actually satisfy a PM, or will they keep Productboard?*
- ✅ **Honesty:** the roadmap is live over the engineers' rows — it cannot drift from execution (M1's core
  pain solved). This is the structural win Productboard *cannot* offer (it's a parallel copy).
- ✅ **Approachability:** `comfortable` density + timeline projection + outcome/owner/target fields +
  pointer-friendly chrome — calm and narrative (Axis 1 calm / Axis 4 warm), a *hard requirement* not a
  nicety (§2; Sourcehut lesson).
- ⚠️ **The genuine gap (the rank-1 risk, R-05 B-a):** PMs often want an **aspirational/narrative layer**
  (bets not yet broken into issues; confidence ranges; stakeholder framing) that may have **no engineer-
  side row to project from**. If the only data is committed issues, the roadmap can feel like a *reporting*
  view, not a *planning* view. **Resolution direction (HOUSE STYLE, must be validated §6):** model the
  aspirational layer as **first-class issue-model records at a coarser granularity** (initiative/bet
  records that *are* in the one schema, decomposed later into issues) — so the narrative layer is still
  the same data, not a second store. This keeps D1 a true same-data pair. **If PMs reject coarse records
  and demand a separate narrative tool, D1 is falsified** (§6 falsifier). **Verdict: excellent IF the
  aspirational layer is same-schema-coarse-records; this is the load-bearing bet.**

**L3 critiqued as P11 (CTO/VP).** P11 wants org-wide health from *trustworthy* shared data, no vanity
metric (personas.md P11; R-05 K5). *Is the rollup a real lens or a fork into a BI tool?*
- ✅ **Ground-truth:** the rollup is an aggregate **projection of the same rows** — the exec number and
  the engineer number are derived from one event stream, so they cannot disagree (K5 rule). This is the
  consolidation pitch P11 buys (G1).
- ✅ **Drill-down:** every rollup cell drills to L2 then L1 — the exec can see the ground truth behind a
  number (no "trust me" abstraction).
- ⚠️ **Risk:** execs may want **cross-org/portfolio** cuts (cost, headcount) that pull data *beyond* the
  issue model (billing, identity). **Rule:** L3 stays a lens of the **issue/event data**; cross-domain
  exec data is a *dashboard* surface (§4), not a forked issue view — keeping L3 honest about its data
  scope. **Verdict: excellent within its data scope; do not overload it.**

**Cross-lens verdict.** Neither lens is averaged: L1 stays dense/keyboard, L2 stays calm/narrative, L3
stays drill-downable-truth — *because each lens owns its axis-1–5 defaults*. The single load-bearing
uncertainty is L2's aspirational layer (R-05 rank 1) — addressed by design (coarse same-schema records),
falsifiable by users (§6). **(All instantiations HYPOTHESIS; the mechanism PROVEN.)**

---

## 3. Surface 2 — Knowledge databases (the **D2** pair)

### 3.1 (a) The jobs over the same data

**Shared data = a knowledge database (typed records + views; ADR-06).** Two jobs (R-03 D2):

| Lens | Persona | Job |
|---|---|---|
| **L1 Engineer table** | P1/P3 | *"query a structured collection (e.g. a service catalogue / runbook index) as a table I can edit inline"* |
| **L2 PM/designer board** | P6/P9 | *"see the same records as a board/gallery/timeline for planning"* (UC-KN-4) |

(No distinct exec lens here — D2 is dual, not tri; tagged so it isn't faked into three.)

### 3.2 (b) The one component

The **same views component** (R-10 §2) — *the* issues↔knowledge reuse boundary (R-10 §2 opener; design-
language §5.6 "used by both the issue tracker AND knowledge databases"). The engines underneath differ
(issue workflow/SLA vs. knowledge formula/collab) and surface their own field controls, but the table/
board/field UX is **one component** (PROVEN — §5.6; ADR-06). A knowledge page can **embed a live board**
over an `ArtifactRef` (R-10 §2.3/§3.2) — the editor hosts the views organism inline.

### 3.3 (c) The deltas as configuration

| Axis | **L1 Engineer table** | **L2 PM/designer board/gallery** |
|---|---|---|
| 1 Projection | table (inline-edit, frozen first col) | board / gallery / timeline |
| 2 Density | `compact` | `comfortable` |
| 3 Vocabulary | "record", "field", "query" | "card", "property", "view" |
| 4 Fields/group/sort | all fields, sorted by key; formula columns visible | cover field + a few props; grouped by status/category |
| 5 Landing/chrome | engineer lands on table; keyboard cell-nav | PM/designer lands on board; drag, hover-peek |

### 3.4 (d) Per-lens critique

**L1 as P1.** ✅ The table is the spreadsheet mental model engineers expect (R-01 §2.2 "zero new
concepts"); inline-edit with `Tab`/`Enter` spreadsheet contract (R-10 §2.1); formula/relation fields
exposed. *Not starved.* ⚠️ Risk: if the knowledge DB hides the query/formula power behind a PM-friendly
default, P1 loses the "edit-as-data" leverage — **rule:** L1 exposes the field-definition + query UI
(progressive disclosure: power *available*, not *defaulted-away* — NN/g three-layer). **Verdict:
excellent.**

**L2 as P6/P9.** ✅ Board/gallery over the same records gives planning/visual browsing without a second
database; cover-field gallery suits a designer's asset view. ⚠️ Risk: a designer (P9) is the **lowest-
confidence persona for dual-homing** (R-05 B-e/K7) — they author in Figma; the knowledge-DB board only
helps if their records are *referenced live*, not copied. **Rule:** ref-typed fields render the live R-09
chip (to Figma frames etc.), never a snapshot — so the designer's record stays a live reference. **Verdict:
excellent IF references stay live (depends on B-e/K7 holding — flagged §6).**

**Cross-lens verdict.** D2's risk is lower than D1's (no exec stakes, no aspirational-layer problem) — but
it is the surface where the **issues↔knowledge fracture** would show first: if the issue DB and the
knowledge DB silently ship two view components, the reference graph fractures (R-03 D2 "endangered if
forked"). The single-component rule (§1.1) is the defence.

---

## 4. Surface 3 — Dashboards / metrics (the **D5** pair, tri-audience)

### 4.1 (a) The jobs over the same data

**Shared data = one event stream (one metric derivation; design-language §3.7/§7.3).** Three jobs (R-03
D5):

| Lens | Persona | Job |
|---|---|---|
| **L1 Engineer "my flow"** | P1/P3 | *"my flow / my CI health"* (personal velocity, my PRs, my build health) |
| **L2 Team analytics** | P7 EM / P8 PgM | M4/M7: *"team flow, dependencies, delivery health — insight not surveillance"* |
| **L3 Org rollup** | P11 exec | G1: *"org-wide health & cost"* |

### 4.2 (b) The one component

**The dashboards + charting language** (design-language §3.7 data-viz tokens; §7.3) — one charting
component, one metric-definition layer over **one event stream**. Every chart is a projection/aggregation
of the same events (so L1/L2/L3 numbers reconcile by construction).

### 4.3 (c) The deltas as configuration

| Axis | **L1 My flow** | **L2 Team analytics** | **L3 Org rollup** |
|---|---|---|---|
| 1 Projection | personal scorecards / sparklines | team trend charts, dependency graph | org KPI tiles, cost/throughput |
| 2 Density | `compact` | `comfortable` | `comfortable` |
| 3 Vocabulary | "my PRs", "my build health" | "cycle time", "WIP", "review latency" | "throughput", "delivery health", "spend" |
| 4 Fields/scope | filtered to *me* | scoped to a team; **never default individual-ranking** | aggregated org-wide |
| 5 Landing/chrome | engineer's home widget | EM team page | exec rollup |

### 4.4 (d) Per-lens critique

**L1 as P1.** ✅ Personal, terse, my-data-only — useful self-signal, not someone watching. **Verdict:
excellent.**

**L2 as P7 (EM) — the surveillance trap (R-05 K4).** ⚠️ The genuine danger: metrics drawn for a team can
read as **surveillance**, and reports may **game** them (R-05 B-c/K4; personas.md "fairness, not
gaming"). **Rules (HOUSE STYLE):** (1) **team-level by default, never individual-ranking** (axis-4
default); (2) framed as *team-unblocking insight* not performance scoring; (3) transparency about *what*
is measured. **Verdict: excellent ONLY with the anti-surveillance defaults — this is a design
responsibility, not an averaging problem.** Without them, L2 is rejected by the very persona it serves.

**L3 as P11.** ✅ Because every tile aggregates the same events L1/L2 use, the **exec number and the
engineer number cannot disagree** (R-05 K5 — the D5 anti-gaming rule; the consolidation pitch G1). ⚠️
Risk: vanity metrics. **Rule:** every L3 tile drills to its L2/L1 ground truth (no un-drillable number).
**Verdict: excellent — its trust comes from same-stream derivation + drill-down.**

**Cross-lens verdict.** D5's whole value is that **one event stream feeds all three** so the numbers
reconcile — the moment metrics are re-derived per audience, they disagree and the trap reopens (R-03 D5
"endangered if forked"; R-05 K4/K5). The single-derivation rule is the mechanism.

---

## 5. The persona-adaptive vocabulary proposal (fracturing-risk bounded)

This is design-language **§9's named open question** (`[OPEN → P4/legal]`: "how far per-tenant terminology
customization goes without fracturing the shared model") and R-06's flagged fracturing-risk. R-16 owns
the bounded proposal.

### 5.1 The proposal (HOUSE STYLE)

**Vocabulary is axis-3: a presentation-layer label map over ONE unchanging schema** — never a schema
fork (PROVEN constraint — design-language §2 "terminology is a presentation choice over one model…never a
schema fork"). The mapping is held in **tokens/config** (R-06: labels config/token-held, cheap to change,
tree-testable) so it never reaches the data model, the query AST, the `ArtifactRef`, the API, or the CLI
vocabulary contract (§7.7 — CLI uses the same `ArtifactRef`/vocabulary as the UI).

### 5.2 The three-tier bound (the fracturing-risk control)

To get the translation benefit without fracturing the shared model, **bound where vocabulary may vary:**

| Tier | Variability | Rule |
|---|---|---|
| **T1 — Schema / refs / API / CLI / search** | **FROZEN** | One canonical term per concept. The `issue` type is `issue` in the schema, the `ArtifactRef`, the query AST, and the CLI — forever. Vocabulary **never** varies here. (PROVEN constraint.) |
| **T2 — Lens label (role/space)** | **Bounded set** | A small, *curated* synonym set per concept ("issue"↔"work item"↔"deliverable"; "cycle"↔"sprint"↔"iteration"). Chosen per-space or per-lens from the curated set — **not** free-text. This is the translation §2 wants. |
| **T3 — Free per-tenant rename** | **DISCOURAGED / opt-in, audited** | Arbitrary tenant rename ("issue"→"widget") fractures cross-tenant help, docs, search relevance, and onboarding. Allowed only as an explicit admin opt-in with the cost surfaced; **default is the T2 curated set.** |

**The bound (falsifiable rule):** *vocabulary varies only at T2 (a curated synonym set), is held in config
tokens, and never touches T1. A reviewer who finds a term varying in the schema/ref/API/CLI has found a
fracture.* This is how Myelin gets the "issue↔work item" translation (§2) **without** the Jira-style
config-maze where every tenant invents its own ontology (R-02 trap; R-05 the fracturing-risk).

### 5.3 Why bound it this way (grounded)

- **PROVEN (external):** unbounded customization *harms* cross-user usability and increases support/
  learning cost; system-driven role adaptation outperforms free customization for shared-interface
  multi-role products (Okoone; UXmatters — adaptation vs. customization). T2-curated is adaptation; T3
  free-rename is the customization that fractures.
- **HOUSE STYLE:** the translation value is almost entirely captured by T2 (the role's natural word);
  T3's marginal benefit rarely justifies the cross-tenant coherence cost. **HYPOTHESIS** — whether a
  curated T2 set satisfies real PMs/execs (vs. demanding T3 free rename) is part of §6's validation.

---

## 6. `[DEFERRED-UNTIL-USERS]` — the both-audience validation plan

**This study is the no-user substitute** (the config model §1 + the per-lens critiques §2.4/§3.4/§4.4 +
the bounded vocabulary §5). It is **NOT validation.** Per the honesty rule (VISION §3) and R-05 §5, "one
component serves both" is proven **only** by PMs *and* engineers using the *same* surface and each
accepting their lens. Presenting the critique as validated would be the exact failure forbidden. The
validation is recorded as an executable plan; **it is not done.**

`[DEFERRED-UNTIL-USERS]`

### 6.1 What to test
For each dual-audience surface (D1 issue views first — R-05 rank 1; then D5 dashboards; then D2):
put a **same-data, two/three-lens prototype** in front of each audience and observe whether **each lens
is accepted *by its own persona as excellent*, not merely tolerable**, AND whether each user believes
they are looking at the **same underlying objects** as the other audience (not two different things).

### 6.2 With whom (per-segment — required; shared recruit with R-03 §6.2 + R-05 §5.2 to save fieldwork)
- **Engineers (L1):** 8–12 across P1–P5, **startup + scale-up** archetypes (also tests the archetype-cut,
  R-05 §3).
- **PM/delivery (L2):** 8–12 across P6–P10 — **recruited from the *same orgs* as the engineers** so the
  K1 same-surface tension is *observed*, not inferred (R-05 §5.2). Designers (P9) included for D2.
- **Corporate/exec (L3):** 6–10 across P11/P7 (for dashboards), weighted to where rollups decide adoption.

### 6.3 Method (per-segment usability + RITE — method #7/#24 family)
- **Per-segment usability** on the *same* surface in *both/all lenses*: each persona does their job (P1 a
  sprint on L1; P6 a roadmap on L2; P11 a rollup on L3) on shared data, then *switches lens* to see the
  other audience's view of the same rows.
- **RITE** (Rapid Iterative Testing & Evaluation): fix the config-delta/default after each small batch;
  re-run. The unit of iteration is **a config value (axis 1–5), not new code** — proving the model's
  agility claim in practice.
- **Vocabulary:** A/B the T2 curated set vs. canonical-only vs. (for the bravest tenants) T3 free-rename
  on findability/comprehension (tree-test from R-07 reuses these tasks).

### 6.4 What would falsify "one component serves both" (per-lens falsifiers)
- **D1/L2 (the rank-1 bet, R-05 B-a):** PMs, given the same-data roadmap, **still maintain a separate
  narrative/aspirational tool** (reject coarse same-schema records) → D1 is *not* a same-data pair; the
  §2 split reopens; the "one product" thesis for the PM audience fails.
- **D1/L1 (R-05 B-b):** engineers experience the board as "a PM tool with engineer paint" (the lens
  defaults leaked the wrong way) → the config-not-fork model failed *or* per-lens defaults were wrong.
- **Either lens "averaged" (§1.3 mode 2):** either segment rates its lens **merely acceptable** (not
  excellent) and would prefer a dedicated tool → the serves-neither trap is real; the component must
  diverge further on the five axes or it must, in the worst case, fork (the falsification of the whole
  bet).
- **"Different objects" perception:** users believe the two lenses are *different data* → the same-data
  promise didn't *land* even if it's true under the hood (a legibility failure to fix in design).
- **D5/L2 (surveillance, K4):** EMs/reports perceive surveillance or game the metrics → the anti-
  surveillance defaults (§4.4) are insufficient.
- **Vocabulary (§5):** the T2 curated set leaves a persona reaching for T3 free-rename, OR T3 rename
  measurably harms cross-tenant findability → re-bound the tiers.
- **Archetype-cut (R-05 §3/X-a):** the *archetype* lens (startup↔enterprise) explains acceptance better
  than the *role* lens → re-assign axis-5 defaults by archetype (model holds; defaults change).

### 6.5 The Phase-6 mandate (recorded, per the prompt)
**Phase 6 MUST sketch every dual-audience surface in BOTH/ALL lenses over the same data** — the *same
issue records* as an engineer board AND a PM roadmap (AND an exec rollup), the same knowledge DB as
table AND board, the same event stream as my-flow AND team AND org. This is the sketch-funnel
**comparable-screen-set** requirement (sketch-funnel.md §"comparable screen set"; lines 160–167: "the
same data shown as engineer board AND PM roadmap … *that* is the dual-audience / one-product proof") and
the literal artifact rubric **D5** scores. A finalist that shows only one lens of a dual-audience surface
**cannot be scored on D5** and is incomplete.

### 6.6 What we will NOT claim until this runs
Until 6.1–6.4 execute with real users, **§1–§5 stand as an unvalidated, HYPOTHESIS-tagged design model** —
not a validated claim that one component serves both. The per-lens critiques are *expert heuristic
arguments* that each lens *can* be excellent; only paired per-segment testing proves it *is*.

---

## 7. Completeness-critic (README §9) — gloss-risks this item touches

R-16 is a *dual-audience model* deliverable; it owns the §9 **dual-audience compromise** risk and routes
state/a11y depth to owners:

- **The §2 "serves-neither" averaged compromise** — **OWNED & covered**: §1.3 names it as a falsifiable
  failure mode; §2.4/§3.4/§4.4 critique each lens against its persona to prove non-averaging; §6.4 gives
  the user-facing falsifier. This is R-16's core charge.
- **Persona-adaptive vocabulary fracturing (design-language §9 / R-06)** — **OWNED & covered**: §5 bounds
  it to a three-tier rule (T1 frozen / T2 curated / T3 audited-opt-in).
- **Keyboard operability of the hard components (board drag, table inline-edit)** — **routed to R-10
  §2.4 / R-17**; R-16 *depends on* the keyboard model (it's load-bearing for L1's "excellent for P1"
  critique, §2.4) but does not re-spec it.
- **Permission-denied / erased-tombstoned rows in a view** — **routed to R-10 §2.2 / R-09 / R-21**; R-16
  notes only that a lens is **permission-pre-filtered by construction** (ADR-03), so neither audience can
  infer rows they can't see — a same-component property both lenses inherit free.
- **Storm / optimistic-rollback / conflict / loading-skeleton states** — **routed to R-21**; not re-
  specced here (the cumulative-corpus rule).
- **The archetype-cut process-risk (R-05 §3)** — **named & accommodated** (§1.2 the cut-agnostic model;
  §6.4 the falsifier). Consciously carried, not resolved (needs users).
- **CLI as a peer surface (§7.7)** — **named in §5.1**: vocabulary tier T1 binds the CLI to the canonical
  term, so the chip/term you see in the UI matches the CLI (no per-lens drift on the CLI).

---

## 8. Actionability toward the control artifacts

| Control artifact | What R-16 equips | Where |
|---|---|---|
| **rubric D5** (dual-/tri-audience, 10%) | The **falsifiable mechanism** (one component, five config axes, the switch-the-lens-on-live-data test) + the **per-lens excellence bar** (D5 scores 4 only when *both lenses are excellent*, 0 when one is starved or it forks) + the per-surface critiques that *are* the D5 evidence for D1/D2/D5. | §1, §2.4, §3.4, §4.4 |
| **rubric D4** (coherence) | Vocabulary tier T1 (frozen schema/ref/CLI) + single-component-per-surface rule keep the product mechanically coherent across audiences. | §5, §1.1 |
| **sketch-funnel Axis 3** (surface unification) | The five-axis lens model *is* how a finalist places on Axis 3: a highly-unified finalist tunes only density (axis-2); a distinct-per-surface finalist diverges on projection/chrome (axis-1/5) — **but never forks**. R-16 gives the funnel the vocabulary to occupy *different points* on Axis 3 without fracturing. | §1.1 |
| **sketch-funnel comparable-screen set** | The **both-lens mandate** (§6.5): every dual-audience surface sketched in both/all lenses over the same data — the dual-audience / one-product proof. | §6.5 |
| **R-05 inputs consumed** | Rank-1 K1 → D1/L2's load-bearing critique (§2.4); K4/K5 → D5's anti-surveillance/anti-gaming rules (§4.4); the archetype-cut → the cut-agnostic model (§1.2). | §2.4, §4.4, §1.2 |

---

## 9. Sources (web-verified 2024–2026 + cited platform contracts)

**External (PROVEN):**
- NN/g, "Progressive Disclosure" (defer advanced/role-gated features; novice+expert served by one UI;
  the core/advanced/expert three-layer model): https://www.nngroup.com/articles/progressive-disclosure/
- Okoone, "How adaptive UI can make every user experience smarter" (2025) — adaptation (system-driven,
  role-based) vs. customization (user-driven); role-based density/tool-access improves usability:
  https://www.okoone.com/spark/product-design-research/how-adaptive-ui-can-make-every-user-experience-smarter/
- UXmatters, "UX Design for Personalization" (the adaptation-vs-customization distinction for multi-role
  interfaces): https://www.uxmatters.com/mt/archives/2018/07/ux-design-for-personalization.php

**Platform contracts surfaced (PROVEN, internal — not re-derived):**
- design-language §2 (the dual-audience resolution: same data different lens; default-by-role-switchable-
  by-anyone; vocabulary-translates-never-schema-fork; density-adapts-same-components; approachability-as-
  hard-requirement), §5.6 (views component = one organism over issues AND knowledge), §3.4 (density modes
  comfortable/compact as token sets), §3.7/§7.3 (dashboards/data-viz), §9 (`[OPEN]` persona-adaptive
  vocabulary), §7.7 (CLI same vocabulary/`ArtifactRef`); ADR-06 (shared views primitive), ADR-07 (query
  AST), ADR-03 (permission-pre-filter).

**Prior corpus (built upon, PROVEN-as-cited):** R-03 §5 (D1–D5 pairs), R-05 §2/§4/§3 (K1–K7, rank-1 K1,
archetype-cut risk), R-10 §2 (views component spec, the four config deltas, the two-components trap).

**Honest limitation:** the external findings ground *that* role-based adaptation + progressive disclosure
+ bounded customization are sound *in general*; they do **not** validate Myelin's *specific* lenses or the
five-axis bundle's sufficiency — that is §6's deferred test. Every specific instantiation is HYPOTHESIS.

---

## 10. Self-check against R-16 acceptance criteria

| Criterion (prompt R-16 / ws-g) | Status | Evidence |
|---|---|---|
| **Each dual-audience surface has both/three jobs over the SAME data** | ✅ Met | §2.1 (D1: P1/P6/P11), §3.1 (D2: P1/P6-P9), §4.1 (D5: P1/P7-P8/P11) — each names the shared record set/event stream |
| **One component identified per surface** | ✅ Met | §2.2 views component; §3.2 views component (issues↔knowledge boundary); §4.2 dashboards/charting over one event stream |
| **Deltas expressed as CONFIGURATION, not separate code** | ✅ Met | §1.1 the five-axis bundle + invariant; §2.3/§3.3/§4.3 each surface's deltas as values on axes 1–5 |
| **Each lens critiqued against its persona; shown NOT a degraded compromise** | ✅ Met | §2.4 (L1=P1, L2=P6, L3=P11), §3.4 (P1, P6/P9), §4.4 (P1, P7, P11) — each with ✅ strengths + ⚠️ risk + verdict; §1.3 the averaging failure named falsifiably |
| **Vocabulary mapping proposed with fracturing-risk BOUNDED** | ✅ Met | §5: presentation label-map over frozen schema; three-tier bound (T1 frozen / T2 curated / T3 audited-opt-in); falsifiable rule |
| **Deferred both-audience validation executable-as-written + flagged** | ✅ Met | §6 `[DEFERRED-UNTIL-USERS]`: what/with-whom/method/per-lens falsifiers; not faked; §6.6 "will not claim until run" |
| **"Sketch in both lenses" requirement recorded** | ✅ Met | §6.5 the Phase-6 mandate (comparable-screen set; D5 cannot be scored on a single-lens finalist) |
| **Build ON R-03/R-05/R-10 + canon, don't duplicate** | ✅ Met | Deepens D1–D5 (R-03), consumes K1/K4/K5/archetype-cut (R-05), applies the views component (R-10 §2); references by id, re-derives nothing |
| **Date; PROVEN/HOUSE-STYLE/HYP tags; cited web sources; name uncertainties** | ✅ Met | Dated 2026-06-20; tagged throughout; NN/g + Okoone + UXmatters cited (§9); uncertainties below |
| **Completeness-critic §9 gloss-risks addressed** | ✅ Met | §7: owns the serves-neither + vocabulary-fracturing risks; routes states/a11y to R-10/R-17/R-21; carries the archetype-cut |
| **Actionable toward rubric D5 + sketch-funnel Axis 3 / comparable set** | ✅ Met | §8 mapping |

**Top uncertainties (honest).**
1. **The five-axis config bundle's *sufficiency*** (§1.2) is the load-bearing HOUSE-STYLE bet — that no
   lens needs a sixth axis or a code fork to be excellent. Only §6's per-segment RITE settles it; if a
   lens needs more than config, the model weakens.
2. **D1/L2's aspirational layer** (§2.4) — whether modelling PM bets as coarse same-schema records
   satisfies real PMs, or whether they demand a separate narrative tool, is **R-05 rank-1, confidence L**
   — the single most likely point of falsification for the whole "one product" thesis.
3. **The role-cut vs. archetype-cut** (§1.2, R-05 §3) — R-16 lenses by role; if archetype (startup↔
   enterprise) dominates, the model survives (cut-agnostic) but the default-assignment changes; unproven
   until users.
4. **Vocabulary tier T2 vs. T3** (§5) — whether a curated synonym set is enough or tenants demand free
   rename (and whether free rename measurably fractures findability) is HYPOTHESIS until the §6 A/B runs.

---

*End of R-16 deliverable. Date: 2026-06-20. Method #18 (one component, many lenses) PROVEN; all specific
lenses, deltas, critiques, and the vocabulary bound are HYPOTHESIS-instantiation (no users). Both-audience
validation is `[DEFERRED-UNTIL-USERS]` (§6), recorded as a plan, not done. Builds on R-03 (D1–D5), R-05
(K1/K4/K5/archetype-cut), R-10 (views component). Feeds rubric D5, sketch-funnel Axis 3 + comparable-screen
set, and Phase 6 (both-lens sketches).*
