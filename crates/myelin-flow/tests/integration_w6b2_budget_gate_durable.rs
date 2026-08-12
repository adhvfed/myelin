#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::{BudgetGate, Wallet};
use myelin_storage::migration::HotTables;
use myelin_storage::reserve_settle::{
    MeteredUnit, MicroUsd, ReservationState, RunId as LedgerRunId,
};
use myelin_storage::reserve_settle_durable::reserve_settle_durable_migrations;
use myelin_storage::SubstrateProvider;
use myelin_tenancy::TenantId;

fn test_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

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

async fn app_provider() -> SubstrateProvider {
    SubstrateProvider::connect(test_config(), 6)
        .await
        .expect("connect to the app-role Postgres required by the durable budget story")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w6b2_budget_gate_durable_reserve_settle_events_on_live_pg() {
    let admin = SubstrateProvider::connect(admin_config(&test_config()), 4)
        .await
        .expect("connect to the Postgres required by the durable budget integration story");
    admin
        .migrate(&reserve_settle_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the cost-ledger migration (0050)");

    let app = app_provider().await;
    let suffix = uniq();
    let tenant = TenantId(format!("01J0BG{suffix}"));
    let run = LedgerRunId::new(format!("bg-run-{suffix}"));

    let gate = BudgetGate::with_pg(Wallet::new(MicroUsd(5_000)), app.clone());
    gate.reserve(&tenant, &run, MicroUsd(1_000))
        .expect("a funded reserve admits through the durable gate");
    assert_eq!(
        gate.balance(),
        MicroUsd(4_000),
        "the wallet is debited by the reserved amount"
    );
    gate.begin(&tenant, &run)
        .expect("begin (durable) marks in-flight");

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
    let outcome = gate
        .settle(&tenant, &run, &units)
        .expect("settle (durable) records the cost events on Pg");
    assert_eq!(
        outcome.cost_events.len(),
        2,
        "one cost event per metered unit on Pg"
    );
    assert_eq!(outcome.billed_total, MicroUsd(400));
    assert_eq!(outcome.refunded, MicroUsd(600));
    assert_eq!(outcome.cost_events[0].unit, "llm.tokens");
    assert_eq!(
        gate.balance(),
        MicroUsd(4_600),
        "settled into the same wallet - only the billed 400 is drawn"
    );
    assert_eq!(
        gate.state_of(&tenant, &run),
        Ok(Some(ReservationState::Settled))
    );
    assert_eq!(
        gate.inflight_interrupt_count(),
        0,
        "0 interrupts (structural)"
    );

    let gate2 = BudgetGate::with_pg(Wallet::new(MicroUsd(5_000)), app_provider().await);
    assert_eq!(
        gate2.state_of(&tenant, &run),
        Ok(Some(ReservationState::Settled)),
        "the settled reservation survived fresh-pool reconstruction"
    );
    let again = gate2
        .settle(&tenant, &run, &units)
        .expect("idempotent re-settle on the fresh gate");
    assert_eq!(
        again.cost_events.len(),
        2,
        "the re-settle re-reads the SAME 2 cost events from PG (they survived reconstruction)"
    );
    assert_eq!(again.billed_total, MicroUsd(400));
    assert_eq!(
        again.refunded,
        MicroUsd(600),
        "the same outcome - no double-charge on the durable re-read"
    );
    assert_eq!(
        gate2.balance(),
        MicroUsd(5_000),
        "the idempotent re-settle does NOT re-credit the fresh wallet (no double-credit)"
    );

    let run_over = LedgerRunId::new(format!("bg-over-{suffix}"));
    gate2
        .reserve(&tenant, &run_over, MicroUsd(100))
        .expect("reserve run_over");
    gate2.begin(&tenant, &run_over).expect("begin run_over");
    let over = gate2
        .settle(
            &tenant,
            &run_over,
            &[MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(700),
                markup: MicroUsd(300),
            }],
        )
        .expect("settle run_over");
    assert_eq!(
        over.billed_total,
        MicroUsd(100),
        "settle is capped at the reserved amount on Pg"
    );
    assert_eq!(over.refunded, MicroUsd::ZERO);
}
