# 01 — Research & Synthesis (the no-user toolkit)

> The hard constraint governs this whole file: **no access to real users.** Every method here either
> produces signal without users, or is ADAPTED to a no-user mode now with an explicit deferred validation.
> See [`README.md`](./README.md) §2 for the lens. Tags: **PROVEN** / **HOUSE STYLE** (VISION §3 honesty).

---

## 1. Jobs-to-be-Done (JTBD), reasoned from personas — **ADAPT**

**What it is.** JTBD reframes a product around the *progress a user is trying to make in a circumstance*
(the functional, emotional, and social "job"), rather than around features or demographics. Ulwick's
Outcome-Driven Innovation (ODI) operationalises it by quantifying *desired outcomes* and ranking them by
importance × satisfaction to find underserved needs. (Sources: Strategyn; jobs-to-be-done.com.) The
canonical artifact is a **jobs-story**: "When [situation], I want to [motivation], so I can [expected
outcome]."

**Why it fits Myelin (specifically).** Myelin's wedge is cross-subsystem flow, and JTBD is the cleanest way
to express the *cross-tool jobs* the platform exists to serve — e.g. P1's "When a teammate's PR breaks my
mental model, I want to find the commit, the issue, the design doc, and the discussion in one place, so I can
understand *why* without three logins" (personas.md P1: "cross-references rot"). It directly de-couples the
job from the subsystem, which is exactly the integration thesis (VISION §1; P6 "reference everything"). For
the dual-audience problem (design-language §2), JTBD is decisive: the PM (P6) and the engineer (P1) have
*different jobs over the same data* ("communicate a roadmap" vs. "burn down a cycle"), which is the precise
justification for "one schema, many lenses" rather than two products.

**How WE would use it.**
- *Now (no-user mode):* author a **jobs-story catalogue** keyed to personas P1–P15 and the cross-subsystem
  use-cases, tagged PROVEN-theory / HYPOTHESIS-instantiation. This feeds Phase 5 surface-mapping (each
  surface in the §7 catalogue maps to the jobs it serves) and gives Phase 6 sketches a "what job does this
  screen finish" test.
- *Deferred (Phase 4):* run qualitative JTBD interviews to get the *real* jobs and the importance×satisfaction
  ranking; replace the hypothesised stories. This is the step that makes JTBD decisive rather than
  decorative.

**Effort/cost.** Low now (writing). High later (interviews + analysis). **Proven theory; HOUSE STYLE in its
no-user instantiation.**

**Uncertainties & risks.** Reasoned-from-persona jobs inherit the persona hypotheses' unreliability
(README §5.1). The importance ranking — the part that prevents building the wrong thing — is *exactly* what
we can't get without users. Risk: treating hypothesised jobs as validated. Mitigation: tag every story
HYPOTHESIS and forbid Phase 5/6 from citing them as fact.

**Verdict: ADAPT.** Use the framing now to structure surface→job mapping; defer the quantitative core to
Phase 4. Do **not** attempt full ODI need-ranking now (SKIP that sub-method — needs a survey pool).

---

## 2. Comparative / competitive teardown — **ADOPT**

**What it is.** A structured expert analysis of competitor products: walk the real product, document its IA,
flows, interaction patterns, and visual language, and classify each finding as a strength to steal or a trap
to avoid. (Source: competitive-audit practice; pairs with heuristic evaluation.) It produces **signal without
your own users** because the competitor's users already stress-tested the design.

**Why it fits Myelin (specifically).** Myelin already named its North Stars and traps:
competitive-landscape.md sets **Linear (speed/keyboard/command-palette), Notion (block editor/database-views),
Slack (channels/unfurls/slash-commands)** as steal targets and **Jira/Atlassian/Teams** as anti-patterns
(stitched-together, slow, noisy). This is the single best no-user method we have, because the whole product
strategy is "Linear-and-Notion-grade UX with Jira-grade depth." A teardown converts those named targets into
concrete, reusable interaction specs and a *judging baseline*: a Phase-7 sketch can be scored against "does
this PR view meet GitHub's review bar? does the board feel as fast as Linear?"

**How WE would use it.**
- *Phase 2/5:* produce a **teardown dossier** per North Star, mapped to the §7 view catalogue (Linear → issue
  board/triage/command palette; Notion → block editor/database-views; Slack/Zulip → chat timeline/unfurl/
  threads; GitHub → PR/diff/review; Jira/Teams → the "what not to do" register). Each entry: pattern,
  why-it-works, how-Myelin-adapts-it, the trap-to-avoid.
- *Phase 6:* sketches reference the dossier as the bar to clear.
- *Phase 7:* the dossier is the **comparative rubric** ("meets/beats the North Star, or regresses").
- *Phase 8:* informs framework choice (which framework's primitives can hit Linear-grade speed and Notion-grade
  editing).

