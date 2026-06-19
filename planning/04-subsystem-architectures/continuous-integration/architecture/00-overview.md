# CI/CD — 00 Overview (role, boundaries, component map)

> Phase 5-B — CI subsystem detailed architecture, **rewritten from scratch against the reconciled shared
> layer**. This document set is the **final build-to surface** for the Continuous-Integration / CD
> subsystem. It carries forward the sound Phase-4 design (`../sketches/`, `../design/`, and the preserved
> design record) and **conforms to every Phase-5 reconciliation decision** + the **frozen contract index**.
>
> **Canonical inputs (never contradicted):** [`VISION.md`](../../../../VISION.md); doctrine
> [`EI-03`](../../../../external-insights/03-agent-native-fabric.md) (agent-native fabric),
> [`EI-04`](../../../../external-insights/04-hard-problems.md) (hard problems),
> [`EI-05`](../../../../external-insights/05-ux-and-design.md) (UX). **The reconciled layer (binding):**
> [`05/00-reconciliation-decisions.md`](../../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> (read its §X-1, §X-6, §X-7, §OQ-E, §OQ-F, and the Part-4 CI punch list) +
> [`05/contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md) (the **FROZEN**
> surface) + the refined shared-system docs (`agent-fabric.md`, `durable-workflow.md`, `event-bus.md`,
> `storage.md`). **Tightest seam:** the Git↔CI `CheckStatus` contract (frozen as contract **5.9** / §X-1).
>
> Document split (unchanged): **00** role/boundaries/component map · **01** tech & data model · **02**
> internals & algorithms · **03** events, contracts & glue · **04** views, CLI & API · **05** hard problems ·
> **06** reconciliation compliance (how CI implements the frozen contracts) · **07** drills & open questions.

---

## 0. Changes vs the Phase-4 first pass (what changed and why)

The Phase-4 architecture was sound; reconciliation **froze** several seams that Phase-4 had left as
"named-but-open" shapes, and renamed a handful of CI events to the canonical taxonomy. The deltas absorbed,
each with its reconciliation anchor:

| # | Change | Why (reconciliation anchor) |
|---|---|---|
| **Δ1** | **The Git↔CI check event is now `ci.check.updated` carrying the frozen `CheckStatus` struct** — *not* the Phase-4 `ci.status.updated` + `ci.run.passed/failed` pair. CI stamps `trust_tier` + `run_attempt` from run provenance; Git owns the projection table, supersession rule, branch-protection `required`-set, and fork-endorsement. **Plus** the separate **`ci.result` rollup signal** that wakes the merge-queue workflow. | **X-1 / OQ-A**, contract **5.9** + **2.9**. The single most load-bearing seam, now jointly specified + frozen. Phase-4 had the right idea (`(commit_oid, context)` last-writer-wins) but Phase-5 freezes the **struct**, the **monotonic-`run_attempt` supersession** (clocks are not authority), and the **fork-trust-tier gating** (an `untrusted_fork` success is **neutral for gating** until endorsed). |
| **Δ2** | **`run_attempt` (u32) added to the run/check model** as the monotonic supersession key; re-run supersession is on `run_attempt`, never on `completed_at`. | **X-1.** A lower-attempt late arrival is dropped; this is mandatory under the bus's at-least-once delivery. |
| **Δ3** | **`trust_tier` is stamped by CI from run provenance** + the ReBAC ABAC edge `read & !is_untrusted_fork`; Git *reads* it off the fact, never recomputes. The poisoned-pipeline-execution defence is now explicit and contract-level. | **X-1**, contract **4.9** (CI fragment). |
| **Δ4** | **`SCHEDULE_AND_RUN_JOB` is now a frozen first-class `myelin-flow` idiom** with the `job.done` durable signal keyed by a workflow-minted `idem_token`. Phase-4 designed this as a CI-local pattern; Phase-5 elevated it to a shared engine idiom (the merge queue, any long CI stage, agent runs all use it). | **OQ-F**, contracts **9.2 / 9.4**. The signal name is **`job.done`** (per-job) and **`ci.result`** (the merge-queue rollup) — two registered names on one engine. |
| **Δ5** | **`details_ref` jump-to-failure sub-anchor is `#step-<n>`**, resolved through the frozen T3 `(job, step, byte-range)` index. The Phase-4 `#step-<step_id>#L42-L88` form is retained as the deeper anchor; the `CheckStatus.details_ref` carries the `#step-<n>` form. | **OQ-D** (`#sub` grammar) + **Storage 11.8**. |
| **Δ6** | **`list_objects` over `run_id` is now the frozen `SetExpr` push-down** — CI lowers `Filter{set_expr, zookie}` to a JOIN against the per-tenant authz reverse index on the `ci_run.run_id` column. No N+1, no post-filter. | **OQ-E**, contract **4.3**. Phase-4 named this as a CONFIRM; Phase-5 froze the `SetExpr`/`via_column` mechanism. |
| **Δ7** | **Per-subject DEK for inline log PII is now frozen (not just a named floor)** — the T3 log tier keys a subject's isolable inline PII to a per-subject DEK, alongside the per-tenant default. The Phase-4 `[OPEN → LEGAL]` GD-6 floor is now the **built** structural mechanism; only the *residual* third-party free-text basis stays `[OPEN — LEGAL]`. | **Storage C1 / 11.4** + **X-7** (the one erasure posture). |
| **Δ8** | **The erasure residual is instantiated by reference, not restated.** CI no longer authors its own free-text-PII residual statement; it says "the residual is handled per the platform posture in `05 §X-7`." | **X-7 / OQ-G**, contract **10.9**. |
| **Δ9** | **The four uniform sandbox guarantees are pinned as contract**, inherited by `ToolHands::exec` = the CI runner's `kind=agent` job. Phase-4 stated UNIFY; Phase-5 froze the four guarantees (universal cost gate, per-run-token attribution, HITL withhold, isolation floor + drill). The `requires_approval` defaults table is frozen. | **X-6**, contracts **8.1 / 8.4**. |
| **Δ10** | **Trust-tier/branch-scoped cache namespaces + the within-EU CDN clone/bundle class are now NEW frozen Storage contracts** (Phase-4 had them as CRs). | **Storage C3 / C4 / 11.2**. |
| **Δ11** | **The firehose now has the frozen `subscribe/resume/scope` resume-cursor protocol**; CI's live-log view subscribes with `scope = run:<id>/job:<id>` and resumes losslessly on reconnect. Phase-4 used `publish/tail`; that still holds, but the live-tail UI now rides the resume-cursor protocol. | **OQ-J**, contract **3.5**. |
| **Δ12** | **`initiative` and the two `ci.*` tokens (`ci.check.updated`, `ci.result`) are registered** under the Bus §6 grammar; CI completes its dotted-name list against the frozen taxonomy. | **contract 2.9**. |

Everything else in the Phase-4 design — the pull-leasing DRR scheduler, the EU `FleetProvider` autoscaler,
the Firecracker-default sandbox + the escape drill as the single hard gate, resource-seconds metering,
digest-pin supply chain, the config grammar, the five-service component map — **stands unchanged** and is
re-confirmed against the frozen contracts below.

---

## 1. What CI is, in one paragraph

CI is **the execution arm of the event fabric and the platform's single hardest security surface** — the
one place untrusted customer code runs. It turns a triggering event (a push, an issue transition, a manual
or agent request) into a **content-addressed, reproducible run** of declared work on an **elastic,
EU-resident, hardware-isolated runner fleet**; it streams structured logs to the firehose; it gates deploys
behind durable human approvals; it meters real resource consumption against the wallet; and it **reports
facts** back through the shared fabric — Git checks, the inbox, chat, search, refs — **without gating
anything itself** (per X-1: CI emits the `CheckStatus` fact, Git owns the merge gate). Critically, CI also
**owns the one unified sandbox runner** on which the Agent Fabric's `ToolHands::exec` runs (ADR-20 /
TE-31=UNIFY): the catastrophic surface — untrusted execution — is **built and drilled once**, here, behind
the **one real-kernel escape drill that gates ALL agent execution** (X-6).

The platform did most of the heavy lifting *for* CI in Phase 3–5 (the durable-workflow engine + the
`SCHEDULE_AND_RUN_JOB` idiom, the firehose + resume-cursor protocol, the blob/log storage tiers + the frozen
`(job,step,byte-range)` index, the reserve/settle gate, the per-run token mint, the `list_objects` push-down).
**CI's genuine green-field work is the scheduler + the fleet autoscaler + the sandbox backend** — the rest
is disciplined composition of the frozen contracts.

## 2. Role & responsibilities

| Responsibility | What CI does |
|---|---|
| **Trigger & dispatch** | Match triggering events to a project's pipelines via the shared `EventMatcher` (= the frozen `QueryAst`, contract 3.4 — no CEL); dedup on `event_id` (exactly-once *effect*); resolve + content-address the definition snapshot; start the `ci.pipeline` durable workflow. |
| **Pipeline orchestration** | A run **is a `myelin-flow` durable workflow** (stages → DAG of jobs); CI owns the workflow *definition*, the engine owns lifecycle/replay/timers/HITL waits/reserve-settle. Jobs dispatch via the frozen **`SCHEDULE_AND_RUN_JOB`** idiom (OQ-F). |
| **Distributed scheduling** | The hard core: fair-share across tenants (DRR), priority lanes, concurrency groups, affinity, pull-leasing onto an elastic fleet, dead-runner reaping — CI's core competency (the platform's heaviest scheduling problem). |
| **Runner fleet & elasticity** | Autoscale-on-queue-depth over **EU-controlled infra** (no hyperscaler autoscaling, ADR-11), per residency zone; pre-warmed microVM snapshot pools; scale-to-zero; self-hosted runner attestation. |
| **The unified sandbox runner (ADR-20 / X-6)** | The one hardened runner for `kind ∈ {ci, agent}`; **owns the real-kernel escape drill** — the single hard go/no-go gate before any untrusted customer code runs (CI *or* agent); enforces the **four uniform guarantees** (cost gate, attribution, HITL withhold, isolation floor). |
| **Logs / artifacts / caches** | Logs ride the **firehose** (`ci.log.appended` frames + the resume-cursor protocol); `ci.log.available` *pointer* events are CI's only log-related durable bus event; artifacts/caches are content-addressed blobs on Storage T2 with **trust-tier/branch-scoped namespaces**; all residency-pinned + crypto-shred-capable (**per-subject DEK for inline log PII**). |
| **Config-as-code** | A declarative JSON-schema'd grammar + the shared bounded `QueryAst` + a sandboxed dynamic-generation escape hatch; shift-left `validate`/`plan` (no runner spend). |
| **Supply-chain trust** | Digest-pin-or-fail-closed (images + components); sign + verify-before-use (sigstore); SLSA provenance + SBOM for produced artifacts. |
| **Deployments & HITL** | Protected-environment gates as durable signals; approvals queue + chat approval card (per-effect `idem_key`, OQ-F); rollback as a first-class action. |
| **Metering** | Meter **resource-seconds** (the wholesale unit); one `cost_event` schema fronts CI + agent; **reserve/settle is the one metering path** (X-6.1) — reserve at dispatch (incl. each `SCHEDULE_AND_RUN_JOB`), settle on `job.done`. |
| **Cross-fabric surfacing (facts only)** | Emit `ci.check.updated` (CheckStatus) for Git's gate; feed the one inbox, chat unfurls/cards, knowledge embeds, search — via events + `project` + refs. **CI reports; Git gates.** |

