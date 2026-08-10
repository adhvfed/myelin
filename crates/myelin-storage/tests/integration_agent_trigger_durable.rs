#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    all_durable_migrations, AgentTriggerClaimRequest, AgentTriggerFiringState,
    AgentTriggerLifecycleAction, ChangeAgentTriggerLifecycleOutcome,
    CreateAgentTriggerBindingOutcome, DurableAgentTriggerBacking, NewAgentTriggerBinding,
    ReserveAgentTriggerFiringOutcome, SubstrateProvider,
};
use sqlx::types::chrono::Utc;
use sqlx::types::Uuid;

fn admin_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
}

fn unique_tenant() -> String {
    format!(
        "trigger-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos()
    )
}

async fn seed_people_and_agent(
    provider: &SubstrateProvider,
    tenant: &str,
    agent_id: Uuid,
    runtime_ref: &str,
) {
    let tenant = tenant.to_string();
    let region = provider.config().region.clone();
    let human_kind = serde_json::to_string(&PrincipalKind::Human).unwrap();
    let agent_kind = serde_json::to_string(&PrincipalKind::Agent {
        runtime_ref: RuntimeRef(runtime_ref.into()),
        on_behalf_of: Some(PrincipalId("founder".into())),
    })
    .unwrap();
    let controller = serde_json::to_string(&DataRole::Controller).unwrap();
    let processor = serde_json::to_string(&DataRole::Processor).unwrap();
    let active = serde_json::to_string(&PrincipalStatus::Active).unwrap();
    let agent_principal = format!("agent:{agent_id}");
    let runtime_ref = runtime_ref.to_string();
    provider
        .with_tenant_tx(&tenant.clone(), move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO principal \
                       (tenant_id, region, principal_id, kind, data_role, status) VALUES \
                       ($1, $2, 'founder', $3, $4, $5), \
                       ($1, $2, $6, $7, $8, $5)",
                )
                .bind(&tenant)
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
                     VALUES ($1, $2, $3, 'triage-bot', $4, 'founder', 'agent-create', \
                             ARRAY['issue.create'], ARRAY['issue.create'], clock_timestamp())",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(agent_id)
                .bind(&runtime_ref)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .expect("seed one active human and the explicitly selected active agent");
}

