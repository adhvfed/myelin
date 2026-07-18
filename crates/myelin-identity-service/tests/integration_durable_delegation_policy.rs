//! Live PostgreSQL proof for durable server-side delegation policy resolution.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_identity::{
    DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RunId, RuntimeRef,
};
use myelin_identity_service::{
    Authority, DelegationInput, DelegationPolicyError, DelegationPolicySource,
    DelegationPolicyVersionCursor,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    delegation_policy_durable_migrations, DurableDelegationPolicyBacking, SubstrateProvider,
    TenantScope,
};
use myelin_tenancy::{Region, TenantId};

fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut admin = cfg.clone();
    admin.database_url = admin
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    admin
}

fn unique(label: &str) -> String {
    format!(
        "delegation-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    )
}

async fn providers() -> Option<(SubstrateProvider, SubstrateProvider)> {
    let cfg = MyelinConfig::dev();
    let admin = SubstrateProvider::connect(admin_config(&cfg), 8)
        .await
        .ok()?;
    admin
        .migrate(&delegation_policy_durable_migrations(), &HotTables::none())
        .await
        .expect("apply delegation policy migrations");
    let app = SubstrateProvider::connect(cfg, 16)
        .await
        .expect("connect constrained app role");
    Some((admin, app))
}

fn principals(tenant: &str, region: &str) -> (TenantScope, Principal, Principal) {
    principals_named(tenant, region, "agent:worker", "human:trigger")
}

fn principals_named(
    tenant: &str,
    region: &str,
    agent_id: &str,
    actor_id: &str,
) -> (TenantScope, Principal, Principal) {
    let actor_id = PrincipalId(actor_id.into());
    let actor = Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        actor_id.clone(),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let agent = Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        PrincipalId(agent_id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("runtime:worker".into()),
            on_behalf_of: Some(actor_id),
        },
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope = TenantScope::from_verified_token(&agent, Region(region.into()));
    (scope, agent, actor)
}

fn policy(extra: &[&str]) -> DelegationInput {
    let mut agent = vec!["repo:read", "repo:write"];
    agent.extend_from_slice(extra);
    let mut delegated = vec!["repo:read", "repo:write"];
    delegated.extend_from_slice(extra);
    let mut tenant = vec!["repo:read", "repo:write"];
    tenant.extend_from_slice(extra);
    let mut held = vec!["repo:read", "repo:write"];
    held.extend_from_slice(extra);
    DelegationInput {
        agent_policy: Authority::of(agent),
        delegation: Authority::of(delegated),
        tenant_policy: Authority::of(tenant),
        trigger_actor_held: Authority::of(held),
    }
}

struct PolicyCursors {
    agent: DelegationPolicyVersionCursor,
    delegation: DelegationPolicyVersionCursor,
    tenant: DelegationPolicyVersionCursor,
    trigger_actor: DelegationPolicyVersionCursor,
}

async fn provision_initial(
    source: &DelegationPolicySource,
    scope: &TenantScope,
    agent: &Principal,
    actor: &Principal,
    input: &DelegationInput,
) -> PolicyCursors {
    let tenant = source
        .provision_tenant_policy(scope, None, &input.tenant_policy)
        .await
        .expect("tenant policy");
    let agent_cursor = source
        .provision_agent_policy(scope, agent, None, &input.agent_policy)
        .await
        .expect("agent policy");
    let trigger_actor = source
        .provision_trigger_actor_held(scope, actor, None, &input.trigger_actor_held)
        .await
        .expect("trigger actor held policy");
    let delegation = source
        .provision_delegation(scope, agent, actor, None, &input.delegation)
        .await
        .expect("delegation policy");
    PolicyCursors {
        agent: agent_cursor,
        delegation,
        tenant,
        trigger_actor,
    }
}

async fn cleanup(admin: &SubstrateProvider, tenant: &str) {
    for sql in [
        "DELETE FROM delegation_run_snapshot WHERE tenant_id = $1",
        "DELETE FROM delegation_policy_head WHERE tenant_id = $1",
        "DELETE FROM delegation_policy_version WHERE tenant_id = $1",
    ] {
        let _ = sqlx::query(sql).bind(tenant).execute(admin.db_pool()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_survives_a_fresh_pool_and_same_run_is_idempotent() {
    let Some((admin, app)) = providers().await else {
        eprintln!("SKIP: dev PostgreSQL unavailable");
        return;
    };
    let tenant = unique("restart");
    let region = app.config().region.clone();
    let (scope, agent, actor) = principals(&tenant, &region);
    let source = DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(app));
    provision_initial(&source, &scope, &agent, &actor, &policy(&[])).await;
    let run = RunId("run:restart".into());
    let first = source
        .resolve_for_run(&scope, &agent, &actor, &run)
        .await
        .expect("first snapshot");
    drop(source);

    let fresh_app = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("fresh pool after process-shaped restart");
    let restarted = DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(fresh_app));
    let second = restarted
        .resolve_for_run(&scope, &agent, &actor, &run)
        .await
        .expect("same snapshot after restart");
    assert_eq!(second, first);

    cleanup(&admin, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_cross_tenant_and_cross_region_inputs_fail_closed() {
    let Some((admin, app)) = providers().await else {
        eprintln!("SKIP: dev PostgreSQL unavailable");
        return;
    };
    let tenant_a = unique("isolation-a");
    let tenant_b = unique("isolation-b");
    let region = app.config().region.clone();
    let source = DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(app));
    let (scope_a, agent_a, actor_a) = principals(&tenant_a, &region);
    let (scope_b, agent_b, actor_b) = principals(&tenant_b, &region);

    assert!(matches!(
        source
            .resolve_for_run(&scope_a, &agent_a, &actor_a, &RunId("run:missing".into()))
            .await,
        Err(DelegationPolicyError::MissingPolicy(_))
    ));
    provision_initial(&source, &scope_a, &agent_a, &actor_a, &policy(&[])).await;
    assert!(matches!(
        source
            .resolve_for_run(&scope_b, &agent_b, &actor_b, &RunId("run:tenant-b".into()))
            .await,
        Err(DelegationPolicyError::MissingPolicy(_))
    ));

    let (wrong_scope, wrong_agent, wrong_actor) = principals(&tenant_a, "us-east");
    assert!(matches!(
        source
            .resolve_for_run(
                &wrong_scope,
                &wrong_agent,
                &wrong_actor,
                &RunId("run:wrong-region".into())
            )
            .await,
        Err(DelegationPolicyError::ScopeMismatch)
    ));

    cleanup(&admin, &tenant_a).await;
    cleanup(&admin, &tenant_b).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn updates_never_grow_an_existing_run_and_revocation_denies() {
    let Some((admin, app)) = providers().await else {
        eprintln!("SKIP: dev PostgreSQL unavailable");
        return;
    };
    let tenant = unique("stale");
    let region = app.config().region.clone();
    let (scope, agent, actor) = principals(&tenant, &region);
    let source = DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(app));
    let v1 = provision_initial(&source, &scope, &agent, &actor, &policy(&[])).await;
    let old_run = RunId("run:old".into());
    let old = source
        .resolve_for_run(&scope, &agent, &actor, &old_run)
        .await
        .expect("v1 snapshot");
    assert!(!old.input.agent_policy.holds("repo:admin"));

    let widened = policy(&["repo:admin"]);
    let _agent_v2 = source
        .provision_agent_policy(&scope, &agent, Some(&v1.agent), &widened.agent_policy)
        .await
        .expect("agent v2");
    let _tenant_v2 = source
        .provision_tenant_policy(&scope, Some(&v1.tenant), &widened.tenant_policy)
        .await
        .expect("tenant v2");
    let _actor_v2 = source
        .provision_trigger_actor_held(
            &scope,
            &actor,
            Some(&v1.trigger_actor),
            &widened.trigger_actor_held,
        )
        .await
        .expect("actor v2");
    let delegation_v2 = source
        .provision_delegation(
            &scope,
            &agent,
            &actor,
            Some(&v1.delegation),
            &widened.delegation,
        )
        .await
        .expect("delegation v2");
    assert!(matches!(
        source
            .resolve_for_run(&scope, &agent, &actor, &old_run)
            .await,
        Err(DelegationPolicyError::StaleSnapshot)
    ));
    let new = source
        .resolve_for_run(&scope, &agent, &actor, &RunId("run:new".into()))
        .await
        .expect("new run sees v2");
    assert!(new.input.agent_policy.holds("repo:admin"));
    assert!(new.effective_policy.caveats.contains(&"repo:admin".into()));
    assert_ne!(new.cursor.snapshot, old.cursor.snapshot);

    source
        .revoke_delegation(&scope, &agent, &actor, &delegation_v2)
        .await
        .expect("append revocation tombstone");
    assert!(matches!(
        source
            .resolve_for_run(&scope, &agent, &actor, &RunId("run:after-revoke".into()))
            .await,
        Err(DelegationPolicyError::RevokedPolicy("delegation"))
    ));

    cleanup(&admin, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_tenant_policy_supports_two_agents_and_two_actors() {
    let Some((admin, app)) = providers().await else {
        eprintln!("SKIP: dev PostgreSQL unavailable");
        return;
    };
    let tenant = unique("multi-pair");
    let region = app.config().region.clone();
    let source = DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(app));
    let (scope_a, agent_a, actor_a) = principals_named(&tenant, &region, "agent:a", "human:a");
    let (scope_b, agent_b, actor_b) = principals_named(&tenant, &region, "agent:b", "human:b");

    source
        .provision_tenant_policy(
            &scope_a,
            None,
            &Authority::of(["common", "pair:a", "pair:b"]),
        )
        .await
        .expect("one shared tenant policy");
    for (scope, agent, actor, pair_grant) in [
        (&scope_a, &agent_a, &actor_a, "pair:a"),
        (&scope_b, &agent_b, &actor_b, "pair:b"),
    ] {
        let pair_policy = Authority::of(["common", pair_grant]);
        source
            .provision_agent_policy(scope, agent, None, &pair_policy)
            .await
            .expect("independent agent head");
        source
            .provision_trigger_actor_held(scope, actor, None, &pair_policy)
            .await
            .expect("independent actor head");
        source
            .provision_delegation(scope, agent, actor, None, &pair_policy)
            .await
            .expect("independent pair head");
    }

    let resolved_a = source
        .resolve_for_run(&scope_a, &agent_a, &actor_a, &RunId("run:pair:a".into()))
        .await
        .expect("pair A resolves");
    let resolved_b = source
        .resolve_for_run(&scope_b, &agent_b, &actor_b, &RunId("run:pair:b".into()))
        .await
        .expect("pair B resolves");
    assert!(resolved_a.input.delegation.holds("pair:a"));
    assert!(!resolved_a.input.delegation.holds("pair:b"));
    assert!(resolved_b.input.delegation.holds("pair:b"));
    assert!(!resolved_b.input.delegation.holds("pair:a"));
    assert_eq!(resolved_a.effective_policy.caveats, ["common", "pair:a"]);
    assert_eq!(resolved_b.effective_policy.caveats, ["common", "pair:b"]);

    let head_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delegation_policy_head WHERE tenant_id = $1 AND region = $2",
    )
    .bind(&tenant)
    .bind(&region)
    .fetch_one(admin.db_pool())
    .await
    .expect("count distinct scoped heads");
    assert_eq!(head_count, 7, "1 tenant + 2 agent + 2 actor + 2 pair heads");

    cleanup(&admin, &tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_update_and_resolution_never_observe_a_torn_bundle() {
    let Some((admin, app)) = providers().await else {
        eprintln!("SKIP: dev PostgreSQL unavailable");
        return;
    };
    let tenant = unique("concurrent");
    let region = app.config().region.clone();
    let (scope, agent, actor) = principals(&tenant, &region);
    let source = DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(app));
    let v1 = provision_initial(&source, &scope, &agent, &actor, &policy(&[])).await;

    let updater = {
        let source = source.clone();
        let scope = scope.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            source
                .provision_agent_policy(
                    &scope,
                    &agent,
                    Some(&v1.agent),
                    &policy(&["repo:admin"]).agent_policy,
                )
                .await
        })
    };
    let mut readers = Vec::new();
    for index in 0..24 {
        let source = source.clone();
        let scope = scope.clone();
        let agent = agent.clone();
        let actor = actor.clone();
        readers.push(tokio::spawn(async move {
            source
                .resolve_for_run(
                    &scope,
                    &agent,
                    &actor,
                    &RunId(format!("run:concurrent:{index}")),
                )
                .await
        }));
    }
    updater.await.expect("updater task").expect("v2 update");
    for reader in readers {
        let resolved = reader.await.expect("reader task").expect("snapshot");
        let versions = resolved.cursor.versions;
        assert_eq!(versions.delegation, 1);
        assert_eq!(versions.tenant, 1);
        assert_eq!(versions.trigger_actor, 1);
        let has_admin = resolved.input.agent_policy.holds("repo:admin");
        assert_eq!(has_admin, versions.agent == 2);
        assert!(
            !resolved
                .effective_policy
                .caveats
                .contains(&"repo:admin".into()),
            "one widened raw conjunct is never effective authority"
        );
    }

    cleanup(&admin, &tenant).await;
}
