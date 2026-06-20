# WS-F — Agent-UX

> Workstream F (see [`README.md`](./README.md)). The agent-native half of the mandate made legible and
> trustworthy in the UI — the visible side of design-language §6 (plan-then-apply / HITL / attribution).
> Agent-UX wins or loses trust (P7, P12/P13's deepest fear). Build ON §6 (the contract is *more* specific
> than any external guideline); use HAX/PAIR/NN-g as a floor/checklist, not a replacement. Phase-1
> methods #15 (HAX), #16 (PAIR, ADAPT), #17 (NN/g + §6 critique).

---

## R-14 — Agent legibility & the plan-then-apply / HITL trust pattern set

**Questions answered.** How is "an agent did this / an agent proposes this" made unmistakable everywhere
(the §3.2 agent treatment, never colour-alone, never magic/sparkle)? How does plan-then-apply surface
**proposed effects** before they happen — what will change, on which artifacts, under whose delegated
authority? How does the HITL approval card (Approve / **Edit** / Reject) behave, and where does it
surface (chat, inbox, inline)? Does it satisfy the HAX 18-guideline floor (esp. "Initially" + "When
wrong")?

**Phase-1 methodology.** #15 Microsoft HAX 18 guidelines (audit each agent surface; PROVEN, CHI 2019);
#17 NN/g agentic patterns + the §6.1–§6.5 critique checklist (is the agent labelled? proposes before
acting? plan legible? gate on consequential actions? attributed + audit-linked? volume calm?).

**Inputs.** design-language §6 (the full contract), §5.4 (the HITL card component), §3.2 (the `agent`
treatment), §8b.3 (no sparkle/emoji); `agent-native-design.md` §4 (plan-then-apply), §8 (the worked
flows); R-04 (the agent flagship flow); agent-fabric architecture (`05-refined/.../agent-fabric.md` —
the effect/gate/attribution mechanics to surface, not redesign).

**Deliverable.** `design-planning/04-research/agent-ux/legibility-and-hitl.md`. The agent legibility +
HITL pattern set: the agent-treatment spec (badge/colour/icon + label, color-blind-safe); the
plan-then-apply card showing concrete proposed effects per artifact + delegated authority; the
Approve/Edit/Reject behaviour incl. the **Edit** path (human amends the proposed effect); the
surfaces it appears on (chat primary, inbox, inline); a **per-surface HAX-18 conformance note** for each
agent-touching §7 surface (PR agent-reviewer, issue triage inbox, CI triage view, chat HITL card, agent
governance console); and the agent **state set** (agent-pending, agent-working, gate-awaiting,
gate-rejected, agent-error, budget-exceeded). Where §6 doctrine is stricter than HAX, doctrine wins.

**Sequencing & dependencies.** Seq #11. Depends on R-04 (agent flagship flow). Feeds R-15, the rubric D6,
and Phase 6 (every finalist's agent/HITL moment).

**User-dependency.** none for the patterns/audit; trust-*calibration* testing is in R-15 (deferred).

**Effort.** L.

**Acceptance criteria.** Agent treatment is unmistakable and color-blind-safe; plan-then-apply shows
concrete proposed effects + authority; the Edit path is specified; every agent-touching §7 surface has a
HAX-18 conformance note; the full agent state set (incl. partial-failure: rejected/error/budget) is
present; doctrine-beats-HAX conflicts are resolved in doctrine's favour and noted; surfaces the existing
agent-fabric mechanics rather than inventing new ones.

---

## R-15 — Agent attribution/audit + calm-agent-volume patterns; trust-calibration plan

**Questions answered.** How is every agent action attributed (who/what, on-behalf-of, under which
trigger, with `correlation_id`) and linked to the tamper-evident audit trail so "why did this happen?" is
answerable inline (§6.4)? How is agent volume kept calm (out of the main timeline, threaded, collapsible,
inboxed — P8/§6.5)? And how will we later *test* whether users correctly calibrate trust (don't over- or
under-trust the agent)?

**Phase-1 methodology.** #16 Google PAIR (mental models / explainability+trust / errors+graceful-failure
— principles now, **trust-calibration testing deferred**); #15 HAX ("convey consequences", "make clear
how well it can do it"); #17.

**Inputs.** design-language §6.4 (attribution/audit affordances), §6.5 (calm volume), §7.6 (agent
governance console, audit log explorer); `agent-native-design.md` §5.5 (attribution/audit/GDPR); R-14
(the legibility patterns); the existing audit/correlation mechanics.

**Deliverable.** `design-planning/04-research/agent-ux/attribution-and-calm.md`. Two parts: **(1)
patterns** — per-action provenance affordance (who/what/on-behalf-of/trigger/correlation), the inline
"why did this happen?" + audit-trail link, the scope/budget/delegation inspector, the agent governance
console + kill-switch surface, and the calm-volume patterns (threading, collapsible summaries, inbox
routing, agent-out-of-main-timeline). **(2) the deferred trust-calibration plan** — a PAIR-style study
design: do users correctly understand what the agent can/can't do and when to trust it; tested on the
HITL flow + the agent-reviewed PR; flagged deferred-until-users (and note: mock-agent trust may not
predict real-LLM trust — design the *contract* to be trustworthy regardless of runtime).

**Sequencing & dependencies.** Seq #12. Depends on R-14. Feeds the rubric D6/D9 and Phase 6.

**User-dependency.** none for the patterns; **trust-calibration testing is deferred-until-users** (carried
from README §5.4).

**Effort.** M.

**Acceptance criteria.** Per-action provenance + inline "why" + audit link specified; calm-volume
patterns concrete; governance/kill-switch surface specced; the deferred trust-calibration study is
executable-as-written and explicitly flagged; the "design the contract trustworthy regardless of runtime"
caveat is recorded.
