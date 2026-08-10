use myelin_agent::{AgentRuntime, Conversation, MeteredRuntime, StepOutcome};
use myelin_agent_service::{
    MockToolExecutor, MockToolSurface, RunOutcomeKind, RunSubstrate, RunTokenRevoker, SkeletonAgent,
    SkeletonAgentRuntime, SkeletonTelemetry,
};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter, WfJournal};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::agent_run_gate::AgentRunGate;
use myelin_storage::reserve_settle::{CostLedger, MicroUsd};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

#[derive(Default)]
struct ProviderMinter;
impl RunTokenMinter for ProviderMinter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        _caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        Ok(RunTokenHandle {
            token: format!("tok:{agent_id}:{run_id}"),
            jti: format!("jti:{agent_id}:{run_id}"),
            ttl_secs,
        })
    }
}

#[derive(Default)]
struct ProviderRevoker {
    revoked: std::sync::Mutex<std::collections::HashSet<String>>,
}
impl RunTokenRevoker for ProviderRevoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        let mut g = self.revoked.lock().unwrap();
        if !g.insert(jti.to_string()) {
            return 0;
        }
        (now_secs - teardown_secs).max(0) as u64
    }
    fn is_dead(&self, jti: &str, _now: i64) -> bool {
        self.revoked.lock().unwrap().contains(jti)
    }
}

fn agent_principal(tenant: &TenantId) -> Principal {
    Principal::stub(
        PrincipalId("psn:agent-7".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("skeleton".into()),
            on_behalf_of: None,
        },
        tenant.clone(),
    )
}

#[test]
fn provider_skeleton_loop_drives_the_chained_substrate_path() {
    let tenant = TenantId("acme".into());
    let rt = SkeletonAgentRuntime::new();
    let agent_loop = SkeletonAgent::new();
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let outbox = myelin_events::OutboxStore::new();
    let mut tele = SkeletonTelemetry::new();
    let cat = MockToolSurface::new();
    let exec = MockToolExecutor::new();

    let mut sub = RunSubstrate {
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        agent: agent_principal(&tenant),
        run_id: "R1".into(),
        minter_token: Arc::new(ProviderMinter),
        agent_id: "psn:agent-7".into(),
        caveats: DelegationCaveats(vec!["delegated:human-x".into()]),
        token_ttl_secs: 300,
        revoker: &revoker,
        catalogue: &cat,
        executor: &exec,
        wallet: None,
        gate: &mut gate,
        ledger: &mut ledger,
        available: MicroUsd(100),
        estimate: MicroUsd(10),
        outbox: &outbox,
        minter: Arc::new(myelin_events::MonotonicMinter::new()),
        journal: WfJournal::new(),
        trace_writer: Arc::new(myelin_storage::InMemoryAgentTraceStore::new()),
        now_secs: 1000,
    };

    let out = agent_loop
        .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
        .expect("the SKELETON loop drives the chain to completion");

    assert!(
        out.0.contains("completed"),
        "the dispatch tier reads a completed RunOutcome: {out:?}"
    );
    assert_eq!(
        tele.traces_written(),
        1,
        "the loop wrote exactly one trace row"
    );
    assert!(
        tele.ledger_balanced(),
        "reserved == settled (the balanced-ledger gate)"
    );
    assert_eq!(
        tele.tokens_revoked(),
        1,
        "the per-run token was revoked on teardown"
    );
}

#[test]
fn consumer_loop_drives_any_runtime_through_the_dyn_seam() {
    struct OtherSubmit;
    impl AgentRuntime for OtherSubmit {
        fn step(&self, _c: &Conversation) -> StepOutcome {
            StepOutcome::Submit(myelin_agent::Submission("other".into()))
        }
    }
    impl MeteredRuntime for OtherSubmit {}
    let tenant = TenantId("acme".into());
    let agent_loop = SkeletonAgent::new();
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let outbox = myelin_events::OutboxStore::new();
    let mut tele = SkeletonTelemetry::new();
    let cat = MockToolSurface::new();
    let exec = MockToolExecutor::new();
    let mut sub = RunSubstrate {
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        agent: agent_principal(&tenant),
        run_id: "R2".into(),
        minter_token: Arc::new(ProviderMinter),
        agent_id: "psn:agent-7".into(),
        caveats: DelegationCaveats(vec![]),
        token_ttl_secs: 300,
        revoker: &revoker,
        catalogue: &cat,
        executor: &exec,
        wallet: None,
        gate: &mut gate,
        ledger: &mut ledger,
        available: MicroUsd(100),
        estimate: MicroUsd(10),
        outbox: &outbox,
        minter: Arc::new(myelin_events::MonotonicMinter::new()),
        journal: WfJournal::new(),
        trace_writer: Arc::new(myelin_storage::InMemoryAgentTraceStore::new()),
        now_secs: 2000,
    };
    let dyn_rt: &dyn MeteredRuntime = &OtherSubmit;
    let out = agent_loop
        .handle_run(dyn_rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
        .expect("the loop drives a different brain through the seam");
    assert!(
        out.0.contains("completed"),
        "the loop is brain-agnostic - only the decision differs"
    );
    assert!(
        tele.ledger_balanced(),
        "the substrate path is identical for any brain"
    );
}
