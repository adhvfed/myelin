//! # `myelin-flow` — the durable-workflow substrate: the six-table data model (P-FLOW-01 → P-197, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §3 (the data model:
//! `workflow_run`, `wf_history`, `wf_timer`, `wf_signal`, `wf_activity_attempt`, `wf_definition` —
//! carried verbatim from Phase-3 §3), §2 (the BUILD/DBOS-class, Postgres-embedded decision, ADR-09
//! — NO new datastore).
//!
//! **Contract-index cluster:** 9 — Durable-Workflow (`myelin-flow`). This crate's data model BACKS
//! the 9.1/9.6 surface (the durable-execution API), but those trait surfaces ship later
//! (P-FLOW-04..08). Consumed/wired here: 11.1 OLTP/RLS, 12.1 `(tenant, region)`, 1.5 forward-only
//! migrations, 10.2 the `#[personal_data]` classification.
//!
//! ## What this prompt (P-FLOW-01) ships — the SCHEMA ONLY
//!
//! 1. **The six-table data model** (the [`migrations`] module): forward-only, `(tenant, region)`-
//!    first, RLS-scoped migrations for `workflow_run` / `wf_history` / `wf_timer` / `wf_signal` /
//!    `wf_activity_attempt` (the five tenant tables) + `wf_definition` (the global, NON-tenant
//!    definition registry). Built through [`myelin_substrate::MigrationRunner`] so the boot-time
//!    runner applies them forward-only AND the `forward-only-migration` lint reads them at
//!    source-scan.
//!
//! 2. **The row TYPE carriers** (the [`schema`] module): the six row types with the
//!    `#[derive(PersonalData)]` classify-derive + the `#[personal_data(...)]` tags (contract 10.2)
//!    on the ONLY PII-bearing columns — the rare inline-PII `result_key_ref` / `payload_key_ref`
//!    envelope key refs (the crypto-shred levers). The engine is references-not-payloads, so almost
//!    every column is PII-free by construction.
//!
//! ## FLOORS named (this is explicitly NOT a working engine)
//!
//! - **The AppSpec service shell** (boot → migrate → outbox relay → empty consumer slot → three
//!   ports → graceful drain, liveness≠readiness) → **LANDED at P-FLOW-02** (P-198), see [`app`]
//!   plus `src/main.rs`. [`app::flow_app_spec`] assembles the [`AppSpec`](myelin_substrate::AppSpec)
//!   the harness wires; the `myelin-flow` binary hands it to `serve`. The `consumers` slot is the
//!   EMPTY seam (the replay engine plus the signal/timer consumers are P-FLOW-04..05/09/13).
//! - **The `PersonalDataHolder` auto-registration** over `workflow_run` / `wf_history` / `wf_signal`
//!   (the structural references-not-payloads half) → **LANDED at P-FLOW-03** (P-201), see [`holder`]
//!   ([`WfHistoryHolder`]: `locate`/`export` real over the journal, `erase` structurally wired; the
//!   flow store classifies to H8 — the §5.5 references-not-payloads reconcile). **The per-subject-DEK
//!   crypto-shred reach (the P-FLOW-03 named floor) is now CLOSED at P-FLOW-24 (FLOW-D9), see
//!   [`crypto_shred`]** ([`WfCryptoShred`]: erasing a subject DESTROYS their per-subject DEK over the
//!   ONE [`myelin_storage::kms::KmsEngine`] so the inline-PII `result_key_ref`/`payload_key_ref`
//!   history/signal rows are unrecoverable incl. backups, WITHOUT rewriting the journal — structure
//!   preserved (replay still works, the PII is a tombstone) + the crypto-shred-lag telemetry, contract
//!   1.8; wired into [`holder::WfHistoryHolder::with_crypto_shred`]; the FLOW-D9 drill is
//!   `tests/drills_flow_d9_crypto_shred.rs`. **Restore-verify to a consistent point is now LANDED at
//!   P-FLOW-25** (FLOW-D10, M5), see [`restore_verify`] ([`WfRestoreVerify`]: restore the myelin-flow
//!   run-store/journal/outbox to a consistent point `T` (the event-log offset, contract 11.5) → assert
//!   in-flight runs RESUME via the P-FLOW-05 replay short-circuit with 0 re-executed side effect, NO run
//!   points at a vanished result (every retained `wf_history` result ref is in the restored set), and
//!   store↔outbox offsets RECONCILE at one `T`; the dated consistent-point green artifact +
//!   restore-verify telemetry, contract 1.8; the FLOW-D10 drill is
//!   `tests/drills_flow_d10_restore_verify.rs`)).
//! - **The algorithms**: WfCtx + journal/outbox co-commit (**P-FLOW-04**, FLOW-D5) — **LANDED**,
//!   see [`wfctx`] ([`WfCtx`]: `activity`/`now`/`rand`/`emit` + the single-txn co-commit; the
//!   FLOW-D5 drill is `tests/drills_flow_d5_cocommit.rs`); deterministic
//!   replay + lease dispatch + crash recovery (**P-FLOW-05**, FLOW-D1) — **LANDED**, see [`engine`]
//!   ([`drive`] short-circuits the journaled prefix with 0 re-execution + [`RunStore`] lease
//!   re-lease + [`FlowDispatcher`] the consumer-seam worker loop + [`FlowTelemetry`] the replay-rate
//!   + 0-double-effect signals; the FLOW-D1 drill is `tests/drills_flow_d1_replay.rs`, the live-PG
//!     apply `tests/integration_flow_replay.rs`); the DurableExecutor start/describe/cancel + the
//!     engine telemetry set (**P-FLOW-06**) — **LANDED**, see [`executor`] ([`FlowExecutor`]:
//!     `start` idempotent-on-`idem_key` seeds a runnable run the dispatcher drives, `describe` reads
//!     the [`RunStatus`], `cancel` terminates; the §1.8 activity-queue/retry/dead-letter telemetry
//!     leg on [`FlowTelemetry`]; the 9.1 CDC pair is `tests/cdc_9_1_executor.rs`); the
//!     replay-divergence guard (**P-FLOW-07**, FLOW-D2); the flow-determinism lint fixtures
//!     (**P-FLOW-08**); durable signals (**P-FLOW-09**) — **LANDED**, see [`executor`]
//!     ([`DurableExecutor::signal`] buffers into `wf_signal` idempotently via `ON CONFLICT (tenant,
//!     run_id, signal_name, idem_key) DO NOTHING` so a double-delivery wakes the workflow once) +
//!     [`signal_consumer`] ([`FlowSignalConsumer`] the bus side wired into the consumer slot via
//!     [`flow_signal_consumer_reg`]) + the signal-buffer-depth telemetry on [`FlowTelemetry`]; the
//!     9.1 signal CDC pair is `tests/cdc_9_1_signal.rs`, the live-PG ON CONFLICT apply
//!     `tests/integration_flow_signal.rs`; the consuming wait (`wait_for_signal`) is **P-FLOW-11**);
//!     the per-effect `idem_key`-construction rule for batch/partial HITL approval (**P-FLOW-10**)
//!     — **LANDED**, see [`approval`] ([`per_effect_idem_key`] the FROZEN §6.4 rule — `card_id`
//!     single / `card_id:effect_idx` multi — over the P-FLOW-09 signal delivery + [`apply_approved_effects`]
//!     the gated loop: each approved effect → exactly one `EffectApi::apply`, a declined effect is
//!     WITHHELD (`Denied`, 0 mutation, AG-8), a double-click on "approve all" re-sends the same keys
//!     → ON CONFLICT DO NOTHING → 0 double-apply; the 9.1 per-effect CDC pair vs the Agent Fabric
//!     `EffectApi` consumer is `tests/cdc_9_1_per_effect.rs`; the F-4-extended drill across a
//!     restart+deploy lands at **P-FLOW-12** and at **CHAT-D10** M4); the consuming WAIT
//!     (`wait_for_signal` + the multi-day HITL approval-card round-trip, **P-FLOW-11**, FLOW-D4) —
//!     **LANDED**, see [`wfctx`] ([`WfCtx::wait_for_signal`]: parks the run `state=waiting` holding NO
//!     runtime on an absent named signal (journals `signal_waited`); a buffered signal RESUMES + is
//!     CONSUMED exactly once via [`SignalStore::consume`] (stamp `consumed_seq` WHERE NULL, journals
//!     `signal_received`); the optional timeout arms the P-FLOW-13 durable timer + returns
//!     [`WaitOutcome::TimedOut`] when the deadline passes; replay short-circuits the journaled
//!     `signal_received` to the SAME signal — consume-exactly-once across a re-drive) + the HITL
//!     approval-card round-trip [`request_approval_and_wait`] in [`approval`] (emit
//!     `agent.approval.requested` via the outbox ONCE → wait → resume/withhold/timeout; the card
//!     UX/visual data model is Chat+Agent-Fabric product work, OQ #1, NOT this engine) + the
//!     oldest-unconsumed-wait-age telemetry on [`FlowTelemetry`] (§5.4); the FLOW-D4 drill across a
//!     restart+deploy with a days-later double-click is `tests/drills_flow_d4_multiday_hitl.rs`, the
//!     9.4 wait CDC pair is `tests/cdc_9_4_wait.rs`, the live-PG consume-once apply is
//!     `tests/integration_flow_wait.rs`; the `ci.result`/`job.done` long-park PRODUCER wiring is
//!     **P-FLOW-14/15**); durable timers
//!     (**P-FLOW-13**, FLOW-D3) — **LANDED**, see [`timer`] ([`TimerStore`] the minute-bucket wheel:
//!     `arm` (idempotent on the deterministic `timer_id`) + the bucketed due-scan ([`TimerStore::scan_due`]:
//!     `bucket <= now AND NOT fired`, `FOR UPDATE SKIP LOCKED`, far-future NEVER scanned — the SC-11
//!     partial-index move) + effectively-once `fire` (set fired + idempotent `timer_fired` journal +
//!     wake the parked run); [`WfCtx::sleep_until`]/[`WfCtx::sleep_for`] arm a `wf_timer` row + park the
//!     run holding no runtime; the [`TimerWheel`] scan loop wired into the consumer seam alongside the
//!     dispatcher ([`app::flow_app_spec_with_engine`]); the timer-wheel-lag telemetry on
//!     [`FlowTelemetry`] (the SC-11 health signal); the FLOW-D3-floor drill at 100k+ is
//!     `tests/drills_flow_d3_timer_wheel.rs`, the live-PG bucketed-scan apply
//!     `tests/integration_flow_timer.rs`; the cheap disarm/re-arm half is **P-FLOW-14**, the 1M+
//!     seven-figure cell-scale run is **P-FLOW-24**); the **`SCHEDULE_AND_RUN_JOB` long-park idiom**
//!     (**P-FLOW-15**, §4.9) — **LANDED**, see [`job`] ([`WfCtx::schedule_and_run_job`]: dispatch-and-
//!     return as a journaled activity minting `idem_token` DETERMINISTIC on the dispatch `command_id`
//!     ([`job_idem_token`]) + stamping it on the [`JobSpec`]`{kind: ci|agent}` handed to the unified
//!     runner ([`JobRunner::dispatch`] = `ToolHands::exec`, contract 8.4 CONSUMED) + journaling
//!     `activity_completed{job_dispatched, idem_token}` + RETURNING; then park on
//!     `wait_for_signal("job.done", idem_key=idem_token)` with a timeout-timer bounding a vanished
//!     runner; a double-delivered `job.done` wakes the run ONCE (the `wf_signal` PK dedup); replay
//!     re-derives the SAME token + short-circuits dispatch+wait — consume-exactly-once. The dispatch
//!     into the runner is GATED by AG-D4 (Agent-Fabric/CI-owned, `04-sandbox-AG-D4.md` — NO untrusted
//!     code runs until green); the 9.2/9.4 CDC pair is `tests/cdc_9_2_schedule_and_run_job.rs`.
//!     **NAMED FLOORS:** reserve/settle bookend **P-FLOW-16**) — the rest land later. An empty journal
//!     is not a working engine.
//! - **The merge-queue durable workflow body** (the durable-execution half of the X-1 seam, §6.5)
//!   — **LANDED at P-FLOW-19** (P-215), see [`merge_queue`] ([`WfCtx::run_merge_attempt`]: ONE merge
//!   attempt per queued PR — compute the speculative merge commit + dispatch the required CI under a
//!   DETERMINISTIC `merge_attempt_id` ([`merge_attempt_id`]) reserving budget at dispatch (no balance
//!   → no CI), park on `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` holding NO runtime
//!   with a timeout-timer bounding a vanished CI run; on a `success` rollup for ALL required contexts
//!   → merge ([`MergePerformer`]) + emit `git.pr.merged` via the outbox + settle → [`MergeOutcome::
//!   Merged`]; on `failure` / a missing required context / a failed merge → dequeue with a HUMANISED
//!   reason (contract 7.3, [`humanise_dequeue_reason`]) → [`MergeOutcome::Dequeued`]; a vanished CI
//!   run's timeout → [`MergeOutcome::TimedOut`]). A double-delivered `ci.result` wakes the run ONCE
//!   (the `wf_signal` PK dedup) → 0 double-merge; replay re-derives the SAME id + short-circuits
//!   dispatch+wait+merge. The body imports the CI-owned [`CiResult`](myelin_events::check_seam::
//!   CiResult) / `ci.result` data shape (contract 5.9) from `myelin-events` — it does NOT redefine it
//!   ([`encode_ci_result`]/[`decode_ci_result`] are the references-not-payloads codec over that
//!   shape). The CI dispatch into [`CiDispatcher`] is GATED by AG-D4 (Agent-Fabric/CI-owned —
//!   `04-sandbox-AG-D4.md`, NO untrusted code until green). The merge-queue-in-isolation drill (0
//!   double-merge, 1 wake/attempt, timeout-bounded) is `tests/drills_flow_merge_queue.rs`; the 5.9
//!   merge-queue CONSUMER CDC pair (vs CI's `ci.result` producer shape) is
//!   `tests/cdc_5_9_merge_queue.rs`. The body was built + drilled IN ISOLATION against a MOCK
//!   [`MockCiResultProducer`] (P-FLOW-19, the floor). **P-FLOW-23 (P-346, M4) CLOSED that floor —
//!   the X-1 seam END-TO-END**: [`RealCiResultProducer`] replaces the mock with CI's REAL producer
//!   wiring (the merge queue wakes on the rollup CI DERIVES from per-context `ci.check.updated` facts
//!   through `myelin_events::check_seam::{CheckSeamOrder, rollup_ci_result}` + Git's `run_attempt`
//!   supersession, keyed idempotently on the `merge_attempt_id`). GIT-D10 / CI-D8 green (0
//!   double-merge, merge-count == 1/attempt, 0 spurious unblock, across re-delivery + restart) —
//!   `tests/drills_flow_d10_x1_seam_e2e.rs`; the consumer half is now paired with CI's real provider
//!   half in `tests/cdc_5_9_merge_queue.rs`. (M2.4 — and the whole M2 engine surface for myelin-flow
//!   — is covered across P-FLOW-13..19.)
//! - **The §6.6 resumable maintenance activities + the history-rewrite invalidation fan-out** (the M3
//!   support Git's GC / repack / bundle-gen / history-rewrite ride) → **LANDED at P-FLOW-20** (P-265),
//!   see [`maintenance`] ([`WfCtx::run_maintenance`]: a maintenance op as a sequence of journaled
//!   activities so a crash mid-repack replays to the un-journaled step §4.1 with 0 re-executed side
//!   effect; [`WfCtx::run_heavy_maintenance`]: the heavy ops ride the `SCHEDULE_AND_RUN_JOB` long-park;
//!   [`WfCtx::run_history_rewrite_invalidation`]: the fan-out over the trust-scoped cache namespaces
//!   [`CacheNamespace`] — Fork/Mirror/CloneBundle, contract 11.2 — as a journaled sequence replaying
//!   FROM the last journaled step). CONSUMES contract 10.6 (the history-rewrite audited op — GDPR/
//!   Audit-owned) + 11.2 (the trust-scoped cache namespaces — Storage-owned). NO new engine primitive
//!   (the application of the §4.4 activity model). NO new FLOW drill is owed in M3 (roadmap §2 M3 —
//!   GIT-D9 is Git's gate); the gate artifact is the crash-mid-repack-resumes-with-no-side-effect +
//!   the fan-out-replays-from-last-step drill `tests/drills_flow_maintenance.rs`. **NAMED FLOORS:** the
//!   Git GC/repack/bundle-gen/history-rewrite CALL SITES are co-built in Git's M3 prompts (GIT-D9).
//! - **The §6.6 cheap SLA-timer re-arm CONFIRMED under the Git/Issues + Trigger call sites + the
//!   merge-queue holds-no-runtime re-green** (the M3 confirmation pass) → **LANDED at P-FLOW-21**
//!   (P-266), see [`timer::sla`] ([`SlaTimerCall`]: the ONE documented call-site helper both the Issues
//!   SLA-breach deadline ([`sla_timer_id`]) and the Event-Bus stateful Trigger's `stale_after`
//!   ([`trigger_stale_timer_id`], contract 3.3) call — a re-arm at the call boundary is a SINGLE row
//!   update ([`SlaTimerCall::re_arm`], the IDENTICAL path for both producers, no ad-hoc key, no second
//!   code path), a disarm is one cheap row op ([`SlaTimerCall::disarm`])). NO new engine primitive (the
//!   M3 confirmation that the P-FLOW-14 cheap row op is what the REAL call sites hit). NO new FLOW drill
//!   is owed in M3 (roadmap §2 M3 — GIT-D9 is Git's gate); the gate artifacts are the
//!   re-arm-is-row-update-at-the-call-boundary CDC pair `tests/cdc_9_3_disarm_rearm.rs` (now routed
//!   through the helper, not ad-hoc keys) + the P-FLOW-19 merge-queue-in-isolation drill RE-GREEN
//!   (holds-no-runtime across the wait, 0 double-merge) `tests/drills_flow_merge_queue.rs`. **NAMED
//!   FLOOR:** the X-1 seam END-TO-END against CI's real `ci.result` producer is **P-FLOW-22** (M4).
//! - **The §6.2 loop-safety enforcement** (the causal-depth ceiling + the shared-root tripwire + the
//!   bounded activity pool — *an adversarial workflow→event→workflow loop is dropped/parked, NEVER
//!   forked*) → **LANDED at P-FLOW-18** (P-214), see [`loopsafety`] ([`CausalGuard`]: `admit_child`
//!   gates a would-be child start against the causal-depth [`CEILING`] (the in-engine `workflow_run.
//!   depth` bound, §3.1) THEN the shared-root tripwire ([`SHARED_ROOT_WINDOW_CAP`] same-root starts in
//!   the window → trip); `admit_activity`/`release_activity` bound the concurrent-activity pool
//!   ([`ACTIVITY_POOL_CAP`]); every refusal is a [`LoopVerdict::Drop`]/[`LoopVerdict::Park`] — there is
//!   NO `Fork` variant). The causal-depth-histogram + 0-fork telemetry are on [`FlowTelemetry`]
//!   ([`FlowTelemetry::observe_causal_depth`]/[`FlowTelemetry::causal_depth_max`]/`depth_ceiling_hits`/
//!   `shared_root_tripwire_firings`/`activity_pool_sheds`/`fork_count`, §5.4 / contract 1.8). The
//!   FLOW-D7 drill (the adversarial loop on the M0 failure-injection harness, asserting causal-depth
//!   `<=` ceiling + 0 fork) is `tests/drills_flow_d7_loop_safety.rs`; the 9.2 loop-safety CDC pair is
//!   `tests/cdc_9_2_loop_safety.rs`. **NAMED FLOOR:** the bus's dispatch-tier shared-root tripwire
//!   COUNTER (the cross-subsystem mirror, event-bus §4.7) is EB-23 (P-143) — this is the in-engine
//!   half the `workflow_run.depth` reads.
//! - **The mid-workflow `mint_run_token` re-mint on resume** (token life == activity life, contract
//!   4.7 CONSUMED, §6.2) → **LANDED at P-FLOW-17** (P-213), see [`remint`]
//!   ([`WfCtx::remint_on_resume`]: a resume across a multi-day wait re-mints a fresh SHORT-LIVED
//!   ATTENUATED per-run token via the contract-4.7 [`RunTokenMinter`] seam — token life == activity
//!   life, NOT the days-long workflow life; the workflow holds no long-lived privileged token across a
//!   wait. Wired into the resume legs of [`WfCtx::wait_for_signal`] (the HITL approval resume) +
//!   [`WfCtx::schedule_and_run_job`] (the long-park `job.done` resume, through the wait). The 4.7 CDC
//!   pair vs Identity's `IdentityService::mint_run_token` provider is `tests/cdc_4_7_remint.rs`. **NAMED
//!   FLOOR:** the `mint_run_token` BODY is Identity's (P-ID-18, M1); the E2E-2 spine re-mint assertion
//!   is now **LANDED at P-FLOW-28** (the global ledger P-477 — the §6.2 cross-ref to "P-FLOW-27"
//!   pre-dated the E2E split; the E2E-2 spine that asserts the re-mint end-to-end across the whole
//!   multi-day HITL + long-park flow is the P-FLOW-28 flagship, `tests/drills_flow_e2e2_spine.rs`).
//! - **The E2E-2 durable-workflow + HITL SPINE** (the agent-native flagship — myelin-flow's role in the
//!   whole-system E2E-2 wedge) → **LANDED at P-FLOW-28** (P-477, M5), see
//!   `tests/drills_flow_e2e2_spine.rs`. The scenario chains the mutations CI-fail → triage agent
//!   workflow → issue (no approval) → HITL `git.merge` gate (`request_approval_and_wait` parks,
//!   `state=waiting` holding no runtime — **0 mutation before approval**, AG-8) → KILL the Agent +
//!   Workflow worker mid-`ack_window` → DAYS-later double-click approve (`ON CONFLICT DO NOTHING` →
//!   buffered=1, 1 wake) → RESUME (FLOW-D4) → RE-MINT the run token on resume (contract 4.7 — wired
//!   through the new [`FlowDispatcher::with_run_identity`] so the dispatcher mints from the run's agent
//!   identity, the production shape) → consume the approval EXACTLY ONCE → the merge applies EXACTLY
//!   ONCE (FLOW-D1, merge-count == 1, 0 double-effect across the kill) → the fix-PR's CI goes green →
//!   the merge-queue workflow wakes on `ci.result` IDEMPOTENTLY (X-1, via [`RealCiResultProducer`], a
//!   doubly-delivered rollup → 1 wake → merge-count == 1) → reserve/settle BALANCED across the WHOLE
//!   run (the new [`FlowDispatcher::with_budget`] meters every spend-bearing dispatch into the ONE
//!   wallet — settle-count == completed-dispatch-count, 0 rejects, 0 in-flight interrupts, the wallet
//!   conserved, contract 11.7/9.5). Contracts exercised: 9.1/9.4 (signal + wait), 4.7 (re-mint), 5.9
//!   (the merge-queue wake), 9.5/11.7 (reserve/settle parity). The dated SCHED green artifact is the
//!   run trace + the HITL withhold→approve→apply ledger + reserve/settle parity + merge-count == 1.
//!   **CROSS-SUBSYSTEM FACES recorded as their owners':** the real agent plan loop (Agent Fabric's E2E
//!   leg), the Issues row (Issues' leg), the Notif approval-card RENDER (P-471 Notif/Chat's leg — the
//!   `agent.approval.requested` emit is real here; the card UX is theirs), Git's real merge (Git's leg),
//!   the Identity `mint_run_token` BODY (P-ID-18 — a recording minter fixture here proves the engine
//!   CALLS the surface). This prompt owns ONLY the durable-workflow + HITL spine. (M5 for myelin-flow is
//!   now covered across P-FLOW-24..28.)
//!
//! There is **no mandatory-core algorithm module** here (it is the schema + frozen type shapes), so
//! there is no mutation-score floor on this prompt — stated explicitly per the template's TESTS
//! field. The contracts owned (none yet) / consumed (11.1, 12.1) are recorded above.
//!
//! ## DAG position (a documented, NAMED leaf consumer)
//! Like `myelin-notif` / `myelin-agent-service`, this crate is a LEAF CONSUMER above the glue crates
//! (depends on `-tenancy` / `-refs` / `-gdpr` / `-substrate`) and is NOT a node in the eleven-crate
//! library DAG modelled by `myelin-substrate::crate_graph` — nothing in the production DAG depends
//! back on it; `substrate_is_root()` / `identity_is_sink()` are preserved (a subsystem schema crate
//! is the graph's terminal consumer, not a node in it).

