//! # AG-D11 — the reserve/settle cost gate as the runaway self-limiter (AG-P14 → P-227, M2-B).
//!
//! The chained-e2e drill the AG-P14 prompt's TESTS field names: *drive a runaway mock loop into an
//! exhausted wallet → assert reserve refuses the next run, the in-flight one completes, and the loop
//! stops at the wallet.* Drill catalogue
//! (`testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4, row **AG-D11** / F9):
//! *Runaway loop vs an exhausted wallet → reserve refuses new runs (never interrupts in-flight); the
//! loop stops at the wallet.* Telemetry: **reserve refusals; 0 interrupt**.
//!
//! Unlike the raw gate-level AG-D11 in `myelin-storage` (which drives the `AgentRunGate` directly),
//! THIS drill exercises the FULL agent-fabric path: a [`MockAgentRuntime`] brain looping run-after-run
//! through [`SkeletonAgent::handle_run`] (mint → reserve → step → trace → settle → revoke) against ONE
//! draining wallet. It proves the cost gate is the runaway self-limiter at the tier the agent fabric
//! actually runs, NOT just the storage primitive. EI-01 §4 — real sessions CHAIN mutations.
//!
//! The dated GREEN artifact: `reserve_refusals > 0` (the runaway over-budget tail was shed) AND
//! `inflight_interrupt_count == 0` (no in-flight run torn down — the loop stops at the wallet, not by
//! a kill) AND `runs_completed + reserve_refusals == runs_attempted` (no run silently vanished) AND
//! `total_reserved == total_settled` (every minor-unit a completed run reserved was settled — a Mock
//! bills 0, the reservation refunds). The M2-B deterministic-correctness family (AG-D1/D2/D3/D5/D7/
//! D8/D9/D11) is now complete and green.
//!
//! FLOOR (EI-01 §1): the REAL per-model-call cost event arrives with `LlmAgentRuntime` (AG-P25,
//! post-M5 — designed-not-built). The Mock meters ZERO, which is correct: the limiter is
//! brain-independent (the wallet stops the loop regardless of which brain runs — that is the point).
//! The gate MECHANISM (reserve refuses past exhaustion, never interrupts in-flight, settles on
//! completion) is complete. No new data-layer trait is touched (this drives the Storage-owned
//! AgentRunGate/CostLedger proven against the live PG tier by P-103/P-146) → no new integration drill
//! owed (recorded in the P-227 report).

use myelin_agent::{AgentRuntime, Conversation, MeteredRuntime, StepOutcome};
use myelin_agent_service::{
    runaway_brain, AgentFabricCostSignal, MockToolExecutor, MockToolSurface, RunOutcomeKind,
    RunSubstrate, RunTokenRevoker, RunawaySelfLimiter, RunawayStep, SkeletonAgent, SkeletonError,
    SkeletonTelemetry,
};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter, WfJournal};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::agent_run_gate::AgentRunGate;
use myelin_storage::reserve_settle::{CostLedger, MinorUnits, ReservationState, RunId};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

/// A REAL provider on the contract-4.7 mint surface — binds the jti to `(agent, run)`.
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

/// A REAL provider on the contract-4.7 revoke surface — idempotent even on crash.
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

