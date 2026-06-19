# Phase 6 — Roadmap: Continuous Integration (CI/CD) subsystem

> Phase `06-roadmaps`. The detailed, sequenced build roadmap for the **Continuous-Integration / CD** subsystem.
> It slots into the master sequencing bands (M0..M6) and **must not contradict** the band ordering or the gate
> invariant in [`../00-master-sequencing.md`](../00-master-sequencing.md). It refines the work *inside* the
> bands; it does not redesign.
>
> **Canonical inputs (never contradicted):** [`VISION.md`](../../../VISION.md) §3 (name-your-floors,
> agent-native, world-scale, EU-sovereign); doctrine
> [`EI-01`](../../../external-insights/01-process-and-quality-doctrine.md) (§2 order-by-non-negotiability — RCE
> before any feature; §3 prove-it-or-it-isn't-real; §5 the committed ratchet),
> [`EI-04`](../../../external-insights/04-hard-problems.md) (§5 untrusted code is a never-"done" surface, reindex-
> from-source). **FROZEN architecture (this roadmap sequences, it does not redesign):** the CI subsystem
> architecture [`../../04-subsystem-architectures/continuous-integration/architecture/`](../../04-subsystem-architectures/continuous-integration/architecture/)
> (00..07) + design [`../../04-subsystem-architectures/continuous-integration/design/`](../../04-subsystem-architectures/continuous-integration/design/);
> the reconciled shared layer + [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md);
> the testing strategy + the drill catalogue
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md).
> Plain text identifiers (no backticks-as-emphasis). Markdown only; no commits. Date: 2026-06-19.

---

## 0. Where CI sits in the master sequence — the one-paragraph orientation

CI is a **consumer subsystem** in the master plan: its primary, full landing is **M4** (the consumer band,
CI first within the band because it closes the X-1 `CheckStatus` seam Git already built the gate for in M3).
But CI is unusual: **its single hardest, most catastrophic property — the real-kernel sandbox-escape gate
(AG-D4 / CI-T1) — is owned by CI yet drilled in M2**, because the *runner* CI owns is the same unified sandbox
the agent fabric's `ToolHands::exec` runs on (ADR-20 / X-6). So CI's risk is front-loaded out of band order:
the escape drill is a **Tier-2 keystone** that must be green before any untrusted code (CI step *or* agent
compute) runs in M3+. The rest of CI is, by the architecture's own admission, **disciplined composition of
frozen contracts** plus two genuinely green-field cores: **the distributed scheduler** and **the EU fleet
autoscaler**. This roadmap therefore has an unusual shape: a small but load-bearing **M2 contribution** (the
unified runner + the escape gate, co-owned with the agent fabric), then the **bulk of CI in M4**, then
**world-scale hardening + floor follow-ons in M5**, then dogfooding in M6.

The critical-path facts that pin CI's sequencing (master §3.1):
- **AG-D4 / CI-T1 is on the critical path and is a permanent GATE.** It blocks ALL untrusted execution. CI
  must deliver the unified runner + the green escape attestation **in M2** (not M4), or M3 and everything
  downstream cannot run untrusted code.
- **X-1 / contract 5.9 is the tightest cross-subsystem seam**, split producer/consumer across M4/M3: Git built
  the projection + merge gate + supersession rule in M3; **CI ships the producer half in M4** to close it. The
  E2E-2 flagship (M5) depends on this seam being end-to-end green.

---

## 1. First runnable / first useful / production-hardened — the honest progression

| Stage | What exists | What is explicitly NOT yet real | Band |
|---|---|---|---|
| **First runnable** | The unified sandbox runner boots a `JobSpec` on a real Firecracker microVM behind `SandboxBackend`; the hardening profile is on; the escape-drill harness can launch the adversarial corpus and read a telemetry assertion. No scheduler, no fleet, no pipelines — just "untrusted bytes run isolated and we can prove it." | Fair-share scheduling, autoscaling, the `ci.pipeline` workflow, the check seam, deploys, metering on a real wallet. | **M2** |
| **First useful** | A push triggers a `ci.pipeline` durable workflow; jobs dispatch via `SCHEDULE_AND_RUN_JOB` onto a pull-leased fleet; logs stream over the firehose; CI emits `ci.check.updated` + `ci.result`; Git's merge gate goes green and merges. Reserve/settle gates every run. A developer can push, watch a build, and have it gate a PR. | 30× surge fairness proven, multi-cell runs, gVisor second backend, object-backed-pack-scale clone cost, SLSA L3, the registry product. | **M4** |
| **Production-hardened** | The F6 surge family green (human lane holds, batch/CI lane sheds 429+Retry-After, cross-tenant impact 0); residency attested at cell scale; reserve/settle parity proven across a pricing-change replay; the floor follow-ons (gVisor second backend, time-series log tier) promoted as their triggers fire; CI participates in the E2E-2 flagship and E2E-1/E2E-3 scenarios. | The deferred-by-design floors that remain named floors (laptop `myelin ci local`, the registry product, cross-cell-spanning pipelines until OQ-I lifts). | **M5** |
| **Dogfooded** | The Myelin monorepo's build/test/lint/mutation pipeline IS a Myelin CI pipeline; the twelve lints + the mutation gate run as Myelin CI jobs on every Myelin commit; the self-hosting CI graph is green. | — (this is the done-bar) | **M6** |

