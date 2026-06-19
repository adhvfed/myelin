# CI/CD — 02 Internals & Algorithms

> Phase 5-B — CI detailed architecture (rewritten against the reconciled layer). The subsystem-specific
> algorithms in depth: trigger→dispatch (incl. the trust-tier stamp + the `CheckStatus` emit), the
> pipeline-as-durable-workflow mapping (the **frozen `SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal
> idiom**, OQ-F), the distributed scheduler (DRR fair-share / lanes / concurrency / affinity / leasing /
> reaping), the EU fleet autoscaler, the **unified sandbox runner + the four uniform guarantees + the escape
> drill** (X-6), and logs/artifacts/caches/secrets. Each hard problem is resolved + cited; 05 consolidates.

---

## 1. Trigger → dispatch (the front of every run)

A run begins when Trigger & Dispatch consumes a triggering event off the durable bus (`git.ref.updated`,
`git.pull_request.synchronized`, `issue.transitioned`, a manual API call, a schedule timer, or an agent
request). The path:

1. **Match** — evaluate the project's armed pipelines against the event via the shared **`EventMatcher` =
   the frozen `QueryAst`** (contract 3.4 — bounded interpreter, no UDFs/loops/recursion, statically
   cost-bounded, permission-aware; **not CEL**). CI does **not** invent a trigger language; a
   `on: pull_request: {...}` compiles to a `QueryAst`. This runs close to the bus, cheaply.
2. **Dedup** — on the triggering `event_id` via the `consumer_dedup` ledger (contract 2.5). One push = one
   run (**exactly-once *effect*** even under at-least-once delivery; Helland, *Idempotence Is Not a Medical
   Condition*, 2012).
3. **Trust-tier evaluation + stamp** — classify the run `Trusted` (member push) / `UntrustedFork` (PR from a
   fork, or any run executing untrusted contributor code) / `SelfHosted` (targets a self-hosted pool), using
   run provenance + the ReBAC ABAC edge `read & !is_untrusted_fork` (contract 4.9). The result is **stamped
   once** onto (a) the `JobSpec.trust_tier` (gating secrets/cache-scope/egress) and (b) every emitted
   `CheckStatus.trust_tier` (X-1). This is the security-critical stamp: an `untrusted_fork` success cannot
   turn the gate green by itself (Git's rule; §4 / X-1).
4. **Definition resolution → content-addressed snapshot** — read `.myelin/ci.*` at the triggering commit,
   validate against the published JSON Schema, expand the matrix deterministically, resolve every
   component/image reference **to a digest** (fail-closed on a floating tag, 05 §HP-4), and write the
   resolved DAG as a **CAS blob** (T2). This snapshot is the run's reproducible, auditable definition; it is
   identical to the `myelin ci plan` output (shift-left, 04 §CLI).
5. **Reserve + start** — call `DurableExecutor::start(StartSpec{ input: snapshot_ref, .. })` for the
   `ci.pipeline` workflow; the workflow's first act is the reserve bookend (refuse-start-on-exhaustion; §6).
   The `ci_run` row is written and `ci.run.started` + the first `ci.check.updated{state: queued}` per context
   are emitted **via the outbox in the same tx**.

## 2. The distributed scheduler (the hard core — carried forward)

### 2.1 Pull-leasing (the assignment model)

Runners **claim** work rather than the control plane pushing it (the Buildkite-agent / Nomad-pull model). A
runner long-polls the `job_queue`, claims the next eligible job for its labels via `FOR UPDATE SKIP LOCKED`,
takes a **lease** (`lease_owner` + `lease_expires`), and **heartbeats** to extend it. This reuses the
platform's existing lease primitive (the outbox relay, the timer wheel) — proven, not novel — and scales
horizontally (more runners = more pulls) with no central live-capacity tracking. A **dead-runner reaper**
sweeps expired leases and re-queues their jobs, which makes the run's `SCHEDULE_AND_RUN_JOB` activity retry
idempotently (§3).

The claim is the scheduler's whole intelligence; it encodes fairness, lanes, concurrency, affinity, and
residency **as predicates in one query**:

```sql
-- The claim (conceptual): a runner claims the highest-priority, fairest, label-eligible, in-region job.
WITH eligible AS (
  SELECT q.* FROM job_queue q
  WHERE q.state='queued'
    AND q.region = $cell_region                          -- RESIDENCY: in-region only (no global pool)
    AND q.labels <@ $runner_labels                       -- AFFINITY: job labels ⊆ runner labels
    AND q.trust_tier = ANY($runner_allowed_tiers)        -- TRUST: untrusted never reaches a self-hosted-trusted runner
    AND NOT EXISTS (                                       -- CONCURRENCY (serialize): one deploy:prod at a time
      SELECT 1 FROM job_queue r
      WHERE r.concurrency_group = q.concurrency_group AND r.state='running'
        AND q.concurrency_group LIKE 'deploy:%')
)
SELECT * FROM eligible e
JOIN fair_deficit f USING (tenant, region, fair_key)
ORDER BY lane_priority(e.lane) DESC,                      -- LANES: interactive > batch > deploy (strict)
         f.deficit DESC,                                  -- FAIRNESS: least-recently-served tenant first (DRR)
         e.enqueued_at ASC                                -- then oldest
FOR UPDATE SKIP LOCKED LIMIT 1;
-- On claim: set lease_owner/expires, state='leased', advance fair_deficit for the claimed fair_key.
```

### 2.2 Fairness — Deficit Round Robin over `fair_key`

The fairness intuition is **DRR** (Shreedhar & Varghese, *Efficient Fair Queueing using Deficit Round
Robin*, SIGCOMM 1996), applied at claim time, with the intuition of Linux CFS. Each `fair_key`
(= tenant, or tenant:project) holds a **deficit counter**; a runner claims the oldest job of the
*least-recently-served* eligible tenant, not the globally-oldest job. On claim, the served tenant's deficit
is decremented (and periodically replenished, weighted by plan tier). This prevents one tenant's 10k-job
matrix from starving every other tenant — **the canonical CI multi-tenant fairness failure**. *Floor:* DRR
ships; a richer hierarchical (per-tenant→per-project→per-pipeline) scheduler is **promotion-triggered by a
measured starvation signal** (the per-`fair_key` wait-time histogram + lag telemetry, contract 1.8).

### 2.3 Priority lanes, concurrency groups, affinity

- **Lanes** (`interactive` > `batch` > `deploy`) are a strict order in the claim's `ORDER BY`. This is the
  **protected-human-lane analogue inside CI**: interactive PR-check feedback must never queue behind a
  nightly batch matrix. It composes with the platform shed order (speculative → batch/CI → agent →
  human-last, contract 1.11): under surge, CI sheds the batch lane first, holds interactive. The per-surface
  shed budget for CI dispatch is the named v1 floor (OQ-K: bounded run-queue per tenant; runners
  pull-bounded; CI and agent share the wallet, so shed order is speculative → batch/CI → agent → human-last).
- **Concurrency groups** — `deploy:prod` is a **serialization key** (the partial unique index `jq_serialize`;
  one running at a time). `pr:web:42` is **cancel-superseded** (a new push to the PR cancels the in-flight
  run for that group, so only the latest PR head is tested). Both are claim-time predicates + a cancel hook
  on enqueue.
- **Affinity** — `labels <@ runner_labels` (job labels are a subset of the runner's). `gpu`, `arm64`,
  `large`, `linux`.

### 2.4 Backpressure & abuse

Per-tenant in-flight caps (a bounded run-queue, OQ-K), statement timeouts, and a per-tenant in-flight
ceiling: over-cap jobs **queue** gracefully, never collapse the scheduler. A 30× surge on one tenant sheds
the batch/CI lane (429 + `Retry-After`, honoured by the `myelin ci` `ResilientClient`), holds the
interactive lane, and **leaves other tenants unaffected** (the CI-surge drill, 07 D-2). Crypto-mining abuse
(sustained high-CPU / no-IO) is flagged by a heuristic; the **economic** control is the wallet (reserve: no
balance → no start, §6), the **structural** control is the bounded queue + the sandbox `pids.max`/cpu limits.

## 3. The pipeline IS a durable workflow (the `myelin-flow` mapping + the frozen OQ-F idiom)

### 3.1 The hybrid boundary (the chosen model)

A run = a `ci.pipeline` **workflow definition** (a deterministic Rust function registered at `serve`, guarded
by the `flow-determinism` lint). The engine owns lifecycle/replay/timers/HITL-waits/reserve-settle for free
(its whole value); CI owns the scheduler/fleet, reached through the **frozen `SCHEDULE_AND_RUN_JOB` idiom**.
The boundary, explicitly:

| Concern | Owner | Mechanism |
|---|---|---|
| Run lifecycle, crash-recovery, deterministic replay | `myelin-flow` | the `ci.pipeline` workflow def + its journal |
| Deploy/manual gate (waits days) | `myelin-flow` | `ctx.wait_for_signal("approval:<stage>", window)` (contract 9.4) |
| Step/queue/deploy SLA timers | `myelin-flow` | `ctx.sleep_*` on the timer wheel (contract 9.3) |
| Reserve/settle (no balance → no run) | `myelin-flow` bookends | reserve at dispatch (incl. each `SCHEDULE_AND_RUN_JOB`), settle on `job.done` (contract 11.7) |
| **Which runner, when; fairness; lanes; affinity; leasing; reaping** | **CI scheduler** | inside `SCHEDULE_AND_RUN_JOB` dispatch |
| Sandbox execution of the job | **CI runner** | `SandboxBackend::launch` (§4); = `ToolHands::exec` for `kind=agent` |
| Definition resolution + CAS snapshot + trust-tier stamp | **CI** | before `start`, pinned into `StartSpec.input` |

```text
workflow ci_pipeline(run_input):                 // deterministic; the flow-determinism lint guards it
  reserve_budget()                               // D8/CI-2 — refuse to START if wallet exhausted (never interrupt in flight)
  def = run_input.definition_snapshot            // content-addressed, resolved+pinned by CI BEFORE start
  for stage in def.stages:                       // stages gate sequentially
    if stage.gate:                               // protected-env / manual approval
      d = ctx.wait_for_signal("approval:"+stage.id, timeout=stage.window)   // may wait DAYS (contract 9.4)
      if d.denied or d.timed_out: ctx.emit(ci.deployment.rejected); return
    results = parallel for job in stage.jobs (respecting `needs` DAG + concurrency group):
        // THE FROZEN OQ-F IDIOM — dispatch + park, woken hours later by job.done:
        j = ctx.activity(SCHEDULE_AND_RUN_JOB, JobSpec{ kind: Ci, .., idem_token })   // dispatches, returns; holds no runtime
        ctx.wait_for_signal("job.done", idem_key = j.idem_token)                       // parks (contract 9.4)
    if any(results.failed) and not stage.continue_on_error:
        emit_check(CheckStatus{ state: failure, run_attempt, trust_tier, details_ref: "#step-<n>" })  // X-1 (per context)
        ctx.emit(ci.run.failed, structured_failure(results))                          // the agent-native triage hook
        ctx.signal_merge_queue(ci.result{ commit_oid, overall: failure, contexts })   // the rollup signal (X-1)
        return
  emit_check(CheckStatus{ state: success, ... }) ; ctx.emit(ci.run.succeeded)
  ctx.signal_merge_queue(ci.result{ commit_oid, overall: success, contexts })          // wakes Git's merge queue (X-1)
  settle_budget()
```

### 3.2 Granularity: the activity boundary is the **JOB**, not the step

A *job* is the unit scheduled onto one runner in one sandbox; its steps run *inside* the sandbox and stream
to the firehose. Making the **job** the dispatch unit keeps the journal small (one dispatch + one signal per
job, not per step/log-line — critical at CI's firehose volume) while preserving DAG-level crash recovery.
Step-level progress is firehose/log state (the `log_anchor` index), recovered by **re-running the job on
retry**, not journaled. (Confirmed against durable-workflow §6: CI-pipeline stage/step granularity is the
`SCHEDULE_AND_RUN_JOB` idiom.)

### 3.3 The frozen `SCHEDULE_AND_RUN_JOB` handshake (OQ-F — long-park-completed-by-signal)

This is now a **first-class `myelin-flow` idiom** (contracts 9.2/9.4, frozen — not a CI-local invention).
The mechanism, per durable-workflow §4.9:

1. **Dispatch (an activity).** `SCHEDULE_AND_RUN_JOB` is an ordinary journaled activity. It **mints the
   `idem_token` at the workflow** (deterministic from `command_id`, so producer and consumer agree on the key
   with **no coordination round-trip**), **enqueues** the job into `job_queue` (idempotent on `idem_token` via
   `jq_idem`) with the lane/labels/trust-tier/concurrency-group/fair-key derived from the snapshot, **reserves
   at dispatch** (11.7), journals `activity_completed{ job_dispatched: true, idem_token }`, and **returns**.
   It does **not** block on completion; the activity worker is freed.
2. **Park (a durable signal wait).** The workflow immediately `wait_for_signal("job.done", idem_key =
   idem_token)`. It journals `signal_waited`, sets a timeout timer for the job's max duration, and **holds no
   runtime** while the job runs for minutes or hours (contract 9.4).
3. **Resume (woken hours later).** The runner leases the job, launches the sandbox, streams frames, and on
   terminal `signal(run, "job.done", { result }, idem_key = idem_token)`. The signal is **idempotent on
   `idem_token`** (the runner can deliver "done" twice under at-least-once; the workflow wakes once). The
   payload carries the terminal `result_summary` (pass/fail, artifact refs, the `ci.log.available` pointer);
   the DAG proceeds; the reserve **settles on the `job.done` signal**.
4. **Reaper retry.** A dead runner → the reaper re-queues → a fresh lease → a fresh `attempt`. The enqueue is
   idempotent on `idem_token`, and any downstream emit (e.g. a deploy effect) is dedup-keyed, so
   **at-least-once activity + idempotent job = effectively-once** (no double-deploy, no duplicate artifact
   publish). This is the D-1 drill (07).

**Determinism is preserved:** the scheduler's choice of runner is non-deterministic, but it happens *inside*
the dispatch / the external lease; only the journaled **result** (the `job.done` signal payload) feeds the
deterministic workflow body. The `flow-determinism` lint guards `ci_pipeline` (D-9).

The **merge-queue** (X-1) uses the same idiom one level up: Git's merge-queue workflow dispatches the required
CI via `SCHEDULE_AND_RUN_JOB` and waits on the **`ci.result`** rollup signal (per target ref), distinct from
the per-job `job.done`. CI emits `ci.result` once all required contexts for a commit reach terminal (§4).

### 3.4 Definition snapshot vs workflow versioning

`myelin-flow` versions the `ci_pipeline` *orchestration code* (a deploy of new orchestration drains old runs
against old code). CI separately content-addresses the *customer's pipeline definition* (the `.myelin/ci.*`
resolved at the commit, components digest-pinned). Orthogonal; both forward-only.

## 4. The Git↔CI check seam, produced (X-1 — CI's producer side)

CI is the **producer** of the frozen `CheckStatus` fact (contract 5.9); Git owns the projection table, the
supersession rule, the branch-protection `required`-set, the fork-endorsement, and the merge queue. CI's
algorithm:

1. **On dispatch / state change per context**, bump the `(commit_oid, context)` attempt counter
   (`check_attempt.next_attempt`, §3.2 of doc 01) and assemble the `CheckStatus` fact:
   `{ repo, commit_oid, context, state, run, run_attempt, trust_tier, details_ref: "#step-<n>",
   summary: (template_key, args), started_at, completed_at?, cost_settled }`. The **`trust_tier`** is the
   value stamped at trigger time (§1.3); the **`run_attempt`** is monotonic so Git's last-writer-wins is on
   the attempt, **not** wall-clock.
2. **Emit `ci.check.updated`** via the **outbox only** (contract 2.2), envelope `subject =
   repo#commit-<oid>/check-<context>` (an `ArtifactRef` sub-anchor, OQ-D), `aggregate = (repo, commit_oid)`
   so all checks for one commit are per-aggregate ordered. The event carries the small PII-free `CheckStatus`
   struct (references-not-payloads), never log bytes.
3. **`cost_settled`** is `false` until the run's reserve/settle bookend closes; a check is **not "final"
   until settled** (X-1). The terminal `ci.check.updated` carries `cost_settled: true`.
4. **Emit the `ci.result` rollup signal** once all required contexts for the commit reach terminal:
   `signal(merge_queue_run, "ci.result", { commit_oid, overall: success|failure, contexts: [CheckContext],
   idem_token })`, idempotent on `idem_token`. This wakes Git's merge-queue durable workflow (X-1).

**What CI does NOT do:** CI never owns the `check_status` projection table, never decides which contexts are
`required`, never endorses a fork run, never merges. Those are Git's (contract 5.9). A fork run's success is
recorded faithfully with `trust_tier = untrusted_fork`; **Git** treats it as neutral-for-gating until a
maintainer endorses (`approve_untrusted_ci`) or the context is re-run trusted. CI reports facts; Git gates.

## 5. The unified sandbox runner + the four uniform guarantees + the escape drill (X-6)

### 5.1 Backend decision: microVM (Firecracker) default; gVisor named-second; one to the drill first

**Decision (confirmed): Firecracker microVM is the default backend for untrusted code; gVisor is the named
second backend behind the same `SandboxBackend` trait; ship ONE backend through the escape drill first.** The
reasoning is that **the drill governs, not the benchmark**: for the platform's single hard gate (zero escapes
on a real kernel), **hardware virtualization** (KVM VT-x/AMD-V + a minimal VMM) is the more defensible "zero
escapes" claim to a DPO and the security gate than a userspace-kernel reimplementation. CI's real workloads
(Docker-in-CI, image builds, nested-virt, arbitrary syscalls) need a real guest kernel, which gVisor's
syscall gaps make a compat fight. Start latency is solved-enough by pre-warmed snapshot pools (§5.4). gVisor
remains valuable for very-high-density, low-risk, short agent `compute` calls — added later as a backend impl
+ its own drill, not a rewrite (the trait makes this cheap).