**Effort/cost.** Medium (hands-on product use + write-up). **PROVEN.**

**Uncertainties & risks.** Teardowns capture *what* competitors do, not *why their users tolerate it* — risk
of cargo-culting a pattern that works only in their context (e.g. Jira's depth that we explicitly want to make
*progressive*, P4). Some competitor AI/agent features move fast (competitive-landscape flags GitLab Duo,
CircleCI Chunk as `[VERIFY]`) — date the dossier. Mitigation: pair every "steal" with the relevant Myelin
principle it must serve, not just "they do it."

**Verdict: ADOPT.** Highest-leverage no-user method; it is *already* half-done in the research docs and just
needs to be made concrete and screen-mapped.

---

## 3. Persona pressure-testing & proto-persona discipline — **ADAPT**

**What it is.** Proto-personas are explicitly-labelled *assumption-based* personas used as a placeholder until
research validates them; the discipline is to (a) state assumptions, (b) pressure-test for gaps/overlaps/
contradictions, and (c) plan their replacement with researched personas. (Source: standard UX practice;
proto-persona concept.)

**Why it fits Myelin (specifically).** personas.md is *unusually honest*: it already declares P1–P15/A1–A5 as
hypotheses with no interviews done (personas.md §0, §7). That makes them textbook proto-personas. The method
fits because Myelin's persona span is extreme — a 3-person startup founder (P1) *and* a regulated-enterprise
DPO (P13) on one architecture (personas.md §6) — and the dual-audience mandate hinges on P1-vs-P6 being
*correctly* characterised. Pressure-testing surfaces the contradictions that would otherwise become design
debt: e.g. P5 (OSS maintainer, public) vs. P12 (security, private governance) have opposing
visibility-default instincts that the same UI must serve.

**How WE would use it.**
- *Now:* a **persona pressure-test pass** — for each persona, list its load-bearing assumptions, find pairs
  whose needs conflict (the design-tension list), and flag which assumptions, if wrong, break a key surface.
  Output feeds Phase 2's "which personas to validate first" decision.
- *Phase 4:* replace proto-personas with research-grounded personas (interviews/JTBD), then re-run the
  pressure test against reality.

**Effort/cost.** Low now. **PROVEN (proto-persona practice).**

**Uncertainties & risks.** The deepest risk in the whole effort: building a beautiful product for personas
that don't match real buyers (personas.md §7 says priority/WTP are unvalidated). Mitigation: never let a
proto-persona silently become "the user"; carry the HYPOTHESIS tag into every downstream artifact.

**Verdict: ADAPT.** Use the discipline now to harden and prioritise; the real-persona replacement is the
single most important Phase-4 research item.

---

## 4. Assumption & risk mapping (uncertainty register) — **ADOPT**

**What it is.** A living register of every design-relevant assumption, its confidence, its blast radius if
wrong, and the cheapest test that would resolve it — a lightweight "assumption mapping" / risk-register
practice. (Source: assumption-mapping practice; mirrors VISION's honesty rule.)

**Why it fits Myelin (specifically).** It is the literal embodiment of VISION §3 "honesty about uncertainty"
and the doctrine's "name your floors." The research docs already carry rich open-question sections
(personas §7, agent-native §7, design-language §9); this method *consolidates* them into one prioritised
register that Phase 2 can directly turn into a research roadmap. It is also how we keep the no-user methods
honest — every HYPOTHESIS tag elsewhere should resolve to a row here.

**How WE would use it.**
- *Phase 2:* the register **is** the seed of the research roadmap. Each row: assumption, source, confidence,
  what-breaks-if-wrong, cheapest-resolving-method (mapped to a method in this folder), defer/now.
- *All phases:* updated as sketches/critiques surface new assumptions; dated per the doctrine.

**Effort/cost.** Low, ongoing. **HOUSE STYLE (disciplined; the practice is generic, our rigour is the value).**

**Uncertainties & risks.** Registers rot if not maintained; a stale register that "looks done" is exactly the
doctrine's failure mode. Mitigation: date every row; review at each phase boundary.

**Verdict: ADOPT.** Cheap, and it is the connective tissue between this phase and Phase 2's roadmap.

---

## SKIPs in this theme (do not relitigate)
- **Quantitative surveys / ODI need-importance ranking — SKIP now.** No respondent pool. Re-open in Phase 4.
- **Contextual inquiry / diary studies / ethnography — SKIP now (no users).** Flag as the *highest-value*
  deferred methods for Phase 4 — they would most directly de-risk the persona hypotheses (README §5.1).
- **Kano model — SKIP now.** Needs survey data to classify must-have/delighter; revisit in Phase 4 if
  prioritisation needs it.
- **Analytics / behavioural data analysis — SKIP.** No live product, no telemetry (and privacy-by-default,
  opt-in telemetry per ADR-12, will keep this thin even post-launch).
