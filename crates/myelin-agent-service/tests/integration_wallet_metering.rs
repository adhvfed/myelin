#![cfg(feature = "integration")]

use myelin_agent::{
    AgentRuntime, Conversation, EffectKind, MeteredRuntime, MeteredStep, StepOutcome, Submission,
    TokenUsage, ToolCall, ToolCallId, ToolDef, ToolName, Turn,
};
use myelin_agent_service::{
    price, MockToolExecutor, MockToolSurface, RunOutcomeKind, RunSubstrate, RunTokenRevoker,
    SkeletonAgent, SkeletonError, SkeletonTelemetry, LUNA_RATES,
};
use myelin_config::MyelinConfig;
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter, WfJournal};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::agent_run_gate::AgentRunGate;
use myelin_storage::agent_wallet::{agent_wallet_migrations, AgentWallet, CreditKind, MicroUsd};
use myelin_storage::migration::HotTables;
use myelin_storage::reserve_settle::CostLedger;
use myelin_storage::SubstrateProvider;
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

#[derive(Default)]
struct ProviderMinter;
impl RunTokenMinter for ProviderMinter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        _caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        Ok(RunTokenHandle {
            token: format!("tok:{agent_id}:{run_id}"),
            jti: format!("jti:{agent_id}:{run_id}"),
            ttl_secs,
        })
    }
}

#[derive(Default)]
struct ProviderRevoker {
    revoked: std::sync::Mutex<std::collections::HashSet<String>>,
}
impl RunTokenRevoker for ProviderRevoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        let mut g = self.revoked.lock().unwrap();
        if !g.insert(jti.to_string()) {
            return 0;
        }
        (now_secs - teardown_secs).max(0) as u64
    }
    fn is_dead(&self, jti: &str, _now: i64) -> bool {
        self.revoked.lock().unwrap().contains(jti)
    }
}

fn agent_principal(tenant: &TenantId) -> Principal {
    Principal::stub(
        PrincipalId("psn:agent-7".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("skeleton".into()),
            on_behalf_of: None,
        },
        tenant.clone(),
    )
}

const TEST_USAGE: TokenUsage = TokenUsage::Reported {
    input: 1_000,
    cached_input: 500,
    output: 200,
};

fn tool_def(name: &str) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: "test".into(),
        version: 1,
        input_schema: "{}".into(),
        required_caps: vec![],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: false,
    }
}

fn tool_call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId(format!("call:{name}")),
        name: ToolName(name.into()),
        arguments: serde_json::json!({}),
    }
}