---

## 2. Upstream dependencies — what must exist (and be drilled green) before CI can build

CI is "the rest is disciplined composition of frozen contracts" (00 §1) — which means its dependencies are
heavy and **the bands before it must be green**. The gate invariant (EI-01 §2) means CI cannot claim a
milestone done over a red upstream gate.

### 2.1 Hard prerequisites for CI's M2 contribution (the unified runner + escape gate)

| Upstream (band) | Contract(s) | Why CI needs it before the runner |
|---|---|---|
| Failure-injection harness (M0) | testing-strategy §3 | The escape drill IS a harness drill — the 1×/10×/30× generator, the scoped dependency-break, the telemetry-assertion library. No harness → the escape property is a claim, not a fact. |
| `serve(AppSpec)` + three-port + liveness≠readiness (M0) | 1.1, 1.2, 1.3 | Every CI service is a `serve` shell; the public/internal split is the runner's trust boundary. |
| The transactional outbox + idempotent-consumer template (M0) | 2.1–2.5 | Every `ci.*` event is outbox-only (`no-raw-publish`); consumers dedup on `event_id`. |
| The twelve lints (M0) | 1.6 | `no-host-exec` (the sandbox-bypass forbidder), `residency-pin`, `flow-determinism`, `search-requires-acl-filter` all gate CI code. |
| Identity `mint_run_token` + `check` (M1) | 4.1, 4.2, 4.7 | The per-job attenuated token (life == job life, callable mid-workflow on resume); the self-hosted-runner scoped token. The runner cannot attribute or isolate without it. |
| Reserve/settle gate + KMS hierarchy (M1) | 11.7, 11.3 | The universal cost gate is one of the four uniform guarantees; per-subject DEK is the crypto-shred substrate for log PII. |
| Agent fabric `ToolHands::exec` + `EffectApi` + `JobSpec` (M2, co-built) | 8.1, 8.4 | CI's runner IS `ToolHands::exec`'s `kind=agent` job; the four uniform guarantees are jointly defined. This is co-developed with the agent fabric in M2, not a one-way dependency. |
| Durable workflow `SCHEDULE_AND_RUN_JOB` + `WfCtx` + durable signal (M2) | 9.1, 9.2, 9.4 | The dispatch-and-park idiom; the `job.done` / `ci.result` signal waits. CI's pipeline IS a `myelin-flow` workflow. |
| Firehose resume-cursor transport (M2) | 3.5 | CI logs ride this; the live-tail subscribes with `scope=run:<id>/job:<id>` and resumes losslessly. |

### 2.2 Additional prerequisites for CI's M4 full landing

| Upstream (band) | Contract(s) | Why |
|---|---|---|
| **Git** produces commits/refs/PRs + **owns the `check_status` projection + supersession rule + merge queue** (M3) | 5.9 (Git/gate half) | CI is the *producer*; Git built the *consumer/gate* half in M3. CI closes the seam in M4. Without Git's projection + merge-queue workflow already existing, CI's producer emits into a void. |
| Identity `list_objects` `SetExpr` push-down (M1) | 4.3 | The leak-free pre-filter for every CI run list/search (the OQ-E JOIN over `ci_run.run_id`). |
| Identity ReBAC engine + CI fragment slot (M1 engine, M4 fragment) | 4.9 | The `ci_project/environment/secret/run` namespace + the `read & !is_untrusted_fork` ABAC edge. |
| Storage T2 `BlobStore` + trust-scoped cache namespaces + T3 log tier `(job,step,byte-range)` index + per-subject log DEK (M1 base, C3/C4/11.8 land with CI) | 11.2, 11.8, 11.4 | Artifacts/caches/logs; the trust-tier cache-poisoning structural defence; the jump-to-failure index. |
| Refs `ArtifactRef` + `#sub` grammar + `project(ref, viewer)` + tombstone ladder (M2) | 5.1, 5.6, 5.7 | CI mints `ci/run/...#step-<n>` refs; `project` is the only way Refs/Search/Notif read a CI artifact. |
| Search `declare_indexable` (M2) | 6.3 | CI declares its `IndexSpec`; Search projects off the bus, conjoining the OQ-E Filter. |
| Notif `humanise` (M2) | 7.3 | Every CI status message / `CheckStatus.summary` is a `(template_key, args)` HumanisedRef. |
| Tenancy `(tenant, region)` + `residency_verify` (M1) | 12.1, 12.4 | No global runner pool; the residency attestation covers runner/log/artifact/cache region. |
| `myelin-query` `QueryAst` frozen (M2) | 3.4, 13.3 | The `EventMatcher` = the config-grammar expression language (no CEL). |

**The acyclicity rule (master §3.2, `no-cross-sync-cycle`):** CI never synchronously calls Git to ask "is it
green." CI emits `ci.check.updated`; Git reads its own projection. Git never calls CI; it consumes events.
Every cross-subsystem dependency is an async event/projection. Enforced at compile time by the lint (M0).

---

## 3. The milestones (mapped to master bands, with the work, floors, dependencies, gates)

### CI-M2 — The unified sandbox runner + the escape GATE (the Tier-2 keystone, co-owned with the agent fabric)

