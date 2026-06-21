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
//!   flow store classifies to H8 — the §5.5 references-not-payloads reconcile; the per-subject-DEK
//!   crypto-shred reach is the NAMED M5 follow-on **P-FLOW-23**).
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
//!     seven-figure cell-scale run is **P-FLOW-24**) — the rest land later. An empty journal is not a
//!     working engine.
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
pub mod engine;
pub mod executor;
pub mod holder;
pub mod migrations;
pub mod schema;
pub mod signal_consumer;
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
pub use engine::{
    drive, drive_full, drive_versioned, drive_with_timers, run_state, DriveOutcome, FlowDispatcher,
    FlowTelemetry, RunRow, RunStore, SignalRow, SignalStore, WorkflowBody,
};
pub use timer::{epoch_minute, ArmOutcome, FireOutcome, TimerRow, TimerStore, TimerWheel, SECS_PER_MINUTE};
pub use executor::{
    DurableExecutor, ExecutorError, FlowExecutor, RunBudget, RunId, RunStatus, SignalOutcome,
    SignalSpec, StartSpec, PARTITION_COUNT,
};
pub use signal_consumer::{FlowSignalConsumer, SIGNAL_EVENT_TYPE};
pub use holder::{
    flow_history_holder, flow_store_classifier, register_flow_holder, FlowBacking,
    FlowHolderRegistration, RestrictSet, WfHistoryHolder, FLOW_OLTP_STORE,
};
pub use wfctx::{
    attempt_state, history_kind, ActivityError, RetryPolicy, WaitOutcome, WfCtx, WfError, WfJournal,
    WfResult,
};
