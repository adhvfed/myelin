# WS-J — Onboarding & Empty/Loading/Error Craft + Wedge Moments

> Workstream J (see [`README.md`](./README.md)). The lovability craft the autonomous pipeline most
> reliably skips: first-run delight, the unglamorous states done well, and the cross-artifact "wedge"
> moments where the integration delights at the seams. These are the items the completeness-critic
> (README §9) exists to protect. Phase-1 methods #20 (cognitive walkthrough), #9 (job-flow/states), #19
> (heuristics), #8 (blueprint), #2 (teardown), #8b.6 (state specifics).

---

## R-20 — First-run / onboarding delight patterns (3 archetypes)

**Questions answered.** How does the empty platform welcome each archetype — the low-friction startup
(P1, must be near-zero-friction or lost instantly), the scale-up introducing PMs/process, and the
regulated-enterprise admin (P15) standing up SSO/residency/agent-policy? What is the guided first-run that
ties the empty states (§5.10) into a coherent start (first repo → first issue → first doc → first channel
→ first agent run)? Where does onboarding delight without becoming a tutorial slog (P4 progressive
disclosure)?

**Phase-1 methodology.** #20 cognitive walkthrough (learnability — can a first-timer do the first task
without prior knowledge?); #2 teardown (Linear/Notion/Slack onboarding bar); #19 heuristics.

**Inputs.** design-language §5.10 (empty = onboarding-forward), §7.6 (onboarding & empty-platform flows;
the startup vs. enterprise-admin first-run); `personas.md` §6 (the archetypes + "weak onboarding loses
the startup instantly"); P4 (progressive disclosure); R-01 (onboarding teardown); R-04 (the flows
onboarding leads into).

**Deliverable.** `design-planning/04-research/craft/onboarding-delight.md`. First-run patterns for the
**three archetypes** (startup / scale-up / regulated-enterprise-admin): the guided-start sequence; the
zero-data shell; how depth is disclosed progressively (the admin's SSO/residency/agent-policy depth is one
layer down, not in the startup's face); the delight moments (the first wedge appearing, the first agent
proposal) without a tutorial slog. Each first-step is cognitive-walkthrough-checked (will the user know
what to do / see the control / understand the feedback?).

**Sequencing & dependencies.** Seq #18. Depends on R-01, R-04. Feeds the rubric D2 (first-run delight)
and Phase 6 (every finalist's empty/first-run state).

**User-dependency.** none for the patterns (cognitive walkthrough is no-user); first-time-user
*validation* folds into the deferred RITE/usability track.

**Effort.** M.

**Acceptance criteria.** Three archetype first-runs specified; the guided-start ties the empty states
together; progressive disclosure keeps enterprise depth out of the startup's face; each first-step is
walkthrough-checked; delight moments named without a tutorial slog.

---

## R-21 — Empty / loading / error / permission / erased state craft

**Questions answered.** What does each unglamorous state look like *well done*, per the §5.10 + §8b.6
specifics — empty (onboarding-forward), loading (structure-skeleton, never blank spinner), error (blame
the system in one quiet line + a path), permission-denied (graceful no-access, never a leak),
erased/tombstoned (GDPR-aware degraded), agent-pending — across the shared components and the §7 views?
Plus the states the happy-path bias skips: optimistic-rollback, conflict-surfacing, stale/reconnecting,
degraded-surface-temporarily-unavailable, storm/surge.

**Phase-1 methodology.** #9 job-flow (the per-screen state checklist is the point); #19 heuristics
(error-recovery, visibility of status); #8b.6 (the concrete state specifics).

**Inputs.** design-language §5.10 (the cross-cutting state patterns), §8b.6 (loading-shows-structure /
error-blames-system / fails-static); `external-insights/05` §4 (states as first-class designed);
README §9 (the completeness-critic state list — this item *owns* it); R-09 (the chip/unfurl's no-access/
tombstone states), R-10 (each component's states); R-13 (skeleton/optimistic-rollback patterns).

**Deliverable.** `design-planning/04-research/craft/state-craft.md`. A **state-craft catalogue** that, per
shared component and per primary §7 surface, specifies all states: empty / loading-skeleton / error /
permission-denied / erased-tombstone / agent-pending — PLUS optimistic-rollback, conflict-surfacing (the
CAS→CRDT path shown legibly), stale/offline/reconnecting (firehose drop+resume), degraded-surface
"temporarily unavailable", and the storm/30×-agent-surge inbox experience. Each with the §8b.6 specifics
applied. This is the checklist Phase 6 finalists demonstrate (rubric §"comparable screens" requires the
unglamorous states on ≥1 surface).

**Sequencing & dependencies.** Seq #19. Depends on R-09, R-10, R-13. Directly addresses the
completeness-critic (README §9). Feeds the rubric D8 + the switch test, and Phase 6.

**User-dependency.** none.

**Effort.** M.

**Acceptance criteria.** Every shared component + primary surface has its full state set; the skipped
states (optimistic-rollback, conflict, reconnecting, degraded-static, storm) are present, not just the
six common ones; §8b.6 specifics applied (structure-skeleton, quiet-system-blame error, fail-static);
the catalogue is usable as the Phase-6 state checklist; it explicitly covers the README §9 list.

---

## R-22 — The cross-artifact "wedge" moments (delight at the seams)

**Questions answered.** Where does the integration *delight* — the moments where, because Myelin is one
platform, a thing happens that the stitched-together stack cannot do? (The PR context-pane assembling
issue + CI + doc + discussion inline; a chat unfurl with live inline actions; a notification that carries
"why it fired" and pre-fetches the next hop; a reference chip that stays live and backlinked across
subsystems.) How are these designed as deliberate love-moments rather than incidental features?

**Phase-1 methodology.** #8 service blueprinting (the wedge flows); #9 job-flow (the moment-by-moment
experience).

**Inputs.** design-language P6 (reference everything — the wedge), §5.3 (chip/unfurl), §8b.6 (the system
assembles + pre-fetches context); `system-overview.md` §8.1 (PR context pane — the wedge flagship);
`competitive-landscape.md` §6 (the integration *is* the differentiator); R-04 (the cross-surface flows),
R-09 (the chip/unfurl), R-13 (prefetch/context-assembly); the perceived-performance + unfurl-projection
extensions.

**Deliverable.** `design-planning/04-research/craft/wedge-moments.md`. A catalogue of named **wedge
moments** — each a specific point in a cross-surface flow where the integration produces delight the
fragmented stack can't — with: the moment, the cross-surface mechanics behind it (the events/refs that
make it possible), the design that makes it *felt* (not buried), and the "the old stack can't do this"
contrast. At minimum: the PR context-pane assembly; the live chat unfurl with inline actions; the
"why-it-fired + pre-fetched next hop" notification; the cross-subsystem live backlinked reference; the
agent flow threading one `correlation_id` across surfaces visibly.

**Sequencing & dependencies.** Seq #20. Depends on R-04, R-09, R-13. Feeds the rubric D4/D10 and Phase 6
(every finalist's wedge moment, per the funnel comparable-screen set).

**User-dependency.** none.

**Effort.** M.

**Acceptance criteria.** ≥5 named wedge moments, each with its cross-surface mechanics, its felt-design,
and its "old-stack-can't" contrast; each maps to a real cross-surface flow (R-04) and a real component
(R-09); the moments are designed as deliberate love-moments, not listed as features; they are usable as
the Phase-6 wedge-moment screen.
