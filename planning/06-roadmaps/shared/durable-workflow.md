# Phase 6 — Roadmap: Durable-Workflow Substrate (myelin-flow)

> Phase: 06-roadmaps (shared system). Sequences the build of the durable-execution substrate.
> Canonical brief: VISION.md §6. Binding doctrine: external-insights/01-process-and-quality-doctrine.md
> (order-by-non-negotiability; prove-it-or-it-isn't-real; the ratchet/committed gates; name-your-floors;
> code-wins-over-docs) and external-insights/04-hard-problems.md (§1 erasure-vs-immutability, §2 CRDT-after-CAS
> + resume-cursor transport, §5 untrusted-code-execution + event-volume seam).
> FROZEN architecture this roadmap sequences (does not redesign):
> planning/05-refined-shared-systems-architecture/durable-workflow.md (the refined myelin-flow architecture),
> planning/05-refined-shared-systems-architecture/contract-index.md §9 (contracts 9.1–9.6) + the consumed rows
> (1.x, 2.x, 4.7, 5.9, 11.7), and the master sequencing planning/06-roadmaps/00-master-sequencing.md (bands
> M0..M6, the critical path, the gate invariant). Drills owed: testing-strategy/01-...-drill-catalogue.md
> FLOW-D1..FLOW-D10 + the E2E-2 flagship. Plain-text identifiers (no backticks-as-emphasis). Markdown only; no
> commits. Date: 2026-06-19.
>
> **What this is.** The detailed sequenced roadmap for myelin-flow, slotted into the master bands. It states the
> milestones for THIS shared system, the floor-then-full progression with each floor named and each follow-on
> scheduled, the upstream dependencies (what must be green first), and the quantified gates/drills that call each
> milestone done. The architecture is frozen; this document sequences it. Where the master sequencing places a
> drill, this roadmap refines the work inside that band and must not contradict the band ordering or the gate
> invariant.

---

## 0. Where myelin-flow sits in the master sequence (the one-paragraph placement)

myelin-flow is a **M2 reactive-shared-layer system** (master §2, M2): it ships its engine in M2, alongside the
agent fabric it underpins, because **the agent fabric, CI pipelines, the merge queue, Notif escalation, Issues
SLA timers, and multi-day HITL are all workflows** — nothing downstream of M2 can run a metered, gated, or
long-running process until the durable executor exists. It is **on the critical path** (master §3.1): the chain
runs harness+outbox+lints (M0) → Identity+restore-verify+tenancy (M1) → **agent fabric + workflow + the firehose
resume-cursor transport + AG-D4 (M2)** → Git merge gate (M3) → CI CheckStatus producer (M4) → the X-1 seam
end-to-end + the E2E-2 flagship (M5). myelin-flow owns the **merge-queue workflow mechanics** and the
**SCHEDULE_AND_RUN_JOB long-park** that the X-1 seam rides — so although the CheckStatus data shape is owned by
CI+Git, the durable-execution half of the single most load-bearing cross-subsystem seam is this system's. The
engine itself is **BUILD, DBOS-class, Postgres-embedded** (architecture §2, ADR-09) — no new datastore, it sits
inside each service's existing Postgres and inherits the substrate's outbox/idempotent-consumer/crypto-shred/
tenant-partition/fail-static primitives for free.

The honest progression for this system:

- **First runnable** (M2.1): a single workflow runs deterministically to completion against the outbox; a
  worker kill mid-run replays and resumes exactly-once (FLOW-D1). This is the engine's heartbeat.
- **First useful** (M2.2–M2.4): durable timers at scale, durable signals (multi-day HITL), the
  SCHEDULE_AND_RUN_JOB long-park, per-effect idem_key, reserve/settle, the determinism lint + divergence guard —
  enough that agent runs, the merge queue, SLA timers, and HITL all have a substrate to ride on. This is the M2
  exit state for myelin-flow.
- **Production-hardened** (M5): 1M+ timers within the tick budget (FLOW-D3), the 30× agent-workflow surge with
  lane shedding (FLOW-D8), restore-verify to a consistent point (FLOW-D10), crypto-shred reaching history
  (FLOW-D9), and the merge-queue long-park proven end-to-end inside the E2E-2 flagship. The history-archival /
  continue-as-new tier and cross-cell workflow spanning ship as **named floors with measured-trigger follow-ons**.

---

## 1. Contracts this system must implement, and by which milestone

From contract-index.md §9 (the frozen build-to surface for myelin-flow), plus the consumed rows it stands on.
Each row names the milestone by which it must be implemented and CDC-covered (the contract-coverage scanner,
M0, fails the build if any row lacks provider+consumer coverage — master §2 M0).

