# 04 — Agent-UX & Dual-Audience Methods

> The two pressures a generic UX toolkit would omit and Myelin cannot: agents as first-class users (VISION;
> P7; design-language §6) and the keyboard-first-engineer *and* approachable-PM/corporate dual audience
> (design-language §2). Tags: **PROVEN** / **HOUSE STYLE**.

---

## 15. Microsoft HAX — 18 Guidelines for Human-AI Interaction — **ADOPT**

**What it is.** Microsoft's HAX Toolkit codifies **18 evidence-based guidelines** (CHI 2019, synthesising 20+
years of research) organised across four temporal phases — **Initially**, **During interaction**, **When
wrong**, **Over time** — plus a Design Library (patterns per guideline), a Workbook (prioritise/apply), and a
Playbook. (Source: microsoft.com/haxtoolkit/ai-guidelines.) Examples: "make clear what the system can do,"
"make clear how well it can do it," "support efficient correction," "convey consequences of user actions,"
"remember recent interactions."

**Why it fits Myelin (specifically).** This is the most *structured, pattern-backed* agent-UX resource and it
maps almost one-to-one onto design-language §6's agent-native UX contract and P7/P12/P13's trust fears.
"Make clear what the system can do / how well" ↔ plan-then-apply showing proposed effects (§6.2). "Support
efficient correction / scope service" ↔ the HITL Approve/**Edit**/Reject card (§6.3). "Convey consequences"
↔ showing what will change, on which artifacts, under whose delegated authority (§6.2/§6.4). "When wrong" ↔
the agent-error and reversibility patterns (§8b.6). The four-phase structure gives us a *checklist to audit
every agent surface against* — exactly what we need since we ship mock agents now and must get the UX right
before real agents arrive.

**How WE would use it.**
- *Phase 5:* audit each agent-touching surface in §7 (PR agent-reviewer, issue triage inbox, CI triage view,
  chat HITL card, agent governance console) against the 18 guidelines; output a per-surface conformance note.
- *Phase 6:* agent sketches must satisfy the relevant guidelines (esp. the "Initially" + "When wrong" sets,
  often neglected).
