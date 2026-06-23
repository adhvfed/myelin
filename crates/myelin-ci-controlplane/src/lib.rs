//! # myelin-ci-controlplane — the CI Control Plane service shell (CI-P6 → P-349, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/00-overview.md` §4 (the
//! five logical services — this is service #2, the CI Control Plane: scheduler + reaper + fleet
//! autoscaler + log-pipeline coordinator + secret broker + supply-chain verifier + the check
//! emitter; each a `serve(AppSpec)` shell, its own Postgres, no cross-DB) + §5 (cell topology — no
//! global pool, residency by construction); `01-tech-and-data-model.md` §3 (the complete
//! control-plane schema this shell migrates) + §4 (the encryption/residency/GDPR posture).
//! **Contracts:** `contract-index.md` rows 1.1 (`serve(AppSpec)` — the service shell, NOT a
//! hand-rolled `main`), 1.2/1.3 (the three ports + liveness ≠ readiness), 1.5 (the forward-only
//! migrations + hot-table flags), 11.1 (OLTP), 12.1 (the `(tenant, region)` partition key), 10.2
//! (the `#[personal_data(...)]` tags), 4.8 (pseudonym subjects — `triggered_by`/`approved_by`).
//!
//! ## What CI-P6 ships here — the bootable SHELL + the COMPLETE data model, NOT the behaviour
//! [`controlplane_app_spec`] assembles the CI Control Plane [`AppSpec`] the harness's ONE call
//! drives (boot → migrate → outbox relay → consumers → three ports → graceful drain, liveness ≠
//! readiness). The CI Control Plane is an `AppSpec`, not a hand-rolled lifecycle — the EXACT analog
//! of the Search / Refs / Identity service shells. The shell:
//!   - declares the **three ports** (public / internal / metrics-health) via the harness (1.2/1.3) —
//!     liveness must not check deps; readiness gates on the DB pool + the declared critical deps
//!     (arch 00 §4: DB + broker + authz + at-least-one-healthy-runner-pool);
//!   - runs the **complete forward-only data-model migrations** ([`migrations::ci_controlplane_migrations`]):
//!     all fourteen CI Control-Plane tables (`ci_run`, `ci_job`, `check_attempt`, `job_queue` +
//!     its three claim indexes, `fair_deficit`, `runner`, `log_segment`, `log_anchor`, `artifact`,
//!     `cache_entry`, `environment`, `deployment`, `secret_binding`, `cost_event`), each
//!     `(tenant_id, region)`-first + RLS-on (contract 11.1/12.1/1.5);
//!   - declares the **four hot tables** ([`migrations::ci_controlplane_hot_tables`]) — `job_queue`,
//!     `log_segment`, `cost_event`, `check_attempt` (arch 01 §3 "Hot-table flags declared");
//!   - declares its critical downstreams (`broker`, `authz`, `runner_pool`; the OLTP store is
//!     implicitly critical) for the readiness probe (§4.3, SUB-D9 — readiness red until DB + broker
//!     + authz + at-least-one-healthy-runner-pool reachable, arch 00 §4);
//!   - carries the `#[personal_data(...)]`-tagged row mirrors ([`schema`]) so the
//!     `no-untagged-personal-data` lint is GREEN on the CI schema (contract 10.2 / 4.8).
//!
//! ## Floors named (the per-table-behaviour follow-ons — see [`migrations`])
//! The CI-P6 shell shipped the table SHAPES + the bootable shell. The per-table behaviour lands in
//! its own prompt: the scheduler pull-lease claim over `job_queue` + concurrency + affinity + the
//! dead-runner reaper is now SHIPPED in [`scheduler`] (CI-P12 / P-355); DRR fair-share over
//! `fair_deficit` (CI-P13 — the claim ORDERs on `fair_deficit.deficit DESC`, the advance/replenish is
//! CI-P13), the fleet autoscaler over `runner` (CI-P14), the `check_attempt` counter + the
//! `ci.check.updated` producer (CI-P18), the log index (CI-P20), trust-scoped artifacts/caches
//! (CI-P22), reserve/settle metering into `cost_event` (CI-P17), the deploy/secret broker (CI-P24).
//! No consumers are registered at the shell (the Trigger & Dispatch dedup consumer is the OTHER
//! shell, [`myelin-ci-dispatch`]; the scheduler is not a bus consumer).
//!
//! ## DB-free by default; the live-stack proof behind `integration`
//! `cargo build --workspace` / `cargo test --workspace` stay DB-free (the shell boots over the
//! substrate's in-process floor pool; the migrations are `&str` DDL the runner admits without a DB).
//! The REAL forward-only apply against the dev-stack Postgres (RLS isolation + the claim indexes) is
//! `tests/integration_ci_p6_controlplane_schema.rs` behind the `integration` cargo feature.