| Contract | What it is | Milestone implemented | Notes / drill that proves it |
|---|---|---|---|
| 9.1 DurableExecutor{start, signal, describe, cancel} | engine-agnostic seam; signal idempotent on idem_key; the **per-effect idem_key rule** (card_id single / card_id:effect_idx multi) | start/describe/cancel: **M2.1**; signal + per-effect rule: **M2.3** | FLOW-D4, CHAT-D10/E2E-2 (per-effect) |
| 9.2 WfCtx{activity, sleep_until, sleep_for, wait_for_signal, now, rand, emit} | the deterministic definition surface; emit via outbox; the **SCHEDULE_AND_RUN_JOB long-park idiom** | activity/now/rand/emit + determinism: **M2.1**; sleep_*: **M2.2**; wait_for_signal: **M2.3**; SCHEDULE_AND_RUN_JOB: **M2.4** | FLOW-D1/D2/D5 (core), FLOW-D3 (timers), FLOW-D4 (signal), CI-D1/E2E-2 (long-park) |
| 9.3 Durable timer wheel | minute-bucket partial index + FOR UPDATE SKIP LOCKED; millions of timers as an indexed range read; cheap SLA disarm/re-arm | **M2.2** | FLOW-D3 (floor: 100k in M2; 1M+ at cell scale in M5) |
| 9.4 Durable signal (multi-day HITL) | state=waiting holds no runtime; approval/cancel/ci.result/job.done arrive hours/days later, idempotent | **M2.3**; the ci.result/job.done merge-queue wiring lands **M2.4 (frame)** and goes live in **M4** with CI's producer | FLOW-D4; X-1 seam GIT-D10/CI-D8 in M4; E2E-2 in M5 |
| 9.5 Workflow↔agent mapping | workflow owns RunBudget/gates/state; step/exec are activities; reserve/settle bookends | **M2.4** (reserve/settle bookend) — agent-run body co-built with Agent Fabric M2 | FLOW-D6 (reserve), AG-D5/AG-D9 |
| 9.6 PersonalDataHolder(workflow history) + replay | references-not-payloads; inline-PII rows per-subject crypto-shred | trait + auto-registration: **M2.1**; crypto-shred reach: **M5** | FLOW-D9 |
| **Consumed — must exist first** | | | |
| 1.1 serve(AppSpec) + three-surface + liveness≠readiness | the service shell every workflow worker boots from | M0 (consume in M2.1) | — |
| 2.2/2.3/2.4/2.5 OutboxTx::emit + outbox table + EventHandler + consumer_dedup | the ONLY emit path; WfCtx::emit + journal co-commit | M0 (consume in M2.1) | FLOW-D5 (co-commit) |
| 1.6 flow-determinism lint | a workflow reading clock/RNG/IO outside WfCtx fails to compile | lint shipped in M0; **myelin-flow ships its red+green fixtures in M2.1** | FLOW-D2, CI-D9 |
| 4.7 mint_run_token (mid-workflow re-mint on resume) | a days-later resume re-mints a short-lived attenuated per-run token | Identity M1; consumed M2.4 | E2E-2 step 6 |
| 11.7 reserve/settle cost gate | fronts every agent run, every CI run, every SCHEDULE_AND_RUN_JOB | Storage M1; consumed M2.4 | FLOW-D6 |
| 5.9 CheckStatus / ci.result seam | CI-owned data shape; this engine owns only the merge-queue durable mechanics | CI producer M4; **myelin-flow ships the merge-queue workflow body + wait in M2.4 (awaiting producer)** | GIT-D10/CI-D8 (M4), E2E-2 (M5) |

**The acyclicity rule (no-cross-sync-cycle lint, M0):** myelin-flow never synchronously calls CI/Git to ask "is
the job done." Completion arrives as a durable signal (job.done / ci.result) delivered through the bus. Every
cross-subsystem dependency is async event/projection, enforced at compile time.

---

## 2. The milestones (mapped to master bands)

The master places all of myelin-flow's engine work inside **M2** (the reactive shared layer), its world-scale
hardening + floor follow-ons inside **M5**, and the X-1 seam end-to-end across **M3/M4/M5**. I refine M2 into
four ordered sub-milestones (M2.1..M2.4) by the floor-then-full and dependency logic below; they parallelise
inside the band but are internally ordered by what each later piece needs from the earlier.

### M2.1 — The engine heartbeat: deterministic replay + the outbox co-commit (FIRST RUNNABLE)

**Master band:** M2. **Thesis:** stand up the smallest thing that is honestly a durable executor — a workflow
that runs deterministically, journals every non-deterministic interaction, replays exactly to its cursor after a
crash, and emits only through the outbox in the same transaction as its journal write. Nothing here is a feature;
it is the correctness spine every later idiom rides.

