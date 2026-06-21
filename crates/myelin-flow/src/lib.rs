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
//!     leg on [`FlowTelemetry`]; the 9.1 CDC pair is `tests/cdc_9_1_executor.rs`; the `signal` half
//!     is the named follow-on **P-FLOW-09**); the replay-divergence guard (**P-FLOW-07**, FLOW-D2); the flow-determinism
//!     lint fixtures (**P-FLOW-08**); durable signals (**P-FLOW-09**); durable timers
//!     (**P-FLOW-13**, FLOW-D3) — the rest land later. An empty journal is not a working engine.
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
pub mod engine;
pub mod executor;
pub mod holder;
pub mod migrations;
pub mod schema;
pub mod wfctx;

pub use app::{boot_flow, flow_app_spec, flow_app_spec_with_engine, run_flow, SERVICE_NAME};
pub use engine::{
    drive, run_state, DriveOutcome, FlowDispatcher, FlowTelemetry, RunRow, RunStore, WorkflowBody,
};
pub use executor::{
    DurableExecutor, ExecutorError, FlowExecutor, RunBudget, RunId, RunStatus, StartSpec,
    PARTITION_COUNT,
};
pub use holder::{
    flow_history_holder, flow_store_classifier, register_flow_holder, FlowBacking,
    FlowHolderRegistration, RestrictSet, WfHistoryHolder, FLOW_OLTP_STORE,
};
pub use wfctx::{
    attempt_state, history_kind, ActivityError, RetryPolicy, WfCtx, WfError, WfJournal, WfResult,
};
