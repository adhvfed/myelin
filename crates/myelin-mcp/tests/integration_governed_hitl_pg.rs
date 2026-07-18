//! Live PostgreSQL proof for the governed MCP HITL lifecycle: approve/reject survive fresh router
//! instances and a gate id never crosses the tenant RLS partition.
#![cfg(feature = "integration")]

use std::sync::Arc;

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
    SkeletonEffectApi, ToolRegistry,
};
use myelin_storage::hitl_gate_durable::{
    hitl_gate_durable_migrations, GateDecideError, HitlVerdictStore,
};
use myelin_storage::{HotTables, SubstrateProvider, TenantScope};
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

fn router(provider: SubstrateProvider, tenant: &str, region: &str) -> GovernedRouter {
    let s7 = RevocationStore::new();
    let minter =
        RunTokenMinter::with_signer_and_tuples(s7, None, Arc::new(StructuralTokenSigner::new()));
    let agent = principal("agent:mcp", tenant, region, false);
    let trigger = principal("human:trigger", tenant, region, true);
    let scope = TenantScope::from_verified_token(&trigger, Region(region.into()));
    let grants = ["pull_request.merge"];
    let run_id = RunId("run:restart-proof".into());
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
        Box::new(SkeletonEffectApi::new()),
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

    let first = router(app1.clone(), &tenant, &region);
    let gate = match first.call(
        merge,
        &serde_json::json!({"repo":"alpha","number":1}),
        &now,
        None,
    ) {
        CallOutcome::Gated { gate_id, .. } => gate_id,
        other => panic!("expected durable gate, got {other:?}"),
    };
    let second = router(app2.clone(), &tenant, &region);
    second
        .approve_gate(&principal("human:lead", &tenant, &region, true), &gate)
        .expect("fresh router approves");
    let restarted = router(app1.clone(), &tenant, &region);
    assert!(matches!(
        restarted.call(
            merge,
            &serde_json::json!({"repo":"alpha","number":1}),
            &now,
            Some(&gate)
        ),
        CallOutcome::Applied { .. }
    ));

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
        ),
        Err(GateDecideError::NotEligible)
    );
    assert_eq!(
        second.reject_gate(
            &principal("agent:mcp", &tenant, &region, true),
            &rejected_gate,
        ),
        Err(GateDecideError::SelfApproval)
    );
    assert_eq!(
        second.reject_gate(
            &principal("human:lead", &tenant, &region, false),
            &rejected_gate,
        ),
        Err(GateDecideError::MachineApproverRefused)
    );
    second
        .reject_gate(
            &principal("human:lead", &tenant, &region, true),
            &rejected_gate,
        )
        .expect("fresh router rejects");
    assert!(matches!(
        router(app2.clone(), &tenant, &region).call(
            merge,
            &serde_json::json!({"repo":"alpha","number":2}),
            &now,
            Some(&rejected_gate)
        ),
        CallOutcome::Denied { .. }
    ));

    assert!(matches!(
        router(app2, &other_tenant, &region).call(
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
