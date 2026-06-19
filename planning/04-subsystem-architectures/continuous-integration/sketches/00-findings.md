# CI/CD — Stage-1 Findings (what I learned, what I commit, what I hand forward)

> Phase 4 — CI subsystem, Stage-1 (Design & Sketch) synthesis. Consumes the six exploration sketches
> (01–06) + the three design sketches (`design/`). Canonical brief: VISION. Binding: integration
> directives **CI-1, CI-2**, X-1…X-5, **ADR-20/D5** (one sandbox, TE-31=UNIFY), **D8** (reserve/settle),
> EI-03 §3/§5/§6, EI-04 §5.1. Phase-3 build-to surface: the contract-index + the 11 system docs.
> Date: 2026-06-19. **Status convention:** *COMMITTED* = decided for the architecture stage; *FLOOR* =
> partial answer with a named follow-on; *[OPEN → architecture/P5/LEGAL]* = handed forward.

---

## 1. What I learned (the shape of this subsystem)

CI is **the execution arm of the event fabric** and the platform's **single hardest security surface**
— the one place untrusted customer code runs. Phase 3 has already done most of the heavy lifting *for*
me: the sandbox is a unified runner I own but whose hardening profile + escape-drill obligation are
pre-specified (ADR-20/CI-1); a CI pipeline **is a durable workflow** on an engine that already exists
(`myelin-flow`); logs ride a firehose that already exists (Bus §4.3); artifacts/caches/crypto-shred
ride storage tiers that already exist (Storage T2/T3); the reserve/settle gate already exists (contract
11.7); per-run job tokens are Id's `mint_run_token`. **My job is mostly composition + the scheduler +
the two genuinely-CI-hard problems (isolation backend, EU fleet elasticity), not green-field
invention.** The biggest realisation: **TE-31=UNIFY collapses CI and the agent sandbox into one
build-and-drill-once primitive**, and the cleanest way to say where they unify is *"shared hands +
hardening; distinct head + governance"* (sketch 06).

A second realisation: the Rust default holds with **zero justified divergence** — every CI component is
either a hot path (scheduler, state machine, runner agent) or a contract surface (outbox, ToolDefs).
The only "divergence" is by *constraint* (no hyperscaler autoscaling → we build the fleet autoscaler on
EU infra), not by language (ADR-02; Phase-2 §3).

## 2. Committed direction on each hard problem

