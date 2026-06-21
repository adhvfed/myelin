# Phase 1 — Design & UX Methodologies for Myelin (Curated, Opinionated)

> Phase: `design-planning/01-methodologies`. Canonical inputs (read these, not just this doc):
> [`VISION.md`](../../VISION.md), [`planning/02-holistic-architecture/design-language.md`](../../planning/02-holistic-architecture/design-language.md)
> (principles P1–P9, the §7 view catalogue, the §8b day-one UX primitives — **already mature; we build ON it**),
> [`external-insights/05-ux-and-design.md`](../../external-insights/05-ux-and-design.md) (binding UX doctrine),
> [`planning/01-research/personas.md`](../../planning/01-research/personas.md) (P1–P15, A1–A5 — **hypotheses, no real research done**),
> [`planning/01-research/competitive-landscape.md`](../../planning/01-research/competitive-landscape.md),
> [`planning/01-research/agent-native-design.md`](../../planning/01-research/agent-native-design.md).
>
> Status date: **2026-06-20**. Honesty rule (VISION §3): every method below is tagged **PROVEN** (an
> established, evidenced method) or **HOUSE STYLE** (our taste / synthesis), and every method names its
> **deferred dependency on real-user validation** where it has one.

## 0. What this document is — and is not

This is **not** a survey of UX methods. It is a **decision**: the specific, opinionated set of methodologies
*we* will use to design Myelin, chosen through one hard lens and routed to the phases that will execute them.
For each method we say what it is (cited), why it fits *Myelin specifically* (tied to P1–P9 / personas /
doctrine — never generic), how we'll use it (which phase, which surface, which artifact), its effort and
proven-vs-taste tag, its risks, and a blunt **ADOPT / ADAPT / SKIP** verdict. SKIPs carry a reason so we
don't relitigate them.

The methods are grouped into thematic files:

| File | Covers |
|---|---|
| [`01-research-and-synthesis.md`](./01-research-and-synthesis.md) | No-user research & synthesis: JTBD-from-personas, comparative/competitive teardown, persona pressure-testing, assumption/risk mapping. |
| [`02-ia-and-flow-design.md`](./02-ia-and-flow-design.md) | Information architecture & interaction: IA design, card sorting / tree testing (deferred), service blueprinting, user-flow & job-flow mapping, the Double Diamond as the macro-frame. |
| [`03-design-system-and-visual.md`](./03-design-system-and-visual.md) | Design-system & visual direction: design tokens (W3C DTCG), atomic design, measured-token QA, visual/aesthetic direction & mood-boarding, the live styleguide. |
| [`04-agent-and-dual-audience.md`](./04-agent-and-dual-audience.md) | Agent-UX methods (Microsoft HAX, Google PAIR, NN/g AI patterns, plan-then-apply critique) and dual-audience / persona-adaptive design. |
| [`05-evaluation-and-qa.md`](./05-evaluation-and-qa.md) | Evaluation, critique & QA: heuristic evaluation, cognitive walkthrough, accessibility audit (WCAG 2.2 / EN 301 549), structured design critique, RITE, and the switch test as definition-of-done. |

## 1. The overarching design goal

**Build a product people *love* to use — not merely one they tolerate.** Myelin's whole thesis is that the
*integration* is the feature (VISION §1; competitive-landscape §6): five subsystems that feel like one fast,
calm, trustworthy product, where agents are legible first-class participants and EU-sovereignty/GDPR is *felt*
in the interface, not buried in settings. "Top-of-the-line UX" is a VISION §3 non-negotiable, and design must
precede code. This methodology set serves that goal under one brutal constraint — **we have no access to real
users right now** — by front-loading the methods that produce real signal *without* users (expert/heuristic
evaluation, comparative teardowns of the named North Stars, JTBD reasoned from personas, explicit design
principles + disciplined critique, accessibility audits, prototype-driven self-critique, and the doctrine's
"switch test"), while *scaffolding* the user-dependent methods so Phase 2 can turn them into a research
roadmap and Phase 4 can execute them the moment we have users. We optimise for *love* by treating speed,
calm, coherence, trust, and approachability (P1–P9) as testable criteria, not vibes.

## 2. The lens that decided what made the cut

Five filters, applied to every candidate method:

1. **Produces signal without real users?** We have none. Methods that need users to produce *any* value are
   ADAPTED to a no-user mode now + a deferred validation step, or SKIPped for this design effort.