**Work:**
- The data model (architecture §3, carried verbatim): workflow_run, wf_history (append-only journal, source of
  truth, UNIQUE(tenant, run_id, command_id) makes journaling idempotent), wf_definition (versioned, run pinned
  to wf_version at start). Tenant+region first column, RLS, per-tenant envelope-encrypted, crypto-shred-capable.
- WfCtx core surface (9.2): activity (journaled, retried, §4.4), now/rand as journaled side-markers, emit via
  the outbox (BUS-2 — no second emit path; the journal write and the outbox row co-commit in one txn).
- DurableExecutor::{start, describe, cancel} (9.1, partial — signal lands M2.3).
- The deterministic replay/recovery algorithm (§4.1): crash at step 5/10, re-lease, replay to step 6, 0
  re-executed side effects. Lease-based dispatch + crash recovery (§4.7).
- The flow-determinism lint's myelin-flow fixtures: a red fixture (a workflow body that reads SystemTime/IO
  outside WfCtx — must fail to compile) + a green fixture (the same logic via ctx.now/ctx.activity — must
  compile). The lint itself ships in M0; this milestone proves it rejects.
- The divergence guard: a replay against a divergent/wrong-version definition halts as state=nondeterministic +
  dead-letters; 0 silent divergence.
- PersonalDataHolder auto-registration (9.6) via the harness (references-not-payloads from day one — input/state
  are refs, not payloads).
- The telemetry set on the metrics-health port (§5.4, contract 1.8): runnable-run lag, replay rate,
  nondeterministic-halt count, activity queue depth + retry + dead-letter.

**Upstream dependencies (must be green to start):**
- **M0 green:** serve(AppSpec) (1.1), the transactional outbox + idempotent-consumer template (2.2–2.5), the
  flow-determinism lint (1.6, shipped, fixtures owed here), the failure-injection harness (so FLOW-D1/D2/D5 are
  drillable), the EventEnvelope (2.1) frozen.
- **M1 green:** tenant+region partition (12.1) and RLS (11.1) so the tables are partitioned correctly; storage
  OLTP tier (11.1). (Identity check is not yet needed by the bare engine; it enters at M2.4 with run tokens.)

**Exit gate (quantified — must be green to claim M2.1):**
- **FLOW-D1** (CI) — kill a worker at activity 5/10 mid-run → another re-leases, replays, resumes at step 6 with
  **0 re-executed side effects, 0 lost progress, exactly-once-in-effect**. Green artifact: replay-rate signal; 0
  double-effect counter.
- **FLOW-D2** (CI) — replay against a divergent/wrong-version definition → divergence guard halts as
  nondeterministic + dead-letters; **0 silent divergence**. Green artifact: nondeterministic-halt count.
- **FLOW-D5** (CI) — crash between journaling an activity's DB write and emitting its event → **journal + outbox
  committed together (one txn); 0 ghost, 0 lost**. Green artifact: co-commit proof. (This is myelin-flow's face
  of the Tier-1 silent-data-loss floor — BUS-D4-equivalent for the workflow journal.)
- **flow-determinism lint green** on both fixtures (red rejects, green admits).
- Contract-coverage scanner passes for 9.1 (start/describe/cancel), 9.2 (activity/now/rand/emit), 9.6.

### M2.2 — Durable timers at scale (the SC-11 substrate)

**Master band:** M2. **Thesis:** add durable time. A workflow that sleeps for days must hold a row, not a
runtime, and millions of outstanding timers must cost an indexed range read, not a scan. This is the substrate
under Issues SLA timers, Trigger stale_after, snooze re-surfacing, HITL timeouts, and KN living-doc automations.

**Work:**
- wf_timer (§3.3): bucket = epoch_minute(fire_at) + the partial index (bucket, partition) WHERE NOT fired — the
  world-scale move (a 30-day timer is never read until its minute).
- WfCtx sleep_until / sleep_for (9.2/9.3): durable timer, effectively-once fire, a crash re-fires only the
  unfired.
- The timer wheel scan: bucket <= now AND NOT fired, FOR UPDATE SKIP LOCKED; **no calendar logic on the wheel**.
- Cheap SLA-timer disarm/re-arm (§6.6, the Issues ask, was blocking, now confirmed): a re-arm is a row update of
  fire_at + bucket; a disarm sets fired=true or deletes — millions re-arm at row-update cost, not wheel-scan cost.
- Telemetry: timer-wheel lag (the SC-11 health signal).

**Upstream dependencies:** M2.1 green (the engine + journal). No new external dependency.

