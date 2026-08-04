use myelin_ci_controlplane::ci_pipeline::run_ci_pipeline_body;
use myelin_ci_controlplane::{
    CheckFacts, CiResultSignal, PipelineRun, PipelineStage, RollupDelivery, RunVerdict,
    CI_PIPELINE_WF_TYPE,
};
use myelin_ci_sandbox::events::{CI_CHECK_UPDATED, CI_RESULT};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp as EvTs,
};
use myelin_flow::engine::SignalRow;
use myelin_flow::{
    job_idem_token, merge_attempt_id, stage_verdict_marker, ActivityError, CiDispatch,
    CiDispatcher, CiStage, MergeOutcome, MergeRequest, MicroUsd, SignalStore, TimerStore, WfCtx,
    WfJournal, JOB_DONE_SIGNAL,
};
use myelin_git::check_status::{
    CheckContext, CheckState as GitState, CheckStatus, CheckStatusConsumer, CheckStatusProjection,
    GitOid, TrustTier as GitTier,
};
use myelin_git::merge_gate::MergeGatePolicy;
use myelin_git::merge_queue::GitMergePerformer;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "deadbeefcafe";
const CI_RUN: &str = "run-7";
const MQ_RUN: &str = "R1";

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn minter() -> std::sync::Arc<dyn IdMinter> {
    std::sync::Arc::new(MonotonicMinter::new())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: EvTs("2026-06-23T00:00:00Z".into()),
        recorded_at: EvTs("2026-06-23T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:run-7".into())),
    }
}

#[derive(Default)]
struct RecordingRunner {
    calls: AtomicUsize,
}
impl myelin_flow::JobRunner for RecordingRunner {
    fn dispatch(&self, _spec: &myelin_flow::JobSpec) -> Result<(), ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingCi {
    calls: AtomicUsize,
    dispatched: Mutex<Vec<CiDispatch>>,
}
impl CiDispatcher for RecordingCi {
    fn dispatch(&self, ci: &CiDispatch) -> Result<(), ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.dispatched.lock().unwrap().push(ci.clone());
        Ok(())
    }
}

fn ci_run(merge_idem_token: &str, trust_tier: &str) -> PipelineRun {
    PipelineRun {
        stages: vec![
            PipelineStage::job(CiStage::new(
                "build",
                "pipeline://acme/ci/run-7#build",
                MicroUsd(0),
                Some(3600),
            )),
            PipelineStage::job(CiStage::new(
                "test",
                "pipeline://acme/ci/run-7#test",
                MicroUsd(0),
                Some(3600),
            )),
        ],
        contexts: vec!["build".into(), "test".into()],
        facts: CheckFacts {
            repo: REPO.into(),
            commit_oid: COMMIT.into(),
            run_ref: format!("myelin://acme/ci/run/{CI_RUN}"),
            run_attempt: 1,
            trust_tier: trust_tier.into(),
            merge_idem_token: merge_idem_token.into(),
        },
    }
}

fn stage_token(stage_idx: usize) -> String {
    job_idem_token(CI_RUN, &format!("{CI_PIPELINE_WF_TYPE}:{}", stage_idx * 2))
}

fn deliver_stage_done(signals: &SignalStore, token: &str, stage: &str, pass: bool) {
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: CI_RUN.into(),
        signal_name: JOB_DONE_SIGNAL.into(),
        idem_key: token.into(),
        payload: vec![stage_verdict_marker(stage, pass)],
        payload_key_ref: None,
        received_unix_ms: 0,
        consumed_seq: None,
    });
}

