#![cfg(feature = "integration")]

use chrono::{DateTime, Duration, SecondsFormat, TimeZone, Utc};
use myelin_config::MyelinConfig;
use myelin_events::IdMinter;
use myelin_identity::{DataRole, PrincipalKind, PrincipalStatus, RuntimeRef};
use myelin_storage::{
    all_durable_migrations, ActivateAgentThreadOutcome, AgentThreadExpiryCompletion,
    AgentThreadExpiryFailure, BindAgentThreadRunOutcome, CreateAgentThreadOutcome,
    CreateWorkspaceSshGrantOutcome, DurableAgentThreadBacking, HotTables,
    ListWorkspaceSessionsOutcome, NewAgentThread, NewWorkspaceSshGrant, NewWorkspaceSshSession,
    SealKey, SubstrateProvider, WorkspaceSessionMode, WorkspaceSshRouteKey,
    AGENT_THREAD_EXPIRY_GRACE_SECONDS, WORKSPACE_SSH_SESSION_STARTED,
};
use sqlx::types::Uuid;

fn test_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

fn admin_config(config: &MyelinConfig) -> MyelinConfig {
    let mut admin = config.clone();
    admin.database_url = admin
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    admin
}

fn unique() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn proposal(owner: &str, agent_id: Uuid, suffix: u128) -> NewAgentThread {
    let created_at = Utc.timestamp_opt(1_787_356_800, 0).single().unwrap();
    NewAgentThread {
        thread_id: Uuid::from_u128(suffix),
        owner_principal_id: owner.into(),
        agent_id,
        conversation_id: format!("01J{:023}", suffix % 10_u128.pow(23)),
        workspace_id: Uuid::from_u128(suffix + 10_000),
        name: "Investigate checkout race".into(),
        project_id: None,
        retention_days: 3,
        client_nonce: "private-thread-retry".into(),
        created_at,
        expires_at: created_at + Duration::days(3),
    }
}

async fn seed_agent(provider: &SubstrateProvider, tenant: &str, owner: &str, agent_id: Uuid) {
    let tenant = tenant.to_string();
    let region = provider.config().region.clone();
    let owner = owner.to_string();
    provider
        .with_tenant_tx(&tenant.clone(), move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO principal (tenant_id, region, principal_id, kind, data_role, status)
                     VALUES ($1,$2,$3,$4,$5,$6)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&owner)
                .bind(serde_json::to_string(&PrincipalKind::Human).unwrap())
                .bind(serde_json::to_string(&DataRole::Controller).unwrap())
                .bind(serde_json::to_string(&PrincipalStatus::Active).unwrap())
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                let agent_principal = format!("agent:{agent_id}");
                sqlx::query(
                    "INSERT INTO principal (tenant_id, region, principal_id, kind, data_role, status)
                     VALUES ($1,$2,$3,$4,$5,$6)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&agent_principal)
                .bind(
                    serde_json::to_string(&PrincipalKind::Agent {
                        runtime_ref: RuntimeRef("external:mcp".into()),
                        on_behalf_of: Some(myelin_identity::PrincipalId(owner.clone())),
                    })
                    .unwrap(),
                )
                .bind(serde_json::to_string(&DataRole::Controller).unwrap())
                .bind(serde_json::to_string(&PrincipalStatus::Active).unwrap())
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                sqlx::query(
                    "INSERT INTO identity_agent (
                       tenant_id, region, agent_id, name, runtime_ref, created_by,
                       client_nonce, tools, grants, created_at)
                     VALUES ($1,$2,$3,$4,'external:mcp',$5,$6,$7,$8,CURRENT_TIMESTAMP)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(agent_id)
                .bind(format!("helper-{agent_id}"))
                .bind(&owner)
                .bind(format!("agent-{agent_id}"))
                .bind(vec!["chat.read_messages"])
                .bind(vec!["chat.read"])
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .unwrap();
}

