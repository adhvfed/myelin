#![cfg(feature = "integration")]

use std::sync::{Arc, Mutex};

use myelin_agent_host::{
    register_hosted_agent_workflow, HostedAgentActivityOutcome, HostedAgentInputResolver,
    HostedAgentRunExecutor, HostedAgentStopReason, HostedAgentWorkflowInput,
};
use myelin_agent_service::hosted_run_contract::{AGENT_RUN_WORKFLOW, AGENT_RUN_WORKFLOW_VERSION};
use myelin_agent_service::trigger_handoff::TriggerRunHandoff;
use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole as EventDataRole, EventEnvelope, EventId,
    EventType, IdMinter, MonotonicMinter, Timestamp, Visibility,
};
use myelin_flow::{
    partition_for_run_id, DriveOutcome, PgClaimedDriveInput, PgFlowWorker, PgInputResolveError,
    PgRunOnceOutcome, PgWorkerScope, PgWorkflowInputResolver,
};
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef,
};
use myelin_identity_service::HOSTED_LUNA_RUNTIME;
use myelin_storage::migration::HotTables;
use myelin_storage::reserve_settle::{CostLedger, MicroUsd, ReservationState, RunId as CostRunId};
use myelin_storage::{
    all_durable_migrations, AgentTriggerClaimRequest, CreateAgentTriggerBindingOutcome,
    DurableAgentTriggerBacking, NewAgentTriggerBinding, ReserveAgentTriggerFiringOutcome,
    SubstrateProvider,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use sqlx::types::chrono::Utc;
use sqlx::types::Uuid;

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

fn unique_tenant() -> TenantId {
    TenantId(format!(
        "hosted-input-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos()
    ))
}

async fn test_provider() -> SubstrateProvider {
    let admin = SubstrateProvider::connect(admin_config(), 4)
        .await
        .expect("integration tests require the configured Postgres backend");
    admin
        .migrate_foundation()
        .await
        .expect("the event foundation is present");
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("the governed trigger schema is present");
    SubstrateProvider::connect(app_config(), 8)
        .await
        .expect("open the constrained app provider")
}

async fn a_founder_has_one_hosted_agent(
    provider: &SubstrateProvider,
    tenant: &TenantId,
    agent_id: Uuid,
) {
    let tenant_id = tenant.0.clone();
    let region = provider.config().region.clone();
    let human_kind = serde_json::to_string(&PrincipalKind::Human).unwrap();
    let agent_kind = serde_json::to_string(&PrincipalKind::Agent {
        runtime_ref: RuntimeRef(HOSTED_LUNA_RUNTIME.into()),
        on_behalf_of: Some(PrincipalId("founder".into())),
    })
    .unwrap();
    let controller = serde_json::to_string(&DataRole::Controller).unwrap();
    let processor = serde_json::to_string(&DataRole::Processor).unwrap();
    let active = serde_json::to_string(&PrincipalStatus::Active).unwrap();
    let agent_principal = format!("agent:{agent_id}");

    provider
        .with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO principal \
                       (tenant_id, region, principal_id, kind, data_role, status) VALUES \
                       ($1, $2, 'founder', $3, $4, $5), \
                       ($1, $2, $6, $7, $8, $5)",
                )
                .bind(&tenant_id)
                .bind(&region)
                .bind(&human_kind)
                .bind(&controller)
                .bind(&active)
                .bind(&agent_principal)
                .bind(&agent_kind)
                .bind(&processor)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                sqlx::query(
                    "INSERT INTO identity_agent \
                       (tenant_id, region, agent_id, name, runtime_ref, created_by, client_nonce, \
                        tools, grants, created_at) \
                     VALUES ($1, $2, $3, 'repair-bot', $4, 'founder', 'create-repair-bot', \
                             ARRAY['git.read', 'issue.create'], ARRAY['git.read'], \
                             clock_timestamp())",
                )
                .bind(&tenant_id)
                .bind(&region)
                .bind(agent_id)
                .bind(HOSTED_LUNA_RUNTIME)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .expect("the founder has one explicitly hosted agent");
}

