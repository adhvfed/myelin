#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    all_durable_migrations, AgentTriggerApprovalDecision, AgentTriggerCapacityScope,
    AgentTriggerClaimRequest, AgentTriggerEvaluationErrorCode, AgentTriggerFiringState,
    AgentTriggerLifecycleAction, AgentTriggerStartRequest, ChangeAgentTriggerApprovalOutcome,
    ChangeAgentTriggerLifecycleOutcome, CreateAgentTriggerBindingOutcome,
    DurableAgentTriggerBacking, NewAgentTriggerBinding, ReserveAgentTriggerFiringOutcome,
    StartAgentTriggerFiringOutcome, SubstrateProvider, TerminalizeAgentTriggerClaimOutcome,
};
use sqlx::types::chrono::Utc;
use std::time::Duration;
use uuid::Uuid;

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
                       ($1, $2, 'reviewer', $3, $4, $5), \
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

async fn seed_agent_for_owner(
    provider: &SubstrateProvider,
    tenant: &str,
    agent_id: Uuid,
    owner: &str,
    runtime_ref: &str,
) {
    let tenant = tenant.to_string();
    let region = provider.config().region.clone();
    let agent_kind = serde_json::to_string(&PrincipalKind::Agent {
        runtime_ref: RuntimeRef(runtime_ref.into()),
        on_behalf_of: Some(PrincipalId(owner.into())),
    })
    .unwrap();
    let processor = serde_json::to_string(&DataRole::Processor).unwrap();
    let active = serde_json::to_string(&PrincipalStatus::Active).unwrap();
    let agent_principal = format!("agent:{agent_id}");
    let owner = owner.to_string();
    let runtime_ref = runtime_ref.to_string();
    provider
        .with_tenant_tx(&tenant.clone(), move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO principal \
                       (tenant_id, region, principal_id, kind, data_role, status) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&agent_principal)
                .bind(&agent_kind)
                .bind(&processor)
                .bind(&active)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                sqlx::query(
                    "INSERT INTO identity_agent \
                       (tenant_id, region, agent_id, name, runtime_ref, created_by, client_nonce, \
                        tools, grants, created_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, \
                             ARRAY['issue.create'], ARRAY['issue.create'], clock_timestamp())",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(agent_id)
                .bind(format!("{owner}-automation"))
                .bind(&runtime_ref)
                .bind(&owner)
                .bind(format!("agent-create-{owner}"))
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .expect("seed an active agent owned by the selected human");
}

async fn clean_tenant(provider: &SubstrateProvider, tenant: &str) {
    let cleanup_tenant = tenant.to_string();
    provider
        .with_tenant_tx(tenant, move |conn| {
            Box::pin(async move {
                for table in [
                    "agent_trigger_firing",
                    "agent_trigger_binding",
                    "identity_agent",
                    "principal",
                ] {
                    sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id = $1"))
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
async fn an_automation_roster_starts_with_recent_work_and_pages_from_there() {
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
        .expect("the governed trigger schema migrates with the durable platform");

    let app = SubstrateProvider::connect(app_config(), 8)
        .await
        .expect("open the constrained app provider");
    let tenant = unique_tenant();
    let agent_id = Uuid::new_v4();
    seed_people_and_agent(&app, &tenant, agent_id, "hosted:luna").await;
    let triggers = DurableAgentTriggerBacking::new(app.clone());
    let now = Utc::now();
    let older = Uuid::parse_str("f0000000-0000-4000-8000-000000000001").unwrap();
    let middle = Uuid::parse_str("10000000-0000-4000-8000-000000000002").unwrap();
    let newest = Uuid::parse_str("80000000-0000-4000-8000-000000000003").unwrap();

    for (binding_id, client_nonce, created_at) in [
        (older, "roster-older", now - Duration::from_secs(2)),
        (middle, "roster-middle", now - Duration::from_secs(1)),
        (newest, "roster-newest", now),
    ] {
        let mut proposal = red_mainline_binding(agent_id);
        proposal.binding_id = binding_id;
        proposal.client_nonce = client_nonce.into();
        proposal.created_at = created_at;
        assert!(matches!(
            triggers.create(&tenant, proposal).await.unwrap(),
            CreateAgentTriggerBindingOutcome::Created(_)
        ));
    }

    let first_page = triggers
        .list_for_owner(&tenant, "founder", None, 2)
        .await
        .expect("the owner opens their automation roster");
    assert_eq!(
        first_page
            .iter()
            .map(|binding| binding.binding_id.as_str())
            .collect::<Vec<_>>(),
        [newest.to_string(), middle.to_string()],
        "the roster follows human recency rather than random UUID order"
    );
    let second_page = triggers
        .list_for_owner(&tenant, "founder", Some(middle), 2)
        .await
        .expect("the owner continues from the last visible automation");
    assert_eq!(
        second_page
            .iter()
            .map(|binding| binding.binding_id.as_str())
            .collect::<Vec<_>>(),
        [older.to_string()],
        "the recency cursor neither repeats nor skips older work"
    );

    clean_tenant(&app, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_human_binding_wakes_one_named_agent_once_when_main_goes_red() {
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
        .expect("the governed trigger schema migrates with the durable platform");

    let app = SubstrateProvider::connect(app_config(), 8)
        .await
        .expect("open the constrained app provider");
    let tenant = unique_tenant();
    let agent_id = Uuid::parse_str("20000000-0000-4000-8000-000000000002").unwrap();
    seed_people_and_agent(&app, &tenant, agent_id, "hosted:luna").await;
    let triggers = DurableAgentTriggerBacking::new(app.clone());
    let proposal = red_mainline_binding(agent_id);

    let mut borrowed_agent = proposal.clone();
    borrowed_agent.binding_id = Uuid::new_v4();
    borrowed_agent.owner_principal_id = "reviewer".into();
    borrowed_agent.client_nonce = "reviewer-cannot-spend-founders-agent".into();
    assert_eq!(
        triggers.create(&tenant, borrowed_agent).await.unwrap(),
        CreateAgentTriggerBindingOutcome::AgentUnavailable,
        "a colleague cannot author prompts or spend through an agent they do not own"
    );

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
    assert_eq!(
        triggers
            .get_for_owner(&tenant, "founder", proposal.binding_id)
            .await
            .expect("the owner addresses the new automation directly"),
        Some(binding.clone())
    );
    assert_eq!(
        triggers
            .get_for_owner(&tenant, "reviewer", proposal.binding_id)
            .await
            .expect("a peer cannot discover the automation through direct lookup"),
        None
    );
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

    let mut guarded = proposal.clone();
    guarded.binding_id = Uuid::parse_str("10000000-0000-4000-8000-000000000010").unwrap();
    guarded.client_nonce = "red-main-needs-a-human".into();
    guarded.max_firings = 2;
    guarded.require_human_approval = true;
    triggers
        .create(&tenant, guarded.clone())
        .await
        .expect("the founder creates a guarded variant");
    for event_id in ["guarded-red-approved", "guarded-red-rejected"] {
        let reserved = triggers
            .reserve_firing(
                &tenant,
                guarded.binding_id,
                event_id,
                "ci.run.failed",
                serde_json::json!({"event_id": event_id}),
                1,
                false,
                Utc::now(),
            )
            .await
            .expect("a matching guarded event waits for a person");
        assert!(matches!(
            reserved,
            ReserveAgentTriggerFiringOutcome::Reserved(ref firing)
                if firing.state == AgentTriggerFiringState::AwaitingApproval
        ));
    }
    assert_eq!(
        triggers
            .claim_next_firing(
                &tenant,
                AgentTriggerClaimRequest::new("hosted:luna", "host-before-approval", 30).unwrap(),
            )
            .await
            .expect("the worker checks its queue"),
        None,
        "human-gated work cannot race ahead of its decision"
    );
    assert_eq!(
        triggers
            .change_firing_approval(
                &tenant,
                "reviewer",
                guarded.binding_id,
                "guarded-red-approved",
                AgentTriggerApprovalDecision::Approve,
            )
            .await
            .expect("a peer cannot infer or decide another person's guarded work"),
        ChangeAgentTriggerApprovalOutcome::NotFound
    );
    let approved = triggers
        .change_firing_approval(
            &tenant,
            "founder",
            guarded.binding_id,
            "guarded-red-approved",
            AgentTriggerApprovalDecision::Approve,
        )
        .await
        .expect("the owner approves the exact event");
    let ChangeAgentTriggerApprovalOutcome::Complete(approved) = approved else {
        panic!("an awaiting firing must be approvable, got {approved:?}");
    };
    assert!(approved.changed);
    assert_eq!(approved.firing.state, AgentTriggerFiringState::Queued);
    assert_eq!(
        approved.firing.approval_decision,
        Some(AgentTriggerApprovalDecision::Approve)
    );
    assert_eq!(
        approved.firing.approval_decided_by.as_deref(),
        Some("founder")
    );
    assert!(approved.firing.approval_decided_at.is_some());
    let replayed_approval = triggers
        .change_firing_approval(
            &tenant,
            "founder",
            guarded.binding_id,
            "guarded-red-approved",
            AgentTriggerApprovalDecision::Approve,
        )
        .await
        .expect("the CLI safely retries its approval");
    assert!(matches!(
        replayed_approval,
        ChangeAgentTriggerApprovalOutcome::Complete(ref outcome) if !outcome.changed
    ));
    let approved_claim = triggers
        .claim_next_firing(
            &tenant,
            AgentTriggerClaimRequest::new("hosted:luna", "host-after-approval", 30).unwrap(),
        )
        .await
        .expect("the worker checks its queue after approval")
        .expect("approved work becomes claimable");
    assert_eq!(approved_claim.event_id, "guarded-red-approved");

    let rejected = triggers
        .change_firing_approval(
            &tenant,
            "founder",
            guarded.binding_id,
            "guarded-red-rejected",
            AgentTriggerApprovalDecision::Reject,
        )
        .await
        .expect("the owner rejects the other exact event");
    assert!(matches!(
        rejected,
        ChangeAgentTriggerApprovalOutcome::Complete(ref outcome)
            if outcome.changed
                && outcome.firing.state == AgentTriggerFiringState::Terminal
                && outcome.firing.run_id.is_none()
                && outcome.firing.approval_decision == Some(AgentTriggerApprovalDecision::Reject)
    ));
    assert_eq!(
        triggers
            .change_firing_approval(
                &tenant,
                "founder",
                guarded.binding_id,
                "guarded-red-rejected",
                AgentTriggerApprovalDecision::Approve,
            )
            .await
            .expect("a rejected decision is final"),
        ChangeAgentTriggerApprovalOutcome::InvalidTransition
    );
    let guarded_disabled = triggers
        .change_lifecycle(
            &tenant,
            "founder",
            guarded.binding_id,
            AgentTriggerLifecycleAction::Disable,
        )
        .await
        .expect("retire the completed approval exercise");
    assert!(matches!(
        guarded_disabled,
        ChangeAgentTriggerLifecycleOutcome::Complete(ref outcome)
            if outcome.canceled_firings == 1
    ));
    let guarded_history = triggers
        .list_firings_for_owner(&tenant, "founder", guarded.binding_id, None, 100)
        .await
        .expect("the approval decisions remain inspectable");
    assert_eq!(guarded_history.len(), 2);
    assert!(guarded_history
        .iter()
        .all(|firing| firing.state == AgentTriggerFiringState::Terminal));
    assert!(guarded_history
        .iter()
        .all(|firing| firing.approval_decision.is_some()));

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
    let run_id = Uuid::new_v4();
    let start = AgentTriggerStartRequest::from_claim(&reclaimed, run_id).unwrap();
    let start_tenant = tenant.clone();
    let start_backing = triggers.clone();
    assert_eq!(
        app.with_tenant_tx(&tenant, move |conn| {
            Box::pin(async move {
                start_backing
                    .start_claimed_firing_on_conn(conn, &start_tenant, &start)
                    .await
            })
        })
        .await
        .expect("promote the reclaimed firing to durable started work"),
        StartAgentTriggerFiringOutcome::Started
    );

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

    let refused = triggers
        .change_lifecycle(
            &tenant,
            "founder",
            proposal.binding_id,
            AgentTriggerLifecycleAction::Disable,
        )
        .await
        .expect_err("a storage-only lifecycle call cannot strand started workflow state");
    assert!(
        refused
            .to_string()
            .contains("coordinated lifecycle cleanup"),
        "the refusal explains how live work must be closed: {refused}"
    );
    assert_eq!(
        triggers
            .get_for_owner(&tenant, "founder", proposal.binding_id)
            .await
            .expect("the refused transaction leaves the binding readable")
            .expect("the binding still exists")
            .state,
        "active",
        "the whole storage-only disable rolls back"
    );

    let lifecycle_tenant = tenant.clone();
    let lifecycle_backing = triggers.clone();
    let disabled = app
        .with_tenant_tx(&tenant, move |conn| {
            Box::pin(async move {
                lifecycle_backing
                    .change_lifecycle_on_conn(
                        conn,
                        &lifecycle_tenant,
                        "founder",
                        proposal.binding_id,
                        AgentTriggerLifecycleAction::Disable,
                    )
                    .await
            })
        })
        .await
        .expect("the control-plane transaction fences the automation and its live run");
    let ChangeAgentTriggerLifecycleOutcome::Complete(disabled) = disabled else {
        panic!("an active trigger can be disabled, got {disabled:?}");
    };
    assert!(disabled.changed);
    assert_eq!(disabled.binding.state, "disabled");
    assert_eq!(
        disabled.canceled_firings, 1,
        "the started run is fenced for coordinated cleanup"
    );
    assert_eq!(disabled.canceled_run_ids, [run_id.to_string()]);
    let final_history = triggers
        .list_firings_for_owner(&tenant, "founder", proposal.binding_id, None, 100)
        .await
        .expect("the owner sees the retired automation's final history");
    assert_eq!(final_history[0].state, AgentTriggerFiringState::Terminal);
    assert_eq!(final_history[0].run_id, Some(run_id.to_string()));
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

    clean_tenant(&app, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_automation_capacity_is_fair_retry_safe_and_concurrency_fenced() {
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
        .expect("the governed trigger schema migrates with the durable platform");

    let app = SubstrateProvider::connect(app_config(), 8)
        .await
        .expect("open the constrained app provider");
    let tenant = unique_tenant();
    let founder_agent = Uuid::new_v4();
    let reviewer_agent = Uuid::new_v4();
    seed_people_and_agent(&app, &tenant, founder_agent, "hosted:luna").await;
    seed_agent_for_owner(&app, &tenant, reviewer_agent, "reviewer", "hosted:luna").await;
    let triggers = DurableAgentTriggerBacking::with_capacity_for_test(app.clone(), 2, 2);

    let mut first = red_mainline_binding(founder_agent);
    first.binding_id = Uuid::new_v4();
    first.client_nonce = "founder-first-red-mainline".into();
    let mut second = first.clone();
    second.binding_id = Uuid::new_v4();
    second.client_nonce = "founder-second-red-mainline".into();
    assert!(matches!(
        triggers.create(&tenant, first.clone()).await.unwrap(),
        CreateAgentTriggerBindingOutcome::Created(_)
    ));
    assert!(matches!(
        triggers.create(&tenant, second.clone()).await.unwrap(),
        CreateAgentTriggerBindingOutcome::Created(_)
    ));

    let mut exact_retry = second.clone();
    exact_retry.binding_id = Uuid::new_v4();
    assert!(
        matches!(
            triggers.create(&tenant, exact_retry).await.unwrap(),
            CreateAgentTriggerBindingOutcome::Replayed(_)
        ),
        "an exact retry still finds its durable result when the event is full"
    );

    let mut third_for_founder = first.clone();
    third_for_founder.binding_id = Uuid::new_v4();
    third_for_founder.client_nonce = "founder-third-red-mainline".into();
    assert_eq!(
        triggers.create(&tenant, third_for_founder).await.unwrap(),
        CreateAgentTriggerBindingOutcome::CapacityReached(AgentTriggerCapacityScope::OwnerEvent),
        "one owner cannot monopolize an event's automation capacity"
    );

    let mut first_for_reviewer = first.clone();
    first_for_reviewer.binding_id = Uuid::new_v4();
    first_for_reviewer.owner_principal_id = "reviewer".into();
    first_for_reviewer.run_as_agent_id = reviewer_agent;
    first_for_reviewer.client_nonce = "reviewer-first-red-mainline".into();
    assert_eq!(
        triggers
            .create(&tenant, first_for_reviewer.clone())
            .await
            .unwrap(),
        CreateAgentTriggerBindingOutcome::CapacityReached(AgentTriggerCapacityScope::TenantEvent),
        "the shared event limit remains explicit even when this owner has room"
    );

    triggers
        .change_lifecycle(
            &tenant,
            "founder",
            first.binding_id,
            AgentTriggerLifecycleAction::Pause,
        )
        .await
        .expect("the founder makes room for a collaborator");
    assert!(matches!(
        triggers
            .create(&tenant, first_for_reviewer.clone())
            .await
            .unwrap(),
        CreateAgentTriggerBindingOutcome::Created(_)
    ));
    assert_eq!(
        triggers
            .change_lifecycle(
                &tenant,
                "founder",
                first.binding_id,
                AgentTriggerLifecycleAction::Resume,
            )
            .await
            .unwrap(),
        ChangeAgentTriggerLifecycleOutcome::CapacityReached(AgentTriggerCapacityScope::TenantEvent),
        "resume cannot bypass the same capacity contract as creation"
    );
    triggers
        .change_lifecycle(
            &tenant,
            "reviewer",
            first_for_reviewer.binding_id,
            AgentTriggerLifecycleAction::Pause,
        )
        .await
        .expect("the reviewer makes room again");
    assert!(matches!(
        triggers
            .change_lifecycle(
                &tenant,
                "founder",
                first.binding_id,
                AgentTriggerLifecycleAction::Resume,
            )
            .await
            .unwrap(),
        ChangeAgentTriggerLifecycleOutcome::Complete(ref outcome)
            if outcome.changed && outcome.binding.state == "active"
    ));
    clean_tenant(&app, &tenant).await;

    let concurrent_tenant = unique_tenant();
    let concurrent_founder_agent = Uuid::new_v4();
    let concurrent_reviewer_agent = Uuid::new_v4();
    seed_people_and_agent(
        &app,
        &concurrent_tenant,
        concurrent_founder_agent,
        "hosted:luna",
    )
    .await;
    seed_agent_for_owner(
        &app,
        &concurrent_tenant,
        concurrent_reviewer_agent,
        "reviewer",
        "hosted:luna",
    )
    .await;
    let one_slot = DurableAgentTriggerBacking::with_capacity_for_test(app.clone(), 1, 1);
    let mut founder_race = red_mainline_binding(concurrent_founder_agent);
    founder_race.binding_id = Uuid::new_v4();
    founder_race.client_nonce = "founder-races-for-one-slot".into();
    let mut reviewer_race = founder_race.clone();
    reviewer_race.binding_id = Uuid::new_v4();
    reviewer_race.owner_principal_id = "reviewer".into();
    reviewer_race.run_as_agent_id = concurrent_reviewer_agent;
    reviewer_race.client_nonce = "reviewer-races-for-one-slot".into();
    let (founder_outcome, reviewer_outcome) = tokio::join!(
        one_slot.create(&concurrent_tenant, founder_race),
        one_slot.create(&concurrent_tenant, reviewer_race),
    );
    let outcomes = [founder_outcome.unwrap(), reviewer_outcome.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, CreateAgentTriggerBindingOutcome::Created(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| {
                matches!(
                    outcome,
                    CreateAgentTriggerBindingOutcome::CapacityReached(
                        AgentTriggerCapacityScope::TenantEvent
                    )
                )
            })
            .count(),
        1,
        "concurrent owners cannot overbook the last event slot"
    );
    assert_eq!(
        one_slot
            .active_for_event(&concurrent_tenant, "ci.run.failed", 10)
            .await
            .expect("the consumer sees the bounded result")
            .len(),
        1
    );
    clean_tenant(&app, &concurrent_tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_poison_firing_becomes_explainable_history_without_harming_the_queue() {
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
        .expect("the terminal-reason migration is part of the durable platform");

    let app = SubstrateProvider::connect(app_config(), 8)
        .await
        .expect("open the constrained app provider");
    let tenant = unique_tenant();
    let agent_id = Uuid::new_v4();
    seed_people_and_agent(&app, &tenant, agent_id, "hosted:luna").await;
    let triggers = DurableAgentTriggerBacking::new(app.clone());
    let mut proposal = red_mainline_binding(agent_id);
    proposal.binding_id = Uuid::new_v4();
    proposal.client_nonce = "poison-firing-isolation".into();
    triggers
        .create(&tenant, proposal.clone())
        .await
        .expect("the founder creates one narrow automation");
    triggers
        .reserve_firing(
            &tenant,
            proposal.binding_id,
            "malformed-event",
            "ci.run.failed",
            serde_json::json!({"event_id": "malformed-event"}),
            1,
            false,
            Utc::now(),
        )
        .await
        .expect("the event consumer reserves the matching firing");
    let claim = triggers
        .claim_next_firing(
            &tenant,
            AgentTriggerClaimRequest::new("hosted:luna", "worker-a", 30).unwrap(),
        )
        .await
        .expect("the worker can reach its queue")
        .expect("the firing is claimable");

    let mut stale_replica = claim.clone();
    stale_replica.claim_owner = "worker-b".into();
    assert_eq!(
        triggers
            .terminalize_claim(&tenant, &stale_replica, "invalid trigger claim")
            .await
            .expect("a stale replica gets a fenced answer"),
        TerminalizeAgentTriggerClaimOutcome::ClaimUnavailable,
        "a worker can only isolate the exact claim it owns"
    );
    let reason = "invalid trigger claim: envelope identity does not match its firing record";
    assert_eq!(
        triggers
            .terminalize_claim(&tenant, &claim, reason)
            .await
            .expect("the owning worker isolates poison work"),
        TerminalizeAgentTriggerClaimOutcome::Terminalized
    );
    assert_eq!(
        triggers
            .claim_next_firing(
                &tenant,
                AgentTriggerClaimRequest::new("hosted:luna", "worker-b", 30).unwrap(),
            )
            .await
            .expect("the healthy worker keeps polling"),
        None,
        "poison work never crash-loops after its lease"
    );

    let history = triggers
        .list_firings_for_owner(&tenant, "founder", proposal.binding_id, None, 25)
        .await
        .expect("the owner can understand why no run appeared");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, AgentTriggerFiringState::Terminal);
    assert_eq!(history[0].run_id, None);
    assert_eq!(history[0].terminal_reason.as_deref(), Some(reason));

    clean_tenant(&app, &tenant).await;
}

#[tokio::test]
async fn an_automation_owner_sees_the_newest_rule_evaluation_error() {
    let admin = SubstrateProvider::connect(admin_config(), 4)
        .await
        .expect("open the migration provider");
    admin
        .migrate_foundation()
        .await
        .expect("foundation is present");
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("the automation diagnostic is part of the durable platform");

    let app = SubstrateProvider::connect(app_config(), 8)
        .await
        .expect("open the constrained app provider");
    let tenant = unique_tenant();
    let agent_id = Uuid::new_v4();
    seed_people_and_agent(&app, &tenant, agent_id, "hosted:luna").await;
    let triggers = DurableAgentTriggerBacking::new(app.clone());
    let mut proposal = red_mainline_binding(agent_id);
    proposal.binding_id = Uuid::new_v4();
    proposal.client_nonce = "visible-evaluation-diagnostic".into();
    triggers
        .create(&tenant, proposal.clone())
        .await
        .expect("the founder creates one narrow automation");

    let older = sqlx::types::chrono::DateTime::parse_from_rfc3339("2026-08-10T08:00:01Z")
        .unwrap()
        .with_timezone(&Utc);
    let newer = sqlx::types::chrono::DateTime::parse_from_rfc3339("2026-08-10T08:00:02Z")
        .unwrap()
        .with_timezone(&Utc);
    assert!(triggers
        .record_evaluation_error(
            &tenant,
            proposal.binding_id,
            "event-newer",
            AgentTriggerEvaluationErrorCode::TypeError,
            "comparison is not defined over the operand types",
            newer,
        )
        .await
        .expect("the evaluation failure is recorded"));
    assert!(!triggers
        .record_evaluation_error(
            &tenant,
            proposal.binding_id,
            "event-older",
            AgentTriggerEvaluationErrorCode::MissingContext,
            "an older delivery cannot replace a newer diagnosis",
            older,
        )
        .await
        .expect("an older delivery is ignored deterministically"));

    let binding = triggers
        .get_for_owner(&tenant, "founder", proposal.binding_id)
        .await
        .expect("the owner can inspect the automation")
        .expect("the automation remains present");
    let diagnostic = binding
        .last_evaluation_error
        .expect("the owner receives an actionable evaluation diagnostic");
    assert_eq!(diagnostic.code, AgentTriggerEvaluationErrorCode::TypeError);
    assert_eq!(diagnostic.event_id, "event-newer");
    assert_eq!(
        diagnostic.detail,
        "comparison is not defined over the operand types"
    );
    assert!(diagnostic
        .event_recorded_at
        .starts_with("2026-08-10T08:00:02"));
    assert!(
        triggers
            .get_for_owner(&tenant, "reviewer", proposal.binding_id)
            .await
            .expect("another human gets a scoped answer")
            .is_none(),
        "diagnostics follow automation ownership rather than becoming tenant-wide telemetry"
    );

    clean_tenant(&app, &tenant).await;
}