async fn set_agent_status(
    provider: &SubstrateProvider,
    tenant: &str,
    agent_id: Uuid,
    status: PrincipalStatus,
) {
    let tenant = tenant.to_string();
    let region = provider.config().region.clone();
    provider
        .with_tenant_tx(&tenant.clone(), move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE principal SET status = $4
                      WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(format!("agent:{agent_id}"))
                .bind(serde_json::to_string(&status).unwrap())
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .unwrap();
}

async fn seed_ready_run(
    provider: &SubstrateProvider,
    tenant: &str,
    owner: &str,
    agent_id: Uuid,
    run_id: Uuid,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) {
    let tenant = tenant.to_string();
    let region = provider.config().region.clone();
    let owner = owner.to_string();
    provider
        .with_tenant_tx(&tenant.clone(), move |connection| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO external_agent_run (
                       tenant_id, region, run_id, agent_id, trigger_actor_id,
                       trigger_credential_jti, trigger_authority, client_nonce, token_jti,
                       state, issued_at, expires_at)
                     VALUES ($1,$2,$3,$4,$5,'browser-session',ARRAY['agent.run'],
                             'thread-run-retry','thread-run-jti','ready',$6,$7)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(run_id)
                .bind(agent_id)
                .bind(&owner)
                .bind(issued_at)
                .bind(expires_at)
                .execute(&mut *connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_thread_is_one_retry_safe_workspace_lifecycle() {
    let config = test_config();
    let admin = SubstrateProvider::connect(admin_config(&config), 4)
        .await
        .expect("connect the migration role for the durable thread story");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    let app = SubstrateProvider::connect(config, 6)
        .await
        .expect("connect the constrained runtime role");
    let tenant = format!("agent-thread-{}", unique());
    let owner = "p:alice";
    let agent_id = Uuid::from_u128(41);
    seed_agent(&app, &tenant, owner, agent_id).await;
    let threads = DurableAgentThreadBacking::new(app.clone());
    let intended = proposal(owner, agent_id, 101);

    let created = threads
        .create(&tenant, intended.clone())
        .await
        .expect("claim the private thread");
    let CreateAgentThreadOutcome::Created(created) = created else {
        panic!("first request should create the thread: {created:?}");
    };
    assert_eq!(
        created.state,
        myelin_storage::AgentThreadState::Provisioning
    );
    assert_eq!(created.retention_days, 3);
    assert_eq!(created.expires_at, "2026-08-25T00:00:00Z");
    assert!(created.storage_locator.is_none());

    let replay_proposal = NewAgentThread {
        thread_id: Uuid::from_u128(102),
        conversation_id: "01J00000000000000000000102".into(),
        workspace_id: Uuid::from_u128(10_102),
        created_at: intended.created_at + Duration::hours(1),
        expires_at: intended.expires_at + Duration::hours(1),
        ..intended.clone()
    };
    set_agent_status(&app, &tenant, agent_id, PrincipalStatus::Suspended).await;
    let replay = threads.create(&tenant, replay_proposal).await.unwrap();
    assert_eq!(replay, CreateAgentThreadOutcome::Replayed(created.clone()));
    set_agent_status(&app, &tenant, agent_id, PrincipalStatus::Active).await;

    let conflicting_retry = threads
        .create(
            &tenant,
            NewAgentThread {
                name: "A different problem".into(),
                ..intended.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(conflicting_retry, CreateAgentThreadOutcome::Conflict);

    let same_name = threads
        .create(
            &tenant,
            NewAgentThread {
                thread_id: Uuid::from_u128(103),
                conversation_id: "01J00000000000000000000103".into(),
                workspace_id: Uuid::from_u128(10_103),
                client_nonce: "another-retry-key".into(),
                ..intended.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(same_name, CreateAgentThreadOutcome::NameConflict);

    let wrong_workspace = threads
        .activate(
            &tenant,
            owner,
            intended.thread_id,
            Uuid::from_u128(999),
            &intended.conversation_id,
            "workspace://local/wrong",
        )
        .await
        .unwrap();
    assert_eq!(wrong_workspace, ActivateAgentThreadOutcome::NotFound);

    let activated = threads
        .activate(
            &tenant,
            owner,
            intended.thread_id,
            intended.workspace_id,
            &intended.conversation_id,
            "workspace://local/101",
        )
        .await
        .unwrap();
    let ActivateAgentThreadOutcome::Activated(ready) = activated else {
        panic!("the exact provisioning receipt should activate: {activated:?}");
    };
    assert_eq!(ready.state, myelin_storage::AgentThreadState::Ready);
    assert_eq!(
        ready.storage_locator.as_deref(),
        Some("workspace://local/101")
    );

    assert_eq!(
        threads
            .activate(
                &tenant,
                owner,
                intended.thread_id,
                intended.workspace_id,
                &intended.conversation_id,
                "workspace://local/101",
            )
            .await
            .unwrap(),
        ActivateAgentThreadOutcome::AlreadyReady(ready.clone())
    );

    let ssh_issued_at = intended.created_at + Duration::hours(1);
    let ssh_grant_id = Uuid::from_u128(151);
    let route_key = WorkspaceSshRouteKey::from_seal_key(&SealKey::from_bytes([0x51; 32]));
    let route_username = route_key.seal(&tenant, ssh_grant_id).unwrap();
    let ssh_intent = NewWorkspaceSshGrant {
        grant_id: ssh_grant_id,
        route_username: route_username.clone(),
        thread_id: intended.thread_id,
        owner_principal_id: owner.into(),
        public_key_fingerprint: format!("SHA256:{}", "A".repeat(43)),
        client_nonce: "private-workspace-ssh-retry".into(),
        issued_at: ssh_issued_at,
        expires_at: ssh_issued_at + Duration::minutes(5),
    };
    let ssh_grant = threads
        .create_ssh_grant(&tenant, ssh_intent.clone())
        .await
        .unwrap();
    let CreateWorkspaceSshGrantOutcome::Created(ssh_grant) = ssh_grant else {
        panic!("the owner and ephemeral key should receive one grant: {ssh_grant:?}");
    };
    assert_eq!(ssh_grant.workspace_id, intended.workspace_id.to_string());
    assert_eq!(ssh_grant.workspace_generation, 1);
    assert_eq!(ssh_grant.route_username, route_username);
    assert_eq!(
        route_key.open(&route_username).unwrap().grant_id,
        ssh_grant_id
    );

    let retried_ssh_intent = NewWorkspaceSshGrant {
        grant_id: Uuid::from_u128(152),
        route_username: route_key.seal(&tenant, Uuid::from_u128(152)).unwrap(),
        issued_at: ssh_issued_at + Duration::seconds(30),
        expires_at: ssh_issued_at + Duration::minutes(5),
        ..ssh_intent.clone()
    };
    assert_eq!(
        threads
            .create_ssh_grant(&tenant, retried_ssh_intent)
            .await
            .unwrap(),
        CreateWorkspaceSshGrantOutcome::Replayed(ssh_grant.clone())
    );
    assert_eq!(
        threads
            .create_ssh_grant(
                &tenant,
                NewWorkspaceSshGrant {
                    public_key_fingerprint: format!("SHA256:{}", "B".repeat(43)),
                    ..ssh_intent.clone()
                },
            )
            .await
            .unwrap(),
        CreateWorkspaceSshGrantOutcome::Conflict
    );

    let admitted = threads
        .live_ssh_admission(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_issued_at,
        )
        .await
        .unwrap()
        .expect("the exact ephemeral key enters the exact live workspace generation");
    assert_eq!(admitted.thread_id, intended.thread_id.to_string());
    assert_eq!(admitted.workspace_id, intended.workspace_id.to_string());
    assert_eq!(admitted.workspace_generation, 1);
    assert_eq!(admitted.storage_locator, "workspace://local/101");
    assert!(threads
        .live_ssh_admission(
            &tenant,
            ssh_grant_id,
            &route_username,
            &format!("SHA256:{}", "B".repeat(43)),
            ssh_issued_at,
        )
        .await
        .unwrap()
        .is_none());
    assert!(threads
        .live_ssh_admission(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_intent.expires_at,
        )
        .await
        .unwrap()
        .is_none());
    assert!(threads
        .live_ssh_admission(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_intent.issued_at - Duration::seconds(1),
        )
        .await
        .unwrap()
        .is_none());
    let continuing_session = threads
        .live_ssh_session(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_issued_at,
            ssh_intent.expires_at + Duration::hours(1),
        )
        .await
        .unwrap()
        .expect("a session admitted in time continues after its connection grant expires");
    assert_eq!(continuing_session.workspace_id, admitted.workspace_id);
    let session_ids = myelin_events::UlidMinter::new();
    let first_session_id = session_ids.mint().0;
    let first_session = threads
        .start_ssh_session(
            &tenant,
            NewWorkspaceSshSession {
                session_id: first_session_id.clone(),
                grant_id: ssh_grant_id,
                route_username: route_username.clone(),
                public_key_fingerprint: ssh_intent.public_key_fingerprint.clone(),
                admitted_at: ssh_issued_at,
                started_at: ssh_issued_at + Duration::seconds(1),
                mode: WorkspaceSessionMode::Shell,
                terminal: true,
            },
        )
        .await
        .unwrap()
        .expect("a launched confined shell records its exact workspace generation");
    assert_eq!(first_session.admission, admitted);
    assert_eq!(first_session.session.session_id, first_session_id);
    assert_eq!(first_session.session.access_method, "ssh");
    assert_eq!(first_session.session.mode, WorkspaceSessionMode::Shell);
    assert!(first_session.session.terminal);

    let second_session_id = session_ids.mint().0;
    threads
        .start_ssh_session(
            &tenant,
            NewWorkspaceSshSession {
                session_id: second_session_id.clone(),
                started_at: ssh_issued_at + Duration::seconds(2),
                mode: WorkspaceSessionMode::Command,
                terminal: false,
                grant_id: ssh_grant_id,
                route_username: route_username.clone(),
                public_key_fingerprint: ssh_intent.public_key_fingerprint.clone(),
                admitted_at: ssh_issued_at,
            },
        )
        .await
        .unwrap()
        .expect("a launched confined command records a distinct access");
    let ListWorkspaceSessionsOutcome::Page(first_page) = threads
        .list_workspace_sessions_for_owner(&tenant, owner, intended.thread_id, None, 1)
        .await
        .unwrap()
    else {
        panic!("the first workspace history page has no cursor to resolve");
    };
    assert_eq!(first_page[0].session_id, second_session_id);
    let ListWorkspaceSessionsOutcome::Page(second_page) = threads
        .list_workspace_sessions_for_owner(
            &tenant,
            owner,
            intended.thread_id,
            Some(second_session_id.clone()),
            2,
        )
        .await
        .unwrap()
    else {
        panic!("the second workspace history page resolves its own cursor");
    };
    assert_eq!(second_page, vec![first_session.session.clone()]);
    assert_eq!(
        threads
            .list_workspace_sessions_for_owner(
                &tenant,
                owner,
                intended.thread_id,
                Some(session_ids.mint().0),
                2,
            )
            .await
            .unwrap(),
        ListWorkspaceSessionsOutcome::CursorNotFound
    );
    let event: serde_json::Value =
        sqlx::query_scalar("SELECT envelope FROM outbox WHERE event_id = $1")
            .bind(&first_session_id)
            .fetch_one(admin.db_pool())
            .await
            .unwrap();
    assert_eq!(event["type_"], WORKSPACE_SSH_SESSION_STARTED);
    assert_eq!(event["payload"]["method"], "ssh");
    assert_eq!(event["payload"]["mode"], "shell");
    let minimized = event["payload"].to_string();
    for forbidden in [
        "fingerprint",
        "grant",
        "host",
        "locator",
        "public_key",
        "route",
        "username",
    ] {
        assert!(!minimized.contains(forbidden));
    }
    assert!(threads
        .live_ssh_session(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_issued_at,
            intended.expires_at,
        )
        .await
        .unwrap()
        .is_none());
    assert!(threads
        .live_ssh_session(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_issued_at,
            ssh_issued_at - Duration::seconds(1),
        )
        .await
        .unwrap()
        .is_none());
    set_agent_status(&app, &tenant, agent_id, PrincipalStatus::Suspended).await;
    assert!(threads
        .live_ssh_admission(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_issued_at,
        )
        .await
        .unwrap()
        .is_none());
    assert!(threads
        .live_ssh_session(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_issued_at,
            ssh_intent.expires_at + Duration::hours(1),
        )
        .await
        .unwrap()
        .is_none());
    set_agent_status(&app, &tenant, agent_id, PrincipalStatus::Active).await;

    let run_id = Uuid::from_u128(201);
    let run_started_at = intended.created_at + Duration::hours(1);
    seed_ready_run(
        &app,
        &tenant,
        owner,
        agent_id,
        run_id,
        run_started_at,
        run_started_at + Duration::minutes(5),
    )
    .await;
    let bound = threads
        .bind_run(&tenant, owner, intended.thread_id, run_id, run_started_at)
        .await
        .unwrap();
    let BindAgentThreadRunOutcome::Bound(binding) = bound else {
        panic!("the exact live run and thread should bind: {bound:?}");
    };
    assert_eq!(binding.thread_id, intended.thread_id.to_string());
    assert_eq!(binding.conversation_id, intended.conversation_id);
    assert_eq!(binding.workspace_id, intended.workspace_id.to_string());
    assert_eq!(binding.workspace_generation, 1);
    assert_eq!(
        binding.workspace_expires_at,
        intended
            .expires_at
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    );
    assert_eq!(
        threads
            .bind_run(&tenant, owner, intended.thread_id, run_id, run_started_at)
            .await
            .unwrap(),
        BindAgentThreadRunOutcome::Replayed(binding.clone())
    );
    assert_eq!(
        threads
            .live_binding_for_run(&tenant, run_id, agent_id, "thread-run-jti", run_started_at,)
            .await
            .unwrap(),
        Some(binding)
    );
    assert!(threads
        .live_binding_for_run(
            &tenant,
            run_id,
            agent_id,
            "another-credential",
            run_started_at,
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        threads
            .bind_run(&tenant, "p:bob", intended.thread_id, run_id, run_started_at)
            .await
            .unwrap(),
        BindAgentThreadRunOutcome::NotFound
    );
    assert!(threads
        .live_binding_for_run(
            &tenant,
            run_id,
            agent_id,
            "thread-run-jti",
            intended.expires_at,
        )
        .await
        .unwrap()
        .is_none());

    assert_eq!(
        threads
            .get_for_owner(&tenant, owner, intended.thread_id)
            .await
            .unwrap(),
        Some(ready)
    );
    let listed = threads
        .list_for_owner(&tenant, owner, None, 100)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].thread_id, intended.thread_id.to_string());
    assert!(threads
        .get_for_owner(&tenant, "p:bob", intended.thread_id)
        .await
        .unwrap()
        .is_none());

    assert!(threads
        .start_due_expirations(&tenant, intended.expires_at - Duration::seconds(1), 10)
        .await
        .unwrap()
        .is_empty());
    let expirations = threads
        .start_due_expirations(&tenant, intended.expires_at, 10)
        .await
        .unwrap();
    assert_eq!(expirations.len(), 1);
    let expiration = &expirations[0];
    assert_eq!(expiration.thread_id, intended.thread_id);
    assert_eq!(expiration.workspace_id, intended.workspace_id);
    assert_eq!(
        expiration.storage_locator.as_deref(),
        Some("workspace://local/101")
    );
    assert!(threads
        .live_ssh_session(
            &tenant,
            ssh_grant_id,
            &route_username,
            &ssh_intent.public_key_fingerprint,
            ssh_issued_at,
            ssh_intent.expires_at + Duration::hours(1),
        )
        .await
        .unwrap()
        .is_none());
    assert!(threads
        .live_binding_for_run(&tenant, run_id, agent_id, "thread-run-jti", run_started_at,)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM external_agent_run WHERE tenant_id = $1 AND run_id = $2",
        )
        .bind(&tenant)
        .bind(run_id)
        .fetch_one(admin.db_pool())
        .await
        .unwrap(),
        "terminal"
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
               SELECT 1 FROM run_token_teardown WHERE tenant_id = $1 AND jti = $2
             )",
    )
    .bind(&tenant)
    .bind("thread-run-jti")
    .fetch_one(admin.db_pool())
    .await
    .unwrap());

    let grace = Duration::seconds(AGENT_THREAD_EXPIRY_GRACE_SECONDS);
    assert!(threads
        .expirations_ready_for_cleanup(
            &tenant,
            intended.expires_at + grace - Duration::seconds(1),
            10
        )
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        threads
            .expirations_ready_for_cleanup(&tenant, intended.expires_at + grace, 10)
            .await
            .unwrap(),
        expirations
    );
    assert!(threads
        .record_expiration_failure(
            &tenant,
            expiration,
            AgentThreadExpiryFailure::WorkspaceCleanupFailed,
            intended.expires_at + grace,
        )
        .await
        .unwrap());
    assert!(threads
        .expirations_ready_for_cleanup(
            &tenant,
            intended.expires_at + grace + grace - Duration::seconds(1),
            10,
        )
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        threads
            .expirations_ready_for_cleanup(&tenant, intended.expires_at + grace + grace, 10)
            .await
            .unwrap(),
        expirations
    );
    assert_eq!(
        threads
            .complete_expiration(&tenant, expiration, intended.expires_at + grace + grace)
            .await
            .unwrap(),
        AgentThreadExpiryCompletion::Deleted
    );
    assert_eq!(
        threads
            .complete_expiration(&tenant, expiration, intended.expires_at + grace + grace)
            .await
            .unwrap(),
        AgentThreadExpiryCompletion::AlreadyDeleted
    );
    let receipt = threads
        .get_for_owner(&tenant, owner, intended.thread_id)
        .await
        .unwrap()
        .expect("the owner retains a lifecycle receipt after cleanup");
    assert_eq!(receipt.state, myelin_storage::AgentThreadState::Deleted);
    assert!(receipt.storage_locator.is_none());
    assert!(threads
        .list_for_owner(&tenant, owner, None, 100)
        .await
        .unwrap()
        .is_empty());
    let ListWorkspaceSessionsOutcome::Page(retained_access_history) = threads
        .list_workspace_sessions_for_owner(&tenant, owner, intended.thread_id, None, 10)
        .await
        .unwrap()
    else {
        panic!("workspace history remains owner-visible beside the deleted receipt");
    };
    assert_eq!(retained_access_history.len(), 2);

    sqlx::query("DELETE FROM agent_thread_ssh_grant WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_thread_workspace_session WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id = ANY($1)")
        .bind(vec![first_session_id, second_session_id])
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_thread_run WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM external_agent_run WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_thread WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM identity_agent WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM principal WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
}
