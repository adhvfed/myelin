# Continuous Integration / CD — Subsystem Architecture (Phase 5-B)

> Detailed architecture for Myelin's **Continuous-Integration / CD** subsystem, **rewritten from scratch
> against the reconciled shared layer** (Phase 5-B). CI is the **execution arm of the event fabric** and the
> platform's **single hardest security surface** — the one place untrusted customer code runs. It also owns
> the **one unified sandbox runner** (ADR-20 / TE-31=UNIFY / X-6) on which the Agent Fabric's
> `ToolHands::exec` runs, so the catastrophic surface — untrusted execution — is **built and drilled once**,
> here, behind the **one real-kernel escape drill that gates ALL agent execution**.
>
> **Canonical inputs (never contradicted):** VISION; doctrine EI-03/04/05. **The reconciled layer (binding,
> frozen):** [`05/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> (read §X-1, §X-6, §X-7, §OQ-E, §OQ-F + the Part-4 CI punch list) +
> [`05/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md). Tightest seam:
> the **Git↔CI `CheckStatus` contract** (frozen as contract 5.9 / X-1).

---

## Design (Stage 1 — `design/`, PRESERVED, not rewritten)

The UX surface, designed before architecture (VISION §3/§5.2). **The design record is preserved as-is**; note
that its prose references the Phase-4 event name `ci.status.updated` for the Git seam — the architecture
(below) supersedes that with the frozen `ci.check.updated` (carrying `CheckStatus`) + the `ci.result` rollup
signal.

- [`design/information-architecture.md`](./design/information-architecture.md) — where CI lives in the one
  shell (rail + CI sidebar + pre-fetched context pane), the view inventory, deep-linking to sub-artifact
  granularity, the cross-subsystem surfaces CI feeds.
- [`design/user-flows.md`](./design/user-flows.md) — push→checks→merge gate; the **agent-triage flagship**
  (structured failure → plan-then-apply → HITL gate → resume → PR); deploy-gated-on-issue; shift-left
  validate/plan; self-hosted attestation; GDPR erasure reaching CI; the engineer-vs-PM dual audience.
- [`design/wireframes.md`](./design/wireframes.md) — the primary screens, each with empty / loading / error /
  permission-denied / erased states + the day-one primitives.

## Sketches (Stage 1 exploration — `sketches/`, PRESERVED)

- [`sketches/00-findings.md`](./sketches/00-findings.md) — the Stage-1 synthesis (committed directions).
- `01-isolation-model.md` · `02-pipeline-as-durable-workflow.md` · `03-scheduler-and-fleet-elasticity.md` ·
  `04-logs-artifacts-caches-firehose.md` · `05-config-grammar-and-supply-chain.md` ·
  `06-metering-unit-and-substrate-unification.md`.

## Architecture (Stage 2 — `architecture/`) — the build-to surface, reconciled

| Doc | Contents |
|---|---|
| [`architecture/00-overview.md`](./architecture/00-overview.md) | **Changes vs the Phase-4 first pass (Δ1–Δ12)**; role & responsibilities; owns-vs-delegates; the component map; the cell topology; floors. |
| [`architecture/01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | **Rust throughout** (justified, zero divergence); the `JobSpec`/`SandboxBackend`/`FleetProvider` seams; the full schema incl. the **CheckStatus source columns** (`run_attempt`) + the **per-subject log DEK** + **trust-scoped caches**; residency/encryption. |
| [`architecture/02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | Trigger→dispatch (trust-tier stamp + CheckStatus emit); the DRR pull-leasing **scheduler**; the **frozen `SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom**; the **sandbox runner + the four uniform guarantees + the escape drill**; the EU fleet autoscaler; logs (firehose + resume-cursor)/artifacts/caches/secrets; the resource-second meter. |
| [`architecture/03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete **`ci.*` taxonomy** incl. **`ci.check.updated` + `ci.result`**; the **Git↔CI `CheckStatus` seam**; every glue contract against the frozen shapes (envelope/outbox, `ArtifactRef`+`#sub`, `project`, `replay`, `check`/**`list_objects` `SetExpr` push-down** + the **ReBAC fragment**, `PersonalDataHolder` + erasure-by-reference, `IndexSpec`, reserve/settle). |
| [`architecture/04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The view inventory; the `myelin ci` CLI; the **`ToolDef`** surface + the **frozen X-6 `requires_approval` defaults**; the public/internal API split. |
| [`architecture/05-hard-problems.md`](./architecture/05-hard-problems.md) | HP-0 (the CheckStatus seam) … HP-9 resolutions + **cited prior art** + named floors. |
| [`architecture/06-reconciliation-compliance.md`](./architecture/06-reconciliation-compliance.md) | **How CI implements the frozen reconciled contracts** (the compliance map), the deltas absorbed, and the RESIDUAL Phase-6 / Legal items. (Replaces the Phase-4 "06 shared-system change requests".) |
| [`architecture/07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The quantified drills (T-1 the escape gate, D-1…D-11, R-3) + open questions by resolver (with the Phase-5-closed items recorded). |

---

## The decisions in one place

- **Language/DB:** **Rust throughout, zero justified divergence** (reconciliation forced no change); storage
  delegated to the frozen tiers (OLTP + `BlobStore` T2 + T3 log tier + OLAP). The only divergence is **by
  constraint** — CI *builds* the EU fleet autoscaler (ADR-11 forbids hyperscaler autoscaling).
- **The Git seam (X-1, frozen):** CI emits **`ci.check.updated`** carrying the `CheckStatus` fact + the
  **`ci.result`** rollup signal; stamps `trust_tier` + `run_attempt`; **Git** owns the projection, the gate,
  and fork-endorsement. CI reports facts; Git gates; CI never merges.
- **Isolation (HP-1, X-6):** Firecracker microVM default, gVisor named-second, one backend drilled first; the
  **four uniform guarantees** (cost gate, attribution, HITL withhold, isolation floor) inherited by
  `ToolHands::exec` = the CI runner's `kind=agent` job.
- **Scheduler (HP-2):** pull-leasing + DRR fair-share + lanes + concurrency groups + affinity; EU
  `FleetProvider` autoscaler; no global pool.
- **Pipeline = durable workflow:** `myelin-flow` owns lifecycle/gates/timers/reserve-settle; CI owns the
  scheduler behind the frozen **`SCHEDULE_AND_RUN_JOB`** idiom (dispatch + park, woken by `job.done`); the
  activity boundary is the **job**.
- **Metering (HP-6, the one path):** resource-seconds wholesale → credits markup; reserve/settle fronts every
  run + every dispatch into the same wallet as agents.
- **Supply chain (HP-4):** digest-pin-or-fail-closed; sigstore sign-verify; SLSA + SBOM.
- **Logs:** ride the firehose + the resume-cursor protocol; CI owns `ci.log.available` **pointer** events only
  on the durable bus; the frozen `(job,step,byte-range)` index resolves `CheckStatus.details_ref`.
- **GDPR:** per-subject DEK for isolable inline log PII (**built**); the residual handled **by reference** to
  the one platform erasure posture (X-7).
- **The gate:** **T-1, the real-kernel escape drill, precedes everything** that runs untrusted code (CI *or*
  agent).