/// **THE AG-D11 CHAINED E2E DRILL — a runaway mock loop into an exhausted wallet.**
///
/// A wallet of 50 minor-units affords exactly 5 runs of 10. A runaway loop tries 12 runs. The first
/// 5 are ADMITTED — each drives the FULL SKELETON substrate path (mint → reserve → step → trace →
/// settle → revoke) and completes. Once the wallet cannot afford the 6th, the reserve REFUSES it (no
/// balance → no run); the loop stops at the wallet, NOT by a kill. The already-completed runs were
/// never interrupted — `inflight_interrupt_count == 0`.
#[test]
fn ag_d11_runaway_mock_loop_stops_at_the_wallet_never_interrupting() {
    let brain = runaway_brain();
    let agent_loop = SkeletonAgent::new();
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let outbox = myelin_events::OutboxStore::new();
    // ONE shared id minter across the loop (the realistic dispatch-tier shape — a per-cell minter):
    // event ids stay globally unique across runs (no UNIQUE(event_id) collision).
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::MonotonicMinter::new());
    let mut tele = SkeletonTelemetry::new();

    // The wallet affords exactly 5 runs of 10; the runaway loop tries 12.
    let limiter = RunawaySelfLimiter::new(MinorUnits(50), MinorUnits(10));
    let attempts = 12u64;
    // The runaway brain submits on turn 0 (single-turn) → an EMPTY catalogue + a no-op executor
    // (the driving loop's tool body is never entered).
    let cat = MockToolSurface::new();
    let exec = MockToolExecutor::new();

    // Drive each run through the REAL SkeletonAgent::handle_run path. `drive_one` builds a fresh
    // substrate over the SHARED gate/ledger (per-run mutable borrows are scoped to this call), drives
    // the mock-brained run to completion, and returns the settled minor-units (a Mock bills 0 → the
    // whole reservation refunds → settled == reserved). A no-balance reserve surfaces
    // SkeletonError::DispatchRefused — which the limiter records as a RunawayStep::Refused.
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
            // The settled delta this run contributed (a Mock bills 0 → == the reservation).
            Ok(telemetry.settled() - before)
        },
    );

    // Exactly the funded prefix completed; the runaway tail was shed.
    let admitted = steps.iter().filter(|s| s.is_admitted()).count();
    let refused = steps.iter().filter(|s| s.is_refused()).count();
    assert_eq!(
        admitted, 5,
        "the wallet afforded exactly 5 runs — the funded prefix completed"
    );
    assert_eq!(
        refused, 7,
        "the runaway over-budget tail was REFUSED — the loop stopped at the wallet"
    );

    // NOT ONE in-flight run was interrupted by the refusals (the gate has no tear-down-in-flight API).
    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "the headline zero: 0 in-flight interrupts"
    );
    // The gate counted exactly the refusals (the AG-D11 reserve-refusal telemetry).
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

    // Every ADMITTED run completed the full chain: 5 traces written, 5 tokens revoked (one per run);
    // the refused runs STILL torn down their (un-dispatched) tokens (the teardown is unconditional).
    assert_eq!(
        tele.traces_written(),
        5,
        "each completed run wrote exactly one trace row"
    );
    assert_eq!(tele.runs_completed(), 5, "5 runs completed the chain");
    assert_eq!(
        tele.runs_killed(),
        0,
        "no run was killed — the loop stopped at the wallet, not by a kill"
    );
    assert_eq!(
        tele.tokens_revoked(),
        12,
        "every run (admitted + refused) tore down its token"
    );

    // The books balance: every completed run reserved 10 and settled 10 (a Mock bills 0, refunds 10).
    assert!(
        tele.ledger_balanced(),
        "reserved {} == settled {}",
        tele.reserved(),
        tele.settled()
    );
    assert_eq!(tele.reserved(), 50, "5 runs reserved 10 each");
    assert_eq!(tele.settled(), 50, "every reserved minor-unit was settled");

    // Every completed run's reservation is SETTLED in the ledger (not left dangling, not interrupted).
    for i in 0..5u32 {
        let run = RunId::new(format!("runaway-{i}"));
        assert_eq!(
            ledger.state_of(&tenant(), &run),
            Some(ReservationState::Settled),
            "completed run {i} settled cleanly"
        );
    }
    // The refused runs have NO reservation row (the run never started — no balance → no run).
    for i in 5..12u32 {
        let run = RunId::new(format!("runaway-{i}"));
        assert!(
            ledger.state_of(&tenant(), &run).is_none(),
            "refused run {i} never reserved"
        );
    }

    // THE GREEN ARTIFACT.
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

/// **The in-flight run that is RUNNING when the wallet is exhausted is NEVER interrupted.** A subtle
/// leg: dispatch one run and leave it in-flight (not yet settled), then drive a runaway loop against
/// the now-empty wallet — every loop run is refused, and the live in-flight run is UNTOUCHED (still
/// in-flight, still settle-able). This pins "never interrupt ONE in flight" precisely.
#[test]
fn ag_d11_a_live_in_flight_run_survives_an_exhausted_wallet() {
    let revoker = ProviderRevoker::default();
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    // Dispatch ONE run that holds the WHOLE wallet (100/100) and leave it IN-FLIGHT (do not settle).
    let live = gate
        .dispatch(
            &mut ledger,
            tenant(),
            RunId::new("live"),
            MinorUnits(100),
            MinorUnits(100),
        )
        .expect("the live run is funded and dispatched");
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Some(ReservationState::InFlight)
    );

    // Now a runaway loop against the EXHAUSTED wallet (0 remaining): every run is refused.
    let brain = runaway_brain();
    let agent_loop = SkeletonAgent::new();
    let outbox = myelin_events::OutboxStore::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::MonotonicMinter::new());
    let mut tele = SkeletonTelemetry::new();
    let cat = MockToolSurface::new();
    let exec = MockToolExecutor::new();
    let limiter = RunawaySelfLimiter::new(MinorUnits(0), MinorUnits(10));
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

    // THE LIVE IN-FLIGHT RUN IS UNTOUCHED — still in-flight, never interrupted, still settle-able.
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Some(ReservationState::InFlight),
        "the live run kept running — the runaway refusals never touched it"
    );
    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "0 in-flight interrupts"
    );
    // It settles NORMALLY on completion — it was never torn down.
    live.settle(&mut ledger, &[])
        .expect("the live run settles on its own completion");
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Some(ReservationState::Settled)
    );
}

/// **The runaway self-limiter is brain-INDEPENDENT.** The cost gate stops the loop regardless of
/// which `AgentRuntime` runs — proving the limiter does not depend on the brain (or on the cost being
/// non-zero). A different deterministic brain on the SAME loop hits the SAME wallet wall.
#[test]
fn ag_d11_limiter_is_brain_independent() {
    // A second deterministic brain (no model) — still stopped by the wallet, not by itself.
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

    // The wallet affords 3 runs of 10; the loop tries 8.
    let wallet = MinorUnits(30);
    let estimate = MinorUnits(10);
    let attempts = 8u64;
    let mut spent = MinorUnits(0);
    let mut completed = 0u64;
    let mut refused = 0u64;
    // OtherBrain submits on turn 0 → an EMPTY catalogue + a no-op executor (loop body never entered).
    let cat = MockToolSurface::new();
    let exec = MockToolExecutor::new();
    for i in 0..attempts {
        let available = MinorUnits(wallet.0.saturating_sub(spent.0));
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
                spent = MinorUnits(spent.0 + estimate.0);
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
