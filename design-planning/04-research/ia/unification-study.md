# R-07 — Unification-vs-Distinctness Study + Card-Sort/Tree-Test Plan

> **Phase 4 research corpus** · deliverable of prompt **R-07** (workstream
> [`ws-c`](../../02-research-roadmap/), Seq #7). **File date: 2026-06-20.**
> Method **#6 (expert-led IA design — ADOPT)** for Part 1; **#7 (card-sort / tree-test design)**
> for Part 2. This file is the **per-surface ruling** on the central design problem's
> defining tension — *how much each surface unifies with the shared system vs. earns its own
> distinctness* — and the concrete, executable validation plan that would confirm or break it.
>
> **It builds directly ON** [R-06 platform-ia](./platform-ia.md) — the IA tree, shell regions,
> `ArtifactRef` scheme, and persona-adaptive vocabulary this study *rules on* and *tests* — and
> [R-03 jtbd-catalogue](../jtbd-flows/jtbd-catalogue.md) — the jobs that become realistic
> tree-test task scenarios and the dual-audience same-data pairs (D1–D5) the per-segment runs
> must expose. Where this file says "surface X", it is an R-06 §2-tree node; where it says "flow
> F-X" or "job E3/M1/G5", it is R-03/R-04, not re-stated.
>
> **This is the direct, explicit input to sketch-funnel [Axis 3](../../02-research-roadmap/sketch-funnel.md)
> (surface unification ↔ distinct-per-surface)** — *the axis that IS the central design problem*
> ([README §1](../../02-research-roadmap/README.md)) — and to **rubric D4** (one-product coherence).
> Part 1 §4 states the Axis-3 handoff in funnel terms.
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited standard / an existing architecture
> contract we surface (ADR-06/13, §5.6, §2). **HOUSE STYLE** = our design synthesis / taste. The
> per-surface rulings are **HOUSE STYLE expert judgement over PROVEN constraints**; *none is
> user-validated* — Part 2's `[DEFERRED-UNTIL-USERS]` card-sort + tree-test is what would validate
> them, and **this IA must not be treated as validated before that runs** (§Part 2.6, binding).

---

## 0. How to read this file

Two parts, as the prompt requires.

- **Part 1 — The study (method #6).** §1 frames the tension as *two layers* and gives the one
  **rule for earning distinctness** (the whole study in one rule). §2 is the **per-surface ruling
  table** — every R-06 surface placed on the unification↔distinctness axis with the *reason it
  earns (or does not earn) its own density tier*. §3 names the **shared invariants no surface may
  break** (the "always unified" floor). §4 is the **explicit Axis-3 / rubric-D4 handoff**.
- **Part 2 — The `[DEFERRED-UNTIL-USERS]` validation plan (method #7).** A closed/hybrid
  **card-sort** design (§Part 2.2) + a **tree-test** design over the R-06 IA with R-03-grounded
  tasks (§Part 2.3), run **per-segment** (engineer / PM-delivery / corporate-governance) to expose
  the dual-audience split (§Part 2.4), with sample sizes, metrics, success thresholds, and the
  **falsification conditions** (§Part 2.5) — and the binding **not-validated caveat** (§Part 2.6).

Then **§5** completeness-critic gloss-risks, **§6** self-check.

---

# PART 1 — THE STUDY (where unification yields to distinctness, and why)

## 1. The tension, framed as two layers (the central problem, made rulable)

R-06 §1.1 already proved the load-bearing structural fact: **unification lives at the
object/address/navigation layer** (one tree, one `ArtifactRef`, one shell skeleton, one set of
global surfaces), and **distinctness lives only in the *content region*** (R-06 §3.3 — the one
region that earns per-surface density). This study's job is to rule, *per surface*, **how much
distinctness the content region earns, and on what justification** — so the answer is a *rule*,
not case-by-case whim (the acceptance criterion).

> **The central problem restated as the axis this file rules on:** too unified and a diff is
> starved of density or a roadmap suffocates under a one-size grid; too distinct and the product
> fractures back into the Atlassian "stitched-together" feel (R-01 §4.1; competitive-landscape §6)
> — the exact failure Myelin exists to beat. *(PROVEN framing — README §1; HOUSE STYLE rulings
> below.)*

### 1.1 The rule for *earning* distinctness (the whole study in one rule)

A surface earns its own density/interaction tier **if and only if** the *job's information
shape demands it AND a shared invariant is not broken to provide it.* Concretely, distinctness is
earned by exactly **three justification types** — and *nothing else* counts:

| # | Justification a surface may invoke to earn distinctness | Example | Counter-example (NOT earned) |
|---|---|---|---|
| **J1 — Information shape** | The job's primary object has an intrinsic structure (a 2-D positional artifact, a time-ordered stream, a temporal range) that a generic table/list cannot render without loss. | A **diff** (two-column, line-anchored, syntax-coloured) — a list of lines loses the change; a **chat timeline** (reverse-chronological, presence, typing). | A roadmap rendered as a *bespoke* layout when it is a **view of the issue model** (J3 says it is the views component, not a fork). |
| **J2 — Density tier the audience requires** | The primary audience's job is *speed through volume* (P3/P5), needing a tighter spacing/row tier than the calm default — delivered by a **density token**, not different code (§2/§5.6). | A diff and a CI log at **compact** tier by default; a board at the engineer's tier. | A PM dashboard given its own spacing system instead of the calm tier of the *same* token ramp. |
| **J3 — Interaction grammar the job needs** | The job needs verbs a generic surface lacks (drag-between-columns, range-resize, inline diff-comment) — added as **component-tier behaviour on the shared component**, not a parallel component. | Board drag, calendar drag-to-reschedule, timeline range-resize — all the **§5.6 views component's own projections** (PROVEN, ADR-06). | A separate "PM roadmap app" with its own nav, chip, and editor (the fork that re-creates the dual-product split, §2 trap). |

**The earning test (HOUSE STYLE, the binding rule):** *distinctness is permitted exactly to the
extent J1/J2/J3 require it, expressed as **density tokens + component-tier behaviour + view
projections over shared primitives** — and the moment it requires a second shell, a second chip, a
second identity badge, a second palette/inbox, or a second editor, it has stopped being earned
distinctness and become a **fork** (the §3 invariant breach).* This is Nielsen's "internal
consistency / break a convention only for a *new and better* pattern justified against the added
cognitive load" applied platform-internally (PROVEN —
[NN/g, Consistency and Standards](https://www.nngroup.com/articles/consistency-and-standards/);
the cost of breaking consistency is added cognitive load, so the bar is "the job demands it", not
"it would look nicer").

This rule is what makes per-surface density **"tuning of shared components, not a fork"** — the
exact wording of rubric **D4** (which scores 4 = "dense and calm surfaces are visibly the same
system tuned by density").

---

## 2. The per-surface ruling (every R-06 surface placed + the reason)

**Axis read:** `0.0` = maximally unified (one skin, density-tuned only); `1.0` =
maximally distinct (own identity within shared tokens). **Every surface keeps the §3 invariants
regardless of position** — the number is *only* how much the *content region* diverges. The
"Earns via" column cites J1/J2/J3 from §1.1; "Stays unified on" names what does **not** move.
*(All rulings HOUSE STYLE over PROVEN ADR-06/§5.6/§2 constraints; column "Job" is R-03.)*

| Surface (R-06 §2 node) | Job (R-03) | Axis-3 position | Earns distinctness via | Stays unified on | The one-line ruling |
|---|---|---:|---|---|---|
| **Diff / files-changed** (`code/<repo>/pr/<pr>`) | E4 review, E3 fix | **0.85 (high)** | **J1** (2-D line-anchored two-column artifact; a list loses the change) + **J2** (compact tier) + **J3** (inline diff-comment, "since I last looked") | shell, chip/unfurl, identity, palette, comment thread, editor (for comments) | *A diff earns its own dense, two-column, line-anchored tier because the change itself has a 2-D shape a generic table cannot render — but its comments are the one editor and its links are the one chip.* |
| **CI run / live log** (`ci/<run>`) | E3 fail→step→line | **0.8 (high)** | **J1** (DAG + time-ordered append-only log) + **J2** (compact, monospace, virtualised) | shell, chip (log-line ref §5.2), inbox routing, palette | *A live log earns a dense streaming monospace tier because a log is a time-ordered stream, not rows — but a log-line is an `ArtifactRef` like everything else.* |
| **Issue board / cycle** (`issues/<project>/view`) | D1 engineer lens | **0.55 (mid-high)** | **J3** (board drag, WIP, swimlanes — **§5.6 projection**) + **J2** (engineer density tier) | **it IS the §5.6 views component** — shell, chip, editor (issue body), palette, identity | *A board earns drag + a tight tier as a **projection** of the shared views component, never a separate board engine.* |
| **Roadmap / timeline** (`issues/<project>/roadmap`) | D1 PM lens, M1 | **0.5 (mid)** | **J1** (temporal ranges + dependencies) + **J2** (calm/spacious tier) — *as another §5.6 projection* | same §5.6 component, same records, same chip/editor | *A roadmap earns spacious pacing + range bars because outcomes-over-time is its information shape — but it is **the same component over the same issue records** as the board (D1), the dual-audience proof.* |
| **Portfolio / exec rollup** (`issues/portfolio`) | G1 exec lens | **0.45 (mid)** | **J2** (read-forward, low-density, chart-forward calm tier) | same §5.6 component + dashboards/charting (§3.7) | *A portfolio earns a calm read-forward tier; it is the roadmap's records rolled up, not a third app.* |
| **Knowledge page / editor** (`knowledge/<space>/page`) | M2 spec, E5 read | **0.4 (mid)** | **J1** (long-form prose canvas) + **J3** (slash-menu blocks) | **it IS the §5.9 editor** + chip + views (embeds) — used identically for PR/issue/chat bodies | *A page earns a spacious prose canvas, but it renders through the **one editor** every other body uses; "page-ness" is layout, not a fork.* |
| **Knowledge database** (`knowledge/<space>/db`) | D2 same-data | **0.5 (mid)** | **J3** (same view projections as Issues — **the §5.6 reuse seam**) | **same §5.6 views component** as Issues (ADR-06, the biggest reuse boundary) | *A knowledge DB earns the same table/board/gallery views as Issues because it IS the same component over `db-row` instead of `issue` — distinctness here would re-fracture the reuse boundary.* |
| **Chat timeline / thread** (`chat/<channel>/thread`) | E9 incident, M6 | **0.7 (high)** | **J1** (reverse-chrono live stream, presence, typing, threading) + **J3** (composer, unfurl-in-place) | shell, chip/unfurl (live, inline actions), inbox, identity, editor (composer) | *Chat earns a live conversational stream layout because a conversation has a distinct temporal/presence shape — but an unfurl is the one chip and a slash-command is the one palette verb vocabulary.* |
| **Inbox** (`[G] Inbox`) | M5, D3 | **0.2 (low)** | **J2 only** (per-segment weighting/density) | **shell-owned global surface (R-06 §4.3)** — one read-state truth, one "why-fired" provenance, identical in Code and Chat | *The inbox earns almost no distinctness — it is the anti-firehose spine and must be the **identical** component everywhere or the calm promise dies.* |
| **Command palette** (`⌘K`) | E6/E7 | **0.1 (very low)** | none beyond density | **shell-owned global surface (R-06 §4.1)** — one query AST, one ToolDef vocabulary, every screen | *The palette is maximally unified by mandate; a per-subsystem palette IS the fracture (R-06 §4 invariant).* |
| **Reference chip / unfurl** | the wedge, all flows | **0.05 (minimal)** | none (per-artifact-*type* rendering ≠ distinctness; same component) | **the single most unified component** (R-06 §5.3; P6) — one chip, one resolver, four guarantees | *The chip is the unification mechanism made visible; it renders every artifact type through one component — variety of *content*, zero variety of *component*.* |
| **Identity / scope selector** | (cross) | **0.0 (none)** | none | **shell-owned (R-06 §4.4)** — one `Principal` badge, one scope selector governs all five sidebars | *Zero distinctness by mandate: one identity, one scope, everywhere — the single-identity embodiment of the central problem.* |
| **Agent panel / HITL card** (§5.4) | E11, G4, agent flagship | **0.3 (low-mid)** | **J3** (plan-then-apply effects list, Approve/Edit/Reject) — *as one card component* | the agent treatment (§3.2) is **identical on every surface** it appears (chat, inbox, inline) — R-14 owns | *The HITL card earns a structured proposed-effects layout but is **one card** appearing in many places, never per-subsystem agent UIs (the bolted-on-bot-console fracture).* |
| **Governance / GDPR / audit consoles** (`[A] Platform`) | G2–G10 | **0.4 (mid)** | **J1** (DSR fan-out, audit-graph, RoPA map are genuinely distinct information artifacts) + **J3** (locate/export/erase verbs) | shell, chip, identity, palette, audit-link provenance (P7/P9) | *Consoles earn their own dense, specialised layouts because a DSR fan-out / audit graph is a genuinely different information shape — R-19 owns; one layer down (P4), never imposed.* |

**How to read the spread (HOUSE STYLE):** the high-distinctness surfaces (diff `0.85`, log `0.8`,
chat `0.7`) are exactly the ones with an **intrinsic non-tabular information shape (J1)** and a
**speed-through-volume audience (J2)**. The mid-band (board/roadmap/portfolio/DB `0.45–0.55`) are
**all the same §5.6 views component in different projections** — they *look* different because the
*projection* differs, not the component (the dual-audience mechanism, D1/D2). The low band
(inbox/palette/chip/identity `0.0–0.2`) are the **shell-owned global surfaces that must be
identical or the product fractures** (R-06 §4 invariant). **The pattern is the rule (§1.1):
distinctness tracks information-shape + audience-density, never subsystem identity.**

### 2.1 The two surfaces the funnel most needs spread on (and the trap each guards)

Per sketch-funnel §Part 3 (the binding spread rule: finalists must occupy *materially different*
Axis-3 positions), the two surfaces where a finalist's Axis-3 stance is most *visible and
contestable* — i.e. where Phase 6 should deliberately place finalists differently — are:

1. **The board↔roadmap pair (the §5.6 views component, D1).** A *highly-unified* finalist makes
   board and roadmap visibly one component density-tuned (low Axis-3); a *distinct-per-surface*
   finalist gives the roadmap more of its own pacing/identity (higher Axis-3). **The trap a
   finalist must not fall into either way:** over-unify and the roadmap suffocates under the
   board's grid (PM surface starved → D5 fork-by-starvation); over-distinguish and they read as
   two apps over two data sets (the §2 dual-product split returns). *This pair is the literal D5
   "neither lens a degraded compromise" test.*
2. **The diff vs. the rest of the shell.** The diff is the highest-distinctness surface (`0.85`);
   a finalist's coherence is most tested by *whether the diff still feels like the same product*
   despite its earned density. **The trap:** a *too-uniform* finalist starves the diff to match a
   calm default (engineer surface starved → D1/D7 loss); a *too-distinct* finalist lets the diff
   grow its own chip/comment/identity (fracture → D4 loss). The §3 invariants are what keep the
   diff coherent *while* dense.

---

## 3. The shared invariants (the "always unified" floor no surface may cross)

This is the non-negotiable counterpart to §2: regardless of a surface's Axis-3 position, these
**never vary** — they are the mechanical coherence guarantee (P1; R-06 §4 invariant), and a sketch
that breaks any one of them has **forked the product** (the D4 = 0 anchor: "five stitched-together
looks"). *(PROVEN — design-language P1; R-06 §3.5/§4 invariant.)*

1. **One shell skeleton** — primary rail + contextual sidebar + content + context pane; switching
   subsystem never re-skins the rail (R-06 §3.1).
2. **One identity / scope** — one `Principal` badge, one scope selector governs all five sidebars
   (R-06 §4.4).
3. **One reference chip / unfurl** — every artifact type through one component + one resolver +
   the four resolution guarantees (R-06 §5.3; the wedge).
4. **One command palette + one search** — one query AST, one ToolDef verb vocabulary, every screen
   (R-06 §4.1/§4.2).
5. **One inbox** — one read-state truth, one "why-fired" provenance, cross-subsystem (R-06 §4.3).
6. **One editor / one views component** — every body through the §5.9 editor; every structured
   collection through the §5.6 views component (ADR-05/06).
7. **One token system** — every surface consumes the *same* semantic tokens (§3.1); distinctness
   is a **density-token + layout** choice, never a second palette of colours/spacing.
8. **One agent treatment** — the badge/colour/glyph is identical on every surface an agent appears
   (§3.2; R-14).

> **The reviewer's one-test for the whole study (HOUSE STYLE, = rubric D4 made concrete):** *open
> the same chip, palette, inbox, and identity badge in the diff (Axis-3 `0.85`) and in the roadmap
> (Axis-3 `0.5`). If they are the identical component, distinctness was **earned**; if either is a
> bespoke clone, distinctness became a **fork**.* This is the exact D4 check from R-06 §4, now with
> the per-surface positions that make it testable.

---

## 4. The explicit Axis-3 / rubric-D4 handoff (this is the point of Part 1)

**This study IS the content of sketch-funnel Axis 3 (surface unification ↔ distinct-per-surface).**
Stated in funnel terms so Phase 6 can act on it directly:

- **What Axis 3 means, concretely, after this study:** a finalist's Axis-3 position is *how far it
  pushes the content region's distinctness on the high-J1 surfaces (diff, log, chat) and on the
  §5.6 mid-band (board/roadmap)* — **never** how much it varies the §3 invariants (those are fixed
  at `0` for every finalist).
- **The spread rule, made checkable (sketch-funnel §Part 3 binding rule):** finalists must differ
  on Axis 3 — i.e. differ in *how distinct the diff and the roadmap are allowed to feel* — **while
  every finalist holds all eight §3 invariants.** A finalist that achieves spread by breaking an
  invariant is a **cull failure**, not a valid Axis-3 position (it has forked, not distinguished).
- **The comparable-screen-set tie-in (sketch-funnel §Part 4):** screens 2 (dense engineer surface)
  and 3 (approachable PM/corporate surface) "must use the same shared components tuned differently"
  — **§2 of this study says *which* tuning (density token + view projection) is legitimate (J1/J2/J3)
  and §3 says which sharing is mandatory (the eight invariants).** That pairing *is* the D4/D5
  proof Phase 7 scores.
- **The two designed-in spread points:** §2.1 — the board↔roadmap pair and the diff-in-shell — are
  the two places Phase 6 should *deliberately* place finalists at different Axis-3 positions,
  because they are where the tension is most visible and most decision-relevant for the human.

| Control artifact | What Part 1 equips | Where |
|---|---|---|
| **sketch-funnel Axis 3** (THE central-problem axis) | The per-surface position table (§2) + the earning rule (§1.1) = the literal definition of what "more/less unified" means per surface; the two designed-in spread points (§2.1). | §1.1, §2, §2.1, §4 |
| **rubric D4** (one-product coherence, 14%) | The eight invariants (§3) = the D4 floor; the "same chip in diff and roadmap" test (§3) = the D4 check made concrete with positions. | §3 |
| **rubric D5** (dual-/tri-audience, 10%) | The board↔roadmap ruling (§2.1) = the "neither lens a degraded compromise" target, with the two failure traps named. | §2.1 |
| **rubric D1/D7** (power-density / density-made-calm) | J2 (density tier earned by audience) is the rule that lets the diff be dense (D1) without the roadmap being noisy (D7). | §1.1, §2 |

---

# PART 2 — `[DEFERRED-UNTIL-USERS]` VALIDATION PLAN (card-sort + tree-test)

> **`[DEFERRED-UNTIL-USERS]`** — No real users exist (personas P1–P15 are hypotheses, R-03 §0).
> Part 1 is the **no-user expert substitute**; the rulings and the R-06 IA are **NOT validated.**
> What follows is the **concrete, executable plan** (method #7) that would validate them, written
> so it can be run as-is the moment users exist. *(Methods PROVEN —
> [NN/g, Card Sorting vs. Tree Testing](https://www.nngroup.com/articles/card-sorting-tree-testing-differences/);
> [NN/g, Interpreting Tree-Test Results](https://www.nngroup.com/articles/interpreting-tree-test-results/);
> [MeasuringU, Tree-Testing IA](https://measuringu.com/tree-testing-ia/).)*

## Part 2.1 — What we are validating, and the order

Two complementary studies, run in this order (the standard generative→evaluative IA sequence —
PROVEN, [NN/g card-sort vs tree-test](https://www.nngroup.com/articles/card-sorting-tree-testing-differences/)):

1. **Hybrid card-sort (generative)** — does the R-06 §2 *grouping* and the §6 *labels* match users'
   mental models? Validates **structure + vocabulary** (incl. the §6.3 fracturing-risk, the file's
   top uncertainty).
2. **Tree-test (evaluative)** — given the R-06 tree, can users *find* things via realistic R-03/R-04
   tasks? Validates **findability** of the structure the card-sort informed.

Both run **per-segment** (§Part 2.4) because the dual-audience split is the whole point — an IA
"validated" only on engineers would re-create the very fracture Myelin exists to kill.

## Part 2.2 — The card-sort design (closed/hybrid)

- **Type: hybrid** (closed for the canonical set + open to catch missing categories) — PROVEN fit
  when an IA already exists but its labels are unproven (our exact case, R-06 §6.3).
  [NN/g, Card Sorting](https://www.nngroup.com/articles/card-sorting-definition/).
- **Cards (the artifacts/destinations, ~40–50):** drawn from R-06 §8 coverage — `repo`, `PR`,
  `diff`, `CI run`, `failing step`, `log line`, `issue`, `sub-issue`, `cycle/sprint`, `roadmap`,
  `portfolio`, `space`, `page`, `database`, `block`, `backlink`, `channel`, `thread`, `message`,
  `unfurl`, `inbox item`, `saved view`, `dashboard`, `audit entry`, `DSR request`, `residency
  setting`, `agent`, `agent proposal`, `kill-switch`, `permission/role`, `SSO config`, `export`,
  etc. — written as **artifacts a user would look for**, not subsystem names.
- **The provided categories (closed half):** the R-06 §2 top-level groups — `Code · CI · Issues ·
  Knowledge · Chat · [G] Home · [G] Inbox · [G] Search · [A] Platform/Governance` — **plus an
  "I'd-look-somewhere-else (write it)" bucket** (the open half) to catch where the tree is wrong.
- **The two label conditions (the §6.3 fracturing test — the decisive sub-study):** run the *same*
  card-sort with **two label sets** between subjects — (A) engineer-neutral (`Issue · Project ·
  Cycle · Board`) and (B) PM/exec lens (`Work item · Initiative · Sprint · Roadmap`). The
  hypothesis under test: *both segments co-locate the same underlying object regardless of the
  label shown.* (R-06 §6.2 mapping; the file's largest uncertainty.)
- **Analysis:** standardisation grid + dendrogram / similarity matrix; flag any card a segment
  places in a category the tree does not predict; flag any label that scatters (= it fractures).
- **Sample: ≥ 30 per segment** for quantitative similarity-matrix confidence (PROVEN — 30 gives
  statistical confidence for card sorts; 15 is the qualitative floor —
  [Maze, how many users per method](https://maze.co/blog/user-testing-how-many-users/);
  NN/g). With 3 segments × 2 label conditions, budget recruits accordingly (§Part 2.4).

## Part 2.3 — The tree-test design (over the R-06 tree, R-03-grounded tasks)

- **The tree:** the R-06 §2 tree exactly as published (depth ≤4 to any item, ≤5 to sub-artifact —
  the NN/g tree-test sweet spot, R-06 §2 rule 1). Labels held in config (R-06 §6) so the tested
  tree = the buildable tree; **run twice (engineer-label tree, PM/exec-label tree)** to test §6
  per-segment.
- **Tasks — derived from R-03 jobs / R-04 flows (realistic, find-oriented, no clue-words):** each
  task names a *goal*, never a path, and avoids the tree's own labels (PROVEN tree-test discipline,
  [NN/g interpreting results](https://www.nngroup.com/articles/interpreting-tree-test-results/)):

  | # | Task wording (find-oriented) | From job/flow | Tests |
  |---|---|---|---|
  | T1 | "A check on your change just failed. Find the exact step that broke." | **E3** / F-ENG-1 | the wedge spine `Code/CI` boundary |
  | T2 | "Find the runbook document linked to last night's incident." | **E9/M6** / F-PM | cross-subsystem Knowledge↔Chat |
  | T3 | "You manage delivery. Find where 'now/next/later' for the quarter lives." | **M1** / D1 PM lens | roadmap label (the §6 fracture test) |
  | T4 | "Find the board where you'd drag your current sprint's tasks." | **D1** engineer lens | board label co-location vs T3 |
  | T5 | "Find every place a specific person's personal data appears." | **G5** / F-GOV-1 | `[A] Platform/Governance` discoverability |
  | T6 | "Find where you'd see which automated agents can act in your org, and stop one." | **G4** | agent-governance + kill-switch placement |
  | T7 | "Find the one place that shows everything waiting on *you* across all tools." | **M5** / D3 | `[G] Inbox` hoisted-above-subsystems hypothesis (R-06 nav rule 2) |
  | T8 | "Find where this project's data is physically stored." | **G7** | residency console placement (Axis-6 tie) |

- **Metrics + thresholds (PROVEN — Treejack/NN/g):** report **success rate** (target **≥ 80%**),
  **directness** (% reaching the answer without backtracking; **≥ 70%** is the method average),
  and **first-click**; visualise with a **pietree** to see *where* wrong paths went.
  ([Optimal Workshop, interpreting Treejack task results](http://support.optimalworkshop.com/en/articles/2626846-understand-the-overall-task-results-in-treejack);
  [NN/g interpreting tree-test results](https://www.nngroup.com/articles/interpreting-tree-test-results/).)
- **Sample: ≥ 50 per segment per tree** for narrow confidence intervals on success rate (PROVEN —
  50–150 is the NN/g quantitative range; at n≈90 a 50% finding has a ±10% CI —
  [NN/g](https://www.nngroup.com/articles/interpreting-tree-test-results/);
  [MeasuringU](https://measuringu.com/tree-testing-ia/)). The 15–20 floor yields qualitative
  signal only — acceptable for an early formative pass, *not* for a pass/fail verdict.

## Part 2.4 — Per-segment runs (the dual-audience split is the point)

Run **both studies separately for three segments**, matched to R-03's audiences and R-05's
validation-priority (the P6-PM vs P1-engineer tension is the #1 priority pair):

| Segment | Personas (R-03) | Card-sort n | Tree-test n (per label tree) | What its result decides |
|---|---|---:|---:|---|
| **Engineers** | P1–P5 | ≥30 | ≥50 | Do the engineer-neutral labels + the dense surfaces' placement hold for the speed audience? |
| **PM / delivery** | P6–P10 | ≥30 | ≥50 | Do the PM-lens labels (roadmap/work-item/initiative) score *as well* as engineer labels — the D5 dual-audience bet? |
| **Corporate / governance** | P11–P15 | ≥20 | ≥30 | Are the `[A]` consoles (DSR, audit, agents, residency) discoverable by the buyers who decide adoption? (overlaps R-19's regulated-buyer review — run jointly to save fieldwork, per R-03 §6.2) |

**The decisive cross-segment comparison (the central-problem validation):** for the §5.6 same-data
pairs (D1 board↔roadmap; D2 issue-DB↔knowledge-DB), check that **both** segments **co-locate the
same object under their own label** and **both** trees clear the success threshold. If engineers
clear it but PMs do not (or vice-versa), the "one component serves both" premise is weakened *for
that surface* — feeding R-16's both-audience validation and the D5 score.

## Part 2.5 — What would falsify the R-06 IA (the falsification register)

The IA / this study's rulings are **falsified** (and must be revised, not defended) if:

1. **Grouping fails:** any T-task lands **below 80% success** (or below ~70% directness) in a
   segment → the R-06 §2 grouping is wrong for that segment (the structure, not just the label).
2. **A label fractures (the §6.3 top-uncertainty test):** in the card-sort, the *same* object
   scatters across different categories under its two labels — e.g. PMs file "work item" where no
   engineer looks for "issue" → the persona-adaptive vocabulary **fractures the shared mental
   model** (R-06 §6.3 / §12 top uncertainty confirmed). This is the single most important result.
3. **Nav rule 2 is wrong:** users systematically expect a **subsystem L1 (`Issues`)** as the
   *primary* entry rather than `[G] Home`/`[G] Inbox`/palette (T7 fails; or the open card-sort
   bucket shows users never hoist Inbox/Search/Home above subsystems) → R-06 nav rule 2 is falsified.
4. **The dual-audience premise weakens:** a §5.6 same-data pair (D1/D2) clears one segment's tree
   but not the other's → "one component, both lenses" is not yet earned for that surface (feeds D5,
   R-16).
5. **A console is undiscoverable:** T5/T6/T8 fail in the corporate/governance segment → the `[A]`
   "one-layer-down" placement (P4) is too buried for the buyers it serves (feeds R-19).

For each, the **remedy is cheap by construction:** R-06 held labels and landings in config/tokens
*precisely so* a failed result is applied by re-mapping, not re-coding (R-06 §6/§9 caveat).

## Part 2.6 — The binding "not-validated" caveat

> **Do NOT treat the R-06 IA or this study's per-surface rulings as validated before Part 2 runs.**
> Part 1 is expert judgement (method #6) — the no-user substitute, honestly bounded. The Axis-3
> positions in §2 are **HOUSE STYLE hypotheses** that the tree-test would confirm or break; the §6
> persona-adaptive vocabulary is an **unvalidated bet** whose §6.3 fracturing-risk is exactly what
> the two-label card-sort (§Part 2.2) exists to test. Phase 5/6 may **build on** these rulings (the
> corpus must move forward) but must carry them as **provisional**, and Phase 6 must keep labels and
> Axis-3 stances **cheap to change** (config/tokens) so a post-user result is applied without a
> restart. *(VISION §3 honesty rule; consistent with R-03 §6 / R-06 §9 deferred-handling.)*

---

## 5. Completeness-critic (README §9) — gloss-risks this item touches

R-07 is the **unification-ruling + IA-validation-plan** owner. Relevant §9 risks:

- **Persona-adaptive vocabulary fracturing** — **OWNED as the validation target here**: R-06 §6.3
  flags it; this file makes it the **decisive two-label card-sort** (§Part 2.2) and falsification
  #2 (§Part 2.5). The per-lens *critique* is R-16; the *test* is here.
- **The dual-audience same-data pairs (D1/D2) shown in both lenses** — **placed**: §2.1 (the
  board↔roadmap spread point) + the cross-segment comparison (§Part 2.4) ensure neither lens is a
  starved compromise (the §2 trap); depth of the per-lens critique → R-16.
- **CLI as a peer surface (§7.7)** — **placed**: the CLI is the same R-06 §2 tree rendered
  textually (R-06 §8), so it inherits the §3 invariants and §2 rulings (textual density is its J2
  tier); not separately card-sorted (it shares the tree). Named, scoped to R-06's coverage.
- **Consciously deferred (with reason):** per-component state sets (R-21), the chip/palette/inbox
  *interaction* specs (R-08/R-09/R-10), the per-lens persona critique (R-16), the regulated-buyer
  console review (R-19) — this file rules on *position* and plans *IA validation*, not component
  internals; duplicating them would break the cumulative-corpus rule.

---

## 6. Self-check against R-07 acceptance criteria

| Criterion (prompt R-07) | Status | Evidence |
|---|---|---|
| **Every surface has a unification↔distinctness ruling with a stated *rule* (not case-by-case whim)** | ✅ Met | §1.1 the single earning rule (J1/J2/J3 + the fork line); §2 the per-surface table places **every** R-06 surface with its J-justification; §3 the eight invariants floor |
| **The ruling feeds Axis 3 (said explicitly)** | ✅ Met | §4 states the Axis-3 handoff in funnel terms (what "more/less unified" means per surface; the spread rule made checkable; the two designed-in spread points §2.1) |
| **Card-sort design executable as-written** | ✅ Met | §Part 2.2: hybrid type, ~40–50 cards from R-06 §8, closed categories + open bucket, the two-label condition for the §6.3 test, analysis method, n≥30/segment (cited) |
| **Tree-test design executable as-written with grounded tasks** | ✅ Met | §Part 2.3: the R-06 tree, 8 find-oriented tasks each mapped to an R-03 job / R-04 flow, success ≥80% / directness ≥70% / pietree, n≥50/segment (cited) |
| **Per-segment runs (engineer vs PM/corporate) to expose the dual-audience split** | ✅ Met | §Part 2.4: three segments, sample sizes, the decisive cross-segment same-data comparison (D1/D2) |
| **The deferred flag + "don't treat as validated" caveat explicit** | ✅ Met | Part 2 header `[DEFERRED-UNTIL-USERS]`; §Part 2.6 binding caveat; falsification register §Part 2.5 |
| **Build ON R-06 + R-03, don't duplicate** | ✅ Met | §2 cites R-06 §2 nodes + R-03 jobs by id; tasks derive from R-03/R-04; the IA tree is referenced, not restated |
| **PROVEN/HOUSE-STYLE tags + date + cited web sources** | ✅ Met | Tagged throughout; dated 2026-06-20; NN/g (consistency, card-sort vs tree-test, interpreting results), Optimal Workshop, MeasuringU, Maze cited |
| **Actionable toward rubric D4 + funnel Axis 3** | ✅ Met | §4 table (Axis 3 = §2/§1.1; D4 = §3 invariants + the same-chip test; D5 = §2.1; D1/D7 = J2) |
| **Completeness-critic §9 gloss-risks addressed** | ✅ Met | §5 (owns vocabulary-fracturing as the test target; places dual-audience-both-lenses + CLI; defers component internals with reason) |

**Top uncertainties (honest):**
1. **The persona-adaptive vocabulary (§6.3) remains the largest open risk** — the two-label
   card-sort (§Part 2.2, falsification #2) is the decisive test; until it runs, "bounded label-lens
   variance holds without fracturing the shared model" is a HOUSE-STYLE bet, not a finding.
2. **The Axis-3 *numbers* in §2 are calibrated HOUSE STYLE judgement, not measured** — the
   *ordering* (diff/log/chat high; chip/identity/palette/inbox low; views-component mid) is
   well-grounded in J1/J2/J3, but the specific values (`0.85` vs `0.8`) are illustrative spacing for
   Phase-6 spread, not precision claims.
3. **The board↔roadmap "neither lens degraded" ruling (§2.1)** rests on ADR-06 (PROVEN one
   component) but whether *one views component* genuinely serves both *without* one lens feeling
   starved is the D5/R-16 question the per-segment tree-test (§Part 2.4) and Phase-6 dual-lens
   sketches must jointly answer.
4. **Sample sizes assume reachable per-segment recruits** — the corporate/governance segment
   (P11–P15) is the hardest to recruit at n≥30; §Part 2.4 lowers its tree-test floor to ≥30 and
   piggybacks R-19's regulated-buyer review, but this segment's confidence will lag the other two.

---

*End of R-07 deliverable. Date: 2026-06-20. Part 1 rulings HOUSE STYLE over PROVEN ADR-06/§5.6/§2
constraints; Part 2 methods PROVEN (NN/g card-sort/tree-test). Not user-validated — see Part 2.6.
Feeds sketch-funnel Axis 3, rubric D4/D5/D1/D7, Phase 5, Phase 6, R-16. Do not commit (orchestrator
handles git).*