- *Phase 7:* the 18 guidelines (filtered to Myelin's contract) are a **scoring dimension** in the rubric for
  any sketch with agent UI.

**Effort/cost.** Low-medium (checklist application). **PROVEN (CHI 2019).**

**Uncertainties & risks.** HAX predates the autonomous-agent/plan-then-apply era; some guidelines assume an
*assistive* AI inline in a host app, not an autonomous principal that *acts*. Our HITL/attribution contract
(§6) is *more* specific. Mitigation: use HAX as a floor/checklist; where our doctrine is stricter, doctrine
wins (don't downgrade to HAX).

**Verdict: ADOPT.** Best structured agent-UX checklist available; complements (doesn't replace) our §6
contract.

---

## 16. Google PAIR — People + AI Guidebook — **ADAPT**

**What it is.** Google PAIR's People + AI Guidebook (2019, updated 2021): six chapters with worksheets —
user needs & defining success, mental models, explainability + trust, feedback + control, errors + graceful
failure, and data collection — for human-centred AI products. (Source: pair.withgoogle.com/guidebook.)

**Why it fits Myelin (specifically).** PAIR's strongest chapters — **mental models**, **explainability +
trust**, and **errors + graceful failure** — are exactly the three places Myelin's agent UX wins or loses
trust (P7: "never magic, never hidden"; P12/P13's fear of ungoverned automation). "Calibrate user trust"
(don't over- or under-trust the AI) is the precise framing for plan-then-apply: we deliberately *don't* let
agents act invisibly because mis-calibrated trust is the failure. The "set the right expectations" and "errors
& graceful failure" chapters inform the agent-pending and agent-error states (design-language §5.10/§8b.6).

**Why ADAPT not ADOPT.** PAIR's methods are most powerful *with users* — trust calibration and mental-model
validation are research activities. We ADOPT the *principles* now (they shape sketches) but the *methods*
(measuring whether users correctly understand agent capability/limits) are deferred to Phase 4. We also drop
PAIR's data-collection chapter as largely covered by our GDPR/ADR-12 work.

**How WE would use it.**
- *Now/Phase 6:* apply the mental-model, explainability, and error principles to agent sketches (the rationale
  string, the proposed-effects legibility, the graceful-failure states).
- *Phase 4 (deferred):* run PAIR-style trust-calibration testing on agent surfaces with real users — do they
  correctly understand what the agent can/can't do and when to trust it?

**Effort/cost.** Low now; medium later. **PROVEN.**

**Uncertainties & risks.** We ship *mock* agents — trust calibration against deterministic mocks may not
predict trust in real LLM agents (which fail differently). Mitigation: design the *contract* (plan-then-apply,
attribution) to be trustworthy regardless of runtime (the strategy-pattern payoff, §6); re-test when real
agents arrive.

**Verdict: ADAPT.** Principles now, trust-testing deferred to Phase 4.

---

## 17. NN/g agentic-UX patterns + plan-then-apply critique — **ADOPT**

**What it is.** Nielsen Norman Group's 2025–2026 work on agentic UX (an agent "iteratively takes actions,
evaluates progress, decides next steps"; "context architecture applies IA principles to AI"; reliability/
adaptivity/accuracy as agent quality criteria), combined with a Myelin-specific critique lens derived from our
plan-then-apply / HITL contract. (Source: NN/g AI topic articles, 2025–2026.) The house-style half is a
critique checklist: *is the agent labelled? does it propose before acting? is the plan legible? is there a
gate on consequential actions? is every action attributed + audit-linked? is agent volume kept calm?* — i.e.
design-language §6.1–§6.5 as a pass/fail review.

**Why it fits Myelin (specifically).** Our §6 contract is *more* specific and opinionated than any external
guideline — so the most reliable agent-UX method is *critiquing against our own contract*. The NN/g framing
("context architecture = IA for AI") reinforces that the cross-artifact reference graph (P6) *is* the agent's
context substrate. NN/g's reliability/accuracy criteria matter because mock→real swap means the UI must make
agent *un*reliability legible (the "make clear how well it can do it" duty).

**How WE would use it.**
- *Phase 5/6:* the §6.1–§6.5 critique checklist is applied to every agent surface and every agent sketch.
- *Phase 7:* it's the agent-specific half of the rubric.

**Effort/cost.** Low. **PROVEN (NN/g) + HOUSE STYLE (our §6 checklist).**

**Uncertainties & risks.** Agent-UX canon is young and moving (README §5.4); date the references. Our contract
is *un*tested against real agents (we ship mocks). Mitigation: the checklist tests the *contract's presence*,
which is what makes mock and real safe identically (§6 strategy-pattern payoff).

**Verdict: ADOPT.** The §6-contract critique is the most Myelin-true agent-UX method we have.

---

## 18. Dual-audience / persona-adaptive design ("one component, many lenses") — **ADOPT**

**What it is.** A HOUSE-STYLE method synthesising design-language §2: serve two co-equal audiences by building
*one component over shared primitives* and adapting presentation by role/density/vocabulary — never forking
the product. The method is a discipline: for any dual-audience surface, (a) identify the engineer job and the
PM/corporate job over the *same* data (JTBD, #1), (b) design one component, (c) define the role/density/
vocabulary deltas as *configuration*, not separate code, (d) critique both lenses against their personas.

**Why it fits Myelin (specifically).** This is "the single hardest UX mandate in Myelin" (design-language §2):
the issue tracker (and knowledge, chat) must serve engineers (P1–P5) *and* PM/corporate (P6–P11) at once —
the market's defining failure (Jira-for-eng vs Productboard-for-PM, competitive-landscape §3). The shared
views component (§5.6, ADR-06) is the literal mechanism (same records → engineer board *or* PM roadmap). No
external method addresses this specific tension, so we name our own and make it auditable. It also governs
density (P5: comfortable default + compact toggle) and the persona-adaptive vocabulary open question (§2/§9).

**How WE would use it.**
- *Phase 5:* for each dual-audience surface (issue views, knowledge databases, dashboards), document the two
  jobs, the one component, and the role/density/vocabulary deltas.
- *Phase 6:* dual-audience surfaces are sketched in *both* lenses (engineer view + PM/exec view of the same
  data) to prove "one component, many lenses" holds.
- *Phase 7:* the rubric scores whether a sketch serves *both* audiences from one component, or quietly forks.

**Effort/cost.** Medium. **HOUSE STYLE (synthesis of §2).**

**Uncertainties & risks.** It's our synthesis, not a proven external method, and only *both* audiences using
the same surface can prove it works (README §5.5) — deferred validation. Risk: a "compromise" UI that serves
neither audience well (the exact trap §2 warns of). Mitigation: critique each lens against its persona
separately; validate with both segments in Phase 4 (card sort/tree test #7 run per-segment; usability testing
#23).

**Verdict: ADOPT.** Non-negotiable given the mandate; named and made auditable so it can't be quietly dropped.

---

## SKIPs in this theme (do not relitigate)
- **Anthropomorphic / "personality" agent design — SKIP.** Doctrine forbids magic-wand/sparkle/emoji AI
  framing (§8b.3); agents are legible labelled principals, not characters. Don't design an agent "persona"
  beyond identity/attribution.
- **Conversational-only AI UX patterns (chatbot-first) — SKIP as the frame.** Myelin agents act across
  subsystems via plan-then-apply on real surfaces (PR/issue/doc), not primarily via a chat box; chat is *one*
  HITL surface (§6.3), not the agent paradigm.
