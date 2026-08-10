use myelin_agent_host::{HostedAgentInputResolver, HostedAgentWorkflowInput};
use myelin_agent_service::hosted_run_contract::{AGENT_RUN_WORKFLOW, AGENT_RUN_WORKFLOW_VERSION};
use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole as EventDataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_flow::{PgClaimedDriveInput, PgInputResolveError, PgWorkflowInputResolver};
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef,
};
use myelin_identity_service::HOSTED_LUNA_RUNTIME;
use myelin_storage::migration::HotTables;
use myelin_storage::{
    all_durable_migrations, AgentTriggerClaimRequest, AgentTriggerStartRequest,
    CreateAgentTriggerBindingOutcome, DurableAgentTriggerBacking, NewAgentTriggerBinding,
    ReserveAgentTriggerFiringOutcome, StartAgentTriggerFiringOutcome, SubstrateProvider,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use sqlx::types::chrono::Utc;
use sqlx::types::Uuid;

fn admin_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hosted_run_receives_only_the_work_its_founder_governed() {
    let admin = match SubstrateProvider::connect(admin_config(), 4).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate_foundation()
        .await
        .expect("the event foundation is present");
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("the governed trigger schema is present");

    let app = SubstrateProvider::connect(MyelinConfig::dev(), 8)
        .await
        .expect("open the constrained app provider");
    let tenant = unique_tenant();
    let agent_id = Uuid::parse_str("20000000-0000-4000-8000-000000000012").unwrap();
    let binding_id = Uuid::parse_str("10000000-0000-4000-8000-000000000011").unwrap();
    let run_id = Uuid::parse_str("30000000-0000-4000-8000-000000000013").unwrap();
    a_founder_has_one_hosted_agent(&app, &tenant, agent_id).await;

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
                delegation_caveats: vec!["repo:core".into(), "issue:create".into()],
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
    let start = AgentTriggerStartRequest::from_claim(&claim, run_id).unwrap();
    let triggers_for_start = triggers.clone();
    let tenant_for_start = tenant.0.clone();
    let started = app
        .with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                triggers_for_start
                    .start_claimed_firing_on_conn(conn, &tenant_for_start, &start)
                    .await
            })
        })
        .await
        .expect("the firing becomes the run of record");
    assert_eq!(started, StartAgentTriggerFiringOutcome::Started);

    let claimed_input = PgClaimedDriveInput {
        tenant: tenant.clone(),
        region: event.region.clone(),
        run_id: run_id.to_string(),
        wf_type: AGENT_RUN_WORKFLOW.into(),
        wf_version: AGENT_RUN_WORKFLOW_VERSION,
        input: vec![event.subject.clone()],
        budget: Some(serde_json::json!({"minor_units": 250_000})),
        correlation_id: event.correlation_id.0.clone(),
        causation_id: Some(event.event_id.0.clone()),
        caused_by: None,
        depth: i32::try_from(event.depth).expect("bounded event depth fits the workflow schema"),
        partition: 0,
    };
    let resolver = HostedAgentInputResolver::new(app.clone());
    let material = resolver
        .resolve(claimed_input.clone())
        .await
        .expect("the worker resolves its input from governed records");
    let work: HostedAgentWorkflowInput =
        serde_json::from_slice(&material).expect("the worker input is legible");

    assert_eq!(
        work.task,
        "Explain the failure and prepare the smallest safe fix."
    );
    assert_eq!(work.trigger_actor.principal_id.0, "founder");
    assert_eq!(work.agent_id, agent_id.to_string());
    assert_eq!(work.delegation_caveats, ["repo:core", "issue:create"]);
    assert_eq!(work.selected_tools, ["git.read", "issue.create"]);
    assert_eq!(work.budget_minor_units, 250_000);
    assert_eq!(work.event, event);

    let mut forged_input = claimed_input;
    forged_input.input = vec![ArtifactRef(format!(
        "myelin://{}/ci/run/someone-elses-build",
        tenant.0
    ))];
    assert!(matches!(
        resolver.resolve(forged_input).await,
        Err(PgInputResolveError::Permanent(reason))
            if reason.contains("immutable trigger event")
    ));

    let cleanup_tenant = tenant.0.clone();
    app.with_tenant_tx(&tenant.0, move |conn| {
        Box::pin(async move {
            for statement in [
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
    .expect("clean the isolated tenant story");
}
