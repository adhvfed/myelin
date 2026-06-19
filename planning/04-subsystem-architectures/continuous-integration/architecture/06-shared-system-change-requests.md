# CI/CD — 06 Required Shared-System Changes (Phase-5 reconciliation list)

> Phase 4 — CI Stage-2. Itemized, explicit changes CI needs from the shared systems that are **not already
> in the Phase-3 contracts**, for the Phase-5 reconciliation agent. For each: **what, why (the consuming CI
> surface), and whether it is NEW or a CONFIRM** of an already-named seam. CONFIRMs are listed because the
> reconciliation pass must verify the seam is honoured at the contract's edge, even where Phase 3 already
> intended it.

---

## Identity & Access

| # | Change | Why (CI surface) | Nature |
|---|---|---|---|
| **CR-ID-1** | **Per-job attenuated token mintable mid-workflow on resume**, bound to one job/repo, life == job life; **self-hosted runner tokens scoped to one tenant's `SelfHosted`-tier jobs only**. | The scheduler→runner handoff (02 §3.3); self-hosted attestation (02 §5). A multi-day-gated pipeline re-mints its job token when a stage resumes. | CONFIRM (Id contract 4.7 + S-11; the **self-hosted-scope** specificity is the new part). |
| **CR-ID-2** | **CI's ReBAC namespace fragment** (`ci_project` / `ci_environment` / `ci_secret` / `ci_run`) compiled into the cell schema, incl. the **ABAC edge `read & !is_untrusted_fork`** on `ci_secret` and the `approver` relation as the `list_subjects` HITL target. | The trust-tier secret gate (02 §7.3) and the protected-env approver set (03 §5.2). | **NEW** (sharpens Id §5 / contract 4.9 with CI's fragment + the fork-tier ABAC exclusion). |
| **CR-ID-3** | **`list_objects` push-down composable over the `run_id` column** at run-list/search scale (no post-filter, no N+1). | The run list, "all runs", release readiness, and CI search (03 §5.1, 04 §1). | CONFIRM (Id §8.2 + S-10; CI is a named consumer over `run_id`). |

## Event Bus

