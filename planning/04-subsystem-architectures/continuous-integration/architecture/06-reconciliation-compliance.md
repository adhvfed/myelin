# CI/CD — 06 Reconciliation Compliance (how CI implements the frozen contracts)

> Phase 5-B — CI detailed architecture (rewritten against the reconciled layer). This doc **replaces** the
> Phase-4 "06 — required shared-system change requests": those requests were resolved in Phase 5. This is now
> the record of **how this subsystem IMPLEMENTS the frozen reconciled contracts** — `CheckStatus`,
> `myelin-content` (n/a to CI), `myelin-query`/`QueryAst`, the `list_objects` `SetExpr` Filter, the unified
> `#sub` grammar, the erasure posture, the four uniform sandbox guarantees, `SCHEDULE_AND_RUN_JOB`,
> reserve/settle — plus any **RESIDUAL** request for Phase 6. Every row references the frozen
> [`05/contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md) entry it
> implements. **No drift:** CI builds to the frozen shapes exactly.

---

## 1. What CI implements against the frozen contracts (the compliance map)

| Frozen contract | What CI implements (no drift) | Where |
|---|---|---|
| **5.9 — the Git↔CI `CheckStatus` seam (NEW, X-1)** | CI is the **producer**: emits `ci.check.updated` carrying the frozen `CheckStatus` struct per `(commit_oid, context)`; stamps **`trust_tier`** from run provenance + the `!is_untrusted_fork` ABAC edge; supplies the monotonic **`run_attempt`** (the `check_attempt` counter, doc 01 §3.2); sets `details_ref = #step-<n>`; `summary` as a `(template_key,args)` HumanisedRef; `cost_settled` flips on settle. Emits the **`ci.result`** rollup signal that wakes Git's merge queue. **CI does NOT** own the projection table, decide `required`, recompute trust, endorse forks, or merge — those are Git's. | 03 §1.1/§4; 02 §4 |
| **2.9 — event taxonomy + tokens** | Registers `ci.check.updated` + `ci.result` (+ consumes the `initiative` token where issue-triggered); completes the `ci.*` dotted-name list under the Bus §6 grammar; `ci` subsystem token + `run`/`deployment`/`pipeline`/`runner`/`artifact` type tokens (Refs validates, never re-authors). | 03 §1 |
| **2.1 / 2.2 — `EventEnvelope` + `OutboxTx::emit`** | Every `ci.*` bus event drafted into the per-service `outbox` in the same tx as the state change; no `publish_now` (the `no-raw-publish` lint); references-not-payloads (the check fact + log pointers, never log bytes). | 03 §3 |
| **2.6 — reindex-from-source `replay`** | `ci.run.snapshot` / `ci.deployment.snapshot` / `ci.pipeline.snapshot` through the live consumer path; **sub-artifact-granular** (one-run scope); also the post-restore re-erasure path. | 03 §7.3 |
| **3.4 — `EventMatcher` = the frozen `QueryAst`** | CI trigger predicates (`on: pull_request`/`issue.transitioned` with path/branch/status filters) compile to the `QueryAst` core; **no CEL, no CI-specific trigger language**. The config-grammar expression language is the same `QueryAst`. | 02 §1/§7.4; 05 HP-3 |
| **3.5 — firehose `subscribe/resume/scope` (OQ-J)** | `ci.log.appended` frames ride the firehose (CI = the heaviest producer); the live-log view `subscribe(stream, scope = run:<id>/job:<id>)` and `resume(stream, scope, last_seq)` — **loses zero lines on reconnect**, `resync_required` → range-read; scope is **bounded**, never `*`. Durable bus carries only `ci.log.available` pointers. | 02 §7.1; 04 §1 |
| **4.1 / 4.7 — machine-identity + `mint_run_token`** | Per-job attenuated token mintable **mid-workflow on resume** (S-11); self-hosted runner token **scoped to one tenant's `SelfHosted` jobs**; deploy-key/SSH/PAT/per-job token → Principal at the runner/CLI entrypoints. | 02 §5; 03 §5.1 |
| **4.2 — `check` + `CaveatContext`** | `Id.check` at every write/read entrypoint, fail-closed; field/transition ABAC (e.g. secret-value read) as a `CaveatContext` check at `check`-time, **off** the hot `list_objects` path. | 03 §5.1 |
| **4.3 — `list_objects` `SetExpr` push-down (OQ-E)** | The run list / "all runs" / release-readiness / CI search lower `Filter{set_expr}` to a **JOIN against the per-tenant authz reverse index over `ci_run.run_id`** (`ColRef{table:"ci_run", column:"run_id"}`) — **no N+1, no post-filter** (the `search-requires-acl-filter` lint). | 03 §5.1 |
| **4.4 — `list_subjects`** | Resolves the HITL **approver set** for a protected deploy (`list_subjects(env, approve)`); `watcher`-relation read-fanout for Notif served by the same reverse index. | 03 §5.1/§5.2 |
| **4.8 — pseudonym grammar** | Actors stored as `<pseudonym>@<tenant>.noreply` references; erasure via `resolve_pseudonym`/`erase`. | 01 §3.1; 03 §6 |
| **4.9 — CI ReBAC namespace fragment** | The frozen `ci_project / ci_environment / ci_secret / ci_run` fragment + the **`read & !is_untrusted_fork`** ABAC edge + the `approver` `list_subjects` target + the `watcher` relation. | 03 §5.2 |
| **5.1 / 5.7 — `ArtifactRef` + unified `#sub` grammar (X-4/OQ-D)** | `myelin://<t>/ci/<type>/<id>[#sub]`; the CI-owned `#sub` kinds **`step-<n>`** + **`check-<context>`** (+ `L<a>-L<b>`); stable opaque ids across retries; Refs stores full sub-URN + stripped root; the 4-step tombstone ladder degrades broken sub-anchors to the parent run. | 03 §7.1 |
| **5.2 / 5.6 — `resolve` + `project`** | `project(ref, viewer)` is the only cross-subsystem read of a CI artifact (per-viewer, pre-permission-checked); backs unfurls/embeds/inbox/search; `Display` mode = the humanisation projection. | 03 §7.2 |
| **6.3 — `declare_indexable`** | The CI `IndexSpec` (ft/struct/semantic fields, `acl_object_type: ci_run`); Search conjoins the OQ-E `Filter` before scoring; restriction flag honoured. SCIP/LSIF "find usages" is a named follow-on (6.5). | 03 §7.4 |
| **7.3 — `humanise` (the ONE templating surface)** | Every CI status summary, agent-authored card/message registers into the NOTIF-1 ICU template registry — `(template_key, args)` + routable `ArtifactRef`s, **never** a raw string or a frontend string map. The `CheckStatus.summary` is a HumanisedRef. | 03 §1.1; 04 §3 |
| **7.5 / 7.6 — notif rules + escalation** | CI registers its `define_notif_rule` set (failure-on-my-work / deploy-awaiting-me / quota-alert); deploy escalation rides the frozen escalation-chain shape on the timer wheel. | 04 §1 |
| **8.1 — `ToolDef` + frozen `requires_approval` defaults (X-6)** | CI's tool set registered into the one `ToolSurface`, MCP-exposable, with the **frozen** defaults (deploy/secret = yes; reversible/cheap = no). | 04 §3 |
| **8.4 — `ToolHands::exec` + the four uniform guarantees (X-6)** | `ToolHands::exec` **is** the CI runner's `kind=agent` job on the ONE unified sandbox; the four guarantees (cost gate, per-run-token attribution, HITL withhold, isolation floor + drill) inherited by construction; dispatched via `SCHEDULE_AND_RUN_JOB`. CI owns the runner + the **real-kernel escape drill that gates ALL agent execution**. | 02 §5; 05 HP-5/HP-9 |
| **9.1 / 9.2 / 9.4 — `SCHEDULE_AND_RUN_JOB` + per-effect `idem_key` (OQ-F)** | The pipeline-as-workflow dispatch uses the frozen long-park-completed-by-signal idiom (`idem_token` minted at the workflow, `job.done` signal idempotent on it); batch deploy-approval cards use the per-effect `idem_key` (`card_id:<effect_idx>`). | 02 §3.3; 04 §3 |
| **9.3 / 9.4 — timers + durable HITL signal** | Step/queue/deploy SLA timers on the wheel; multi-day protected-env deploy gate via `wait_for_signal("approval", …)` holding no runtime. | 02 §3.1 |
| **10.1 — `PersonalDataHolder`** | `locate/export/rectify/restrict/erase` over run-state/logs/artifacts/caches/deployments; harness auto-registers every store; `restrict` suppresses index/agent/analytics/notif. | 03 §6 |
| **10.9 — the ONE erasure posture (X-7), by reference** | CI does **not** restate a CI-local free-text-PII residual statement; it instantiates the platform posture by reference ("the residual is handled per `05 §X-7`"). Structural floor (per-subject DEK + pseudonym shred + `restrict`) ships regardless. | 03 §6 |
| **11.1 / 11.2 / 11.3 / 11.4 / 11.6 / 11.8 — Storage** | OLTP (per-service, RLS); T2 `BlobStore` with **trust-tier/branch-scoped cache namespaces** (C4) + the **within-EU CDN clone/bundle class** (C3) consumed for clone acceleration; KMS hierarchy; **per-subject DEK for isolable inline log PII** (C1); OLAP honours the restriction flag (11.6); the frozen **T3 `(job,step,byte-range)` index** (11.8) resolves `CheckStatus.details_ref`. | 01 §3.5/§3.6; 02 §7 |
| **11.7 — reserve/settle (the one metering path)** | Reserve at workflow start **and at each `SCHEDULE_AND_RUN_JOB` dispatch**; settle on `job.done`; never interrupt in-flight; resource-seconds meter; wholesale ≠ markup; same wallet as agent runs. | 02 §6/§8 |
| **12.2 / 12.4 — placement + residency** | In-cell hot path; `discover`/`placement_of` PII-free routing only; `residency_verify` covers the runner pool + log/artifact/cache region (the no-global-pool attestation). | 00 §5; 01 §4 |
| **1.x — service shell + lints** | `serve(AppSpec)`; three-surface topology; liveness≠readiness; the lints (`no-cross-db`, `no-raw-publish`, `tenant-predicate`, `no-host-exec`, `forward-only-migration`, `residency-pin`, `search-requires-acl-filter`, `no-untagged-personal-data`, `flow-determinism`). Hot-table declaration: `job_queue`, `log_segment`, `cost_event`, `check_attempt`. | 00 §4; 01 §3 |

## 2. The reconciliation deltas CI absorbed (vs the Phase-4 first pass)

The condensed list (full detail in 00 §0):

- **Δ1–Δ3 (X-1):** the Git seam is now `ci.check.updated` carrying the frozen `CheckStatus` + the `ci.result`
  rollup signal; `run_attempt` is the monotonic supersession key; `trust_tier` is stamped by CI and gated by
  Git (fork success = neutral until endorsed). Phase-4's `ci.status.updated`/`ci.run.passed/failed` are
  superseded.
- **Δ4 (OQ-F):** `SCHEDULE_AND_RUN_JOB` is a frozen shared idiom (`job.done` signal, workflow-minted
  `idem_token`); the merge queue uses `ci.result`; batch approvals use the per-effect `idem_key`.
- **Δ5 (OQ-D / 11.8):** `details_ref = #step-<n>` resolves through the frozen `(job,step,byte-range)` index.
- **Δ6 (OQ-E):** `list_objects` over `run_id` is the frozen `SetExpr` JOIN push-down.
- **Δ7 (Storage C1 / 11.4 + X-7):** per-subject DEK for isolable inline log PII is **built**; the residual is
  by reference to the one posture.
- **Δ8 (X-7 / 10.9):** the erasure residual is instantiated by reference, not restated.
- **Δ9 (X-6):** the four uniform sandbox guarantees + the frozen `requires_approval` defaults table.
- **Δ10 (Storage C3/C4):** trust-scoped cache namespaces + the within-EU CDN clone class are NEW frozen
  contracts CI consumes.
- **Δ11 (OQ-J):** the live-log view rides the frozen resume-cursor protocol.
- **Δ12 (2.9):** the two `ci.*` tokens + `initiative` are registered.

## 3. RESIDUAL requests for Phase 6 (nothing is blocking — these are build-phase items)

Reconciliation resolved every Phase-4 cross-subsystem request; the residuals are **build-phase tasks**, not
contract changes:

| # | Residual | Owner | Status |
|---|---|---|---|
| **R-1** | **The full AG-D4 adversarial escape-drill corpus + the green-attestation artifact format.** CI enumerated the obligation; the concrete exploit set is built/executed in Phase 6 (the gating milestone). | CI build | `[OPEN → P6]` |
| **R-2** | **The resource-second → Commercial credit/price mapping + the immutable-pricing-history guarantee.** The unit is CI's; the markup table + replay-stability is Commercial's. | CI + Commercial (C-1) | named follow-on |
| **R-3** | **SCIP/LSIF "find usages" index input** (CI produces the artifact; Search consumes it). | CI + Search (6.5) | named follow-on, gap report |
| **R-4** | **Per-subject DEK granularity for the *non-isolable* residual third-party log PII.** The isolable case ships built; the residual third-party basis is `[OPEN — LEGAL]` per the one posture (X-7). | GDPR/DPO + counsel | `[OPEN — LEGAL]` |
| **R-5** | **Build-data-as-LLM-training lawful basis (AG-8); CD-as-PaaS product scope (PR-5).** Foreclosed/flagged by default (OQ-H); no platform code path feeds tenant build data to training; CD-as-PaaS is a Commercial scope question, not an engineering blocker. | Counsel + Commercial | `[OPEN — LEGAL]` / flagged |
| **R-6** | **gVisor second-backend promotion trigger** (the measured density/latency economics for sub-second agent `compute`). | CI build | measured-not-predicted |

## 4. Names & units cross-check (the reconciliation anchors, unchanged)

CI aligns to the canonical anchors (contract index §14): **timestamps RFC-3339 UTC; costs integer minor-units
(never floats); TTLs/lease-windows/timers in seconds; resilient-client timeouts in milliseconds;
`pii_key_ref = kms://<tenant>/<dek-epoch>/<class>` with `<class> ∈ {tenant, subject:<id>, blob}`**. The
`subject:<id>` class is the per-subject log DEK (Storage C1). CI emits the `ci` subsystem token + the
`run`/`deployment`/`pipeline`/`runner`/`artifact` type tokens under the Bus §6.2 table; Refs validates, never
re-authors.
