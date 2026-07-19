//! # The reserve/settle parity drill — CI-D5 (CI-P17 → P-360, M4)
//!
//! **The CI-P17 GATE (the drill catalogue row CI-D5, EI-01 §4 — chain mutations end-to-end):** exhaust
//! ONE wallet, then start a CI run AND an agent compute job → BOTH refuse-start (never interrupt in
//! flight); replay the settled metered units across a PRICING CHANGE → **0 starts past exhaustion,
//! wholesale ≠ markup holds (one `cost_event` per metered unit)**. The headline numbers: 0
//! over-exhaustion starts, cost parity CI ↔ agent.
//!
//! This drill exercises TWO layers of the ONE metering path (arch 02 §6 — CI builds no second metering
//! path):
//!
//! 1. **The CI meter parity aggregate** ([`reserve_settle_parity_drill`]) — the synthetic CI + agent
//!    runs against ONE shared [`myelin_flow::BudgetGate`]/wallet, asserting the green
//!    [`ReserveSettleParitySignal`] (both kinds refuse-start, 0 over-exhaustion, wholesale ≠ markup
//!    stable under a pricing change).
//! 2. **The REAL `ci.pipeline` body on the SAME shared wallet** — a CI pipeline stage dispatched
//!    through the FROZEN [`myelin_flow::WfCtx::metered_schedule_and_run_job`] (the per-stage
//!    reserve/settle bookend the body uses) refuses-to-start when the shared wallet is exhausted by a
//!    prior agent reserve: the runner is NEVER handed the job (the parity bites the real body, not just
//!    the synthetic aggregate). The in-sandbox EXECUTION of the dispatched job is GATED by AG-D4; this
//!    drill exercises the reserve-fronts-the-dispatch bookend over the engine.
//!
//! FLOOR named: the resource-second → credit/price MARKUP mapping is Commercial's (arch 06 R-2); the
//! drill supplies a `FlatBpsMarkup` test stand-in for the markup column (the LIVE pricing table is
//! Commercial-owned). CI owns only the meter + the wholesale column.
//!
//! ## The CDC pair (contract 11.7 — reserve/settle, the CI run-dispatch consumer half)
//! This file is the CI half of the row-11.7 consumer-driven-contract pair:
//! - **CONSUMER side** — CI is the run-dispatch CONSUMER of the frozen reserve/settle ledger (contract
//!   11.7, Storage-owned): the CI meter reserves/settles against the SAME `BudgetGate`/`CostLedger`
//!   the agent fabric consumes, proving the gate fronts CI runs (the named M4 follow-on row 11.7
//!   recorded) with the SAME refuse-to-start / never-interrupt-in-flight semantics the agent consumer
//!   relies on (the cost-parity property).
//! - **PROVIDER side** — CI is the PROVIDER of the resource-second meter + the `cost_event` schema row
//!   (the wholesale column + `kind ∈ {ci, agent}`); the markup column is the Commercial R-2 provider's
//!   (carried via the `MarkupPolicy` seam). The provider assertion: one `cost_event` per metered unit,
//!   wholesale ≠ markup, the wholesale basis STABLE under a pricing change.

use myelin_ci_controlplane::ci_pipeline::{
    run_ci_pipeline_body, CheckFacts, PipelineRun, PipelineStage,
};
use myelin_ci_controlplane::{
    reserve_settle_parity_drill, FlatBpsMarkup, Meter, CI_PIPELINE_WF_TYPE,
};
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    BudgetGate, CiStage, JobKind, JobRunner, JobSpec, MinorUnits, TimerStore, Wallet, WfCtx,
    WfJournal,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::reserve_settle::RunId as LedgerRunId;
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: None,
    }
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

/// A runner that records how many times it was handed a job (the "did the dispatch start" probe).
#[derive(Default)]
struct RecordingRunner {
    calls: AtomicUsize,
    dispatched: Mutex<Vec<JobSpec>>,
}
impl JobRunner for RecordingRunner {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.dispatched.lock().unwrap().push(spec.clone());
        Ok(())
    }
}

