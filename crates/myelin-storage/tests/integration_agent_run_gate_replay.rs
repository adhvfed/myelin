#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::agent_run_gate::{AgentRunGate, DispatchError};
use myelin_storage::migration::HotTables;
use myelin_storage::reserve_settle::{CostLedger, MicroUsd, ReservationState, RunId};
use myelin_storage::reserve_settle_durable::reserve_settle_durable_migrations;
use myelin_storage::SubstrateProvider;
use myelin_tenancy::TenantId;

fn admin_config(config: &MyelinConfig) -> MyelinConfig {
    let mut admin = config.clone();
    admin.database_url = admin
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    admin
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_nanos()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_workflow_resumes_its_exact_cost_reservation() {
    let config = MyelinConfig::dev();
    let admin = match SubstrateProvider::connect(admin_config(&config), 4).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&reserve_settle_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the durable cost ledger");

    let tenant = TenantId(format!("01J0RESUME{}", unique_suffix()));
    let run = RunId::new(format!("agent-run-{}", unique_suffix()));
    let first_provider = SubstrateProvider::connect(config.clone(), 4)
        .await
        .expect("connect the first workflow process");
    let mut first_ledger = CostLedger::with_pg(first_provider);
    let mut first_gate = AgentRunGate::new();
    first_gate
        .dispatch_or_resume_workflow(
            &mut first_ledger,
            tenant.clone(),
            run.clone(),
            MicroUsd(250_000),
            MicroUsd(1_000_000),
        )
        .expect("the first workflow drive reserves its declared budget");
    drop(first_ledger);

    let restarted_provider = SubstrateProvider::connect(config, 4)
        .await
        .expect("restart with a fresh database pool");
    let mut restarted_ledger = CostLedger::with_pg(restarted_provider);
    let mut restarted_gate = AgentRunGate::new();
    let resumed = restarted_gate
        .dispatch_or_resume_workflow(
            &mut restarted_ledger,
            tenant.clone(),
            run.clone(),
            MicroUsd(250_000),
            MicroUsd::ZERO,
        )
        .expect("the restarted workflow recognizes its exact in-flight reservation");

    assert_eq!(resumed.reserved(), MicroUsd(250_000));
    assert_eq!(
        restarted_ledger
            .reservation_of(&tenant, &run)
            .unwrap()
            .unwrap()
            .state,
        ReservationState::InFlight,
        "the restart continued the original row instead of inventing another reservation",
    );
    assert_eq!(
        restarted_gate.runs_dispatched(),
        0,
        "resuming durable work is not counted as a fresh dispatch",
    );
    assert_eq!(
        restarted_gate.dispatch(
            &mut restarted_ledger,
            tenant.clone(),
            run.clone(),
            MicroUsd(250_000),
            MicroUsd(1_000_000),
        ),
        Err(DispatchError::AlreadyDispatched),
        "an ordinary duplicate dispatch remains an error",
    );
    assert_eq!(
        restarted_gate.dispatch_or_resume_workflow(
            &mut restarted_ledger,
            tenant,
            run,
            MicroUsd(250_001),
            MicroUsd(1_000_000),
        ),
        Err(DispatchError::AlreadyDispatched),
        "workflow replay cannot widen the reservation it originally governed",
    );
}