**Cited prior art:** Firecracker (Agache et al., *Firecracker: Lightweight Virtualization for Serverless
Applications*, NSDI 2020); gVisor (Young et al., *The True Cost of Containing*, HotCloud 2019, the `runsc`
runtime); Cloud Hypervisor (the alternative VMM behind the same trait).

### 5.2 The four uniform guarantees (X-6 — pinned by reconciliation, inherited by construction)

Every execution on this runner — **CI run or agent `ToolHands::exec`** — inherits, by construction (no
subsystem re-implements them; contract 8.4):

1. **Universal cost gate.** Reserve at dispatch, refuse-on-exhaustion, settle on completion, never interrupt
   in-flight; CI runs and agent runs meter into the **same wallet** (11.7). The reserve fronts each
   `SCHEDULE_AND_RUN_JOB` dispatch.
2. **Attribution.** The job runs under a **per-run attenuated token** (`mint_run_token`, contract 4.7), life
   == run life, auto-revoked on teardown, re-mintable mid-workflow on resume (S-11). Every effect is
   attributed with nested causality.
3. **HITL withhold (plan-then-apply).** Side-effecting mutation never goes through `ToolHands::exec`; it goes
   through `EffectApi::apply` (contract 8.2). A gated tool whose name is not in the approved set is
   **withheld** (returns `Denied`, does not mutate). `ToolHands::exec` carries only `compute`/`external`
   untrusted code, never privileged mutation — the routing split is the safety boundary.