## 3. What CI **owns** vs **delegates**

| CI **owns** (authoritative) | CI **delegates** (consumes a frozen contract) |
|---|---|
| The `ci.*` event taxonomy + `ci.check.updated`/`ci.result` (the check seam producer side) + `ci.log.available` pointers | The durable event bus / outbox / envelope (`myelin-events`, contracts 2.1–2.9) + the firehose transport + resume-cursor protocol (3.5) |
| The distributed **scheduler** (DRR fair-share, lanes, concurrency, affinity, leases, reaper) | The **durable-workflow engine** (`myelin-flow`) — lifecycle, replay, timers, HITL signals, the `SCHEDULE_AND_RUN_JOB` idiom (9.1–9.4) |
| The **runner fleet** + the EU **fleet autoscaler** + self-hosted attestation | **Identity** — `authenticate`/`check`/`list_objects` (the `SetExpr` push-down) / `list_subjects`, the ReBAC engine, `mint_run_token` (4.1–4.10) |
| The **unified sandbox runner** + the hardening profile + the **escape drill** (the four uniform guarantees enforced here) | **Storage** — OLTP, `BlobStore` T2 (+ trust-scoped cache namespaces, CDN clone class), the T3 log tier (`(job,step,byte-range)` index), KMS/crypto-shred (per-subject log DEK), OLAP (11.1–11.8) |
| The **trust-tier evaluator** (trusted / untrusted_fork / self_hosted) — **stamps `trust_tier` onto every CheckStatus** | **Refs** — `ArtifactRef` resolution, the backlink graph, the `#sub` grammar + tombstone ladder (5.1–5.8) |
| The pipeline **definition grammar** + resolver + content-addressed snapshot | **Search** — indexing (CI declares its `IndexSpec`, projects text off the bus) (6.3) |
| The **metering unit** (resource-seconds) + the `cost_event` shape | **Notifications** — the one inbox + `humanise` templating (CI registers notif rules + summaries) (7.x) |
| **Caches & artifacts** semantics (poisoning resistance via trust-scoped namespaces, retention/GC) | **Reserve/settle gate + the wallet** (Agent gate; Commercial owns wallet/price) (11.7) |
| The **deployment / environment** model + protected-env gates + rollback | **GDPR/Audit** — the DSR orchestrator, the tamper-evident log, the classification derive, the **one erasure posture** (10.9) |
| **Supply-chain trust model** (digest-pin, sign-verify, SLSA, SBOM) | **The Agent Fabric** — the brain (`AgentRuntime::step`), `EffectApi` governed mutation, the `ToolDef` registry (8.1–8.8) |
| The secret broker's CI usage (resolve-by-name inside the boundary, scoped per job) | **Tenancy / control plane** — placement, `discover`, `residency_verify`, the partition key (12.x) |

