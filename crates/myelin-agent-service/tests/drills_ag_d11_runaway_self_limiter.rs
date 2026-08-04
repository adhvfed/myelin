use myelin_agent::{AgentRuntime, Conversation, MeteredRuntime, StepOutcome};
use myelin_agent_service::{
    runaway_brain, AgentFabricCostSignal, MockToolExecutor, MockToolSurface, RunOutcomeKind,
    RunSubstrate, RunTokenRevoker, RunawaySelfLimiter, RunawayStep, SkeletonAgent, SkeletonError,
    SkeletonTelemetry,
};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter, WfJournal};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::agent_run_gate::AgentRunGate;
use myelin_storage::reserve_settle::{CostLedger, MicroUsd, ReservationState, RunId};
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

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent_principal() -> Principal {
    Principal::stub(
        PrincipalId("psn:agent-7".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("mock".into()),
            on_behalf_of: None,
        },
        tenant(),
    )
}

#[test]
fn ag_d11_runaway_mock_loop_stops_at_the_wallet_never_interrupting() {
    let brain = runaway_brain();
    let agent_loop = SkeletonAgent::new();
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let outbox = myelin_events::OutboxStore::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::MonotonicMinter::new());
    let mut tele = SkeletonTelemetry::new();

    let limiter = RunawaySelfLimiter::new(MicroUsd(50), MicroUsd(10));
    let attempts = 12u64;
    let cat = MockToolSurface::new();
    let exec = MockToolExecutor::new();

    let steps: Vec<RunawayStep> = limiter.run_loop(
        &brain,
        attempts,
        &mut tele,
        |run_id, available, estimate, telemetry| -> Result<u64, SkeletonError> {
            let mut sub = RunSubstrate {
                tenant: tenant(),
                region: Region("fr-par".into()),
                agent: agent_principal(),
                run_id: run_id.clone(),
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
                available,
                estimate,
                outbox: &outbox,
                minter: minter.clone(),
                journal: WfJournal::new(),
                now_secs: 1000,
            };
            let before = telemetry.settled();
            agent_loop.handle_run(&brain, &mut sub, telemetry, RunOutcomeKind::Completed)?;
            Ok(telemetry.settled() - before)
        },
    );

    let admitted = steps.iter().filter(|s| s.is_admitted()).count();
    let refused = steps.iter().filter(|s| s.is_refused()).count();
    assert_eq!(
        admitted, 5,
        "the wallet afforded exactly 5 runs - the funded prefix completed"
    );
    assert_eq!(
        refused, 7,
        "the runaway over-budget tail was REFUSED - the loop stopped at the wallet"
    );

    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "the headline zero: 0 in-flight interrupts"
    );
    assert_eq!(
        gate.reserve_refusals(),
        7,
        "the gate counted the runaway refusals"
    );
    assert_eq!(
        gate.runs_dispatched(),
        5,
        "the gate fronted exactly the funded runs"
    );

    assert_eq!(
        tele.traces_written(),
        5,
        "each completed run wrote exactly one trace row"
    );
    assert_eq!(tele.runs_completed(), 5, "5 runs completed the chain");
    assert_eq!(
        tele.runs_killed(),
        0,
        "no run was killed - the loop stopped at the wallet, not by a kill"
    );
    assert_eq!(
        tele.tokens_revoked(),
        12,
        "every run (admitted + refused) tore down its token"
    );

    assert!(
        tele.ledger_balanced(),
        "reserved {} == settled {}",
        tele.reserved(),
        tele.settled()
    );
    assert_eq!(tele.reserved(), 50, "5 runs reserved 10 each");
    assert_eq!(tele.settled(), 50, "every reserved minor-unit was settled");

    for i in 0..5u32 {
        let run = RunId::new(format!("runaway-{i}"));
        assert_eq!(
            ledger.state_of(&tenant(), &run),
            Some(ReservationState::Settled),
            "completed run {i} settled cleanly"
        );
    }
    for i in 5..12u32 {
        let run = RunId::new(format!("runaway-{i}"));
        assert!(
            ledger.state_of(&tenant(), &run).is_none(),
            "refused run {i} never reserved"
        );
    }

    let signal = RunawaySelfLimiter::signal(&steps, ledger.inflight_interrupt_count());
    assert!(signal.is_green(), "AG-D11 must be GREEN: {signal:?}");
    assert!(
        signal.reserve_refusals > 0,
        "the runaway must have been shed"
    );
    assert_eq!(signal.inflight_interrupt_count, 0, "the headline zero");
    assert_eq!(signal.runs_completed, 5);
    eprintln!("AG-D11 (agent-fabric tier) GREEN [2026-06-21]: {signal:?}");
}

