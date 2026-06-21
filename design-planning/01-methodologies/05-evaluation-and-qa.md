# 05 — Evaluation, Critique & QA

> The methods that produce the most signal *without users* — expert evaluation, accessibility audit, critique
> — plus the deferred user-test method (RITE) and the doctrine's definition-of-done (the switch test). This
> theme supplies most of the **Phase 7 judging rubric**. Tags: **PROVEN** / **HOUSE STYLE**.

---

## 19. Heuristic evaluation — Nielsen's 10 + a Myelin P1–P9 heuristic set — **ADOPT**

**What it is.** A usability-inspection method where 2–5 expert evaluators independently walk an interface,
flag violations of recognised heuristics, and rate severity. Three evaluators catch ~60% of issues; it needs
**no users**. Nielsen's 10 heuristics (1994) are the standard base set. (Source: NN/g — how to conduct a
heuristic evaluation.)

**Why it fits Myelin (specifically).** It's the **highest-value no-user evaluation method**, which is exactly
our constraint. Nielsen's 10 cover the universals (visibility of system status, match to real world, user
control, consistency, error prevention, recognition over recall, flexibility, minimalist aesthetic, error
recovery, help). But Myelin's point of view is encoded in **P1–P9**, so we extend the base set with a
**Myelin heuristic set** derived directly from the principles: *coherence/one-product (P1), instant-feel speed
(P2), keyboard-first-mouse-complete (P3), progressive disclosure (P4), earned density (P5), reference-
everything (P6), agents-visible-and-trustworthy (P7), calm-by-default (P8), sovereignty-as-UX (P9)*. Evaluators
score against both sets. This is how "top-of-the-line UX" becomes a *measurable* judgement rather than a vibe.

**How WE would use it.**
- *Phase 6:* each sketch is self-evaluated against Nielsen's 10 + the P1–P9 set before submission (catches the
  obvious before judging).
- *Phase 7:* heuristic evaluation against the combined set is the **core judging method** — multiple evaluator
  passes, severity-rated, per principle.
- *Execution:* repeated on built surfaces.

**Effort/cost.** Low-medium. **PROVEN.**

