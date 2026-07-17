//! # `ci-controlplane` — the CI Control Plane service binary (CI-P6 → P-349, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.6, the W3b.4 pattern) and hands the CI Control Plane
//! [`AppSpec`](myelin_ci_controlplane::controlplane_app_spec) to the harness's one call,
//! [`run_controlplane`](myelin_ci_controlplane::run_controlplane) (a thin wrapper over `serve`).
//! The harness owns the whole lifecycle (boot → migrate → outbox relay → consumers → three ports
//! → graceful drain, with liveness ≠ readiness); this `main` composes and hands off — no
//! hand-rolled lifecycle logic.
//!
//! On boot the CI Control Plane shell runs the complete forward-only data-model migrations (every CI
//! Control-Plane table — `ci_run` … `cost_event` — `(tenant, region)`-first + RLS-on) and
//! auto-registers its OLTP store as a `PersonalDataHolder`. A failed boot / incomplete drain returns
//! non-zero (§3.1) — loud, never a silent success.
//!
//! **DURABLE-BY-DEFAULT (MR-009b W3b.6 / SI-007):** the outbox the relay drains is the PG-backed
//! `outbox` table (`OutboxStore::durable(PgOutboxBacking)`) over the MR-022 `SubstrateProvider`
//! pool, with the substrate foundation migrations (`outbox` + `consumer_dedup`) applied at boot —
//! committed events survive a process restart. **FAIL LOUD on missing durable config** (the W3b.4
//! service-main pattern): a missing `DATABASE_URL`, an unreachable pool, or a failed foundation
//! migration each exit non-zero — NEVER a silent in-memory fallback (the in-memory
//! `OutboxStore::new()` is `test-support`-gated and does not even compile here).
//!
//! The runtime is the multi-thread `#[tokio::main]` flavor (required): the sync
//! `DurableOutboxBacking` verbs bridge to async sqlx via `block_in_place` + `block_on`, which
//! panics on a current-thread runtime.
//!
//! **Floor:** the env-first `Config::from_env()` parse for the substrate AppSpec config lands with
//! the driver (P-S15); the shell boots over the validated default (the durable config THIS root
//! depends on is the PG DSN, required explicitly above). The per-table behaviour (the scheduler
//! claim, the check emitter, the log index, the metering) is the CI-P12..CI-P24 surface — this
//! shell runs no job yet.

use myelin_ci_controlplane::run_controlplane;
use myelin_config::{Mode, MyelinConfig};
use myelin_events::OutboxStore;
use myelin_storage::{all_durable_migrations, HotTables, PgOutboxBacking, SubstrateProvider};
use myelin_substrate::Config;
use std::sync::Arc;

/// The four-guarantee hooks the runner drives on every launch (X-6; arch 02 §5.2). The gVisor backend
/// ALSO enforces the mandatory hardening profile internally (isolation floor) regardless of the hook,
/// so these accept and let the launch reach the real `runsc` run. The REAL metering reserve/settle
/// (against the cost store) + the per-run token attribution are the CT-004d bookend — wired here as the
/// four-guarantee seam the sandbox drives (never bypassed).
fn runner_hooks() -> myelin_ci_sandbox::RunnerHooks {
    myelin_ci_sandbox::RunnerHooks {
        reserve: Box::new(|m| Ok(myelin_ci_sandbox::ReserveHandle(m.reserve_id.clone()))),
        settle: Box::new(|_h, _u| Ok(())),
        attribute: Box::new(|_t| Ok(())),
        isolation_floor: Box::new(|_s| Ok(())),
    }
}