**The one-line boundary that resolves the two biggest "who owns this" risks:**

- **CI vs Workflow:** `myelin-flow` decides *what runs next in this run* (the DAG walk, gates, timers,
  crash-recovery, the reserve/settle bookends). **CI's scheduler decides *which runner runs it, when, fairly,
  across all tenants*.** The seam is the frozen **`SCHEDULE_AND_RUN_JOB`** activity that dispatches and parks,
  completion arriving as a `job.done` durable signal hours later (02 §3, OQ-F).
- **CI vs Agent Fabric:** they **share the hands and the hardening** (the sandbox, the job spec, the escape
  drill, the cost gate, the secret broker); they **differ in the head and the governance** (the orchestration
  workflow, the brain, the `EffectApi` mutation path). `ToolHands::exec` *is*
  `SandboxBackend::launch(JobSpec{ kind: Agent })` on CI's runner, inheriting the four uniform guarantees
  (02 §5, 05 §HP-5, X-6).
- **CI vs Git (X-1):** CI **reports** the `CheckStatus` fact (state + `trust_tier` + `run_attempt`); **Git
  decides** which contexts gate and whether a fork run may turn the gate green. The dependency is acyclic:
  CI emits, Git reads its own projection. CI **never merges**.

## 4. Internal component map

CI is a small number of cooperating services inside the cell, each a thin `serve(AppSpec)` shell over the
shared plumbing (contract 1.1). The **trust boundary** is the public/internal split (contract 1.2): the
control plane is internal-RPC; the sandbox is the hard isolation boundary.

