use myelin_ci_controlplane::ci_pipeline::{
    run_ci_pipeline_body, CheckFacts, PipelineRun, PipelineStage,
};
use myelin_ci_controlplane::{
    reserve_settle_parity_drill, FlatBpsMarkup, Meter, CI_PIPELINE_WF_TYPE,
};
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    BudgetGate, CiStage, JobKind, JobRunner, JobSpec, MicroUsd, TimerStore, Wallet, WfCtx,
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

#[test]
fn ci_d5_reserve_settle_parity_aggregate_is_green() {
    let samples = [
        (Meter::CpuSeconds, 120u64, MicroUsd(200)),
        (Meter::MemGbSeconds, 64, MicroUsd(40)),
        (Meter::EgressGb, 3, MicroUsd(30)),
    ];
    let before = FlatBpsMarkup::new(2_000);
    let after = FlatBpsMarkup::new(4_000);

    let signal =
        reserve_settle_parity_drill(&tenant(), MicroUsd(300), 6, &samples, &before, &after);

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
    assert_eq!(signal.cost_events_recorded, 18);
    assert_eq!(signal.metered_units, 18);
    assert_eq!(
        signal.ci_cost_events, 9,
        "CI runs metered into the shared path"
    );
    assert_eq!(
        signal.agent_cost_events, 9,
        "agent runs metered into the SAME path"
    );
    assert_ne!(
        signal.wholesale_total, signal.markup_total_before,
        "wholesale ≠ markup"
    );
    assert_ne!(
        signal.markup_total_before, signal.markup_total_after,
        "the pricing change re-prices the markup column, never the wholesale"
    );
}

#[test]
fn ci_d5_ci_pipeline_body_refuses_when_shared_wallet_exhausted_by_agent() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let runner = RecordingRunner::default();

    let gate = BudgetGate::new(Wallet::new(MicroUsd(200)));

    let agent_run = LedgerRunId::new("agent/run/storm");
    gate.reserve(&tenant(), &agent_run, MicroUsd(200))
        .expect("the agent run reserves the whole wallet");
    gate.begin(&tenant(), &agent_run)
        .expect("the agent run is in-flight (never interrupted)");

    let run = PipelineRun {
        stages: vec![PipelineStage::job(CiStage::new(
            "build",
            "pipeline://acme/ci/pr-7#build",
            MicroUsd(100),
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
    assert!(
        format!("{err:?}").contains("wallet exhausted") || format!("{err:?}").contains("refused"),
        "the refusal is loud: {err:?}"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        0,
        "the CI job was NEVER handed to the runner (refuse-to-start, never interrupt in flight)"
    );
    assert_eq!(
        gate.inflight_interrupt_count(),
        0,
        "the in-flight agent run was never torn down (the headline zero)"
    );
}

#[test]
fn ci_d5_funded_ci_stage_starts_and_settles_into_the_shared_wallet() {
    use myelin_flow::JobOutcome;

    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = myelin_flow::SignalStore::new();
    let runner = RecordingRunner::default();

    let gate = BudgetGate::new(Wallet::new(MicroUsd(1_000)));

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

    let out = ctx
        .metered_schedule_and_run_job(
            JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-10#build"),
            &runner,
            Some(600),
            MicroUsd(300),
            vec![
                myelin_storage::reserve_settle::MeteredUnit {
                    unit: Meter::CpuSeconds.token(),
                    wholesale: MicroUsd(120),
                    markup: MicroUsd(24),
                },
                myelin_storage::reserve_settle::MeteredUnit {
                    unit: Meter::MemGbSeconds.token(),
                    wholesale: MicroUsd(40),
                    markup: MicroUsd(8),
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
    assert_eq!(
        gate.balance(),
        MicroUsd(808),
        "settled the CI stage's resource-seconds into the SAME wallet (only the billed 192 drawn)"
    );
    assert_eq!(gate.inflight_interrupt_count(), 0, "0 in-flight interrupts");
    assert!(Meter::from_token("cpu_seconds").is_some());
}