4. **Isolation floor + drill.** gVisor-class userspace-kernel **or** microVM + the hardening profile (§5.3);
   the **real-kernel escape drill is the single hard go/no-go before any untrusted customer code runs (CI *or*
   agent)** (§5.5).

### 5.3 The hardening profile (backend-independent, mandatory on both — CI-1)

Applied identically to every sandbox regardless of backend or kind:

- **No host network — egress default-deny**, allowlist opt-in; the cloud-metadata endpoint
  (169.254.169.254), the control-plane/internal RPC, and any cross-tenant network are **always blocked**.
- **Read-only root + tmpfs scratch**; all Linux **caps dropped**; **no-new-privileges**; **seccomp**.
- **Images pinned by digest** — an un-digested tag is **rejected, fail-closed** (CI-1; 05 §HP-4).
- **`pids.max`** (fork-bomb ceiling) + **zero swap**; disk quota on scratch.
- **Whole-guest kill on teardown**; **one-job-per-sandbox, ephemeral, never reused across tenants/jobs**.
- **Secrets resolved by name *inside* the boundary** (§7), never baked into images, never handed to the agent
  runtime to forward.

### 5.4 Pre-warmed snapshot pools (the cold-start mitigation)

Firecracker resumes from a memory **snapshot** in tens of ms. The fleet autoscaler keeps a small warm buffer
of resumed-from-snapshot microVMs per (region, label-class), sized to the recent arrival rate, so "time to
first log line" is warm-pool-fast; the cold path is the boundary's cost, mitigated not eliminated. The
density tax (per-VM memory floor) is a cost-model input to the autoscaler (§5.5), not a safety compromise.
Rootless in-guest builders (Buildah/Kaniko/BuildKit-rootless) remain the *preferred* image-build path; the
microVM just removes the "can't even" floor.

