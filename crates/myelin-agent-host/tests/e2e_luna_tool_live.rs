//! # LIVE tool-executing drill — real Luna INVOKES a governed READ tool → real tenant data → answer.
//!
//! The headline of this slice, against BOTH real dependencies:
//!   - the **real Luna brain** ([`LunaClient::from_env`], `OPENAI_API_KEY` injected via `fed`) that
//!     actually DECIDES to call the `git.read_check_status` tool, and
//!   - the **real Git subsystem read** ([`PgCheckStatusProjection::rows_for_commit`] via
//!     [`GitCheckStatusReadExecutor`]) over the real, RLS-scoped `check_status` table on live
//!     Postgres `:5433` — the SAME stack + gating as `e2e_luna_live`.
//!
//! It SEEDS a readable check-status row (a `ci/build = failure` row for a unique tenant/commit),
//! then drives a real Luna run whose task REQUIRES reading it, and proves the agent CALLED the tool,
//! the executor returned the real seeded data, the agent's answer reflects it, and the wallet was
//! debited for MULTIPLE turns (the tool turn + the answer turn) — metered end-to-end.
//!
//! Run it (key + DB present):
//! ```text
//! DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!   AWS_DEFAULT_REGION=fr-par \
//!   cargo test -p myelin-agent-host --test e2e_luna_tool_live -- --ignored --nocapture
//! ```
//! It skips GRACEFULLY (no failure) when `OPENAI_API_KEY` is absent or the DB is unreachable, and is
//! `#[ignore]` so the default hermetic suite never reaches the network — the mock sibling
//! (`e2e_mock_tool_run.rs`) proves the identical tool path with no network. The API key rides only in
//! the Luna `Authorization` header (never logged); this drill prints the real answer + the tool
//! result + the wallet debit, never the key.

use std::sync::Arc;

use myelin_agent::{ToolCall, ToolCallId, ToolName};
use myelin_agent_host::{
    git_check_status_read_tool_def, git_check_status_read_tool_schema, timestamp_from_epoch,
    AgentHost, CapEnforcingExecutor, CreditKind, GitCheckStatusReadExecutor, LlmRunTask, MicroUsd,
    RunSubstrateWiring, ToolCatalogue, GIT_READ_CHECK_STATUS_TOOL,
};
use myelin_agent_model::{LunaClient, ModelError};
use myelin_agent_service::ToolExecutor;
use myelin_config::MyelinConfig;
use myelin_events::{MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::WfJournal;
use myelin_git::check_status_store::{CHECK_STATUS_TABLE, CREATE_CHECK_STATUS_DDL};
use myelin_identity::{
    IdentityService, ObjectId, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple,
    RevokeTarget, RunId, RuntimeRef, TupleDelta,
};
use myelin_identity_service::{run_token_jti, RevocationStore, RunTokenState, StoreBackedCheck, TupleStore};
use myelin_storage::agent_wallet::agent_wallet_migrations;
use myelin_storage::migration::{HotTables, Migration, Migrations};
use myelin_storage::reserve_settle::CostLedger;
use myelin_storage::{
    cell_root_durable_migrations, identity_durable_migrations, DurableRevocationBacking, SealKey,
    SubstrateProvider, TenantScope,
};
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

/// Apply the agent-wallet migration (0080) AND the Git `check_status` projection migration as the
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
        .migrate(&agent_wallet_migrations(), &HotTables::none())
        .await
        .expect("apply the agent-wallet migration (0080)");
    // Apply ONLY the plain `check_status` table DDL (the PK covers the (tenant, region, repo, commit)
    // read; the CONCURRENTLY commit-index is an unnecessary optimisation for this drill). The id
    // matches the real Git migration, so on a dogfood DB that already has it applied this is a no-op.
    let check_status_table = Migrations::of([Migration::plain_on(
        "git_0014_check_status",
        CREATE_CHECK_STATUS_DDL,
        CHECK_STATUS_TABLE,
    )]);
    admin
        .migrate(&check_status_table, &HotTables::none())
        .await
        .expect("apply the Git check_status projection table (git_0014)");
    // The REAL per-run token mint/revoke needs the durable S7 denylist tables (`revocation` +
    // `run_token_teardown`, via `identity_durable_migrations`) and the cell token-authority signing
    // root table (`cell_token_root`, via `cell_root_durable_migrations`). Idempotent on a dogfood DB
    // that already has them applied.
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the identity durable (S7 revocation + run-token teardown) migrations");
    admin
        .migrate(&cell_root_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the cell token-authority root migration");
    Some(admin)
}

/// Count the run-linked `debit` rows for `(tenant, region, run_id)` directly in SQL (the independent
/// multi-turn-metering oracle).
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

/// Seed ONE readable `check_status` row for `(tenant, region, repo, commit)` through the app role's
/// `with_tenant_tx` (RLS GUCs set → the WITH CHECK passes; a genuinely tenant-scoped write). A
/// distinctive `ci/build = failure` row so the agent's answer is checkable.
async fn seed_check_row(
    app: &SubstrateProvider,
    tenant: &str,
    region: &str,
    repo: &str,
    commit: &str,
) {
    let tenant_s = tenant.to_string();
    let region_s = region.to_string();
    let repo_s = repo.to_string();
    let commit_s = commit.to_string();
    app.with_tenant_tx(tenant, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO check_status (tenant_id, region, repo_ref, commit_oid, \
                    context_provider, context_name, state, required, run_ref, run_attempt, \
                    trust_tier, details_ref, summary_key, summary_args, cost_settled, started_at, \
                    completed_at) \
                 VALUES ($1,$2,$3,$4,'ci','build','failure',true,$5,7,'trusted',$6,\
                    'ci.check.updated','{}'::jsonb,true,'2026-06-22T00:00:00Z','2026-06-22T00:01:00Z') \
                 ON CONFLICT (tenant_id, region, repo_ref, commit_oid, context_provider, context_name) \
                 DO UPDATE SET state = EXCLUDED.state",
            )
            .bind(&tenant_s)
            .bind(&region_s)
            .bind(&repo_s)
            .bind(&commit_s)
            .bind(format!("myelin://{tenant_s}/ci/run/7"))
            .bind(format!("myelin://{tenant_s}/ci/run/7#step-2"))
            .execute(&mut *conn)
            .await
            .map(|_| ())
            .map_err(|e| myelin_storage::PgError::Query(e.to_string()))
        })
    })
    .await
    .expect("seed the check_status row (app-role, RLS-scoped)");
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

