#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::agent_run_gate::{AgentRunGate, DispatchError};
use myelin_storage::encryption::SubjectId;
use myelin_storage::migration::HotTables;
use myelin_storage::reerase::PostRestoreErasureLedger;
use myelin_storage::reerase_durable::{post_pit_durable_migrations, DurablePostPitLedger};
use myelin_storage::reserve_settle::{
    CostLedger, MeteredUnit, MicroUsd, ReservationState, ReserveError, RunId, SettleError,
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

async fn app_provider() -> SubstrateProvider {
    SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role")
}

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

    for tbl in ["restore_erasure_ledger", "post_pit_erasure_ledger"] {
        sqlx::query(&format!("DELETE FROM {tbl} WHERE region = $1"))
            .bind(&region)
            .execute(admin.db_pool())
            .await
            .expect("clean erasure-record table for this region");
    }

    let tenant = TenantId(format!("01J0COST{suffix}"));
    let run = RunId::new(format!("run-{suffix}"));
    let cost1 = DurableCostLedger::new(app.clone());
    cost1
        .reserve(
            tenant.clone(),
            run.clone(),
            MicroUsd(1_000),
            MicroUsd(5_000),
        )
        .expect("a funded reserve admits on Pg");
    cost1.begin(&tenant, &run).expect("begin on Pg");
    let units = vec![
        MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        },
        MeteredUnit {
            unit: "ci.minute",
            wholesale: MicroUsd(200),
            markup: MicroUsd(50),
        },
    ];
    let outcome = cost1.settle(&tenant, &run, &units).expect("settle on Pg");
    assert_eq!(
        outcome.cost_events.len(),
        2,
        "one cost event per metered unit"
    );
    assert_eq!(outcome.billed_total, MicroUsd(400));
    assert_eq!(outcome.refunded, MicroUsd(600));

    let cost2 = DurableCostLedger::new(app_provider().await);
    assert_eq!(
        cost2.state_of(&tenant, &run),
        Ok(Some(ReservationState::Settled)),
        "the settled reservation survived reconstruction from a fresh pool"
    );
    assert_eq!(
        cost2.cost_events_for(&tenant, &run).unwrap().len(),
        2,
        "the durable cost events survived reconstruction"
    );

    let again = cost2
        .settle(&tenant, &run, &units)
        .expect("re-settle on Pg");
    assert_eq!(again.billed_total, MicroUsd(400));
    assert_eq!(again.refunded, MicroUsd(600));
    assert_eq!(
        cost2.cost_events_for(&tenant, &run).unwrap().len(),
        2,
        "a double-settle records NO further cost events on Pg (no double-charge)"
    );
    let mut divergent_units = units.clone();
    divergent_units[0].wholesale = MicroUsd(121);
    assert_eq!(
        cost2.settle(&tenant, &run, &divergent_units),
        Err(SettleError::UsageDivergence),
        "an acknowledgement-loss retry cannot alter durable metered units"
    );
    assert_eq!(cost2.cost_events_for(&tenant, &run).unwrap().len(), 2);

    assert_eq!(
        cost2.outstanding_reservations(&tenant),
        Ok(MicroUsd::ZERO),
        "a fully-settled tenant has zero outstanding on Pg"
    );
    let run_out_a = RunId::new(format!("run-out-a-{suffix}"));
    let run_out_b = RunId::new(format!("run-out-b-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_out_a.clone(),
            MicroUsd(300),
            MicroUsd(9_000),
        )
        .expect("reserve run_out_a");
    assert_eq!(
        cost2.outstanding_reservations(&tenant),
        Ok(MicroUsd(300)),
        "a Reserved row counts toward outstanding on Pg"
    );
    cost2
        .reserve(
            tenant.clone(),
            run_out_b.clone(),
            MicroUsd(500),
            MicroUsd(9_000),
        )
        .expect("reserve run_out_b");
    cost2.begin(&tenant, &run_out_b).expect("begin run_out_b");
    assert_eq!(
        cost2.outstanding_reservations(&tenant),
        Ok(MicroUsd(800)),
        "Reserved (300) + InFlight (500) = 800 outstanding on Pg"
    );
    cost2
        .begin(&tenant, &run_out_a)
        .expect("begin run_out_a before settle");
    cost2
        .settle(&tenant, &run_out_a, &[])
        .expect("settle run_out_a");
    assert_eq!(
        cost2.outstanding_reservations(&tenant),
        Ok(MicroUsd(500)),
        "a Settled row is excluded from outstanding on Pg"
    );
    let other_tenant = TenantId(format!("01J0COSTX{suffix}"));
    cost2
        .reserve(
            other_tenant.clone(),
            RunId::new(format!("run-out-x-{suffix}")),
            MicroUsd(7_777),
            MicroUsd(9_000),
        )
        .expect("reserve for other tenant");
    assert_eq!(
        cost2.outstanding_reservations(&tenant),
        Ok(MicroUsd(500)),
        "another tenant's outstanding never bleeds into this tenant's (tenant-isolated on Pg)"
    );
    assert_eq!(
        cost2.outstanding_reservations(&other_tenant),
        Ok(MicroUsd(7_777))
    );

    let run_tx = RunId::new(format!("run-tx-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_tx.clone(),
            MicroUsd(1_000),
            MicroUsd(9_000),
        )
        .expect("reserve run_tx");
    cost2.begin(&tenant, &run_tx).expect("begin run_tx");
    let tx_units = vec![MeteredUnit {
        unit: "ci.minute",
        wholesale: MicroUsd(200),
        markup: MicroUsd(50),
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
        Ok(Some(ReservationState::InFlight)),
        "caller rollback preserves the in-flight reservation"
    );
    assert!(cost2.cost_events_for(&tenant, &run_tx).unwrap().is_empty());

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
    assert_eq!(committed.billed_total, MicroUsd(250));
    assert_eq!(
        cost2.state_of(&tenant, &run_tx),
        Ok(Some(ReservationState::Settled))
    );
    assert_eq!(cost2.cost_events_for(&tenant, &run_tx).unwrap().len(), 1);

    let run_skip = RunId::new(format!("run-skip-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_skip.clone(),
            MicroUsd(700),
            MicroUsd(9_000),
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
        MicroUsd(700)
    );
    tx.rollback().await.expect("roll back skip cancellation");
    assert_eq!(
        cost2.state_of(&tenant, &run_skip),
        Ok(Some(ReservationState::Reserved))
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
            MicroUsd(700)
        );
    }
    tx.commit().await.expect("commit skip cancellation");
    assert_eq!(
        cost2.state_of(&tenant, &run_skip),
        Ok(Some(ReservationState::Cancelled))
    );

    let run_over = RunId::new(format!("run-over-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_over.clone(),
            MicroUsd(100),
            MicroUsd(9_000),
        )
        .expect("reserve run_over");
    cost2.begin(&tenant, &run_over).expect("begin run_over");
    let over = cost2
        .settle(
            &tenant,
            &run_over,
            &[MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(500),
                markup: MicroUsd(500),
            }],
        )
        .expect("settle run_over");
    assert_eq!(
        over.billed_total,
        MicroUsd(100),
        "settle is capped at the reserved amount on Pg"
    );
    assert_eq!(over.refunded, MicroUsd::ZERO);

    let run_live = RunId::new(format!("run-live-{suffix}"));
    cost2
        .reserve(
            tenant.clone(),
            run_live.clone(),
            MicroUsd(500),
            MicroUsd(9_000),
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
        Ok(Some(ReservationState::InFlight)),
        "the in-flight run is untouched"
    );
    assert_eq!(
        cost2.inflight_interrupt_count(),
        0,
        "0 interrupts (structural)"
    );

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

    let windowed = TenantId(format!("01J0WIN{suffix}"));
    let led1 = ErasureLedger::with_pg(DurableRestoreErasureLedger::new(app.clone()));
    led1.record_erased_at(windowed.clone(), 140);

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

    for tbl in ["restore_erasure_ledger", "post_pit_erasure_ledger"] {
        let _ = sqlx::query(&format!("DELETE FROM {tbl} WHERE region = $1"))
            .bind(&region)
            .execute(admin.db_pool())
            .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unavailable_cost_ledger_refuses_work_without_crashing_the_worker() {
    let Some(admin) = migrate_admin().await else {
        return;
    };
    let app = app_provider().await;
    let region = app.config().region.clone();
    let tenant = TenantId(format!("01J0OUTAGE{}", uniq()));
    let reserved_run = RunId::new("reserved-before-the-outage");
    let refused_run = RunId::new("refused-during-the-outage");
    let mut ledger = CostLedger::with_pg(app.clone());

    ledger
        .reserve(
            tenant.clone(),
            reserved_run.clone(),
            MicroUsd(100),
            MicroUsd(1_000),
        )
        .expect("the reservation is durable before the simulated outage");
    app.db_pool().close().await;

    assert!(matches!(
        ledger.begin(&tenant, &reserved_run),
        Err(SettleError::StoreUnavailable(_))
    ));
    assert!(matches!(
        ledger.settle(&tenant, &reserved_run, &[]),
        Err(SettleError::StoreUnavailable(_))
    ));
    assert!(matches!(
        ledger.cancel_unstarted(&tenant, &reserved_run),
        Err(SettleError::StoreUnavailable(_))
    ));
    assert!(ledger.reservation_of(&tenant, &reserved_run).is_err());
    assert!(ledger.state_of(&tenant, &reserved_run).is_err());
    assert!(ledger.cost_events_for(&tenant, &reserved_run).is_err());
    assert!(matches!(
        ledger.outstanding_reservations(&tenant),
        Err(ReserveError::StoreUnavailable(_))
    ));

    let mut gate = AgentRunGate::new();
    let refusal = gate
        .dispatch(
            &mut ledger,
            tenant.clone(),
            refused_run,
            MicroUsd(100),
            MicroUsd(1_000),
        )
        .expect_err("an unavailable ledger refuses the run instead of panicking");
    assert!(matches!(refusal, DispatchError::StoreUnavailable(_)));
    assert_eq!(gate.runs_dispatched(), 0, "the run never started");
    assert_eq!(
        gate.reserve_refusals(),
        0,
        "an outage is not misreported as an exhausted wallet"
    );

    for table in ["cost_event", "cost_reservation"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE tenant_id = $1 AND region = $2"
        ))
        .bind(&tenant.0)
        .bind(&region)
        .execute(admin.db_pool())
        .await
        .expect("clean the outage proof rows");
    }
}