```
                          ┌──────────────── one Myelin cell (region-pinned) ────────────────┐
 git.* / issue.* /        │                                                                 │
 manual / agent  ───────► │  ┌───────────────────────┐   reserve/settle    ┌─────────────┐  │
 (durable bus)            │  │  Trigger & Dispatch    │◄──── (bookends) ───►│ myelin-flow │  │
                          │  │  • EventMatcher        │                     │  (durable   │  │
                          │  │    (= frozen QueryAst) │   start(ci.pipeline)│   workflow  │  │
                          │  │  • dedup(event_id)     ├────────────────────►│   engine)   │  │
                          │  │  • trust-tier eval     │                     │             │  │
                          │  │    → stamps CheckStatus│ SCHEDULE_AND_RUN_JOB └──────┬──────┘  │
                          │  │  • definition resolver │  (dispatch + park,         │         │
                          │  │    → CAS snapshot      │   woken by job.done signal) │         │
                          │  └───────────────────────┘                              ▼         │
                          │            ┌──────────────── CI Control Plane (Rust) ─────────┐   │
                          │            │  Scheduler (pull-lease, DRR fair-share, lanes,   │   │
                          │            │   concurrency groups, affinity)  ── job_queue ── │   │
                          │            │  Reaper (dead-lease re-queue → activity retry)   │   │
                          │            │  Fleet autoscaler (FleetProvider trait, EU IaaS) │   │
                          │            │  Log-pipeline coordinator (firehose + resume-cur)│   │
                          │            │  Secret broker (resolve-in-boundary, scoped)     │   │
                          │            │  Supply-chain verifier (digest-pin, sigstore)    │   │
                          │            │  Check emitter (CheckStatus + run_attempt        │   │
                          │            │   + trust_tier → ci.check.updated / ci.result)   │   │
                          │            └───────┬──────────────────────────────┬───────────┘   │
                          │       lease + token│ (mint_run_token via Id)      │ provision     │
                          │                    ▼                              ▼               │
                          │   ┌──────────── Runner host (KVM bare-metal / EU IaaS) ────────┐  │
                          │   │  Runner agent (small attested Rust binary)                 │  │
                          │   │   • claims jobs for its labels  • heartbeats the lease     │  │
                          │   │   • SandboxBackend::launch(JobSpec)  (= ToolHands::exec    │  │
                          │   │     for kind=agent; four uniform guarantees inherited)     │  │
                          │   │   ┌──────── microVM (Firecracker, default) ───────────┐    │  │
                          │   │   │  guest kernel · read-only root + tmpfs · egress    │    │  │
                          │   │   │  default-deny · caps dropped · seccomp · pids.max  │    │  │
                          │   │   │  one-job-per-sandbox, ephemeral, killed on teardown│    │  │
                          │   │   └────────────────────────────────────────────────────┘    │  │
                          │   └──────────────┬──────────────────────────────┬──────────────┘  │
                          │   firehose frames │ (logs, resume-cursor)        artifacts/caches│  │
                          │                   ▼          (trust-scoped)      ▼ (BlobStore T2) │
                          │     Storage: T3 log tier ((job,step,byte-range) index, per-subj   │
                          │              DEK) · T2 blobs (trust-scoped cache ns) · OLAP        │
                          └─────────────────────────────────────────────────────────────────┘
              outputs: ci.run.* / ci.check.updated (CheckStatus) / ci.result (rollup signal) /
                       ci.log.available / ci.deployment.* (outbox→bus)
                       → Git CHECK_STATUS projection + merge gate · Notif inbox · Chat cards · Search · Refs · Agents
```

