#![cfg(feature = "integration")]

use myelin_agent_service::trigger_handoff::{
    TriggerRunHandoff, TriggerRunStart, HOSTED_AGENT_RUNTIME,
};
use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole as EventDataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    all_durable_migrations, AgentTriggerClaimRequest, AgentTriggerFiringState,
    CreateAgentTriggerBindingOutcome, DurableAgentTriggerBacking, NewAgentTriggerBinding,
    ReserveAgentTriggerFiringOutcome, SubstrateProvider,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use sqlx::types::chrono::Utc;
use sqlx::types::Uuid;
use sqlx::Row;

fn admin_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
}

fn unique_tenant() -> TenantId {
    TenantId(format!(
        "handoff-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos()
    ))
}

async fn seed_human_and_hosted_agent(
    provider: &SubstrateProvider,
    tenant: &TenantId,
    agent_id: Uuid,
) {
    let tenant_id = tenant.0.clone();
    let region = provider.config().region.clone();
    let human_kind = serde_json::to_string(&PrincipalKind::Human).unwrap();
    let agent_kind = serde_json::to_string(&PrincipalKind::Agent {
        runtime_ref: RuntimeRef(HOSTED_AGENT_RUNTIME.into()),
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
                     VALUES ($1, $2, $3, 'triage-bot', $4, 'founder', 'create-hosted-agent', \
                             ARRAY['issue.create'], ARRAY['issue.create'], clock_timestamp())",
                )
                .bind(&tenant_id)
                .bind(&region)
                .bind(agent_id)
                .bind(HOSTED_AGENT_RUNTIME)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .expect("the founder has one explicitly hosted agent");
}

fn failed_main_event(tenant: &TenantId, region: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("ci-failed-main-1".into()),
        type_: EventType("ci.run.failed".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region(region.into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        subject: ArtifactRef(format!("myelin://{}/ci/run/run-1", tenant.0)),
        aggregate: AggregateKey("ci-run:run-1".into()),
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
async fn one_red_build_becomes_one_durable_agent_workflow() {
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
    admin
        .migrate(
            &myelin_flow::migrations::migrations(),
            &HotTables::declare(["workflow_run"]),
        )
        .await
        .expect("the durable workflow schema is present");

    let app = SubstrateProvider::connect(MyelinConfig::dev(), 8)
        .await
        .expect("open the constrained app provider");
    let tenant = unique_tenant();
    let agent_id = Uuid::parse_str("20000000-0000-4000-8000-000000000002").unwrap();
    let binding_id = Uuid::parse_str("10000000-0000-4000-8000-000000000001").unwrap();
    seed_human_and_hosted_agent(&app, &tenant, agent_id).await;

    let triggers = DurableAgentTriggerBacking::new(app.clone());
    let created = triggers
        .create(
            &tenant.0,
            NewAgentTriggerBinding {
                binding_id,
                owner_principal_id: "founder".into(),
                run_as_agent_id: agent_id,
                client_nonce: "red-main-to-triage".into(),
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

    let event = failed_main_event(&tenant, &app.config().region);
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
        .expect("red main reserves exactly one governed firing");
    assert!(matches!(
        reserved,
        ReserveAgentTriggerFiringOutcome::Reserved(_)
    ));

    let claim = triggers
        .claim_next_firing(
            &tenant.0,
            AgentTriggerClaimRequest::new(HOSTED_AGENT_RUNTIME, "host-1", 30).unwrap(),
        )
        .await
        .expect("the hosted lane is available")
        .expect("the named hosted agent is woken");
    let handoff = TriggerRunHandoff::new(app.clone(), tokio::runtime::Handle::current());
    tokio::task::block_in_place(|| {
        handoff
            .register_workflow(&tenant)
            .expect("the compiled agent workflow is registered")
    });
    let first = handoff
        .start(&tenant, &claim)
        .await
        .expect("the claim and workflow co-commit");
    assert_eq!(first.outcome, TriggerRunStart::Started);

    let replay = handoff
        .start(&tenant, &claim)
        .await
        .expect("an uncertain handoff can be retried safely");
    assert_eq!(replay.outcome, TriggerRunStart::AlreadyStarted);
    assert_eq!(replay.run_id, first.run_id);
    assert_eq!(
        triggers
            .claim_next_firing(
                &tenant.0,
                AgentTriggerClaimRequest::new(HOSTED_AGENT_RUNTIME, "host-2", 30).unwrap(),
            )
            .await
            .unwrap(),
        None,
        "a started firing never returns to the runtime queue"
    );

    let firing = triggers
        .list_firings_for_owner(&tenant.0, "founder", binding_id, None, 10)
        .await
        .expect("the founder inspects the firing")
        .pop()
        .expect("one firing exists");
    assert_eq!(firing.state, AgentTriggerFiringState::Started);
    assert_eq!(firing.run_id.as_deref(), Some(first.run_id.as_str()));

    let expected_subject = event.subject.0.clone();
    let run_id = first.run_id.clone();
    let inspect_tenant = tenant.0.clone();
    let workflow = app
        .with_tenant_tx(&tenant.0, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT state, input, budget, idem_key FROM workflow_run \
                      WHERE tenant_id = $1 AND run_id = $2",
                )
                .bind(&inspect_tenant)
                .bind(&run_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok((
                    row.try_get::<String, _>("state").unwrap(),
                    row.try_get::<serde_json::Value, _>("input").unwrap(),
                    row.try_get::<serde_json::Value, _>("budget").unwrap(),
                    row.try_get::<String, _>("idem_key").unwrap(),
                ))
            })
        })
        .await
        .expect("the started firing names a real workflow row");
    assert_eq!(workflow.0, "running");
    assert_eq!(workflow.1, serde_json::json!([expected_subject]));
    assert_eq!(workflow.2, serde_json::json!({"minor_units": 250_000}));
    assert_eq!(
        workflow.3,
        format!("agent-trigger:{binding_id}:{}", event.event_id.0)
    );

    let cleanup_tenant = tenant.0.clone();
    app.with_tenant_tx(&tenant.0, move |conn| {
        Box::pin(async move {
            for statement in [
                "DELETE FROM agent_trigger_firing WHERE tenant_id = $1",
                "DELETE FROM agent_trigger_binding WHERE tenant_id = $1",
                "DELETE FROM workflow_run WHERE tenant_id = $1",
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
