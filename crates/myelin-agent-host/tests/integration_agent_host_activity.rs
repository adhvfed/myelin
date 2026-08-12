use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use myelin_agent_host::{
    AgentHost, AgentHostActivityExecutor, DeadlineClock, HostedAgentActivityOutcome,
    HostedAgentRunExecutor, HostedAgentStopReason, HostedAgentWorkflowInput, HostedModelFactory,
};
use myelin_agent_model::{ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, Usage};
use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole as EventDataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef,
};
use myelin_identity_service::{NewAgent, PgAgentRegistry, HOSTED_LUNA_RUNTIME};
use myelin_notif::pg_inbox::PgInboxStore;
use myelin_notif::{agent_effect_approval_targets, pending_agent_effect_approval};
use myelin_storage::hitl_gate_durable::{
    opaque_gate_id, DurableHitlGateBacking, GateRecord, GateState, HitlVerdictStore,
};
use myelin_storage::migration::{HotTables, Migrations};
use myelin_storage::reserve_settle::{CostLedger, ReservationState, RunId};
use myelin_storage::{
    all_durable_migrations, CreditKind, MicroUsd, SealKey, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use tokio::sync::OnceCell;

static HOST_SCHEMA_READY: OnceCell<()> = OnceCell::const_new();

fn app_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

fn admin_config() -> MyelinConfig {
    let mut config = app_config();
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
}

fn unique(label: &str) -> String {
    format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos()
    )
}

async fn test_provider() -> Option<SubstrateProvider> {
    let admin = match SubstrateProvider::connect(admin_config(), 4).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return None;
        }
    };
    HOST_SCHEMA_READY
        .get_or_init(|| async move {
            admin.migrate_foundation().await.unwrap();
            let host_migrations = Migrations::of(
                all_durable_migrations()
                    .0
                    .into_iter()
                    .chain(myelin_notif::migrations::migrations().0),
            );
            admin
                .migrate(&host_migrations, &HotTables::none())
                .await
                .unwrap();
            sqlx::query("GRANT SELECT, INSERT, UPDATE, DELETE ON notif_inbox_item TO myelin_app")
                .execute(admin.db_pool())
                .await
                .expect("grant the runtime role access to hosted approval cards");
        })
        .await;
    Some(
        SubstrateProvider::connect(app_config(), 8)
            .await
            .expect("open the constrained app provider"),
    )
}

struct RefusingModelFactory {
    calls: AtomicUsize,
}

struct FinalModelFactory {
    clients: Arc<AtomicUsize>,
    provider_calls: Arc<AtomicUsize>,
}

struct FinalModelClient {
    provider_calls: Arc<AtomicUsize>,
}

struct FixedDeadlineClock(i64);

impl DeadlineClock for FixedDeadlineClock {
    fn now_unix_secs(&self) -> Result<i64, String> {
        Ok(self.0)
    }
}

impl HostedModelFactory for FinalModelFactory {
    fn client(&self) -> Result<Box<dyn ModelClient + Send + Sync>, ModelError> {
        self.clients.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FinalModelClient {
            provider_calls: self.provider_calls.clone(),
        }))
    }
}

impl ModelClient for FinalModelClient {
    fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.provider_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ModelResponse {
            reply: ModelReply::Final {
                content: "The governed run was explained without an external credential.".into(),
            },
            usage: Usage::Reported {
                input: 100,
                cached_input: 0,
                output: 10,
            },
        })
    }
}

impl HostedModelFactory for RefusingModelFactory {
    fn client(&self) -> Result<Box<dyn ModelClient + Send + Sync>, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ModelError::Transport(
            "the replay test must not construct a provider client".into(),
        ))
    }
}

