# CI/CD — 02 Internals & Algorithms

> Phase 4 — CI Stage-2. The subsystem-specific algorithms in depth: the trigger→dispatch path, the
> pipeline-as-durable-workflow mapping (the `SCHEDULE_AND_RUN_JOB` handshake), the distributed scheduler
> (DRR fair-share / lanes / concurrency / affinity / leasing / reaping), the fleet autoscaler, the
> sandbox runner + the escape drill, and the logs/artifacts/caches/secrets pipeline. Each hard problem is
> resolved here and cited; 05 consolidates the resolution table.

---

## 1. Trigger → dispatch (the front of every run)

A run begins when Trigger & Dispatch consumes a triggering event off the durable bus (`git.ref.updated`,
`git.pull_request.synchronized`, `issue.transitioned`, a manual API call, a schedule timer, or an agent
request). The path:

1. **Match** — evaluate the project's armed pipelines against the event via the shared **`EventMatcher`**
   (the query-AST predicate core, contract 3.4 — JSON, bounded, no UDFs/loops, statically cost-bounded,
   permission-aware; **not CEL**). CI does **not** invent a trigger language; a `on: pull_request: {...}`
   compiles to an `EventMatcher`. This runs close to the bus, cheaply.
2. **Dedup** — on the triggering `event_id` via the `consumer_dedup` ledger (contract 2.5). One push =
   one run (**exactly-once *effect*** even under at-least-once delivery; Helland 2012).
3. **Trust-tier evaluation** — classify the run `Trusted` (member push) / `UntrustedFork` (PR from a
   fork) / `SelfHosted` (targets a self-hosted pool). This gates secrets, cache-write, and egress for
   every job (05 §HP-1/HP-6).
4. **Definition resolution → content-addressed snapshot** — read `.myelin/ci.*` at the triggering commit,
   validate against the published JSON Schema, expand the matrix deterministically, resolve every
   component/image reference **to a digest** (fail-closed on a floating tag, 05 §HP-4), and write the
   resolved DAG as a **CAS blob** (T2). This snapshot is the run's reproducible, auditable definition; it
   is identical to the `myelin ci plan` output (shift-left, 04 §CLI).
5. **Reserve + start** — call `DurableExecutor::start(StartSpec{ input: snapshot_ref, .. })` for the
   `ci.pipeline` workflow; the workflow's first act is the reserve bookend (refuse-start-on-exhaustion;
   §6). The run row (`ci_run`) is written and `ci.run.started` emitted **via the outbox in the same tx**.

## 2. The distributed scheduler (the hard core — TE-29 / sketch 03)

### 2.1 Pull-leasing (the assignment model)

Runners **claim** work rather than the control plane pushing it (the Buildkite-agent / Nomad-pull model).
A runner long-polls the `job_queue`, claims the next eligible job for its labels via `FOR UPDATE SKIP
LOCKED`, takes a **lease** (`lease_owner` + `lease_expires`), and **heartbeats** to extend it. This reuses
the platform's existing lease primitive (the outbox relay, the timer wheel) — proven, not novel — and
scales horizontally (more runners = more pulls) with no central live-capacity tracking. A **dead-runner
reaper** sweeps expired leases and re-queues their jobs (which makes the run's `SCHEDULE_AND_RUN_JOB`
activity retry — §3).

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

The fairness intuition is **DRR** (Shreedhar & Varghese, *Deficit Round Robin*, 1996), applied at claim
time, with the fairness intuition of Linux CFS. Each `fair_key` (= tenant, or tenant:project) holds a
**deficit counter**; a runner claims the oldest job of the *least-recently-served* eligible tenant, not
the globally-oldest job. On claim, the served tenant's deficit is decremented (and periodically
replenished, weighted by plan tier). This prevents one tenant's 10k-job matrix from starving every other
tenant — **the canonical CI multi-tenant fairness failure**. *Floor (Stage-1):* DRR ships; a richer
hierarchical (per-tenant→per-project→per-pipeline) scheduler is **promotion-triggered by a measured
starvation signal** (the `causal-depth`/lag telemetry plus a per-`fair_key` wait-time histogram).

### 2.3 Priority lanes, concurrency groups, affinity

- **Lanes** (`interactive` > `batch` > `deploy`) are a strict order in the claim's `ORDER BY`. This is the
  **protected-human-lane analogue inside CI**: interactive PR-check feedback must never queue behind a
  nightly batch matrix. It composes with the platform shed order (speculative → batch/CI → agent →
  human-last, `00 §7`): under surge, CI sheds the batch lane first, holds interactive.