### 5.5 The escape drill (D-4 / T-5) — CI's single hard go/no-go (PROVE-IT)

CI owns the drill; it is the gating milestone, not a late add — and per X-6 it **gates ALL agent execution**,
not only CI. The quantified gate: **zero escapes** under an adversarial corpus on a **real kernel**. A
`ci`/`compute` job attempts: kernel-exploit primitives; the cloud-metadata SSRF→cred-theft
(169.254.169.254); the control-plane / internal RPC; another tenant's network/storage; a fork bomb (assert
the `pids.max` ceiling holds); disk fill; and a secret exfiltration via egress (assert default-deny holds).
The drill emits a **green attestation artifact** or **CI is no-go for untrusted code** (CI *and* agent). It
runs against the *production* backend on a *real* host, re-run on **every backend/image/kernel change**. The
full adversarial corpus is enumerated by CI and executed in the build phase (07 §open; `[OPEN → P6]`). Cited:
EI-04 §5.1 ("a property not drilled on a real kernel is a claim, not a fact").

## 6. Reserve/settle (CI-2 / D8 / X-6.1) — the one metering path

CI builds **no second metering path.** `reserve_budget()` at workflow start checks the prepaid balance + any
per-capability add-on and **refuses to start** when exhausted; **each `SCHEDULE_AND_RUN_JOB` dispatch reserves
too**; `settle_budget()` on the `job.done` signal releases the unused reserve; a long/expensive run is
**never interrupted in flight**. The **meter** is resource-seconds (§8). The wallet/price is Commercial's
(C-1); CI owns only the metering **unit** + the `cost_event` rows. A runaway agent-triggered CI storm spends
down the wallet and **stops** — not a surprise infra bill (the D8 "runaway is self-limiting" property). This
is the **one metering path** for both CI and agent runs into the same wallet (X-6.1).