fn drive_ci_body(run: &PipelineRun, build_pass: bool, test_pass: bool) -> CiProducerFacts {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    deliver_stage_done(&signals, &stage_token(0), "build", build_pass);
    if build_pass {
        deliver_stage_done(&signals, &stage_token(1), "test", test_pass);
    }

    let mut ctx = WfCtx::begin(
        &outbox,
        minter(),
        WfJournal::new(),
        ctx_base(),
        CI_RUN,
        CI_PIPELINE_WF_TYPE,
        "2026-06-23T00:00:00Z",
        7,
    )
    .with_signals(signals)
    .with_timers(timers, 0, 1000);

    let verdict =
        run_ci_pipeline_body(&mut ctx, run, &runner).expect("the CI body runs to terminal");
    ctx.commit().expect("co-commit the body's producer facts");

    let mut check_payloads = Vec::new();
    let mut ci_result_payload = None;
    for r in outbox.committed_rows() {
        match r.envelope.type_.0.as_str() {
            CI_CHECK_UPDATED => check_payloads.push(r.envelope.payload.clone()),
            CI_RESULT => ci_result_payload = Some(r.envelope.payload.clone()),
            _ => {}
        }
    }
    CiProducerFacts {
        verdict,
        check_payloads,
        ci_result_payload,
    }
}

struct CiProducerFacts {
    verdict: RunVerdict,
    check_payloads: Vec<serde_json::Value>,
    ci_result_payload: Option<serde_json::Value>,
}

fn policy() -> MergeGatePolicy {
    MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap()
}

fn request() -> MergeRequest {
    MergeRequest {
        pr_ref: format!("{REPO}#pr-7"),
        target_ref: "refs/heads/main".into(),
        speculative_commit_oid: COMMIT.into(),
        required_contexts: vec!["ci/build".into(), "ci/test".into()],
    }
}

fn begin_merge_queue(outbox: &OutboxStore, signals: SignalStore) -> WfCtx {
    WfCtx::begin(
        outbox,
        minter(),
        WfJournal::new(),
        ctx_base(),
        MQ_RUN,
        "merge.queue",
        "2026-06-21T00:00:00Z",
        42,
    )
    .with_signals(signals)
}