- **Concurrency groups** — `deploy:prod` is a **serialization key** (the partial unique index
  `jq_serialize`; one running at a time). `pr:web:42` is **cancel-superseded** (a new push to the PR
  cancels the in-flight run for that group, so only the latest PR head is tested). Both are claim-time
  predicates + a cancel hook on enqueue.
- **Affinity** — `labels <@ runner_labels` (job labels are a subset of the runner's). `gpu`, `arm64`,
  `large`, `linux`.

### 2.4 Backpressure & abuse

Per-tenant in-flight caps (a bounded queue, X-3), statement timeouts, and a per-tenant in-flight ceiling:
over-cap jobs **queue** gracefully, never collapse the scheduler. A 30× surge on one tenant sheds the
batch/CI lane (429 + `Retry-After`, honoured by the `myelin ci` client), holds the interactive lane, and
**leaves other tenants unaffected** (the SRCH-D6-analogue CI surge drill, 07 D-2). Crypto-mining abuse
(sustained high-CPU / no-IO) is flagged by a heuristic; the **economic** control is the wallet (reserve:
no balance → no start, §6), the **structural** control is the bounded queue + the sandbox `pids.max`/cpu
limits.

## 3. The pipeline IS a durable workflow (the `myelin-flow` mapping — sketch 02)

### 3.1 The hybrid boundary (the chosen model)

A run = a `ci.pipeline` **workflow definition** (a deterministic Rust function registered at `serve`,
guarded by the `flow-determinism` lint). The engine owns lifecycle/replay/timers/HITL-waits/reserve-settle
for free (its whole value); CI owns the scheduler/fleet, reached through a **durable activity**. The
boundary, explicitly:

| Concern | Owner | Mechanism |
|---|---|---|
| Run lifecycle, crash-recovery, deterministic replay | `myelin-flow` | the `ci.pipeline` workflow def + its journal |
| Deploy/manual gate (waits days) | `myelin-flow` | `ctx.wait_for_signal("approval:<stage>", window)` (contract 9.4) |
| Step/queue/deploy SLA timers | `myelin-flow` | `ctx.sleep_*` on the SC-11 timer wheel (contract 9.3) |
| Reserve/settle (no balance → no run) | `myelin-flow` bookends | reserve at start, settle on completion (contract 11.7) |
| **Which runner, when; fairness; lanes; affinity; leasing; reaping** | **CI scheduler** | inside `SCHEDULE_AND_RUN_JOB` |
| Sandbox execution of the job | **CI runner** | `SandboxBackend::launch` (§4); = `ToolHands::exec` for `kind=agent` |
| Definition resolution + CAS snapshot | **CI** | before `start`, pinned into `StartSpec.input` |

```text
workflow ci_pipeline(run_input):                 // deterministic; the flow-determinism lint guards it
  reserve_budget()                               // D8/CI-2 — refuse to START if wallet exhausted (never interrupt in flight)
  def = run_input.definition_snapshot            // content-addressed, resolved+pinned by CI BEFORE start
  for stage in def.stages:                       // stages gate sequentially
    if stage.gate:                               // protected-env / manual approval
      d = ctx.wait_for_signal("approval:"+stage.id, timeout=stage.window)   // may wait DAYS (contract 9.4)
      if d.denied or d.timed_out: ctx.emit(ci.deployment.rejected); return
    results = parallel for job in stage.jobs (respecting `needs` DAG + concurrency group):
        ctx.activity(SCHEDULE_AND_RUN_JOB, JobSpec{ kind: Ci, .. })          // ← CI scheduler + runner live HERE
    if any(results.failed) and not stage.continue_on_error:
        ctx.emit(ci.run.failed, structured_failure(results))                 // the agent-native triage hook
        return
  ctx.emit(ci.run.succeeded); settle_budget()
```

### 3.2 Granularity: the activity boundary is the **JOB**, not the step (Q17 answer)

A *job* is the unit scheduled onto one runner in one sandbox; its steps run *inside* the sandbox and
stream to the firehose. Making the **job** the activity keeps the journal small (one row per job, not per
step/log-line — critical at CI's firehose volume) while preserving DAG-level crash recovery. Step-level
progress is firehose/log state (the `log_anchor` index), recovered by **re-running the job on retry**, not
journaled.

### 3.3 The `SCHEDULE_AND_RUN_JOB` ↔ scheduler handshake (Stage-1 open Q2, resolved)

The activity is a `myelin-flow` activity (contract 9.2) whose body is **non-blocking with a durable wait**:

1. **Enqueue** — insert the job into `job_queue` (idempotent on the activity's `idem_token =
   (run_id, command_id, attempt)`; contract under §3.5 of the workflow doc). The enqueue carries the
   lane/labels/trust-tier/concurrency-group/fair-key derived from the snapshot.
2. **Park** — the activity returns a *pending* result and the workflow's command stays open; the runner
   leases the job, launches the sandbox, streams frames, and on terminal reports back by **`signal`ing the
   workflow** (`ci.result:<job_id>`, idempotent on `idem_key`). *(Decision: a **signal**, not
   activity-completion-blocking — the runner may run for hours; a long-held activity attempt is
   wasteful, and the signal path is exactly the multi-day HITL mechanism the engine already provides.
   The activity records "enqueued + lease pending"; the **terminal signal** is what advances the DAG.)*
3. **Resume** — the `ci.result` signal carries the terminal `result_summary` (pass/fail, artifact refs,
   the `ci.log.available` pointer); the workflow consumes it and the DAG proceeds.
4. **Reaper retry** — a dead runner → the reaper re-queues → a fresh lease → a fresh attempt. The job's
   `idem_token` and any downstream emit (e.g. a deploy effect) are dedup-keyed, so **at-least-once activity
   + idempotent job = effectively-once** (no double-deploy, no duplicate artifact publish). This is the
   D-1 drill (07).

**Determinism is preserved:** the scheduler's choice of runner is non-deterministic, but it happens
*inside* the activity / the external lease; only the journaled **result** (the terminal signal payload)
feeds the deterministic workflow body. The `flow-determinism` lint guards `ci_pipeline`.

### 3.4 Definition snapshot vs workflow versioning

`myelin-flow` versions the `ci_pipeline` *orchestration code* (a deploy of new orchestration drains old
runs against old code). CI separately content-addresses the *customer's pipeline definition* (the
`.myelin/ci.*` resolved at the commit, components digest-pinned). Orthogonal; both forward-only (STOR-2).

## 4. The unified sandbox runner + the escape drill (TE-28 / CI-1 / sketch 01)

### 4.1 Backend decision: microVM (Firecracker) default; gVisor named-second; one to the drill first

**Decision (Stage-1, confirmed): Firecracker microVM is the default backend for untrusted code; gVisor is
the named second backend behind the same `SandboxBackend` trait; ship ONE backend through the escape
drill first.** The reasoning is the **drill governs, not the benchmark**: for the platform's single hard
gate (zero escapes on a real kernel), **hardware virtualization** (KVM VT-x/AMD-V + a minimal VMM) is the
more defensible "zero escapes" claim to a DPO and the security gate than a userspace kernel
reimplementation. CI's real workloads (Docker-in-CI, image builds, nested-virt, arbitrary syscalls) need a
real guest kernel, which gVisor's syscall gaps make a compat fight. Start latency is solved-enough by
pre-warmed snapshot pools (§4.4). gVisor remains valuable for very-high-density, low-risk, short agent
`compute` calls — added later as a backend impl + its own drill, not a rewrite (the trait makes this cheap).

**Cited prior art:** Firecracker (Agache et al., *Firecracker: Lightweight Virtualization for Serverless*,
NSDI 2020); gVisor (Young et al., *The True Cost of Containing*, 2019, the `runsc` runtime); Cloud
Hypervisor (the alternative VMM behind the same trait).

### 4.2 The hardening profile (backend-independent, mandatory on both — CI-1)

Applied identically to every sandbox regardless of backend or kind:

- **No host network — egress default-deny**, allowlist opt-in; the cloud-metadata endpoint
  (169.254.169.254), the control-plane/internal RPC, and any cross-tenant network are **always blocked**.
- **Read-only root + tmpfs scratch**; all Linux **caps dropped**; **no-new-privileges**; **seccomp**.
- **Images pinned by digest** — an un-digested tag is **rejected, fail-closed** (CI-1; 05 §HP-4).
- **`pids.max`** (fork-bomb ceiling) + **zero swap**; disk quota on scratch.
- **Whole-guest kill on teardown**; **one-job-per-sandbox, ephemeral, never reused across tenants/jobs**.
- **Secrets resolved by name *inside* the boundary** (§7), never baked into images, never handed to the
  agent runtime to forward.

### 4.3 The escape drill (AG-D4) — CI's single hard go/no-go (PROVE-IT)

CI owns the drill; it is the gating milestone, not a late add. The quantified gate: **zero escapes** under
an adversarial corpus on a **real kernel**. A `ci`/`compute` job attempts: kernel-exploit primitives; the
cloud-metadata SSRF→cred-theft (169.254.169.254); the control-plane / internal RPC; another tenant's
network/storage; a fork bomb (assert the `pids.max` ceiling holds); disk fill; and a secret exfiltration
via egress (assert default-deny holds). The drill emits a **green attestation artifact** or **CI is no-go
for untrusted code** (CI *and* agent). It runs against the *production* backend on a *real* host, re-run on
**every backend/image/kernel change**. The full adversarial corpus is enumerated by CI and executed in
Phase 5 (07 §open; `[OPEN → P5]`).

### 4.4 Pre-warmed snapshot pools (the cold-start mitigation)

Firecracker resumes from a memory **snapshot** in tens of ms. The fleet autoscaler keeps a small warm
buffer of resumed-from-snapshot microVMs per (region, label-class), sized to the recent arrival rate, so
"time to first log line" is warm-pool-fast; the cold path is the boundary's cost, mitigated not eliminated.
The density tax (per-VM memory floor) is a cost-model input to the autoscaler (§5), not a safety
compromise. Rootless in-guest builders (Buildah/Kaniko/BuildKit-rootless) remain the *preferred* image-build
path; the microVM just removes the "can't even" floor.

## 5. Fleet elasticity on EU infra (TE-29 — the divergence-by-constraint)

ADR-11 forbids hyperscaler autoscaling, so CI **builds** the autoscaler. A **pool manager** watches
`job_queue` depth per (region, label-class) and provisions/deprovisions runner hosts via the EU provider's
own API behind a `FleetProvider` adapter (Hetzner / OVH / Scaleway / Exoscale / bare-metal-PXE; the menu
is a commercial pick). **Scale-to-zero** for idle tenants/regions (compute is the dominant cost);
**bin-packing** places jobs onto hosts to maximize density under the microVM memory floor. Cited prior art:
the AWS cell/bulkhead pattern (no global pool); DRR for the claim-side fairness (§2.2); Kubernetes is kept
as a `FleetProvider` *option* (for customers already running it), **never the default** — K8s-the-autoscaler
is the very primitive ADR-11 declines, and the security boundary is still ours.

**Residency by construction:** there is **no global runner pool** — pools are partitioned per residency
zone; the `region` predicate at claim time (§2.1) enforces it, and the `residency-pin` lint enforces it on
every store. **Self-hosted runners** register, attest (TPM / provisioning-signed token), receive a
**scoped short-TTL job token** (`mint_run_token`, contract 4.7) bound to one job/repo, and claim only their
tenant's `SelfHosted`-tier jobs; a compromised self-hosted runner is bounded by its scoped token (it cannot
read other tenants' jobs/secrets).

## 6. Reserve/settle (CI-2 / D8) — the workflow's bookends

CI builds **no second metering path.** `reserve_budget()` at workflow start checks the prepaid balance +
any per-capability add-on and **refuses to start** when exhausted; `settle_budget()` on completion releases
the unused reserve; a long/expensive run is **never interrupted in flight**. The **meter** is
resource-seconds (§8, 05 §HP-6/TE-32). The wallet/price is Commercial's (C-1); CI owns only the metering
**unit** + the `CostEvent` rows. A runaway agent-triggered CI storm spends down the wallet and **stops** —
not a surprise infra bill (the D8 "runaway is self-limiting" property).

## 7. Logs, artifacts, caches, secrets (sketch 04)

### 7.1 Logs ride the firehose; CI owns the `ci.log.available` pointer (the ownership line)

The hard rule (ADR-04.5; Bus §4.3): **the durable bus must not carry one event per log line.** CI's log
pipeline coordinator:

```text
fn ship_line(run, job, step, line):
  redacted = secret_redact(line)                         // in-flight masking — defence-in-depth, NOT the boundary
  firehose::publish(stream_of(run,job,step), frame(redacted))   // live tail (SSE/websocket fan-out to many viewers)
  seal_and_flush_if_segment_full()                       // → T2 content-addressed blob + OLTP range index (log_segment)
  emit_pointer_if_threshold(ci.log.available { run, job, step, range })   // COALESCED durable pointer (not per line)
```

- **`ci.log.appended` frames ride the firehose transport** (`firehose::publish/tail`, contract 3.5), keyed
  by `(run, job, step)` — never the durable bus.
- **`ci.log.available` is the ONLY log-related *durable* event** — a **pointer** ("lines N..M of
  `run/job/step` are ready at `<ArtifactRef>`"). Search/Refs/agents consume the pointer and pull the range
  they need. **This pointer taxonomy is CI's to own and complete** (03 §1).
- **Durable archive** — frames append to a current segment; sealed segments flush to T2 as
  content-addressed blobs with a `(job, step, byte-range) → (segment-blob, offset)` range index
  (`log_segment`/`log_anchor`). The standard "log = immutable segments + a small index" (Kreps, *The Log*).
- **Structured around the step graph** (the `log_anchor` index) so the live-log view is collapsible per
  step and a failed step deep-links to its byte range (`ArtifactRef#step-3#L42-L88`, the stable `#sub`
  scheme). This resolves the diff-anchored-log item: stable `(job, step, byte-range)` sub-anchors.

### 7.2 Artifacts & caches — content-addressed, poison-resistant, residency-local

Both ride T2 `BlobStore` (BLAKE3, **plaintext-hash-within-tenant-keyspace → per-tenant dedup**;
cross-tenant dedup would be a residency leak). **Artifacts** are retained outputs (correctness;
ArtifactRef-addressable, TTL/GC). **Caches** are reconstructible (perf only; key = `hash(lockfile + os +
toolchain + ...)`; LRU eviction). **Poisoning resistance (a known exploit class):** cache writes from an
`UntrustedFork` run get a **read-restricted/isolated scope** and **cannot write the trusted (default-branch)
cache** — a restored cache is a build *input*, so a poisoned cache is a build compromise; the scope
boundary is the defence (this is the fork-cannot-poison drill, 07). Cache/artifact blobs live **near the
runner region** (residency + download-cost) — no global blob pool.

### 7.3 Secrets resolved inside the boundary (CI-1 — non-negotiable)

The `JobSpec` carries secret **names** (`SecretRef`), not values. An **in-boundary broker** resolves them
*after* the sandbox is up, **scoped to exactly this job's references**, via the shared secret capability
(placed under Id/GDPR). CI mints **OIDC short-lived audience-scoped federated credentials** over static
keys (a strong EU-sovereign least-privilege fit) for talking to a registry/cloud target. **Untrusted/fork
runs get NO secrets by default** (the canonical "fork exfiltrates prod secrets" CVE class) — gated on
`trust_tier`; protected environments require explicit grants/approval. **Log masking is best-effort
defence-in-depth, NOT the boundary** — egress default-deny is the boundary.

### 7.4 Config grammar (config-as-code — sketch 05 / 05 §HP-3)

Declarative JSON-schema'd core (authored as YAML/TOML, validated against a published JSON Schema);
expressions use the **same bounded query-AST** as triggers (one expression language platform-wide, no
second mini-language); a **dynamic-generation step** (a job that *emits* a pipeline fragment) is the
escape hatch for genuinely programmatic fan-out — and that generation step **runs in the sandbox**, so
"run code to compute the pipeline" inherits the *same* isolation as any other untrusted code (no
privileged config-eval path). Deterministic resolution → the CAS snapshot (§1.4). `validate` (schema +
lint) and `plan` (resolved DAG + matrix + referenced secrets, **no runner spend**) are first-class
(shift-left; the cost center is runner compute).

## 8. Metering algorithm (TE-32 — resource-seconds)

The runner agent samples each sandbox's held resources and emits, per job, integer-quantized
**resource-seconds**: `cpu_seconds`, `mem_gb_seconds`, `gpu_seconds`, plus `storage_gb_hours`
(artifact/cache) and `egress_gb`. These are the **wholesale** meter (the honest cost basis; bin-packs well;
**directly comparable to an agent `compute` call**). Commercial maps resource-seconds → a credit/price at
the **markup** layer (kept in a separate column, immutable pricing). One `cost_event` row per metered unit;
`kind ∈ {ci, agent}` distinguishes for reporting, not for the mechanism (UNIFY). Users *see* credits; the
*meter* is resource-seconds (the usage view, 04). Wholesale ≠ markup is an invariant a pricing-change
replay must preserve (07 D-5).
