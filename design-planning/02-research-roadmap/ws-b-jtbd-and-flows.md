# WS-B — JTBD for Three Audiences & Cross-Surface Flows

> Workstream B (see [`README.md`](./README.md)). Anchors the corpus in **personas + Jobs-To-Be-Done for
> the THREE audiences** (engineers · PMs/delivery · corporate/governance) and in **named cross-surface
> task flows** that traverse multiple subsystems — the flows are how we prove the "one product" thesis.
> Phase-1 methods #1 (JTBD), #8 (service blueprint), #9 (job-flow), #3 (proto-persona), #4 (assumption
> map).

---

## R-03 — JTBD catalogue for the three audiences

**Questions answered.** What progress is each audience trying to make, expressed as cross-tool jobs
(decoupled from any one subsystem)? Where do the engineer job and the PM/corporate job sit over the
*same* data (the dual-audience justification)? Which jobs are the platform's reason to exist?

**Phase-1 methodology.** #1 JTBD reasoned from personas (ADAPT — no-user instantiation now; the
importance×satisfaction ranking is **deferred-until-users**). #3 proto-persona discipline (carry the
HYPOTHESIS tag).

**Inputs.** `personas.md` P1–P15 (the three clusters: §2 engineers, §3 PM/delivery, §4
corporate/enterprise); `use-cases.md` (the UC catalogue as raw jobs material); design-language §2
(dual-audience), §7.7 (CLI as a job surface).

**Deliverable.** `design-planning/04-research/jtbd-flows/jtbd-catalogue.md`. Jobs-stories ("When
[situation], I want to [motivation], so I can [outcome]") grouped by the three audiences, each tagged
PROVEN-theory / **HYPOTHESIS-instantiation**, each mapped to the §7 surface(s) that finish it and the
persona(s) that hold it. Must include the dual-audience pairs explicitly (e.g. P1 "burn down a cycle"
vs. P6 "communicate a roadmap" over one issue model). Reserve a clearly-marked section for the
**deferred** importance×satisfaction ranking (the ODI core), to be filled in Phase 4 from interviews.

**Sequencing & dependencies.** Seq #3 (parallel-start track). No hard dependency; feeds R-04 (flows are
the jobs realized), R-05, R-16 (dual-audience), and Phase 5 surface-mapping.

**User-dependency.** none for the catalogue; **deferred-until-users** for the importance×satisfaction
ranking (carried verbatim from Phase-1 README §5.2).

**Effort.** M.

**Acceptance criteria.** All three audiences have jobs (corporate/governance not skipped); every job
maps to a §7 surface and a persona; every job is HYPOTHESIS-tagged; the dual-audience same-data pairs are
named; the deferred ranking is recorded as a deferred item, not faked.

---

## R-04 — Named cross-surface task flows (service blueprints + job flows)

**Questions answered.** How does a real task traverse Git → CI → Issues → Knowledge → Chat as one flow,
and where do the seams currently appear that Myelin must dissolve? What are the backstage events/triggers
(ADR-04/08) and the human/agent hand-offs at each step? What states (including the unglamorous ones)
does each step have?

**Phase-1 methodology.** #8 service blueprinting (frontstage screens + backstage events + every actor
including agents); #9 user-flow/job-flow mapping (the per-screen states checklist, incl. §5.10 states).

**Inputs.** R-03 (the jobs the flows realize); `agent-native-design.md` §8 (the worked agent flows that
are blueprints-in-waiting); `system-overview.md` §8.1 (PR context pane), §8.2 (agent HITL flagship), §8.3
(DSR fan-out); `use-cases.md` UC-ISS-13/14/15, UC-GIT-3/17, UC-CI-4; design-language §5.10 (states).

**Deliverable.** `design-planning/04-research/jtbd-flows/cross-surface-flows.md`. A set of **named**
cross-surface flows, **at least one per audience**, each drawn as a service blueprint (frontstage §7
screens → backstage events/triggers → actors incl. agents) PLUS a job-flow enumerating entry points,
keyboard + pointer paths (P3), and **all states** (empty/loading/error/permission/erased/agent-pending,
+ partial-failure agent branches). Required flows:
- **Engineer:** failing CI check → step → line of code → open fix PR → link issue (the wedge engineer
  flagship).
- **PM/delivery:** triage an incident from chat → issue → knowledge runbook → back to chat (the
  cross-surface PM flow named in the steer).
- **Corporate/governance:** a DPO answers a data-subject-access request across all five surfaces (the DSR
  fan-out, §8.3).
- Plus the **agent HITL flagship** (CI fail → triage agent → issue → chat → proposed fix PR → approval
  card → human approves → review agent), drawn with its partial-failure branches.

**Sequencing & dependencies.** Seq #4. Depends on R-03. Heavily consumed by Phase 5 (surface map +
per-surface DoD) and Phase 6 (the blueprints are the spec the multi-screen finalists implement). Feeds
R-22 (wedge moments) and R-19 (the DSR flow).

**User-dependency.** none.

**Effort.** L.

**Acceptance criteria.** ≥1 named flow per audience + the agent flagship; each flow shows frontstage
screens mapped to §7, backstage events, and agent actors; each flow's job-flow enumerates the full state
set incl. partial-failure agent branches; the seams (where today's stack forces a tab-switch) are
explicitly marked as the moments Myelin dissolves.

---

## R-05 — Persona pressure-test & validation-priority register

**Questions answered.** Which persona assumptions are load-bearing? Which persona *pairs* have
conflicting needs the same UI must serve (the design-tension list)? Which personas, if wrong, break a key
surface — i.e. which to validate first?

**Phase-1 methodology.** #3 proto-persona pressure-testing; #4 assumption/risk mapping (the register is
this output).

**Inputs.** `personas.md` (all, esp. §6 archetypes, §7 open questions); R-03 (jobs inherit persona
assumptions); design-language §9 (open questions).

**Deliverable.** `design-planning/04-research/jtbd-flows/persona-pressure-test.md`. Per persona: its
load-bearing assumptions + confidence + what-breaks-if-wrong. Plus a **conflict matrix** of persona
pairs whose needs collide (e.g. P5 OSS-public-default vs. P12 private-governance-default; P1 dense vs.
P6 calm). Plus a ranked **validation-priority register** (which personas to validate first) — nominate
per Phase-1 README §4: the **P6-PM vs P1-engineer dual-audience tension** and the **P13-DPO/P12-security
sovereignty-legibility** bet as first. Every row carries the deferred-until-users replacement flag.

**Sequencing & dependencies.** Seq #5. Depends on R-03 (and informed by R-04's tensions). Feeds Phase 4's
"which personas to validate first" decision and R-16 (dual-audience).

**User-dependency.** none for the pressure-test/register; the **real-persona replacement is
deferred-until-users** (carried from README §5.1 — the single most important Phase-4 research item).

**Effort.** S.

**Acceptance criteria.** Every persona has its load-bearing assumptions named; the conflict matrix names
real pairs with the surface each conflict endangers; the validation-priority ranking is explicit and
justified; the deferred real-persona replacement is recorded, not done.