**Exit gate:**
- **FLOW-D3 floor** (SCHED, run at **100k+ timers** in M2 — the 1M+ cell-scale run is scheduled in M5): arm 100k+
  durable timers + a burst due in one minute → due timers fire **within the tick budget**; far-future ~free; a
  crash re-fires unfired. **0 lost / 0 double-fire**. Green artifact: timer-wheel lag; 0 lost/dup. (Named floor:
  the M2 run proves the algorithm at six figures; the master schedules the seven-figure prod-scale run + the
  per-cell timer-wheel-promotion-threshold measurement in M5 — architecture §7.3, OQ #5.)

### M2.3 — Durable signals + multi-day HITL + the per-effect idem_key (FIRST USEFUL begins)

**Master band:** M2. **Thesis:** add durable signals so a workflow can park for days waiting on a human approval
(or a job completion) holding no runtime, and resume exactly where it parked across restarts and deploys. This is
the substrate under the HITL approval round-trip and the batch/partial approval semantics.

**Work:**
- wf_signal (§3.4): durably-buffered inbound signals, PK (tenant, run_id, signal_name, idem_key) — the PK is
  exactly what makes idempotency true by construction.
- DurableExecutor::signal (9.1) — idempotent on idem_key; the **per-effect idem_key rule** (§6.4): single-effect
  card → idem_key = card_id; multi-effect card → idem_key = card_id:effect_idx. A double-click is one approval; a
  partial approval (approve 0 and 2, decline 1) is well-defined; a declined effect is withheld (AG-8, returns
  Denied, never mutates).
- WfCtx wait_for_signal (9.2/9.4): state=waiting holds no runtime; the registered signal names approval, cancel
  (ci.result and job.done are registered here as names but their producers/long-park wiring land M2.4).
- The HITL approval-card round-trip mechanics (§6.3): gated tool → wait_for_signal("approval:<call>",
  timeout=window) → emits agent.approval.requested via the outbox; the workflow parks for up to window (days);
  a human Approve/Deny → signal lands in wf_signal (idempotent) → resumes/withholds/timeout-path. (The card's
  visual design + data model is Chat+Agent-Fabric product work, OQ #1 — not this engine.)
- Telemetry: signal buffer depth + oldest unconsumed wait age.

**Upstream dependencies:** M2.1 + M2.2 green (the engine + the timeout timer the wait needs). Co-built with the
**Agent Fabric (M2)** which owns the gated-tool set and the EffectApi the withheld effect maps to, and **Notif
humanise (7.3, M2)** which renders the card.

**Exit gate:**
- **FLOW-D4** (CI) — a gated workflow waits across a worker restart + a deploy; deliver approval days later
  (double-click) → **resumes, consumes once, runs/withholds correctly**. Green artifact: 1 consume; withhold = 0
  mutation. (Partial-approval per-effect idempotency is asserted in F-4's extended form, architecture §8 — and at
  the subsystem face in CHAT-D10, M4.)
- Contract-coverage scanner passes for 9.1 (signal + per-effect rule), 9.4.

### M2.4 — The long-park idiom + reserve/settle + the merge-queue workflow frame (FIRST USEFUL complete; M2 exit)

**Master band:** M2. **Thesis:** add the SCHEDULE_AND_RUN_JOB long-park so a workflow can dispatch a multi-hour
CI/agent job and park on its completion holding no runtime; front it with reserve/settle so every dispatched job
is metered; and ship the merge-queue workflow body so the X-1 seam's durable-execution half is ready the moment
CI's producer lands. This is the M2 exit state for myelin-flow.

**Work:**
- The SCHEDULE_AND_RUN_JOB idiom (§4.9, 9.2): an ordinary journaled activity that mints idem_token at the
  workflow (deterministic on command_id, so producer and consumer agree without coordination), stamps it on the
  JobSpec{kind: ci|agent}, hands it to the unified runner (ADR-20 / X-6), **reserves budget at dispatch** (11.7),
  journals activity_completed{job_dispatched: true, idem_token}, and **returns immediately** (frees the worker).
  Then wait_for_signal("job.done", idem_key=idem_token) with a timeout timer bounding a vanished runner.
- Reserve/settle bookend (9.5, 11.7): reserve at dispatch (no balance → no dispatch), settle on the
  job.done/ci.result signal, never interrupt in-flight, meters into the same wallet as a synchronous activity.
- mint_run_token mid-workflow re-mint on resume (4.7, §6.2): a days-later resume re-mints a fresh short-lived
  attenuated per-run token (token life == activity life, not the days-long workflow life). Closes the Phase-3
  open item.
- The merge-queue durable workflow body (§6.5, 9.4 / 5.9): one workflow per target ref; for each queued PR it
  dispatches required CI via SCHEDULE_AND_RUN_JOB, waits on ci.result (idem_key = merge_attempt_id), and on
  success-for-all-required-contexts performs the merge + emits git.pr.merged via the outbox + settles; on
  failure dequeues with a humanised reason. **The ci.result/job.done producer is CI (M4); this milestone ships
  the workflow side + the wait, awaiting the producer** — the seam goes live in M4.
- The agent-run workflow body co-built with Agent Fabric (9.5): the plan-then-apply loop as the deterministic
  workflow body, step/exec as activities, RunBudget/gates/state owned by the workflow.

**Upstream dependencies:**
- M2.1–M2.3 green (engine, timers, signals).
- **Identity M1:** mint_run_token (4.7) callable mid-workflow.
- **Storage M1:** reserve/settle cost gate (11.7) + the wallet.
- The **unified runner (ToolHands::exec, 8.4, Agent Fabric M2)** is the dispatch target — and **AG-D4 (the
  sandbox-escape GATE) must be green before any SCHEDULE_AND_RUN_JOB dispatch runs untrusted code.** myelin-flow
  dispatches into the runner; it does not own the sandbox. The 5.9 CheckStatus seam's producer (CI) is M4 — so
  the merge-queue body is built-and-drilled-in-isolation in M2 and exercised end-to-end in M4/M5.

**Exit gate (the M2 exit for myelin-flow; band gate is AG-D4 owned by Agent Fabric/CI):**
- **FLOW-D6** (CI) — runaway agent loop vs a depleting wallet → a new spend-bearing activity (incl. a
  SCHEDULE_AND_RUN_JOB dispatch) **refused at reserve**; an in-flight one **never interrupted**. Green artifact:
  reserve refusals; 0 interrupt. (F-6 extended to assert reserve-at-dispatch for the long-park, architecture §8.)
- **FLOW-D7** (CI) — adversarial workflow→event→workflow loop → depth ceiling + bus tripwire + bounded activity
  pool **stop it (drops/parks, never forks)**. Green artifact: causal-depth; 0 fork.
- The merge-queue workflow body passes its **in-isolation drill** against a mock ci.result producer
  (double-delivery → one wake; timeout branch bounds a vanished CI run) — the full GIT-D10/CI-D8 end-to-end is an
  **M4 gate** once CI's producer exists.
- Contract-coverage scanner passes for 9.2 (SCHEDULE_AND_RUN_JOB), 9.4 (ci.result/job.done waits), 9.5.

**Note on the band gate.** The master M2→M3 hard go/no-go is **AG-D4 / CI-T1 (real-kernel sandbox escape = 0)**,
owned by Agent Fabric/CI, not myelin-flow. myelin-flow's M2 work must be green for M3 to start, but the
SCHEDULE_AND_RUN_JOB dispatch path into the runner is **gated by AG-D4** — no long-park dispatch executes
untrusted code until AG-D4 is green on the production backend.

### M3 — The merge-gate consumer half goes live with Git (no new engine work)

**Master band:** M3 (producer subsystems). **Thesis:** Git ships its merge gate + check_status projection + merge
queue in M3, riding the myelin-flow merge-queue workflow body built in M2.4. **myelin-flow ships no new engine
here** — it provides the durable-execution mechanics Git's merge queue consumes. The seam's producer (CI) is
still M4, so the queue is wired-and-waiting.

**Work (myelin-flow contribution):** support Git's adoption of the merge-queue workflow (one per target ref) and
the resumable maintenance activities (§6.6: Git GC / repack / bundle-gen / history-rewrite as resumable journaled
activities or SCHEDULE_AND_RUN_JOB long-parks; a crash mid-repack replays to the un-journaled step).

**Upstream dependency:** M2 green (the full engine). **Exit gate:** GIT-D9 (push outbox emit-iff-committed) is
Git's gate; myelin-flow's relevant assertion is that the merge-queue workflow holds no runtime across the wait
(re-confirmed by the M2.4 in-isolation drill; no new FLOW drill is owed in M3).

### M4 — The X-1 seam end-to-end: the merge-queue long-park wakes on the real ci.result

**Master band:** M4 (consumer subsystems; CI lands first). **Thesis:** CI ships the CheckStatus producer (5.9),
closing the X-1 seam. The merge-queue workflow (built M2.4, adopted by Git M3) now wakes on the **real** ci.result
rollup signal. CI pipelines themselves are durable workflows (the ci.pipeline workflow body) riding
SCHEDULE_AND_RUN_JOB. **myelin-flow ships no new engine here** — the seam goes live; the drills that prove it are
CI/Git-owned but they exercise this engine's long-park + idempotent-signal mechanics.

**Work (myelin-flow contribution):** the merge-queue workflow consumes the live ci.result; CI's pipeline-as-
workflow uses WfCtx + SCHEDULE_AND_RUN_JOB + the flow-determinism lint (no clock/RNG/IO outside WfCtx).

**Upstream dependency:** M3 green (Git produces commits/PRs; the merge gate + projection exist) + AG-D4 green
(the CI runner is the unified sandbox).

**Exit gate (the seam, CI/Git-owned, exercising myelin-flow):**
- **GIT-D10 / CI-D8** (CI) — push → ci.check.updated per context → green → merge; out-of-order/re-delivered;
  fork success neutral; merge-queue **wakes on ci.result idempotently; 0 double-merge; 0 spurious unblocks**.
  Green artifact: correct row; 0 double-merge. (This is the X-1 seam end-to-end — the most load-bearing
  cross-subsystem contract — and the durable-execution half is this engine's long-park + wait_for_signal.)
- **CI-D1** (CI) — kill runner + control plane mid-run → run resumes (replay + SCHEDULE_AND_RUN_JOB idempotent
  re-dispatch); effectively-once; 0 lost runs/double-deploys/duplicate publishes.
- **CI-D9** (CI) — the ci.pipeline workflow body: no clock/RNG/IO outside WfCtx; flow-determinism lint passes;
  replay bit-identical; only journaled job.done feeds the body.

### M5 — World-scale hardening + the floor follow-ons + the E2E wedge (PRODUCTION-HARDENED)

**Master band:** M5. **Thesis:** with all five subsystems on one substrate and the deterministic correctness
drills green, prove myelin-flow as a whole under world-scale load, ship the named floor follow-ons, and green the
E2E-2 flagship (the agent-native flagship) that exercises the full agent loop + durable workflow + HITL across
five subsystems.

**Work — world-scale hardening (the scheduled scale drills):**
- The 1M+ timer prod-scale run (FLOW-D3 at cell scale).
- The 30× agent-workflow surge with lane shedding (FLOW-D8).
- Crypto-shred reaching history (FLOW-D9).
- Restore-verify to a consistent point (FLOW-D10).
- The per-cell timer-wheel-promotion threshold measured (OQ #5) — the due-now rate at which the PG-indexed wheel
  yields to a dedicated scheduling tier or the escape hatch.

**Work — the floor follow-ons (each named in its band; here is its scheduled follow-on):**
- **History-archival / continue-as-new + object-store archival tier** (§7.5, OQ #4) — SPECIFIED-not-built floor.
  **Follow-on trigger: measured history growth.** Continue-as-new snapshot shape + terminal-run history archival
  to the object store. Scheduled in M5, promoted when growth is measured.
- **Cross-cell workflow spanning** (§7.4, OQ #3) — the named multi-cell FLOOR. v1 is single-home-cell;
  cross-cell spanning is designed-not-built, riding the control-plane PII-free pointer bridge (12.6, frame frozen
  CrossCellPointer{subject, type, correlation_id, home_cell}, resolution always cell-local). **Follow-on trigger:
  cross-cell rollup/collab/cross-org demand (OQ-I).** The DurableExecutor contract is cell-agnostic so it extends
  without a rewrite; the FLOOR drills GA-D8 / CP-D7 / CP-D8 are owed when multi-cell goes live (master M5).
- **Event-volume column-store seam** for the highest-volume workflow streams (EI-04 §5.2) — added **only once
  volume is measured**, not before.

**Work — the E2E wedge (myelin-flow's role in E2E-2, the agent-native flagship):** the durable-workflow + HITL
spine of E2E-2 — a failing CI run wakes a mock triage agent; the agent run is a workflow; the open_pr/git.merge
path is HITL-gated; the Agent+Workflow services are killed mid-ack_window; approval arrives days later
(double-click) → the workflow **resumes (FLOW-D4), consumes the approval exactly once, re-mints the run token on
resume (4.7), the merge applies once (FLOW-D1, no double-effect)**; the fix-PR's CI goes green → the merge-queue
workflow **wakes on ci.result idempotently (X-1) and merges**.

**Upstream dependency:** M4 green (the X-1 seam is live; all five subsystems exist).

**Exit gate (world-scale readiness for myelin-flow):**
- **FLOW-D3** (SCHED, cell scale) — **1M+ durable timers** + a burst due in one minute → due timers fire within
  the tick budget; far-future ~free; a crash re-fires unfired. **0 lost / 0 double-fire**.
- **FLOW-D8** (SCHED) — **30× surge** of agent-initiated workflows → **human-initiated lane holds, agent sheds**
  (429 + Retry-After), others unaffected. Green artifact: shed-counts/lane.
- **FLOW-D9** (SCHED) — erase a subject with inline-PII history/signal rows → **keys destroyed (unrecoverable
  incl. backups), references tombstoned, structure preserved**. Green artifact: crypto-shred-lag; 0 recoverable.
- **FLOW-D10** (SCHED) — restore myelin-flow PG to a consistent point → **in-flight runs resume; store↔outbox
  offsets↔referenced rows at one consistent point; no run pointing at a vanished result**. Green artifact:
  restore-verify; consistent point.
- **E2E-2 green** (the agent-native flagship) — the durable-workflow + HITL spine asserted exactly-once across
  the kill + days-later approval + the merge-queue wake.

### M6 — Dogfooding (no new engine work)

**Master band:** M6. Myelin's own CI pipelines, merge queue, SLA timers, and any agent runs become myelin-flow
workflows on the self-hosting platform. The dogfood loop exercises every engine path on the platform's own
commits. No new FLOW drill; the gate is the self-hosting CI graph green + the truth-up pass confirming 0 red
earlier FLOW gates.

---

## 3. Floors and their follow-ons (name-your-floors, master §5)

| Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| **100k-timer FLOW-D3 run** (algorithm proven at six figures) | M2.2 | **1M+ cell-scale FLOW-D3 + timer-wheel-promotion threshold measured** | M5 | the measured due-now rate (OQ #5) |
| **Single-home-cell workflows** (cross-cell spanning designed-not-built, §7.4) | M2/M4 | **Cross-cell workflow spanning** over the PII-free pointer bridge (12.6); FLOOR drills GA-D8/CP-D7/CP-D8 | M5 | cross-cell rollup/collab/cross-org demand (OQ-I) |
| **No history compaction** (full journal retained, §7.5) | M2 | **Continue-as-new + object-store archival tier** | M5 | measured history growth (OQ #4) |
| **Single-region event log** (general-purpose PG) | M0 (substrate) | **Column-store/time-series seam** for highest-volume workflow streams | post-M5 | event volume measured to outgrow the DB (EI-04 §5.2) |
| **Mock agent runtime** as the workflow's brain activity (--use-mock) | M2 | **Real LlmAgentRuntime** (config/impl swap behind AgentRuntime::step, owned by Agent Fabric) | post-M5 | after AG-D4/D2/D3/D5 green (VISION §3) |
| **The merge-queue workflow built-and-drilled-in-isolation** (mock ci.result producer) | M2.4 | **The X-1 seam end-to-end** (real CI ci.result producer) | M4 | CI's CheckStatus producer ships |

Every floor is tracked in the gap report with its claimed/proven status and linked follow-on; the gap being
*invisible* is the only failure (EI-04 §4). The DurableExecutor trait is **engine-agnostic** (the escape hatch:
a self-hosted Temporal/Restate engine behind the same trait, ADR-11-legal) — the cell-agnostic + engine-agnostic
contracts are what let cross-cell and history-archival extend without a rewrite.

---

## 4. The drills owed by myelin-flow, by band (quantified)

| Drill | Band claimed done | Threshold | Freq |
|---|---|---|---|
| FLOW-D1 | M2.1 | kill worker at 5/10 → resume at 6, 0 re-executed side effects, exactly-once-in-effect | CI |
| FLOW-D2 | M2.1 | divergent/wrong-version replay → halt nondeterministic + dead-letter, 0 silent divergence | CI |
| FLOW-D5 | M2.1 | crash between journal write + emit → journal+outbox co-commit, 0 ghost/0 lost | CI |
| flow-determinism lint fixtures | M2.1 | red rejects, green admits | CI |
| FLOW-D3 (floor) | M2.2 | 100k+ timers burst due in 1 min → fire within tick budget, 0 lost/0 double-fire | SCHED |
| FLOW-D4 | M2.3 | gated wait across restart+deploy, approval days later (double-click) → consume once, withhold = 0 mutation | CI |
| FLOW-D6 | M2.4 | runaway loop vs depleting wallet → refuse at reserve, 0 interrupt in-flight | CI |
| FLOW-D7 | M2.4 | adversarial wf→event→wf loop → depth ceiling + tripwire + bounded pool, 0 fork | CI |
| merge-queue in-isolation drill | M2.4 | mock ci.result double-delivery → one wake; timeout bounds vanished runner | CI |
| GIT-D10/CI-D8 (exercises this engine) | M4 | X-1 seam: merge-queue wakes on ci.result idempotently, 0 double-merge | CI |
| CI-D1 / CI-D9 (exercise this engine) | M4 | runner+CP kill → replay + idempotent re-dispatch; ci.pipeline determinism | CI |
| FLOW-D3 (full) | M5 | 1M+ timers within tick budget, 0 lost/0 double-fire | SCHED |
| FLOW-D8 | M5 | 30× agent-workflow surge → human lane holds, agent sheds, others unaffected | SCHED |
| FLOW-D9 | M5 | erase inline-PII history/signal → keys destroyed (incl. backups), structure preserved | SCHED |
| FLOW-D10 | M5 | restore PG → in-flight resume, store↔outbox↔rows one consistent point | SCHED |
| E2E-2 (durable-workflow + HITL spine) | M5 | exactly-once across kill + days-later approval + merge-queue wake | SCHED |

The gate invariant (master §4): no later FLOW milestone is claimed done over a red earlier FLOW gate. FLOW-D5
(co-commit, the silent-data-loss floor for the journal) and FLOW-D1 (exactly-once replay) are the
must-be-green-first pair — nothing rides this engine until they are green.

---

## 5. Digest

**Milestones (mapped to master bands):**
- **M2.1 (M2) — engine heartbeat / FIRST RUNNABLE:** data model + WfCtx core + deterministic replay + outbox
  co-commit + the flow-determinism lint fixtures + the divergence guard. Gate: FLOW-D1, FLOW-D2, FLOW-D5, lint.
- **M2.2 (M2) — durable timers at scale:** the minute-bucket wheel, sleep_until/sleep_for, cheap SLA re-arm.
  Gate: FLOW-D3 floor (100k+ timers).
- **M2.3 (M2) — durable signals + multi-day HITL + per-effect idem_key / FIRST USEFUL begins:** wf_signal,
  DurableExecutor::signal, wait_for_signal, the HITL round-trip mechanics. Gate: FLOW-D4.
- **M2.4 (M2) — long-park + reserve/settle + merge-queue frame / FIRST USEFUL complete:** SCHEDULE_AND_RUN_JOB,
  reserve-at-dispatch/settle-on-completion, mint_run_token mid-workflow re-mint, the merge-queue workflow body
  built-and-drilled-in-isolation. Gate: FLOW-D6, FLOW-D7, merge-queue in-isolation drill. (Band gate AG-D4 is
  Agent Fabric/CI-owned; the long-park dispatch into the runner is gated by it.)
- **M3 (producers) — merge gate consumer half goes live with Git:** no new engine; resumable maintenance
  activities. **M4 (consumers) — the X-1 seam end-to-end:** the merge queue wakes on the real ci.result; CI
  pipelines as workflows. Gate (CI/Git-owned, exercises this engine): GIT-D10/CI-D8, CI-D1, CI-D9.
- **M5 — world-scale hardening + floor follow-ons + E2E / PRODUCTION-HARDENED:** 1M+ timers (FLOW-D3), 30× surge
  (FLOW-D8), crypto-shred reach (FLOW-D9), restore-verify (FLOW-D10), the E2E-2 flagship durable-workflow+HITL
  spine. **M6 — dogfooding:** Myelin's own pipelines/queues/SLA-timers as myelin-flow workflows.

**Floors + follow-ons:** (1) 100k-timer floor → 1M+ cell-scale FLOW-D3 + measured promotion threshold (M5);
(2) single-home-cell → cross-cell workflow spanning over the PII-free pointer bridge, trigger = cross-cell demand
(M5); (3) no history compaction → continue-as-new + object-store archival, trigger = measured growth (M5);
(4) mock agent brain → real LlmAgentRuntime (post-M5, config swap); (5) merge-queue built-in-isolation against a
mock producer → the X-1 seam end-to-end against CI's real producer (M4); (6) single-region event log →
column-store seam for highest-volume streams, trigger = measured volume (post-M5).

**Critical upstream dependencies:**
- **M0:** serve(AppSpec) (1.1), the transactional outbox + idempotent-consumer template (2.2–2.5), the
  flow-determinism lint (1.6), the EventEnvelope (2.1), the failure-injection harness — all must be green before
  M2.1 (the engine emits only through the outbox and journals every non-deterministic interaction).
- **M1:** tenant+region partition (12.1) + RLS/OLTP storage (11.1) for M2.1; **mint_run_token (4.7)** and the
  **reserve/settle cost gate + wallet (11.7)** for M2.4.
- **M2 (peer, co-built):** the **unified runner ToolHands::exec (8.4, Agent Fabric)** is the SCHEDULE_AND_RUN_JOB
  dispatch target — and **AG-D4 (the sandbox-escape GATE) must be green before any long-park dispatch runs
  untrusted code**; Notif humanise (7.3) renders HITL cards; Agent Fabric owns the gated-tool set + EffectApi.
- **M4:** the **CI CheckStatus producer (5.9)** is what makes the merge-queue ci.result wait real — this engine
  owns the durable half of the single most load-bearing cross-subsystem seam (X-1), so the queue is
  built-and-drilled in M2.4 and goes live end-to-end in M4.