## 7. Logs, artifacts, caches, secrets

### 7.1 Logs ride the firehose + the resume-cursor protocol; CI owns the `ci.log.available` pointer

The hard rule (ADR-04.5; event-bus §4.3): **the durable bus must not carry one event per log line.** CI's
log pipeline coordinator:

```text
fn ship_line(run, job, step, line):
  redacted = secret_redact(line)                         // in-flight masking — defence-in-depth, NOT the boundary
  firehose::publish(stream_of(run,job,step), frame(redacted))   // live tail; subscribers use the resume-cursor protocol
  seal_and_flush_if_segment_full()                       // → T2 content-addressed blob + the (job,step,byte-range) index (11.8)
  emit_pointer_if_threshold(ci.log.available { run, job, step, range })   // COALESCED durable pointer (not per line)
```

- **`ci.log.appended` frames ride the firehose transport** (contract 3.5), keyed by `(run, job, step)` —
  never the durable bus. **CI is the heaviest firehose producer** (event-bus §4.3, confirmed).
- **The live-log view uses the frozen resume-cursor protocol** (OQ-J, contract 3.5): a viewer
  `subscribe(stream, scope = run:<id>/job:<id>, cursor?)`; on reconnect it `resume(stream, scope, last_seq)`
  and the transport backfills `(last_seq, now]` — **a reconnect loses zero log lines** (the T-5
  reconnect-loses-zero-ops drill applies to CI live-tail); if `last_seq` is past the retention window, a
  `resync_required` falls back to a range-read of the sealed segments. Scope is **bounded** (never `*`).