#[test]
fn git_d10_ci_d8_full_seam_gate_from_ci_real_producer() {
    let attempt = merge_attempt_id(MQ_RUN, "merge.queue:0");
    let run = ci_run(&attempt, "trusted");
    let produced = drive_ci_body(&run, true, true);
    assert_eq!(
        produced.verdict,
        RunVerdict::Succeeded {
            stages_completed: 2
        },
        "CI's real body reached a green terminal verdict"
    );
    assert_eq!(
        produced.check_payloads.len(),
        2,
        "CI emitted one ci.check.updated per context (build + test)"
    );
    assert!(
        produced.ci_result_payload.is_some(),
        "CI emitted the ci.result BUS event (the carriage) too"
    );

    let mut proj = CheckStatusProjection::new();
    let build_fact = decode_real(&produced, "build");
    proj.apply(&bump_attempt(
        &build_fact,
        2,
        GitState::Success,
        GitTier::Trusted,
    ));
    proj.apply(&bump_attempt(
        &build_fact,
        1,
        GitState::Failure,
        GitTier::Trusted,
    ));
    proj.apply(&bump_attempt(
        &build_fact,
        2,
        GitState::Success,
        GitTier::Trusted,
    ));
    let test_fact = decode_real(&produced, "test");
    proj.apply(&bump_attempt(
        &test_fact,
        1,
        GitState::Success,
        GitTier::UntrustedFork,
    ));

    let build_key = key("build");
    let test_key = key("test");
    let build_row = proj.current(&build_key).expect("build has a current row");
    assert_eq!(build_row.run_attempt, 2, "the highest attempt SUPERSEDES");
    assert_eq!(
        build_row.state,
        GitState::Success,
        "the stale lower-attempt FAILURE was dropped (monotonic supersession)"
    );
    assert!(
        proj.current(&test_key).is_some(),
        "exactly 1 current row per key"
    );

    let mq_signals = SignalStore::new();
    let producer = CiResultSignal::new(&mq_signals, tenant(), region(), MQ_RUN);
    let current = current_verdicts(&proj, &[("ci/build", "build"), ("ci/test", "test")]);
    let required = vec!["ci/build".to_string(), "ci/test".to_string()];

    let first = producer.signal_ci_result(COMMIT, &current, &required, &attempt);
    let second = producer.signal_ci_result(COMMIT, &current, &required, &attempt);
    assert_eq!(
        first,
        RollupDelivery::Woke,
        "first ci.result delivery wakes"
    );
    assert_eq!(
        second,
        RollupDelivery::Duplicate,
        "(d) the at-least-once DOUBLE delivery is absorbed (wf_signal PK)"
    );
    assert_eq!(
        mq_signals.buffered_depth(),
        1,
        "ONE buffered ci.result row (woke ONCE)"
    );

    let outbox = OutboxStore::new();
    let ci = RecordingCi::default();
    let merges = Cell::new(0u32);
    {
        let merger = GitMergePerformer::new(&proj, GitOid(COMMIT.into()), policy(), vec![], |_r| {
            merges.set(merges.get() + 1);
            Ok("should-not-merge".into())
        });
        let mut wf = begin_merge_queue(&outbox, mq_signals.clone());
        let out = wf
            .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
            .expect("dispatch + dequeue");
        match out {
            MergeOutcome::Dequeued { reason } => {
                assert!(!reason.is_empty(), "a humanised dequeue reason");
                assert!(
                    !reason.contains("Blocked"),
                    "no raw gate struct in the reason: {reason}"
                );
            }
            other => panic!("(b) un-endorsed fork must DEQUEUE, got {other:?}"),
        }
        assert_eq!(
            merges.get(),
            0,
            "(b) fork self-green is NEUTRAL - merge-count == 0"
        );
    }

    let outbox2 = OutboxStore::new();
    let ci2 = RecordingCi::default();
    let merges2 = Cell::new(0u32);
    let mq_signals2 = SignalStore::new();
    let producer2 = CiResultSignal::new(&mq_signals2, tenant(), region(), MQ_RUN);
    producer2.signal_ci_result(COMMIT, &current, &required, &attempt);
    let dup = producer2.signal_ci_result(COMMIT, &current, &required, &attempt);
    assert_eq!(
        dup,
        RollupDelivery::Duplicate,
        "(d) double-delivery absorbed"
    );

    let merger = GitMergePerformer::new(
        &proj,
        GitOid(COMMIT.into()),
        policy(),
        vec![CheckContext::ci("test")],
        |r| {
            merges2.set(merges2.get() + 1);
            Ok(format!("merged-{}", r.speculative_commit_oid))
        },
    );
    let mut wf = begin_merge_queue(&outbox2, mq_signals2.clone());
    let out = wf
        .run_merge_attempt(&request(), &ci2, &merger, None, MicroUsd(0), vec![])
        .expect("dispatch + merge");
    match out {
        MergeOutcome::Merged {
            merge_attempt_id: id,
            merged_commit_oid,
        } => {
            assert_eq!(
                id, attempt,
                "CI echoed the no-coordination merge_attempt_id"
            );
            assert_eq!(merged_commit_oid, "merged-deadbeefcafe");
        }
        other => panic!("(c) endorsed fork must MERGE, got {other:?}"),
    }

    assert_eq!(
        merges2.get(),
        1,
        "(d) GIT-D10/CI-D8: 0 double-merge - merge-count == 1"
    );
    assert_eq!(
        wf.consumed_signals().len(),
        1,
        "the doubly-delivered ci.result woke the merge queue ONCE"
    );
    assert_eq!(wf.staged_emit_len(), 1, "EXACTLY one git.pr.merged emitted");
    assert_eq!(
        ci2.calls.load(Ordering::SeqCst),
        1,
        "the merge queue dispatched CI exactly once"
    );

    println!(
        "[2026-06-23] PASS  drill=GIT-D10/CI-D8  producer=CI-REAL(run_ci_pipeline_body)  \
         supersession=monotonic(1-current-row-per-key)  fork-self-green=NEUTRAL(merge-count==0)  \
         endorsed=GREEN  double-delivered-ci.result=wakes-once  merge-count==1  0-double-merge"
    );
}

