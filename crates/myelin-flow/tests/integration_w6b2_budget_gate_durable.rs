//! # MR-009b W6b2 — the durable `BudgetGate` arm, proven against LIVE Postgres.
//!
//! Closes the W6b honest-STOP: the `myelin-flow::BudgetGate` bookend now reserves/settles against the
//! DURABLE `CostLedger` (`BudgetGate::with_pg` → `CostLedger::with_pg` → `DurableCostLedger` over the
//! FORCE-RLS `cost_reservation`/`cost_event` tables, migration 0050) — NOT the in-memory test double.
//! This drives a full reserve → begin → settle → events cycle THROUGH the gate on live PG and proves:
//!   - **idempotent double-settle** — a re-settle on an already-`Settled` run (the `recorded_outcome`
//!     SQL RE-READ) returns the SAME outcome, records NO further events, and does NOT re-credit the
//!     wallet (never a double-charge / double-credit);
//!   - **settle-capped-at-reserved** — an over-run is clamped to the reservation on Pg;
//!   - **survival across FRESH-pool reconstruction** — a NEW `BudgetGate::with_pg` over a FRESH
//!     provider (a kill-9-equivalent: new connections, nothing carried in-process) reads the settled
//!     reservation + its cost events back from PG.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build/test --workspace` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (migrations need the owner role):
//!
//!   DATABASE_URL=postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin \
//!     AWS_DEFAULT_REGION=fr-par cargo test -p myelin-flow --features integration \
//!       --test integration_w6b2_budget_gate_durable -- --nocapture
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::{BudgetGate, Wallet};
use myelin_storage::migration::HotTables;
use myelin_storage::reserve_settle::{MeteredUnit, MicroUsd, ReservationState, RunId as LedgerRunId};
use myelin_storage::reserve_settle_durable::reserve_settle_durable_migrations;
use myelin_storage::SubstrateProvider;
use myelin_tenancy::TenantId;

/// The admin/owner-role config (the migration role) derived from the app-role dev config.
fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

/// A per-run unique suffix so the FORCE-RLS cost rows never collide across runs.
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

/// A FRESH app-role provider over a NEW pool — the kill-9-equivalent reconstruction seam.
async fn app_provider() -> SubstrateProvider {
    SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w6b2_budget_gate_durable_reserve_settle_events_on_live_pg() {
    // Migrations need the owner role (the cost tables + FORCE-RLS policies, migration 0050).
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&reserve_settle_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the cost-ledger migration (0050)");

    let app = app_provider().await;
    let suffix = uniq();
    let tenant = TenantId(format!("01J0BG{suffix}"));
    let run = LedgerRunId::new(format!("bg-run-{suffix}"));

    // ── Drive reserve → begin → settle → events THROUGH the DURABLE BudgetGate arm on live PG. ──
    let gate = BudgetGate::with_pg(Wallet::new(MicroUsd(5_000)), app.clone());
    gate.reserve(&tenant, &run, MicroUsd(1_000))
        .expect("a funded reserve admits through the durable gate");
    assert_eq!(
        gate.balance(),
        MicroUsd(4_000),
        "the wallet is debited by the reserved amount"
    );
    gate.begin(&tenant, &run).expect("begin (durable) marks in-flight");

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
    // The unit label round-tripped through the DB as an owned String (no Box::leak).
    assert_eq!(outcome.cost_events[0].unit, "llm.tokens");
    // wallet: 5000 − 1000 (reserve) + 600 (refund of 1000−400) = 4600 (only the billed 400 is drawn).
    assert_eq!(
        gate.balance(),
        MicroUsd(4_600),
        "settled into the same wallet — only the billed 400 is drawn"
    );
    assert_eq!(
        gate.state_of(&tenant, &run),
        Some(ReservationState::Settled)
    );
    assert_eq!(gate.inflight_interrupt_count(), 0, "0 interrupts (structural)");

    // ── Survival across FRESH-pool reconstruction + idempotent double-settle. ──
    // A NEW gate over a FRESH provider (new connections) reads the settled state from PG; a re-settle
    // re-reads the SAME events (they survived) and returns the SAME outcome, recording NOTHING new.
    let gate2 = BudgetGate::with_pg(Wallet::new(MicroUsd(5_000)), app_provider().await);
    assert_eq!(
        gate2.state_of(&tenant, &run),
        Some(ReservationState::Settled),
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
        "the same outcome — no double-charge on the durable re-read"
    );
    assert_eq!(
        gate2.balance(),
        MicroUsd(5_000),
        "the idempotent re-settle does NOT re-credit the fresh wallet (no double-credit)"
    );

    // ── settle-capped-at-reserved through the durable BudgetGate arm: reserve 100, bill 1000 → 100. ──
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
