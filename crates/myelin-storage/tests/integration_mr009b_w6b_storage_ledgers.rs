//! # MR-009b Wave 6b — the durable in-crate storage ledgers, proven against LIVE Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build/test --workspace` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (or the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     AWS_DEFAULT_REGION=fr-par cargo test -p myelin-storage --features integration \
//!       --test integration_mr009b_w6b_storage_ledgers -- --nocapture
//!
//! It proves the W6b deliverables — each MUST hit the live DB (a pass on the in-memory model would
//! NOT count):
//!   A. **`DurableCostLedger` (SI-021) — durability + the four invariants on Pg:** a reserve→begin→
//!      settle cycle survives reconstruction from a FRESH pool (kill-9 equivalent); the idempotent
//!      exact double-settle records NO further events while divergent units are refused; a
//!      caller-owned transaction can roll settlement back atomically; settle-capped;
//!      never-interrupt-in-flight (cancel of an in-flight run refuses; interrupt count 0). The
//!      `cost_reservation`/`cost_event` tables are FORCE-RLS (tenant-owned billing data) — the app
//!      role drives them through the MR-022 `with_tenant_tx` convention.
//!   B. **`DurablePostPitLedger` (P-ST-14) — durability + post-PIT selection on Pg:** an erasure
//!      recorded through one instance is selected by `erasures_completed_after` on a FRESH instance;
//!      a pre-PIT erasure is NOT selected.
//!   C. **`DurableRestoreErasureLedger` + the R1 §7.6 fold-in — restore-inside-window CAUGHT:** an
//!      erasure recorded with a COMPLETION offset AFTER the restore PIT (inside the backup window) is
//!      caught/refused by the restore-verify gate reading the DURABLE ledger from a FRESH instance.
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::encryption::SubjectId;
use myelin_storage::migration::HotTables;
use myelin_storage::reerase::PostRestoreErasureLedger;
use myelin_storage::reerase_durable::{post_pit_durable_migrations, DurablePostPitLedger};
use myelin_storage::reserve_settle::{
    MeteredUnit, MinorUnits, ReservationState, RunId, SettleError,
};
use myelin_storage::reserve_settle_durable::{
    reserve_settle_durable_migrations, DurableCostLedger,
};
use myelin_storage::restore_verify::{ErasureLedger, GateFailure, GateInputs, RestoreVerifyGate};
use myelin_storage::restore_verify_durable::{
    restore_verify_durable_migrations, DurableRestoreErasureLedger,
};
use myelin_storage::{
    BlobPresence, ContinuousArchiver, KmsEngine, RestoredObject, SourceLog, SubstrateProvider,
    WalRow, WalSegment,
};
use myelin_tenancy::TenantId;

fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Apply the three W6b durable migrations (0050 cost + 0051 restore-erasure + 0052 post-pit) as the
/// admin/owner role. `None` (SKIP) if unreachable.
async fn migrate_admin() -> Option<SubstrateProvider> {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return None;
        }
    };
    admin
        .migrate(&reserve_settle_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the cost-ledger migration (0050)");
    admin
        .migrate(&restore_verify_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the restore-erasure-ledger migration (0051)");
    admin
        .migrate(&post_pit_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the post-pit-erasure-ledger migration (0052)");
    Some(admin)
}

/// A FRESH app-role provider (NOBYPASSRLS, reset-on-release) over a NEW pool — the kill-9-equivalent
/// reconstruction seam (new connections, nothing carried in-process).
async fn app_provider() -> SubstrateProvider {
    SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role")
}

/// A reachable archiver (base at 0, tail at `tail`) — the restore-verify gate's PITR source.
fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment {
        end_offset: 0,
        committed_at: 0,
    })
    .unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment {
        end_offset: tail,
        committed_at: 10,
    })
    .unwrap();
    arch
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mr009b_w6b_storage_ledgers_durable() {
    let Some(admin) = migrate_admin().await else {
        return;
    };
    let app = app_provider().await;
    let region = app.config().region.clone();
    let suffix = uniq();

    // The erasure-record tables (NO RLS) persist across runs (durable!). Clean this region so the
    // gate's region-wide `records()` read is deterministic for THIS run.
    for tbl in ["restore_erasure_ledger", "post_pit_erasure_ledger"] {
        sqlx::query(&format!("DELETE FROM {tbl} WHERE region = $1"))
            .bind(&region)
            .execute(admin.db_pool())
            .await
            .expect("clean erasure-record table for this region");
    }

    // =============================================================================================
    // A — DurableCostLedger: durability + the four invariants on live Pg (FORCE-RLS, with_tenant_tx).
    // =============================================================================================
    let tenant = TenantId(format!("01J0COST{suffix}"));
    let run = RunId::new(format!("run-{suffix}"));
    let cost1 = DurableCostLedger::new(app.clone());
    cost1
        .reserve(
            tenant.clone(),
            run.clone(),
            MinorUnits(1_000),
            MinorUnits(5_000),
        )
        .expect("a funded reserve admits on Pg");
    cost1.begin(&tenant, &run).expect("begin on Pg");
    let units = vec![
        MeteredUnit {
            unit: "llm.tokens",
            wholesale: MinorUnits(120),
            markup: MinorUnits(30),
        },
        MeteredUnit {
            unit: "ci.minute",
            wholesale: MinorUnits(200),
            markup: MinorUnits(50),
        },
    ];
    let outcome = cost1.settle(&tenant, &run, &units).expect("settle on Pg");
    assert_eq!(
        outcome.cost_events.len(),
        2,
        "one cost event per metered unit"
    );
    assert_eq!(outcome.billed_total, MinorUnits(400));
    assert_eq!(outcome.refunded, MinorUnits(600));

    // Durability: a FRESH ledger over a FRESH pool reads the settled reservation + its events back.
    let cost2 = DurableCostLedger::new(app_provider().await);
    assert_eq!(
        cost2.state_of(&tenant, &run),
        Some(ReservationState::Settled),
        "the settled reservation survived reconstruction from a fresh pool"
    );
    assert_eq!(
        cost2.cost_events_for(&tenant, &run).len(),
        2,
        "the durable cost events survived reconstruction"
    );

    // Invariant 4 — idempotent double-settle (the recorded_outcome SQL RE-READ): SAME outcome, NO
    // further events (never a double-charge), on the fresh instance.
    let again = cost2
        .settle(&tenant, &run, &units)
        .expect("re-settle on Pg");
    assert_eq!(again.billed_total, MinorUnits(400));
    assert_eq!(again.refunded, MinorUnits(600));
    assert_eq!(
        cost2.cost_events_for(&tenant, &run).len(),
        2,
        "a double-settle records NO further cost events on Pg (no double-charge)"
    );
    let mut divergent_units = units.clone();
    divergent_units[0].wholesale = MinorUnits(121);
    assert_eq!(
        cost2.settle(&tenant, &run, &divergent_units),
        Err(SettleError::UsageDivergence),
        "an acknowledgement-loss retry cannot alter durable metered units"
    );
    assert_eq!(cost2.cost_events_for(&tenant, &run).len(), 2);

    // Caller-transaction API: a rollback leaves both the reservation and event log untouched; the
    // same exact operation can then commit in a later scoped transaction.
    let run_tx = RunId::new(format!("run-tx-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_tx.clone(),
            MinorUnits(1_000),
            MinorUnits(9_000),
        )
        .expect("reserve run_tx");
    cost2.begin(&tenant, &run_tx).expect("begin run_tx");
    let tx_units = vec![MeteredUnit {
        unit: "ci.minute",
        wholesale: MinorUnits(200),
        markup: MinorUnits(50),
    }];
    let mut tx = app
        .db_pool()
        .begin()
        .await
        .expect("begin rollback proof tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), \
                set_config('myelin.region', $2, true)",
    )
    .bind(&tenant.0)
    .bind(&region)
    .execute(&mut *tx)
    .await
    .expect("scope rollback proof tx");
    cost2
        .settle_in_tx(&mut tx, &tenant, &run_tx, &tx_units)
        .await
        .expect("settle inside caller tx");
    tx.rollback().await.expect("roll back settlement");
    assert_eq!(
        cost2.state_of(&tenant, &run_tx),
        Some(ReservationState::InFlight),
        "caller rollback preserves the in-flight reservation"
    );
    assert!(cost2.cost_events_for(&tenant, &run_tx).is_empty());

    let mut tx = app.db_pool().begin().await.expect("begin commit proof tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), \
                set_config('myelin.region', $2, true)",
    )
    .bind(&tenant.0)
    .bind(&region)
    .execute(&mut *tx)
    .await
    .expect("scope commit proof tx");
    let committed = cost2
        .settle_in_tx(&mut tx, &tenant, &run_tx, &tx_units)
        .await
        .expect("settle inside committed caller tx");
    tx.commit().await.expect("commit settlement");
    assert_eq!(committed.billed_total, MinorUnits(250));
    assert_eq!(
        cost2.state_of(&tenant, &run_tx),
        Some(ReservationState::Settled)
    );
    assert_eq!(cost2.cost_events_for(&tenant, &run_tx).len(), 1);

    // Caller-transaction cancellation: skipped work refunds only with its companion accounting
    // transaction, rolls back cleanly, and exactly replays after commit.
    let run_skip = RunId::new(format!("run-skip-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_skip.clone(),
            MinorUnits(700),
            MinorUnits(9_000),
        )
        .expect("reserve skipped run");
    let mut tx = app.db_pool().begin().await.expect("begin skip rollback tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), \
                set_config('myelin.region', $2, true)",
    )
    .bind(&tenant.0)
    .bind(&region)
    .execute(&mut *tx)
    .await
    .expect("scope skip rollback tx");
    assert_eq!(
        cost2
            .cancel_unstarted_in_tx(&mut tx, &tenant, &run_skip)
            .await
            .expect("cancel skipped run in caller tx"),
        MinorUnits(700)
    );
    tx.rollback().await.expect("roll back skip cancellation");
    assert_eq!(
        cost2.state_of(&tenant, &run_skip),
        Some(ReservationState::Reserved)
    );

    let mut tx = app.db_pool().begin().await.expect("begin skip commit tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), \
                set_config('myelin.region', $2, true)",
    )
    .bind(&tenant.0)
    .bind(&region)
    .execute(&mut *tx)
    .await
    .expect("scope skip commit tx");
    for _ in 0..2 {
        assert_eq!(
            cost2
                .cancel_unstarted_in_tx(&mut tx, &tenant, &run_skip)
                .await
                .expect("exact cancellation replay"),
            MinorUnits(700)
        );
    }
    tx.commit().await.expect("commit skip cancellation");
    assert_eq!(
        cost2.state_of(&tenant, &run_skip),
        Some(ReservationState::Cancelled)
    );

    // Invariant 3 — settle-capped-at-reserved on Pg: an over-run is clamped to the reservation.
    let run_over = RunId::new(format!("run-over-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_over.clone(),
            MinorUnits(100),
            MinorUnits(9_000),
        )
        .expect("reserve run_over");
    cost2.begin(&tenant, &run_over).expect("begin run_over");
    let over = cost2
        .settle(
            &tenant,
            &run_over,
            &[MeteredUnit {
                unit: "llm.tokens",
                wholesale: MinorUnits(500),
                markup: MinorUnits(500),
            }],
        )
        .expect("settle run_over");
    assert_eq!(
        over.billed_total,
        MinorUnits(100),
        "settle is capped at the reserved amount on Pg"
    );
    assert_eq!(over.refunded, MinorUnits::ZERO);

    // Invariant 1 — never-interrupt-in-flight on Pg: cancel of an in-flight run REFUSES; count 0.
    let run_live = RunId::new(format!("run-live-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_live.clone(),
            MinorUnits(500),
            MinorUnits(9_000),
        )
        .expect("reserve run_live");
    cost2.begin(&tenant, &run_live).expect("begin run_live");
    assert_eq!(
        cost2.cancel_unstarted(&tenant, &run_live),
        Err(SettleError::NoSuchReservation),
        "an in-flight run is NEVER torn down on Pg"
    );
    assert_eq!(
        cost2.state_of(&tenant, &run_live),
        Some(ReservationState::InFlight),
        "the in-flight run is untouched"
    );
    assert_eq!(
        cost2.inflight_interrupt_count(),
        0,
        "0 interrupts (structural)"
    );

    // =============================================================================================
    // B — DurablePostPitLedger: durability + the post-PIT `completed_after` selection on live Pg.
    // =============================================================================================
    let pp_tenant = TenantId(format!("01J0PP{suffix}"));
    let subj_post = SubjectId::new(format!("subj-post-{suffix}"));
    let subj_pre = SubjectId::new(format!("subj-pre-{suffix}"));
    let pp1 = DurablePostPitLedger::new(app.clone());
    pp1.record(&pp_tenant, &subj_post, 140)
        .await
        .expect("record a post-PIT erasure (offset 140)");
    pp1.record(&pp_tenant, &subj_pre, 60)
        .await
        .expect("record a pre-PIT erasure (offset 60)");

    // A FRESH instance selects exactly the post-PIT erasure (offset > 100), not the pre-PIT one.
    let pp2 = DurablePostPitLedger::new(app_provider().await);
    let after = pp2.erasures_completed_after(100);
    let ids: Vec<String> = after.iter().map(|r| r.subject.0.clone()).collect();
    assert!(
        ids.contains(&subj_post.0),
        "the post-PIT erasure survived + is selected on the fresh instance: {ids:?}"
    );
    assert!(
        !ids.contains(&subj_pre.0),
        "the pre-PIT erasure is NOT selected: {ids:?}"
    );

    // =============================================================================================
    // C — DurableRestoreErasureLedger + the R1 §7.6 fold-in: restore-inside-window CAUGHT on Pg.
    // =============================================================================================
    let windowed = TenantId(format!("01J0WIN{suffix}"));
    let led1 = ErasureLedger::with_pg(DurableRestoreErasureLedger::new(app.clone()));
    // Completed at offset 140 — AFTER the restore PIT T=100 (inside the backup window). The backup
    // predates the erasure completion, so it physically holds the pre-erasure key.
    led1.record_erased_at(windowed.clone(), 140);

    // The gate reads the DURABLE ledger from a FRESH instance (durability) and must CATCH the
    // restore-inside-window resurrection (§7.6) — the bare gate has no re-erasure pass to re-kill it.
    let led2 = ErasureLedger::with_pg(DurableRestoreErasureLedger::new(app_provider().await));
    let kms = KmsEngine::new();
    let arch = reachable_archiver(300);
    let rows: Vec<WalRow> = vec![];
    let objects: Vec<RestoredObject> = vec![];
    let source = SourceLog::new();
    let _presence = BlobPresence::new();
    let inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &led2,
    };
    let verdict = RestoreVerifyGate::new().run(&inputs);
    assert!(
        !verdict.is_green(),
        "the restore-inside-window resurrection MUST be CAUGHT by the durable-ledger-driven gate"
    );
    assert!(
        matches!(
            verdict.failure(),
            Some(GateFailure::ErasureResurrected { tenant }) if tenant == &windowed
        ),
        "the gate refuses the restore-inside-window resurrection for the windowed tenant: {:?}",
        verdict.failure()
    );

    // =============================================================================================
    // Cleanup — the erasure-record tables (NO RLS) via admin; the FORCE-RLS cost rows use unique
    // tenants (no cross-run pollution) so they are left as durable evidence.
    // =============================================================================================
    for tbl in ["restore_erasure_ledger", "post_pit_erasure_ledger"] {
        let _ = sqlx::query(&format!("DELETE FROM {tbl} WHERE region = $1"))
            .bind(&region)
            .execute(admin.db_pool())
            .await;
    }
}
