//! # `ci-dispatch` — the CI Trigger & Dispatch service binary (CI-P6 → P-349, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.6, the W3b.4 pattern) and hands the CI Trigger & Dispatch
//! [`AppSpec`](myelin_ci_dispatch::dispatch_app_spec) to the harness-owned persistent lifecycle via
//! [`run_dispatch_until_shutdown`](myelin_ci_dispatch::run_dispatch_until_shutdown). The harness
//! owns the whole lifecycle (boot → migrate → outbox relay → consumers → three ports → graceful
//! drain, with liveness ≠ readiness); this `main` composes and hands off.
//!
//! On boot the Trigger & Dispatch shell runs the forward-only migration that creates the
//! `consumer_dedup` ledger (the exactly-once-effect anchor) and auto-registers its OLTP store as a
//! `PersonalDataHolder`. A failed boot / incomplete drain returns non-zero (§3.1) — loud.
//!
//! **DURABLE-BY-DEFAULT (MR-009b W3b.6 / SI-007):** emitted rows use the PG-backed `outbox` table
//! (`OutboxStore::durable(PgOutboxBacking)`) over the MR-022 `SubstrateProvider`
//! runtime pool, after a privileged migration pool applies the complete schema and is destroyed —
//! committed events survive a process restart. **FAIL LOUD on missing durable config** (the W3b.4
//! service-main pattern): missing/distinct `DATABASE_URL` and `DATABASE_MIGRATION_URL`, an
//! unreachable pool, or a failed migration each exit non-zero — NEVER a silent in-memory fallback
//! (the in-memory
//! `OutboxStore::new()` is `test-support`-gated and does not even compile here).
//!
//! The runtime is the multi-thread `#[tokio::main]` flavor (required): the sync
//! `DurableOutboxBacking` verbs bridge to async sqlx via `block_in_place` + `block_on`, which
//! panics on a current-thread runtime.
//!
//! **Floor:** the substrate AppSpec config still uses its validated default, while every production
//! endpoint and both PostgreSQL roles are explicit through `Mode::RequireEnv`. The dispatch
//! intake is a named durable JetStream pull consumer; Git reads use a validated shared root; CAS
//! snapshots use the configured S3 backing; the durable handler executes the CI-P10/CI-P11
//! matcher, trust stamp, definition resolution, and reserve/start handoff.