**The five logical services** (each its own `serve` shell, its own Postgres, no cross-DB):

1. **Trigger & Dispatch** — close to the bus; matches `EventMatcher` (= `QueryAst`), dedups, evaluates +
   **stamps the trust tier**, resolves + content-addresses the definition, starts the workflow. Stateless
   except the dedup ledger.
2. **CI Control Plane** — the scheduler + reaper + fleet autoscaler + log-pipeline coordinator + secret
   broker + supply-chain verifier + the **check emitter** (the `CheckStatus`/`run_attempt`/`ci.result`
   producer). The latency-/correctness-critical Rust core. Owns `job_queue`, `runner`, `lease`,
   deployment/environment state, the log range index, artifact/cache indices, the per-`(commit_oid,context)`
   attempt counter.
3. **The runner agent** — a small attested Rust binary on each runner host; pulls leases, launches the
   sandbox via `SandboxBackend`, streams frames, reports terminal via the `job.done` signal. Same binary
   hosted + self-hosted.
4. **The sandbox backend** — Firecracker (default) behind the `SandboxBackend` trait; gVisor is the named
   second backend; self-hosted is a delegated backend. **The trust boundary; the four uniform guarantees
   live here.**
5. **The `ci.pipeline` / `ci.deploy` workflow definitions** — deterministic Rust functions registered on
   `myelin-flow` at `serve`; the DAG walk + gates + reserve/settle bookends + the `SCHEDULE_AND_RUN_JOB`
   dispatches live here.

