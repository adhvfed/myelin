# Sketch 02 — A CI pipeline IS a durable workflow (the scheduler ↔ `myelin-flow` boundary)

> Phase 4 — CI exploration. Resolves how the CI run state machine, the distributed job scheduler, and
> the stage/step DAG map onto the Phase-3 durable-workflow substrate (`myelin-flow`, contracts §9), and
> what stays CI-owned. This is the Phase-3 handoff line: *"a CI pipeline is a durable workflow whose
> stages/steps are activities on the runner"* (README §5; Workflow §5.3/§11.7; drills Q17). It also
> answers Q17: the `kind=ci` job-spec → activity mapping granularity.

---

## The tension

Two readings of "a pipeline is a durable workflow" are both wrong at the extremes:

- **Too literal:** make *every step* a `WfCtx::activity` and let `myelin-flow` be the scheduler. But
  `myelin-flow` is a **deterministic-replay durable-execution engine** (DBOS-class, Workflow §2), not a
  fair-share multi-tenant job scheduler. The hard CI scheduler problems — fair-share across tenants,
  priority lanes, concurrency groups, affinity, pull-leasing onto an elastic fleet, dead-runner reaping
  (CI-DD §5.2) — are **not** what a durable-execution engine does. CI must own that (Phase-2 §1.2: the
  scheduler is CI's core competency, "the platform's single heaviest scheduling problem").
- **Too loose:** keep the run state machine entirely in CI's own Postgres and only "use a workflow for
  deploy approvals." Then the multi-day HITL gate, the SLA-style timers, the crash-recovery of a
  partially-run pipeline, and the reserve/settle bookends each get a bespoke CI reimplementation —
  exactly the second divergent state machine the doctrine warns against (Workflow §2.3; EI-02 §8).

## Candidate approaches

### A — `myelin-flow` is the run orchestrator; CI owns scheduling as an activity boundary

The **run = a `ci.pipeline` workflow definition** (a deterministic Rust function registered at
`serve`). Its `WfCtx` body walks the resolved DAG. **Each job is a `ctx.activity(SCHEDULE_JOB, spec)`**
whose implementation *enqueues onto CI's own scheduler and waits for the leased runner to report
terminal* (the activity blocks as a durable activity; the runner's terminal report is an activity
completion / or a signal). Deploy gates are `ctx.wait_for_signal("approval", window)`. Timers
(step timeout, queued-too-long, deploy wait) are `ctx.sleep_*`. The reserve/settle gate is the
workflow's bookends (Workflow §6.2; D8/CI-2).

- **Pros.** Crash-recovery, multi-day HITL waits, deterministic replay, and the reserve/settle bookends
  come **for free** from `myelin-flow` (its whole value, Workflow §1.1). The journal commits in the
  same Postgres transaction as the outbox event (DBOS embedding, Workflow §2.3) → "run started" the
  event and "run started" the state can't diverge. Versioning of the pipeline definition is the engine's
  (Workflow §4.6) — but see the snapshot note below.
- **Cons.** The CI scheduler still exists as its own thing (the activity *calls into* it); we must be
  careful the workflow's determinism constraint (Workflow §2.5) isn't violated by scheduler
  non-determinism — but it isn't, because the *scheduling decision* (which runner, when) happens
  **inside the activity**, whose *result* (the terminal job report) is journaled, not the decision.

### B — CI owns the run state machine; `myelin-flow` is used only for the long-wait pieces

CI's Postgres holds `run`/`job`/`step` state with leases/heartbeats; CI's scheduler drives transitions;
**only deploy approvals and SLA timers** call `DurableExecutor`.

- **Pros.** Maximum CI control; the hot scheduler path has no workflow-engine in it.
- **Cons.** Re-implements crash-recovery, the timer wheel, and the reserve/settle bookends that
  `myelin-flow` already provides; two state machines (CI run-state + workflow) to keep consistent for
  any run that *does* have a gate. Rejected as the default — it is the "too loose" reading.

### C — Hybrid (the chosen one): workflow owns **run lifecycle + gates + budget + timers**; CI owns the **scheduler + fleet + the job-execution activity**

This is approach A with the boundary drawn explicitly:

| Concern | Owner | Mechanism |
|---|---|---|
| Run lifecycle, crash-recovery, replay | `myelin-flow` | the `ci.pipeline` workflow def + journal |
| Deploy/manual gate (waits days) | `myelin-flow` | `ctx.wait_for_signal` (durable signal §9.4) |
| Step/queue/deploy timers, SLA | `myelin-flow` | `ctx.sleep_*` on the SC-11 timer wheel (§9.3) |
| Reserve/settle (no balance → no run) | `myelin-flow` bookends | reserve at workflow start, settle on completion (D8/CI-2) |
| **Which runner, when; fair-share; lanes; affinity; pull-leasing; dead-runner reaping** | **CI scheduler** | inside the `SCHEDULE_JOB` activity |
| Sandbox execution of the job | **CI runner** | `SandboxBackend::launch` (sketch 01); this *is* `ToolHands::exec` for `kind=agent` |
| Definition resolution + content-addressed snapshot | **CI** | before `start`, pinned into `StartSpec.input` |

---

## Leaning: **C (hybrid).** The pipeline run is a `myelin-flow` workflow; the scheduler/fleet is CI's, reached through a durable activity.

### The mapping, concretely

```
workflow ci_pipeline(run_input):              // deterministic; registered at serve(); the determinism lint guards it
  reserve_budget()                            // D8/CI-2 — refuse to START if wallet exhausted (never interrupt in flight)
  def = run_input.definition_snapshot         // content-addressed, resolved+pinned by CI BEFORE start (reproducibility)
  for stage in def.stages:                    // stages gate sequentially
    if stage.gate:                            // e.g. protected-env / manual approval
      d = ctx.wait_for_signal("approval:"+stage.id, timeout=stage.window)   // may wait DAYS (§9.4)
      if d.denied or d.timed_out: ctx.emit(ci.deployment.rejected); return
    results = parallel for job in stage.jobs (respecting `needs` DAG + concurrency group):
        ctx.activity(SCHEDULE_AND_RUN_JOB, JobSpec{ kind: Ci, .. })   // ← CI scheduler + runner live HERE
    if any(results.failed) and not stage.continue_on_error:
        ctx.emit(ci.run.failed, structured_failure(results))   // the agent-native triage hook (CI-DD §8.2)
        return
  ctx.emit(ci.run.succeeded); settle_budget()
```

- **Granularity (Q17 answer): the activity boundary is the JOB, not the step.** A *job* is the unit
  scheduled onto one runner in one sandbox (CI-DD §2.1); its steps run *inside* the sandbox and stream
  to the firehose (sketch 04). Making the *job* the activity keeps the journal small (one row per job,
  not per step/log-line — critical at CI's firehose volume) while preserving DAG-level crash recovery.
  Step-level progress is firehose/log state, recovered by re-running the job on retry, not journaled.
- **`SCHEDULE_AND_RUN_JOB` is a durable activity** (Workflow §4.4): it enqueues into CI's scheduler,
  the scheduler leases it to a runner, the runner launches the sandbox (sketch 01), streams logs, and
  reports terminal. The activity completes when the job reaches a terminal state; a dead runner →
  CI's reaper re-queues → the activity retries (idempotent on `idem_token`, Workflow §3.5). At-least-
  once activity + idempotent job = effectively-once (no double-deploy).
- **Determinism is preserved.** The scheduler's choice of runner is non-deterministic, but it happens
  *inside* the activity; only the journaled *result* (terminal job report: pass/fail, artifact refs,
  log pointer) feeds the deterministic workflow. The `flow-determinism` lint guards the workflow body.

### Why the scheduler stays CI-owned (not a workflow concern)

`myelin-flow`'s own dispatch is `partition = hash(run_id) % N` lease-stealing for *durability* (Workflow
§7.2) — that is **not** tenant-fair-share, priority lanes, affinity, or elastic-fleet bin-packing. CI's
scheduler is a distinct, harder problem (Borg/Nomad/Buildkite class, CI-DD §5.2) that the workflow
engine has no opinion on. The clean line: **`myelin-flow` decides *what runs next in this run*; CI's
scheduler decides *which runner runs it and when, fairly, across all tenants*.** Sketch 03 designs the
scheduler; this sketch only fixes that it sits behind `SCHEDULE_AND_RUN_JOB`.

### Definition snapshot vs workflow versioning

`myelin-flow` versions the *workflow definition* (the `ci_pipeline` orchestration code, Workflow §4.6).
CI separately content-addresses the *pipeline definition snapshot* (the customer's `.myelin/ci.*`
resolved at the triggering commit, with components pinned by digest — sketch 05, supply-chain). These
are orthogonal: a deploy of new `ci_pipeline` orchestration code drains old runs against the old code;
the *customer's* pipeline is data (`StartSpec.input`), snapshotted for reproducibility/audit. **Both
are forward-only** (STOR-2).

## Reserve/settle wiring (CI-2 / D8) — CI's obligation made concrete

The reserve/settle gate is **the workflow's bookends** (Workflow §6.2; contract 11.7). CI does **not**
build a second metering path: `reserve_budget()` at workflow start checks the prepaid balance + any
per-capability add-on and **refuses to start** when exhausted; `settle_budget()` on completion releases
the unused reserve; a long/expensive run never gets interrupted in flight (EI-03 §5.2). **The metering
unit** (what one reserve/settle event measures) is sketch 06 (TE-32). The wallet is Commercial's (C-1).

## Floors & follow-ons
- **FLOOR:** single-cell pipelines only; a pipeline whose jobs span cells of a multi-cell tenant is
  designed-not-built (inherits Workflow §7.4's cross-cell floor).
- **FLOOR:** history-archival/compaction for very-long pipelines inherits Workflow §7.5's tier.
- **Drill owed (PROVE-IT):** kill the runner mid-job → the run resumes (workflow replay + activity
  retry) with **no double-effect** (no double-deploy, no duplicate artifact publish); kill the control
  plane mid-run → the run resumes from the journal. Gate: **effectively-once job execution; zero lost
  runs.**