2. **Serves the agent-native + dual-audience + sovereignty mandates?** Generic UX methods that ignore these
   three Myelin-defining pressures are deprioritised. We added agent-UX and persona-adaptive methods that a
   generic toolkit would omit.
3. **Feeds the downstream pipeline?** Phase 2 → research roadmap; Phase 4 → executes research; Phase 5 →
   maps user-facing surfaces; Phase 6 → 15 HTML design sketches; Phase 7 → judges them; Phase 8 → picks a
   visual framework. A method earns its place by producing an artifact a later phase consumes.
4. **Honest about proven vs. taste?** Mirrors VISION §3 / doctrine §3's tagging rule.
5. **Builds ON the existing design language, not over it.** The principles (P1–P9), the §7 view catalogue,
   and the §8b day-one primitives already exist and are mature. Methods that would re-derive them are SKIPped.

## 3. Recommendation summary table

| # | Methodology | Verdict | Powers which phase(s) | Proven/Taste | Needs real users later? |
|---|---|---|---|---|---|
| **Research & synthesis (no-user)** |||||
| 1 | Jobs-to-be-Done (JTBD) / Outcome-Driven framing, reasoned from personas | **ADAPT** | P2 (roadmap), P4 (validate), P5 (surface mapping) | Proven (theory) / Taste (no-user instantiation) | **Yes** — JTBD is normally interview-derived |
| 2 | Comparative / competitive teardown (Linear, Notion, Slack; anti-patterns Jira/Teams) | **ADOPT** | P2, P5, P6 (sketch references), P7 (judging rubric), P8 | Proven | No |
| 3 | Persona pressure-testing & proto-persona discipline | **ADAPT** | P2 (roadmap), P4 (replace with real personas) | Proven (proto-personas) | **Yes** — current personas are hypotheses |
| 4 | Assumption & risk mapping (uncertainty register) | **ADOPT** | P2 (the roadmap *is* this output) | House style (disciplined) | Partial |
| **IA & flow design** |||||
| 5 | Double Diamond as the macro-frame | **ADAPT** | All phases (orientation only) | Proven | No |
| 6 | Information architecture design (expert-led) | **ADOPT** | P5 (surface map), P6 (sketch IA) | Proven | Partial |
| 7 | Card sorting / tree testing | **ADAPT (defer)** | P2 (plan), P4 (run), validate P5/P6 IA | Proven | **Yes** — both need participants |
| 8 | Service blueprinting (cross-subsystem flows) | **ADOPT** | P5, P6 (key flows), P4 | Proven | Partial |
| 9 | User-flow / job-flow & jobs-story mapping | **ADOPT** | P5, P6 (the flows behind every sketch) | Proven | No |
| **Design system & visual** |||||
| 10 | Design tokens — W3C DTCG (2025.10 stable) three-tier | **ADOPT** | P6 (sketch tokens), P8 (framework), execution | Proven | No |
| 11 | Atomic design (component taxonomy) | **ADAPT** | P6, P8, execution | Proven | No |
| 12 | Measured-not-claimed token QA (contrast/spec gates) | **ADOPT** | P6, P7 (judging), P8, P5 (gate) | Proven (WCAG) | No |
| 13 | Visual/aesthetic direction & mood-boarding | **ADOPT** | P6 (before sketches), P8 (framework choice) | House style | Partial |
| 14 | Live styleguide rendered from real tokens | **ADOPT** | P8, execution | House style (doctrine) | No |
| **Agent-UX & dual-audience** |||||
| 15 | Microsoft HAX 18 Guidelines (Human-AI Interaction) | **ADOPT** | P5 (agent surfaces), P6 (agent sketches), P7 (rubric) | Proven (CHI 2019) | Partial |
| 16 | Google PAIR People+AI Guidebook (mental models, explainability, errors) | **ADAPT** | P5, P6 (agent flows), P4 (trust testing) | Proven | **Yes** — for trust calibration |
| 17 | NN/g agentic-UX patterns + plan-then-apply critique | **ADOPT** | P5, P6, P7 | Proven + House style | No |
| 18 | Dual-audience / persona-adaptive design method ("one component, many lenses") | **ADOPT** | P5, P6, P7 (rubric) | House style (synthesis) | **Yes** — needs both audiences to validate |
| **Evaluation, critique & QA** |||||
| 19 | Heuristic evaluation (Nielsen's 10 + a Myelin P1–P9 heuristic set) | **ADOPT** | P6 (self-crit), P7 (the core judging method), execution | Proven | No |
| 20 | Cognitive walkthrough (learnability, no users) | **ADOPT** | P6, P7, P4 | Proven | No |
| 21 | Accessibility audit — WCAG 2.2 AA / EN 301 549 / EAA | **ADOPT** | P6, P7, P5 (CI gate), P8, execution | Proven (legal) | Partial (AT user testing later) |
| 22 | Structured design critique (framed, criteria-anchored) | **ADOPT** | P6, P7 (judging format) | Proven | No |
| 23 | RITE (Rapid Iterative Testing & Evaluation) | **ADAPT (defer)** | P4 (with users), execution | Proven | **Yes** — RITE is user-test-driven |
| 24 | The switch test (drive-the-real-UI definition-of-done) | **ADOPT** | P5 (DoD), P7, execution | Proven-by-doctrine | Partial |

**Notable SKIPs (with reasons, so we don't relitigate — detail in the thematic files):**
- **A/B testing, multivariate testing, analytics-driven optimisation** — SKIP for design phases: no live
  product, no traffic, no users. Belongs to post-launch, not Phase 1–8.
- **Surveys / quantitative ODI need-importance ranking** — SKIP now: requires a respondent pool we don't
  have; the *qualitative* JTBD framing is ADAPTED instead. Re-open in Phase 4.
- **Diary studies, contextual inquiry, ethnographic field research** — SKIP now (no users); flag as the
  *highest-value* deferred methods for Phase 4 because they would most de-risk the persona hypotheses.
- **Crazy-8s / Design-Studio mass-ideation as a *primary* method** — SKIP as a named ceremony: with a single
  agent author and a mature design language, structured critique + teardown produce better signal than
  divergent group sketching. (Phase 6 still *generates* 15 sketches; that breadth substitutes for it.)
- **Kano model** — SKIP now: needs survey data to classify features; revisit in Phase 4 if prioritisation
  needs it.

## 4. How these methodologies should be utilised across the upcoming phases

This is the hand-off Phase 2 turns into a roadmap. Read it as "what runs when, and what it outputs."

- **Phase 2 (research roadmap).** Consumes this whole document. Turn methods #1, #3, #7, #16, #23 (the
  user-dependent ones) into a sequenced **research backlog** with the explicit "deferred until users"
  flags carried verbatim. Turn method #4 (assumption/risk mapping) into the roadmap's spine — the open
  questions in §5 below are its seed. Decide *which* personas (P1–P15) get validated first (recommend: P6
  PM vs P1 engineer dual-audience tension, and P13 DPO sovereignty legibility — the two highest-risk bets).
- **Phase 4 (executes research).** Runs the deferred user methods: real JTBD interviews (#1), real-persona
  replacement (#3), card sorting + tree testing on the Phase-5 IA (#7), PAIR-style trust testing of agent
  surfaces (#16), and RITE loops on the Phase-6 sketches (#23). Also runs accessibility *assistive-tech*
  user testing (#21) that the audit alone can't cover.
- **Phase 5 (maps user-facing surfaces).** Driven by IA design (#6), service blueprinting (#8), job-flow
  mapping (#9), and JTBD framing (#1) over the §7 view catalogue. The switch test (#24) is written here as
  the per-surface **definition-of-done**. Accessibility (#21) and measured-token (#12) gates are specified
  here so they bind in Phase-5 testing strategy.
- **Phase 6 (15 HTML design sketches).** Every sketch is built against: tokens (#10/#12), atomic component
  taxonomy (#11), visual direction/mood-board (#13), the dual-audience method (#18), HAX/PAIR/NN-g agent
  patterns (#15–#17) on agent surfaces, and the §5.10 empty/loading/error state checklist. Each sketch is
  self-critiqued with heuristic evaluation (#19) and cognitive walkthrough (#20) before submission.
- **Phase 7 (judges the sketches).** The judging rubric **is** methods #19 (Myelin P1–P9 heuristics) +
  #22 (structured critique format) + #12 (measured-token pass/fail) + #21 (accessibility pass/fail) +
  #15/#18 (agent + dual-audience scoring) + #24 (would-this-survive-the-switch-test). #2 (teardown) supplies
  the North-Star comparison baseline.
- **Phase 8 (picks a visual framework).** Decided against: token portability (#10 DTCG compliance), accessible
  primitive maturity (#21), the live-styleguide requirement (#14), and atomic-component fit (#11). The
  framework must make coherence (P1) mechanical and accessibility (§4) inherited.

## 5. Uncertainties & open questions (hand-off to Phase 2)

Honest about uncertainty (VISION §3). These are the open questions this methodology selection raises — Phase 2
turns them into the research roadmap.

1. **The personas are unvalidated (the load-bearing risk).** Every no-user method here *reasons from* P1–P15.
   If the persona hypotheses are wrong, the JTBD jobs, the dual-audience balance, and the prioritisation are
   all wrong. Phase 2 must rank which personas to validate first; we nominate the **P6-PM vs P1-engineer
   dual-audience tension** (the single hardest UX mandate, design-language §2) and the **P13-DPO/P12-security
   sovereignty-legibility** bet (P9) as first to test.
2. **Which JTBD jobs are real vs. assumed?** We can *write* jobs-stories from personas now, but the
   *importance × satisfaction* ranking that makes JTBD decisive is interview/survey-derived. Open: do we run
   qualitative JTBD interviews (Phase 4) before or in parallel with the first sketches?
3. **Can expert evaluation alone clear "top-of-the-line"?** Heuristic evaluation by ~3 evaluators catches
   ~60% of issues (NN/g); the switch test catches more but is still expert-driven. Open: how much real
   usability testing (Phase 4) is *required* before we trust a surface, vs. nice-to-have?
4. **Agent-UX has no settled canon.** HAX/PAIR predate the current agent wave; NN/g's agentic patterns are
   2025–2026 and still moving. Our plan-then-apply / HITL contract (design-language §6) is more specific than
   any public guideline. Open: how much do we trust external agent-UX guidance vs. our own doctrine, and what
   would falsify our HITL-card design without real agents (we ship mocks)?
5. **Dual-audience validation needs both audiences.** "One component, many lenses" (#18) is our synthesis,
   not a proven external method. We can critique it, but only PMs *and* engineers using the same surface
   prove it. Deferred to Phase 4.
6. **Visual direction is inherently taste-laden.** #13 (mood-boarding/direction) and the Phase-6/7/8 visual
   choices are HOUSE STYLE. Open: what is the decision rule when judges (Phase 7) disagree on aesthetics —
   whose taste wins, and against which written criteria? (We propose: P1–P9 + measured gates decide; pure
   aesthetics break ties only.)
7. **Sovereignty-as-UX has no playbook.** Making GDPR/residency *legible* (P9) is novel; no external method
   covers it. We treat it via heuristic + service-blueprint of the DSR/residency consoles, but it is
   under-evidenced. Open: is there a regulated-buyer (P13/P14) review we can substitute for user testing?
8. **Card sorting / tree testing assume content we haven't finalised.** The §7 catalogue gives a candidate IA;
   tree testing it (Phase 4) needs realistic task scenarios derived from the (unvalidated) jobs. Circular
   dependency to sequence in Phase 2.

---

### Sources (web-verified, 2026-06)
- JTBD / Outcome-Driven Innovation (Ulwick / Strategyn): https://strategyn.com/jobs-to-be-done/ · https://jobs-to-be-done.com/
- Nielsen's 10 heuristics & heuristic evaluation (NN/g): https://www.nngroup.com/articles/how-to-conduct-a-heuristic-evaluation/
- Double Diamond (Design Council; Framework for Innovation 2019): https://en.wikipedia.org/wiki/Double_Diamond_(design_process_model)
- Design Tokens Format Module 2025.10 (first stable, DTCG/W3C): https://www.designtokens.org/tr/2025.10/format/ · https://www.w3.org/community/design-tokens/2025/10/28/design-tokens-specification-reaches-first-stable-version/
- Microsoft HAX Toolkit — 18 Guidelines for Human-AI Interaction: https://www.microsoft.com/en-us/haxtoolkit/ai-guidelines/
- Google PAIR People + AI Guidebook: https://pair.withgoogle.com/guidebook/
- WCAG 2.2 / EN 301 549 / European Accessibility Act (enforceable 2025-06-28): https://en.wikipedia.org/wiki/EN_301_549 · https://www.levelaccess.com/blog/eu-accessibility-requirements-and-eaa-compliance/
- Card sorting vs. tree testing (NN/g): https://www.nngroup.com/articles/card-sorting-tree-testing-differences/
- RITE method: https://en.wikipedia.org/wiki/RITE_Method · https://www.uxmatters.com/mt/archives/2024/11/testing-digital-products-the-rite-way.php
- Design critiques (NN/g): https://www.nngroup.com/articles/design-critiques/
