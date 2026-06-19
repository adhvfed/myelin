# Continuous Integration / CD — Subsystem Architecture (Phase 4)

> Phase 4 subsystem architecture for Myelin's **Continuous-Integration / CD** subsystem. CI is the
> **execution arm of the event fabric** and the platform's **single hardest security surface** — the one
> place untrusted customer code runs. It also owns the **one unified sandbox runner** (ADR-20 / TE-31=UNIFY)
> on which the Agent Fabric's `ToolHands::exec` runs, so the catastrophic surface — untrusted execution —
> is **built and drilled once**, here.
>
> Canonical inputs: VISION; doctrine EI-03/04/05; directives CI-1/CI-2, ADR-20/D5, D8; the Phase-3
> contract-index + the 11 system docs; the Phase-3 handoff (README §5, CI bullet). Tightest seam: the
> **Git↔CI checks/merge-gate** contract.

---

## Design (Stage 1 — `design/`)

The UX surface, designed before architecture (VISION §3/§5.2):

- [`design/information-architecture.md`](./design/information-architecture.md) — where CI lives in the one
  shell (rail + CI sidebar + pre-fetched context pane), the §7.2 view inventory, deep-linking down to
  sub-artifact granularity, the cross-subsystem surfaces CI feeds.
- [`design/user-flows.md`](./design/user-flows.md) — push→checks→merge gate; the **agent-triage flagship**
  (structured failure → plan-then-apply → HITL gate → resume → PR); deploy-gated-on-issue; shift-left
  validate/plan; self-hosted attestation; GDPR erasure reaching CI; the engineer-vs-PM dual audience.
- [`design/wireframes.md`](./design/wireframes.md) — the 8 primary screens, each with empty / loading /
  error / permission-denied / erased states + the §8b day-one primitives.

## Sketches (Stage 1 exploration — `sketches/`)

- [`sketches/00-findings.md`](./sketches/00-findings.md) — the Stage-1 synthesis (committed directions).
- `01-isolation-model.md` · `02-pipeline-as-durable-workflow.md` · `03-scheduler-and-fleet-elasticity.md` ·
  `04-logs-artifacts-caches-firehose.md` · `05-config-grammar-and-supply-chain.md` ·
  `06-metering-unit-and-substrate-unification.md`.

## Architecture (Stage 2 — `architecture/`) — the build-to surface for Phase 5

| Doc | Contents |
|---|---|
| [`architecture/00-overview.md`](./architecture/00-overview.md) | Role & responsibilities; owns-vs-delegates; the internal component map; the cell topology; floors. |
| [`architecture/01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | **Rust throughout** (justified, zero divergence); the `JobSpec` / `SandboxBackend` / `FleetProvider` seams; the full schema; residency/encryption posture. |
| [`architecture/02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | Trigger→dispatch; the DRR-fair-share pull-leasing **scheduler**; the **pipeline-as-durable-workflow** mapping (the `SCHEDULE_AND_RUN_JOB` handshake); the **sandbox runner + escape drill**; the EU **fleet autoscaler**; logs/artifacts/caches/secrets; the resource-second meter. |
| [`architecture/03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete **`ci.*` taxonomy** + consumed events; the **Git↔CI seam**; every glue contract (envelope/outbox, `ArtifactRef`+`#sub`, `project`, `replay`, Identity `check`/`list_objects` + the **ReBAC fragment**, `PersonalDataHolder`, `IndexSpec`, reserve/settle). |
| [`architecture/04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The view inventory; the `myelin ci` CLI; the **`ToolDef`** agent-tool surface + the `requires_approval` defaults; the public/internal API split. |
| [`architecture/05-hard-problems.md`](./architecture/05-hard-problems.md) | HP-1…HP-9 resolutions + **cited prior art** + named floors. |
| [`architecture/06-shared-system-change-requests.md`](./architecture/06-shared-system-change-requests.md) | The itemized Phase-5 reconciliation list (Identity/Bus/Workflow/Storage/Agents/GDPR/Tenancy). |
| [`architecture/07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The quantified drills (T-1 the escape gate, D-1…D-10, R-3) + open questions by resolver. |

---

## The decisions in one place

- **Language/DB:** **Rust throughout, zero justified divergence** (every CI component is a latency/correctness
  hot path or a trust-boundary surface, ADR-02); storage delegated to the Phase-3 tiers (OLTP + `BlobStore`
  T2 + log tier T3 + OLAP). The only divergence is **by constraint** — CI *builds* the EU fleet autoscaler
  because ADR-11 forbids hyperscaler autoscaling.
- **Isolation (HP-1):** Firecracker microVM default, gVisor named-second, one backend drilled first;
  mandatory hardening profile.
- **Scheduler (HP-2):** pull-leasing + DRR fair-share + priority lanes + concurrency groups + affinity;
  EU `FleetProvider` autoscaler; no global pool.
- **Pipeline = durable workflow:** `myelin-flow` owns lifecycle/gates/timers/reserve-settle; CI owns the
  scheduler behind the `SCHEDULE_AND_RUN_JOB` activity; the activity boundary is the **job**.
- **UNIFY (HP-5):** shared hands + hardening; distinct head + governance. `ToolHands::exec` *is* CI's runner.
- **Metering (HP-6):** resource-seconds wholesale → credits markup (separate, immutable).
- **Supply chain (HP-4):** digest-pin-or-fail-closed; sigstore sign-verify; SLSA + SBOM.
- **Logs:** ride the firehose; CI owns `ci.log.available` **pointer** events only on the durable bus.
- **The gate:** **T-1, the real-kernel escape drill, precedes everything** that runs untrusted code.
