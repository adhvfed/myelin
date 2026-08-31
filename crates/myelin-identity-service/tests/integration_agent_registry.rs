#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RunId, RuntimeRef,
};
use myelin_identity_service::{
    AgentLifecycleAction, AgentLifecycleRequest, AgentRegistryError, AgentWorkspaceGuidance,
    AgentWorkspaceGuidanceUpdate, Authority, DelegationPolicySource, NewAgent, PgAgentRegistry,
    EXTERNAL_MCP_RUNTIME,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    all_durable_migrations, ColumnCryptor, DurableDelegationPolicyBacking, KekId, KmsEngine,
    SubjectId, SubjectKeyScope, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use uuid::Uuid;

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

fn unique(label: &str) -> String {
    format!(
        "agent-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    )
}

async fn providers() -> (SubstrateProvider, SubstrateProvider) {
    let config = test_config();
    let admin = SubstrateProvider::connect(admin_config(&config), 8)
        .await
        .expect("connect to the Postgres required by the agent registry stories");
    admin
        .migrate_foundation()
        .await
        .expect("apply event foundation migrations");
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the durable identity/delegation aggregate");
    let app = SubstrateProvider::connect(config, 16)
        .await
        .expect("connect constrained app role");
    (admin, app)
}

fn human(tenant: &str, region: &str) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        PrincipalId("human:activation-owner".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn proposal(nonce: &str) -> NewAgent {
    NewAgent {
        agent_id: Uuid::new_v4(),
        name: "Review companion".into(),
        runtime_ref: EXTERNAL_MCP_RUNTIME.into(),
        tools: vec!["git.open_pr".into(), "ci.read_run".into()],
        grants: vec![
            "agent.tools.read".into(),
            "edge.identity.read".into(),
            "repo.push".into(),
            "run.view".into(),
        ],
        workspace_guidance: None,
        tenant_policy_if_missing: vec![
            "agent.tools.read".into(),
            "edge.identity.read".into(),
            "pull_request.merge".into(),
            "repo.push".into(),
            "run.view".into(),
        ],
        trigger_actor_policy_if_missing: vec![
            "agent.tools.read".into(),
            "edge.identity.read".into(),
            "repo.push".into(),
            "run.view".into(),
        ],
        client_nonce: nonce.into(),
    }
}

async fn cleanup(admin: &SubstrateProvider, tenant: &str) {
    for sql in [
        "DELETE FROM delegation_run_snapshot WHERE tenant_id = $1",
        "DELETE FROM delegation_policy_head WHERE tenant_id = $1",
        "DELETE FROM delegation_policy_version WHERE tenant_id = $1",
        "DELETE FROM identity_agent WHERE tenant_id = $1",
        "DELETE FROM principal WHERE tenant_id = $1",
        "DELETE FROM outbox_quarantine WHERE event_id IN \
           (SELECT event_id FROM outbox WHERE envelope->>'tenant' = $1)",
        "DELETE FROM outbox WHERE envelope->>'tenant' = $1",
    ] {
        let _ = sqlx::query(sql).bind(tenant).execute(admin.db_pool()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_human_activation_is_retry_safe_durable_and_ready_for_a_governed_run() {
    let (admin, app) = providers().await;
    let tenant = unique("converges");
    let region = app.config().region.clone();
    let actor = human(&tenant, &region);
    let registry = PgAgentRegistry::new(app.clone());
    let intent = proposal("activate-review-companion-v1");

    let (left, right) = tokio::join!(
        registry.create(&actor, intent.clone()),
        registry.create(&actor, intent.clone())
    );
    let left = left.expect("first concurrent activation converges");
    let right = right.expect("second concurrent activation converges");
    assert_eq!(
        left.agent, right.agent,
        "both retries name one durable agent"
    );
    assert_ne!(
        left.created, right.created,
        "exactly one request owns creation; the other is an idempotent replay"
    );

    let agent = left.agent;
    assert_eq!(agent.runtime_ref, EXTERNAL_MCP_RUNTIME);
    assert_eq!(agent.created_by, actor.principal_id.0);
    assert_eq!(agent.status, PrincipalStatus::Active);
    assert_eq!(
        registry
            .list(&actor, None, 10)
            .await
            .expect("the human can see their agent roster"),
        vec![agent.clone()]
    );
    assert_eq!(
        registry
            .get(&actor, &agent.id)
            .await
            .expect("the agent is addressable after activation"),
        agent
    );

    let governed_agent = Principal::new(
        actor.tenant.clone(),
        actor.region.clone(),
        PrincipalId(agent.principal_id.clone()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef(agent.runtime_ref.clone()),
            on_behalf_of: Some(actor.principal_id.clone()),
        },
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope = TenantScope::from_verified_token(&actor, actor.region.clone());
    let resolved =
        DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(app.clone()))
            .resolve_for_run(
                &scope,
                &governed_agent,
                &actor,
                &RunId("run:activation-proof".into()),
            )
            .await
            .expect("all four durable policy conjuncts are ready immediately");
    for grant in &agent.grants {
        assert!(
            resolved.input().agent_policy.holds(grant)
                && resolved.input().delegation.holds(grant)
                && resolved.input().tenant_policy.holds(grant)
                && resolved.input().trigger_actor_held.holds(grant),
            "the activated run carries `{grant}` through every conjunct"
        );
    }

    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE aggregate = $1 \
           AND envelope->>'type_' = 'identity.agent.created'",
    )
    .bind(format!("identity:agent:{}", agent.id))
    .fetch_one(admin.db_pool())
    .await
    .expect("count activation events");
    assert_eq!(event_count, 1, "retries emit one durable activation event");

    let restarted = PgAgentRegistry::new(
        SubstrateProvider::connect(test_config(), 4)
            .await
            .expect("fresh app pool"),
    );
    assert_eq!(
        restarted
            .get(&actor, &agent.id)
            .await
            .expect("the roster survives a process-shaped restart"),
        agent
    );
    cleanup(&admin, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_workspace_defaults_are_encrypted_revisioned_and_retry_safe() {
    let (admin, app) = providers().await;
    let tenant = unique("workspace-profile");
    let region = app.config().region.clone();
    let actor = human(&tenant, &region);
    let registry = PgAgentRegistry::new(app.clone());
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(actor.tenant.clone(), actor.region.clone()))
        .unwrap();
    let seal = |content: &[u8]| AgentWorkspaceGuidance {
        content_digest: format!("blake3:{}", blake3::hash(content).to_hex()),
        encrypted_content: ColumnCryptor::new(&kms, actor.region.clone())
            .encrypt_for_subject_scope(
                &actor.tenant,
                &SubjectId::new(&actor.principal_id.0),
                SubjectKeyScope::AgentData,
                content,
            )
            .unwrap(),
    };
    let original = b"Explain the smallest safe change first.";
    let mut intent = proposal("activate-configurable-agent-v1");
    intent.workspace_guidance = Some(seal(original));
    let created = registry.create(&actor, intent).await.unwrap().agent;
    assert_eq!(created.workspace_guidance_revision, 1);
    assert_eq!(
        created.workspace_guidance.as_ref().unwrap().content_digest,
        format!("blake3:{}", blake3::hash(original).to_hex())
    );

    let ciphertext: Vec<u8> = sqlx::query_scalar(
        "SELECT workspace_guidance_ciphertext FROM identity_agent \
          WHERE tenant_id = $1 AND region = $2 AND agent_id = $3",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(Uuid::parse_str(&created.id).unwrap())
    .fetch_one(admin.db_pool())
    .await
    .unwrap();
    assert!(!ciphertext
        .windows(original.len())
        .any(|window| window == original));

    let revised = b"Start with the failing full-system journey.";
    let update = AgentWorkspaceGuidanceUpdate {
        agent_id: created.id.clone(),
        expected_revision: 1,
        guidance: Some(seal(revised)),
    };
    let changed = registry
        .update_workspace_guidance(&actor, update.clone())
        .await
        .unwrap();
    assert!(changed.changed);
    assert_eq!(changed.agent.workspace_guidance_revision, 2);
    let replay = registry
        .update_workspace_guidance(&actor, update)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.agent, changed.agent);

    let suspend = AgentLifecycleRequest {
        agent_id: created.id.clone(),
        action: AgentLifecycleAction::Suspend,
        client_nonce: "suspend-configurable-agent-v1".into(),
    };
    let suspended = registry
        .change_status(&actor, suspend.clone())
        .await
        .unwrap();
    assert!(suspended.changed);
    assert_eq!(suspended.agent.status, PrincipalStatus::Suspended);
    assert_eq!(
        suspended.agent.workspace_guidance, changed.agent.workspace_guidance,
        "lifecycle changes preserve the encrypted workspace-default receipt"
    );
    let suspend_replay = registry.change_status(&actor, suspend).await.unwrap();
    assert_eq!(suspend_replay, suspended);
    registry
        .change_status(
            &actor,
            AgentLifecycleRequest {
                agent_id: created.id.clone(),
                action: AgentLifecycleAction::Resume,
                client_nonce: "resume-configurable-agent-v1".into(),
            },
        )
        .await
        .unwrap();

    let stale = registry
        .update_workspace_guidance(
            &actor,
            AgentWorkspaceGuidanceUpdate {
                agent_id: created.id.clone(),
                expected_revision: 1,
                guidance: None,
            },
        )
        .await;
    assert!(matches!(stale, Err(AgentRegistryError::Conflict(_))));

    let clear = AgentWorkspaceGuidanceUpdate {
        agent_id: created.id.clone(),
        expected_revision: 2,
        guidance: None,
    };
    let cleared = registry
        .update_workspace_guidance(&actor, clear.clone())
        .await
        .unwrap();
    assert!(cleared.changed);
    assert_eq!(cleared.agent.workspace_guidance_revision, 3);
    assert!(cleared.agent.workspace_guidance.is_none());
    let clear_replay = registry
        .update_workspace_guidance(&actor, clear)
        .await
        .unwrap();
    assert!(!clear_replay.changed);
    assert_eq!(clear_replay.agent, cleared.agent);

    let events: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT envelope FROM outbox \
          WHERE aggregate = $1 \
            AND envelope->>'type_' = 'identity.agent.workspace_guidance_updated' \
          ORDER BY seq",
    )
    .bind(format!("identity:agent:{}", created.id))
    .fetch_all(admin.db_pool())
    .await
    .unwrap();
    assert_eq!(
        events.len(),
        3,
        "activation, replacement, and clear emit once; retries and stale writes emit nothing"
    );
    assert_eq!(events[0]["payload"]["agent_id"], created.id);
    assert_eq!(events[0]["payload"]["revision"], 1);
    assert_eq!(events[0]["payload"]["configured"], true);
    assert_eq!(
        events[0]["payload"]["content_digest"],
        format!("blake3:{}", blake3::hash(original).to_hex())
    );
    assert_eq!(events[1]["payload"]["revision"], 2);
    assert_eq!(events[1]["payload"]["configured"], true);
    assert_eq!(
        events[1]["payload"]["content_digest"],
        format!("blake3:{}", blake3::hash(revised).to_hex())
    );
    assert_eq!(events[2]["payload"]["revision"], 3);
    assert_eq!(events[2]["payload"]["configured"], false);
    assert!(events[2]["payload"]["content_digest"].is_null());
    for event in &events {
        assert_eq!(event["contains_personal_data"], false);
        assert_eq!(event["visibility"], "Internal");
        assert!(event["pii_key_ref"].is_null());
        let mut payload_keys = event["payload"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        payload_keys.sort_unstable();
        assert_eq!(
            payload_keys,
            ["agent_id", "configured", "content_digest", "revision"]
        );
        let rendered = event.to_string();
        assert!(!rendered.contains(std::str::from_utf8(original).unwrap()));
        assert!(!rendered.contains(std::str::from_utf8(revised).unwrap()));
        for forbidden in [
            "ciphertext",
            "workspace_guidance_key_ref",
            "nonce",
            "instructions",
        ] {
            assert!(!rendered.contains(forbidden));
        }
    }
    cleanup(&admin, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_rosters_keep_the_newest_collaborator_in_reach_across_pages() {
    let (admin, app) = providers().await;
    let tenant = unique("listing");
    let region = app.config().region.clone();
    let actor = human(&tenant, &region);
    let registry = PgAgentRegistry::new(app);

    let mut oldest_proposal = proposal("activate-oldest");
    oldest_proposal.name = "Oldest companion".into();
    let oldest = registry
        .create(&actor, oldest_proposal)
        .await
        .expect("activate the oldest collaborator")
        .agent;

    let mut middle_proposal = proposal("activate-middle");
    middle_proposal.name = "Middle companion".into();
    let middle = registry
        .create(&actor, middle_proposal)
        .await
        .expect("activate the middle collaborator")
        .agent;

    let mut newest_proposal = proposal("activate-newest");
    newest_proposal.name = "Newest companion".into();
    let newest = registry
        .create(&actor, newest_proposal)
        .await
        .expect("activate the newest collaborator")
        .agent;

    sqlx::query(
        "UPDATE identity_agent \
            SET created_at = CASE name \
                WHEN 'Oldest companion' THEN TIMESTAMPTZ '2026-01-01T00:00:00Z' \
                WHEN 'Middle companion' THEN TIMESTAMPTZ '2026-01-02T00:00:00Z' \
                WHEN 'Newest companion' THEN TIMESTAMPTZ '2026-01-03T00:00:00Z' \
                ELSE created_at \
            END \
          WHERE tenant_id = $1 AND region = $2",
    )
    .bind(&tenant)
    .bind(&region)
    .execute(admin.db_pool())
    .await
    .expect("make recency deterministic without coupling it to random agent ids");

    let first_page = registry
        .list(&actor, None, 2)
        .await
        .expect("list the first page of collaborators");
    assert_eq!(
        first_page.iter().map(|agent| &agent.id).collect::<Vec<_>>(),
        vec![&newest.id, &middle.id],
        "newly activated collaborators belong at the front of the roster"
    );

    let second_page = registry
        .list(&actor, Some(&middle.id), 2)
        .await
        .expect("continue from the last collaborator on the first page");
    assert_eq!(
        second_page
            .iter()
            .map(|agent| &agent.id)
            .collect::<Vec<_>>(),
        vec![&oldest.id],
        "the cursor continues by stable activation order without gaps or repeats"
    );
    assert!(
        registry
            .list(&actor, Some("00000000-0000-0000-0000-000000000000"), 2,)
            .await
            .expect("an unknown cursor fails closed")
            .is_empty(),
        "a cursor must name an agent in the caller's roster"
    );

    cleanup(&admin, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_platform_managed_tenant_ceiling_learns_new_catalogue_grants_additively() {
    let (admin, app) = providers().await;
    let tenant = unique("platform-policy-upgrade");
    let region = app.config().region.clone();
    let actor = human(&tenant, &region);
    let registry = PgAgentRegistry::new(app);

    let mut before_catalogue_growth = proposal("before-catalogue-growth");
    before_catalogue_growth
        .trigger_actor_policy_if_missing
        .push("issue.view".into());
    let before = registry
        .create(&actor, before_catalogue_growth)
        .await
        .expect("the old catalogue seeds a platform-managed tenant default");

    let mut after_catalogue_growth = proposal("after-catalogue-growth");
    after_catalogue_growth.name = "Issue companion".into();
    after_catalogue_growth.tools.push("issues.view".into());
    after_catalogue_growth.grants.push("issue.view".into());
    after_catalogue_growth
        .tenant_policy_if_missing
        .push("issue.view".into());
    after_catalogue_growth
        .trigger_actor_policy_if_missing
        .push("issue.view".into());
    let after = registry
        .create(&actor, after_catalogue_growth)
        .await
        .expect("a platform catalogue addition evolves its managed default");

    assert_eq!(
        after.policy_versions.tenant,
        before.policy_versions.tenant + 1
    );
    let (grants, platform_managed): (Vec<String>, bool) = sqlx::query_as(
        "SELECT v.grants, h.platform_managed \
           FROM delegation_policy_head h \
           JOIN delegation_policy_version v \
             ON v.tenant_id = h.tenant_id AND v.region = h.region \
            AND v.policy_kind = h.policy_kind AND v.subject_id = h.subject_id \
            AND v.trigger_actor_id = h.trigger_actor_id AND v.version = h.version \
          WHERE h.tenant_id = $1 AND h.region = $2 AND h.policy_kind = 'tenant'",
    )
    .bind(&tenant)
    .bind(&region)
    .fetch_one(admin.db_pool())
    .await
    .expect("read evolved tenant policy");
    assert!(platform_managed);
    assert!(grants.binary_search(&"issue.view".into()).is_ok());
    assert!(grants.binary_search(&"repo.push".into()).is_ok());

    cleanup(&admin, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tenant_ceiling_refusal_leaves_no_half_activated_agent() {
    let (admin, app) = providers().await;
    let tenant = unique("ceiling");
    let region = app.config().region.clone();
    let actor = human(&tenant, &region);
    let scope = TenantScope::from_verified_token(&actor, actor.region.clone());
    DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(app.clone()))
        .provision_tenant_policy(&scope, None, &Authority::of(["agent.tools.read"]))
        .await
        .expect("the organization has an intentionally narrow policy ceiling");

    let error = PgAgentRegistry::new(app)
        .create(&actor, proposal("refused-by-tenant-ceiling"))
        .await
        .expect_err("an agent cannot silently widen the tenant ceiling");
    assert!(matches!(error, AgentRegistryError::Policy(_)));

    let registered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM identity_agent WHERE tenant_id = $1 AND region = $2",
    )
    .bind(&tenant)
    .bind(&region)
    .fetch_one(admin.db_pool())
    .await
    .expect("count agent registrations");
    let principals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM principal \
          WHERE tenant_id = $1 AND region = $2 AND principal_id LIKE 'agent:%'",
    )
    .bind(&tenant)
    .bind(&region)
    .fetch_one(admin.db_pool())
    .await
    .expect("count agent principals");
    let pair_policies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM delegation_policy_head \
          WHERE tenant_id = $1 AND region = $2 AND policy_kind IN ('agent', 'delegation')",
    )
    .bind(&tenant)
    .bind(&region)
    .fetch_one(admin.db_pool())
    .await
    .expect("count agent policy heads");
    assert_eq!((registered, principals, pair_policies), (0, 0, 0));
    cleanup(&admin, &tenant).await;
}