fn red_mainline_binding(agent_id: Uuid) -> NewAgentTriggerBinding {
    NewAgentTriggerBinding {
        binding_id: Uuid::parse_str("10000000-0000-4000-8000-000000000001").unwrap(),
        owner_principal_id: "founder".into(),
        run_as_agent_id: agent_id,
        client_nonce: "red-main-to-triage".into(),
        event_type: "ci.run.failed".into(),
        matcher: serde_json::json!({
            "object_type": "run",
            "predicate": {
                "and": [
                    { "eq": ["event.type", "ci.run.failed"] },
                    { "eq": ["payload.source_ref", "refs/heads/main"] }
                ]
            }
        }),
        task: "Find the failure, open an issue, and prepare the smallest safe fix.".into(),
        delegation_caveats: vec!["repo:core".into(), "issue:create".into()],
        budget_minor_units: 250_000,
        max_firings: 1,
        max_causal_depth: 4,
        require_no_personal_data: true,
        require_human_approval: false,
        created_at: Utc::now(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_human_binding_wakes_one_named_agent_once_when_main_goes_red() {
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
        .expect("the governed trigger schema migrates with the durable platform");

    let app = SubstrateProvider::connect(MyelinConfig::dev(), 8)
        .await
        .expect("open the constrained app provider");
    let tenant = unique_tenant();
    let agent_id = Uuid::parse_str("20000000-0000-4000-8000-000000000002").unwrap();
    seed_people_and_agent(&app, &tenant, agent_id, "hosted:luna").await;
    let triggers = DurableAgentTriggerBacking::new(app.clone());
    let proposal = red_mainline_binding(agent_id);

    let created = triggers
        .create(&tenant, proposal.clone())
        .await
        .expect("the founder binds red mainline CI to triage-bot");
    let CreateAgentTriggerBindingOutcome::Created(binding) = created else {
        panic!("a fresh binding must be created, got {created:?}");
    };
    assert_eq!(binding.owner_principal_id, "founder");
    assert_eq!(binding.run_as_agent_id, agent_id.to_string());
    assert_eq!(binding.firings_used, 0);
    let candidates = triggers
        .active_for_event(&tenant, "ci.run.failed", 100)
        .await
        .expect("the CI consumer discovers bindings by exact event type");
    assert_eq!(
        candidates
            .iter()
            .map(|item| &item.binding_id)
            .collect::<Vec<_>>(),
        vec![&binding.binding_id],
        "event intake sees the one explicitly named run-as agent, never an arbitrary active agent"
    );

    let mut exact_retry = proposal.clone();
    exact_retry.binding_id = Uuid::parse_str("90000000-0000-4000-8000-000000000009").unwrap();
    assert!(
        matches!(
            triggers.create(&tenant, exact_retry).await.unwrap(),
            CreateAgentTriggerBindingOutcome::Replayed(_)
        ),
        "retrying the same CLI request finds the same durable binding despite a fresh server candidate id"
    );
    let mut conflicting_retry = proposal.clone();
    conflicting_retry.task = "Do something broader.".into();
    assert_eq!(
        triggers.create(&tenant, conflicting_retry).await.unwrap(),
        CreateAgentTriggerBindingOutcome::Conflict,
        "one idempotency key cannot silently acquire a different task"
    );

    let paused = triggers
        .change_lifecycle(
            &tenant,
            "founder",
            proposal.binding_id,
            AgentTriggerLifecycleAction::Pause,
        )
        .await
        .expect("the owner pauses the automation before maintenance");
    let ChangeAgentTriggerLifecycleOutcome::Complete(paused) = paused else {
        panic!("an owned active trigger must pause, got {paused:?}");
    };
    assert!(paused.changed);
    assert_eq!(paused.binding.state, "paused");
    assert_eq!(paused.canceled_firings, 0);
    assert!(
        triggers
            .active_for_event(&tenant, "ci.run.failed", 100)
            .await
            .expect("event intake observes the pause")
            .is_empty(),
        "a paused automation is quiet without losing its intent"
    );
    assert_eq!(
        triggers
            .reserve_firing(
                &tenant,
                proposal.binding_id,
                "ci-failed-during-maintenance",
                "ci.run.failed",
                serde_json::json!({"event_id": "ci-failed-during-maintenance"}),
                1,
                false,
                Utc::now(),
            )
            .await
            .expect("a matching event arrives while maintenance is active"),
        ReserveAgentTriggerFiringOutcome::BindingUnavailable,
        "events cannot sneak into the queue after pause returns"
    );
    let paused_again = triggers
        .change_lifecycle(
            &tenant,
            "founder",
            proposal.binding_id,
            AgentTriggerLifecycleAction::Pause,
        )
        .await
        .expect("the CLI safely retries pause");
    assert!(matches!(
        paused_again,
        ChangeAgentTriggerLifecycleOutcome::Complete(ref outcome) if !outcome.changed
    ));
    assert_eq!(
        triggers
            .change_lifecycle(
                &tenant,
                "reviewer",
                proposal.binding_id,
                AgentTriggerLifecycleAction::Disable,
            )
            .await
            .expect("another person cannot discover ownership through mutation"),
        ChangeAgentTriggerLifecycleOutcome::NotFound
    );
    let resumed = triggers
        .change_lifecycle(
            &tenant,
            "founder",
            proposal.binding_id,
            AgentTriggerLifecycleAction::Resume,
        )
        .await
        .expect("the owner resumes normal operation");
    assert!(matches!(
        resumed,
        ChangeAgentTriggerLifecycleOutcome::Complete(ref outcome)
            if outcome.changed && outcome.binding.state == "active"
    ));

    let event = serde_json::json!({
        "event_id": "ci-failed-1",
        "type": "ci.run.failed",
        "tenant": tenant,
        "payload": {
            "run": format!("myelin://{tenant}/ci/run/run-1"),
            "commit_oid": "deadbeef",
            "source_ref": "refs/heads/main"
        }
    });
    let first = triggers
        .reserve_firing(
            &tenant,
            proposal.binding_id,
            "ci-failed-1",
            "ci.run.failed",
            event.clone(),
            1,
            false,
            Utc::now(),
        )
        .await
        .expect("reserve the matching CI failure");
    let ReserveAgentTriggerFiringOutcome::Reserved(receipt) = first else {
        panic!("the first matching failure must reserve, got {first:?}");
    };
    assert_eq!(receipt.state, AgentTriggerFiringState::Queued);

    let redelivery = triggers
        .reserve_firing(
            &tenant,
            proposal.binding_id,
            "ci-failed-1",
            "ci.run.failed",
            event,
            1,
            false,
            Utc::now(),
        )
        .await
        .expect("redeliver the same event after a process restart");
    assert!(
        matches!(
            redelivery,
            ReserveAgentTriggerFiringOutcome::AlreadyReserved(_)
        ),
        "the same durable event wakes no second agent run"
    );

    let history = triggers
        .list_firings_for_owner(&tenant, "founder", proposal.binding_id, None, 100)
        .await
        .expect("the owner can inspect what the trigger actually reserved");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].event_id, "ci-failed-1");
    assert_eq!(history[0].state, AgentTriggerFiringState::Queued);
    assert_eq!(
        history[0].run_id, None,
        "queued is not misreported as started"
    );

    let incompatible = triggers
        .claim_next_firing(
            &tenant,
            AgentTriggerClaimRequest::new("external:mcp", "external-worker", 30).unwrap(),
        )
        .await
        .expect("look for work for an incompatible runtime");
    assert_eq!(
        incompatible, None,
        "an external MCP worker cannot steal a hosted agent's firing"
    );

    let first_claim = triggers
        .claim_next_firing(
            &tenant,
            AgentTriggerClaimRequest::new("hosted:luna", "host-1", 30).unwrap(),
        )
        .await
        .expect("claim the firing for its exact runtime")
        .expect("one hosted firing is claimable");
    assert_eq!(first_claim.event_id, "ci-failed-1");
    assert_eq!(first_claim.run_as_agent_id, agent_id.to_string());
    assert_eq!(first_claim.owner_principal_id, "founder");
    assert_eq!(first_claim.runtime_ref, "hosted:luna");
    assert_eq!(first_claim.claim_attempts, 1);
    assert_eq!(first_claim.claim_owner, "host-1");

    let held = triggers
        .claim_next_firing(
            &tenant,
            AgentTriggerClaimRequest::new("hosted:luna", "host-2", 30).unwrap(),
        )
        .await
        .expect("another worker observes the live lease");
    assert_eq!(held, None, "a live claim is never double-delivered");

    let expiry_tenant = tenant.clone();
    app.with_tenant_tx(&tenant, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "UPDATE agent_trigger_firing \
                    SET claim_until = clock_timestamp() - INTERVAL '1 second' \
                  WHERE tenant_id = $1 AND event_id = 'ci-failed-1'",
            )
            .bind(&expiry_tenant)
            .execute(&mut *conn)
            .await
            .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            Ok(())
        })
    })
    .await
    .expect("the database clock expires the abandoned worker lease");
    let reclaimed = triggers
        .claim_next_firing(
            &tenant,
            AgentTriggerClaimRequest::new("hosted:luna", "host-2", 30).unwrap(),
        )
        .await
        .expect("reclaim work after its worker disappears")
        .expect("the expired claim returns to the runtime queue");
    assert_eq!(reclaimed.event_id, "ci-failed-1");
    assert_eq!(reclaimed.claim_attempts, 2);
    assert_eq!(reclaimed.claim_owner, "host-2");

    let later_failure = triggers
        .reserve_firing(
            &tenant,
            proposal.binding_id,
            "ci-failed-2",
            "ci.run.failed",
            serde_json::json!({"event_id": "ci-failed-2"}),
            1,
            false,
            Utc::now(),
        )
        .await
        .expect("consider a second matching failure");
    assert_eq!(
        later_failure,
        ReserveAgentTriggerFiringOutcome::BudgetExhausted,
        "the binding's durable budget is enforced across events and replicas"
    );

    let suspended_agent = format!("agent:{agent_id}");
    let suspension_tenant = tenant.clone();
    app.with_tenant_tx(&tenant, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "UPDATE principal SET status = '\"Suspended\"' \
                  WHERE tenant_id = $1 AND principal_id = $2",
            )
            .bind(&suspension_tenant)
            .bind(&suspended_agent)
            .execute(&mut *conn)
            .await
            .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            Ok(())
        })
    })
    .await
    .expect("suspend triage-bot through the durable identity state");
    let unavailable = triggers
        .reserve_firing(
            &tenant,
            proposal.binding_id,
            "ci-failed-after-suspension",
            "ci.run.failed",
            serde_json::json!({"event_id": "ci-failed-after-suspension"}),
            1,
            false,
            Utc::now(),
        )
        .await
        .expect("observe the existing binding after suspension");
    assert_eq!(
        unavailable,
        ReserveAgentTriggerFiringOutcome::BindingUnavailable,
        "a durable trigger never wakes an agent whose identity is no longer active"
    );

    let disabled = triggers
        .change_lifecycle(
            &tenant,
            "founder",
            proposal.binding_id,
            AgentTriggerLifecycleAction::Disable,
        )
        .await
        .expect("the owner permanently retires the automation");
    let ChangeAgentTriggerLifecycleOutcome::Complete(disabled) = disabled else {
        panic!("an active trigger can be disabled, got {disabled:?}");
    };
    assert!(disabled.changed);
    assert_eq!(disabled.binding.state, "disabled");
    assert_eq!(
        disabled.canceled_firings, 1,
        "the claimed but not started run is atomically canceled"
    );
    let final_history = triggers
        .list_firings_for_owner(&tenant, "founder", proposal.binding_id, None, 100)
        .await
        .expect("the owner sees the retired automation's final history");
    assert_eq!(final_history[0].state, AgentTriggerFiringState::Terminal);
    assert_eq!(final_history[0].run_id, None);
    assert_eq!(final_history[0].outcome, None);
    assert_eq!(
        triggers
            .change_lifecycle(
                &tenant,
                "founder",
                proposal.binding_id,
                AgentTriggerLifecycleAction::Resume,
            )
            .await
            .expect("retirement remains fail-closed"),
        ChangeAgentTriggerLifecycleOutcome::InvalidTransition
    );

    let cleanup_tenant = tenant.clone();
    app.with_tenant_tx(&tenant, move |conn| {
        Box::pin(async move {
            sqlx::query("DELETE FROM agent_trigger_firing WHERE tenant_id = $1")
                .bind(&cleanup_tenant)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            sqlx::query("DELETE FROM agent_trigger_binding WHERE tenant_id = $1")
                .bind(&cleanup_tenant)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            sqlx::query("DELETE FROM identity_agent WHERE tenant_id = $1")
                .bind(&cleanup_tenant)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
            sqlx::query("DELETE FROM principal WHERE tenant_id = $1")
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