**Uncertainties & risks.** Catches ~60%, not 100% — misses learnability and real-task issues (covered by #20,
#23, #24). Evaluator bias toward our own principles. Mitigation: combine with cognitive walkthrough (#20) and
the switch test (#24); use ≥3 independent evaluators; the P1–P9 set keeps evaluation tied to *our* stated
point of view, not generic taste.

**Verdict: ADOPT.** The backbone of no-user evaluation and the Phase-7 rubric.

---

## 20. Cognitive walkthrough — **ADOPT**

**What it is.** A task-based, no-user inspection focused on **learnability**: for each step of a task, ask
"will the user know what to do? will they see the control? will they understand the feedback?" (Polson/Lewis,
1990). Complements heuristic evaluation by being task-driven rather than principle-driven. (Source: NN/g; CHI
1990 origin.)

**Why it fits Myelin (specifically).** Myelin's progressive-disclosure principle (P4) — "simple by default,
powerful on demand," the startup founder (P1) and the enterprise admin (P15) using the *same* product — is
fundamentally a *learnability* claim, and cognitive walkthrough is the method that tests learnability without
users. It's especially apt for the onboarding/empty-state flows (design-language §5.10/§7.6 first-run) and for
the keyboard-first paths (P3) — can a *new* PM discover the action that an engineer reaches by muscle memory?
Also ideal for the agent HITL flow (will a first-time approver understand what they're approving and the
consequence? — maps to HAX "convey consequences").

**How WE would use it.**
- *Phase 6/7:* walk the flagship first-use tasks (create first repo/issue/doc; approve first agent action;
  find the residency setting) step-by-step against each sketch; flag where a step relies on prior knowledge.
- *Phase 4:* re-run with real first-time users to confirm.

**Effort/cost.** Low-medium. **PROVEN.**

**Uncertainties & risks.** Depends on choosing the *right* tasks (themselves derived from unvalidated jobs).
Mitigation: prioritise the empty/first-run flows VISION explicitly cares about; revisit task selection after
Phase-4 JTBD.

**Verdict: ADOPT.** The learnability complement to heuristic evaluation; directly tests P4.

---

## 21. Accessibility audit — WCAG 2.2 AA / EN 301 549 / EAA — **ADOPT**

**What it is.** Systematic conformance evaluation against **WCAG 2.2 Level AA** and the European standard
**EN 301 549** (which gives a "presumption of conformity" with the **European Accessibility Act**, enforceable
since **2025-06-28**). Combines automated checks, manual expert review (keyboard, focus, contrast, ARIA,
reflow/zoom), and — later — assistive-technology user testing. (Sources: EN 301 549 / EAA compliance guides;
WCAG 2.2.)

**Why it fits Myelin (specifically).** Binding, not optional: design-language §4 sets **WCAG 2.2 AA as the
platform target** and names **EN 301 549 / EAA** as the bar because Myelin sells to **EU public-sector buyers
for whom accessibility is a legal procurement requirement** (personas §6; P15). It's also a *correctness*
property of shared components (§4: "screen-reader correctness is a component contract"). The audit operationalises
§4's specifics: full keyboard operability (P3), visible focus (one `focus-ring` token), contrast as a token
constraint (overlaps #12), status-never-by-colour-alone, RTL via logical properties, reduced-motion, 200% zoom.
For an EU-sovereign product, failing accessibility isn't just bad UX — it disqualifies the product from its
core market.

**How WE would use it.**
- *Phase 5:* specify the accessibility CI gate in the testing strategy (overlaps §8b.3 measured tokens).
- *Phase 6:* each sketch is audited for keyboard path, focus visibility, contrast, semantic structure, and
  the §5.10 states' accessibility; non-conformance is flagged before aesthetics.
- *Phase 7:* WCAG 2.2 AA / EN 301 549 conformance is a **gate** in the rubric (a beautiful inaccessible sketch
  fails).
- *Phase 8:* the framework must ship accessible primitives (combobox, dialog, table, menu) — a top selection
  criterion (§8.2 rationale).
- *Phase 4 / execution:* assistive-technology *user* testing (the part the audit can't fully cover).

**Effort/cost.** Medium. **PROVEN (legal standard).**

**Uncertainties & risks.** Automated tools catch only ~30–40% of issues; AA conformance ≠ usable-with-AT
(that needs real AT users, deferred to Phase 4). Note WCAG 2.2 is incorporated into EN 301 549 via the
forthcoming v4.x; cite versions carefully and date the audit. Mitigation: manual expert audit now + AT user
testing in Phase 4; treat AA as floor, pursue AAA contrast on reading/code surfaces per §4.

**Verdict: ADOPT.** Legally and strategically mandatory; the audit is no-user, AT-user-testing is the deferred
top-up.

---

## 22. Structured design critique (framed, criteria-anchored) — **ADOPT**

**What it is.** A facilitated critique with discipline: frame the problem the design solves, constrain scope,
use *explicit criteria*, surface risks before solutions, anchor feedback to personas/principles/research, and
leave with decisions. Formats include round-robin, quota (N positives / N concerns each), and silent critique
(write before discuss). (Sources: NN/g design critiques; UX Tigers; UXmatters ground rules.)

**Why it fits Myelin (specifically).** Phase 7 *is* a structured critique of 15 sketches — without a disciplined
format it degenerates into taste warfare (the README §5.6 risk). Anchoring feedback to **explicit criteria**
(our P1–P9 heuristics #19, the measured gates #12/#21, the agent #15/#17 and dual-audience #18 dimensions, the
North-Star teardown #2 baseline) is what makes the judging *defensible* and honest (VISION §3) rather than
"the orchestrator preferred sketch 7." It also runs *within* Phase 6 so each sketch improves before judging.

**How WE would use it.**
- *Phase 6:* lightweight self-critique against the criteria after each sketch.
- *Phase 7:* the formal judging is a structured critique — recommend **silent critique first** (each evaluator
  scores all 15 against the rubric independently, in writing, before discussion) to avoid anchoring/groupthink,
  then converge. Output: ranked sketches with criteria-anchored rationale + a merged "best-of" direction.

**Effort/cost.** Low (process). **PROVEN.**

**Uncertainties & risks.** Single-author/orchestrator context limits "multiple independent evaluators" — the
critique may lack diversity. Mitigation: use multiple evaluation *passes* (heuristic + cognitive walkthrough +
accessibility as separate lenses) as a proxy for multiple evaluators; consider specialised review agents per
lens.

**Verdict: ADOPT.** The format that makes Phase 7 fair and honest; silent-first to fight bias.

---

## 23. RITE — Rapid Iterative Testing & Evaluation — **ADAPT (defer execution)**

**What it is.** Run a usability test on a prototype, fix the issue *immediately* after 1–3 participants hit it,
then keep testing the fixed version — designers/PMs/engineers in the room making real-time decisions.
Low/mid-fidelity prototypes preferred for fast iteration. (Sources: RITE Method origin; UXmatters 2024.)

**Why it fits Myelin (specifically).** RITE is the *fastest* way to turn the Phase-6 sketches into validated
designs once we have users — its in-the-room, fix-immediately loop matches our sequential-agent build cadence
well. It pairs naturally with the Phase-6 HTML sketches (they're already interactive enough to test) and would
be the method to validate the dual-audience surfaces (#18) and IA (#7) with *both* engineer and PM
participants.

**Why ADAPT (defer).** RITE is fundamentally user-test-driven — it produces *nothing* without participants.
We cannot run it now. We ADAPT by (a) building Phase-6 sketches in a RITE-ready state (interactive, quick to
change) and (b) scheduling RITE loops for Phase 4 / execution.

**How WE would use it.**
- *Phase 4 / execution (deferred):* RITE loops on the highest-risk surfaces (dual-audience issue views, agent
  HITL flow, onboarding/empty states) as soon as users are available.

**Effort/cost.** Medium-high; **blocked on users.** **PROVEN.**

**Uncertainties & risks.** Pure no-user blocker (README §5.1). RITE's small-sample fixes can over-fit to a few
participants. Mitigation: combine with the broader Phase-4 research; use heuristic evaluation/walkthrough now
as the no-user substitute.

**Verdict: ADAPT (defer).** Plan the loops now; run them the moment users exist.

---

## 24. The switch test (drive-the-real-UI definition-of-done) — **ADOPT**

**What it is.** Binding doctrine (external-insights §7 / design-language §8b.7): a surface is **done only when,
by driving the *real* UI in a browser, a team could move to it without hitting a wall the old tool didn't
have.** A dedicated "does this feel finished?" drive-through routinely finds a dozen-plus issues a feature
checklist misses. It's the design analogue of the process doctrine's "actually try it."

**Why it fits Myelin (specifically).** It's the **frontend definition-of-done** mandated by doctrine, and it's
the most honest single test of "top-of-the-line UX" (VISION §3) we can run *without a formal user study* — a
skilled person actually using the thing. It directly tests the things heuristics miss: the latency budgets
(§8b.6: keyboard <100ms, no flash-of-spinner <1s, "pages render, they don't animate in"), the layout/mobile
bug classes (§8b.4), the humanised-strings tell (§8b.5: no `merge_request merged` / raw ids), and the overall
"feels finished" quality that *is* the product's love-ability. For the switch thesis specifically (we want
teams to *switch* off Jira/Slack/Notion), "could a team switch to this without regressing" is the literal
success criterion.

**How WE would use it.**
- *Phase 5:* written as the per-surface **definition-of-done** in the surface map and the testing strategy.
- *Phase 7:* "would this survive a switch-test drive-through?" is a rubric question (sketches are limited HTML,
  so applied as a thought-experiment + interaction walkthrough of what exists).
- *Execution:* the binding DoD before any surface is called done — a real drive-through in a browser.

**Effort/cost.** Low-medium (skilled time), high value. **PROVEN-by-doctrine (drive-through evidence).**

**Uncertainties & risks.** It's expert drive-through, not naive-user testing — can miss what a *new* user
stumbles on (cognitive walkthrough #20 and Phase-4 user testing cover that). On Phase-6 sketches it's
partial (they're not the real app). Mitigation: full switch test binds at execution; in Phase 7 it's a
directional judgement; pair with #20 for the naive-user angle.

**Verdict: ADOPT.** The doctrine-mandated definition-of-done and the truest no-formal-study quality bar.

---

## SKIPs in this theme (do not relitigate)
- **A/B & multivariate testing — SKIP for design phases.** No live product, no traffic. Post-launch only.
- **Large-sample summative usability testing / SUS benchmarking — SKIP now (no users).** RITE (#23) +
  Phase-4 testing cover formative needs; summative benchmarking waits for a product + users.
- **Eye-tracking / biometric testing — SKIP.** Disproportionate cost for our stage; heuristic + walkthrough +
  switch test give better ROI now.