struct MeteredBrain;
impl AgentRuntime for MeteredBrain {
    fn step(&self, conv: &Conversation) -> StepOutcome {
        let model_turns = conv
            .turns
            .iter()
            .filter(|t| matches!(t, Turn::Model(_)))
            .count();
        match model_turns {
            0 => StepOutcome::UseTools(vec![tool_call("search")]),
            1 => StepOutcome::UseTools(vec![tool_call("read")]),
            _ => StepOutcome::Submit(Submission("done".into())),
        }
    }
}
impl MeteredRuntime for MeteredBrain {
    fn step_metered(&self, conv: &Conversation) -> MeteredStep {
        MeteredStep {
            outcome: self.step(conv),
            usage: TEST_USAGE,
        }
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metered_run_debits_the_durable_wallet_per_turn() {
    let Some(_admin) = migrate_admin().await else {
        return;
    };
    let app = SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role");
    let region_s = app.config().region.clone();
    let tenant = TenantId(format!("01J0METER{}", uniq()));
    let wallet = AgentWallet::new(app.clone());

    let per_turn = price(&TEST_USAGE, &LUNA_RATES)
        .expect("prices without overflow")
        .total()
        .expect("total fits");
    let three_turns = MicroUsd(per_turn.0 * 3);

    let topup = MicroUsd(1_000_000);
    wallet
        .credit(&tenant, topup, CreditKind::Topup, None)
        .expect("topup seeds the wallet");

    let brain = MeteredBrain;
    let agent_loop = SkeletonAgent::new();
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let outbox = myelin_events::OutboxStore::new();
    let mut tele = SkeletonTelemetry::new();
    let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
    let exec = MockToolExecutor::new();
    let run_id = "Rmeter-live";
    let mut sub = RunSubstrate {
        tenant: tenant.clone(),
        region: Region(region_s.clone()),
        agent: agent_principal(&tenant),
        run_id: run_id.into(),
        minter_token: Arc::new(ProviderMinter),
        agent_id: "psn:agent-7".into(),
        caveats: DelegationCaveats(vec![]),
        token_ttl_secs: 300,
        revoker: &revoker,
        catalogue: &cat,
        executor: &exec,
        wallet: Some(&wallet),
        gate: &mut gate,
        ledger: &mut ledger,
        available: MicroUsd(100),
        estimate: MicroUsd(10),
        outbox: &outbox,
        minter: Arc::new(myelin_events::MonotonicMinter::new()),
        journal: WfJournal::new(),
        now_secs: 1000,
    };

    let out = agent_loop
        .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
        .expect("the metered run completes");
    assert!(out.0.contains("completed"), "the run completed: {out:?}");

    assert_eq!(
        wallet.balance(&tenant),
        MicroUsd(topup.0 - three_turns.0),
        "balance dropped by exactly 3 × (wholesale + markup)"
    );
    assert_eq!(
        debit_row_count(app.db_pool(), &tenant.0, &region_s, run_id).await,
        3,
        "one run-linked debit ledger row per turn"
    );
    assert_eq!(tele.charged_micro(), three_turns.0);
    assert!(tele.ledger_balanced(), "reserved == settled is unaffected");
    assert_eq!(tele.reserved(), 10);
    assert_eq!(tele.settled(), 10);
    assert_eq!(tele.traces_written(), 1);
    assert_eq!(tele.tokens_revoked(), 1);
    assert_eq!(tele.runs_completed(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metered_run_dry_durable_wallet_halts_gracefully() {
    let Some(_admin) = migrate_admin().await else {
        return;
    };
    let app = SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role");
    let region_s = app.config().region.clone();
    let tenant = TenantId(format!("01J0METERDRY{}", uniq()));
    let wallet = AgentWallet::new(app.clone());

    let per_turn = price(&TEST_USAGE, &LUNA_RATES).unwrap().total().unwrap();
    let two_turns = MicroUsd(per_turn.0 * 2);
    wallet
        .credit(&tenant, two_turns, CreditKind::Topup, None)
        .expect("topup seeds exactly two turns");

    let brain = MeteredBrain;
    let agent_loop = SkeletonAgent::new();
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let outbox = myelin_events::OutboxStore::new();
    let mut tele = SkeletonTelemetry::new();
    let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
    let exec = MockToolExecutor::new();
    let run_id = "Rmeter-dry";
    let mut sub = RunSubstrate {
        tenant: tenant.clone(),
        region: Region(region_s.clone()),
        agent: agent_principal(&tenant),
        run_id: run_id.into(),
        minter_token: Arc::new(ProviderMinter),
        agent_id: "psn:agent-7".into(),
        caveats: DelegationCaveats(vec![]),
        token_ttl_secs: 300,
        revoker: &revoker,
        catalogue: &cat,
        executor: &exec,
        wallet: Some(&wallet),
        gate: &mut gate,
        ledger: &mut ledger,
        available: MicroUsd(100),
        estimate: MicroUsd(10),
        outbox: &outbox,
        minter: Arc::new(myelin_events::MonotonicMinter::new()),
        journal: WfJournal::new(),
        now_secs: 2000,
    };

    let err = agent_loop
        .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
        .expect_err("the underfunded run halts at the spend cap");
    assert!(
        matches!(err, SkeletonError::WalletSpendCapReached { .. }),
        "graceful spend-cap halt: {err}"
    );
    assert_eq!(
        wallet.balance(&tenant),
        MicroUsd::ZERO,
        "balance drained to exactly 0 by the two funded turns (never negative)"
    );
    assert_eq!(
        debit_row_count(app.db_pool(), &tenant.0, &region_s, run_id).await,
        2,
        "exactly two debit rows - the refused turn wrote nothing"
    );
    assert_eq!(tele.charged_micro(), two_turns.0);
    assert_eq!(tele.tokens_revoked(), 1, "torn down on the spend-cap path");
    assert_eq!(tele.traces_written(), 0);
    assert_eq!(tele.runs_completed(), 0);
}