These are *logical* services; small self-host cells may co-locate them in one process (the harness permits
binary co-location; the DB-per-service + contract boundaries hold regardless).

## 5. Cell topology & where CI sits

CI lives **inside each cell** (a cell = a complete region-pinned stack; Tenancy §7). There is **no global
runner pool** — pools are partitioned per residency zone; an EU-resident tenant's job is claimed only by a
runner in its region, and its logs/artifacts/caches/state stay in-region (the `residency-pin` lint on every
store CI writes; `residency_verify` attests the runner pool + log/artifact/cache region, contract 12.4). The
PII-free global control plane (`discover`/`placement_of`) is consulted only for routing (the CLI/wire
reaching the right cell); the per-request hot path is in-cell. Scaling is **add-a-cell** (bulkhead); within a
cell, CI scales by adding runner hosts (the pull model makes this trivial) and by partitioning the
scheduler's claim load (01 §6, 02 §2). Multi-cell-spanning runs are a named floor (designed-not-built;
inherits the cross-cell PII-free pointer bridge, contract 12.6 / OQ-I).

## 6. Floors named up front (VISION §3)

| Floor (ships v1) | Named follow-on |
|---|---|
| **One sandbox backend (Firecracker) through the escape drill** | gVisor second backend behind the same trait + its own drill (density/latency-triggered) |
| **Single-cell pipelines** | Cross-cell-spanning runs (inherits the Workflow / OQ-I multi-cell floor) |
| **One/two `FleetProvider` adapters + self-hosted** | More EU-provider adapters (adapters, not redesigns) |
| **DRR fair-share at claim time** | A richer hierarchical scheduler (measured-starvation-triggered) |
| **Object-segment T3 log tier + OLTP `(job,step,byte-range)` index** | A dedicated time-series/wide-column log tier (measured-volume) |
| **Per-subject DEK crypto-shred for isolable inline log PII (now built)** | The residual third-party free-text PII basis (handled per the platform posture, X-7, `[OPEN — LEGAL]`) |
| **SLSA L1–L2 provenance + SBOM** | Hermetic / two-party (L3+) provenance (demand-triggered) |
| **Component trust model (digest-pin + sign-verify + SLSA)** | The registry *product* (hosting/discovery) — commercial-flagged |
| **`myelin ci local` not built** | Laptop execution (UX-vs-fidelity; deferred) |

See 05 §floors for the full table and 07 for the drills + open questions.