/// **CI-D5 (layer 1): the reserve/settle parity aggregate is GREEN.** Both a CI run AND an agent run
/// refuse-start against the exhausted shared wallet; 0 starts past exhaustion; one cost_event per
/// metered unit; and a pricing change re-prices the markup column while the wholesale column + the
/// 0-over-exhaustion property are STABLE.
#[test]
fn ci_d5_reserve_settle_parity_aggregate_is_green() {
    let samples = [
        (Meter::CpuSeconds, 120u64, MinorUnits(200)),
        (Meter::MemGbSeconds, 64, MinorUnits(40)),
        (Meter::EgressGb, 3, MinorUnits(30)),
    ];
    let before = FlatBpsMarkup::new(2_000); // 20%
    let after = FlatBpsMarkup::new(4_000); // a PRICING CHANGE — 40%

    let signal =
        reserve_settle_parity_drill(&tenant(), MinorUnits(300), 6, &samples, &before, &after);

    assert!(
        signal.is_green(),
        "CI-D5 parity drill must be GREEN: {signal:?}"
    );
    assert!(
        signal.ci_refused_when_exhausted && signal.agent_refused_when_exhausted,
        "the PARITY: both a CI run and an agent run refuse-start past exhaustion"
    );
    assert_eq!(
        signal.starts_past_exhaustion, 0,
        "0 over-exhaustion starts (the headline)"
    );
    assert_eq!(signal.inflight_interrupt_count, 0, "0 in-flight interrupts");
    // 6 runs * 3 dimensions = 18 cost events, one per metered unit.
    assert_eq!(signal.cost_events_recorded, 18);
    assert_eq!(signal.metered_units, 18);
    // BOTH kinds metered into the SAME wallet/ledger (the unified-meter parity — 3 ci + 3 agent runs
    // * 3 dimensions = 9 events each).
    assert_eq!(
        signal.ci_cost_events, 9,
        "CI runs metered into the shared path"
    );
    assert_eq!(
        signal.agent_cost_events, 9,
        "agent runs metered into the SAME path"
    );
    // wholesale is the SAME basis under both pricings (the pricing change touches ONLY markup).
    assert_ne!(
        signal.wholesale_total, signal.markup_total_before,
        "wholesale ≠ markup"
    );
    assert_ne!(
        signal.markup_total_before, signal.markup_total_after,
        "the pricing change re-prices the markup column, never the wholesale"
    );
}

/// **CI-D5 (layer 2): the REAL `ci.pipeline` body refuses-to-start a CI stage when the SHARED wallet
/// is exhausted by a prior agent reserve — the runner is NEVER handed the job (the parity bites the
/// real body).** This proves the reserve/settle bookend the body uses (`metered_schedule_and_run_job`)
/// and the CI meter draw down the SAME wallet (UNIFY / X-6): an agent run that exhausts the wallet
/// stops the next CI run at the reserve, exactly as a CI run would stop the next agent run.
#[test]
fn ci_d5_ci_pipeline_body_refuses_when_shared_wallet_exhausted_by_agent() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let runner = RecordingRunner::default();

    // ONE shared wallet with exactly enough for ONE 200-unit reserve (the agent run will take it).
    let gate = BudgetGate::new(Wallet::new(MinorUnits(200)));

    // An AGENT run exhausts the shared wallet first (the same gate both kinds use). This is the
    // unified-meter property: a prior agent reserve depletes the wallet the CI body will draw.
    let agent_run = LedgerRunId::new("agent/run/storm");
    gate.reserve(&tenant(), &agent_run, MinorUnits(200))
        .expect("the agent run reserves the whole wallet");
    gate.begin(&tenant(), &agent_run)
        .expect("the agent run is in-flight (never interrupted)");

    // Now drive the REAL ci.pipeline body with a single 100-unit CI stage over the SAME gate. The
    // reserve must REFUSE (the wallet is exhausted) → the runner is NEVER called.
    let run = PipelineRun {
        stages: vec![PipelineStage::job(CiStage::new(
            "build",
            "pipeline://acme/ci/pr-7#build",
            MinorUnits(100), // the CI stage's reserve — refused, the wallet is empty.
            Some(600),
        ))],
        contexts: vec!["build".to_string()],
        facts: CheckFacts {
            repo: "myelin://acme/repo/web".into(),
            commit_oid: "deadbeef".into(),
            run_ref: "myelin://acme/ci/run/9".into(),
            run_attempt: 0,
            trust_tier: "trusted".into(),
            merge_idem_token: "merge-attempt-9".into(),
        },
    };

    let mut ctx = WfCtx::begin(
        &outbox,
        minter(),
        journal,
        ctx_base(),
        "R9",
        CI_PIPELINE_WF_TYPE,
        "2026-06-23T00:00:00Z",
        42,
    )
    .with_budget(gate.clone());

    let err = run_ci_pipeline_body(&mut ctx, &run, &runner)
        .expect_err("the CI stage's reserve must be REFUSED on the exhausted shared wallet");
    // The refusal is the loud no-balance → no-dispatch floor surfaced through the body.
    assert!(
        format!("{err:?}").contains("wallet exhausted") || format!("{err:?}").contains("refused"),
        "the refusal is loud: {err:?}"
    );
    // The runner was NEVER handed the job — the CI run did not start (the parity: an agent storm stops
    // the next CI run exactly as a CI storm stops the next agent run).
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        0,
        "the CI job was NEVER handed to the runner (refuse-to-start, never interrupt in flight)"
    );
    // The agent run was never interrupted by the CI refusal — 0 in-flight interrupts.
    assert_eq!(
        gate.inflight_interrupt_count(),
        0,
        "the in-flight agent run was never torn down (the headline zero)"
    );
}

