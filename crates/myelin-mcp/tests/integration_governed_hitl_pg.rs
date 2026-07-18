//! Live PostgreSQL proof for the governed MCP HITL verdict lifecycle: exact-bound approval,
//! one-shot consumption, approve/reject across fresh routers, and tenant RLS isolation.
//!
//! This deliberately uses a structural signer, memory S7/audit, test approvers, and a counting
//! effect. A single production-composition test spanning secure-file PASETO authentication,
//! durable delegation/S7, live ReBAC, filesystem Git, and durable outbox remains a named gap;
//! those components are covered separately rather than overstated as one end-to-end proof here.
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use myelin_agent::{EffectApi, EffectAuthority, EffectResult, ProposedEffect, RunCtx};
use myelin_config::MyelinConfig;
use myelin_events::{MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind, RunId, RuntimeRef,
};
use myelin_identity_service::delegation::DelegationInput;
use myelin_identity_service::machine_auth::{Authority, MachineKind};
use myelin_identity_service::mint::{RunTokenMinter, StructuralTokenSigner};
use myelin_identity_service::revocation::RevocationStore;
use myelin_identity_service::ResolvedDelegationPolicy;
use myelin_mcp::{
    CallOutcome, GateApproverPolicy, GovernedRouter, OutboxGovernanceAudit, RunPrincipal,
    ToolRegistry,
};
use myelin_storage::hitl_gate_durable::{
    hitl_gate_durable_migrations, GateDecideError, HitlVerdictStore,
};
use myelin_storage::{
    identity_durable_migrations, DurablePrincipalBacking, DurablePrincipalRow, HotTables,
    SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};

fn admin_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
}

fn principal(id: &str, tenant: &str, region: &str, human: bool) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        PrincipalId(id.into()),
        if human {
            PrincipalKind::Human
        } else {
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt:mcp-live-pg".into()),
                on_behalf_of: Some(PrincipalId("human:trigger".into())),
            }
        },
        myelin_identity::DataRole::Controller,
        myelin_identity::PrincipalStatus::Active,
    )
}

struct TestApprovers(Vec<PrincipalId>);

impl GateApproverPolicy for TestApprovers {
    fn eligible_approvers(
        &self,
        _tool: &str,
        _args: &serde_json::Value,
    ) -> Result<Vec<PrincipalId>, String> {
        Ok(self.0.clone())
    }
}

struct CountingEffectApi(Arc<AtomicUsize>);

impl EffectApi for CountingEffectApi {
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        self.0.fetch_add(1, Ordering::SeqCst);
        EffectResult::Applied(myelin_agent::EventId("event:counted".into()))
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        _authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        self.apply(run, effect)
    }
}