fn governed_input(tenant: &TenantId, region: &Region, run_id: &str) -> HostedAgentWorkflowInput {
    let founder = Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId("founder".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let agent = Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId("agent:replay-safe".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("hosted:luna".into()),
            on_behalf_of: Some(founder.principal_id.clone()),
        },
        DataRole::Processor,
        PrincipalStatus::Active,
    );
    HostedAgentWorkflowInput {
        tenant: tenant.clone(),
        region: region.clone(),
        run_id: run_id.into(),
        agent_id: "replay-safe".into(),
        agent: agent.clone(),
        trigger_actor: founder,
        task: "Explain the completed run.".into(),
        delegation_caveats: vec![],
        selected_tools: vec!["ci.read_run.v1".into()],
        budget_minor_units: 250_000,
        event: EventEnvelope {
            event_id: EventId("completed-run-event".into()),
            type_: EventType("ci.run.failed".into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(agent),
            subject: ArtifactRef(format!("myelin://{}/ci/run/failed", tenant.0)),
            aggregate: AggregateKey("ci-run:failed".into()),
            causation_id: None,
            correlation_id: CorrelationId("completed-run-event".into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: EventDataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-08-10T09:00:00Z".into()),
            recorded_at: Timestamp("2026-08-10T09:00:01Z".into()),
            payload: serde_json::json!({}),
        },
    }
}

struct WaitingApprovalStory {
    input: HostedAgentWorkflowInput,
    cost_run: RunId,
    gate_id: String,
    scope: TenantScope,
    gate: GateRecord,
    inbox: PgInboxStore,
}

async fn stage_waiting_merge_approval(
    app: &SubstrateProvider,
    tenant: &TenantId,
    region: &Region,
    run_label: &str,
    pull_request_number: u64,
) -> WaitingApprovalStory {
    let run_id = unique(run_label);
    let input = governed_input(tenant, region, &run_id);
    let cost_run = RunId::new(run_id.clone());
    let mut ledger = CostLedger::with_pg(app.clone());
    ledger
        .reserve(
            tenant.clone(),
            cost_run.clone(),
            MicroUsd(input.budget_minor_units),
            MicroUsd(input.budget_minor_units),
        )
        .expect("the waiting run has a governed reservation");
    ledger
        .begin(tenant, &cost_run)
        .expect("the waiting run started before asking a human");

    let gate_id = opaque_gate_id();
    let scope = TenantScope::from_verified_token(&input.agent, region.clone());
    let gate = GateRecord {
        gate_id: gate_id.clone(),
        run_id,
        effect_id: myelin_mcp::governance::mcp_effect_key(
            "git.merge",
            &serde_json::json!({"repo": "platform", "number": pull_request_number}),
        ),
        risk_summary: format!("Merge pull request platform#{pull_request_number}").into_bytes(),
        cost_estimate: 0,
        approver_filter: vec!["founder".into(), "maintainer".into()],
        state: GateState::Waiting,
        card_ref: Some(format!(
            "myelin://{}/git/pr/platform:{pull_request_number}",
            tenant.0
        )),
        requested_by: input.agent.principal_id.0.clone(),
        decided_by: None,
        opened_at_unix: 90,
        decided_at_unix: None,
        expires_at_unix: 100,
        approval_consumed_at_unix: None,
    };
    HitlVerdictStore::with_pg(app.clone())
        .open(&scope, gate.clone())
        .expect("the exact effect waits durably");
    let inbox = PgInboxStore::new(app.db_pool().clone());
    for recipient in &gate.approver_filter {
        inbox
            .ensure(&pending_agent_effect_approval(
                tenant, region, recipient, &gate,
            ))
            .await
            .expect("each eligible human sees the same exact decision");
    }
    WaitingApprovalStory {
        input,
        cost_run,
        gate_id,
        scope,
        gate,
        inbox,
    }
}

async fn terminal_approval_executor(
    app: &SubstrateProvider,
    cell_label: &str,
) -> AgentHostActivityExecutor {
    let seal_key = SealKey::from_encoded(&"66".repeat(32)).expect("a 32-byte test seal key");
    let host = Arc::new(
        AgentHost::new(
            app.clone(),
            unique(cell_label),
            &seal_key,
            tokio::runtime::Handle::current(),
        )
        .await
        .expect("the production host starts"),
    );
    AgentHostActivityExecutor::new(
        host,
        app.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(RefusingModelFactory {
            calls: AtomicUsize::new(0),
        }),
    )
    .with_deadline_clock(Arc::new(FixedDeadlineClock(101)))
}

async fn assert_approval_cards_are_done(story: &WaitingApprovalStory, message: &str) {
    for target in
        agent_effect_approval_targets(&story.input.tenant, &story.input.region, &story.gate)
    {
        assert_eq!(
            story
                .inbox
                .get(&target.scope, &target.item_id)
                .await
                .expect("the terminal approval remains in history")
                .item
                .state,
            "done",
            "{message}",
        );
    }
}

async fn approval_audit_count(
    app: &SubstrateProvider,
    tenant: &TenantId,
    run_id: &str,
    event_type: &str,
) -> i64 {
    let aggregate = format!("mcp-run:{run_id}");
    let event_type = event_type.to_string();
    app.with_tenant_tx(&tenant.0, move |conn| {
        Box::pin(async move {
            sqlx::query_scalar(
                "SELECT count(*) FROM outbox \
                 WHERE aggregate = $1 AND envelope->>'type_' = $2",
            )
            .bind(&aggregate)
            .bind(&event_type)
            .fetch_one(&mut *conn)
            .await
            .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
        })
    })
    .await
    .expect("read the terminal approval audit")
}

async fn clean_terminal_approval_story(app: &SubstrateProvider, tenant: &TenantId) {
    let cleanup_tenant = tenant.0.clone();
    app.with_tenant_tx(&tenant.0, move |conn| {
        Box::pin(async move {
            for statement in [
                "DELETE FROM outbox_quarantine WHERE event_id IN \
                 (SELECT event_id FROM outbox WHERE envelope->>'tenant' = $1)",
                "DELETE FROM outbox WHERE envelope->>'tenant' = $1",
                "DELETE FROM notif_inbox_item WHERE tenant_id = $1",
                "DELETE FROM agent_hitl_gate WHERE tenant_id = $1",
                "DELETE FROM cost_event WHERE tenant_id = $1",
                "DELETE FROM cost_reservation WHERE tenant_id = $1",
            ] {
                sqlx::query(statement)
                    .bind(&cleanup_tenant)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            }
            Ok(())
        })
    })
    .await
    .expect("clean the isolated approval story");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_settled_agent_activity_replays_without_touching_the_model() {
    let Some(app) = test_provider().await else {
        return;
    };
    let tenant = TenantId(unique("hosted-activity"));
    let region = Region(app.config().region.clone());
    let run_id = unique("run");
    let input = governed_input(&tenant, &region, &run_id);
    let cost_run = RunId::new(run_id.clone());
    let mut ledger = CostLedger::with_pg(app.clone());
    ledger
        .reserve(
            tenant.clone(),
            cost_run.clone(),
            MicroUsd(input.budget_minor_units),
            MicroUsd(input.budget_minor_units),
        )
        .expect("the original activity reserved its governed budget");
    ledger
        .begin(&tenant, &cost_run)
        .expect("the original activity began");
    ledger
        .settle(&tenant, &cost_run, &[])
        .expect("the original activity completed and settled");

    let seal_key = SealKey::from_encoded(&"44".repeat(32)).expect("a 32-byte test seal key");
    let host = Arc::new(
        AgentHost::new(
            app.clone(),
            unique("hosted-activity-cell"),
            &seal_key,
            tokio::runtime::Handle::current(),
        )
        .await
        .expect("the production identity host starts"),
    );
    let models = Arc::new(RefusingModelFactory {
        calls: AtomicUsize::new(0),
    });
    let executor = AgentHostActivityExecutor::new(
        host,
        app.clone(),
        tokio::runtime::Handle::current(),
        models.clone(),
    );

    let expected = ArtifactRef(format!("myelin://{}/agent/run/{run_id}", tenant.0));
    assert_eq!(
        executor
            .execute(
                &input,
                &format!("{run_id}/agent.run:1/act"),
                1,
                1_786_352_400,
            )
            .expect("the outer workflow can recover its lost activity result"),
        HostedAgentActivityOutcome::Completed(expected)
    );
    assert_eq!(
        models.calls.load(Ordering::SeqCst),
        0,
        "a settled activity never repeats a provider call"
    );

    let mut changed_budget = input;
    changed_budget.budget_minor_units += 1;
    assert!(executor
        .execute(
            &changed_budget,
            &format!("{run_id}/agent.run:1/act"),
            1,
            1_786_352_400,
        )
        .expect_err("a replay cannot change its governed reservation")
        .contains("different governed budget"));

    let cleanup_tenant = tenant.0.clone();
    app.with_tenant_tx(&tenant.0, move |conn| {
        Box::pin(async move {
            sqlx::query("DELETE FROM cost_event WHERE tenant_id = $1")
                .bind(&cleanup_tenant)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            sqlx::query("DELETE FROM cost_reservation WHERE tenant_id = $1")
                .bind(&cleanup_tenant)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            Ok(())
        })
    })
    .await
    .expect("clean the isolated tenant story");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hosted_activity_uses_real_identity_wallet_and_cost_state_then_replays_cleanly() {
    let Some(app) = test_provider().await else {
        return;
    };
    let tenant = TenantId(unique("hosted-execution"));
    let region = Region(app.config().region.clone());
    let founder = Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId("founder".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let founder_kind = serde_json::to_string(&founder.kind).unwrap();
    let founder_role = serde_json::to_string(&founder.data_role).unwrap();
    let founder_status = serde_json::to_string(&founder.status).unwrap();
    let tenant_for_founder = tenant.0.clone();
    let region_for_founder = region.0.clone();
    app.with_tenant_tx(&tenant.0, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO principal (tenant_id, region, principal_id, kind, data_role, status) \
                 VALUES ($1, $2, 'founder', $3, $4, $5)",
            )
            .bind(&tenant_for_founder)
            .bind(&region_for_founder)
            .bind(&founder_kind)
            .bind(&founder_role)
            .bind(&founder_status)
            .execute(&mut *conn)
            .await
            .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            Ok(())
        })
    })
    .await
    .expect("the founder exists before delegating an agent");

    let grants = vec![
        "agent.tools.read".to_string(),
        "edge.identity.read".to_string(),
        "run.view".to_string(),
    ];
    let activation = PgAgentRegistry::new(app.clone())
        .create(
            &founder,
            NewAgent {
                name: "explain-bot".into(),
                runtime_ref: HOSTED_LUNA_RUNTIME.into(),
                tools: vec!["ci.read_run.v1".into()],
                grants: grants.clone(),
                tenant_policy_if_missing: grants.clone(),
                trigger_actor_policy_if_missing: grants,
                client_nonce: unique("explain-bot"),
            },
        )
        .await
        .expect("agent activation provisions the four policy conjuncts");
    let agent = Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId(activation.agent.principal_id.clone()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef(HOSTED_LUNA_RUNTIME.into()),
            on_behalf_of: Some(founder.principal_id.clone()),
        },
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let run_id = unique("hosted-run");
    let mut input = governed_input(&tenant, &region, &run_id);
    input.agent_id = activation.agent.id;
    input.agent = agent.clone();
    input.trigger_actor = founder;
    input.selected_tools = activation.agent.tools;

    let seal_key = SealKey::from_encoded(&"55".repeat(32)).expect("a 32-byte test seal key");
    let host = Arc::new(
        AgentHost::new(
            app.clone(),
            unique("hosted-execution-cell"),
            &seal_key,
            tokio::runtime::Handle::current(),
        )
        .await
        .expect("the production identity host starts"),
    );
    host.wallet()
        .credit(&tenant, MicroUsd(1_000_000), CreditKind::Topup, None)
        .expect("the organization has hosted-agent credit");
    let clients = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let executor = AgentHostActivityExecutor::new(
        host.clone(),
        app.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(FinalModelFactory {
            clients: clients.clone(),
            provider_calls: provider_calls.clone(),
        }),
    );
    let activity_key = format!("{run_id}/agent.run:1/act");
    let expected = ArtifactRef(format!("myelin://{}/agent/run/{run_id}", tenant.0));

    assert_eq!(
        executor
            .execute(&input, &activity_key, 1, 1_786_352_400)
            .expect("governed input runs with attenuated Identity and durable accounting"),
        HostedAgentActivityOutcome::Completed(expected.clone())
    );
    let balance_after_run = host.wallet().balance(&tenant);
    assert!(balance_after_run < MicroUsd(1_000_000));
    assert_eq!(clients.load(Ordering::SeqCst), 1);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        CostLedger::with_pg(app.clone()).state_of(&tenant, &RunId::new(run_id.clone())),
        Ok(Some(
            myelin_storage::reserve_settle::ReservationState::Settled
        ))
    );
    let trace_tenant = tenant.0.clone();
    let trace_run = run_id.clone();
    let trace = app
        .with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                sqlx::query_as::<_, (String, String, bool, bool, Vec<u8>, i64)>(
                    "SELECT artifact_ref, requested_by, answer IS NULL, trace_body IS NULL, \
                            payload_ciphertext, charged_micro \
                       FROM knowledge_agent_trace \
                      WHERE tenant_id = $1 AND run_id = $2",
                )
                .bind(&trace_tenant)
                .bind(&trace_run)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await
        .expect("the completed agent leaves one durable human-readable result");
    assert!(trace
        .0
        .starts_with(&format!("myelin://{}/knowledge/doc/blake3:", tenant.0)));
    assert_eq!(trace.1, "founder");
    assert!(
        trace.2,
        "the answer does not rest in its plaintext compatibility column"
    );
    assert!(
        trace.3,
        "the block-model body does not rest in its plaintext compatibility column"
    );
    let answer = b"The governed run was explained without an external credential.";
    assert!(
        !trace.4.is_empty(),
        "the private payload rests as ciphertext"
    );
    assert!(
        !trace.4.windows(answer.len()).any(|window| window == answer),
        "ciphertext does not contain the final answer in plaintext"
    );
    assert!(trace.5 > 0, "the result carries its exact metered cost");

    assert_eq!(
        executor
            .execute(&input, &activity_key, 1, 1_786_352_400)
            .expect("a lost outer workflow commit recovers the completed activity"),
        HostedAgentActivityOutcome::Completed(expected)
    );
    assert_eq!(host.wallet().balance(&tenant), balance_after_run);
    assert_eq!(clients.load(Ordering::SeqCst), 1);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    let replay_trace_tenant = tenant.0.clone();
    let replay_trace_run = run_id.clone();
    let trace_count = app
        .with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM knowledge_agent_trace \
                      WHERE tenant_id = $1 AND run_id = $2",
                )
                .bind(&replay_trace_tenant)
                .bind(&replay_trace_run)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await
        .expect("the recovered activity keeps exactly one result");
    assert_eq!(trace_count, 1);

    let cleanup_tenant = tenant.0.clone();
    app.with_tenant_tx(&tenant.0, move |conn| {
        Box::pin(async move {
            for statement in [
                "DELETE FROM outbox_quarantine WHERE event_id IN \
                 (SELECT event_id FROM outbox WHERE envelope->>'tenant' = $1)",
                "DELETE FROM outbox WHERE envelope->>'tenant' = $1",
                "DELETE FROM agent_model_step WHERE tenant_id = $1",
                "DELETE FROM knowledge_agent_trace WHERE tenant_id = $1",
                "DELETE FROM cost_event WHERE tenant_id = $1",
                "DELETE FROM cost_reservation WHERE tenant_id = $1",
                "DELETE FROM agent_wallet WHERE tenant_id = $1",
                "DELETE FROM run_token_teardown WHERE tenant_id = $1",
                "DELETE FROM delegation_run_snapshot WHERE tenant_id = $1",
                "DELETE FROM delegation_policy_head WHERE tenant_id = $1",
                "DELETE FROM delegation_policy_version WHERE tenant_id = $1",
                "DELETE FROM identity_agent WHERE tenant_id = $1",
                "DELETE FROM principal WHERE tenant_id = $1",
            ] {
                sqlx::query(statement)
                    .bind(&cleanup_tenant)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            }
            Ok(())
        })
    })
    .await
    .expect("clean the isolated tenant story");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_hosted_approvals_settle_once_even_when_their_wake_signal_is_lost() {
    let Some(app) = test_provider().await else {
        return;
    };
    let tenant = TenantId(unique("hosted-expiry"));
    let region = Region(app.config().region.clone());
    let expired = stage_waiting_merge_approval(&app, &tenant, &region, "expired-run", 7).await;
    let executor = terminal_approval_executor(&app, "hosted-expiry-cell").await;
    let stop_key = format!("{}/agent.run:2/expire", expired.input.run_id);

    let first = executor
        .stop(
            &expired.input,
            &stop_key,
            90,
            &expired.gate_id,
            HostedAgentStopReason::Expired,
        )
        .expect("recovery closes every durable surface against live time");
    let expired_gate = HitlVerdictStore::with_pg(app.clone())
        .fetch(&expired.scope, &expired.gate_id)
        .expect("the gate remains queryable");
    assert_eq!(expired_gate.state, GateState::Expired);
    assert_eq!(
        expired_gate.decided_at_unix,
        Some(101),
        "journaled workflow time cannot freeze an approval deadline",
    );
    assert_eq!(
        CostLedger::with_pg(app.clone()).state_of(&tenant, &expired.cost_run),
        Ok(Some(ReservationState::Settled)),
    );
    assert_approval_cards_are_done(
        &expired,
        "expiry never leaves another eligible human with a stale action",
    )
    .await;

    assert_eq!(
        executor
            .stop(
                &expired.input,
                &stop_key,
                101,
                &expired.gate_id,
                HostedAgentStopReason::Expired,
            )
            .expect("timer replay is idempotent"),
        first,
    );
    let expiry_audits =
        approval_audit_count(&app, &tenant, &expired.input.run_id, "git.merge.expired").await;
    assert_eq!(expiry_audits, 1, "timer replay emits one governance fact");

    let rejected = stage_waiting_merge_approval(&app, &tenant, &region, "rejected-run", 8).await;
    let decision = DurableHitlGateBacking::new(app.clone())
        .decide(
            &rejected.scope,
            &rejected.gate_id,
            GateState::Rejected,
            "founder",
            PrincipalKind::Human,
            95,
        )
        .await
        .expect("the rejection commits even when its later wake signal is lost")
        .expect("the founder may reject the exact effect");
    assert!(decision.changed);

    let rejected_stop_key = format!("{}/agent.run:2/expire", rejected.input.run_id);
    let rejected_stop = executor
        .stop(
            &rejected.input,
            &rejected_stop_key,
            90,
            &rejected.gate_id,
            HostedAgentStopReason::Expired,
        )
        .expect("the timer discovers and preserves the durable rejection");
    assert!(
        rejected_stop.0.contains(":stopped:rejected:gate:"),
        "a missed wake cannot relabel a human rejection as expiry",
    );
    let durable_rejection = HitlVerdictStore::with_pg(app.clone())
        .fetch(&rejected.scope, &rejected.gate_id)
        .expect("the rejected gate remains queryable");
    assert_eq!(durable_rejection.state, GateState::Rejected);
    assert_eq!(durable_rejection.decided_at_unix, Some(95));
    assert_eq!(
        CostLedger::with_pg(app.clone()).state_of(&tenant, &rejected.cost_run),
        Ok(Some(ReservationState::Settled)),
    );
    assert_approval_cards_are_done(
        &rejected,
        "a missed wake cannot leave another human with a stale rejection card",
    )
    .await;
    assert_eq!(
        executor
            .stop(
                &rejected.input,
                &rejected_stop_key,
                101,
                &rejected.gate_id,
                HostedAgentStopReason::Expired,
            )
            .expect("missed-wake recovery is replay-idempotent"),
        rejected_stop,
    );
    let rejected_audits =
        approval_audit_count(&app, &tenant, &rejected.input.run_id, "git.merge.rejected").await;
    let false_expiry_audits =
        approval_audit_count(&app, &tenant, &rejected.input.run_id, "git.merge.expired").await;
    assert_eq!(
        (rejected_audits, false_expiry_audits),
        (1, 0),
        "recovery records the rejection once and never invents an expiry",
    );
    clean_terminal_approval_story(&app, &tenant).await;
}