pub mod ci_pipeline;
pub mod events;
pub mod fairness;
pub mod fleet;
pub mod holder;
pub mod migrations;
pub mod rebac_fragment;
pub mod schedule_and_run_job;
pub mod scheduler;
pub mod schema;

// CI-P15 (P-358): the `ci.pipeline` DURABLE WORKFLOW BODY + the X-1 producer side. The deterministic
// Rust body registered under `CI_PIPELINE_WF_TYPE` at serve (guarded by the flow-determinism lint):
// the protected-env / manual gates (9.4), the runner stages over the FROZEN `SCHEDULE_AND_RUN_JOB`
// long-park substrate (9.4/11.7/9.3), and CI's X-1 producer emits — the per-context terminal
// `ci.check.updated` facts + `ci.run.failed`/`ci.run.succeeded` + the `ci.result` rollup signal
// (contract 5.9). The `SCHEDULE_AND_RUN_JOB` handshake into the live scheduler/runner is CI-P16; the
// reserve/settle metering into `cost_event` is CI-P17; the `check_attempt` monotonic counter + the
// outbox producer plumbing is CI-P18; the end-to-end merge-queue seam GATE (GIT-D10/CI-D8) is CI-P19.
pub use ci_pipeline::{CheckFacts, PipelineRun, PipelineStage, RunVerdict, CI_PIPELINE_WF_TYPE};

// CI-P16 (P-359): the `SCHEDULE_AND_RUN_JOB` dispatch handshake into CI's `job_queue` + the
// effectively-once invariant (CI-D1). The concrete `JobRunner` that BINDS the FROZEN engine dispatch
// seam (9.2/9.4) onto the scheduler's `job_queue` — minting the deterministic `idem_token` (engine),
// idempotent enqueue on `jq_idem` (a reaper re-queue + a control-plane re-dispatch = ONE row), and the
// runner's terminal `job.done` delivery (idempotent on `idem_token`, the `wf_signal` PK = one wake).
// The reserve/settle metering bookends into `cost_event` are CI-P17 (P-360); the live runner lease +
// in-sandbox execution is GATED by AG-D4.
pub use schedule_and_run_job::{complete_job, JobScheduleTerms, SchedulerJobRunner};

pub use holder::{
    ci_store_classifier, register_ci_holders, CiHolder, CiHolderRegistration, CiStoreClass,
    RestrictionFlag, CI_OLTP_STORE, CI_RESIDUAL_POSTURE_REF,
};

pub use events::{
    ci_event_tokens, is_durable, register_ci_taxonomy, register_ci_tokens, validate_ci_type_token,
    validate_ci_type_tokens, CiTypeTokenError, CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS,
    CI_SUBSYSTEM_TOKEN, CI_TYPE_TOKENS,
};

use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
    ServeError, ServeHandle,
};

pub use scheduler::{
    lane_token, state_token, ClaimRequest, Claimed, EnqueueOutcome, JobState, Lane, QueuedJob,
    SchedulerState, CANCEL_SUPERSEDED_QUERY, CLAIM_QUERY, REAP_QUERY,
};

