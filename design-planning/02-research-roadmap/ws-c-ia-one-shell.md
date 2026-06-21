# WS-C — IA & the "One Shell" Unification

> Workstream C (see [`README.md`](./README.md)). Where the central design problem ("one product, five
> surfaces") becomes a concrete structure: the unified object/navigation model that collapses five
> subsystems' object models into one coherent, deep-linkable shell — and the explicit study of *where
> unification yields to per-surface density and why*. Phase-1 methods #6 (IA design, ADOPT) and #7
> (card-sort/tree-test, ADAPT-defer).

---

## R-06 — Platform IA & the "one shell" unification model

**Questions answered.** How do `repo→PR→diff`, `space→page→block`, `channel→thread→message`,
`run→job→step`, `issue→sub-issue` collapse into ONE navigation/object model and ONE deep-linkable URL /
`ArtifactRef` structure? What is the labelling/taxonomy (incl. the persona-adaptive "issue" vs "work
item" vocabulary)? What lands each role by default (PM→roadmap, engineer→cycle board) without locking
anyone out?

**Phase-1 methodology.** #6 expert-led IA design (ADOPT). Builds ON design-language §5.1 (the shell) and
§7 (the catalogue-as-IA-inventory); does NOT re-derive them.

**Inputs.** design-language §5.1 (nav shell structure), §7 (full view catalogue), §2 (default-landing &
vocabulary), §5.3 (the `ArtifactRef` deep-link spine); `system-overview.md` §1–§2 (the three glue
contracts, one `ArtifactRef`); ADR-13 (`ArtifactRef` down to sub-artifact); R-04 (the flows the IA must
support).

**Deliverable.** `design-planning/04-research/ia/platform-ia.md`. The unified IA: the one
object/navigation model across subsystems; the primary-nav + contextual-sidebar + content + context-pane
structure as a *concrete* tree (not just the principle); the labelling scheme incl. the persona-adaptive
vocabulary candidates; the `myelin://…` / URL `ArtifactRef` structure down to sub-artifact granularity;
the per-role default-landing map. Labels kept in tokens/config so they're cheap to change (and
tree-testable later).

**Sequencing & dependencies.** Seq #6. Depends on R-01 (North-Star IA patterns) and R-04 (the flows the
IA serves). Foundational for R-07, R-08, R-10, R-18, and all of Phase 5/6.

**User-dependency.** none (expert-led IA); its **validation via card-sort/tree-test is deferred** to R-07.

**Effort.** L.

**Acceptance criteria.** Every §7 surface has a place in the unified tree; one `ArtifactRef` scheme
covers all five subsystems down to sub-artifact; the persona-adaptive vocabulary is proposed (with the
fracturing-risk flagged per §9 open question); default-landing per role is specified; labels are
config/token-held; the IA is structured to be tree-tested in Phase 4.

---

## R-07 — Unification-vs-distinctness study + card-sort/tree-test plan

**Questions answered.** Where on the unification↔distinctness spectrum should each surface sit — i.e.
where does "one skin everywhere" serve coherence and where does it starve a dense surface or suffocate a
calm one? What is the *rule* for when a surface earns per-surface density/identity vs. inherits the shared
default? And: how will we *validate* the IA with real users (the deferred study)?

**Phase-1 methodology.** #6 IA design (the study); #7 card-sorting + tree-testing (the **deferred**
validation plan — both participant-driven).

**Inputs.** R-06 (the IA to study/validate); design-language §2 (density adapts), P1 (coherence), P5
(earned density), §5.6 (the views component as the unification mechanism); R-03 (jobs → realistic
tree-test task scenarios); the central design problem statement (README §1).

**Deliverable.** `design-planning/04-research/ia/unification-study.md`. Two parts: **(1) the study** — a
per-surface ruling on where it sits on the unification↔distinctness axis, with the *rule* for earning
distinctness (e.g. "a diff earns its own density tier because <reason>; a roadmap earns its own pacing
because <reason>; both keep the shared chip/identity/palette"); this directly informs sketch-funnel Axis
3. **(2) the deferred validation plan** — a closed/hybrid card-sort design + a tree-test design over the
R-06 IA, with realistic task scenarios derived from R-03 jobs, run **per-segment** (engineer vs.
PM/corporate) to expose the dual-audience split, all clearly flagged deferred-until-users.

**Sequencing & dependencies.** Seq #7. Depends on R-06 (and R-03 for task scenarios). Sequenced **after**
the first JTBD work so tree-test tasks are grounded (resolves the README §5.8 circular dependency). Feeds
sketch-funnel Axis 3 and Phase 5.

**User-dependency.** none for the study; the **card-sort + tree-test are deferred-until-users** (carried
from README §5.8; run per-segment in Phase 4 before Phase 6 hardens the IA).

**Effort.** M.

**Acceptance criteria.** Every surface has a unification↔distinctness ruling with a stated *rule* (not a
case-by-case whim); the ruling feeds Axis 3 of the funnel; the card-sort + tree-test designs are
executable as-written with grounded tasks and per-segment runs; the deferred flag is explicit and the
"don't treat the IA as validated before this runs" caveat is recorded.
