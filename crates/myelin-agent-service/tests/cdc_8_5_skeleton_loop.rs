//! # The CDC pair for contract 8.5 — `Agent::handle` SKELETON loop body (AG-P4 → P-216)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.5
//! (`Agent::handle(InboxEvent, &dyn AgentRuntime) → RunOutcome` — the platform-owned bounded
//! multi-turn loop; a run is a durable workflow; nested causality). Owning architecture:
//! `agent-fabric.md` §2.3 / §5.1 / §5.6. AG-P1 (→ P-130) shipped the SIGNATURE-half CDC
//! (`myelin-agent/tests/cdc_8_5_agent_handle.rs`); THIS pair pins the LOOP-BODY half AG-P4 owns: the
//! provider drives the chained substrate path (mint → reserve → step → trace → settle → revoke) and
//! the consumer (the dispatch tier) reads the run outcome.
//!
//! This is the CDC the prompt's TESTS field names ("the provider+consumer CDC for 8.5") for the
//! loop-body deliverable — distinct from, and extending, the AG-P1 signature CDC (no duplication).

use myelin_agent::{AgentRuntime, Conversation, MeteredRuntime, StepOutcome};
use myelin_agent_service::{
    MockToolExecutor, MockToolSurface, RunOutcomeKind, RunSubstrate, RunTokenRevoker, SkeletonAgent,
    SkeletonAgentRuntime, SkeletonTelemetry,
};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter, WfJournal};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::agent_run_gate::AgentRunGate;
use myelin_storage::reserve_settle::{CostLedger, MinorUnits};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

/// A REAL provider on the contract-4.7 mint surface (the Identity side). Binds the jti to
/// `(agent, run)` — token life == run life.
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

/// A REAL provider on the contract-4.7 revoke surface (the Identity revocation side).
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

/// **PROVIDER side of 8.5 (the agent fabric's SKELETON loop body).** The platform-owned
/// `Agent::handle` loop drives the brain through the `&dyn AgentRuntime` seam AND chains the whole
/// substrate path (mint → reserve → step → trace → settle → revoke) as a durable workflow. It
/// returns a `RunOutcome` the dispatch tier reads. The brain is dynamically dispatched (swappable
/// mock/real); the loop is platform-owned (identical for mock and real).
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
    // The SKELETON registers no tools: an EMPTY catalogue + a no-op executor (the loop body is
    // never entered — the SKELETON submits on turn 0).
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
        available: MinorUnits(100),
        estimate: MinorUnits(10),
        outbox: &outbox,
        minter: Arc::new(myelin_events::MonotonicMinter::new()),
        journal: WfJournal::new(),
        now_secs: 1000,
    };

    let out = agent_loop
        .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
        .expect("the SKELETON loop drives the chain to completion");

    // CONSUMER side of 8.5 (the dispatch tier): reads the run outcome — the run completed, a trace
    // was written, and the ledger is balanced (reserved == settled). Observability is part of the
    // pass (a path that emits no signal has failed the drill).
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

/// **CONSUMER side of 8.5 — the brain is dynamically dispatched + swappable (the strategy seam).**
/// The loop drives ANY `AgentRuntime` through the `&dyn` seam; a different deterministic brain on the
/// SAME loop produces a `RunOutcome` the dispatch tier reads identically (the loop is the constant;
/// only the brain swaps — the whole point of the SKELETON → mock → real build order).
#[test]
fn consumer_loop_drives_any_runtime_through_the_dyn_seam() {
    // A second deterministic brain (still no model) — proves the loop is brain-agnostic.
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
        available: MinorUnits(100),
        estimate: MinorUnits(10),
        outbox: &outbox,
        minter: Arc::new(myelin_events::MonotonicMinter::new()),
        journal: WfJournal::new(),
        now_secs: 2000,
    };
    let dyn_rt: &dyn MeteredRuntime = &OtherSubmit;
    let out = agent_loop
        .handle_run(dyn_rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
        .expect("the loop drives a different brain through the seam");
    assert!(
        out.0.contains("completed"),
        "the loop is brain-agnostic — only the decision differs"
    );
    assert!(
        tele.ledger_balanced(),
        "the substrate path is identical for any brain"
    );
}