// CI-P14 (P-357): the EU fleet autoscaler — the FleetProvider impl + autoscale-on-queue-depth +
// per-residency-zone pools (no global pool) + the fleet events. The residency-pin runner-write
// boundary (1.6), the `residency_verify` report (12.4), the two EU adapters, and the autoscaler that
// sizes the per-(region, label-class) pool to the scheduler's queue depth.
pub use fleet::{
    AutoscalePolicy, Autoscaler, BareMetalPxeAdapter, CrossRegionRunnerWrite, EuFleetProvider,
    FleetAdapter, FleetError, FleetEvent, FleetPools, FleetResidencyReport, GenericEuIaasAdapter,
    PoolKey, RunnerWritePin, ScalePlan, COUNT_RUNNERS_BY_POOL_QUERY, DELETE_RUNNER_QUERY,
    INSERT_RUNNER_QUERY,
};

// CI-P13 (P-356): the scheduler fairness slice — DRR fair-share over `fair_key` + the lane shed
// order + per-tenant backpressure (the slice CI-P12 named as its floor). The deficit advance/
// replenish (plan-weighted) + the bounded run-queue cap + the lane shed order, with the live
// `fair_deficit`/`job_queue` SQL the integration test proves against Postgres.
pub use fairness::{
    shed_order, Backpressure, FairShare, PlanTier, ADVANCE_DEFICIT_QUERY, BASE_QUANTUM,
    DEFAULT_TENANT_IN_FLIGHT_CAP, DEFICIT_CEILING, IN_FLIGHT_COUNT_QUERY, REPLENISH_DEFICIT_QUERY,
};

pub use migrations::{
    ci_controlplane_hot_tables, ci_controlplane_migrations, make_tenant_scoped_ddl, ARTIFACT_TABLE,
    CACHE_ENTRY_TABLE, CHECK_ATTEMPT_TABLE, CI_JOB_TABLE, CI_RUN_TABLE, COST_EVENT_TABLE,
    CREATE_ARTIFACT_DDL, CREATE_CACHE_ENTRY_DDL, CREATE_CHECK_ATTEMPT_DDL, CREATE_CI_JOB_DDL,
    CREATE_CI_RUN_DDL, CREATE_COST_EVENT_DDL, CREATE_DEPLOYMENT_DDL, CREATE_ENVIRONMENT_DDL,
    CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
    CREATE_LOG_ANCHOR_DDL, CREATE_LOG_SEGMENT_DDL, CREATE_RUNNER_DDL, CREATE_SECRET_BINDING_DDL,
    DEPLOYMENT_TABLE, ENVIRONMENT_TABLE, FAIR_DEFICIT_TABLE, JOB_QUEUE_TABLE, JQ_CLAIMABLE_INDEX,
    JQ_IDEM_INDEX, JQ_SERIALIZE_INDEX, LOG_ANCHOR_TABLE, LOG_SEGMENT_TABLE, RUNNER_TABLE,
    SECRET_BINDING_TABLE,
};

/// The deployable service name (the `AppSpec::name` + the telemetry/trace service identifier). The
/// `ci-controlplane` binary (`src/main.rs`) and the `AppSpec` both read this.
pub const SERVICE_NAME: &str = "ci-controlplane";

/// The critical-dependency set the metrics-health readiness probe reads (§4.3, SUB-D9 / arch 00 §4).
/// The OLTP store is implicitly critical (the harness adds it). The CI Control Plane declares:
/// - `broker` — the durable bus the check emitter / log-pipeline coordinator publishes to (an
///   outbox→bus producer cannot serve correct traffic without it);
/// - `authz` — Identity's `check`/`list_objects` the trust-tier evaluation + the surfacing
///   push-down depend on (a dead authz means CI cannot make correct trust/visibility decisions);
/// - `runner_pool` — at-least-one-healthy-runner-pool (arch 00 §4: a control plane with NO runner
///   pool cannot dispatch any job, so it reports not-ready + sheds rather than queuing into a void).
///
/// A dead critical dependency reports not-ready + sheds while liveness stays Up (no restart storm).
fn controlplane_critical() -> CriticalDependencies {
    CriticalDependencies::new(["broker", "authz", "runner_pool"])
}