- **`ci.log.available` is the ONLY log-related *durable* event** — a **pointer** ("lines N..M of
  `run/job/step` are ready at `<ArtifactRef>`"). Search/Refs/agents consume the pointer and pull the range
  they need. **This pointer taxonomy is CI's to own** (03 §1).
- **Durable archive** — frames append to a current segment; sealed segments flush to T2 as content-addressed
  blobs with the frozen `(job, step, byte-range) → (segment-blob, offset)` index (`log_segment`/`log_anchor`,
  Storage 11.8). The standard "log = immutable segments + a small index" (Kreps, *The Log*, 2013).
- **Structured around the step graph** (the `log_anchor` index) so the live-log view is collapsible per step
  and a failed step deep-links to its byte range. The `CheckStatus.details_ref = …/ci/run/<id>#step-<n>`
  resolves through this index (the X-1 / OQ-D jump-to-failure path).

### 7.2 Artifacts & caches — content-addressed, poison-resistant (trust-scoped), residency-local

Both ride T2 `BlobStore` (BLAKE3, **per-tenant dedup** — cross-tenant dedup would be a residency leak).
**Artifacts** are retained outputs (correctness; ArtifactRef-addressable, TTL/GC). **Caches** are
reconstructible (perf only; key = `hash(lockfile + os + toolchain + ...)`; LRU eviction). **Poisoning
resistance (a known exploit class), now structural:** cache writes are namespaced by the
**trust-tier/branch-scoped cache namespace** (Storage C4 / 11.2) — an `UntrustedFork` run's writes land in a
fork-scoped namespace and **cannot reach the trusted (default-branch) cache scope**. A restored cache is a
build *input*, so a poisoned cache is a build compromise; Storage enforces the write-scope rule
**structurally** (not a check) — the storage half of the X-1 trust-tier defence (the fork-cannot-poison
drill, 07 D-6). Cache/artifact blobs live **near the runner region** (residency + download-cost); the within-
EU CDN clone/bundle class (Storage C3) accelerates hot-repo clones without an extra-EU edge.

### 7.3 Secrets resolved inside the boundary (CI-1 — non-negotiable)

The `JobSpec` carries secret **names** (`SecretRef`), not values. An **in-boundary broker** resolves them
*after* the sandbox is up, **scoped to exactly this job's references**, via the shared secret capability
(placed under Id/GDPR). CI mints **OIDC short-lived audience-scoped federated credentials** over static keys
(a strong EU-sovereign least-privilege fit) for talking to a registry/cloud target. **Untrusted/fork runs get
NO secrets by default** (the canonical "fork exfiltrates prod secrets" CVE class) — gated on the
`read & !is_untrusted_fork` ABAC edge (contract 4.9); protected environments require explicit
grants/approval. **Log masking is best-effort defence-in-depth, NOT the boundary** — egress default-deny is
the boundary (07 D-7).

### 7.4 Config grammar (config-as-code — 05 §HP-3)

Declarative JSON-schema'd core (authored as YAML/TOML, validated against a published JSON Schema);
expressions use the **same bounded `QueryAst`** as triggers (one expression language platform-wide, no second
mini-language); a **dynamic-generation step** (a job that *emits* a pipeline fragment) is the escape hatch for
genuinely programmatic fan-out — and that generation step **runs in the sandbox**, so "run code to compute the
pipeline" inherits the *same* isolation as any other untrusted code (no privileged config-eval path).
Deterministic resolution → the CAS snapshot (§1.4). `validate` (schema + lint) and `plan` (resolved DAG +
matrix + referenced secrets, **no runner spend**) are first-class (shift-left; the cost center is runner
compute).

## 8. Metering algorithm (resource-seconds)

The runner agent samples each sandbox's held resources and emits, per job, integer-quantized
**resource-seconds**: `cpu_seconds`, `mem_gb_seconds`, `gpu_seconds`, plus `storage_gb_hours`
(artifact/cache) and `egress_gb`. These are the **wholesale** meter (the honest cost basis; bin-packs well;
**directly comparable to an agent `compute` call**). Commercial maps resource-seconds → a credit/price at the
**markup** layer (kept in a separate column, immutable pricing). One `cost_event` row per metered unit;
`kind ∈ {ci, agent}` distinguishes for reporting, not for the mechanism (UNIFY / X-6). Users *see* credits;
the *meter* is resource-seconds (the usage view, 04). Wholesale ≠ markup is an invariant a pricing-change
replay must preserve (07 D-5).