fn router(
    provider: SubstrateProvider,
    tenant: &str,
    region: &str,
    run_id: &str,
    agent_id: &str,
    applies: Arc<AtomicUsize>,
) -> GovernedRouter {
    let s7 = RevocationStore::new();
    let minter =
        RunTokenMinter::with_signer_and_tuples(s7, None, Arc::new(StructuralTokenSigner::new()));
    let agent = principal(agent_id, tenant, region, false);
    let trigger = principal("human:trigger", tenant, region, true);
    let scope = TenantScope::from_verified_token(&trigger, Region(region.into()));
    let grants = ["pull_request.merge"];
    let run_id = RunId(run_id.into());
    let resolved_policy = ResolvedDelegationPolicy::synthetic_for_test(
        run_id.clone(),
        agent.principal_id.clone(),
        trigger.principal_id.clone(),
        DelegationInput {
            agent_policy: Authority::of(grants),
            delegation: Authority::of(grants),
            tenant_policy: Authority::of(grants),
            trigger_actor_held: Authority::of(grants),
        },
        1,
    );
    GovernedRouter::with_approver_policy(
        minter,
        RunPrincipal {
            scope,
            agent_id: agent.principal_id.clone(),
            agent,
            trigger_actor: trigger,
            trigger_credential_jti: "trigger-jti".into(),
            trigger_expires_at_unix: i64::MAX,
            run_id,
            resolved_policy,
            caveats: DelegationCaveats(vec!["pull_request.merge".into()]),
            kind: MachineKind::Agent,
            ttl: FailStaticBound {
                static_max_secs: 300,
            },
        },
        Box::new(CountingEffectApi(applies)),
        HitlVerdictStore::with_pg(provider),
        Arc::new(TestApprovers(vec![PrincipalId("human:lead".into())])),
        Arc::new(OutboxGovernanceAudit::new(
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
        )),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approve_reject_restart_and_tenant_isolation_hold_on_live_pg() {
    let admin = match SubstrateProvider::connect(admin_config(), 4).await {
        Ok(provider) => provider,
        Err(_) => {
            eprintln!("SKIP: dev PostgreSQL is unreachable");
            return;
        }
    };
    admin
        .migrate(&hitl_gate_durable_migrations(), &HotTables::none())
        .await
        .expect("HITL migration");
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity migration");
    let app1 = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("app provider one");
    let app2 = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("app provider two");
    let region = app1.config().region.clone();
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let tenant = format!("mcp-hitl-{suffix}");
    let other_tenant = format!("mcp-hitl-other-{suffix}");
    let registry = ToolRegistry::with_git();
    let merge = registry.resolve("git.merge").unwrap();
    let now = Timestamp("2026-07-18T00:00:00Z".into());
    let applies = Arc::new(AtomicUsize::new(0));

    let first = router(
        app1.clone(),
        &tenant,
        &region,
        "run:restart-proof",
        "agent:mcp",
        applies.clone(),
    );
    let gate = match first.call(
        merge,
        &serde_json::json!({"repo":"alpha","number":1}),
        &now,
        None,
    ) {
        CallOutcome::Gated { gate_id, .. } => gate_id,
        other => panic!("expected durable gate, got {other:?}"),
    };
    let second = router(
        app2.clone(),
        &tenant,
        &region,
        "run:restart-proof",
        "agent:mcp",
        applies.clone(),
    );
    second
        .approve_gate(
            &principal("human:lead", &tenant, &region, true),
            &gate,
            &now,
        )
        .expect("fresh router approves");
    for (run_id, agent_id) in [
        ("run:different", "agent:mcp"),
        ("run:restart-proof", "agent:different"),
    ] {
        assert!(matches!(
            router(
                app2.clone(),
                &tenant,
                &region,
                run_id,
                agent_id,
                applies.clone(),
            )
            .call(
                merge,
                &serde_json::json!({"repo":"alpha","number":1}),
                &now,
                Some(&gate),
            ),
            CallOutcome::Denied { .. }
        ));
    }
    assert_eq!(applies.load(Ordering::SeqCst), 0);
    let restarted = router(
        app1.clone(),
        &tenant,
        &region,
        "run:restart-proof",
        "agent:mcp",
        applies.clone(),
    );
    assert!(matches!(
        restarted.call(
            merge,
            &serde_json::json!({"repo":"alpha","number":1}),
            &now,
            Some(&gate)
        ),
        CallOutcome::Applied { .. }
    ));
    assert_eq!(applies.load(Ordering::SeqCst), 1);
    assert!(matches!(
        router(
            app2.clone(),
            &tenant,
            &region,
            "run:restart-proof",
            "agent:mcp",
            applies.clone(),
        )
        .call(
            merge,
            &serde_json::json!({"repo":"alpha","number":1}),
            &now,
            Some(&gate),
        ),
        CallOutcome::Denied { .. }
    ));

    let atomic_principal = format!("human:atomic-{suffix}");
    let backing = DurablePrincipalBacking::new(app1.clone());
    assert!(backing
        .put_principal_and_link_credential(
            &tenant,
            DurablePrincipalRow {
                principal_id: atomic_principal.clone(),
                kind: serde_json::to_string(&PrincipalKind::Human).unwrap(),
                data_role: serde_json::to_string(&myelin_identity::DataRole::Controller).unwrap(),
                status: serde_json::to_string(&myelin_identity::PrincipalStatus::Active).unwrap(),
                profile: None,
            },
            "credential\0link",
        )
        .await
        .is_err());
    assert!(
        backing
            .get_principal(&tenant, &atomic_principal)
            .await
            .unwrap()
            .is_none(),
        "credential-link failure must roll back principal provisioning"
    );
    assert_eq!(
        applies.load(Ordering::SeqCst),
        1,
        "a consumed approval cannot mutate twice"
    );

    let rejected_gate = match first.call(
        merge,
        &serde_json::json!({"repo":"alpha","number":2}),
        &now,
        None,
    ) {
        CallOutcome::Gated { gate_id, .. } => gate_id,
        other => panic!("expected second gate, got {other:?}"),
    };
    assert_eq!(
        second.reject_gate(
            &principal("human:stranger", &tenant, &region, true),
            &rejected_gate,
            &now,
        ),
        Err(GateDecideError::NotEligible)
    );
    assert_eq!(
        second.reject_gate(
            &principal("agent:mcp", &tenant, &region, true),
            &rejected_gate,
            &now,
        ),
        Err(GateDecideError::SelfApproval)
    );
    assert_eq!(
        second.reject_gate(
            &principal("human:lead", &tenant, &region, false),
            &rejected_gate,
            &now,
        ),
        Err(GateDecideError::MachineApproverRefused)
    );
    second
        .reject_gate(
            &principal("human:lead", &tenant, &region, true),
            &rejected_gate,
            &now,
        )
        .expect("fresh router rejects");
    assert!(matches!(
        router(
            app2.clone(),
            &tenant,
            &region,
            "run:restart-proof",
            "agent:mcp",
            applies.clone(),
        )
        .call(
            merge,
            &serde_json::json!({"repo":"alpha","number":2}),
            &now,
            Some(&rejected_gate)
        ),
        CallOutcome::Denied { .. }
    ));

    assert!(matches!(
        router(
            app2,
            &other_tenant,
            &region,
            "run:restart-proof",
            "agent:mcp",
            applies.clone(),
        )
        .call(
            merge,
            &serde_json::json!({"repo":"alpha","number":1}),
            &now,
            Some(&gate)
        ),
        CallOutcome::Denied { .. }
    ));

    for cleanup_tenant in [&tenant, &other_tenant] {
        let _ = sqlx::query("DELETE FROM agent_hitl_gate WHERE tenant_id = $1 AND region = $2")
            .bind(cleanup_tenant)
            .bind(&region)
            .execute(admin.db_pool())
            .await;
    }
}