/// Build the REAL ReBAC decision engine (the same [`StoreBackedCheck::check`] a human's request flows
/// through) over an in-memory tuple store. When `grant_pull` is set, seed a GENUINE
/// `repo:core#pull@<agent>` grant through the real [`TupleStore::write_tuples`] path — under the SAME
/// `(tenant, region)` scope the engine derives from the agent's own verified token — so the cap gate
/// finds the grant and ALLOWS the tool. The tool's `repo` argument (`myelin://<tenant>/git/repo/core`)
/// canonicalises to the `repo:core` tuple key, so this grant authorizes the run's exact call.
fn rebac_engine_with_pull(agent: &Principal, grant_pull: bool) -> StoreBackedCheck {
    let tuples = TupleStore::new(OutboxStore::new());
    if grant_pull {
        let scope = TenantScope::from_verified_token(agent, agent.region.clone());
        let admin = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            agent.tenant.clone(),
        );
        tuples
            .write_tuples(
                &scope,
                &admin,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId("repo:core".into()),
                    relation: RelName("pull".into()),
                    subject: PrincipalId("psn:host-agent".into()),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-06-19T00:00:00Z".into()),
            )
            .expect("seed the real `pull` grant via write_tuples");
    }
    StoreBackedCheck::new(tuples)
}

