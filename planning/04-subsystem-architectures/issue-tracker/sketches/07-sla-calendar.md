# Sketch 07 — SLA business-calendar engine (build vs durable-scheduler-provides-primitives)

> Exploration note. Weighs Phase-2 §11 Q9 / deep-dive §6.9: the SLA engine — policies, business-calendar
> awareness, pause/resume, escalation, breach. Phase 3 already decided the *timer substrate* (durable-workflow
> §3.3/§4.2 — millions of durable timers ride the SC-11 wheel; the issue tracker's SLA timers are named as a
> rider). So the question is narrowed: **what does Issues build on top of the timer wheel?** Leans; commit in
> `00-findings.md`.

## What's already decided (Phase 3 — I consume, don't rebuild)

- **Durable timers at world scale** = `myelin-flow`'s bucketed-partial-index wheel (`wf_timer`,
  durable-workflow §3.3/§4.2). Millions of far-future timers cost ~nothing until due; `FOR UPDATE SKIP LOCKED`
  spreads firing; fire effectively-once even after restart (SC-11). **A bare SLA timer emits
  `sla.deadline.reached` via the outbox on fire** (durable-workflow §4.2 explicitly names this). So I do **not**
  build a timer service.
- **Durable signals + workflow** = the pause/resume + escalation orchestration substrate (durable-workflow
  §4.3/§9.4) — an escalation chain is a durable workflow (Notif `page` starts one, Notif §3.7).

So the SLA *engine* I own is: **policy definition + business-calendar arithmetic + pause/resume condition
evaluation + breach/escalation orchestration** — the *logic*, layered over the *timers/signals/workflow*
substrate. This is exactly Phase-2 §1.2's split ("the *timers* are delegated to ADR-09; the *policy/calendar
logic* is owned").

## The genuinely-owned hard part: business-calendar arithmetic

An SLA target is "respond within 4 **business hours**" against a calendar (Mon–Fri 09:00–17:00 Europe/Berlin,
minus public holidays), with **pause windows** (clock stops while "waiting on customer"). The hard part is:
given `started_at`, a duration in *business* time, a calendar, and a set of pause intervals → compute the
*wall-clock* `fire_at` to hand to the durable timer; and **recompute** `fire_at` whenever a pause starts/ends or
the calendar changes.

### Candidate A — Precompute wall-clock deadline; re-arm the durable timer on every pause/resume
At SLA start: convert the business-time budget into a wall-clock `fire_at` over the calendar (skip
nights/weekends/holidays), arm one `myelin-flow` timer. On **pause**: cancel/disarm the timer, record
`remaining_business_time`. On **resume**: recompute `fire_at` from now + remaining over the calendar, re-arm.

- **For:** the timer wheel only ever holds a concrete `fire_at` — it doesn't know about calendars (clean
  separation; the wheel stays the dumb SC-11 substrate). Millions of SLAs = millions of cheap far-future timers.
- **For:** breach fires durably even after restart (the timer is durable); `sla.at_risk` (e.g. 80%) is a second
  timer armed at start.
- **Cost:** the calendar→wall-clock conversion is non-trivial arithmetic (DST, holidays, multi-day spans) — but
  it's pure, testable, deterministic; a well-cited domain (business-day libraries; iCalendar RRULE/`VTIMEZONE`
  for recurrence/timezone correctness). And pause/resume re-arming is a handful of timer ops per SLA, not a hot
  loop.
- **Lean toward this.**

### Candidate B — Poll: a periodic job scans active SLAs and checks business-time-elapsed
A minute-tick job recomputes elapsed business time for every active SLA and fires breaches.
- **Against:** O(active SLAs) work every tick — the exact scan the SC-11 bucketed wheel exists to avoid. Rejected
  (re-implements the thing Phase 3 gave us, worse).

### Candidate C — Encode the calendar into the timer wheel itself
Teach `wf_timer` about business calendars.
- **Against:** pollutes the shared substrate with Issues-specific calendar logic; durable-workflow §4.2 keeps
  the wheel calendar-agnostic deliberately. The calendar belongs in the Issues SLA engine. Rejected.

## Pause/resume + escalation

- **Pause/resume conditions** are expressed in the shared **safe query-AST `EventMatcher`** (event-bus §4.5):
  "pause when `state:waiting-on-customer`," "resume when `state:in-progress`." When a matching `issue.updated`
  event arrives, the SLA engine disarms/re-arms (Candidate A). One predicate language (sketch 02/03) — no
  bespoke condition DSL.
- **Escalation** on `sla.at_risk` / `sla.breached`: emit a Signal → Notif routes it; a breach can **start a
  durable escalation workflow** (Notif `oncall_now`/`page`, Notif §3.7 → durable-workflow) and/or wake a
  drafting agent ("SLA at 80% → agent drafts a holding response," deep-dive §7.3). All via the bus, not a
  bespoke escalation runner.
- **Breach feeds OLAP** (`sla.breached`/`sla.met` events → SLA-compliance reporting, deep-dive §4.3/§6.5).

## Policy model

An **SLA policy** is a config object (a governance scheme, sketch 02): `{ applies_to: AST predicate, metric:
time_to_first_response | time_to_resolution | custom, target: business-duration, calendar_id, pause_conditions:
AST[], escalation_chain }`. Assigned per (type × team/project) like other schemes. Calendars are reusable
config objects (`{ tz, working_hours[], holidays[] }`). The **SLA policy editor (S14)** + **calendar editor** +
**breach-simulation/preview** are design surfaces (wireframes).

## Leaning

**Build the SLA *logic* engine (policy + business-calendar arithmetic + AST-driven pause/resume + escalation
orchestration) over the `myelin-flow` durable-timer/signal/workflow substrate** — Candidate A (precompute
wall-clock `fire_at`, re-arm on pause/resume). Don't build timers (consume SC-11). Don't poll. Don't pollute the
shared wheel. Pause/resume + escalation conditions are safe-AST predicates + Signals + durable workflows — all
shared substrate. Breach events feed OLAP for compliance reporting.

## Hands forward

- The calendar→wall-clock conversion algorithm (DST/holiday/multi-day correctness) — architecture; PROVE-IT =
  a calendar-arithmetic correctness corpus + a breach-fires-after-restart drill (SC-11 rider).
- Whether `time_to_resolution` SLAs that span days of pauses need history-compaction (durable-workflow §7.5
  long-lived-workflow note) — flag.
- The escalation-chain config shape (co-design with Notif `oncall`/`page`) — architecture.