| Hard problem | COMMITTED direction (the leaning Stage 2 builds on) | Sketch | Prior art |
|---|---|---|---|
| **Isolation model (TE-28)** | **microVM (Firecracker) = default for untrusted**, behind a runtime-agnostic `SandboxBackend` trait; **gVisor = named second backend** for high-density/low-risk; **ship ONE backend through the escape drill first**. Hardening profile (CI-1) mandatory on both. | 01 | Firecracker (NSDI 2020); gVisor (2019) |
| **Runner-fleet elasticity, EU infra (TE-29)** | **Pull-leasing** (lease + heartbeat + reaper, the platform's existing `FOR UPDATE SKIP LOCKED` primitive); **autoscale-on-queue-depth over EU IaaS/bare-metal via a `FleetProvider` trait**; pre-warmed microVM snapshot pools; scale-to-zero; **no global pool — partitioned per residency zone**. | 03 | Buildkite/Nomad pull model; DRR (Shreedhar & Varghese 1996) fairness; AWS cell/bulkhead |
| **Config grammar / config-as-code** | **Declarative JSON-schema'd core + the shared bounded query-AST for expressions (one expression language platform-wide, no CEL) + a sandboxed dynamic-generation escape hatch**; deterministic resolution → content-addressed snapshot per run; shift-left (`validate`/`plan`) is core. | 05 | ADR-07 query AST; Buildkite dynamic pipelines |
| **Component/action registry supply-chain (TE-30)** | **Digest-pin or fail-closed** (images AND components); **sign + verify-before-use (sigstore/Fulcio+Rekor) + SLSA provenance + SBOM**; reuse the platform's CT-Merkle pattern (RFC 6962), EU-hosted. Registry *product* is commercial-flagged; the **trust model is built regardless**. | 05 | SLSA (OpenSSF); sigstore; RFC 6962 |
| **CI↔agent unification depth (TE-31 = UNIFY)** | **Shared: the job spec (`kind∈{ci,agent}`), the sandbox runner + hardening, the escape drill, the reserve/settle gate, the secret broker. Distinct: the orchestration workflow, the brain, and the `EffectApi` governed-mutation path** (never `ToolHands::exec`). `ToolHands::exec` IS `SandboxBackend::launch(kind=agent)`. | 06 | ADR-20/D5; Agent §2.2/§5.0 |
| **Metering unit (TE-32)** | **resource-seconds (cpu/mem-GB/gpu-seconds + storage-GB-hours + egress-GB) as the wholesale meter**; Commercial maps → credits at the markup layer (kept separate, immutable pricing); one `CostEvent` schema fronts CI + agent; integer minor-units. | 06 | D8/C-1; EI-03 §5.2 |
| **Caching/artifacts at scale** | **Content-addressed `BlobStore` (BLAKE3, per-tenant dedup), residency-local**; caches **scoped by trust tier** (fork can't poison/write the trusted cache); explicit retention/TTL/GC. | 04 | Storage T2; git/Venti/IPFS CID |
| **Secrets inside the boundary (CI-1)** | **Secret *names* in the job spec; resolved by an in-boundary broker per run, scoped to this job; OIDC short-lived creds over static keys; untrusted/fork runs get NO secrets**; log-masking is defence-in-depth, NOT the boundary (egress default-deny is). | 04 | CI-1; EI-03 §3 |
| **Pipeline = durable workflow** | **Hybrid:** `myelin-flow` owns run lifecycle + gates + budget + timers + crash-recovery; **CI owns the scheduler/fleet, reached through the `SCHEDULE_AND_RUN_JOB` durable activity**; **the activity boundary is the JOB, not the step** (Q17 answer). | 02 | DBOS/Temporal (Workflow §2) |

## 3. Primary screens designed (design/ — required before architecture)

Information-architecture, user-flows, and wireframes are committed. The **8 primary screens** (from
design-language §7.2), each with **empty / loading / error** (+ permission-denied / erased) states and
the §8b day-one primitives applied:

1. **Run list / dashboard** ("is main green?") · 2. **Single-run view** (DAG + jump-to-failure +
pre-fetched context pane) · 3. **Live log view** (firehose, secret-masked, collapsible per step,
range-anchored) · 4. **Environments & deployments + Approvals queue** (incl. the **HITL approval card**
overlay) · 5. **Pipeline editor + validator** (schema/lint/plan, no runner spend) · 6. **Runner fleet**
(health + attestation) · 7. **Agent-surfaced triage** (plan-then-apply, provenance, depth-ceiling
visible) · 8. **Usage / quota / billing** (resource-seconds → credits, reserve-gate honesty).

Plus the **cross-subsystem surfaces CI feeds** (Git checks badge, issue status, chat unfurls/approval
card, knowledge embeds, the one notifications inbox) and the **CLI as a peer surface**. Key flows
designed: push→checks→merge gate; the **agent triage flagship** (the full plan→gate→approve→resume
HITL loop); deploy-gated-on-issue-transition; shift-left validate/plan; self-hosted runner attestation;
GDPR erasure reaching CI; the dual-audience engineer-vs-PM "release readiness" split.

## 4. Decisions I am now committing (carried into the architecture stage)

- The **`JobSpec{ kind ∈ {ci, agent} }` + `SandboxBackend` trait + `FleetProvider` trait** are the three
  seams the architecture builds on; Firecracker is the v1 backend, gVisor the named second.
- **Pull-leasing scheduler** with DRR fairness, priority lanes (interactive > batch > deploy),
  concurrency groups (serialize / cancel-superseded), affinity labels, per-tenant in-flight caps.
- **Pipeline = a `myelin-flow` workflow; scheduler behind a durable activity; job-level granularity.**
- **Reserve/settle = the workflow's bookends** (no second metering path); meter = resource-seconds.
- **Digest-pin-or-fail-closed** for both images and components; sign+verify+SLSA+SBOM.
- **Logs = firehose + `ci.log.available` pointer events ONLY on the durable bus** (CI owns this
  taxonomy); the run-state/log/artifact/cache holders all crypto-shred-capable + residency-pinned.
- **Rust throughout, no justified divergence**; the only constraint-divergence is the EU fleet
  autoscaler.
- The **escape drill (AG-D4) is CI's single hard go/no-go** before any untrusted code (CI or agent)
  runs — the architecture stage treats it as the gating milestone, not a late add.

## 5. Open questions handed to my own architecture stage

1. **Scheduler internals depth** (sketch 03): the exact DRR weights, the priority-lane preemption
   policy, the claim-query SQL at QPS, and the backpressure thresholds — designed at architecture
   altitude with a concrete data model.
2. **The `SCHEDULE_AND_RUN_JOB` activity ↔ scheduler handshake** (sketch 02): the precise contract by
   which the durable activity blocks on, and resumes from, the runner's terminal report (signal vs
   activity-completion; idempotency token threading) — co-finalised with Workflow §9 shapes.
3. **The exact `ci.*` event taxonomy** under the Bus §6 grammar (singular, past-tense, dotted): the
   complete list of lifecycle + pointer + deployment + resource events CI emits/consumes, plus the
   stable `#sub` scheme (`#job-…`/`#step-…`/`#L…`) — CI's Bus-§6 completion obligation.
4. **`project(ref, viewer)` + `IndexSpec` + ReBAC namespace fragment + `ToolDef` set** — the four
   "every subsystem must implement" contracts (README §5), specified concretely (the run/log/artifact
   projection shape; which fields are code-searchable; the CI relation namespace; the
   `requires_approval` defaults per tool — joint with the agent fabric).
5. **`requires_approval` defaults + the approver `list_subjects` set per CI tool** (deploy, cancel,
   secret-write) — the per-subsystem HITL product call (Agent §12 Q1).
6. **The metering ↔ Commercial wallet seam** (sketch 06): the exact `CostEvent` units reconciled with
   Commercial's pricing table (X-5 names-and-units reconciliation).
7. **gVisor-second-backend promotion trigger** (sketch 01/06): the measured density/latency economics
   that justify the second backend (esp. for sub-second agent `compute` calls).
8. **[OPEN → P5]** the full **escape-drill (AG-D4) adversarial corpus** + the
   fork-can't-poison-cache / fork-gets-no-secrets / residency drills (sketch 01/04) — CI enumerates the
   obligation; Phase 5 executes.
9. **[OPEN → LEGAL]** crypto-shred completeness for **free-text PII in logs** (per-tenant vs per-subject
   DEK granularity, GD-6); **build-data-as-LLM-training lawful basis** (AG-8); **CD-as-PaaS** product
   scope (PR-5) — all flagged, none foreclosed.
10. **`myelin ci local`** (laptop execution) — UX win vs fidelity cost; deferred (CI-DD §11 Q12).

## 6. Floors named up front (VISION §3 — name the floor + the follow-on)

- **One sandbox backend (Firecracker) to the drill first**; gVisor-second is the named follow-on.
- **Single-cell pipelines**; cross-cell-spanning runs are designed-not-built (inherits Workflow §7.4).
- **One/two `FleetProvider` adapters** v1 (the commercial infra pick) + self-hosted; more are adapters.
- **DRR fair-share** ships; a richer hierarchical scheduler is promotion-triggered by measured starvation.
- **Object-segment log tier** ships; a dedicated time-series/wide-column tier is measured-volume follow-on.
- **Per-tenant-DEK log crypto-shred** ships; per-subject free-text shred is the GD-6/LEGAL follow-on.
- **SLSA L1–L2 provenance** ships; hermetic/two-party (L3+) is demand-triggered.
- **Component registry trust model** is built; the registry *product* (hosting/discovery) is commercial.