| # | Change | Why (CI surface) | Nature |
|---|---|---|---|
| **CR-BUS-1** | **The firehose transport carries `ci.log.appended` frames at CI's volume** (the heaviest firehose producer), with `firehose::tail(stream, range)` fan-out to many live viewers; the durable bus carries **only** `ci.log.available` pointer events. | The live-log view + the archive seam (02 §7.1). CI is the firehose's primary driver. | CONFIRM (Bus §4.3 / contract 3.5 — CI is the named primary producer; the **per-line-must-not-hit-the-durable-bus** invariant is CI's to honour). |
| **CR-BUS-2** | **`EventMatcher` cost-bound suffices for CI trigger predicates** (`on: pull_request`/`issue.transitioned` with path/branch/status filters) without a CI-specific trigger language. | Trigger & Dispatch (02 §1). CI must not invent CEL. | CONFIRM (Bus §4.5 / contract 3.4). |
| **CR-BUS-3** | **`ci.run.snapshot` etc. support sub-artifact-granular replay** (one run / one deployment / a project scope) through the live consumer path. | `replay(scope, since)` for Search/Refs/OLAP cold rebuild + post-restore re-erasure (03 §7.3). | CONFIRM (Bus §4.9 / contract 2.6 — the sub-artifact granularity for CI runs is the specificity). |

## Durable Workflow

| # | Change | Why (CI surface) | Nature |
|---|---|---|---|
| **CR-WF-1** | **The `SCHEDULE_AND_RUN_JOB` activity + the `ci.result:<job_id>` durable signal handshake**: an activity enqueues a job, parks, and the workflow advances on the runner's **terminal signal** (which may arrive hours later); the reaper-driven retry is idempotent on the activity `idem_token`. | The pipeline-as-workflow mapping (02 §3.3) — the seam between CI's scheduler and the engine. | **NEW** (a concrete activity-vs-signal pattern over Workflow §4.4/§3.4; the engine must support a long-parked activity whose completion is a signal, not in-line blocking). |
| **CR-WF-2** | **Reserve/settle as the `ci.pipeline` workflow's bookends** (reserve at start refuse-on-exhaustion; settle on completion; never interrupt in flight) — the one metering path for CI. | The cost gate (02 §6). | CONFIRM (Workflow §6.2 / contract 11.7 / CI-2). |
| **CR-WF-3** | **Multi-day HITL deploy gate via `wait_for_signal`** holding no runtime, woken by the `approval` signal from the chat card / `myelin ci deploy approve`. | Protected-env deploys (02 §3.1; 04 §1). | CONFIRM (Workflow §3.4 / contract 9.4). |

## Storage

| # | Change | Why (CI surface) | Nature |
|---|---|---|---|
| **CR-STOR-1** | **A T3 log tier that seals firehose frames into T2 content-addressed segments + an OLTP `(job,step,byte-range)` range index**, append-mostly, at CI's volume; per-tenant-DEK envelope encryption for crypto-shred. | The durable log archive + jump-to-failure (02 §7.1; 01 §3.4). | CONFIRM (Storage §3.3 — CI is the heaviest consumer; the **byte-range index keyed by (job, step)** is CI's specificity). |
| **CR-STOR-2** | **Trust-tier/branch-scoped cache namespaces in `BlobStore`** so an `UntrustedFork` write cannot reach the trusted cache scope. | Cache-poisoning resistance (02 §7.2; HP-7). | **NEW** (a scoping convention over Storage §3.2's per-tenant `BlobStore` — the **scope key** is CI's). |
| **CR-STOR-3** | **Per-subject-DEK option for free-text PII in log segments** (the GD-6 follow-on) alongside the per-tenant-DEK default. | Erasure granularity for inline log PII (HP-7 floor; 03 §6). | **NEW (named floor)** (extends Storage §5.1 GD-4 granularity to CI's log free-text case; `[OPEN → LEGAL]`). |

## Agent Fabric

| # | Change | Why (CI surface) | Nature |
|---|---|---|---|
| **CR-AGENT-1** | **`ToolHands::exec` is realised by CI's runner as the `kind=agent` job** on the same hardened sandbox, metered into the same wallet under the agent's reserve; the **escape drill gates both kinds**. | The deepest unification (HP-5; 02 §4/§5). | CONFIRM (Agent §11.2 / contract 8.4/8.8 — CI owns the runner + the drill). |
| **CR-AGENT-2** | **CI's `ToolDef` set + the `requires_approval` defaults** (deploy-on-protected = gated; approve-deploy = always gated; secret-write = always gated) registered into the one `ToolSurface`, MCP-exposable. | The agent-tool surface (04 §3). | CONFIRM (Agent §6 / contract 8.1 — the CI defaults are CI's product call, jointly with the fabric). |

## GDPR / Audit + Legal

| # | Change | Why (CI surface) | Nature |
|---|---|---|---|
| **CR-GDPR-1** | **Harness auto-registration of every CI store** (run-state, logs, artifacts, caches, deployments) as an erasable `PersonalDataHolder`; `restrict` suppresses indexing/agent-use/analytics/notif for the subject. | "We forgot the cache table" must be structurally impossible (03 §6). | CONFIRM (contract 1.4 / 10.1). |
| **CR-GDPR-2** | **The `build-data-as-LLM-training` lawful basis (AG-8) and `CD-as-PaaS` product scope (PR-5)** are Legal/DPO calls CI flags, not foreclosed. | Future agent-on-CI-data + CD product (07 §open). | **NEW** `[OPEN → LEGAL]`. |

## Identity / Tenancy (residency)

| # | Change | Why (CI surface) | Nature |
|---|---|---|---|
| **CR-TEN-1** | **`residency_verify(tenant)` must cover CI's stores** (runner pool region, log/artifact/cache region) — the no-global-pool property is attestable. | The residency drill (07 R-3); the EU-sovereign pitch. | CONFIRM (Tenancy contract 12.4 — CI's stores are added to the verify fan-out). |

---

## Reconciliation cross-check (X-5 names & units)

CI aligns to the canonical anchors: **timestamps RFC-3339 UTC; costs integer minor-units (never floats);
TTLs/lease-windows/timers in seconds; resilient-client timeouts in milliseconds; `pii_key_ref =
kms://<tenant>/<dek-epoch>/<class>`**. The CI-owned units to reconcile with Commercial (X-5): the
**resource-second meter** (`cpu_seconds`, `mem_gb_seconds`, `gpu_seconds`, `storage_gb_hours`, `egress_gb`)
→ Commercial's credit/price markup table (06 → C-1; 07 §open Q6). CI emits the `ci` subsystem token + the
`run`/`deployment`/`pipeline`/`runner`/`artifact` type tokens under the Bus §6.2 table (Refs validates,
never re-authors).
