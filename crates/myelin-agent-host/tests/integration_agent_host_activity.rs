use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use myelin_agent_host::{
    AgentHost, AgentHostActivityExecutor, HostedAgentActivityOutcome, HostedAgentRunExecutor,
    HostedAgentStopReason, HostedAgentWorkflowInput, HostedModelFactory,
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
use myelin_storage::hitl_gate_durable::{opaque_gate_id, GateRecord, GateState, HitlVerdictStore};
use myelin_storage::migration::HotTables;
use myelin_storage::reserve_settle::{CostLedger, ReservationState, RunId};
use myelin_storage::{
    all_durable_migrations, CreditKind, MicroUsd, SealKey, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

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
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
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
        Some(myelin_storage::reserve_settle::ReservationState::Settled)
    );
    let trace_tenant = tenant.0.clone();
    let trace_run = run_id.clone();
    let trace = app
        .with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                sqlx::query_as::<_, (String, String, String, i64)>(
                    "SELECT artifact_ref, requested_by, answer, charged_micro \
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
    assert_eq!(
        trace.2,
        "The governed run was explained without an external credential."
    );
    assert!(trace.3 > 0, "the result carries its exact metered cost");

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
async fn an_expired_hosted_approval_settles_once_and_leaves_no_actionable_card() {
    let Some(app) = test_provider().await else {
        return;
    };
    let tenant = TenantId(unique("hosted-expiry"));
    let region = Region(app.config().region.clone());
    let run_id = unique("expired-run");
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
        .expect("the waiting run has a governed reservation");
    ledger
        .begin(&tenant, &cost_run)
        .expect("the waiting run started before asking a human");

    let gate_id = opaque_gate_id();
    let scope = TenantScope::from_verified_token(&input.agent, region.clone());
    let gate = GateRecord {
        gate_id: gate_id.clone(),
        run_id: run_id.clone(),
        effect_id: myelin_mcp::governance::mcp_effect_key(
            "git.merge",
            &serde_json::json!({"repo": "platform", "number": 7}),
        ),
        risk_summary: b"Merge pull request platform#7".to_vec(),
        cost_estimate: 0,
        approver_filter: vec!["founder".into(), "maintainer".into()],
        state: GateState::Waiting,
        card_ref: Some(format!("myelin://{}/git/pr/platform:7", tenant.0)),
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
                &tenant, &region, recipient, &gate,
            ))
            .await
            .expect("each eligible human sees the same exact decision");
    }

    let seal_key = SealKey::from_encoded(&"66".repeat(32)).expect("a 32-byte test seal key");
    let host = Arc::new(
        AgentHost::new(
            app.clone(),
            unique("hosted-expiry-cell"),
            &seal_key,
            tokio::runtime::Handle::current(),
        )
        .await
        .expect("the production host starts"),
    );
    let executor = AgentHostActivityExecutor::new(
        host,
        app.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(RefusingModelFactory {
            calls: AtomicUsize::new(0),
        }),
    );
    let stop_key = format!("{run_id}/agent.run:2/expire");

    let first = executor
        .stop(
            &input,
            &stop_key,
            100,
            &gate_id,
            HostedAgentStopReason::Expired,
        )
        .expect("the timer closes every durable surface");
    assert_eq!(
        HitlVerdictStore::with_pg(app.clone())
            .fetch(&scope, &gate_id)
            .expect("the gate remains queryable")
            .state,
        GateState::Expired,
    );
    assert_eq!(
        CostLedger::with_pg(app.clone()).state_of(&tenant, &cost_run),
        Some(ReservationState::Settled),
    );
    for target in agent_effect_approval_targets(&tenant, &region, &gate) {
        assert_eq!(
            inbox
                .get(&target.scope, &target.item_id)
                .await
                .expect("the projected approval remains in history")
                .item
                .state,
            "done",
            "expiry never leaves another eligible human with a stale action",
        );
    }

    assert_eq!(
        executor
            .stop(
                &input,
                &stop_key,
                101,
                &gate_id,
                HostedAgentStopReason::Expired,
            )
            .expect("timer replay is idempotent"),
        first,
    );
    let aggregate = format!("mcp-run:{run_id}");
    let expiry_audits: i64 = app
        .with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                sqlx::query_scalar(
                    "SELECT count(*) FROM outbox \
                     WHERE aggregate = $1 AND envelope->>'type_' = 'git.merge.expired'",
                )
                .bind(&aggregate)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await
        .expect("read the expiry audit");
    assert_eq!(expiry_audits, 1, "timer replay emits one governance fact");

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
    .expect("clean the isolated expiry story");
}
