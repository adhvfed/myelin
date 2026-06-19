# Phase 5 — Durable-Workflow Substrate (`myelin-flow`) — REFINED / CANONICAL

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth). Binding doctrine:
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) (EI-02)
> §3/§4/§5/§6/§8/§10, [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
> (EI-04) §1 (erasure), §5.2 (volume), and the agent-fabric doctrine (approval/cost/loops/storms).
> Reconciliation spine (binding): [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md)
> (resolves X-1..X-7, OQ-A..OQ-L) + [`contract-index.md`](./contract-index.md) (the frozen build-to surface,
> which **supersedes** the Phase-3 index). Phase-3 base this carries forward:
> [`../03-shared-systems-architecture/durable-workflow.md`](../03-shared-systems-architecture/durable-workflow.md).
> Change requests folded in:
> [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
> §7 (Durable Workflow), plus the §1/§2/§8/§10 asks that touch this engine, and conflict X-1 / open
> question OQ-F. Date: 2026-06-19.
>
> **What this refined doc is.** The canonical "Durable-workflow substrate" shared-system architecture that
> Phase 6 (roadmaps) and Phase 7/8 (build) build on. It **carries the Phase-3 design forward as the base**
> (the BUILD-vs-ADOPT decision, the data model, the replay/timer/signal algorithms, the contracts, the
> scaling story, the failure drills, the cited prior art are unchanged unless a "Changes vs Phase 3" entry
> says otherwise) and **applies the reconciliation decisions + this system's change requests**. No ADR is
> reversed (change-requests §14.1): every change here is **confirmation + additive sharpening + applying the
> reconciliation decisions**. Where a section is unchanged from Phase 3, this doc says so and cites the
> Phase-3 section rather than restating it.
>
> **Status convention (unchanged).** *DECIDED* = committed. *FLOOR* = partial answer shipped with a named
> follow-on. *[OPEN → P6/LEGAL]* = handed forward. Every failable property names the drill that proves it.

---

## Changes vs Phase 3 (every change, exhaustively)

The Phase-3 engine stands in full. The reconciliation adds **two frozen idioms** and **pins five
confirmations**; nothing is reversed. Each item cites its contract-index row and the reconciliation section.

1. **`SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom — SHARPENED → frozen** (contract 9.2/9.4,
   recon §OQ-F / change-requests §7). A first-class `myelin-flow` idiom: an activity **dispatches** a job
   (reserve at dispatch) and **returns immediately**; completion arrives **hours later as a durable signal**
   keyed by an `idem_token` the workflow minted at dispatch, so the workflow holds **no runtime** while a
   multi-hour CI/agent job runs. This is new prose pinning a pattern that rides the existing §4.2/§4.3/§4.4
   primitives — **no new engine**. New §4.9.

2. **Per-effect `idem_key` rule for batch / partial HITL approval — SHARPENED → frozen** (contract 9.1,
   recon §OQ-F / change-requests §7, CHAT). `DurableExecutor::signal` idempotency key is **per-effect**:
   `idem_key = card_id` for a single-effect card, `idem_key = card_id ":" effect_idx` for a multi-effect
   card. A double-click is one approval; a partial approval (approve effects 0 and 2, decline 1) is
   well-defined — each effect maps to exactly one `EffectApi::apply`, a declined effect is withheld (AG-8).
   This resolves the Phase-3 `[OPEN → P4]` batch-approval question (§6.3 tail). New §6.4.

3. **The merge-queue durable workflow + the `ci.result` rollup-signal wait — CONFIRMED, wiring pinned**
   (contract 9.4 / 5.9, recon §X-1 / OQ-A). The Git↔CI merge queue is named as a concrete `myelin-flow`
   workflow (one per target ref) that uses `SCHEDULE_AND_RUN_JOB` + `wait_for_signal("ci.result", …)`. This
   is the single most load-bearing cross-subsystem seam; the workflow side is pinned in §6.5. The seam's data
   shape (`CheckStatus`, `ci.result` payload) is owned by CI/Git (contract 5.9); this doc owns only the
   durable-workflow mechanics it rides.

4. **`wait_for_signal` waits gain two named signal names — CONFIRMED** (contract 9.4): the existing durable
   multi-day HITL signal mechanism (§4.3) now explicitly carries `ci.result` (merge queue) and `job.done`
   (`SCHEDULE_AND_RUN_JOB`) alongside `approval`/`cancel`. Same engine, two registered names.

5. **Mid-workflow agent-token re-mint on resume — CONFIRMED, contract pinned** (contract 4.7, recon §1 /
   change-requests §1, CHAT/CI). Phase 3 flagged this as `[OPEN → P4 Identity]` (§10.2, §11.2); reconciliation
   confirms `mint_run_token` is callable mid-workflow on resume (a days-later approval resumes under a fresh
   attenuated token). Closed; §6.2 updated, removed from the open-questions list.

6. **CI-pipeline-as-workflow stage/step granularity — CONFIRMED via `SCHEDULE_AND_RUN_JOB`** (recon §OQ-F,
   change-requests §7 CI). Phase 3 flagged this `[OPEN → P4 CI]` (§11.7); the `SCHEDULE_AND_RUN_JOB` idiom +
   the unified-runner `kind=ci|agent` job spec (X-6) is the answer. Closed at the substrate level; the exact
   per-pipeline definition remains CI's P6/P7 build detail.

7. **Resumable maintenance activities + cheap SLA-timer re-arm — CONFIRMED** (contract 9.3, change-requests
   §7, GIT/ISS). Git GC/repack/bundle-gen/history-rewrite as resumable journaled activities; Issues' cheap
   disarm/re-arm of a precomputed `fire_at` without polluting the wheel with calendar logic — both ride the
   existing timer wheel (§4.2) and activity model (§4.4). Pinned in §6.6.

8. **Reserve/settle fronts `SCHEDULE_AND_RUN_JOB` too — CONFIRMED** (contract 11.7, recon §X-6.1 universal
   cost gate): every dispatched job (CI *or* agent) reserves at dispatch and settles on the `job.done` /
   `ci.result` signal, never interrupts in-flight, meters into the same wallet. The bookend now wraps the
   long-park dispatch, not just the synchronous activity (§4.4 / §4.9).

**Unchanged from Phase 3 (carried forward verbatim, cited not restated):** the BUILD-vs-ADOPT decision
(DBOS-class Postgres-embedded, §2 of P3), the determinism constraint + lint + divergence guard (P3 §2.5),
the full data model (`workflow_run`/`wf_history`/`wf_timer`/`wf_signal`/`wf_activity_attempt`/`wf_definition`,
P3 §3), the deterministic-replay/recovery algorithm (P3 §4.1), the timer wheel (P3 §4.2), the signal
round-trip (P3 §4.3), activity execution + retry (P3 §4.4), the outbox seam (P3 §4.5), versioning (P3 §4.6),
lease dispatch (P3 §4.7), GDPR erasure on history (P3 §4.8), the `DurableExecutor`/`WfCtx` contract surface
(P3 §5.1), the telemetry set (P3 §5.4), `PersonalDataHolder` (P3 §5.5), the agent-run mapping (P3 §6.1/§6.2),
the scaling/sharding story incl. SC-11 millions-of-timers and the cross-cell + history-compaction floors
(P3 §7), the ten failure drills F-1..F-10 (P3 §8), the cited prior art (P3 §9). These are the engine; the
reconciliation did not touch them.

---

## 1. Purpose, responsibilities, and the one-paragraph thesis — CONFIRMED (unchanged from Phase 3 §1)

`myelin-flow` is the **durable-execution substrate**: deterministic workflow orchestration over
non-deterministic, retryable, sandboxed activities, with durable timers and durable signals, so a multi-step,
long-running, partially human-gated process survives crashes, restarts, and multi-day waits **without holding
a process or a thread**. It owns the workflow-run lifecycle + journal, the durable timer (SC-11: millions of
SLA timers), the durable signal (multi-day HITL), the activity primitive (at-least-once + idempotent), the
versioned definition registry, and the run's budget/gate/state ownership (ADR-08). It is **not** the event
bus, **not** the agent runtime, **not** the authorization engine, **not** the sandbox, and **not** a general
task queue — see Phase-3 §1.2 for the full boundary list (unchanged).

**One-paragraph thesis (unchanged, Phase-3 §1.3):** a Myelin workflow is a deterministic function whose every
non-deterministic interaction with the world — a timer firing, a signal arriving, an activity returning — is
**journaled to Postgres in the same transaction-discipline as the rest of the platform**, so durable execution
inherits the platform's correctness primitives (transactional outbox, idempotent consumers, crypto-shred,
tenant-partitioning, fail-static) for free instead of bolting on a second, divergent state machine. The
reconciliation **adds two idioms (long-park, per-effect idem_key) to this engine; it does not change the
engine.**

---

## 2. The BUILD-vs-ADOPT decision (TE-20) — CONFIRMED (unchanged from Phase 3 §2)

**DECIDED, unchanged:** BUILD a thin, Postgres-native, embedded durable-execution engine — `myelin-flow`,
DBOS-class — as a Rust crate over each service's existing Postgres; **not** self-hosted Temporal, **not** an
off-the-shelf Rust library. The semantics (ADR-09 — deterministic orchestration + retryable activities +
durable timers + signals) are adopted verbatim; the substrate is built to sit inside the platform's
residency/GDPR/outbox envelope. The candidate weighing, the "why DBOS-class wins for Myelin", the named
escape hatch (a self-hosted Temporal/Restate engine behind the same `DurableExecutor` trait, ADR-11-legal;
Temporal Cloud never is), and the determinism constraint + `flow-determinism` lint + replay-divergence guard
are all **carried forward unchanged** — see Phase-3 §2.1–§2.5. No reconciliation decision touches this
decision; the two new idioms (§4.9, §6.4) ride the chosen substrate.

---

## 3. The data model / schemas — CONFIRMED (unchanged from Phase 3 §3)

All tables live in the `myelin-flow` service's own Postgres (one DB per service), tenant+region first column,
RLS-enforced, per-tenant envelope-encrypted, crypto-shred-capable, harness-auto-registered as a
`PersonalDataHolder`. The full schemas are **carried forward unchanged** from Phase-3 §3:

- **`workflow_run`** (§3.1) — the run lifecycle + durable handle; `state ∈ {running, waiting, completed,
  failed, nondeterministic, terminated}`; `cursor` = replay short-circuit floor; `budget` = the owned
  `RunBudget`; causality columns (`correlation_id`/`causation_id`/`caused_by`/`depth`); `partition` +
  `lease_owner`/`lease_expires` for sharded lease dispatch. `input`/state are references-not-payloads.
- **`wf_history`** (§3.2) — the append-only journal, the source of truth; `command_id` deterministic from
  workflow position (the replay-match key); `UNIQUE(tenant, run_id, command_id)` makes journaling idempotent;
  `result_key_ref` envelope-encrypts the rare inline-PII result for crypto-shred.
- **`wf_timer`** (§3.3) — the durable timer; `bucket = epoch_minute(fire_at)` + the partial index
  `(bucket, partition) WHERE NOT fired` is the SC-11 world-scale move (a 30-day timer is never read until its
  minute).
- **`wf_signal`** (§3.4) — durably-buffered inbound signals; PK `(tenant, run_id, signal_name, idem_key)` —
  **this PK is exactly what makes the per-effect `idem_key` rule (§6.4) and the `SCHEDULE_AND_RUN_JOB`
  handshake (§4.9) idempotent by construction.** `payload_key_ref` crypto-shreds inline PII.
- **`wf_activity_attempt`** (§3.5) — the idempotency ledger; `idem_token` bridges to BUS-2 so a retried
  emit is broker-deduped. **The `SCHEDULE_AND_RUN_JOB` dispatch is one such attempt (§4.9).**
- **`wf_definition`** (§3.6) — the versioned definition registry; a run is pinned to `wf_version` at start
  so a deploy cannot diverge an in-flight run.
- **Stateful-component register** (§3.7, X-4) — unchanged.

**No schema change is required by the reconciliation.** Both new idioms are expressed entirely in the
existing `wf_signal` (the `idem_key` PK) + `wf_activity_attempt` (the dispatch attempt) + `wf_timer` (the
timeout branch) tables. This is the point of §4.9/§6.4 being *idioms*, not new primitives.

---

## 4. The algorithms — CONFIRMED + two new idioms

Phase-3 §4.1–§4.8 are **carried forward unchanged**: deterministic replay/recovery (§4.1), the timer wheel
(§4.2), the signal round-trip (§4.3), activity execution + retry with the reserve/settle bookend (§4.4), the
outbox seam — no second emit path (§4.5), versioning (§4.6), lease-based dispatch + crash recovery (§4.7),
GDPR erasure on history via references-not-payloads + crypto-shred + tombstone (§4.8). The reconciliation adds
one new algorithm subsection.

### 4.9 The `SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom (NEW — OQ-F)

> **Frozen** (contract 9.2/9.4, recon §OQ-F). A first-class `myelin-flow` idiom that composes the existing
> activity (§4.4), durable signal (§4.3), and durable timer (§4.2) primitives. **No new engine, no new
> table.** It is the seam between an external scheduler/runner (CI's runner pool, an agent job) and the
> engine: a job whose completion arrives **hours later** as a durable signal.

The problem it solves: a synchronous `ctx.activity(LONG_CI_STAGE, …)` (Phase-3 §4.4) would hold an activity
worker for the multi-hour life of a CI job — fine for a 30-second HTTP call, wasteful for a 2-hour build, and
it couples the workflow's liveness to a runner it does not own. The idiom **decouples dispatch from
completion**:

```
// inside a workflow definition (the merge queue, a CI pipeline, an agent run):
let job = ctx.activity(SCHEDULE_AND_RUN_JOB, JobSpec{ kind: ci|agent, ..., idem_token })?;
                                              // returns IMMEDIATELY: the job is dispatched, not awaited
ctx.wait_for_signal("job.done", idem_key = job.idem_token)?;
                                              // PARKS: state='waiting', holds NO runtime (Phase-3 §4.3)
// ... woken hours later by signal(run, "job.done", {result}, idem_key=job.idem_token) ...
```

**The mechanics, step by step (all existing primitives):**

1. **Dispatch (an activity, §4.4).** `SCHEDULE_AND_RUN_JOB` is an ordinary journaled activity. It mints the
   `idem_token` *at the workflow* (deterministic from `command_id`, so producer and consumer agree on the key
   **without coordination**), stamps it on the `JobSpec`, hands the spec to the unified runner (`kind ∈
   {ci, agent}`, ADR-20 / X-6), **reserves budget at dispatch** (contract 11.7 — the cost gate now fronts the
   long-park, not just the synchronous activity), journals `activity_completed{ job_dispatched: true,
   idem_token }`, and **returns**. It does **not** block on completion. The activity worker is freed.
2. **Park (a durable signal wait, §4.3).** The workflow immediately `wait_for_signal("job.done",
   idem_key = idem_token)`. It journals `signal_waited`, sets a **timeout timer** (§4.2) for the job's
   max-duration SLA (so a runner that vanishes does not park the workflow forever — the timeout branch
   fails/retries the job), flips `state='waiting'`, and **releases the worker**. The workflow now occupies a
   row, not a runtime, for however long the job runs.
3. **Completion (a signal, hours later).** When the runner finishes, it delivers
   `signal(run, "job.done", {result}, idem_key = idem_token)`. The `INSERT … ON CONFLICT (tenant, run_id,
   signal_name, idem_key) DO NOTHING` makes this **idempotent**: the runner can deliver "done" twice
   (at-least-once delivery is mandatory under the bus) and the workflow **wakes once**. The waiting workflow
   flips to `running`, a worker re-leases, replays to the wait, consumes the signal, and continues.
4. **Settle.** On the consumed result the workflow **settles budget** (contract 11.7) — reserve-at-dispatch /
   settle-on-completion, never interrupt in-flight. A `job.done{ failed }` runs the workflow's error branch
   (retry / compensate / dequeue) exactly like a failed synchronous activity.

**Why the `idem_token` is the agreement.** It is minted by the workflow at dispatch (deterministic on
`command_id`) and stamped on the job, so the **producer (runner) and consumer (workflow) share the dedup key
without any coordination round-trip**. This is the same property the activity ledger's `idem_token` gives a
direct emit (Phase-3 §3.5), generalised across the scheduler boundary. The `wf_signal` PK does the dedup; the
timeout timer bounds the wait; the reserve/settle bookend meters it. Three existing primitives, one named
idiom.

This idiom is the concrete pattern behind: the merge-queue's `ci.result` wait (§6.5, X-1), any long CI
stage (CI pipeline-as-workflow, the Phase-3 §11.7 question now answered), a protected-env deploy that waits
on an external job, and an agent tool that dispatches a long sandboxed computation. **Frozen** as
contract 9.2 (the `WfCtx` `SCHEDULE_AND_RUN_JOB` idiom) + 9.4 (the `job.done` durable signal wait).

---

## 5. Contracts exposed & consumed — SHARPENED (the two idioms freeze; everything else CONFIRMED)

### 5.1 Exposed — the durable-execution API (STABLE; the two idioms pinned)

The `DurableExecutor` trait and the `WfCtx` definition surface are **unchanged in shape** from Phase-3 §5.1;
the reconciliation **pins the contract semantics** of two of their methods (frozen, contract index 9.1/9.2/
9.4). The surface, with the frozen semantics annotated:

```rust
pub trait DurableExecutor {                          // engine-agnostic seam (escape hatch, §2)
    fn start(&self, spec: StartSpec, cause: Option<&EventEnvelope>) -> Result<RunId>;
    /// Deliver an external signal — HITL approval, cancel, ci.result, job.done.
    /// IDEMPOTENT on idem_key (FROZEN, contract 9.1). The per-effect idem_key rule (§6.4):
    ///   single-effect card  → idem_key = card_id
    ///   multi-effect  card  → idem_key = card_id ":" effect_idx
    /// A double-click is one approval; a partial approval is well-defined.
    fn signal(&self, run: RunId, name: &str, payload: Json, idem_key: &str) -> Result<()>;
    fn describe(&self, run: RunId) -> Result<RunStatus>;
    fn cancel(&self, run: RunId, reason: &str) -> Result<()>;
}

pub struct StartSpec { pub wf_type: String, pub input: Json, pub budget: Option<RunBudget>, pub idem_key: String }

pub trait WfCtx {                                    // the deterministic definition surface (flow-determinism lint)
    fn activity<I, O>(&mut self, def: ActivityDef, input: I) -> Result<O>;   // journaled, retried (§4.4)
    // The SCHEDULE_AND_RUN_JOB long-park idiom (FROZEN, contract 9.2, §4.9): an activity that
    // dispatches a job and returns; completion arrives as a durable signal keyed by idem_token.
    fn sleep_until(&mut self, t: Timestamp) -> Result<()>;                   // durable timer (§4.2)
    fn sleep_for(&mut self, d: Seconds) -> Result<()>;
    fn wait_for_signal<P>(&mut self, name: &str, timeout: Option<Seconds>)   // §4.3; FROZEN names
        -> Result<SignalOrTimeout<P>>;                //   include "approval", "cancel", "ci.result", "job.done"
    fn now(&mut self) -> Timestamp;                  // journaled side-marker
    fn rand(&mut self) -> u64;                        // journaled side-marker
    fn emit(&mut self, draft: EventDraft) -> Result<EventId>;               // via the OUTBOX (BUS-2)
}
```

**Units (unchanged, X-5-reconciled against substrate §2.10, restated once and frozen):** timers/timeouts in
**seconds**; timestamps **RFC-3339 UTC**; budgets/costs **integer minor-units** (never floats). `RunBudget`,
`EventEnvelope`, `EventDraft`, `ArtifactRef`, `RunId` are the canonical substrate types. The `idem_token`
minted for `SCHEDULE_AND_RUN_JOB` and the `idem_key` of `signal` are opaque strings under these PKs.

### 5.2 Consumed — CONFIRMED (unchanged from Phase 3 §5.2), one pin

The foundational contracts this engine stands on are **unchanged** (Phase-3 §5.2): `serve(AppSpec)` + the
consumer template + `OutboxTx::emit` + `FailStatic` + `PersonalDataHolder` + `ResilientClient` (substrate);
the envelope + outbox-only emit + Signal/Automation/Trigger tiers + firehose `tail` (Bus); `AgentRuntime::step`
+ `ToolHands::exec` + the unified runner (Agent Fabric + CI); KMS/crypto-shred + `BlobStore` (Storage/GDPR).
**One pin (contract 4.7, was Phase-3 §11.2 `[OPEN → P4]`):** `mint_run_token` / `delegation` / `check` /
`revoke` from Identity now explicitly support **mid-workflow re-mint on resume** — a multi-day HITL workflow
holds no long-lived privileged token; it re-mints a short-lived attenuated per-run token when it resumes after
a wait (token life == *activity* life, not the days-long *workflow* life). Confirmed, no longer open.

### 5.3 Who consumes `myelin-flow` — CONFIRMED + sharpened

Unchanged consumers (Phase-3 §5.3): the **Event Bus** (`action.kind = workflow` automations call `start`; the
stateful `Trigger`'s `stale_after` is one of our timers); **Agent Fabric** (every HITL-gated/multi-step agent
run *is* a workflow); **CI** (a pipeline is a workflow); **Issue tracker** (SLA timers SC-11 + the "unblock me
when…" Trigger UX); **Chat** (the HITL approval-card surface, posts the `approval` signal). Sharpened by the
reconciliation:

- **CI / Git merge queue** now consumes the `SCHEDULE_AND_RUN_JOB` idiom (§4.9) + the `ci.result` rollup-signal
  wait (§6.5, X-1). The merge queue is a `myelin-flow` workflow, one per busy target ref.
- **Chat** now consumes the per-effect `idem_key` rule (§6.4) for batch/partial approval cards.
- **Issues** confirms the cheap SLA-timer re-arm (§6.6, the `[OPEN]`-now-closed disarm/re-arm of a precomputed
  `fire_at` without calendar logic on the wheel).

### 5.4 Telemetry contract — CONFIRMED (unchanged from Phase 3 §5.4)

The drill-survival signals on the metrics-health port are **unchanged** (Phase-3 §5.4, contract 1.8):
runnable-run lag, timer-wheel lag (the SC-11 health signal), activity queue depth + retry rate + dead-letter
count, replay rate + `nondeterministic`-halt count (the divergence guard firing), **signal buffer depth +
oldest unconsumed wait age** (which now also covers `job.done`/`ci.result` long-parks — the merge-queue and
CI-stage backlog), per-tenant in-flight workflows/activities (fairness/agent-surge), causal-depth histogram,
reserve/settle reject rate. No new metric is required; the existing signal-buffer/oldest-wait metric is the
health signal for the long-park idiom.

### 5.5 `PersonalDataHolder` — CONFIRMED (unchanged from Phase 3 §5.5)

`locate`/`export`/`erase` over `workflow_run`/`wf_history`/`wf_signal` unchanged. The `SCHEDULE_AND_RUN_JOB`
`JobSpec` and the `job.done`/`ci.result` payloads are **references-not-payloads** (job ids, `ArtifactRef`s,
`CheckContext`s — PII-free), so the long-park idiom adds **no new PII surface**; the rare inline-PII case still
crypto-shreds via `payload_key_ref` (§3.4). The platform's ONE free-text/immutable-content erasure posture
(contract 10.9, recon §X-7) is instantiated here **by reference**: this engine's residual handling is the
references-not-payloads + crypto-shred + tombstone triad (§4.8) — no restatement of the posture.

---

## 6. The workflow↔agent mapping, HITL round-trip, and the new cross-subsystem idioms

### 6.1 Workflow owns budget/gates/state; reasoning + tools are activities — CONFIRMED (unchanged, Phase 3 §6.1)

An agent run is a workflow whose deterministic body is the plan-then-apply loop (ADR-08.3): it owns the
`RunBudget`, the HITL `gates`, the conversation cursor as durable state; the brain `step` and the tool `exec`
are activities. The mapping, mock/real-runtime swap, and "conversation history is workflow state" are
**unchanged** — see Phase-3 §6.1.

### 6.2 Cost & loop safety — CONFIRMED + mid-workflow token re-mint pinned

Cost pre-flight (reserve refuses to *start* when the wallet is exhausted, never interrupts in-flight), loop
safety (causal-depth ceiling + shared-root tripwire + bounded activity pool), and concurrency caps are
**unchanged** (Phase-3 §6.2). **One sharpening (now closed):** a multi-day HITL workflow's agent token expires
during the wait; on resume the workflow **re-mints** a fresh short-lived attenuated per-run token via
`mint_run_token` (contract 4.7) — the workflow never holds a long-lived privileged token. This was Phase-3
§11.2 `[OPEN → P4 Identity]`; reconciliation §1 confirms `mint_run_token` is callable mid-workflow on resume.

### 6.3 The HITL approval-card round-trip — CONFIRMED (unchanged from Phase 3 §6.3)

The EI-03 §5.1 "approve→resume bridge" is unchanged: a gated tool → `ctx.wait_for_signal("approval:<call>",
timeout=window)` → emits `agent.approval.requested` via the outbox (payload: tool name, args `ArtifactRef`s,
risk, a **live cost estimate** from the reserved budget) → Notif/Chat renders the **approval card** (humanised
at the backend, NOTIF-1 / contract 7.3 — the **one** templating surface, OQ-L); the workflow is `state=waiting`
holding no runtime for up to `window` (which may be days); a human clicks Approve/Deny → Chat calls Id
`check(human, approve, run)` then `DurableExecutor::signal(run, "approval:<call>", {approved, by}, idem_key)`;
the signal lands in `wf_signal` (idempotent), the workflow resumes, runs (approved) or withholds (denied →
ordinary error, no mutation, AG-8) or takes the timeout path. The **durability is the point**: the approval
can arrive days later, across restarts and deploys. The Phase-3 `[OPEN → P4]` on the card's data model/visual
design remains a Chat+Agent-Fabric product deliverable; **the batch-approval semantics question is now closed
by §6.4.**

### 6.4 Per-effect `idem_key` for batch / partial HITL approval (NEW — OQ-F)

> **Frozen** (contract 9.1, recon §OQ-F). Resolves the Phase-3 §6.3 `[OPEN → P4]` "whether a card can approve
> a *batch* of gated calls." No engine change — it is a key-construction rule over the existing `wf_signal` PK.

A batch approval card may gate **multiple effects** (e.g. "approve these 3 proposed merges"). The signal
idempotency key is **per-effect**:

```
idem_key = card_id                    // single-effect card: one approval; a double-click is idempotent
idem_key = card_id ":" effect_idx     // multi-effect card: each effect approved independently + idempotently
```

A **partial approval** (approve effects 0 and 2, decline 1) sends three signals —
`{card_id:0 = approve}`, `{card_id:1 = decline}`, `{card_id:2 = approve}` — each idempotent on its **own** key
(`wf_signal` PK `(tenant, run_id, signal_name, idem_key)`), each mapping to **exactly one** `EffectApi::apply`.
A declined effect is **withheld** (AG-8: returns a `Denied` tool error, never mutates). A double-click on
"approve all" re-sends the same keys → `ON CONFLICT DO NOTHING` → **no double-apply**. This makes both
invariants true by construction: *a double-click is one approval*, and *a partial approval is well-defined*.
The workflow's loop consumes the per-effect signals and gates each tool call independently. Co-owned with Chat
(the card UX) and Agent Fabric (the effect set); the **engine contribution is the key rule + the PK** — both
already exist.

### 6.5 The merge-queue durable workflow + the `ci.result` rollup wait — CONFIRMED, wiring pinned (X-1)

> **Pinned** (contract 9.4 / 5.9, recon §X-1 / OQ-A — the single most load-bearing cross-subsystem seam). The
> `CheckStatus` data shape, the `(commit_oid, context)` keying + `run_attempt` last-writer-wins supersession,
> the `trust_tier` fork-gating, and the Git-owned `check_status` projection + branch-protection policy are
> owned by **CI (producer) + Git (gate)** (contract 5.9) — **not by this engine**. This doc owns only the
> durable-workflow mechanics the merge queue rides.

A **merge queue serialises merges into a busy target ref**; it is a `myelin-flow` workflow (`DurableExecutor::
start`, contract 9.1) — **one workflow per target ref**. For each queued PR the workflow:

1. computes the speculative merge commit and **dispatches the required CI** via the `SCHEDULE_AND_RUN_JOB`
   idiom (§4.9) — reserve at dispatch, return immediately;
2. `wait_for_signal("ci.result", idem_key = <merge_attempt_id>)` — **parks, holds no runtime** while CI runs
   (contract 9.4), waking hours later if needed; the timeout branch (§4.2) bounds a vanished CI run;
3. on a `success` `ci.result` for all required contexts → performs the merge, emits `git.pr.merged` via the
   outbox (BUS-2), settles budget; on `failure`/`error` → dequeues the PR with a humanised reason
   (contract 7.3) and continues the queue.

The `ci.result` signal payload is `{ commit_oid, overall: success|failure, contexts: [CheckContext],
idem_token }`; `signal` is idempotent on `idem_key` (a double-delivery is one wake). **`ci.result` is a
CI-derived rollup signal** that drives the merge-queue workflow's resume — distinct from the per-context
`ci.check.updated` *events* that drive the always-visible PR-checks UI via Git's projection (both emitted by CI
via the outbox; the projection vs. the signal is the events-vs-rollup split of X-1). This is the canonical
application of the §4.9 long-park idiom + the §4.3 durable signal: the merge queue holds no runtime across a
multi-hour CI run and resumes exactly where it parked, across worker restarts and deploys.

### 6.6 Resumable maintenance activities + cheap SLA-timer re-arm — CONFIRMED (X-1 §9.5 wiring)

Two confirmations from change-requests §7, both riding existing primitives (Phase-3 §4.2/§4.4):

- **Maintenance ops as resumable journaled activities (GIT).** Git GC / repack / bundle-gen / history-rewrite
  (the erasure-admin op, contract 10.6) run as activities (or `SCHEDULE_AND_RUN_JOB` long-parks for the heavy
  ones) on a workflow; a crash mid-repack replays to the un-journaled step (§4.1) — resumable with no
  re-executed side effect. The history-rewrite invalidation fan-out (fork/mirror/clone-cache, ties to the
  trust-scoped cache namespaces, contract 11.2) is a sequence of journaled activities.
- **Cheap SLA-timer disarm/re-arm (ISS, was *blocking*).** Issues re-arms a precomputed `fire_at` by
  updating `wf_timer.fire_at` (and its `bucket`) — a cheap row update, **no calendar logic on the wheel**
  (the wheel only ever scans `bucket <= now AND NOT fired`, §4.2). A disarm sets `fired=true` (or deletes the
  row); a re-arm is a new `fire_at`/`bucket`. Millions of SLA timers re-arm at row-update cost, not wheel-scan
  cost — the SC-11 property holds under churn. Confirmed unblocked.

---

## 7. Scaling / sharding in the cell topology — CONFIRMED (unchanged from Phase 3 §7)

**Unchanged, carried forward** (Phase-3 §7): in-cell + tenant-partitioned (§7.1); worker sharding by
`partition = hash(run_id) % N` (§7.2); the SC-11 **millions-of-durable-timers** world-scale case via the
bucketed partial index (§7.3); **cross-cell workflow spanning** is the named **FLOOR — designed-not-built**
(§7.4), riding the control-plane PII-free pointer bridge (contract 12.6, recon §OQ-I — confirmed as the named
multi-cell floor, frame frozen `CrossCellPointer{subject, type, correlation_id, home_cell}`, resolution always
cell-local); the **history-archival/compaction tier** is **SPECIFIED — not built** (§7.5, promotion trigger =
measured history growth, continue-as-new + object-store archival); **bounded everything** with the
principal-aware shed order (§7.6); the stateful register (§7.7 → §3.7).

**The long-park idiom does not change the scaling story:** a parked `SCHEDULE_AND_RUN_JOB`/`ci.result` wait is
a `state='waiting'` row holding no runtime (exactly like a parked HITL approval), so a cell can hold millions
of in-flight-but-parked merge-queue / CI-stage / HITL workflows at the cost of rows, not runtimes — the same
SC-11 property that makes millions of SLA timers free. The per-surface shed budgets (contract 1.11, recon
§OQ-K) name CI-dispatch as a bounded run-queue per tenant on the batch/CI lane (shed order
speculative → batch/CI → agent → human-last); an agent-mention storm sheds its lane with `429 + Retry-After`
while human-initiated workflows hold the protected lane — the F-8 drill asserts this.

---

## 8. Failure modes + the drills owed — CONFIRMED + two extended assertions

The **ten drills F-1..F-10 are carried forward unchanged** (Phase-3 §8): F-1 crash-mid-workflow (zero lost
progress, exactly-once-in-effect), F-2 non-determinism/unversioned-divergence (halt + dead-letter, 0 silent
divergence), F-3 durable-timer-at-scale (1M+ timers, fire within budget, 0 lost/0 double-fire), F-4
multi-day-HITL-signal-round-trip (resume after days, double-signal = one approval, withheld tool no mutation),
F-5 outbox-coherence (0 ghost / 0 lost), F-6 budget-exhaustion-self-limits (no balance → no new spend,
in-flight unharmed), F-7 causal-loop-tripwire (halt ≤ ceiling), F-8 30×-agent-surge/fairness (human lane
holds, cross-tenant unaffected), F-9 crypto-shred-reaches-history (0 recoverable PII), F-10 restore +
cross-seam integrity (consistent resume, no orphaned references).

**The reconciliation extends two existing drills (no new drill needed — the idioms ride existing mechanisms):**

- **F-4 (multi-day HITL signal) now also asserts the `SCHEDULE_AND_RUN_JOB` long-park and the per-effect
  `idem_key`.** Concretely: (a) start a merge-queue / CI-stage workflow that dispatches a job and parks on
  `wait_for_signal("ci.result"/"job.done", idem_key=idem_token)` across a worker restart + a deploy; deliver
  the completion signal hours later **twice** (at-least-once) → assert it resumes, consumes **once**, settles
  budget once; (b) a multi-effect approval card sends `{card_id:0=approve, card_id:1=decline, card_id:2=approve}`
  with a double-click on "approve all" → assert each effect applies/withholds exactly once, the declined effect
  never mutates (AG-8). Gate: **resume-after-days works; double-signal / double-click = one effect; partial
  approval is well-defined; withheld effect does not mutate.** Reads: signal buffer depth, oldest-wait age.
- **F-6 (budget self-limit) now also asserts reserve-at-dispatch for `SCHEDULE_AND_RUN_JOB`.** Dispatch a job
  with a depleting wallet → assert the **dispatch** is refused at reserve (the job never starts) when the
  wallet is exhausted, and an already-dispatched/in-flight job is **never interrupted**, settling on
  completion. Gate: **no balance → no new dispatch; in-flight job unharmed.** Reads: reserve/settle reject rate.

Phase 6/7 executes these drills; this doc enumerates the obligation (PROVE-IT / T-4).

---

## 9. Cited prior art — CONFIRMED (unchanged from Phase 3 §9)

Unchanged — see Phase-3 §9 for the full bibliography. The model is adopted from **Temporal/Cadence** (event
history, deterministic replay, activities as the non-deterministic boundary, signals, durable timers, "crash
at step 5 of 10, replay to step 6", workflow versioning), **Restate** (the journal framing), **Azure Durable
Functions** (replay orchestration), **Resonate / Vanlightly 2025** (the determinism constraint); the substrate
is **DBOS** (Postgres-embedded journaling, every step result committed in the same transaction); coherence from
**Richardson** (transactional outbox), **Kleppmann DDIA** (dual writes / exactly-once-as-illusion), **Helland
2012** (at-least-once + idempotent ≈ effectively-once), **`FOR UPDATE SKIP LOCKED`**; timers from **Varghese &
Lauck 1987** (hierarchical timing wheels); causality from **Lamport 1978** + **W3C Trace Context**; resilience
from **Nygard** + **Brooker 2015**.

**The two new idioms cite no new prior art** — they are compositions of the already-cited primitives
(the `SCHEDULE_AND_RUN_JOB` long-park is Temporal's "activity completion via signal / `signalWithStart`"
pattern realised over the DBOS journal; the per-effect `idem_key` is Helland idempotency applied per-effect).

---

## 10. Required changes to foundational systems — CONFIRMED + closures

Phase-3 §10's three items stand, with two now **closed** by the reconciliation:

1. **Event Bus** — unchanged: the stateful `Trigger`'s `stale_after` is a `myelin-flow` timer; the
   `action.kind = workflow` automation calls `DurableExecutor::start`. **New registered signal names**
   `ci.result` and `job.done` join `approval`/`cancel` (no bus change — they are `wait_for_signal` names and
   `ci.*` event/signal tokens registered under the §6 grammar, contract 2.9).
2. **Identity** — **CLOSED** (was Phase-3 §10.2 / §11.2 `[OPEN → P4]`): `mint_run_token` is callable
   mid-workflow on resume (contract 4.7, recon §1) — a multi-day HITL workflow re-mints a fresh short-lived
   per-run token on resume. No remaining Identity open item from this engine.
3. **Platform substrate** — unchanged: the `flow-determinism` lint (a workflow function reading clock/RNG/IO
   outside `WfCtx` fails to compile) is in the §2.11 lint table (contract 1.6). Ships with this engine.

No change to ADR-09 (this implements it). No store outside `myelin-flow` is read; no second emit path is
introduced. The `SCHEDULE_AND_RUN_JOB` idiom's job dispatch goes through the unified runner (CI/Agent, X-6) —
the runner is the sandbox, not this engine (Phase-3 §1.2 boundary unchanged).

---

## 11. Open questions for Phase 6 — REFINED (two closed, the rest carried)

Phase-3 §11 had seven open questions; the reconciliation **closes three** and **carries four** (re-scoped to
Phase 6/build):

- **CLOSED — #2 Mid-workflow token re-mint on resume.** Confirmed by reconciliation §1 / contract 4.7
  (callable mid-workflow). See §6.2.
- **CLOSED — #7 CI-pipeline-as-workflow stage/step granularity.** Answered at the substrate level by the
  `SCHEDULE_AND_RUN_JOB` long-park idiom (§4.9, OQ-F) + the unified-runner `kind=ci` job spec (X-6). The exact
  per-pipeline definition is CI's P6/P7 build detail, not a substrate open question.
- **PARTIALLY CLOSED — #1 HITL approval-card batch semantics.** The **batch/partial-approval semantics are
  closed** by the per-effect `idem_key` rule (§6.4, OQ-F). The card's **visual design + data model** remain a
  Chat + Agent-Fabric **product/UX** deliverable for Phase 6 (not a substrate question). `[OPEN → P6 Chat +
  Agent Fabric]`.
- **CARRIED — #3 Cross-cell workflow spanning** (FLOOR, §7.4). Reconciliation §OQ-I confirms the cross-cell
  PII-free pointer bridge as the named multi-cell floor (frame frozen, contract 12.6), but **single-home-cell
  is v1; cross-cell workflow spanning is designed-not-built**. `[OPEN → P6 control-plane / SC-2/SC-3]`, jointly
  with the bus's identical floor. The `DurableExecutor` contract is cell-agnostic so this extends without a
  rewrite.
- **CARRIED — #4 History compaction / continue-as-new + archival tier** (§7.5). Measured-growth promotion
  trigger; continue-as-new snapshot shape; object-store archival of terminal-run history. `[OPEN → P6,
  measured]`.
- **CARRIED — #5 Per-cell timer-wheel promotion threshold** (§7.3). The measured due-now rate at which the
  PG-indexed wheel yields to a dedicated scheduling tier (or the escape hatch). `[OPEN → P6, measured]`.
- **CARRIED — #6 Workflow-definition authoring for non-engineers.** Whether the no-code automation builder
  compiles to a *constrained* workflow definition or only to stateless automations; the safe subset of
  `WfCtx` exposable to tenant-authored automations. `[OPEN → P6 product]`.

**`[OPEN — LEGAL]` items instantiated by reference (not owned here):** the ONE free-text/immutable-content
erasure posture (contract 10.9, recon §X-7) and the fail-static staleness-bound ratification (contract 4.11,
L-1) are GDPR/Audit + Identity deliverables; this engine's structural floor (references-not-payloads +
crypto-shred + tombstone, §4.8) ships regardless. No new `[OPEN — LEGAL]` item originates in this engine.

---

## 12. Cross-references

- Reconciliation spine: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (X-1 / OQ-A,
  OQ-F, OQ-I, OQ-K, X-6, X-7); [`contract-index.md`](./contract-index.md) (contracts 9.1/9.2/9.3/9.4/9.5/9.6
  durable workflow; 5.9 the CheckStatus seam; 11.7 reserve/settle; 4.7 mint_run_token; 7.3 humanise; 1.6 lints;
  1.8 telemetry; 12.6 cross-cell bridge).
- Phase-3 base this carries forward (unchanged unless noted in "Changes vs Phase 3"):
  [`../03-shared-systems-architecture/durable-workflow.md`](../03-shared-systems-architecture/durable-workflow.md).
- Change requests: [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
  §7 (Durable Workflow), §1 (token re-mint), §8 (reserve/settle, crypto-shred), §10 (cross-cell), §12 X-1.
- Spine: [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
  — **ADR-09** (this implements its TE-20 decision), ADR-08 (agent fabric), ADR-11 (cells), ADR-12 (GDPR),
  ADR-16/17, ADR-19/04 (bus), ADR-20 (unified runner).
- Doctrine: [`../../external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
  §3/§4/§5/§6/§8/§10; [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
  §1 (erasure), §5.2 (volume).
- Consumed by (P6/P7): Agent Fabric, CI (pipeline + merge-queue), Git (merge queue), Issue tracker
  (SLA/Trigger UX), Chat (approval card).