/// **The CI stage STARTS + settles its resource-seconds when the shared wallet is funded — and the
/// settle records the cost into the SAME wallet (the positive half of the parity).** A funded CI body
/// dispatches once, completes on `job.done`, and the wallet is drawn only the settled amount.
#[test]
fn ci_d5_funded_ci_stage_starts_and_settles_into_the_shared_wallet() {
    use myelin_flow::JobOutcome;

    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = myelin_flow::SignalStore::new();
    let runner = RecordingRunner::default();

    // A funded shared wallet.
    let gate = BudgetGate::new(Wallet::new(MinorUnits(1_000)));

    // Pre-deliver the stage's job.done (a fast job) under the deterministic dispatch token so the
    // long-park completes in one drive.
    // The dispatch command id is `<wf-name>:<n>` (the WfCtx was begun with CI_PIPELINE_WF_TYPE as the
    // workflow name), so the first metered dispatch's job token keys on `ci.pipeline:0`.
    let token = myelin_flow::job_idem_token("R10", &format!("{CI_PIPELINE_WF_TYPE}:0"));
    signals.deliver(myelin_flow::engine::SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: "R10".into(),
        signal_name: myelin_flow::job::JOB_DONE_SIGNAL.into(),
        idem_key: token,
        payload: vec![myelin_refs::ArtifactRef("myelin://acme/ci/green".into())],
        payload_key_ref: None,
        received_unix_ms: 0,
        consumed_seq: None,
    });

    let mut ctx = WfCtx::begin(
        &outbox,
        minter(),
        journal,
        ctx_base(),
        "R10",
        CI_PIPELINE_WF_TYPE,
        "2026-06-23T00:00:00Z",
        42,
    )
    .with_signals(signals)
    .with_timers(TimerStore::new(), 0, 1_000)
    .with_budget(gate.clone());

    // Drive a single metered CI stage directly over the body's engine bookend: reserve 300, bill the
    // resource-seconds (a cpu + mem unit) on the job.done, refund the over-reservation.
    let out = ctx
        .metered_schedule_and_run_job(
            JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-10#build"),
            &runner,
            Some(600),
            MinorUnits(300),
            vec![
                myelin_storage::reserve_settle::MeteredUnit {
                    unit: Meter::CpuSeconds.token(),
                    wholesale: MinorUnits(120),
                    markup: MinorUnits(24),
                },
                myelin_storage::reserve_settle::MeteredUnit {
                    unit: Meter::MemGbSeconds.token(),
                    wholesale: MinorUnits(40),
                    markup: MinorUnits(8),
                },
            ],
        )
        .expect("the funded CI stage dispatches + completes");

    match out {
        JobOutcome::Completed { result, .. } => {
            assert_eq!(
                result,
                vec![myelin_refs::ArtifactRef("myelin://acme/ci/green".into())]
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "the CI stage dispatched once"
    );
    // wallet: 1000 − 300 (reserve) + refund(300 − billed(120+24+40+8 = 192) = 108) = 808.
    assert_eq!(
        gate.balance(),
        MinorUnits(808),
        "settled the CI stage's resource-seconds into the SAME wallet (only the billed 192 drawn)"
    );
    assert_eq!(gate.inflight_interrupt_count(), 0, "0 in-flight interrupts");
    // The meter token used IS a frozen cost_event.meter value (the wholesale meter, X-6).
    assert!(Meter::from_token("cpu_seconds").is_some());
}