#[tokio::main]
async fn main() {
    // FAIL LOUD on missing durable config (the W3b.4 pattern): the durable outbox requires the PG
    // DSN. No DATABASE_URL → refuse to boot (exit non-zero) — never a silent in-memory fallback.
    if std::env::var("DATABASE_URL")
        .map(|v| v.trim().is_empty())
        .unwrap_or(true)
    {
        eprintln!(
            "ci-controlplane: DATABASE_URL is required (durable-by-default outbox, MR-009b \
             W3b.6): refusing to boot without durable config — there is no in-memory fallback"
        );
        std::process::exit(1);
    }
    let config = MyelinConfig::from_env(Mode::DevDefaults).unwrap_or_else(|e| {
        eprintln!("ci-controlplane: invalid config: {e}");
        std::process::exit(1);
    });
    let provider = match SubstrateProvider::connect(config, 8).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "ci-controlplane: cannot reach the durable OLTP pool (durable-by-default \
                 requires PG): {e}"
            );
            std::process::exit(1);
        }
    };
    // The substrate foundation tables (the frozen `outbox` + `consumer_dedup` DDL) must exist
    // before the durable store binds — applied through the MR-022 migrator (idempotent,
    // forward-only, advisory-locked). Only the foundation set is applied here: the tables THIS
    // root's durable path needs, never a silently-widened migration surface.
    if let Err(e) = provider.migrate_foundation().await {
        eprintln!(
            "ci-controlplane: cannot apply the substrate foundation migrations \
             (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    // W7.2 (doc-18 Part 5) — THE BOOT-MIGRATIONS FIX: apply the FULL durable migration aggregate
    // (identity 0010–0019, pseudonym 0020–0022, placement 0030–0039, kms 0040–0042, cost/erasure
    // 0050–0053) after the foundation, so EVERY durable store bound at this main's boot has its
    // tables on a fresh DB (doc-18: a main that migrated only a piecemeal subset left the stores it
    // constructs writing to un-migrated tables). Idempotent + advisory-locked (safe on re-boot);
    // FAIL LOUD, never a silent fallback.
    if let Err(e) = provider
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("ci-controlplane: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // CT-004m — apply the SHARED CI durable writer subset (`ci_run` + `check_attempt` + `ci_cost_event`)
    // at boot, BEFORE the cost store is constructed and independent of `serve(AppSpec)` order. This is
    // the SAME forward-only set (same ids/DDL) `serve` applies as part of the full 14-table
    // `ci_controlplane_migrations`, so the overlap no-ops (idempotent, advisory-locked). ci-dispatch
    // applies the identical set at its boot, so the writer tables exist regardless of which CI service
    // boots first (breaking the former boot-order coupling). FAIL LOUD.
    if let Err(e) = provider
        .migrate(
            &myelin_ci_controlplane::ci_durable_migrations(),
            &myelin_ci_controlplane::ci_durable_hot_tables(),
        )
        .await
    {
        eprintln!("ci-controlplane: cannot apply the shared CI durable migrations (ci_run/check_attempt/ci_cost_event): {e}");
        std::process::exit(1);
    }
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    // CT-004a: construct the REAL durable CI `ci_cost_event` projection store from the provider pool —
    // proving the metering path is a production-callable store (not model-only). DORMANT at the shell
    // (no consumer drives it yet); CT-004d attaches it to the `SCHEDULE_AND_RUN_JOB` dispatch settle
    // bookend. CT-004m resolved the former `cost_event` table-name collision (CI's table is now
    // `ci_cost_event`, created by the shared `ci_durable_migrations` applied above). Building it only
    // wraps the pool — no query runs at boot.
    let _ci_cost_events =
        myelin_ci_controlplane::ci_cost_event_store(provider.db_pool().clone());
    // CT-004c.1: construct the REAL durable `job_queue` store + spawn the dead-runner reaper loop onto
    // the serve runtime (minimal-impact wiring — a bounded background task hung off the existing
    // lifecycle, NOT a new AppSpec schema field). The `job_queue` table the reaper sweeps is created
    // by the full `ci_controlplane_migrations` the `serve(AppSpec)` below applies at boot; the reaper
    // delays its first sweep one interval so that boot-migrate has completed. The reaper is SAFE — it
    // only re-queues expired leases (`leased`→`queued`); it launches NO untrusted code (binding the
    // runner + starting the pipeline body on the sandbox executor is CT-004c.2). The claim path is
    // dormant at the shell (no runner drives `claim` yet — CT-004c.2 wires it).
    let ci_job_queue = myelin_ci_controlplane::ci_job_queue_store(provider.db_pool().clone());
    let reaper = myelin_ci_controlplane::JobQueueReaper::new(
        ci_job_queue,
        provider.config().region.clone(),
        std::time::Duration::from_secs(15),
    );
    tokio::spawn(reaper.run());
    // CT-004c.2 + CT-004d.1: OPT-IN spawn the bounded CI runner loop — it claims from the durable
    // `job_queue`, RESOLVES the leased row to its digest-pinned `JobSpec` via the durable `ci_job_spec`
    // store (CT-004d.1's real resolver), and EXECUTES it in a REAL gVisor (`runsc`) guest (the AG-D4
    // gate). Gated behind `MYELIN_CI_RUNNER=1` (default OFF): a control-plane node runs untrusted code
    // ONLY when explicitly enabled as a runner host — never implicitly on every boot. The loop runs on
    // a DEDICATED thread (off the tokio runtime) so `RunnerAgent::run_one` (sync, blocking for the whole
    // in-line job) does not starve a worker; the lease adapter + the spec resolver bridge their async DB
    // calls onto the runtime handle. The trust-tier/region claim predicate is CT-004c.1's durable store,
    // forwarded UNCHANGED (an `untrusted_fork` job is never claimed by this trusted-only runner).
    //
    // CT-004d.1 replaces the CT-004c.2 fail-closed no-op resolver with the REAL
    // `durable_spec_resolver` over `ci_job_spec` (the dispatch co-persists the spec there); a
    // leased-but-unresolved row (spec absent/corrupt) still resolves fail-closed → the row stays leased
    // and is reaped, never an unresolved/fabricated launch. The lease TTL is `CI_RUNNER_LEASE_TTL_SECS`
    // (ABOVE the max job timeout the spec store enforces) so a long job never lapses its lease mid-run
    // (the CT-004c.2 double-run fix). The shared durable executor the `job.done` wakes a REAL parked
    // `ci.pipeline` body on (registering/starting that body) is CT-004d.2; the live settle bookend is
    // CT-004d.3 — here the executor is the composition placeholder proving the one signal path.
    if std::env::var("MYELIN_CI_RUNNER").as_deref() == Ok("1") {
        let region = provider.config().region.clone();
        let runner_store = myelin_ci_controlplane::ci_job_queue_store(provider.db_pool().clone());
        // The REAL durable spec resolver (CT-004d.1): resolve a leased row → the spec the dispatch
        // co-persisted into `ci_job_spec`. Fail-closed (missing/corrupt spec → no launch, reaped).
        let spec_store = myelin_ci_controlplane::ci_job_spec_store(provider.db_pool().clone());
        let resolver = myelin_ci_controlplane::durable_spec_resolver(
            spec_store,
            region.clone(),
            tokio::runtime::Handle::current(),
        );
        // CT-004d.2 CULMINATION (chunks 2/3/5): the CI pipeline DRIVER — the SHARED `FlowExecutor` the
        // runner's `job.done` wakes + registers/drives `run_ci_pipeline_body` (chunk 5's `DurableJobRunner`
        // dispatches each stage into the DURABLE `job_queue`+`ci_job_spec`, trust_tier/region forwarded
        // UNCHANGED). The runner's terminal reporter is the driver's `CiPipelineReporter` (re-encodes the
        // verdict + wakes the parked run — the ONE signal path). The pinned-snapshot→JobSpec resolver is
        // the fail-closed `unresolved_stage_spec_builder` floor (a real builder is the CT-004d follow-on;
        // the mechanism is proven end-to-end against real `runsc` in the CT-004d.2 integration test).
        let driver = std::sync::Arc::new(myelin_ci_controlplane::CiPipelineDriver::new(
            myelin_tenancy::TenantId("ci-controlplane".into()),
            region.clone(),
            myelin_ci_controlplane::ci_job_spec_store(provider.db_pool().clone()),
            tokio::runtime::Handle::current(),
            myelin_ci_controlplane::unresolved_stage_spec_builder(),
            outbox.clone(),
        ));
        let reporter = driver.reporter();
        let runner = myelin_ci_controlplane::CiRunnerLoop::new(
            format!("ci-runner-{}", std::process::id()),
            vec!["linux".to_string()],
            vec![myelin_ci_sandbox::TrustTier::Trusted],
            region,
            myelin_ci_controlplane::CI_RUNNER_LEASE_TTL_SECS,
            runner_store,
            tokio::runtime::Handle::current(),
            resolver,
            reporter,
            runner_hooks(),
        );
        runner.spawn();
        // Drive the pipeline engine on a DEDICATED thread (off the tokio runtime, like the runner): the
        // body's durable dispatch bridges its async co-persist onto the runtime, and driving off-runtime
        // keeps `block_on` correct. NAMED FLOOR: the `ci_run`-poll STARTER (reading queued `ci_run` rows
        // and `start_run`-ing them under their pre-minted `wf_run_id`) is the production autonomy wire —
        // the integration test drives `start_run` directly to prove the whole spine. Until the starter
        // lands, this loop drives no runs (a cheap idle sweep); the composition is LIVE + correct.
        std::thread::Builder::new()
            .name("ci-pipeline-driver".into())
            .spawn(move || loop {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                driver.drive_once(now, "1970-01-01T00:00:00Z");
                std::thread::sleep(std::time::Duration::from_millis(500));
            })
            .expect("spawn the ci-pipeline-driver thread");
    }
    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
    match run_controlplane(Config::default(), outbox) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("ci-controlplane service failed: {e}");
            std::process::exit(1);
        }
    }
}