**Master band:** M2 (the reactive shared layer + the safety drills). **Why here, not M4:** CI owns the runner,
but the runner is the agent fabric's hands. The escape gate must be green **before any untrusted code runs in
M3+** (master §2 Tier 2; the single hard go/no-go). This is CI's slice of M2's exit gate.

**Work (CI's genuine green-field core #1 + the joint runner):**
- **The `SandboxBackend` trait + the Firecracker-default backend.** microVM (KVM + minimal VMM); the mandatory
  backend-independent hardening profile: egress default-deny, read-only root + tmpfs, caps dropped,
  no-new-privileges, seccomp, digest-pinned images fail-closed, whole-guest kill on teardown, `pids.max` +
  zero swap, secrets-resolved-in-boundary. One-job-per-sandbox, ephemeral, never reused across tenants.
- **`JobSpec{ kind ∈ {Ci, Agent} }` + the four uniform guarantees** (X-6, jointly with the agent fabric):
  the universal reserve/settle cost gate (11.7), per-run-token attribution (`mint_run_token`, 4.7), HITL
  withhold (plan-then-apply via `EffectApi` for agent mutations — CI's `exec` is never a side-effecting tool),
  the isolation floor + the escape drill. `ToolHands::exec` IS `SandboxBackend::launch(JobSpec{kind:Agent})`.
- **The runner agent** (small attested Rust binary): claims leases for its labels, heartbeats, launches the
  sandbox, streams firehose frames, reports terminal via the `job.done` signal. Same binary hosted +
  self-hosted; self-hosted attests and receives a tenant-`SelfHosted`-scoped job token.
- **The escape-drill adversarial corpus** (the `[OPEN → P6]` obligation from 07, now built): kernel-exploit
  primitives, cloud-metadata SSRF (169.254.169.254) → cred theft, control-plane/internal-RPC reach,
  cross-tenant network/storage, fork bomb vs `pids.max`, disk fill, secret exfil via egress. The
  green-attestation artifact format. Run on a **real kernel** on the production backend.

**Floor named here → follow-on:**
- **One backend (Firecracker) through the drill first** → gVisor as the named second backend behind the same
  trait, its own drill (CI-M5). Trigger: measured density/latency economics (esp. sub-second agent `compute`).

**Upstream deps:** M0 (harness, `serve`, outbox, lints, especially `no-host-exec`); M1 (`mint_run_token`,
reserve/settle, KMS); co-developed in M2 with the agent fabric (`ToolHands::exec`, `EffectApi`, `JobSpec`) and
durable workflow (`SCHEDULE_AND_RUN_JOB`, the `job.done` signal) and the firehose transport (3.5).

**Exit gate (CI's contribution to M2 → M3 — quantified, must be green):**
- **CI-T1 / AG-D4 (the hard GATE):** real-kernel adversarial corpus → **ZERO escapes** on the production
  backend. Green escape-attestation artifact **or CI is no-go for untrusted code.** Re-run on every
  backend/image/kernel change. *This blocks M3 and everything downstream of untrusted execution.* — GATE.
- (Joint with agent fabric, master M2 gate:) AG-D1/D2/D3 (no write outside `EffectApi`; effect outside the ∩
  denied; confined to `agent.policy ∩ delegation ∩ tenant.policy`); AG-D5 (HITL withhold) — CI.

---

### CI-M4 — The full subsystem: scheduler, fleet, pipelines, the check seam, deploys, metering

**Master band:** M4 (the consumer subsystems). **CI lands first within M4** (master §2): it closes the X-1
`CheckStatus` seam Git already gates on, and its runner is the same hardened runner already drilled in M2.

This is the bulk of CI. The runner + escape gate already exist (CI-M2). Now build the orchestration, the
scheduling, the fleet, the producer side of the check seam, deploys, metering, and the cross-fabric surfacing.

**Work — green-field core #2: the distributed scheduler (CI's heaviest correctness/latency problem):**
- **Pull-leasing scheduler** (`FOR UPDATE SKIP LOCKED` on `job_queue`; `lease_owner` + `lease_expires`;
  heartbeat extension; the dead-runner **reaper** sweeps expired leases → re-queue → the run's
  `SCHEDULE_AND_RUN_JOB` activity retries). No central live-capacity tracking; horizontal scale = more pulls.
- **DRR fair-share at claim time** across tenants; priority lanes (interactive / batch-CI / agent);
  concurrency groups; affinity. The claim is the scheduler's whole intelligence.
- *Floor:* flat DRR fair-share → a richer hierarchical scheduler (CI-M5/beyond, measured-starvation-triggered).

**Work — green-field core #3: the EU fleet autoscaler:**
- **Autoscale-on-queue-depth** over EU IaaS/bare-metal behind a `FleetProvider` trait (no hyperscaler
  autoscaling, ADR-11); pre-warmed microVM snapshot pools; scale-to-zero; bin-packing under the microVM memory
  floor. **No global pool — partitioned per residency zone** (the `region` predicate at claim time +
  `residency-pin`; attested by `residency_verify`, 12.4).
- *Floor:* one/two `FleetProvider` adapters + self-hosted → more EU-provider adapters (additive, demand-driven).

**Work — pipeline orchestration (composition of frozen `myelin-flow`):**
- **The `ci.pipeline` durable workflow** (stages → DAG of jobs); CI owns the *definition*, `myelin-flow` owns
  lifecycle/replay/timers/HITL/reserve-settle bookends. Jobs dispatch via **`SCHEDULE_AND_RUN_JOB`** (9.2);
  completion arrives as the `job.done` signal hours later (the workflow holds no runtime). The body is
  deterministic — no clock/RNG/IO outside `WfCtx` (the `flow-determinism` lint).
- **Trigger & dispatch:** match triggering events via the `EventMatcher` (= frozen `QueryAst`, 3.4 — no CEL);
  dedup on `event_id` (exactly-once *effect*); resolve + content-address the definition snapshot (T2 CAS blob,
  reproducibility/audit); **stamp `trust_tier`** from run provenance (trusted | untrusted_fork | self_hosted).

**Work — the X-1 `CheckStatus` producer half (the tightest seam, closing what Git built in M3):**
- **Emit `ci.check.updated`** per `(commit_oid, context)` carrying the frozen `CheckStatus` struct:
  `state ∈ {queued, in_progress, success, failure, error, neutral, cancelled}`, the monotonic **`run_attempt`**
  (u32, the supersession key — never `completed_at`), the stamped **`trust_tier`**, `details_ref = #step-<n>`
  (jump-to-failure), `summary` as a `(template_key, args)` HumanisedRef, `cost_settled` flipping true on
  reserve settle.
- **Emit the `ci.result` rollup signal** once all required contexts for the commit reach terminal:
  `{commit_oid, overall, contexts, idem_token}`, idempotent on `idem_token`. This wakes Git's merge-queue
  durable workflow (9.4). CI **never merges**; Git owns `required`, the projection, supersession, and
  fork-endorsement. An `untrusted_fork` success is **neutral for gating** until endorsed (CI stamps the tier
  from provenance; it never endorses — the poisoned-pipeline-execution defence).

**Work — logs / artifacts / caches (composition of frozen Storage):**
- Logs ride the **firehose** (`ci.log.appended` frames + the resume-cursor protocol, 3.5); `ci.log.available`
  *pointer* events are CI's only log-related durable bus event (coalesced, never per-line). Frames seal into
  the T3 log tier: T2 content-addressed segments + the OLTP **`(job, step, byte-range)` index** (11.8); the
  `#step-<n>` `details_ref` resolves through it; **per-subject DEK for isolable inline log PII** (11.4).
- Artifacts/caches: content-addressed T2 blobs, per-tenant dedup (cross-tenant dedup is a residency leak),
  **trust-tier/branch-scoped cache namespaces** (11.2, C3/C4 — a fork write cannot reach the trusted cache).

**Work — deployments & HITL, supply-chain, metering, the ReBAC fragment, cross-fabric surfacing:**
- Protected-environment gates as durable signals (`ci.deployment.approval_required` → the `approved` signal,
  per-effect `idem_key`, OQ-F); approvals queue + chat approval card; rollback first-class.
- **Supply-chain:** digest-pin-or-fail-closed (images + components — a floating tag refused at `plan`);
  sign + verify-before-use (sigstore Fulcio + Rekor, EU-hosted); SLSA L1–L2 provenance + SBOM (CycloneDX/SPDX)
  for produced artifacts; `ci.supply_chain.verification_failed` emitted on refusal (the fail-closed proof).
- **Metering:** resource-seconds (the wholesale unit); one `cost_event` per metered unit; **reserve/settle is
  the one metering path** — reserve at dispatch (incl. each `SCHEDULE_AND_RUN_JOB`), settle on `job.done`;
  refuse-start on exhaustion, never interrupt in-flight; wholesale ≠ markup.
- **The CI ReBAC fragment** (4.9): `ci_project / ci_environment / ci_secret / ci_run` + the frozen
  `read & !is_untrusted_fork` ABAC edge; the `watcher` relation (Notif read-fanout). `list_objects` over
  `run_id` via the OQ-E `SetExpr` JOIN (no N+1, no post-filter).
- **Cross-fabric surfacing (facts only):** `project(ref, viewer)` for run/deployment/pipeline unfurls; the
  `IndexSpec` for code/run search; the inbox + chat cards via `humanise`; `replay(scope, since)` →
  `*.snapshot` (sub-artifact-granular reindex-from-source). CI reports; it gates nothing itself.
- **Secrets:** named secrets in the job spec, resolved by an in-boundary broker scoped to the job; OIDC
  short-lived audience-scoped credentials over static keys; **untrusted/fork runs get NO secrets by default**.
- **`PersonalDataHolder`:** `locate/export/rectify/restrict/erase` over run-state/logs/artifacts/caches/
  deployments; identity stored as pseudonym references; the residual third-party free-text basis by reference
  to the one platform posture (X-7, `[OPEN — LEGAL]`).

**Floors named here → follow-ons (see §4 for the full table):** flat DRR → hierarchical scheduler; object-
segment T3 log tier → time-series/wide-column tier; SLSA L1–L2 → hermetic/L3+; single-cell pipelines →
cross-cell-spanning; `myelin ci local` not built; the registry product (commercial).

**Upstream deps:** M3 green (Git produces commits + owns the projection/merge-gate/supersession; Knowledge
exists; **AG-D4 green** from CI-M2 so any untrusted CI step runs); M1 (`list_objects`, ReBAC engine, Storage
tiers, residency); M2 (Refs `project`, Search, Notif `humanise`, Workflow, the firehose).

**Exit gate (CI's contribution to M4 → M5 — quantified, must be green):**
- **CI-T1 / AG-D4 re-confirmed green on the production CI runner image** (the hard GATE, re-run on the CI
  image) — GATE.
- **GIT-D10 / CI-D8 (the X-1 check seam end-to-end):** out-of-order/dup `ci.check.updated` → `run_attempt`-
  monotonic supersession holds (lower attempt dropped, higher supersedes; **1 current row per
  `(commit_oid, context)`**); a fork PR self-greens → **neutral for gating**; maintainer endorses → green;
  doubly-delivered `ci.result` → merge-queue wakes **exactly once**; **0 double-merge** (merge-count == 1) — CI.
- **CI-D1 (crash-recovery / effectively-once):** kill the runner mid-job; kill the control plane mid-run → run
  resumes (replay + `SCHEDULE_AND_RUN_JOB` idempotent re-dispatch on `idem_token`); **0 lost runs, 0
  double-deploys, 0 duplicate artifact publishes** — CI.
- **CI-D4 (supply-chain fail-closed):** floating tag / tampered-unsigned component → digest-pin + sign-verify
  fail closed at plan/run; `ci.supply_chain.verification_failed` emitted; **0 un-pinned / 0 unsigned
  executions** — CI.
- **CI-D5 (reserve/settle parity CI ↔ agent):** exhaust the wallet, start a CI run + an agent `compute` job;
  replay across a pricing change → refuse-start (never interrupt in-flight); **0 starts past exhaustion**;
  wholesale ≠ markup holds — CI.
- **CI-D6 (fork-cannot-poison-trusted-cache):** an `UntrustedFork` run writes the default-branch cache scope →
  the trust-tier/branch-scoped namespace holds structurally; **0 trusted-cache writes from a fork** — CI.
- **CI-D7 (fork-gets-no-secrets):** an adversarial fork run reads protected secrets → `read &
  !is_untrusted_fork` holds; **0 secret reads by a fork-tier run** — CI.
- **CI-D9 (determinism guard):** the `ci.pipeline` body → no clock/RNG/IO outside `WfCtx`; the
  `flow-determinism` lint passes; **replay is bit-identical**; only the journaled `job.done` signal feeds the
  body — CI.
- **CI-D11 (live-log reconnect):** drop the live-tail mid-run, reconnect with `last_seq` → the firehose
  backfills `(last_seq, now]`; **0 log lines lost**; `last_seq` past the window → `resync_required` →
  range-read fallback; scope bounded, never `*` — CI.
- *(Cross-subsystem, joint:)* The consumer subsystems' M4 gate also requires ISS-D12 (Issues' "can't mark Done
  while CI red" reads `CheckStatus`) and CHAT-D7 (a `ci.check.updated` busts the shared per-ref cache,
  live-updating the card) — CI surfaces the facts these consume.

---

### CI-M5 — World-scale hardening + the floor follow-ons + the E2E wedge

**Master band:** M5 (world-scale hardening + floor follow-ons + the cross-subsystem E2E wedge). With CI on one
substrate and the deterministic correctness drills green, prove CI **under world-scale load**, ship the named
floor follow-ons whose triggers have fired, and green CI's slices of the four whole-system E2E scenarios.

**Work — world-scale hardening (the F6 surge family + scheduled scale drills):**
- The **30× CI surge** drill on one tenant; the per-surface shed budget (OQ-K) concrete numbers (bounded
  run-queue per tenant, runners pull-bounded). Tune DRR weights, replenishment cadence, the per-`fair_key`
  starvation histogram threshold against measured load (the open question 07#1).
- The pre-warm buffer sizing function (warm-pool vs arrival rate vs per-VM memory floor), measured per
  (region, label-class) (07#2).
- Residency attestation re-confirmed at cell scale; the cell bulkhead (a fatal fault in one cell unaffects
  others).

**Work — the floor follow-ons (each named in CI-M2/CI-M4; here is its scheduled promotion):**
- **gVisor second backend** behind the same `SandboxBackend` trait + its own escape drill (the
  density/latency-economics trigger, esp. sub-second agent `compute`). The escape gate re-runs on the new
  backend (it is the permanent gate).
- **Time-series/wide-column log tier** promoted from the object-segment T3 floor — **only once volume is
  measured** to outgrow the OLTP-indexed object-segment tier (EI-04 §5: not before measured).
- **Hierarchical scheduler** promoted from flat DRR — only on a measured starvation signal (07#1).
- **Cross-cell-spanning pipelines** (a pipeline whose jobs span cells of a multi-cell tenant) — designed-not-
  built until the OQ-I cross-cell PII-free pointer bridge (12.6) lifts in M5; then CI's runs inherit it.
- **SLSA L3+ / hermetic provenance** — demand-triggered.

**Work — CI's slices of the whole-system E2E wedge (testing-strategy §2, against a full cell with mock
agents):**
- **E2E-1 (PR context pane):** CI emits `ci.check.updated` (build → success, test → failure); the context
  pane shows the live check rows + the jump-to-failure `#step-<n>` anchor; 0 leak.
- **E2E-2 (CI-fail → triage agent → issue → chat → fix-PR — the agent-native flagship):** CI's
  `ci.run.failed` carries *structured* failure (which step, which test, log excerpt — the deliberate triage
  hook); the triage agent's compute runs on CI's runner (AG-D4-gated); the fix-PR's CI goes green; the
  merge-queue wakes on `ci.result` (idempotent on `idem_token`); reserve/settle balanced; merge-count == 1.
- **E2E-3 (spec-to-ship traceability):** CI runs attach `CheckStatus`; a protected-env deploy (HITL-gated)
  ships it; cold-reindex (`replay`/`*.snapshot`) == live; audit tamper detected.

**Upstream deps:** CI-M4 green (the deterministic correctness drills); M5's multi-cell bridge (12.6) for
cross-cell runs; the four E2E scenarios are joint with all subsystems.

**Exit gate (CI's contribution to M5 → M6 — quantified, must be green):**
- **CI-D2 (CI surge / fairness, the F6 family):** 30× CI surge one tenant → interactive lane holds its
  latency budget; batch/CI lane sheds (**429 + Retry-After** honoured by `myelin ci`); **other tenants
  unaffected**; reserve/settle refuses over-budget; killed-runner jobs re-queue **within the lease TTL**, 0
  orphans — SCHED.
- **CI-D3 (erasure-reaches-every-holder):** `erase(subject)` fans to CI → PII in logs/artifacts/caches/run-
  state destroyed (per-subject DEK where isolable; per-tenant fallback) **incl. backups**; structure survives
  for audit; **0 dangling leak** in any unfurl/embed — SCHED.
- **CI-R3 (residency at scale):** an EU-resident tenant's run → claimed **only** by an in-region runner;
  logs/artifacts/caches never leave the region (CDN edge within-EU only); `residency_verify` attests; the
  `residency-pin` lint passes on every CI write — SCHED.
- **CI-D10 (self-hosted runner trust boundary):** a compromised self-hosted runner → the scoped job token
  bounds it to its own tenant's `SelfHosted` jobs; **0 cross-tenant job/secret reads**; attestation failure →
  cannot claim — SCHED.
- **The gVisor second backend (if promoted) re-greens the escape gate** on the new backend — GATE.
- **E2E-1 / E2E-2 / E2E-3 green** (CI's slices, each emitting its named green artifact).

---

### CI-M6 — Dogfooding: Myelin's own CI runs on Myelin CI

**Master band:** M6 (Myelin hosts itself). The cheapest, most honest load generator is the platform's own
development.

**Work:**
- Migrate the Myelin build/test/lint/mutation pipeline onto a Myelin `ci.pipeline`. The twelve architecture
  lints + the mandatory-core mutation gate (`cargo-mutants`) now run **as Myelin CI jobs on every Myelin
  commit** — the ratchet is now self-hosted.
- The every-incident-adds-a-drill loop files a Myelin issue + a reproducing CI drill.
- Drive the real `myelin ci` CLI + the run/log/deploy views for the switch test (the Git OQ-12 / CI switch
  test): could a GitHub-Actions / GitLab-CI user move to Myelin without hitting a wall the old tool didn't
  have? — reached by driving it, not by reading the feature list.

**Upstream deps:** CI-M5 green (world-scale-ready; restore-verify + DSAR fan-out green before real team data —
the team's build data is real tenant data).

**Exit gate (the done-bar):**
- **The Myelin self-hosting CI graph is green** on the platform's own commits (the dogfood loop is live).
- **The CI switch test passes** (driven in a browser/CLI; measured latency + the run/log UX against the GitHub
  Actions anchor) — SCHED.
- No later-band CI gate is red (the truth-up pass confirms every PROVEN CI row rests on a dated green
  artifact, not a doc claim).

---

## 4. Floors and their scheduled follow-ons (name-your-floors — the honest-floor rule, VISION §3)

Every floor is tracked in the gap report with its claimed/proven status and its linked follow-on. The gap
being *invisible* is the only failure (EI-04 §4).

| Floor (ships) | CI band | The full answer (follow-on) | Band | Trigger |
|---|---|---|---|---|
| **One sandbox backend (Firecracker) through the escape drill** | CI-M2 | gVisor second backend behind the same trait + its own drill | CI-M5 | measured density/latency economics (esp. sub-second agent `compute`) |
| **Flat DRR fair-share at claim time** | CI-M4 | A richer hierarchical scheduler | CI-M5/beyond | a measured per-`fair_key` starvation-histogram signal |
| **1–2 `FleetProvider` adapters + self-hosted** | CI-M4 | More EU-provider adapters (adapters, not redesigns) | demand | customer demand |
| **Object-segment T3 log tier + OLTP `(job,step,byte-range)` index** | CI-M4 | Dedicated time-series/wide-column log tier | CI-M5/post | **measured** event volume outgrowing the DB (EI-04 §5) — not before |
| **Per-subject DEK crypto-shred for isolable inline log PII (BUILT)** | CI-M4 | The residual third-party free-text PII basis (per the one platform posture, X-7) | parallel (legal) | DPO/counsel ratification (`[OPEN — LEGAL]`) — structural floor ships regardless |
| **Single-cell pipelines** | CI-M4 | Cross-cell-spanning runs (inherits the cross-cell PII-free bridge, 12.6) | CI-M5 | the OQ-I multi-cell bridge lifts; cross-cell demand |
| **SLSA L1–L2 provenance + SBOM** | CI-M4 | Hermetic / two-party (L3+) provenance | CI-M5/post | customer demand |
| **Component trust model (digest-pin + sign-verify + SLSA)** | CI-M4 | The registry *product* (hosting/discovery) | commercial | commercial-flagged |
| **`myelin ci local` not built** | — | Laptop execution | deferred | a UX-vs-fidelity decision |
| **External-provider checks via `CheckStatus{provider:external}`** | CI-M4 | Richer external-CI integrations | demand | customer demand |

---

## 5. The contracts CI must implement, by milestone (from contract-index.md)

CI **owns** the producer half of some contracts and **implements/consumes** the rest. The table lists every
contract CI touches, CI's role, and the band by which CI must have it landed.

| Contract | Title (short) | CI's role | By band |
|---|---|---|---|
| **8.4** | `ToolHands::exec` = the CI runner's `kind=agent` job; the four uniform guarantees | **owns the runner** (co-defined with Agent) | **CI-M2** |
| **8.1** | `ToolSurface::register_tool` + frozen `requires_approval` defaults (CI deploy/secret = yes) | implements (registers CI tools) | CI-M4 (runner-tool slot M2) |
| **1.1 / 1.2 / 1.3** | `serve(AppSpec)` + three-port + liveness≠readiness | implements (5 logical services) | CI-M2 (runner) → CI-M4 (control plane) |
| **2.1 / 2.2 / 2.9** | `EventEnvelope` + `OutboxTx::emit` + the `ci.*` taxonomy + the new `ci.check.updated`/`ci.result` tokens | **owns** the `ci.*` list | CI-M4 (taxonomy); M2 for runner events |
| **2.5** | `consumer_dedup` (idempotent on `event_id`) | implements (every consumed event) | CI-M4 |
| **2.6** | Reindex-from-source — `replay(scope, since)` → `*.snapshot`, sub-artifact-granular | implements | CI-M4 |
| **3.4** | `EventMatcher` = the frozen `QueryAst` (the config-grammar expression core) | consumes/implements | CI-M4 |
| **3.5** | Firehose resume-cursor transport (`subscribe/resume/scope`) for logs | consumes (live-tail) | CI-M2 (frames) → CI-M4 (UI) |
| **4.2** | `check` with `CaveatContext` (every write/read; field/transition ABAC) | consumes | CI-M2 (runner) → CI-M4 |
| **4.3** | `list_objects` `SetExpr` push-down over `ci_run.run_id` (OQ-E JOIN) | consumes (every run list/search) | CI-M4 |
| **4.4** | `list_subjects` — the HITL approver set for a protected deploy | consumes | CI-M4 |
| **4.7** | `mint_run_token` — per-job attenuated token; self-hosted `SelfHosted`-scoped | consumes | CI-M2 |
| **4.9** | The CI ReBAC fragment (`ci_project/environment/secret/run` + `read & !is_untrusted_fork`) | **declares the fragment** | CI-M4 (engine: M1) |
| **5.1** | `ArtifactRef` — `myelin://<t>/ci/<type>/<id>` (run/deployment/pipeline/runner/artifact) | implements (mints) | CI-M4 |
| **5.6** | `project(ref, viewer)` — the only cross-DB read of a CI artifact | **implements** (REQUIRED) | CI-M4 |
| **5.7** | The `#sub` scheme — CI-owned `step-<n>` + `check-<context>` + `L<a>-L<b>` + tombstone ladder | implements (stable mint) | CI-M4 |
| **5.9** | **The Git↔CI `CheckStatus` seam** — `ci.check.updated` + `ci.result` | **owns the producer half** | **CI-M4** |
| **6.3** | `declare_indexable` (the CI `IndexSpec`) | implements | CI-M4 |
| **6.5** | Code-search input — consume CI-produced SCIP/LSIF (find-usages) | named follow-on | post-CI-M5 |
| **7.3** | `humanise` — every CI status message / `CheckStatus.summary` as `(template_key, args)` | consumes (registers templates) | CI-M4 |
| **9.1 / 9.2 / 9.4** | `DurableExecutor` + `WfCtx` + `SCHEDULE_AND_RUN_JOB` + the `job.done`/`ci.result` signal | implements (the `ci.pipeline` workflow) | CI-M4 (idiom co-defined M2) |
| **9.5** | Workflow↔agent mapping (reserve/settle = the bookends) | implements | CI-M4 |
| **10.1** | `PersonalDataHolder` — `locate/export/rectify/restrict/erase` over CI stores | **implements** (a spicy holder) | CI-M4 (auto-register: M1 harness) |
| **10.9** | The one free-text/immutable erasure posture (residual by reference) | implements by reference | CI-M4 |
| **11.2** | `BlobStore` T2 + trust-tier/branch-scoped cache namespaces + CDN clone class | consumes (artifacts/caches) | CI-M4 |
| **11.4** | Crypto-shred — per-subject DEK for CI log segments | consumes | CI-M4 |
| **11.7** | Reserve/settle cost gate — fronts every run + every `SCHEDULE_AND_RUN_JOB` | consumes (the one metering path) | CI-M2 (runner) → CI-M4 |
| **11.8** | T3 log tier — `(job, step, byte-range)` index | **co-owns** (heaviest consumer) | CI-M4 |
| **12.1 / 12.4** | `(tenant, region)` partition + `residency_verify` (runner/log/artifact/cache region) | consumes/attests | CI-M2 (region pin) → CI-M4 |
| **12.6** | Cross-cell PII-free pointer bridge frame (cross-cell-spanning runs) | inherits (named floor) | CI-M5 |
| **1.6** | The lints — `no-host-exec`, `residency-pin`, `flow-determinism`, `search-requires-acl-filter`, `no-raw-publish` | obeys (CI code gated) | CI-M2 onward |
| **1.11** | The protected-human-lane shed order + the per-surface CI-surge shed budget | implements (CI's budget) | CI-M4 (numbers: CI-M5) |

---

## 6. Digest (the return value)

**Milestones (mapped to master bands):**
- **CI-M2 — The unified sandbox runner + the escape GATE** (master M2). CI's Tier-2 keystone, co-owned with
  the agent fabric: the `SandboxBackend`/Firecracker runner + the four uniform guarantees + the real-kernel
  escape adversarial corpus. **Exit GATE: CI-T1 / AG-D4 = ZERO escapes** — blocks all untrusted code in M3+.
- **CI-M4 — The full subsystem** (master M4, CI first in band). The two green-field cores (the DRR pull-lease
  scheduler + the EU fleet autoscaler), the `ci.pipeline` durable workflow, **the X-1 `CheckStatus` producer
  half closing the seam Git built in M3**, deploys/HITL, supply-chain, metering, the ReBAC fragment, the
  cross-fabric surfacing. Exit: GIT-D10/CI-D8 (X-1 seam, 0 double-merge), CI-D1/D4/D5/D6/D7/D9/D11, CI-T1
  re-green on the prod runner.
- **CI-M5 — World-scale hardening + floor follow-ons + the E2E wedge** (master M5). The 30× surge family
  (CI-D2), residency at scale (CI-R3), erasure fan-out (CI-D3), self-hosted trust boundary (CI-D10); the
  gVisor second backend, time-series log tier, hierarchical scheduler, cross-cell pipelines; CI's slices of
  E2E-1/E2E-2/E2E-3.
- **CI-M6 — Dogfooding** (master M6). Myelin's own build/test/lint/mutation runs as a Myelin CI pipeline; the
  switch test; the self-hosting CI graph green.

**Floors + follow-ons:**
- Firecracker-only backend (CI-M2) → gVisor second backend (CI-M5, density/latency-triggered).
- Flat DRR fair-share (CI-M4) → hierarchical scheduler (measured starvation).
- Object-segment T3 log tier (CI-M4) → time-series/wide-column tier (measured volume).
- Per-subject DEK crypto-shred BUILT (CI-M4) → residual third-party free-text basis ([OPEN — LEGAL], parallel).
- Single-cell pipelines (CI-M4) → cross-cell-spanning runs (CI-M5, OQ-I bridge lifts).
- SLSA L1–L2 (CI-M4) → hermetic/L3+ (demand); component trust model → the registry product (commercial).
- `myelin ci local` not built (deferred); external-provider checks via `CheckStatus{provider:external}`.

**Critical upstream dependencies:**
- **For CI-M2:** the failure-injection harness + `serve` + outbox + the `no-host-exec` lint (M0);
  `mint_run_token` + reserve/settle + KMS (M1); co-developed with the agent fabric (`ToolHands::exec`,
  `EffectApi`, `JobSpec`) + durable workflow (`SCHEDULE_AND_RUN_JOB`, the `job.done` signal) + the firehose
  resume-cursor transport (all M2).
- **For CI-M4:** **Git must already own the `check_status` projection + supersession rule + merge-queue
  workflow (M3)** — CI ships the *producer* half closing what Git built; **AG-D4 green (CI-M2)** so untrusted CI
  steps run; Identity `list_objects` `SetExpr` push-down + the ReBAC engine (M1); Storage T2/T3 + trust-scoped
  cache namespaces + per-subject log DEK; Refs `project` + the `#sub` grammar, Search `declare_indexable`,
  Notif `humanise` (M2); Tenancy partition + `residency_verify` (M1); the `myelin-query` `QueryAst` (M2).
- **The two CI-relevant permanent gates (re-run forever):** **AG-D4 / CI-T1** (every backend/image/kernel
  change) and, transitively, **STOR-D1/STOR-D2** (every change touching a CI store — restore-verify).

**The two hardest things CI owns:** (1) **AG-D4 / CI-T1** — the real-kernel sandbox-escape gate, on the
critical path, front-loaded into M2, blocks all untrusted execution; (2) **X-1 / contract 5.9** — the Git↔CI
check seam, the tightest cross-subsystem contract, producer in CI-M4 closing the consumer Git built in M3,
proven end-to-end by GIT-D10/CI-D8 and the E2E-2 flagship. Everything else CI builds is disciplined
composition of frozen contracts plus its two green-field cores (the scheduler + the EU fleet autoscaler).