#[test]
fn ag_d11_a_live_in_flight_run_survives_an_exhausted_wallet() {
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    let live = gate
        .dispatch(
            &mut ledger,
            tenant(),
            RunId::new("live"),
            MicroUsd(100),
            MicroUsd(100),
        )
        .expect("the live run is funded and dispatched");
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Some(ReservationState::InFlight)
    );

    let brain = runaway_brain();
    let agent_loop = SkeletonAgent::new();
    let outbox = myelin_events::OutboxStore::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::MonotonicMinter::new());
    let mut tele = SkeletonTelemetry::new();
    let cat = MockToolSurface::new();
    let exec = MockToolExecutor::new();
    let limiter = RunawaySelfLimiter::new(MicroUsd(0), MicroUsd(10));
    let steps = limiter.run_loop(
        &brain,
        5,
        &mut tele,
        |run_id, available, estimate, telemetry| -> Result<u64, SkeletonError> {
            let mut sub = RunSubstrate {
                tenant: tenant(),
                region: Region("fr-par".into()),
                agent: agent_principal(),
                run_id,
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
                available,
                estimate,
                outbox: &outbox,
                minter: minter.clone(),
                journal: WfJournal::new(),
                now_secs: 2000,
            };
            let before = telemetry.settled();
            agent_loop.handle_run(&brain, &mut sub, telemetry, RunOutcomeKind::Completed)?;
            Ok(telemetry.settled() - before)
        },
    );
    assert!(
        steps.iter().all(|s| s.is_refused()),
        "every runaway run was refused (empty wallet)"
    );

    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Some(ReservationState::InFlight),
        "the live run kept running - the runaway refusals never touched it"
    );
    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "0 in-flight interrupts"
    );
    live.settle(&mut ledger, &[])
        .expect("the live run settles on its own completion");
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Some(ReservationState::Settled)
    );
}

#[test]
fn ag_d11_limiter_is_brain_independent() {
    struct OtherBrain;
    impl AgentRuntime for OtherBrain {
        fn step(&self, _c: &Conversation) -> StepOutcome {
            StepOutcome::Submit(myelin_agent::Submission("other".into()))
        }
    }
    impl MeteredRuntime for OtherBrain {}
    let other = OtherBrain;
    let agent_loop = SkeletonAgent::new();
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let outbox = myelin_events::OutboxStore::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::MonotonicMinter::new());
    let mut tele = SkeletonTelemetry::new();

    let wallet = MicroUsd(30);
    let estimate = MicroUsd(10);
    let attempts = 8u64;
    let mut spent = MicroUsd(0);
    let mut completed = 0u64;
    let mut refused = 0u64;
    let cat = MockToolSurface::new();
    let exec = MockToolExecutor::new();
    for i in 0..attempts {
        let available = MicroUsd(wallet.0.saturating_sub(spent.0));
        let mut sub = RunSubstrate {
            tenant: tenant(),
            region: Region("fr-par".into()),
            agent: agent_principal(),
            run_id: format!("other-{i}"),
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
            available,
            estimate,
            outbox: &outbox,
            minter: minter.clone(),
            journal: WfJournal::new(),
            now_secs: 3000,
        };
        match agent_loop.handle_run(&other, &mut sub, &mut tele, RunOutcomeKind::Completed) {
            Ok(_) => {
                spent = MicroUsd(spent.0 + estimate.0);
                completed += 1;
            }
            Err(SkeletonError::DispatchRefused(_)) => refused += 1,
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
    assert_eq!(
        completed, 3,
        "a different brain is ALSO stopped by the wallet at exactly 3 runs"
    );
    assert_eq!(
        refused, 5,
        "the runaway tail is shed regardless of which brain runs"
    );
    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "0 interrupts (brain-independent)"
    );

    let signal = AgentFabricCostSignal {
        runs_attempted: attempts,
        runs_completed: completed,
        reserve_refusals: refused,
        inflight_interrupt_count: ledger.inflight_interrupt_count(),
        total_reserved: completed * estimate.0,
        total_settled: completed * estimate.0,
    };
    assert!(
        signal.is_green(),
        "the brain-independent runaway is GREEN: {signal:?}"
    );
}