#[test]
fn ci_d8_failure_rollup_dequeues_no_merge() {
    let attempt = merge_attempt_id(MQ_RUN, "merge.queue:0");
    let run = ci_run(&attempt, "trusted");
    let produced = drive_ci_body(&run, false, false);
    assert_eq!(
        produced.verdict,
        RunVerdict::Failed {
            stage: "build".into()
        },
        "CI's real body failed at build"
    );

    let mut proj = CheckStatusProjection::new();
    let build_fact = decode_real(&produced, "build");
    proj.apply(&bump_attempt(
        &build_fact,
        1,
        GitState::Failure,
        GitTier::Trusted,
    ));
    let test_fact = decode_real(&produced, "test");
    proj.apply(&bump_attempt(
        &test_fact,
        1,
        GitState::Failure,
        GitTier::Trusted,
    ));

    let mq_signals = SignalStore::new();
    let producer = CiResultSignal::new(&mq_signals, tenant(), region(), MQ_RUN);
    let current = current_verdicts(&proj, &[("ci/build", "build"), ("ci/test", "test")]);
    let required = vec!["ci/build".to_string(), "ci/test".to_string()];
    let out = producer.signal_ci_result(COMMIT, &current, &required, &attempt);
    assert_eq!(
        out,
        RollupDelivery::Woke,
        "the failure rollup wakes the queue"
    );

    let outbox = OutboxStore::new();
    let ci = RecordingCi::default();
    let merger = GitMergePerformer::new(&proj, GitOid(COMMIT.into()), policy(), vec![], |_r| {
        panic!("a CI failure must never merge")
    });
    let mut wf = begin_merge_queue(&outbox, mq_signals);
    let out = wf
        .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
        .expect("dispatch + dequeue");
    match out {
        MergeOutcome::Dequeued { reason } => {
            assert!(!reason.is_empty(), "humanised dequeue reason");
        }
        other => panic!("a CI failure must DEQUEUE, got {other:?}"),
    }
    assert_eq!(
        wf.staged_emit_len(),
        0,
        "no git.pr.merged on a failed CI run"
    );
}

fn decode_real(produced: &CiProducerFacts, context: &str) -> CheckStatus {
    for payload in &produced.check_payloads {
        if payload["context"]["name"] == context {
            return CheckStatusConsumer::decode(payload)
                .expect("CI's real producer payload decodes into Git's frozen 5.9 CheckStatus");
        }
    }
    panic!("CI did not produce a ci.check.updated for context `{context}`");
}

fn bump_attempt(base: &CheckStatus, attempt: u32, state: GitState, trust: GitTier) -> CheckStatus {
    let mut f = base.clone();
    f.run_attempt = attempt;
    f.state = state;
    f.trust_tier = trust;
    f.cost_settled = true;
    f.run = myelin_tenancy::ArtifactRef(format!("myelin://acme/ci/run/{attempt}"));
    f
}

fn key(context: &str) -> myelin_git::check_status::CheckKey {
    myelin_git::check_status::CheckKey {
        commit_oid: GitOid(COMMIT.into()),
        context: CheckContext::ci(context),
    }
}

fn current_verdicts(
    proj: &CheckStatusProjection,
    contexts: &[(&str, &str)],
) -> BTreeMap<String, bool> {
    let mut m = BTreeMap::new();
    for (qualified, projection_ctx) in contexts {
        let succeeded = proj
            .current(&key(projection_ctx))
            .map(|row| row.state == GitState::Success)
            .unwrap_or(false);
        m.insert((*qualified).to_string(), succeeded);
    }
    m
}
