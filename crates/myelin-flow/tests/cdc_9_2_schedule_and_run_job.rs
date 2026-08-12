use myelin_agent::{Command, ToolHands, ToolResult};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
};
use myelin_flow::engine::{SignalRow, SignalStore};
use myelin_flow::{
    job_idem_token, ActivityError, JobKind, JobOutcome, JobRunner, JobSpec, TimerStore, WfCtx,
    WfJournal, JOB_DONE_SIGNAL,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
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
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

struct UnifiedRunner<H: ToolHands> {
    hands: H,
    seen_tokens: Mutex<Vec<String>>,
    last_exec: Mutex<Option<ToolResult>>,
}
impl<H: ToolHands> UnifiedRunner<H> {
    fn new(hands: H) -> Self {
        Self {
            hands,
            seen_tokens: Mutex::new(Vec::new()),
            last_exec: Mutex::new(None),
        }
    }
}
impl<H: ToolHands> JobRunner for UnifiedRunner<H> {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), ActivityError> {
        self.seen_tokens
            .lock()
            .unwrap()
            .push(spec.idem_token.clone());
        let cmd = Command(format!("run {} target={}", spec.kind.as_str(), spec.target));
        let res = self.hands.exec(cmd);
        *self.last_exec.lock().unwrap() = Some(res);
        Ok(())
    }
}

struct SimHands;
impl ToolHands for SimHands {
    fn exec(&self, cmd: Command) -> ToolResult {
        ToolResult::Succeeded(format!("dispatched: {}", cmd.0))
    }
}

fn begin(outbox: &OutboxStore, journal: WfJournal, signals: SignalStore) -> WfCtx {
    WfCtx::begin(
        outbox,
        minter(),
        journal,
        ctx_base(),
        "R1",
        "merge.queue",
        "2026-06-21T00:00:00Z",
        42,
    )
    .with_signals(signals)
}

fn deliver_job_done(signals: &SignalStore, idem_token: &str, result: Vec<ArtifactRef>) {
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: "R1".into(),
        signal_name: JOB_DONE_SIGNAL.into(),
        idem_key: idem_token.into(),
        payload: result,
        payload_key_ref: None,
        received_unix_ms: 0,
        consumed_seq: None,
    });
}

#[test]
fn provider_dispatches_into_the_runner_and_parks_consumer_sees_the_token() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = UnifiedRunner::new(SimHands);

    let mut ctx = begin(&outbox, journal, signals).with_timers(timers.clone(), 0, 1_000);
    let out = ctx
        .schedule_and_run_job(
            JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
            &runner,
            Some(3600),
        )
        .expect("dispatch + park");

    assert_eq!(out, JobOutcome::Parked, "dispatched, parked on job.done");
    assert_eq!(
        timers.armed_count(),
        1,
        "the timed dispatch arms its durable SLA timer"
    );

    let consumer_token = job_idem_token("R1", "merge.queue:0");
    let seen = runner.seen_tokens.lock().unwrap();
    assert_eq!(
        seen.as_slice(),
        &[consumer_token],
        "the runner saw the engine's deterministic token"
    );

    let exec = runner.last_exec.lock().unwrap();
    assert_eq!(
        *exec,
        Some(ToolResult::Succeeded(
            "dispatched: run ci target=pipeline://acme/ci/pr-7".into()
        )),
        "the dispatch reached the unified runner's ToolHands::exec (contract 8.4 consumed)"
    );
}

#[test]
fn consumer_echoes_the_token_on_job_done_and_the_workflow_wakes_once() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let runner = UnifiedRunner::new(SimHands);

    let mut c1 = begin(&outbox, journal.clone(), signals.clone());
    assert_eq!(
        c1.schedule_and_run_job(
            JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
            &runner,
            None
        )
        .expect("dispatch"),
        JobOutcome::Parked
    );
    c1.commit().expect("co-commit the dispatch + park");
    let history = journal.history_for(&tenant(), "R1");

    let token = job_idem_token("R1", "merge.queue:0");
    let result = vec![ArtifactRef("myelin://acme/ci/result/green".into())];
    deliver_job_done(&signals, &token, result.clone());
    deliver_job_done(&signals, &token, result.clone());
    assert_eq!(
        signals.buffered_depth(),
        1,
        "the double delivery deduped to ONE row (wf_signal PK)"
    );

    let mut c2 = WfCtx::resume(
        &outbox,
        minter(),
        journal.clone(),
        ctx_base(),
        "R1",
        "merge.queue",
        "2026-06-21T00:00:00Z",
        42,
        history,
    )
    .with_signals(signals.clone());
    let out = c2
        .schedule_and_run_job(
            JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
            &runner,
            None,
        )
        .expect("resume + complete");
    match out {
        JobOutcome::Completed {
            idem_token,
            result: got,
        } => {
            assert_eq!(
                idem_token, token,
                "the runner echoed the engine's token (agreement held)"
            );
            assert_eq!(got, result, "the job's references-not-payloads result");
        }
        other => panic!("expected Completed, got {other:?}"),
    }
    assert_eq!(
        c2.consumed_signals().len(),
        1,
        "ONE wake per job (the double delivery deduped)"
    );
    assert_eq!(
        signals.buffered_depth(),
        0,
        "the one buffered job.done is consumed once"
    );
}
