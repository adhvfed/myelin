# Phase 2 — The Myelin Design/UX Research Roadmap

> Phase: `design-planning/02-research-roadmap`. Builds directly on Phase 1
> ([`../01-methodologies/`](../01-methodologies/) — the README §3 recommendation table, §4 per-phase
> utilisation, and §5 open questions are this roadmap's seed). Canon: [`VISION.md`](../../VISION.md);
> [`planning/02-holistic-architecture/design-language.md`](../../planning/02-holistic-architecture/design-language.md)
> (P1–P9, §2 dual-audience, §3 tokens, §4 a11y/i18n, §5 shared components, §6 agent UX, §7 view
> catalogue, §8b day-one primitives — **mature; we build ON it, never re-derive it**);
> [`external-insights/05-ux-and-design.md`](../../external-insights/05-ux-and-design.md);
> [`planning/01-research/personas.md`](../../planning/01-research/personas.md) (P1–P15, A1–A5 —
> HYPOTHESES, no real users);
> [`planning/01-research/competitive-landscape.md`](../../planning/01-research/competitive-landscape.md);
> [`planning/01-research/agent-native-design.md`](../../planning/01-research/agent-native-design.md).
>
> Status date: **2026-06-20**. Honesty rule (VISION §3): findings are tagged **PROVEN** (evidenced /
> a cited standard) or **HOUSE STYLE** (our taste / synthesis); user-dependent work is flagged
> **deferred-until-users** and planned now, run later. The flags are carried verbatim from Phase 1.

---

## 0. What this document is

This is the **research roadmap**: a sequenced plan for building a deep, reusable **corpus** of
design/UX research that later phases draw on to make Myelin a product people *love*. It does three
things:

1. **Restates the lovability goal and the central design problem** (§1) and explains how the corpus
   feeds the downstream phases and the final human decision (§2).
2. **Defines the workstreams, the master sequencing, and the full research-item index** (§3–§5) — the
   exact order Phase 4 executes items so each learns from the prior, plus the deferred-until-users
   track (§6) and the corpus definition-of-done (§7).
3. **Anchors the cross-phase control artifacts** the revised process needs — a pre-registered judging
   rubric ([`rubric.md`](./rubric.md)), the sketch-funnel plan with named axes of variation
   ([`sketch-funnel.md`](./sketch-funnel.md)), and the consolidated technical extensions
   ([`extension-planning/`](../../extension-planning/)) — and ends with a deliberate **completeness-critic
   pass** (§9) naming the unglamorous states the pipeline will otherwise skip.

It does **not** produce the research itself (that is Phase 4), write any sketches (Phase 6), or judge
anything (Phase 7). Every research item is right-sized so a fresh-context agent can execute it.

---

## 1. The lovability goal and the central design problem

**The goal: a product people love, not merely tolerate.** "Top-of-the-line UX" is a VISION §3
non-negotiable; Myelin's whole thesis is that the *integration is the feature* — five subsystems that
feel like one fast, calm, trustworthy product, where agents are legible first-class participants and
EU-sovereignty/GDPR is *felt* in the interface (design-language P1–P9). Lovability is treated as a set
of *testable* properties — speed, calm, coherence, trust, approachability, craft — not vibes.

**The central design problem (named, first-class, carried through the whole roadmap and rubric):**

> **"One product, five surfaces."** Git, CI, issues, knowledge, and chat must feel like ONE system —
> one shell, one identity, one command palette, one reference chip, one editor, one views component —
> while allowing appropriate **per-surface density** (a diff is dense; a roadmap breathes). This is the
> **unification-vs-distinctness tension**: too unified and a dense engineer surface is starved or a
> calm PM surface is cluttered; too distinct and the product fractures back into the stitched-together
> Atlassian feel (competitive-landscape §6) that is the exact failure we exist to beat.

Every workstream below is in service of resolving that tension *concretely* — IA proves one shell;
interaction-pattern research proves one chip/palette/card; visual & motion direction proves one skin
that tunes by density; cross-surface flows prove the seams disappear; the rubric scores "one-product
coherence" and "density-made-calm" as named dimensions; the sketch funnel makes *surface unification*
an explicit axis of deliberate variation so the final human can choose where on the spectrum Myelin
should sit.

**Three audiences, not two.** Phase 1's dual-audience framing (engineers vs. PM/corporate) is widened
here, per the steer, to **three** for research purposes: **engineers** (P1–P5), **PMs/delivery**
(P6–P10), and **corporate/governance** (P11–P15 — the CTO, security, DPO, procurement, admin
gatekeepers who *decide adoption* and for whom sovereignty/audit legibility is the product). JTBD and
flows are authored for all three.

---

## 2. How the corpus feeds Phases 5–8 and the final human decision

The revised pipeline runs **fully autonomously until a single human review at the end**, where a human
**decides and may override** the recommended direction. Everything is therefore optimised for a
**decision-ready spread**: easy to choose between, cheap to overrule. The corpus is the shared evidence
base that keeps that final choice defensible rather than arbitrary.

| Phase | What it does | What it consumes from this corpus |
|---|---|---|
| **Phase 4** | Executes the research items below, producing the corpus under `design-planning/04-research/<area>/`. | This roadmap (the backlog + sequencing); the Phase-1 methods. Runs deferred-until-users items when users exist. |
| **Phase 5** | Maps the user-facing surfaces (IA, flows, per-surface definition-of-done) over the §7 catalogue. | North-Star teardowns (the bar to clear), JTBD & cross-surface flows (what each surface finishes), the IA/one-shell study, the interaction-pattern specs, the state/edge-case checklist (§9). |
| **Phase 6** | The two-stage sketch funnel ([`sketch-funnel.md`](./sketch-funnel.md)): diverge cheap → cull → deepen 3–4 finalists into multi-screen mini-systems with tokens. | The axes of variation, the visual/motion direction, the interaction-pattern library, the dual-audience method, the agent-UX patterns, the rubric (sketches are designed TOWARD it). |
| **Phase 7** | Judges the finalists with the multi-lens panel. | [`rubric.md`](./rubric.md) (the pre-registered, binding judging instrument) and the teardown comparison baseline. |
| **Phase 8** | Picks the visual framework. | Token portability findings, accessible-primitive maturity, the live-styleguide requirement, the interaction-pattern feasibility notes. |
| **Final human decision** | Reviews the decision-ready spread; decides; may override. | The whole corpus + the rubric scores + the funnel's deliberately-spread finalists (so picking a non-recommended finalist is **not a restart**). |

The roadmap also produces [`extension-planning/`](../../extension-planning/): the concrete backend/technical
extensions a *lovable* product implies that the existing architecture does not already cover, stated
clearly enough to become implementation tasks later — so lovability is not silently blocked by a missing
substrate.

---

## 3. The workstreams

The backlog is grouped into ten workstream files. Each file holds its research items with full execution
detail (questions, methodology, inputs, deliverable path + contents, dependencies, user-dependency flag,
effort, acceptance criteria).

| WS | File | Covers | Items |
|---|---|---|---|
| **WS-A** | [`ws-a-north-star-teardowns.md`](./ws-a-north-star-teardowns.md) | Deep teardowns of the named North Stars + trap audit; the comparative judging baseline. | R-01, R-02 |
| **WS-B** | [`ws-b-jtbd-and-flows.md`](./ws-b-jtbd-and-flows.md) | JTBD for the THREE audiences; named cross-surface task flows; persona pressure-test & validation-priority. | R-03, R-04, R-05 |
| **WS-C** | [`ws-c-ia-one-shell.md`](./ws-c-ia-one-shell.md) | Platform IA & the "one shell" unification; the unification-vs-distinctness study; card-sort/tree-test plan. | R-06, R-07 |
| **WS-D** | [`ws-d-interaction-patterns.md`](./ws-d-interaction-patterns.md) | The interaction-pattern library: command palette, reference chip/unfurl, agent/HITL card, views component, editor, notifications inbox, overlays. | R-08, R-09, R-10 |
| **WS-E** | [`ws-e-visual-motion-direction.md`](./ws-e-visual-motion-direction.md) | Visual direction & mood-boards; motion & microinteractions; perceived-performance; emotional tone; density-made-calm. | R-11, R-12, R-13 |
| **WS-F** | [`ws-f-agent-ux.md`](./ws-f-agent-ux.md) | Agent-UX patterns: legibility, plan-then-apply/HITL trust, attribution/audit, calm agent volume. | R-14, R-15 |
| **WS-G** | [`ws-g-dual-audience.md`](./ws-g-dual-audience.md) | Dual-/tri-audience persona-adaptive design: one component, many lenses; vocabulary; density adaptation. | R-16 |
| **WS-H** | [`ws-h-accessibility-i18n.md`](./ws-h-accessibility-i18n.md) | Accessibility audit method (WCAG 2.1/2.2 AA, EN 301 549) + i18n/l10n/RTL pattern research. | R-17, R-18 |
| **WS-I** | [`ws-i-sovereignty-as-ux.md`](./ws-i-sovereignty-as-ux.md) | Sovereignty-as-UX: residency/GDPR/DSR/audit legibility patterns. | R-19 |
| **WS-J** | [`ws-j-onboarding-and-craft.md`](./ws-j-onboarding-and-craft.md) | Onboarding/first-run delight; empty/loading/error/permission/erased craft; the cross-artifact "wedge" moments. | R-20, R-21, R-22 |

**Twenty-two items** total (R-01…R-22). Lovability/craft is explicitly represented: R-12 (motion &
microinteractions), R-13 (perceived performance & density-made-calm), R-20 (first-run/onboarding
delight), R-21 (empty/loading/error craft), R-22 (the cross-artifact "wedge" moments), R-14/R-15 (agent
legibility & trust).

---

## 4. Master sequencing

The exact order Phase 4 runs items so each learns from the prior. **Foundational items first**
(teardowns, JTBD/flows, IA, visual/motion direction), then per-surface and agent items that depend on
them, then the craft and sovereignty items that consume the patterns.

**One-line sequence:**
`R-01→R-02 (teardowns) ∥ R-03→R-04 (JTBD→flows) ∥ R-11 (visual direction) → R-05 (persona pressure-test) → R-06→R-07 (IA + unification study) → R-08→R-09→R-10 (interaction patterns) → R-16 (dual-audience) ∥ R-14→R-15 (agent-UX) → R-12→R-13 (motion + perceived-perf) → R-17→R-18 (a11y + i18n/RTL) → R-19 (sovereignty-UX) → R-20→R-21→R-22 (onboarding + state craft + wedge moments).`

The three foundational tracks (teardowns; JTBD→flows; visual direction) can run in **parallel** at the
start — they have no mutual dependency and each unblocks the middle band. Everything downstream of the
IA + interaction-pattern band is sequential because the patterns are shared substrate the later items
critique against. See the `Seq #` column in §5 for the precise numbering and each item's stated
dependencies.

---

## 5. The research-item index

Every item, one row. Deliverable paths are written for Phase 4 to populate under
`design-planning/04-research/<area>/`. User-dependency: **none** (runs now) or **deferred** (planned
now, run when users exist). Effort: **S / M / L**. Full detail is in the workstream files.

| ID | Title | WS | Phase-1 method(s) | Deliverable path | User-dep | Effort | Seq # |
|---|---|---|---|---|---|---|---|
| **R-01** | North-Star teardown dossier (Linear · Notion · Slack · GitHub) | A | #2 teardown | `04-research/north-star/teardown-dossier.md` | none | L | 1 |
| **R-02** | Trap / anti-pattern audit (Jira · Atlassian · Teams) | A | #2 teardown; #19 heuristics | `04-research/north-star/trap-audit.md` | none | M | 2 |
| **R-03** | JTBD catalogue for the three audiences | B | #1 JTBD; #3 proto-persona | `04-research/jtbd-flows/jtbd-catalogue.md` | none (ranking deferred) | M | 3 |
| **R-04** | Named cross-surface task flows (service blueprints + job flows) | B | #8 blueprint; #9 job-flow | `04-research/jtbd-flows/cross-surface-flows.md` | none | L | 4 |
| **R-05** | Persona pressure-test & validation-priority register | B | #3 proto-persona; #4 assumption-map | `04-research/jtbd-flows/persona-pressure-test.md` | none | S | 5 |
| **R-06** | Platform IA & the "one shell" unification model | C | #6 IA design | `04-research/ia/platform-ia.md` | none | L | 6 |
| **R-07** | Unification-vs-distinctness study + card-sort/tree-test plan | C | #6 IA; #7 card-sort/tree-test (defer) | `04-research/ia/unification-study.md` | none (study) / **deferred** (validation) | M | 7 |
| **R-08** | Command palette + search-find interaction spec | D | #2 teardown; #19/#20; #8b | `04-research/interaction/command-palette.md` | none | M | 8 |
| **R-09** | Reference chip + artifact unfurl interaction spec (the wedge component) | D | #2 teardown; #9 job-flow; #8b | `04-research/interaction/reference-unfurl.md` | none | L | 9 |
| **R-10** | Shared interaction patterns: views, editor, notifications inbox, overlays | D | #11 atomic; #8b primitives; #19 | `04-research/interaction/shared-patterns.md` | none | L | 10 |
| **R-11** | Visual direction & mood-boards (3 directions, tone-words) | E | #13 visual direction | `04-research/visual/visual-direction.md` | none | M | 3 (parallel) |
| **R-12** | Motion, microinteractions & emotional tone language | E | #13 direction; #19 heuristics; #8b motion | `04-research/visual/motion-microinteractions.md` | none | M | 13 |
| **R-13** | Perceived-performance & density-made-calm patterns | E | #19 heuristics; #24 switch test; #8b budgets | `04-research/visual/perceived-performance.md` | none | M | 14 |
| **R-14** | Agent legibility & the plan-then-apply/HITL trust pattern set | F | #15 HAX; #17 NN/g+§6 critique | `04-research/agent-ux/legibility-and-hitl.md` | none | L | 11 |
| **R-15** | Agent attribution/audit + calm-agent-volume patterns; trust-calibration plan | F | #16 PAIR; #15 HAX; #17 | `04-research/agent-ux/attribution-and-calm.md` | none (study) / **deferred** (trust-calibration) | M | 12 |
| **R-16** | Dual-/tri-audience persona-adaptive design study | G | #18 dual-audience; #1 JTBD | `04-research/dual-audience/persona-adaptive.md` | none (study) / **deferred** (both-audience validation) | M | 12 |
| **R-17** | Accessibility audit method & per-surface a11y checklist | H | #21 a11y audit; #12 measured tokens | `04-research/accessibility/audit-method.md` | none (audit) / **deferred** (AT-user testing) | M | 15 |
| **R-18** | i18n / l10n / RTL interaction-pattern research | H | #21 a11y; #6 IA | `04-research/accessibility/i18n-rtl-patterns.md` | none | M | 16 |
| **R-19** | Sovereignty-as-UX: residency/GDPR/DSR/audit legibility patterns | I | #8 blueprint; #15 HAX; #19 | `04-research/sovereignty/sovereignty-as-ux.md` | none (patterns) / **deferred** (regulated-buyer review) | M | 17 |
| **R-20** | First-run / onboarding delight patterns (3 archetypes) | J | #20 cognitive walkthrough; #2 teardown | `04-research/craft/onboarding-delight.md` | none | M | 18 |
| **R-21** | Empty / loading / error / permission / erased state craft | J | #9 job-flow; #19; #8b states | `04-research/craft/state-craft.md` | none | M | 19 |
| **R-22** | The cross-artifact "wedge" moments (delight at the seams) | J | #8 blueprint; #9 job-flow | `04-research/craft/wedge-moments.md` | none | M | 20 |

---

## 6. The deferred-until-users track (planned now, run later)

These items (or sub-parts of items) genuinely need real users and are **planned now, executed in Phase 4
when users exist**. The flags are carried verbatim from Phase 1 (README §3 col. "Needs real users
later?", §4 Phase-4 routing, §5 open questions). Do **not** drop them; do **not** let any downstream
phase treat their no-user substitutes as validated.

| Deferred item | Belongs to | Phase-1 source | Why it needs users | No-user substitute running now |
|---|---|---|---|---|
| **JTBD importance × satisfaction ranking** (the decisive ODI core) | R-03 | method #1; README §5.2 | The importance ranking that prevents building the wrong thing is interview/survey-derived. | Hypothesis-tagged jobs-story catalogue (R-03). |
| **Real-persona replacement** (replace proto-personas) | R-05 | method #3; README §5.1 | Current P1–P15 are hypotheses; the load-bearing risk. | Persona pressure-test + validation-priority register (R-05). |
| **Card sorting + tree testing the IA** (run per-segment) | R-07 | method #7; README §5.8 | Both are participant-driven; the dual-audience findability test. | Expert-led IA + the planned study design (R-07). |
| **PAIR-style agent trust-calibration testing** | R-15 | method #16; README §5.4 | Do users correctly understand agent capability/limits and when to trust? | The §6-contract critique + HAX audit (R-14/R-15). |
| **Both-audience validation of "one component, many lenses"** | R-16 | method #18; README §5.5 | Only PMs *and* engineers using the same surface prove it holds. | Per-lens critique against each persona (R-16). |
| **Assistive-technology user testing** (beyond the audit) | R-17 | method #21; README §5.3 | Automated tools catch ~30–40%; AA ≠ usable-with-AT. | Manual expert a11y audit + measured tokens (R-17). |
| **RITE loops on the Phase-6 sketches/finalists** | (cross) | method #23 | RITE produces nothing without participants. | Heuristic eval + cognitive walkthrough + switch test as the no-user substitute. |
| **Regulated-buyer (P13/P14) review of sovereignty consoles** | R-19 | README §5.7 | Sovereignty-as-UX has no playbook; a DPO/procurement review substitutes for user testing. | Expert blueprint + heuristic audit (R-19). |

---

## 7. Corpus definition-of-done

The Phase-2 corpus plan is "done" when all of the following hold (this is the bar Phase 4's *output*
must clear, restated here so the target is fixed before work starts):

1. **Every R-item has a file at its stated path**, each tagged PROVEN/HOUSE STYLE per claim and dated.
2. **The three audiences are covered** — JTBD (R-03) names jobs for engineers, PMs/delivery, and
   corporate/governance; at least one named cross-surface flow (R-04) is authored per audience.
3. **The "one product, five surfaces" problem is answered concretely** — the IA (R-06) shows the one
   shell; the interaction specs (R-08/R-09/R-10) show the shared components; the unification study
   (R-07) names where unification yields to per-surface density and *why*.
4. **Every primary §7 view is reachable from a flow or a pattern spec** — no catalogued surface is
   orphaned with no research behind it.
5. **The lovability/craft items exist and are concrete** — motion (R-12), perceived-perf &
   density-made-calm (R-13), onboarding (R-20), state craft (R-21), wedge moments (R-22), agent
   legibility/trust (R-14/R-15).
6. **The hard gates are specified, not assumed** — the a11y method (R-17) and i18n/RTL research (R-18)
   give Phase 6/7 a *checkable* checklist that the rubric's hard gates point to.
7. **Every deferred-until-users item is recorded** with its no-user substitute and its trigger
   condition; none is silently dropped.
8. **The completeness-critic list (§9) is addressed** — Phase 4/5/6 each show how they cover (or
   consciously defer) every named gloss-risk.
9. **The control artifacts are usable as-is** — [`rubric.md`](./rubric.md) is scoreable without further
   input; [`sketch-funnel.md`](./sketch-funnel.md) tells Phase 6 exactly what to produce; the
   [`extension-planning/`](../../extension-planning/) deltas are stated as implementable tasks.

---

## 8. Cross-phase control artifacts (authored in this phase)

- **[`rubric.md`](./rubric.md)** — the **pre-registered, binding** judging rubric. Written BEFORE any
  sketch exists; Phase 6 sketches are designed *toward* it; Phase 7 judges *against* it. Hard pass/fail
  gates (accessibility; i18n/l10n/RTL), weighted scored dimensions mapped to judge lenses, aggregation
  and tie-break rules.
- **[`sketch-funnel.md`](./sketch-funnel.md)** — the two-stage funnel plan with **named axes of
  variation** (density; navigation paradigm; surface unification; emotional tone; + others), the diverge
  count, the cull-for-merit-AND-spread rule, the deepen spec, and the comparable screen set every
  finalist must include.
- **[`extension-planning/`](../../extension-planning/)** — consolidated, decision-ready technical extensions
  ([`extension-planning/README.md`](../../extension-planning/README.md) is the index): the genuine
  backend/technical gaps a lovable product implies, cross-checked against the existing `planning/`
  architecture so we flag deltas, not re-statements.

---

## 9. Completeness-critic note (the deliberate gloss-risk pass)

Autonomous pipelines reliably skip the unglamorous states. The following are surfaces, UI states,
edge-case flows, accessibility cases, and empty/error/loading states that this roadmap could otherwise
cause Phase 4/5/6 to gloss. **They are named here so later phases MUST cover them or consciously defer
them with a reason.** (Many map to a specific R-item; where they do, the item must include them in its
acceptance criteria.)

**Unglamorous UI states (route to R-21, enforce in every Phase-6 finalist):**
- **Loading that shows structure** — skeletons matching the *final* layout, never a spinner on a blank
  page (design-language §8b.6; the #1 perceived-perf tell).
- **Empty states that are onboarding-forward** — first repo / first issue / first doc / first channel /
  first agent run; the *zero-data* shell (the startup persona P1 lands here first).
- **Error that blames the system in one quiet line + a path** — never a dead end, never blame the user.
- **Permission-denied as a graceful "no access" card** — never a leaked title (the GDPR/ADR-03
  correctness invariant surfaced as UX; the reference chip/unfurl must show this state).
- **Erased / tombstoned** — the GDPR-aware degraded state for deleted/erased artifacts; the chip, the
  unfurl, the backlink, and the search result each need it.
- **Agent-pending** — "an agent is working / awaiting your approval"; the durable-gate-waiting state.
- **Degraded-surface "temporarily unavailable"** — a single surface fails *static* without taking the
  shell down (the cell/region-degradation case).
- **Stale / offline / reconnecting** — what a live surface shows when the firehose transport drops and
  resumes (chat, presence, live CI log, collaborative editing).
- **Optimistic-update rollback** — the honest-failure state when an optimistic write is rejected
  server-side (the "optimism for latency, honesty on failure" promise).
- **Conflict surfacing** — concurrent-edit conflict on issue fields / doc blocks (the CAS→CRDT path)
  shown legibly, not a silent overwrite.

**Edge-case & cross-surface flows the happy-path bias will skip (route to R-04/R-22):**
- The **partial-failure** branches of agent flows (gate rejected; agent errors mid-chain; budget
  exceeded; loop-guard tripped) — not just the approve-happy-path.
- The **cross-cell / cross-tenant reference** that resolves to "no access" or a tombstone (R-09).
- The **diff-anchored comment that relocates or orphans** after a rebase (design-language §5.5; the
  content-anchored resolver).
- The **storm / 30×-agent-surge** notification experience — what the inbox looks like under load, not
  just at rest (R-21 + extension-planning notification semantics).
- The **DSR/erasure flow seen from the data subject's side AND the DPO's side** (R-19).

**Accessibility cases that get glossed (route to R-17, enforce in the rubric hard gate):**
- **Keyboard-only operability of the *hard* components** — the diff, the board drag, the views table
  inline-edit, the block editor, the HITL approval card, the command palette, nested overlays.
- **Screen-reader announcement of live/event-driven updates** (a PR going green, an agent proposal
  arriving) *without spamming* the live region.
- **Visible focus on every surface in every theme** (light / dark / high-contrast) — the focus-ring
  token, not the identity token (§8b.3).
- **Status-not-by-colour-alone** across CI green/red, PR states, SLA breach, agent treatment.
- **200% zoom / reflow** on dense surfaces (diff, table, log) without loss; reduced-motion as a
  first-class path, not a degraded one.
- **RTL mirroring of the *whole* shell** including the editor, the views component, and overlays —
  tested with a real RTL string, not a flipped mockup (R-18 + rubric hard gate).

**Device / form-factor glosses (route to R-13/R-21, design-language §8b.4):**
- **Touch / mobile**: hover-only row actions are invisible on touch; full-width panels laid out beside a
  still-present column clip off-screen; popovers anchored under a bottom-pinned composer render
  off-screen and must flip.
- **The CLI as a peer surface** (design-language §7.7) — its error states and reference rendering must
  follow the same vocabulary; easy to forget it is "in scope for consistency."

**Process gloss-risk:** the autonomous funnel will tend to converge early on one instinct. The named
**axes of variation** in [`sketch-funnel.md`](./sketch-funnel.md) and the **cull-for-spread** rule exist
specifically to force the divergence; Phase 6 must not collapse the finalists into four near-duplicates,
and Phase 7's tie-break must not let pure aesthetics override the hard gates.

---

## 10. Cross-references

- Phase 1 methodologies: [`../01-methodologies/README.md`](../01-methodologies/README.md) (§3 table, §4
  routing, §5 open questions) and the five thematic files.
- [`VISION.md`](../../VISION.md) §3 (top-tier UX, agent-native, GDPR/sovereign non-negotiables;
  design-before-code).
- [`planning/02-holistic-architecture/design-language.md`](../../planning/02-holistic-architecture/design-language.md)
  — the mature design language this roadmap builds ON (P1–P9, §2, §3, §4, §5, §6, §7, §8b).
- [`rubric.md`](./rubric.md), [`sketch-funnel.md`](./sketch-funnel.md),
  [`extension-planning/README.md`](../../extension-planning/README.md) — the control artifacts.