fn the_main_build_failed(tenant: &TenantId, region: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("red-main-1".into()),
        type_: EventType("ci.run.failed".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region(region.into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        subject: ArtifactRef(format!("myelin://{}/ci/run/red-main-1", tenant.0)),
        aggregate: AggregateKey("ci-run:red-main-1".into()),
        causation_id: None,
        correlation_id: CorrelationId("push-main-1".into()),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: EventDataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-08-10T08:00:00Z".into()),
        recorded_at: Timestamp("2026-08-10T08:00:01Z".into()),
        payload: serde_json::json!({
            "source_ref": "refs/heads/main",
            "commit_oid": "deadbeef"
        }),
    }
}

struct StartedHostedRun {
    tenant: TenantId,
    agent_id: Uuid,
    binding_id: Uuid,
    event: EventEnvelope,
    triggers: DurableAgentTriggerBacking,
    run_id: String,
    claimed_input: PgClaimedDriveInput,
    resolved_input: HostedAgentWorkflowInput,
}

async fn start_one_governed_hosted_run(app: &SubstrateProvider) -> StartedHostedRun {
    let tenant = unique_tenant();
    let agent_id = Uuid::parse_str("20000000-0000-4000-8000-000000000012").unwrap();
    let binding_id = Uuid::parse_str("10000000-0000-4000-8000-000000000011").unwrap();
    a_founder_has_one_hosted_agent(app, &tenant, agent_id).await;

    let triggers = DurableAgentTriggerBacking::new(app.clone());
    let created = triggers
        .create(
            &tenant.0,
            NewAgentTriggerBinding {
                binding_id,
                owner_principal_id: "founder".into(),
                run_as_agent_id: agent_id,
                client_nonce: "repair-red-main".into(),
                event_type: "ci.run.failed".into(),
                matcher: serde_json::json!({}),
                task: "Explain the failure and prepare the smallest safe fix.".into(),
                delegation_caveats: vec!["repo:core".into(), "issue.create".into()],
                budget_minor_units: 250_000,
                max_firings: 1,
                max_causal_depth: 4,
                require_no_personal_data: true,
                require_human_approval: false,
                created_at: Utc::now(),
            },
        )
        .await
        .expect("the founder saves the automation");
    assert!(matches!(
        created,
        CreateAgentTriggerBindingOutcome::Created(_)
    ));

    let event = the_main_build_failed(&tenant, &app.config().region);
    let reserved = triggers
        .reserve_firing(
            &tenant.0,
            binding_id,
            &event.event_id.0,
            &event.type_.0,
            serde_json::to_value(&event).unwrap(),
            event.depth,
            event.contains_personal_data,
            Utc::now(),
        )
        .await
        .expect("red main reserves one governed firing");
    assert!(matches!(
        reserved,
        ReserveAgentTriggerFiringOutcome::Reserved(_)
    ));

    let claim = triggers
        .claim_next_firing(
            &tenant.0,
            AgentTriggerClaimRequest::new(HOSTED_LUNA_RUNTIME, "host-1", 30).unwrap(),
        )
        .await
        .expect("the hosted lane is available")
        .expect("the repair bot is woken");
    let handoff = TriggerRunHandoff::new(app.clone(), tokio::runtime::Handle::current());
    handoff
        .register_workflow(&tenant)
        .expect("the hosted workflow definition is known before dispatch");
    let receipt = handoff
        .start(&tenant, &claim)
        .await
        .expect("the firing and workflow start together");
    let run_id = receipt.run_id;

    let claimed_input = PgClaimedDriveInput {
        tenant: tenant.clone(),
        region: event.region.clone(),
        run_id: run_id.clone(),
        wf_type: AGENT_RUN_WORKFLOW.into(),
        wf_version: AGENT_RUN_WORKFLOW_VERSION,
        input: vec![event.subject.clone()],
        budget: Some(serde_json::json!({"minor_units": 250_000})),
        correlation_id: event.correlation_id.0.clone(),
        causation_id: Some(event.event_id.0.clone()),
        caused_by: None,
        depth: i32::try_from(event.depth).expect("bounded event depth fits the workflow schema"),
        partition: partition_for_run_id(&run_id),
    };
    let resolved_input = serde_json::from_slice(
        &HostedAgentInputResolver::new(app.clone())
            .resolve(claimed_input.clone())
            .await
            .expect("the worker resolves its input from governed records"),
    )
    .expect("the worker input is legible");

    StartedHostedRun {
        tenant,
        agent_id,
        binding_id,
        event,
        triggers,
        run_id,
        claimed_input,
        resolved_input,
    }
}

fn worker_for(
    app: &SubstrateProvider,
    run: &StartedHostedRun,
    executor: Arc<dyn HostedAgentRunExecutor>,
) -> PgFlowWorker {
    let worker_actor = Actor(Principal::new(
        run.tenant.clone(),
        run.event.region.clone(),
        PrincipalId("svc:hosted-agent-worker".into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    ));
    let scope = PgWorkerScope::new(
        run.tenant.clone(),
        run.event.region.clone(),
        partition_for_run_id(&run.run_id),
        "hosted-agent-worker-1",
        30,
        worker_actor,
        1,
    )
    .expect("the hosted worker has one exact tenant partition");
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let mut worker = PgFlowWorker::new(
        app.db_pool().clone(),
        tokio::runtime::Handle::current(),
        minter,
        scope,
    );
    register_hosted_agent_workflow(
        &mut worker,
        HostedAgentInputResolver::new(app.clone()),
        executor,
    )
    .expect("the worker owns the hosted-agent definition");
    worker
}

async fn clean_hosted_run(app: &SubstrateProvider, tenant: &TenantId) {
    let cleanup_tenant = tenant.0.clone();
    app.with_tenant_tx(&tenant.0, move |conn| {
        Box::pin(async move {
            for statement in [
                "DELETE FROM cost_event WHERE tenant_id = $1",
                "DELETE FROM cost_reservation WHERE tenant_id = $1",
                "DELETE FROM wf_activity_attempt WHERE tenant_id = $1",
                "DELETE FROM wf_history WHERE tenant_id = $1",
                "DELETE FROM workflow_run WHERE tenant_id = $1",
                "DELETE FROM agent_trigger_firing WHERE tenant_id = $1",
                "DELETE FROM agent_trigger_binding WHERE tenant_id = $1",
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
    .expect("clean the isolated hosted-run story");
}

#[derive(Default)]
struct RecordingHostedAgent {
    executions: Mutex<Vec<(HostedAgentWorkflowInput, String, i64)>>,
}

impl HostedAgentRunExecutor for RecordingHostedAgent {
    fn execute(
        &self,
        input: &HostedAgentWorkflowInput,
        activity_key: &str,
        _attempt: u32,
        now_secs: i64,
    ) -> Result<HostedAgentActivityOutcome, String> {
        self.executions
            .lock()
            .unwrap()
            .push((input.clone(), activity_key.to_string(), now_secs));
        Ok(HostedAgentActivityOutcome::Completed(ArtifactRef(format!(
            "myelin://{}/agent/run/{}",
            input.tenant.0, input.run_id
        ))))
    }

    fn stop(
        &self,
        input: &HostedAgentWorkflowInput,
        _activity_key: &str,
        _now_secs: i64,
        gate_id: &str,
        reason: HostedAgentStopReason,
    ) -> Result<ArtifactRef, String> {
        Ok(ArtifactRef(format!(
            "myelin://{}/agent/run/{}:stopped:{}:gate:{}",
            input.tenant.0,
            input.run_id,
            reason.as_str(),
            myelin_agent_service::hosted_run_contract::gate_ref_token(gate_id)
        )))
    }
}

#[derive(Default)]
struct UnavailableHostedAgent {
    attempts: Mutex<u32>,
}

impl HostedAgentRunExecutor for UnavailableHostedAgent {
    fn execute(
        &self,
        _input: &HostedAgentWorkflowInput,
        _activity_key: &str,
        _attempt: u32,
        _now_secs: i64,
    ) -> Result<HostedAgentActivityOutcome, String> {
        *self.attempts.lock().unwrap() += 1;
        Err("the model provider is unavailable".into())
    }

    fn stop(
        &self,
        _input: &HostedAgentWorkflowInput,
        _activity_key: &str,
        _now_secs: i64,
        _gate_id: &str,
        _reason: HostedAgentStopReason,
    ) -> Result<ArtifactRef, String> {
        Err("an unavailable model never opens an approval gate".into())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hosted_run_receives_only_the_work_its_founder_governed() {
    let app = test_provider().await;
    let run = start_one_governed_hosted_run(&app).await;
    let work = &run.resolved_input;

    assert_eq!(
        work.task,
        "Explain the failure and prepare the smallest safe fix."
    );
    assert_eq!(work.trigger_actor.principal_id.0, "founder");
    assert_eq!(work.agent_id, run.agent_id.to_string());
    assert_eq!(work.delegation_caveats, ["repo:core", "issue.create"]);
    assert_eq!(work.selected_tools, ["git.read", "issue.create"]);
    assert_eq!(work.budget_minor_units, 250_000);
    assert_eq!(work.event, run.event);

    let mut forged_input = run.claimed_input.clone();
    forged_input.input = vec![ArtifactRef(format!(
        "myelin://{}/ci/run/someone-elses-build",
        run.tenant.0
    ))];
    assert!(matches!(
        HostedAgentInputResolver::new(app.clone())
            .resolve(forged_input)
            .await,
        Err(PgInputResolveError::Permanent(reason))
            if reason.contains("immutable trigger event")
    ));

    let agent = Arc::new(RecordingHostedAgent::default());
    let worker = worker_for(&app, &run, agent.clone());

    let run_ref = ArtifactRef(format!(
        "myelin://{}/agent/run/{}",
        run.tenant.0, run.run_id
    ));
    let driven = worker
        .run_once(1_786_352_400, "2026-08-10T09:00:00Z")
        .await
        .expect("the governed work reaches its hosted agent");
    assert!(matches!(
        driven,
        PgRunOnceOutcome::Driven {
            run_id: ref driven_id,
            outcome: DriveOutcome::Completed(ref refs),
            ..
        } if driven_id == &run.run_id && refs == &[run_ref]
    ));

    {
        let executions = agent.executions.lock().unwrap();
        assert_eq!(executions.len(), 1, "one firing becomes one agent run");
        assert_eq!(&executions[0].0, work);
        assert_eq!(executions[0].1, format!("{}/agent.run:1/act", run.run_id));
        assert_eq!(executions[0].2, 1_786_352_400);
    }
    assert!(matches!(
        worker
            .run_once(1_786_352_401, "2026-08-10T09:00:01Z")
            .await
            .expect("a completed workflow stays quiet"),
        PgRunOnceOutcome::Idle
    ));
    assert_eq!(agent.executions.lock().unwrap().len(), 1);
    assert_eq!(
        run.triggers
            .reconcile_terminal_firings(&run.tenant.0, 100)
            .await
            .expect("workflow completion is projected to its firing"),
        1
    );
    let history = run
        .triggers
        .list_firings_for_owner(&run.tenant.0, "founder", run.binding_id, None, 100)
        .await
        .expect("the founder can read the completed firing");
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].state,
        myelin_storage::AgentTriggerFiringState::Terminal
    );
    assert_eq!(history[0].run_id.as_deref(), Some(run.run_id.as_str()));
    assert_eq!(
        history[0].terminal_reason, None,
        "successful work needs no failure guidance",
    );
    clean_hosted_run(&app, &run.tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_hosted_work_releases_its_organization_budget_when_history_catches_up() {
    let app = test_provider().await;
    let run = start_one_governed_hosted_run(&app).await;
    let cost_run = CostRunId::new(run.run_id.clone());
    let mut ledger = CostLedger::with_pg(app.clone());
    ledger
        .reserve(
            run.tenant.clone(),
            cost_run.clone(),
            MicroUsd(250_000),
            MicroUsd(250_000),
        )
        .expect("the organization lends this run its bounded budget");
    ledger
        .begin(&run.tenant, &cost_run)
        .expect("the model attempt puts that budget in flight");

    let agent = Arc::new(UnavailableHostedAgent::default());
    let worker = worker_for(&app, &run, agent.clone());
    let driven = worker
        .run_once(1_786_352_400, "2026-08-10T09:00:00Z")
        .await
        .expect("the worker records the exhausted provider failure");
    assert!(matches!(
        driven,
        PgRunOnceOutcome::Driven {
            run_id: ref driven_id,
            outcome: DriveOutcome::Failed(ref reason),
            ..
        } if driven_id == &run.run_id && reason.contains("model provider is unavailable")
    ));
    assert!(
        *agent.attempts.lock().unwrap() > 1,
        "the bounded activity retry is exhausted before the run becomes terminal",
    );
    assert_eq!(
        ledger.state_of(&run.tenant, &cost_run),
        Ok(Some(ReservationState::InFlight)),
        "the durable workflow outcome lands before its asynchronous history projection",
    );

    assert_eq!(
        run.triggers
            .reconcile_terminal_firings(&run.tenant.0, 100)
            .await
            .expect("terminal history and budget converge together"),
        1,
    );
    assert_eq!(
        ledger.state_of(&run.tenant, &cost_run),
        Ok(Some(ReservationState::Settled)),
        "failed work cannot keep reducing the organization's spendable balance",
    );
    let history = run
        .triggers
        .list_firings_for_owner(&run.tenant.0, "founder", run.binding_id, None, 100)
        .await
        .expect("the founder can inspect the failed run");
    assert_eq!(history.len(), 1);
    assert_eq!(
        (history[0].state, history[0].outcome),
        (
            myelin_storage::AgentTriggerFiringState::Terminal,
            Some(myelin_storage::agent_trigger_durable::AgentTriggerRunOutcome::Failed),
        ),
        "history says failed only after the stranded budget has been released",
    );
    assert_eq!(
        history[0].terminal_reason.as_deref(),
        Some("agent run failed; retry it or inspect the hosted-agent service diagnostics"),
        "the founder gets a safe next step instead of a provider response body",
    );
    assert_eq!(
        run.triggers
            .reconcile_terminal_firings(&run.tenant.0, 100)
            .await
            .expect("terminal reconciliation is replay-safe"),
        0,
    );
    assert_eq!(
        ledger.state_of(&run.tenant, &cost_run),
        Ok(Some(ReservationState::Settled)),
    );
    clean_hosted_run(&app, &run.tenant).await;
}
