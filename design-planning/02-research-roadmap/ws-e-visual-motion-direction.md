# WS-E — Visual & Motion Direction (incl. perceived performance, emotional tone, density-made-calm)

> Workstream E (see [`README.md`](./README.md)). The "loved" half of the mandate: a deliberate visual
> point of view, a motion/microinteraction language, and the perceived-performance + density-made-calm
> craft that make a dense product feel calm and instant. Build ON design-language §3 (token *direction*),
> §3.6 (motion), §8b.3/§8b.6 (the concrete craft mandates); do NOT re-derive the token tiers. Phase-1
> methods #13 (visual direction, HOUSE STYLE), #19 (heuristics), #24 (switch test), #8b (budgets).
> Mostly HOUSE STYLE — tag honestly.

---

## R-11 — Visual direction & mood-boards (3 directions, tone-words)

**Questions answered.** What 1–3 *intentional* visual directions should Phase 6 explore (so the 15→
finalists span aesthetic variety on purpose, not by accident)? What are the tone-words for each? How does
each reconcile the dual-audience visual tension (dense-but-calm engineer ↔ approachable-but-not-toylike
PM/corporate) and honour the §3 system constraints + the §8b.3 anti-aesthetic (no traffic-light fills,
no emoji-as-UI, no AI sparkle)?

**Phase-1 methodology.** #13 visual/aesthetic direction & mood-boarding (ADOPT, HOUSE STYLE).

**Inputs.** design-language §3 (neutral-led, accent-restrained, borders-over-shadow, the reserved `agent`
treatment), §8b.3 (anti-aesthetic + measured rules); `competitive-landscape.md` §1/§4 (Sourcehut-purism
↔ Notion-friendliness poles); `external-insights/05` §3/§4.

**Deliverable.** `design-planning/04-research/visual/visual-direction.md`. **Three** named visual
directions, each with: a mood-board / reference collage, tone-words, the §3 constraints it honours, how
it places on the sketch-funnel emotional-tone axis (Axis 4) and density axis (Axis 1), and the
anti-aesthetic it explicitly avoids. Each tagged HOUSE STYLE with the taste-debate-mitigation noted (ties
broken by P1–P9 + measured gates, README §5.6).

**Sequencing & dependencies.** Seq #3 (parallel-start track — no dependency; unblocks the funnel's tone/
density axes early). Feeds R-12, sketch-funnel Axes 1+4, Phase 6 token sets, Phase 8 framework look-fit.

**User-dependency.** none (taste is inherently un-user-validated now; flagged HOUSE STYLE).

**Effort.** M.

**Acceptance criteria.** Three genuinely distinct directions (not three shades of one); each tied to §3
constraints + tone-words + an axis position; the anti-aesthetic is explicit; every direction is
HOUSE-STYLE-tagged and the tie-break rule is referenced.

---

## R-12 — Motion, microinteractions & emotional tone language

**Questions answered.** What is the functional motion language (state-change communication, not
decoration) — durations, easing tokens, the agent-proposal motion, the live-event-update transition (a PR
going green) — at the §3.6 budget (≈120–200ms, interruptible)? Which microinteractions create *delight*
without violating calm (P8) or reduced-motion (§4)? How does motion carry emotional tone consistently
with R-11?

**Phase-1 methodology.** #13 (direction, extended to motion); #19 heuristics (does motion communicate
state or just decorate?); #8b motion budgets.

**Inputs.** design-language §3.6 (motion principles, reduced-motion first-class), §8b.6 ("pages render,
they don't animate in"); R-11 (the tone motion must match); R-01 (Linear's motion bar).

**Deliverable.** `design-planning/04-research/visual/motion-microinteractions.md`. The motion language:
named easing/duration tokens (DTCG-structured); the catalogue of functional motions (optimistic-settle,
card-moves-column, panel-open, live-update-transition, agent-proposal-appear/resolve); the
microinteraction set that earns delight (and the ones that *don't* — explicitly ruled out); the
reduced-motion equivalents as first-class (not degraded) paths. Each tagged PROVEN (where a perception/
a11y standard backs it) vs HOUSE STYLE.

**Sequencing & dependencies.** Seq #13. Depends on R-11 (tone) and R-08/R-09/R-10 (the components motion
animates). Feeds Phase 6 (D3 craft, D8 perceived-perf) and the rubric.

**User-dependency.** none.

**Effort.** M.

**Acceptance criteria.** Motion tokens are DTCG-structured and within the §3.6 budget; every motion
communicates a state change (no decoration); the agent-proposal and live-update motions are specced;
reduced-motion is a first-class path for every motion; delight microinteractions are named and the
anti-list is explicit; "pages render, they don't animate in" is honoured.

---

## R-13 — Perceived-performance & density-made-calm patterns

**Questions answered.** How does the UI *feel* instant under the residency constraint (no global CDN for
personal data, P2/ADR-11) — i.e. via optimistic UI, in-region edge, prefetch, and skeletons rather than
global replication? What are the skeleton/optimistic/rollback patterns per surface? And how is a *dense*
product kept *calm* (P5+P8) — the interaction philosophy of "shows a lot without shouting"?

**Phase-1 methodology.** #19 heuristics (visibility of system status); #24 switch test (the latency/
"feels finished" bar); #8b.6 hard latency budgets (keyboard <100ms, suppress flash-of-spinner <1s).

**Inputs.** design-language P2 (speed), P5 (earned density), P8 (calm), §8b.6 (budgets + skeleton/error
specifics); `external-insights/05` §4 (density-made-calm philosophy, optimistic+honest-rollback); R-10
(the components these patterns dress); the **prefetch / context-assembly** extension
([`extension-planning/perceived-performance.md`](../../extension-planning/perceived-performance.md)).

**Deliverable.** `design-planning/04-research/visual/perceived-performance.md`. Two halves: **(1)
perceived performance** — per-surface skeleton patterns (structure-matching, never a blank spinner),
optimistic-update + honest-rollback patterns, the prefetch/context-assembly UX (failing check → step →
line, pre-fetched), the latency-budget targets restated as design constraints. **(2) density-made-calm**
— the patterns that make dense surfaces calm: hierarchy from weight/colour before size, borders over
shadow, agent volume out of the main timeline, the one-prioritised-inbox discipline, restraint as a
default. Each tagged PROVEN/HOUSE STYLE.

**Sequencing & dependencies.** Seq #14. Depends on R-10 (components) and R-12 (motion). Cross-references
the perceived-performance extension. Feeds the rubric D7 (density-made-calm) and D8 (perceived-perf), and
the completeness-critic's loading/optimistic-rollback states.

**User-dependency.** none (latency-budget *measurement* binds in Phase 5; here it's the design pattern).

**Effort.** M.

**Acceptance criteria.** Per-surface skeleton + optimistic + rollback patterns specified; the prefetch/
context-assembly UX named and linked to its extension; latency budgets restated as constraints; the
density-made-calm patterns are concrete (not "be calm"); each pattern PROVEN/HOUSE-STYLE-tagged.