/// **Assemble the CI Control Plane service [`AppSpec`] (contract 1.1; the service shell).** The
/// harness owns the lifecycle around it (boot → migrate → relay → consumers → three ports →
/// graceful drain, liveness ≠ readiness). The CI Control Plane is an `AppSpec` + handlers, NOT a
/// hand-rolled `main`.
///
/// `config` is the validated, env-first config (§3.2; `Config::from_env()` lands with the driver,
/// P-S15 — the shell boots over the validated default today). The complete forward-only data-model
/// migrations create all fourteen control-plane tables `(tenant, region)`-first + RLS-on; the four
/// hot tables are declared; `broker` / `authz` / `runner_pool` are declared critical. No consumers
/// are registered here — the scheduler/check-emitter behaviour is the per-table follow-ons (named in
/// [`migrations`]); the dedup consumer is the Trigger & Dispatch shell.
pub fn controlplane_app_spec(config: Config) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: ci_controlplane_migrations(),
        hot_tables: ci_controlplane_hot_tables(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        // No consumers at the shell — the scheduler is not a bus consumer; the dedup consumer is the
        // Trigger & Dispatch shell. The check emitter is an outbox PRODUCER (CI-P18), not registered
        // as a consumer here.
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        // The implicit OLTP store (the harness adds it) is the only store the control plane owns at
        // the shell — every control-plane table lives in the one Postgres; the blob/cache/log-tier
        // stores are declared by their behaviour bands (CI-P20/CI-P22). Auto-registered as holders.
        stores: myelin_substrate::StoreManifest::new(),
        outbox: OutboxSpec::default(),
        critical: controlplane_critical(),
    }
}

/// **Boot the CI Control Plane service to the pre-serve [`ServeHandle`]** (the harness's [`boot`] of
/// [`controlplane_app_spec`]). Separated from [`run_controlplane`] so a test/drill can boot, assert
/// the three ports opened + the migrations ran + the holders registered, drive ticks, and drive the
/// drain deterministically.
pub fn boot_controlplane(config: Config) -> Result<ServeHandle, ServeError> {
    boot(controlplane_app_spec(config))
}