use myelin_ci_dispatch::{
    git_intake_filter, run_dispatch_until_shutdown, AuthoritativeGitRoot, RecoveringIntake,
    EVENT_DURABLE_CONSUMER, EVENT_STREAM_NAME, EVENT_SUBJECT_ROOT,
};
use myelin_config::Mode;
use myelin_events::nats::JetStreamConsumerConfig;
use myelin_events::OutboxStore;
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::Config;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // Production is strict: validate every endpoint plus distinct migration/runtime PostgreSQL
    // roles before any DDL, durable store, consumer, or listener can be created. `PgBootstrap`
    // alone owns the privileged pool.
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("ci-dispatch: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    // The substrate foundation tables (the frozen `outbox` + `consumer_dedup` DDL) must exist
    // before the durable store binds — applied through the MR-022 migrator (idempotent,
    // forward-only, advisory-locked). Only the foundation set is applied here: the tables THIS
    // root's durable path needs, never a silently-widened migration surface.
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "ci-dispatch: cannot apply the substrate foundation migrations \
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
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("ci-dispatch: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // CT-004m — apply the SHARED CI durable writer subset (`ci_run` + `check_attempt` + `ci_cost_event`)
    // at boot. ci-dispatch's reserve/start co-commit writes `ci_run` (owned by myelin-ci-controlplane);
    // its own `serve(AppSpec)` migrate applies ONLY `consumer_dedup`, so before CT-004m the `ci_run`
    // write depended on ci-controlplane booting first (a boot-order coupling in the ONE shared `myelin`
    // DB). Applying the SAME forward-only set (same ids/DDL) both mains carry breaks that coupling — the
    // writer tables exist regardless of boot order (idempotent, advisory-locked). FAIL LOUD.
    if let Err(e) = bootstrap
        .migrate(
            &myelin_ci_controlplane::ci_durable_migrations(),
            &myelin_ci_controlplane::ci_durable_hot_tables(),
        )
        .await
    {
        eprintln!("ci-dispatch: cannot apply the shared CI durable migrations (ci_run/check_attempt/ci_cost_event): {e}");
        std::process::exit(1);
    }
    // Apply Dispatch's AppSpec declaration too. This now repeats the foundation-owned dedup DDL
    // byte-for-byte under the service migration id; it cannot fork the runtime backing's schema.
    if let Err(e) = bootstrap
        .migrate(
            &myelin_ci_dispatch::dispatch_migrations(),
            &HotTables::none(),
        )
        .await
    {
        eprintln!("ci-dispatch: cannot apply the Dispatch service migrations: {e}");
        std::process::exit(1);
    }
    // Re-probe the constrained runtime role, close the privileged pool, and erase its DSN before
    // any runtime query/store/consumer/listener is created.
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("ci-dispatch: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    // #11 — BOOT-TIME SHAPE ASSERTION on the money table. `CREATE TABLE IF NOT EXISTS` above no-ops on
    // a pre-existing (possibly pre-CT-004m mis-shaped) `ci_cost_event`, so assert the columns/types are
    // the CI metering-projection shape before ANY settle can write money data. FAIL LOUD, never write
    // to a wrong-shaped table.
    if let Err(e) = myelin_ci_controlplane::verify_ci_cost_event_shape(provider.db_pool()).await {
        eprintln!("ci-dispatch: ci_cost_event shape assertion failed: {e}");
        std::process::exit(1);
    }
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    // CT-004b / finding #6 — REGISTER the LIVE `ci-dispatch.trigger` consumer (was the `Vec::new()`
    // shell that registered NO consumer in production). The four backings are constructed here and
    // handed to `build_dispatch_consumers` (the `sqlx`-touching `CiRunStore` + dedup backing are
    // built from `provider.db_pool()` by inference — this main names no `sqlx`, the same idiom as the
    // outbox above):
    //   - reserve = `CoCommitReserveStore` (CT-004d.2 chunk 4): the run-of-record `ci_run` ROW
    //     co-commits ATOMICALLY with the dedup mark on the consumer's `HandlerTx`
    //     (`CiRunStore::co_commit_insert`); the events ride the DURABLE outbox in ABSORB mode (the
    //     honest #7 H1 split). Its `ci_run`/`consumer_dedup` tables are migrated above (foundation +
    //     CT-004m), so no boot-order coupling.
    //   - CAS = the real `S3BlobStore` (RustFS/Scaleway). The former "INTEGRATION-GATED CAS" note was
    //     STALE: `aws-sdk-s3` is a NON-OPTIONAL dep via `myelin-storage` (MR-009b Wave 1), so the CAS
    //     store is default-reachable — `cargo tree -p myelin-ci-dispatch -i aws-sdk-s3` shows it in the
    //     default graph; no `myelin-storage/integration` needed.
    //   - git-read = `DurableGitConfigReader` over `DurableGitStore::rooted(MYELIN_GIT_ROOT)` — reads
    //     the pushed repo's `.myelin/ci.*` from the SAME on-disk git-root the edge writes (shared-
    //     volume deploy; the env default mirrors `myelin-edge` main).
    //   - dedup = the durable `DedupLedger` over the shared pool (the exactly-once effect anchor).
    // The consumer LOGIC + the durable `ci_run` ROW ⇄ mark co-commit are proven end-to-end on live PG
    // in `tests/integration_ci_ct004b_trigger_consumer.rs` proofs (4)/(5); this is the production
    // registration that drives it. Durable JetStream intake is constructed below only after the
    // authoritative Git root and all effect backings have been validated/wired.
    let git_root = std::env::var("MYELIN_GIT_ROOT")
        .map_err(|_| "MYELIN_GIT_ROOT is required".to_string())
        .and_then(|path| AuthoritativeGitRoot::validate(path).map_err(|error| error.to_string()))
        .unwrap_or_else(|message| {
            eprintln!("ci-dispatch: {message}; refusing broker intake");
            std::process::exit(1);
        });
    eprintln!(
        "ci-dispatch: authoritative Git reads use {} in cell region {}",
        git_root.as_path().display(),
        provider.config().region
    );
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::UlidMinter::new());
    let ci_run = myelin_ci_controlplane::ci_run_store_factory(provider.db_pool().clone());
    let dedup = myelin_events::DedupLedger::durable(Arc::new(
        myelin_storage::events_durable::DurableDedupBacking::new(
            provider.db_pool().clone(),
            tokio::runtime::Handle::current(),
        ),
    ) as Arc<dyn myelin_events::DurableDedup>);
    let dead_letters: Arc<dyn myelin_events::DurableDeadLetter> = Arc::new(
        myelin_storage::events_durable::DurableDeadLetterBacking::new(
            provider.db_pool().clone(),
            tokio::runtime::Handle::current(),
        ),
    );
    // A SEPARATE durable outbox handle for the reserve store's event-absorb: the `outbox` above is
    // MOVED into `run_dispatch` below; both are cheap handles over the same pool.
    let reserve_outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    let blobs = Arc::new(myelin_storage::s3blob::S3BlobStore::connect(
        &provider.config().s3,
        tokio::runtime::Handle::current(),
    ));
    match blobs.preflight() {
        Ok(()) => {}
        Err(error @ (myelin_storage::blob::BlobDependencyError::PermanentConfig
            | myelin_storage::blob::BlobDependencyError::PermanentAuth)) => {
            eprintln!("ci-dispatch: {error}; refusing broker intake");
            std::process::exit(1);
        }
        Err(myelin_storage::blob::BlobDependencyError::Transient) => {
            eprintln!("ci-dispatch: object-store dependency is temporarily unavailable; starting not-ready");
        }
    }
    let consumers = myelin_ci_dispatch::build_dispatch_consumers(
        git_root,
        blobs.clone(),
        ci_run,
        reserve_outbox,
        dedup,
        dead_letters,
        provider.config().region.clone(),
        minter,
        tokio::runtime::Handle::current(),
    )
    .unwrap_or_else(|e| {
        // Fail LOUD: a service that cannot register its trigger consumer must not boot as a silent
        // shell (that was exactly finding #6). Non-zero exit, never swallowed.
        eprintln!("ci-dispatch: cannot register the ci-dispatch.trigger consumer: {e:?}");
        std::process::exit(1);
    });

    // Intake is pull/ack/NAK/TERM only. The elected cell publisher owns the shared outbox drain;
    // this process cannot claim or mark any outbox row. The server-side filter admits every tenant
    // in this cell but only the git subsystem. Region is a cell property and is checked again by the
    // handler before Git, S3, or PG effects.
    let intake = RecoveringIntake::new(
        JetStreamConsumerConfig::bounded(
            &provider.config().nats_url,
            EVENT_STREAM_NAME,
            EVENT_SUBJECT_ROOT,
            git_intake_filter(),
            EVENT_DURABLE_CONSUMER,
        ),
        blobs,
        tokio::runtime::Handle::current(),
    );
    let delivery_quarantine: Arc<dyn myelin_events::DurableDeliveryQuarantine> = Arc::new(
        myelin_storage::events_durable::DurableDeliveryQuarantineBacking::new(
            provider.db_pool().clone(),
            tokio::runtime::Handle::current(),
        ),
    );

    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
    match run_dispatch_until_shutdown(
        Config::default(),
        outbox,
        consumers,
        Box::new(intake),
        delivery_quarantine,
        shutdown_signal(),
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("ci-dispatch service failed: {e}");
            std::process::exit(1);
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .unwrap_or_else(|error| {
                    eprintln!("ci-dispatch: failed to install SIGTERM handler: {error}");
                    std::process::exit(1);
                });
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("ci-dispatch: failed while waiting for SIGINT: {error}");
                    std::process::exit(1);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    eprintln!("ci-dispatch: SIGTERM stream closed unexpectedly");
                    std::process::exit(1);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("ci-dispatch: failed while waiting for shutdown signal: {error}");
            std::process::exit(1);
        }
    }
}