/// **The headline live drill: seed a check-status row, drive a real Luna run that INVOKES the read
/// tool to fetch it, and assert the tool was CALLED, its real result reached the agent, the answer
/// reflects it, and the wallet was debited for MULTIPLE turns — torn down cleanly.**
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "hits real Luna + live Postgres; requires OPENAI_API_KEY and the dev DB (:5433)"]
async fn live_luna_tool_run_reads_real_tenant_data_and_is_metered() {
    // Gate 1 — the real brain (the key rides only in the Luna Authorization header; never printed).
    let luna = match LunaClient::from_env() {
        Ok(c) => c,
        Err(ModelError::MissingApiKey) => {
            eprintln!("SKIP: OPENAI_API_KEY not set (no real-brain run)");
            return;
        }
        Err(e) => panic!("unexpected Luna construction error: {e}"),
    };
    // Gate 2 — the live durable DB (wallet + check_status projection migrated).
    let Some(_admin) = migrate_admin().await else {
        return;
    };
    let app = SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .expect("connect app role");
    let region_s = app.config().region.clone();
    let tenant = TenantId(format!("01J0HOSTTOOL{}", uniq()));
    let repo = format!("myelin://{}/git/repo/core", tenant.0);
    let commit = "abc123def456live";

    // SEED the readable tenant data (a distinctive `ci/build = failure` row).
    seed_check_row(&app, &tenant.0, &region_s, &repo, commit).await;

    // The F2-airtight composition root wired to the REAL Identity per-run token mint + durable S7
    // revocation. A UNIQUE cell_id per run (a fresh generated signing root, never colliding with a
    // pre-existing sealed root under a different key) + a fixed dev seal key — the SAME durable-cell +
    // seal-key path the edge gateway / CI runner use (a production main sources these from
    // MYELIN_CELL_ID / MYELIN_KMS_SEAL_KEY; the drill controls them so it is self-contained).
    let cell_id = format!("cell-agenthost-{}", uniq());
    let seal_key = SealKey::from_encoded(&"11".repeat(32)).expect("a 32-byte dev seal key");
    let host = AgentHost::with_identity(
        app.clone(),
        cell_id,
        &seal_key,
        tokio::runtime::Handle::current(),
    )
    .await
    .expect("wire the REAL Identity per-run mint + durable revocation");
    let topup = MicroUsd(2_000_000); // a couple cents cap — plenty for a tool call + a short answer.
    host.wallet()
        .credit(&tenant, topup, CreditKind::Topup, None)
        .expect("topup seeds the wallet");

    // The REAL read tool: the durable `Direct` executor over Git's check_status projection, scoped to
    // THIS run's verified token (tenant + region), plus the catalogue + advertised schema the loop +
    // the model use.
    let agent = agent_principal(&tenant);
    let scope = TenantScope::from_verified_token(&agent, Region(region_s.clone()));
    let executor = GitCheckStatusReadExecutor::new(
        app.clone(),
        app.clone(),
        tokio::runtime::Handle::current(),
        scope,
    );
    // THE CAP GATE: the run's `git.read_check_status` tool declares `required_caps = ["pull"]`. Wrap
    // the real read executor in the cap-enforcement checkpoint, which consults the REAL ReBAC engine
    // for the agent principal's `pull` grant on the repo BEFORE the read runs — fail-closed if absent.
    // We seed a genuine grant so the tool is ALLOWED and the drill passes end-to-end.
    let rebac: Arc<dyn IdentityService + Send + Sync> =
        Arc::new(rebac_engine_with_pull(&agent, /* grant_pull */ true));
    let gated = CapEnforcingExecutor::for_git_read_tool(rebac, agent.clone(), &executor);
    let catalogue = ToolCatalogue::new([git_check_status_read_tool_def()]);
    let advertised = [git_check_status_read_tool_schema()];

    let mut ledger = CostLedger::new();
    let outbox = OutboxStore::new();
    let mut wiring = RunSubstrateWiring {
        ledger: &mut ledger,
        outbox: &outbox,
        id_minter: Arc::new(MonotonicMinter::new()),
        journal: WfJournal::new(),
    };

    let run_id = "Rluna-tool-live";
    let task = LlmRunTask::new(
        tenant.clone(),
        agent,
        "psn:host-agent",
        run_id,
        "You are a hosted agent labelled as an agent. You have a tool, git.read_check_status, that \
         reads the CI check status Git recorded for a (repo, commit). When asked about a commit's \
         checks, you MUST call the tool to read the real data before answering.",
        format!(
            "Call git.read_check_status for repo {repo} at commit {commit}, then tell me in one \
             word the state of the 'build' check."
        ),
    )
    .with_max_output_tokens(256)
    .with_now_secs(1000);

    // F1: the durable wallet is threaded non-optionally — a real paid tool run is always billed.
    let report = host
        .run_llm_agent_with_tools(&task, &mut wiring, Box::new(luna), &catalogue, &gated, &advertised)
        .expect("the live Luna tool run completes (the seeded `pull` grant ALLOWS the tool)");

    eprintln!("LIVE Luna tool answer: {:?}", report.answer);
    eprintln!("LIVE tool result the agent read: {:?}", executor.last_result());
    eprintln!(
        "LIVE wallet: topup={} charged_micro={} balance={} tool_invocations={}",
        topup.0,
        report.charged_micro,
        host.wallet().balance(&tenant).0,
        executor.invocations(),
    );

    // THE TOOL WAS ACTUALLY INVOKED — Luna decided to call it and the executor ran the real read.
    assert!(
        executor.invocations() >= 1,
        "the agent CALLED the read tool at least once"
    );
    // The executor returned the REAL seeded data.
    let tool_text = executor.last_result().expect("the executor produced a read result");
    assert!(
        tool_text.contains("ci/build = Failure"),
        "the tool returned the real seeded row: {tool_text}"
    );
    // The agent's final answer REFLECTS the seeded state.
    assert!(!report.answer.trim().is_empty(), "real Luna produced an answer");
    assert!(
        report.answer.to_lowercase().contains("fail"),
        "the answer reflects the seeded build=failure: {:?}",
        report.answer
    );
    assert!(report.outcome.0.contains("completed"), "the run completed cleanly");

    // The wallet was DEBITED for MULTIPLE turns (the tool turn + the answer turn) — metered per turn.
    assert!(report.charged_micro > 0, "the run was priced + debited");
    assert_eq!(
        host.wallet().balance(&tenant),
        MicroUsd(topup.0 - report.charged_micro),
        "balance dropped by exactly the charged amount"
    );
    assert!(
        debit_row_count(app.db_pool(), &tenant.0, &region_s, run_id).await >= 2,
        "at least TWO run-linked debit rows (the tool turn + the answer turn) — multi-turn metering"
    );

    // The run tore down cleanly: a balanced reserve/settle ledger, a trace, the token revoked.
    assert!(report.telemetry.ledger_balanced(), "reserved == settled");
    assert_eq!(report.telemetry.traces_written(), 1);
    assert_eq!(report.telemetry.tokens_revoked(), 1, "per-run token revoked on teardown");
    assert_eq!(report.telemetry.runs_completed(), 1);

    // ── THE REAL-IDENTITY GATE: the per-run token was REALLY minted + REALLY revoked on the durable
    //    S7 denylist (not the in-process stub). The mint's `jti` is deterministic
    //    `runtok:<agent_id>:<run_id>:<mint_instant>`, so we recompute it and consult the durable
    //    store's lifecycle state. ──
    let mint_now = timestamp_from_epoch(1000); // the run's `with_now_secs(1000)` mint instant.
    let jti = run_token_jti(
        &PrincipalId("psn:host-agent".into()),
        &RunId(run_id.into()),
        &mint_now,
    );
    let verify_agent = agent_principal(&tenant);
    let verify_scope = TenantScope::from_verified_token(&verify_agent, Region(region_s.clone()));

    // (a) THIS host's durable S7 store reports the run's token TORN DOWN (the immediate teardown deny).
    let revocations = host
        .revocations()
        .expect("a with_identity host exposes the durable S7 store");
    assert_eq!(
        revocations.run_token_state(&verify_scope, &RevokeTarget::Jti(jti.clone()), &mint_now),
        RunTokenState::TornDown,
        "the REAL per-run token was torn down on the durable S7 denylist (post-run)"
    );

    // (b) DURABILITY: a FRESH S7 store instance over the SAME pool (a distinct connection / a
    //     cold-recovered cell) still sees the teardown denylist row — the revocation survives a
    //     process restart, and is dead for everyone in the tenant partition.
    let fresh_revocations = RevocationStore::with_pg(
        DurableRevocationBacking::new(app.clone()),
        tokio::runtime::Handle::current(),
    );
    assert_eq!(
        fresh_revocations.run_token_state(&verify_scope, &RevokeTarget::Jti(jti), &mint_now),
        RunTokenState::TornDown,
        "the durable revocation survives a fresh store instance (durable + tenant-scoped)"
    );

    eprintln!("LIVE real-Identity revocation: run token TornDown on the durable S7 denylist (durable across a fresh store instance)");

    // ── THE CAP-ENFORCEMENT DENY GATE (the sibling proof): the SAME governed tool call, but with NO
    //    `pull` grant for the agent, is DENIED fail-closed by the REAL ReBAC engine BEFORE the read
    //    runs. This confirms the allowed run above passed BECAUSE of the seeded grant, not because the
    //    cap is unenforced. We drive the executor directly (no second paid Luna turn) with the exact
    //    validated arguments the run used. ──
    let no_grant: Arc<dyn IdentityService + Send + Sync> =
        Arc::new(rebac_engine_with_pull(&verify_agent, /* grant_pull */ false));
    let probe_scope = TenantScope::from_verified_token(&verify_agent, Region(region_s.clone()));
    let probe_exec = GitCheckStatusReadExecutor::new(
        app.clone(),
        app.clone(),
        tokio::runtime::Handle::current(),
        probe_scope,
    );
    let denied_gate =
        CapEnforcingExecutor::for_git_read_tool(no_grant, verify_agent.clone(), &probe_exec);
    let denied = denied_gate.execute(
        &git_check_status_read_tool_def(),
        &ToolCall {
            id: ToolCallId("cap-deny-probe".into()),
            name: ToolName(GIT_READ_CHECK_STATUS_TOOL.into()),
            arguments: serde_json::json!({ "repo": repo, "commit": commit }),
        },
    );
    assert!(
        denied.is_err(),
        "WITHOUT the `pull` grant the SAME tool call is DENIED fail-closed: {denied:?}"
    );
    assert!(
        denied.unwrap_err().to_string().contains("cap-enforcement DENY"),
        "the deny is the cap-enforcement gate (fail-closed)"
    );
    assert_eq!(
        probe_exec.invocations(),
        0,
        "the real read NEVER ran on the denied cap (fail-closed, no execute)"
    );
    eprintln!("LIVE cap-enforcement: WITH the `pull` grant the tool was ALLOWED + executed; WITHOUT it the same call was DENIED fail-closed (real ReBAC check, no read)");
}