/// **The CI Control Plane service entry — the one `serve(AppSpec)` call (contract 1.1).** The
/// `ci-controlplane` binary (`src/main.rs`) does nothing but hand [`controlplane_app_spec`] to this.
/// A failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.
pub fn run_controlplane(config: Config) -> Result<(), ServeError> {
    serve(controlplane_app_spec(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{Liveness, Surface};

    /// **THE CI Control Plane shell boot test (contract 1.1/1.2/1.3): boots from `serve(AppSpec)`
    /// with three ports + liveness ≠ readiness; the complete forward-only data model applies.** This
    /// is the prompt's GATE: the shell compiles + boots from `serve(AppSpec)` with the three-surface
    /// split and liveness ≠ readiness, and the forward-only migrations create every CI table.
    #[test]
    fn controlplane_boots_from_serve_appspec_with_three_ports() {
        let handle = boot_controlplane(Config::default())
            .expect("the CI Control Plane shell boots from serve(AppSpec)");
        assert_eq!(handle.name(), SERVICE_NAME, "the deployable service name");

        // (1.2) the three ports opened in the lifecycle (public / internal / metrics-health).
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports opened (contract 1.2)"
        );

        // (1.3) liveness ≠ readiness: after a successful boot the startup gate is Complete, so
        // readiness is governed by the critical-dependency health (not the same signal as liveness).
        let mh = handle.metrics_health();
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness = not-wedged (never checks a dependency)"
        );
        assert!(
            mh.readiness().is_ready(),
            "readiness = can-serve-now (all critical deps healthy at boot) — distinct from liveness"
        );
    }

    /// **A dead critical dependency (`runner_pool`) flips readiness to not-ready WITHOUT flipping
    /// liveness (liveness ≠ readiness, contract 1.3 / SUB-D9 / arch 00 §4).** The CI Control Plane
    /// cannot dispatch a job with NO healthy runner pool, so it reports not-ready + sheds — but it
    /// stays live (no restart storm). This proves the readiness probe gates on the runner pool (arch
    /// 00 §4: readiness = DB + broker + authz + at-least-one-healthy-runner-pool).
    #[test]
    fn dead_runner_pool_flips_readiness_not_liveness() {
        let handle = boot_controlplane(Config::default()).expect("boot");
        let mh = handle.metrics_health();
        assert!(
            mh.readiness().is_ready(),
            "ready while the runner pool is healthy"
        );

        // Mark the declared-critical `runner_pool` dependency down.
        handle.health_probe().mark_down("runner_pool");

        assert!(
            !mh.readiness().is_ready(),
            "no healthy runner pool → not-ready + shed (arch 00 §4)"
        );
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness stays UP (not-ready is NOT not-alive — no restart storm)"
        );

        // The other two declared critical deps (broker, authz) also gate readiness.
        let handle2 = boot_controlplane(Config::default()).expect("boot");
        handle2.health_probe().mark_down("authz");
        assert!(
            !handle2.metrics_health().readiness().is_ready(),
            "a dead authz also flips readiness (the trust/visibility decision dependency)"
        );
    }

    /// **The CI Control Plane shell runs the whole lifecycle end-to-end and drains cleanly (contract
    /// 1.1).** `run_controlplane` boots → migrates (creates every CI table) → … → graceful-drains →
    /// returns Ok. The CDC consumer side of 1.1 (a service `main` that just calls the one entry).
    #[test]
    fn run_controlplane_runs_lifecycle_and_returns_ok() {
        assert_eq!(
            run_controlplane(Config::default()),
            Ok(()),
            "the CI Control Plane shell boots → … → drains cleanly"
        );
    }

    /// **A failed boot returns non-zero (§3.1).** A config that fails boot-time validation aborts
    /// boot loudly — the shell never starts half-booted.
    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_controlplane(Config("BAD_POOL".into()));
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    /// **The shell's AppSpec carries the complete data model + the four hot tables + the critical
    /// deps, and NO consumers (the behaviour floor).** Pins the shell's surface so a later edit that
    /// smuggles in a consumer without reconciliation, or drops a table / a hot-table flag, is loud.
    #[test]
    fn the_shell_carries_the_complete_data_model_and_no_consumers() {
        let spec = controlplane_app_spec(Config::default());
        assert_eq!(
            spec.migrations.0.len(),
            14,
            "all 14 control-plane tables are in the forward-only migration set"
        );
        assert!(
            spec.consumers.is_empty(),
            "no consumers at the shell (the scheduler is not a bus consumer; dedup is the dispatch shell)"
        );
        // the four hot tables are declared.
        for t in [
            JOB_QUEUE_TABLE,
            LOG_SEGMENT_TABLE,
            COST_EVENT_TABLE,
            CHECK_ATTEMPT_TABLE,
        ] {
            assert!(spec.hot_tables.is_hot(t), "`{t}` is declared hot");
        }
        // the three critical downstreams are declared (beyond the implicit OLTP store).
        let deps: Vec<&str> = spec.critical.deps().iter().map(|d| d.0.as_str()).collect();
        assert!(deps.contains(&"broker"), "broker is critical");
        assert!(deps.contains(&"authz"), "authz is critical");
        assert!(deps.contains(&"runner_pool"), "runner_pool is critical");
    }
}
