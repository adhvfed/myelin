#![cfg(feature = "integration")]

use std::sync::Arc;

use chrono::{Duration, SecondsFormat, Timelike, Utc};
use myelin_config::MyelinConfig;
use myelin_events::Timestamp;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    agent_run_ref, machine_scheme, AgentLifecycleAction, AgentLifecycleRequest, AgentSessionIssuer,
    AgentSessionRequest, Authority, CellTokenAuthority, CredentialPurpose, NewAgent,
    PgAgentRegistry, RunTokenState, StoreBackedCheck, EXTERNAL_MCP_RUNTIME,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{all_durable_migrations, KmsEngine, SubstrateProvider, TenantScope};
use myelin_tenancy::{Region, TenantId};

fn admin_config(config: &MyelinConfig) -> MyelinConfig {
    let mut admin = config.clone();
    admin.database_url = admin
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    admin
}

fn unique(label: &str) -> String {
    format!(
        "agent-session-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    )
}

async fn providers() -> Option<(SubstrateProvider, SubstrateProvider)> {
    let config = MyelinConfig::dev();
    let admin = SubstrateProvider::connect(admin_config(&config), 8)
        .await
        .ok()?;
    admin
        .migrate_foundation()
        .await
        .expect("apply event foundation migrations");
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the durable run-session aggregate");
    let app = SubstrateProvider::connect(config, 16)
        .await
        .expect("connect constrained app role");
    Some((admin, app))
}

fn human(tenant: &str, region: &str) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        PrincipalId("human:agent-owner".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn activation(nonce: &str) -> NewAgent {
    let grants = vec![
        "agent.tools.read".into(),
        "edge.identity.read".into(),
        "repo.push".into(),
        "run.view".into(),
    ];
    NewAgent {
        name: "Release companion".into(),
        runtime_ref: EXTERNAL_MCP_RUNTIME.into(),
        tools: vec!["ci.read_run.v1".into(), "git.open_pr.v1".into()],
        grants: grants.clone(),
        tenant_policy_if_missing: grants.clone(),
        trigger_actor_policy_if_missing: grants,
        client_nonce: nonce.into(),
    }
}

fn run_request(agent_id: &str, now: chrono::DateTime<Utc>) -> AgentSessionRequest {
    AgentSessionRequest {
        agent_id: agent_id.into(),
        client_nonce: "start-release-companion-v1".into(),
        trigger_credential_jti: "human-session-before-the-run".into(),
        trigger_expires_at_unix: (now + Duration::minutes(10)).timestamp(),
        trigger_authority: Authority::of([
            "agent.run",
            "agent.tools.read",
            "edge.identity.read",
            "repo.push",
            "run.view",
        ]),
        now,
    }
}

async fn cleanup(admin: &SubstrateProvider, tenant: &str) {
    for sql in [
        "DELETE FROM agent_lifecycle_command WHERE tenant_id = $1",
        "DELETE FROM external_agent_run WHERE tenant_id = $1",
        "DELETE FROM delegation_run_snapshot WHERE tenant_id = $1",
        "DELETE FROM delegation_policy_head WHERE tenant_id = $1",
        "DELETE FROM delegation_policy_version WHERE tenant_id = $1",
        "DELETE FROM run_token_teardown WHERE tenant_id = $1",
        "DELETE FROM revocation WHERE tenant_id = $1",
        "DELETE FROM rebac_tuple WHERE tenant_id = $1",
        "DELETE FROM identity_agent WHERE tenant_id = $1",
        "DELETE FROM principal WHERE tenant_id = $1",
        "DELETE FROM outbox_quarantine WHERE event_id IN \
           (SELECT event_id FROM outbox WHERE envelope->>'tenant' = $1)",
        "DELETE FROM outbox WHERE envelope->>'tenant' = $1",
    ] {
        let _ = sqlx::query(sql).bind(tenant).execute(admin.db_pool()).await;
    }
}

fn lifecycle_request(
    agent_id: &str,
    action: AgentLifecycleAction,
    nonce: &str,
) -> AgentLifecycleRequest {
    AgentLifecycleRequest {
        agent_id: agent_id.into(),
        action,
        client_nonce: nonce.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_click_starts_one_short_lived_agent_even_when_the_network_retries() {
    let Some((admin, app)) = providers().await else {
        eprintln!("SKIP: dev PostgreSQL unavailable");
        return;
    };
    let tenant = unique("one-run");
    let region = app.config().region.clone();
    let actor = human(&tenant, &region);
    let agent = PgAgentRegistry::new(app.clone())
        .create(&actor, activation("activate-release-companion-v1"))
        .await
        .expect("the human activates a durable external agent")
        .agent;

    let kms = Arc::new(KmsEngine::new());
    let cell = Arc::new(CellTokenAuthority::generate());
    let identity = StoreBackedCheck::with_pg(
        app.clone(),
        kms.clone(),
        cell.clone(),
        tokio::runtime::Handle::current(),
    );
    let issuer = AgentSessionIssuer::new(app.clone(), identity.clone(), 60)
        .expect("the fail-static run lifetime is valid");
    let now = Utc::now()
        .with_nanosecond(0)
        .expect("zero nanoseconds is a valid wall-clock instant");
    let request = run_request(&agent.id, now);

    let (left, right) = tokio::join!(
        issuer.start(&actor, request.clone()),
        issuer.start(&actor, request.clone())
    );
    let left = left.expect("the first delivery starts the run");
    let right = right.expect("the retry recovers the same run");

    assert_eq!(left.session, right.session, "both replies name one run");
    assert_eq!(
        left.run_token, right.run_token,
        "a retry cannot create a second bearer identity"
    );
    assert_ne!(
        left.created, right.created,
        "exactly one delivery owns run creation"
    );
    assert_eq!(
        agent_run_ref(&tenant, &left.session.run_id).0,
        format!("myelin://{tenant}/agent/run/{}", left.session.run_id)
    );
    assert_eq!(left.session.selected_tools, agent.tools);
    assert_eq!(left.session.effective_grants, agent.grants);

    let claims = identity
        .introspect_run_token_at(
            machine_scheme::AGENT,
            &left.run_token,
            &Timestamp(now.to_rfc3339_opts(SecondsFormat::Secs, true)),
        )
        .expect("the issued material is a verifiable agent-run credential");
    assert_eq!(claims.subject_key, agent.principal_id);
    assert_eq!(claims.authority, Authority::of(agent.grants.clone()));
    match claims.purpose {
        CredentialPurpose::AgentRun {
            run_id,
            delegation_snapshot: Some(snapshot),
        } => {
            assert_eq!(run_id, left.session.run_id);
            assert!(
                snapshot > 0,
                "the token is pinned to durable policy history"
            );
        }
        other => panic!("expected a snapshot-bound agent run, got {other:?}"),
    }

    let restarted_provider = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("reconnect the runtime provider");
    let restarted_identity = StoreBackedCheck::with_pg(
        restarted_provider.clone(),
        kms,
        cell,
        tokio::runtime::Handle::current(),
    );
    let after_restart = AgentSessionIssuer::new(restarted_provider, restarted_identity, 60)
        .expect("reconstruct the issuer")
        .start(
            &actor,
            AgentSessionRequest {
                now: now + Duration::seconds(10),
                ..request
            },
        )
        .await
        .expect("a process-shaped restart still recovers the original run");
    assert!(!after_restart.created);
    assert_eq!(after_restart.session, left.session);
    assert_eq!(after_restart.run_token, left.run_token);

    let durable_runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM external_agent_run \
          WHERE tenant_id = $1 AND region = $2 AND state = 'ready'",
    )
    .bind(&tenant)
    .bind(&region)
    .fetch_one(admin.db_pool())
    .await
    .expect("count the durable ready run");
    let policy_snapshots: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM delegation_run_snapshot \
          WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(&left.session.run_id)
    .fetch_one(admin.db_pool())
    .await
    .expect("count the run's immutable policy snapshot");
    assert_eq!((durable_runs, policy_snapshots), (1, 1));

    let agent_actor = Principal::new(
        actor.tenant.clone(),
        actor.region.clone(),
        PrincipalId(agent.principal_id.clone()),
        PrincipalKind::Agent {
            runtime_ref: myelin_identity::RuntimeRef(agent.runtime_ref.clone()),
            on_behalf_of: Some(actor.principal_id.clone()),
        },
        DataRole::Controller,
        agent.status,
    );
    let authorized = issuer
        .authorize(
            &agent_actor,
            &left.session.run_id,
            &left.run_token.jti,
            now + Duration::seconds(15),
        )
        .await
        .expect("a frame resolves only its exact ready durable run");
    assert_eq!(authorized.run_id, left.session.run_id);
    assert_eq!(authorized.agent_id, agent.id);
    assert_eq!(authorized.token_jti, left.run_token.jti);
    assert!(matches!(
        issuer
            .authorize(
                &agent_actor,
                &left.session.run_id,
                "a-different-token",
                now + Duration::seconds(15),
            )
            .await,
        Err(myelin_identity_service::AgentSessionError::RunNotFound)
    ));

    let closed = issuer
        .close(&agent_actor, &left.session.run_id, &left.run_token.jti)
        .await
        .expect("the running agent releases its short-lived identity");
    assert_eq!(closed.run_id, left.session.run_id);
    assert_eq!(closed.agent_id, agent.id);
    assert_eq!(closed.state.token(), "closed");

    let agent_scope = TenantScope::from_verified_token(&agent_actor, agent_actor.region.clone());
    assert_eq!(
        identity.run_token_minter().revocation_state(
            &agent_scope,
            &left.run_token,
            &Timestamp((now + Duration::seconds(20)).to_rfc3339_opts(SecondsFormat::Secs, true)),
        ),
        RunTokenState::TornDown,
        "closing the durable run and revoking its bearer identity are one fact"
    );
    let closed_runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM external_agent_run \
          WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND state = 'closed'",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(&left.session.run_id)
    .fetch_one(admin.db_pool())
    .await
    .expect("observe the durable closed run");
    assert_eq!(closed_runs, 1);
    assert!(matches!(
        issuer
            .authorize(
                &agent_actor,
                &left.session.run_id,
                &left.run_token.jti,
                now + Duration::seconds(20),
            )
            .await,
        Err(myelin_identity_service::AgentSessionError::RunNotFound)
    ));

    let replay_after_close = issuer
        .start(
            &actor,
            AgentSessionRequest {
                now: now + Duration::seconds(20),
                ..run_request(&agent.id, now)
            },
        )
        .await;
    assert!(matches!(
        replay_after_close,
        Err(myelin_identity_service::AgentSessionError::Conflict(_))
    ));

    let mut terminal_request = run_request(&agent.id, now + Duration::seconds(21));
    terminal_request.client_nonce = "start-release-companion-terminal-v1".into();
    let terminal_run = issuer
        .start(&actor, terminal_request)
        .await
        .expect("a later independent run starts normally");
    let terminal = issuer
        .terminate(
            &agent_actor,
            &terminal_run.session.run_id,
            &terminal_run.run_token.jti,
        )
        .await
        .expect("an indeterminate outcome terminates the exact run");
    assert_eq!(terminal.state.token(), "terminal");
    assert_eq!(
        identity.run_token_minter().revocation_state(
            &agent_scope,
            &terminal_run.run_token,
            &Timestamp((now + Duration::seconds(22)).to_rfc3339_opts(SecondsFormat::Secs, true)),
        ),
        RunTokenState::TornDown,
        "terminal state and token teardown are committed together"
    );

    cleanup(&admin, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_human_can_pause_and_retire_an_agent_without_leaving_a_live_run_behind() {
    let Some((admin, app)) = providers().await else {
        eprintln!("SKIP: dev PostgreSQL unavailable");
        return;
    };
    let tenant = unique("lifecycle");
    let region = app.config().region.clone();
    let actor = human(&tenant, &region);
    let registry = PgAgentRegistry::new(app.clone());
    let agent = registry
        .create(&actor, activation("activate-lifecycle-companion-v1"))
        .await
        .expect("the human activates a collaborator")
        .agent;
    let identity = StoreBackedCheck::with_pg(
        app.clone(),
        Arc::new(KmsEngine::new()),
        Arc::new(CellTokenAuthority::generate()),
        tokio::runtime::Handle::current(),
    );
    let issuer = AgentSessionIssuer::new(app, identity.clone(), 60)
        .expect("the fail-static run lifetime is valid");
    let now = Utc::now()
        .with_nanosecond(0)
        .expect("zero nanoseconds is a valid wall-clock instant");
    let first_run = issuer
        .start(&actor, run_request(&agent.id, now))
        .await
        .expect("the active agent begins useful work");
    let active_agent = Principal::new(
        actor.tenant.clone(),
        actor.region.clone(),
        PrincipalId(agent.principal_id.clone()),
        PrincipalKind::Agent {
            runtime_ref: myelin_identity::RuntimeRef(agent.runtime_ref.clone()),
            on_behalf_of: Some(actor.principal_id.clone()),
        },
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope = TenantScope::from_verified_token(&active_agent, active_agent.region.clone());

    let suspension = registry
        .change_status(
            &actor,
            lifecycle_request(
                &agent.id,
                AgentLifecycleAction::Suspend,
                "pause-lifecycle-companion-v1",
            ),
        )
        .await
        .expect("one administrative action pauses the identity and its work");
    assert_eq!(suspension.agent.status, PrincipalStatus::Suspended);
    assert!(suspension.changed);
    assert_eq!(
        suspension.terminated_runs, 1,
        "the response tells the human that in-flight work was stopped"
    );
    assert_eq!(
        identity.run_token_minter().revocation_state(
            &scope,
            &first_run.run_token,
            &Timestamp((now + Duration::seconds(1)).to_rfc3339_opts(SecondsFormat::Secs, true)),
        ),
        RunTokenState::TornDown,
        "suspension and bearer teardown are one durable fact"
    );
    assert!(matches!(
        issuer
            .authorize(
                &active_agent,
                &first_run.session.run_id,
                &first_run.run_token.jti,
                now + Duration::seconds(1),
            )
            .await,
        Err(myelin_identity_service::AgentSessionError::RunNotFound)
    ));

    let replayed_suspension = registry
        .change_status(
            &actor,
            lifecycle_request(
                &agent.id,
                AgentLifecycleAction::Suspend,
                "pause-lifecycle-companion-v1",
            ),
        )
        .await
        .expect("a lost suspension response is safe to retry");
    assert_eq!(replayed_suspension, suspension);
    let mut refused_request = run_request(&agent.id, now + Duration::seconds(2));
    refused_request.client_nonce = "suspended-agent-cannot-start".into();
    assert!(matches!(
        issuer.start(&actor, refused_request).await,
        Err(myelin_identity_service::AgentSessionError::NotFound)
    ));

    let resumed = registry
        .change_status(
            &actor,
            lifecycle_request(
                &agent.id,
                AgentLifecycleAction::Resume,
                "resume-lifecycle-companion-v1",
            ),
        )
        .await
        .expect("a deliberately paused collaborator can return");
    assert_eq!(resumed.agent.status, PrincipalStatus::Active);
    assert!(resumed.changed);
    assert_eq!(resumed.terminated_runs, 0);
    let mut second_request = run_request(&agent.id, now + Duration::seconds(3));
    second_request.client_nonce = "start-after-deliberate-resume".into();
    let second_run = issuer
        .start(&actor, second_request)
        .await
        .expect("resume permits new work without reviving the old bearer");
    assert_ne!(second_run.session.run_id, first_run.session.run_id);
    assert_eq!(
        identity.run_token_minter().revocation_state(
            &scope,
            &first_run.run_token,
            &Timestamp((now + Duration::seconds(4)).to_rfc3339_opts(SecondsFormat::Secs, true)),
        ),
        RunTokenState::TornDown,
        "resume never resurrects an earlier run"
    );

    let retirement = registry
        .change_status(
            &actor,
            lifecycle_request(
                &agent.id,
                AgentLifecycleAction::Retire,
                "retire-lifecycle-companion-v1",
            ),
        )
        .await
        .expect("retirement permanently disables the collaborator");
    assert_eq!(retirement.agent.status, PrincipalStatus::Disabled);
    assert!(retirement.changed);
    assert_eq!(retirement.terminated_runs, 1);
    assert_eq!(
        identity.run_token_minter().revocation_state(
            &scope,
            &second_run.run_token,
            &Timestamp((now + Duration::seconds(4)).to_rfc3339_opts(SecondsFormat::Secs, true)),
        ),
        RunTokenState::TornDown
    );
    assert!(matches!(
        registry
            .change_status(
                &actor,
                lifecycle_request(
                    &agent.id,
                    AgentLifecycleAction::Resume,
                    "retired-agent-cannot-resume",
                ),
            )
            .await,
        Err(myelin_identity_service::AgentRegistryError::Conflict(_))
    ));

    let terminal_runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM external_agent_run \
          WHERE tenant_id = $1 AND region = $2 AND agent_id = $3::uuid AND state = 'terminal'",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(&agent.id)
    .fetch_one(admin.db_pool())
    .await
    .expect("observe terminal run history after lifecycle changes");
    let lifecycle_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE aggregate = $1 \
           AND envelope->>'type_' = 'identity.agent.status_changed'",
    )
    .bind(format!("identity:agent:{}", agent.id))
    .fetch_one(admin.db_pool())
    .await
    .expect("count durable lifecycle events");
    assert_eq!(terminal_runs, 2);
    assert_eq!(
        lifecycle_events, 3,
        "suspend, resume, and retire each emit once; retries do not"
    );
    cleanup(&admin, &tenant).await;
}
