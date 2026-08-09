use std::sync::Arc;

use myelin_agent_host::{
    dispatch_test_run_with_synthetic_identity, CreditKind, LlmRunTask, MicroUsd,
    RunSubstrateWiring, Tools,
};
use myelin_agent_model::{LunaClient, ModelError};
use myelin_config::MyelinConfig;
use myelin_events::{MonotonicMinter, OutboxStore};
use myelin_flow::WfJournal;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::agent_wallet::{agent_wallet_migrations, AgentWallet};
use myelin_storage::migration::HotTables;
use myelin_storage::reserve_settle::CostLedger;
use myelin_storage::SubstrateProvider;
use myelin_tenancy::{Region, TenantId};

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
        .migrate(&agent_wallet_migrations(), &HotTables::none())
        .await
        .expect("apply the agent-wallet migration (0080)");
    Some(admin)
}

async fn debit_row_count(pool: &sqlx::PgPool, tenant: &str, region: &str, run_id: &str) -> i64 {
    let mut tx = pool.begin().await.expect("begin count tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *tx)
    .await
    .expect("scope count tx");
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_wallet_ledger \
         WHERE tenant_id = $1 AND region = $2 AND kind = 'debit' AND run_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await
    .expect("count the debit rows");
    tx.commit().await.expect("commit count tx");
    n
}

fn agent_principal(tenant: &TenantId) -> Principal {
    Principal::stub(
        PrincipalId("psn:host-agent".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("host".into()),
            on_behalf_of: None,
        },
        tenant.clone(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "hits real Luna + live Postgres; requires OPENAI_API_KEY and the dev DB (:5433)"]
async fn live_luna_run_is_metered_end_to_end() {
    let luna = match LunaClient::from_env() {
        Ok(c) => c,
        Err(ModelError::MissingApiKey) => {
            eprintln!("SKIP: OPENAI_API_KEY not set (no real-brain run)");
            return;
        }
        Err(e) => panic!("unexpected Luna construction error: {e}"),
    };
    let Some(_admin) = migrate_admin().await else {
        return;
    };
    let app = SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role");
    let region_s = app.config().region.clone();
    let tenant = TenantId(format!("01J0HOSTLUNA{}", uniq()));

    let wallet = AgentWallet::new(app.clone());
    let region = Region(region_s.clone());

    let topup = MicroUsd(1_000_000);
    wallet
        .credit(&tenant, topup, CreditKind::Topup, None)
        .expect("topup seeds the wallet");

    let mut ledger = CostLedger::new();
    let outbox = OutboxStore::new();
    let mut wiring = RunSubstrateWiring {
        ledger: &mut ledger,
        outbox: &outbox,
        id_minter: Arc::new(MonotonicMinter::new()),
        journal: WfJournal::new(),
    };

    let run_id = "Rluna-live";
    let task = LlmRunTask::new(
        tenant.clone(),
        agent_principal(&tenant),
        "psn:host-agent",
        run_id,
        "You are a terse assistant. Answer in a single word.",
        "Reply with the single word: ready.",
    )
    .with_max_output_tokens(16)
    .with_now_secs(1000);

    let report = dispatch_test_run_with_synthetic_identity(
        &wallet,
        region,
        &task,
        &mut wiring,
        Box::new(luna),
        Tools::none(),
    )
    .expect("the live Luna run completes");

    eprintln!("LIVE Luna answer: {:?}", report.answer);
    eprintln!(
        "LIVE wallet: topup={} charged_micro={} balance={}",
        topup.0,
        report.charged_micro,
        wallet.balance(&tenant).0
    );
    assert!(
        !report.answer.trim().is_empty(),
        "real Luna produced an answer"
    );
    assert!(
        report.outcome.0.contains("completed"),
        "the run completed cleanly"
    );

    assert!(
        report.charged_micro > 0,
        "at least one turn was priced + debited"
    );
    assert_eq!(
        wallet.balance(&tenant),
        MicroUsd(topup.0 - report.charged_micro),
        "balance dropped by exactly the charged amount"
    );
    assert!(
        debit_row_count(app.db_pool(), &tenant.0, &region_s, run_id).await >= 1,
        "a run-linked debit row was written"
    );

    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.traces_written(), 1);
    assert_eq!(
        report.telemetry.tokens_revoked(),
        1,
        "per-run token revoked on teardown"
    );
    assert_eq!(report.telemetry.runs_completed(), 1);
}
