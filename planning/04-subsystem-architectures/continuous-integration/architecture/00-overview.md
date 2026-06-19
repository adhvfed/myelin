# CI/CD — 00 Overview (role, boundaries, component map)

> Phase 4 — CI subsystem, Stage-2 (Detailed Architecture). This document set is the **final detailed
> architecture** for the Continuous-Integration / CD subsystem. It builds directly on the Stage-1
> commitments (`../sketches/00-findings.md` + the six exploration sketches) and the design surface
> (`../design/`), and it is the **build-to surface for Phase 5**.
>
> **Canonical inputs (never contradicted):** VISION; doctrine EI-03 (agent-native fabric), EI-04 (hard
> problems), EI-05 (UX); directives **CI-1 / CI-2**, **ADR-20 / D5** (one sandbox, TE-31=UNIFY), **D8**
> (reserve/settle); the Phase-3 contract-index (the 12 contract groups) + the 11 system docs; the
> Phase-3 handoff (README §5, CI bullet). **Tightest seam:** the Git↔CI checks/merge-gate contract
> (git-hosting `03 §1.1`, `02 §6`).
>
> Document split:
> - **00 — this doc:** role/responsibilities, owns-vs-delegates, the component map, the cell topology.
> - **01 — tech & data model:** language/DB justification, the schema, the job spec, residency.
> - **02 — internals & algorithms:** the scheduler, the pipeline-as-workflow mapping, the sandbox runner,
>   logs/artifacts/caches, each hard problem resolved with cited prior art.
> - **03 — events, contracts & glue:** the complete `ci.*` taxonomy; every glue contract implemented.
> - **04 — views, CLI & API:** the view inventory, the CLI, the agent-tool surface (ToolDefs).
> - **05 — hard problems:** the consolidated resolution table + cited prior art + named floors.
> - **06 — shared-system change requests:** the itemized Phase-5 reconciliation list.
> - **07 — drills & open questions.**

---

## 1. What CI is, in one paragraph

CI is **the execution arm of the event fabric and the platform's single hardest security surface** — the
one place untrusted customer code runs. It turns a triggering event (a push, an issue transition, a
manual or agent request) into a **content-addressed, reproducible run** of declared work on an **elastic,
EU-resident, hardware-isolated runner fleet**, streams structured logs to the firehose, gates deploys
behind durable human approvals, meters real resource consumption against the wallet, and surfaces every
result back through the shared fabric (Git checks, the inbox, chat, search, refs). Critically, CI also
**owns the one unified sandbox runner** on which the Agent Fabric's `ToolHands::exec` runs (ADR-20 /
TE-31=UNIFY): the catastrophic surface — untrusted execution — is **built and drilled once**, here.

The platform did most of the heavy lifting *for* CI in Phase 3 (the durable-workflow engine, the
firehose, the blob/log storage tiers, the reserve/settle gate, the per-run token mint). **CI's genuine
green-field work is the scheduler + the fleet autoscaler + the sandbox backend** — the rest is disciplined
composition of the Phase-3 contracts.

## 2. Role & responsibilities

