#![cfg(feature = "integration")]

use chrono::{Duration, TimeZone, Utc};
use myelin_config::MyelinConfig;
use myelin_identity::{DataRole, PrincipalKind, PrincipalStatus, RuntimeRef};
use myelin_storage::{
    all_durable_migrations, ActivateAgentThreadOutcome, CreateAgentThreadOutcome,
    DurableAgentThreadBacking, HotTables, NewAgentThread, SubstrateProvider,
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