pub mod app;
pub mod approval;
pub mod budget;
pub mod ci_pipeline;
pub mod crypto_shred;
// MR-009b W3b.5: the M6 dogfood drill runners build the in-process memory `Substrate`
// (`OutboxStore::new()` + memory journal/timers) — a drill harness, never production serving
// code. Gated with the events memory arm; the tests-dir drills reach it via the self dev-dep.
#[cfg(any(test, feature = "test-support"))]
pub mod dogfood;
pub mod engine;
pub mod executor;
pub mod holder;
pub mod job;
pub mod loopsafety;
pub mod maintenance;
pub mod merge_queue;
pub mod migrations;
pub mod remint;
// MR-009b W3b.5: the FLOW-D10 restore-verify drill re-hydrates a RESTORED in-memory outbox via
// the `test-support`-gated `OutboxStore::new()` + `restore_committed_row_for_test` seam — a
// PITR-model drill harness, never production serving code. Gated with the events memory arm.
#[cfg(any(test, feature = "test-support"))]
pub mod restore_verify;
pub mod schema;
pub mod signal_consumer;
pub mod surge;
pub mod timer;
pub mod wfctx;

pub use app::{
    boot_flow, flow_app_spec, flow_app_spec_with_engine, flow_signal_consumer_reg, run_flow,
    SERVICE_NAME,
};
pub use approval::{
    apply_approved_effects, approval_wait_name, per_effect_idem_key, request_approval_and_wait,
    ApplyError, ApprovalCard, ApprovalDecision, EffectApplier, EffectOutcome, GateResult,
    GatedEffect, APPROVAL_REQUESTED_EVENT, APPROVAL_SIGNAL_NAME, DECLINE_MARKER,
};
pub use budget::{BudgetError, BudgetGate, BudgetSettle, Wallet};
pub use ci_pipeline::{
    read_stage_verdict, stage_verdict_marker, CiPipelineSpec, CiStage, PipelineOutcome,
    CI_PIPELINE_WF_TYPE,
};
pub use crypto_shred::{
    aggregate_receipt as crypto_shred_receipt, history_row_has_inline_pii,
    is_inline_pii_unrecoverable, open_inline_pii, seal_inline_pii, signal_row_has_inline_pii,
    subject_dek_erasure, subject_dek_id, WfCryptoShred, WfShredReport,
};
// The reserve/settle cost type the public `CiStage` / `metered_schedule_and_run_job` surface takes
// (contract 11.7) — re-exported so a consumer building a `CiStage` does not need a second
// `myelin-storage` edge just to name the cost.
// The M6 dogfood loop (P-FLOW-29 / P-516): Myelin's own pipelines / merge queue / SLA timers run as
// myelin-flow workflows over the platform's own work + the FLOW truth-up pass over FLOW-D1..D10 + E2E-2.
#[cfg(any(test, feature = "test-support"))]
pub use dogfood::{
    proven_flow_rows, run_flow_over_myelins_own_work, run_flow_truth_up_scorecard,
    run_myelin_ci_pipeline, run_myelin_merge_queue, run_myelin_sla_timer, FlowDogfoodArtifact,
    FlowIncident, FlowIncidentDrillTicket, FlowIncidentIssueDraft, FlowRowStatus,
    FlowScorecardEntry, FlowTruthUpPass, FlowTruthUpRed, FlowTruthUpScorecard, FlowTruthUpVerdict,
    MergeFace, PipelineFace, ProvenFlowRow, SlaFace, MYELIN_SELF_REGION, MYELIN_SELF_TENANT,
};
pub use engine::{
    drive, drive_full, drive_versioned, drive_with_timers, run_state, DriveOutcome, FlowDispatcher,
    FlowTelemetry, RunRow, RunStore, SignalRow, SignalStore, WorkflowBody,
};
pub use executor::{
    DurableExecutor, ExecutorError, FlowExecutor, RunBudget, RunId, RunStatus, SignalOutcome,
    SignalSpec, StartSpec, PARTITION_COUNT,
};
pub use holder::{
    flow_history_holder, flow_store_classifier, register_flow_holder, FlowBacking,
    FlowHolderRegistration, RestrictSet, WfHistoryHolder, FLOW_OLTP_STORE,
};
pub use job::{
    job_dispatch_marker, job_idem_token, JobKind, JobOutcome, JobRunner, JobSpec, JOB_DONE_SIGNAL,
};
pub use loopsafety::{
    CausalGuard, LoopVerdict, RefusalReason, ACTIVITY_POOL_CAP, CEILING, SHARED_ROOT_WINDOW_CAP,
};
pub use maintenance::{
    invalidation_marker, maintenance_step_marker, CacheNamespace, MaintenanceOp,
    MaintenancePerformer,
};
pub use merge_queue::{
    ci_dispatch_marker, decode_ci_result, encode_ci_result, git_pr_merged_draft,
    humanise_dequeue_reason, merge_attempt_id, CheckFact, CiDispatch, CiDispatcher, DequeueCause,
    MergeOutcome, MergePerformer, MergeRequest, MockCiResultProducer, RealCiResultProducer,
    CI_RESULT_SIGNAL, GIT_PR_MERGED_EVENT,
};
pub use myelin_storage::reserve_settle::{MeteredUnit, MinorUnits};
pub use remint::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenLease, RunTokenMinter};
#[cfg(any(test, feature = "test-support"))]
pub use restore_verify::{
    ConsistentOffset, ConsistentPointArtifact, RestoreVerifyFailure, RestoreVerifyOutcome,
    RestoredFlow, WfRestore, WfRestoreVerify,
};
pub use signal_consumer::{FlowSignalConsumer, SIGNAL_EVENT_TYPE};
pub use surge::{
    run_flow_surge, FlowShedGate, FlowShedRejection, FlowSurgeReport,
    CROSS_CELL_SPANNING_IS_A_FLOOR, FLOW_SURGE_MULTIPLIER,
};
pub use timer::sla::{sla_timer_id, trigger_stale_timer_id, SlaTimerCall};
pub use timer::{
    epoch_minute, ArmOutcome, DisarmOutcome, FireOutcome, ReArmOutcome, TimerRow, TimerStore,
    TimerWheel, SECS_PER_MINUTE,
};
pub use wfctx::{
    attempt_state, history_kind, ActivityError, RetryPolicy, WaitOutcome, WfCtx, WfError,
    WfJournal, WfResult,
};
