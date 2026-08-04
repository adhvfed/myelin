//! # LIVE end-to-end drill — real Luna brain → the driving loop → per-turn wallet metering → answer.
//!
//! The full hosted-agent composition proven against BOTH real dependencies:
//!   - the **real Luna brain** ([`LunaClient::from_env`], `OPENAI_API_KEY` injected via `fed`), and
//!   - the **real durable wallet** on live Postgres `:5433` (the same stack + gating as
//!     `myelin-agent-service`'s `integration_wallet_metering`).
//!
//! Run it (key + DB present):
//! ```text
//! DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!   AWS_DEFAULT_REGION=fr-par \
//!   cargo test -p myelin-agent-host --test e2e_luna_live -- --ignored --nocapture
//! ```
//! It skips GRACEFULLY (no failure) when `OPENAI_API_KEY` is absent or the DB is unreachable, and is
//! `#[ignore]` so the default hermetic suite never reaches the network — the mock sibling
//! (`e2e_mock_metered_run.rs`) proves the identical composition + F1 + metering path with no network.
//!
//! The API key rides only in the Luna `Authorization` header (never logged); this drill prints the
//! real answer + the wallet debit, never the key.

use std::sync::Arc;

use myelin_agent_host::{AgentHost, CreditKind, LlmRunTask, MicroUsd, RunSubstrateWiring};
use myelin_agent_model::{LunaClient, ModelError};
use myelin_config::MyelinConfig;
use myelin_events::{MonotonicMinter, OutboxStore};
use myelin_flow::WfJournal;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::agent_wallet::agent_wallet_migrations;
use myelin_storage::migration::HotTables;
use myelin_storage::reserve_settle::CostLedger;
use myelin_storage::SubstrateProvider;
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

/// Apply the agent-wallet migration (0080) as the admin/owner role. `None` (SKIP) if unreachable.
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

/// Count the run-linked `debit` rows for `(tenant, region)` directly in SQL (the independent oracle).
async fn debit_row_count(pool: &sqlx::PgPool, tenant: &str, region: &str, run_id: &str) -> i64 {
    let mut tx = pool.begin().await.expect("begin count tx");
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)")
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

/// **The headline live drill: seed a durable wallet, drive a real Luna run through
/// [`AgentHost::run_llm_agent`], and assert it SUBMITTED a real text answer, the wallet DROPPED
/// (metered — a run-linked debit row), and the run tore down cleanly.**
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "hits real Luna + live Postgres; requires OPENAI_API_KEY and the dev DB (:5433)"]
async fn live_luna_run_is_metered_end_to_end() {
    // Gate 1 — the real brain (the key rides only in the Luna Authorization header; never printed).
    let luna = match LunaClient::from_env() {
        Ok(c) => c,
        Err(ModelError::MissingApiKey) => {
            eprintln!("SKIP: OPENAI_API_KEY not set (no real-brain run)");
            return;
        }
        Err(e) => panic!("unexpected Luna construction error: {e}"),
    };
    // Gate 2 — the live durable wallet DB.
    let Some(_admin) = migrate_admin().await else {
        return;
    };
    let app = SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role");
    let region_s = app.config().region.clone();
    let tenant = TenantId(format!("01J0HOSTLUNA{}", uniq()));

    // The F2-airtight composition root: wallet + region both from THIS provider.
    let host = AgentHost::new(app.clone());
    assert_eq!(host.region().0, region_s, "F2: host region == provider region");

    // Seed $1.00 (a few cents cap is plenty for a one-word Luna answer).
    let topup = MicroUsd(1_000_000);
    host.wallet()
        .credit(&tenant, topup, CreditKind::Topup, None)
        .expect("topup seeds the wallet");

    // The in-memory nominal substrate (the reserve/settle nominal layer; the wallet is the real
    // money and is durable).
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

    // F1: the durable wallet is threaded non-optionally — a real paid run is always billed.
    let report = host
        .run_llm_agent(&task, &mut wiring, Box::new(luna))
        .expect("the live Luna run completes");

    // The run SUBMITTED a non-empty text answer from REAL Luna.
    eprintln!("LIVE Luna answer: {:?}", report.answer);
    eprintln!(
        "LIVE wallet: topup={} charged_micro={} balance={}",
        topup.0,
        report.charged_micro,
        host.wallet().balance(&tenant).0
    );
    assert!(!report.answer.trim().is_empty(), "real Luna produced an answer");
    assert!(report.outcome.0.contains("completed"), "the run completed cleanly");

    // The wallet DROPPED — metered per turn (real billing).
    assert!(report.charged_micro > 0, "at least one turn was priced + debited");
    assert_eq!(
        host.wallet().balance(&tenant),
        MicroUsd(topup.0 - report.charged_micro),
        "balance dropped by exactly the charged amount"
    );
    // At least one run-linked debit row landed in the immutable ledger (the independent SQL oracle).
    assert!(
        debit_row_count(app.db_pool(), &tenant.0, &region_s, run_id).await >= 1,
        "a run-linked debit row was written"
    );

    // The run tore down cleanly: a balanced reserve/settle ledger, a trace, the token revoked.
    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.traces_written(), 1);
    assert_eq!(report.telemetry.tokens_revoked(), 1, "per-run token revoked on teardown");
    assert_eq!(report.telemetry.runs_completed(), 1);
}