| Responsibility | What CI does |
|---|---|
| **Trigger & dispatch** | Match triggering events to a project's pipelines via the shared `EventMatcher` (one matcher engine, no CEL); dedup on `event_id` (exactly-once *effect*); resolve + content-address the definition snapshot; start the `ci.pipeline` durable workflow. |
| **Pipeline orchestration** | A run **is a `myelin-flow` durable workflow** (stages → DAG of jobs); CI owns the workflow *definition*, the engine owns lifecycle/replay/timers/HITL waits/reserve-settle. |
| **Distributed scheduling** | The hard core: fair-share across tenants, priority lanes, concurrency groups, affinity, pull-leasing onto an elastic fleet, dead-runner reaping — CI's core competency (the platform's heaviest scheduling problem). |
| **Runner fleet & elasticity** | Autoscale-on-queue-depth over **EU-controlled infra** (no hyperscaler autoscaling, ADR-11), per residency zone; pre-warmed microVM snapshot pools; scale-to-zero; self-hosted runner attestation. |
| **The unified sandbox runner (ADR-20)** | The one hardened runner for `kind ∈ {ci, agent}`; **owns the real-kernel escape drill (AG-D4)** — the single hard go/no-go gate. |
| **Logs / artifacts / caches** | Logs ride the **firehose** (`ci.log.available` *pointer* events are CI's taxonomy on the durable bus); artifacts/caches are content-addressed blobs on Storage T2; all residency-pinned + crypto-shred-capable. |
| **Config-as-code** | A declarative JSON-schema'd grammar + the shared bounded expression AST + a sandboxed dynamic-generation escape hatch; shift-left `validate`/`plan` (no runner spend). |
| **Supply-chain trust** | Digest-pin-or-fail-closed (images + components); sign + verify-before-use (sigstore); SLSA provenance + SBOM for produced artifacts. |
| **Deployments & HITL** | Protected-environment gates as durable signals; approvals queue + chat approval card; rollback as a first-class action. |
| **Metering** | Meter **resource-seconds** (the wholesale unit); one `CostEvent` schema fronts CI + agent; reserve/settle is the workflow's bookends. |
| **Cross-fabric surfacing** | Feed Git checks (`ci.run.*` → merge gate), the one inbox, chat unfurls/cards, knowledge embeds, search — via events + `project` + refs. |

## 3. What CI **owns** vs **delegates**

| CI **owns** (authoritative) | CI **delegates** (consumes a Phase-3 contract) |
|---|---|
| The `ci.*` event taxonomy + the `ci.log.available` pointer events | The durable event bus / outbox / envelope (`myelin-events`) |
| The distributed **scheduler** (fair-share, lanes, concurrency, affinity, leases) | The **durable-workflow engine** (`myelin-flow`) — run lifecycle, replay, timers, HITL signals |
| The **runner fleet** + the EU **fleet autoscaler** + self-hosted attestation | **Identity** — `authenticate`/`check`/`list_objects`, the ReBAC engine, `mint_run_token` |
| The **unified sandbox runner** + the hardening profile + the **escape drill** | **Storage** — OLTP, `BlobStore` (T2), the log/firehose tier (T3), KMS/crypto-shred, OLAP read store |
| The pipeline **definition grammar** + resolver + content-addressed snapshot | **Refs** — URN resolution, the backlink graph (CI emits `ref.created`, implements `project`) |
| The **trust-tier evaluator** (trusted / untrusted-fork / self-hosted) | **Search** — indexing (CI declares its `IndexSpec`, projects text off the bus) |
| The **secret broker's CI usage** (resolve-by-name inside the boundary, scoped per job) | **Notifications** — the one inbox + humanisation (CI emits Signals, defines notif rules) |
| The **metering unit** (resource-seconds) + the `CostEvent` shape | **The reserve/settle gate + the wallet** (Agent Fabric gate; Commercial owns the wallet/price) |
| **Caches & artifacts** semantics (poisoning resistance, retention/GC) | **GDPR/Audit** — the DSR orchestrator, the tamper-evident log, the classification derive |
| The **deployment / environment** model + protected-env gates | **Tenancy / control plane** — placement, `discover`, `residency_verify`, the partition key |
| **Supply-chain trust model** (digest-pin, sign-verify, SLSA, SBOM) | **The Agent Fabric** — the brain (`AgentRuntime::step`), `EffectApi` governed mutation, `ToolDef` registry |

**The one-line boundary that resolves the two biggest "who owns this" risks:**

- **CI vs Workflow:** `myelin-flow` decides *what runs next in this run* (the DAG walk, gates, timers,
  crash-recovery, the reserve/settle bookends). **CI's scheduler decides *which runner runs it and when,
  fairly, across all tenants*.** The seam is the `SCHEDULE_AND_RUN_JOB` durable activity (02 §3).
- **CI vs Agent Fabric:** they **share the hands and the hardening** (the sandbox, the job spec, the
  escape drill, the cost gate, the secret broker); they **differ in the head and the governance** (the
  orchestration workflow, the brain, and the `EffectApi` mutation path). `ToolHands::exec` *is*
  `SandboxBackend::launch(JobSpec{ kind: Agent })` on CI's runner (02 §5, 05 §HP-5).

## 4. Internal component map

CI is a small number of cooperating services inside the cell, each a thin `serve(AppSpec)` shell over the
shared plumbing (`00 §3.1`). The **trust boundary** is the public/internal split (`00 §4`): the
control plane is internal-RPC; the sandbox is the hard isolation boundary.

```
                          ┌──────────────── one Myelin cell (region-pinned) ────────────────┐
 git.* / issue.* /        │                                                                 │
 manual / agent  ───────► │  ┌───────────────────────┐   reserve/settle    ┌─────────────┐  │
 (durable bus)            │  │  Trigger & Dispatch    │◄──── (bookends) ───►│ myelin-flow │  │
                          │  │  • EventMatcher        │                     │  (durable   │  │
                          │  │  • dedup(event_id)     │   start(ci.pipeline)│   workflow  │  │
                          │  │  • trust-tier eval     ├────────────────────►│   engine)   │  │
                          │  │  • definition resolver │                     │             │  │
                          │  │    → CAS snapshot      │  SCHEDULE_AND_RUN_JOB└──────┬──────┘  │
                          │  └───────────────────────┘     (durable activity)      │         │
                          │                                                         ▼         │
                          │            ┌──────────────── CI Control Plane (Rust) ─────────┐   │
                          │            │  Scheduler (pull-lease, DRR fair-share, lanes,   │   │
                          │            │   concurrency groups, affinity)  ── job_queue ── │   │
                          │            │  Reaper (dead-lease re-queue)                    │   │
                          │            │  Fleet autoscaler (FleetProvider trait, EU IaaS) │   │
                          │            │  Log-pipeline coordinator (firehose seam)        │   │
                          │            │  Secret broker (resolve-in-boundary, scoped)     │   │
                          │            │  Supply-chain verifier (digest-pin, sigstore)    │   │
                          │            └───────┬──────────────────────────────┬───────────┘   │
                          │       lease + token│ (mint_run_token via Id)      │ provision     │
                          │                    ▼                              ▼               │
                          │   ┌──────────── Runner host (KVM bare-metal / EU IaaS) ────────┐  │
                          │   │  Runner agent (small attested Rust binary)                 │  │
                          │   │   • claims jobs for its labels  • heartbeats the lease     │  │
                          │   │   • SandboxBackend::launch(JobSpec)                        │  │
                          │   │   ┌──────── microVM (Firecracker, default) ───────────┐    │  │
                          │   │   │  guest kernel · read-only root + tmpfs · egress    │    │  │
                          │   │   │  default-deny · caps dropped · seccomp · pids.max  │    │  │
                          │   │   │  one-job-per-sandbox, ephemeral, killed on teardown│    │  │
                          │   │   └────────────────────────────────────────────────────┘    │  │
                          │   └──────────────┬──────────────────────────────┬──────────────┘  │
                          │   firehose frames │ (logs)        artifacts/caches│ (BlobStore T2)  │
                          │                   ▼                              ▼                 │
                          │         Storage: firehose/T3 log tier · T2 blobs · OLAP read store │
                          └─────────────────────────────────────────────────────────────────┘
                  outputs: ci.run.* / ci.status.updated / ci.log.available / ci.deployment.* (outbox→bus)
                           → Git checks/merge gate · Notif inbox · Chat cards · Search · Refs · Agents
```

**The five logical services** (each its own `serve` shell, its own Postgres, no cross-DB):

1. **Trigger & Dispatch** — close to the bus; matches `EventMatcher`, dedups, evaluates trust tier,
   resolves + content-addresses the definition, starts the workflow. Stateless except the dedup ledger.
2. **CI Control Plane** — the scheduler + reaper + fleet autoscaler + log-pipeline coordinator + secret
   broker + supply-chain verifier. The latency-/correctness-critical Rust core. Owns `job_queue`,
   `runner`, `lease`, deployment/environment state, the log range index, artifact/cache indices.
3. **The runner agent** — a small attested Rust binary on each runner host; pulls leases, launches the
   sandbox via `SandboxBackend`, streams frames, reports terminal. Same binary hosted + self-hosted.
4. **The sandbox backend** — Firecracker (default) behind the `SandboxBackend` trait; gVisor is the named
   second backend; self-hosted is a delegated backend. **The trust boundary.**
5. **The `ci.pipeline` / `ci.deploy` workflow definitions** — deterministic Rust functions registered on
   `myelin-flow` at `serve`; the DAG walk + gates + bookends live here.

These are *logical* services; small self-host cells may co-locate them in one process (the harness
permits binary co-location; the DB-per-service + contract boundaries hold regardless).

## 5. Cell topology & where CI sits

CI lives **inside each cell** (a cell = a complete region-pinned stack; Tenancy §7). There is **no global
runner pool** — pools are partitioned per residency zone; an EU-resident tenant's job is claimed only by a
runner in its region, and its logs/artifacts/caches/state stay in-region (the `residency-pin` lint, S-1,
on every store CI writes). The PII-free global control plane (`discover`/`placement_of`) is consulted only
for routing (the CLI/wire reaching the right cell); the per-request hot path is in-cell. Scaling is
**add-a-cell** (bulkhead); within a cell, CI scales by adding runner hosts (the pull model makes this
trivial) and by partitioning the scheduler's claim load (01 §6, 02 §2). Multi-cell-spanning runs are a
named floor (designed-not-built; inherits Workflow §7.4).

## 6. Floors named up front (VISION §3)

| Floor (ships v1) | Named follow-on |
|---|---|
| **One sandbox backend (Firecracker) through the escape drill** | gVisor second backend behind the same trait + its own drill (density/latency-triggered) |
| **Single-cell pipelines** | Cross-cell-spanning runs (inherits the Workflow multi-cell floor) |
| **One/two `FleetProvider` adapters + self-hosted** | More EU-provider adapters (adapters, not redesigns) |
| **DRR fair-share at claim time** | A richer hierarchical scheduler (measured-starvation-triggered) |
| **Object-segment log tier (T3) + OLTP range index** | A dedicated time-series/wide-column log tier (measured-volume) |
| **Per-tenant-DEK log/artifact crypto-shred** | Per-subject free-text shred in logs (GD-6 / `[OPEN → LEGAL]`) |
| **SLSA L1–L2 provenance + SBOM** | Hermetic / two-party (L3+) provenance (demand-triggered) |
| **Component trust model (digest-pin + sign-verify + SLSA)** | The registry *product* (hosting/discovery) — commercial-flagged |
| **`myelin ci local` not built** | Laptop execution (UX-vs-fidelity; deferred) |

See 05 §floors for the full table and 07 for the drills + open questions.
