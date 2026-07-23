//! # CI-P19 / P-362 — the X-1 `ci.result` rollup + the GIT-D10 / CI-D8 check-seam end-to-end GATE
//!
//! **The joint Git+CI seam gate, driven by CI's REAL producer (the seam-floor closer).** The Git-side
//! `e2e_git_p23_merge_queue.rs` drove the merge queue against a SYNTHETIC `ci.result` producer
//! (`myelin_flow::MockCiResultProducer` — a named seam-floor) and the flow-crate's `RealCiResultProducer`
//! re-derives the rollup from fabricated `CheckFact`s. THIS drill closes the seam END-TO-END from CI's
//! ACTUAL producer: it drives CI's REAL `ci.pipeline` body ([`myelin_ci_controlplane::run_ci_pipeline_body`],
//! the deterministic durable-workflow body) to PRODUCE the per-context `ci.check.updated` facts, feeds
//! them into Git's `check_status` projection (proving the `run_attempt`-monotonic supersession), and
//! delivers CI's REAL `ci.result` rollup SIGNAL ([`myelin_ci_controlplane::CiResultSignal`]) into Git's
//! merge-queue durable workflow ([`myelin_flow::WfCtx::run_merge_attempt`]).
//!
//! **The full GIT-D10 / CI-D8 aggregate (the quantified gate):**
//! - **(a) supersession** — out-of-order/dup `ci.check.updated` → `run_attempt`-monotonic supersession
//!   holds in Git's projection (a lower attempt DROPPED, a higher SUPERSEDES; EXACTLY 1 current row per
//!   `(commit_oid, context)`);
//! - **(b) fork self-green → NEUTRAL** — a fork PR's `untrusted_fork` success rolls up `overall: success`
//!   but the merge gate (over Git's projection) BLOCKS it → the PR is DEQUEUED (merge-count == 0);
//! - **(c) maintainer endorses → green** — with the fork context endorsed, the gate ADMITS → MERGE;
//! - **(d) doubly-delivered `ci.result` → the merge-queue workflow wakes EXACTLY ONCE; 0 double-merge**
//!   (merge-count == 1 — the dated green artifact).
//!
//! **The CDC pair for contract 5.9 (the rollup-signal half).** This drill IS the provider+consumer
//! pair for the X-1 `ci.result` rollup seam. The PROVIDER side is CI's
//! `myelin_ci_controlplane::CiResultSignal`, which derives and delivers the rollup signal (the
//! producer half). The CONSUMER side is Git's merge-queue durable workflow,
//! `myelin_flow::WfCtx::run_merge_attempt` over `myelin_git::merge_queue::GitMergePerformer`, the gate
//! half that consumes the rollup signal and decides the merge. The provider emits the fact; the
//! consumer gates — proven byte-for-byte through the SAME references-not-payloads codec, end-to-end.
//!
//! **Contracts:** OWNED (the provider/producer half, the rollup) 5.9 (the `ci.result` rollup signal — CI
//! is the PRODUCER). CONSUMED 2.9 (the token), 9.4 (the durable `ci.result` wait — Git's consumer side).
//! The seam is implemented to the frozen 5.9 shape EXACTLY (no local divergence). Owning architecture:
//! `continuous-integration/architecture/02-internals-and-algorithms.md` §4 step 4. Reconciliation: X-1.

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
    CiDispatcher, CiStage, MergeOutcome, MergeRequest, MinorUnits, SignalStore, TimerStore, WfCtx,
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

/// A recording CI runner fixture (the contract-8.4 `ToolHands::exec` consumer side, AG-D4-gated).
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

/// A CI-dispatch fixture for the merge-queue's `SCHEDULE_AND_RUN_JOB` dispatch (counts dispatches).
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

/// A two-context CI run (`build` + `test`) with the run's X-1 emit facts. The `merge_idem_token` is the
/// merge-attempt id the merge queue minted (the no-coordination dedup key, OQ-F) — CI echoes it on the
/// rollup so a double-delivered `ci.result` wakes the merge queue ONCE.
fn ci_run(merge_idem_token: &str, trust_tier: &str) -> PipelineRun {
    PipelineRun {
        stages: vec![
            PipelineStage::job(CiStage::new(
                "build",
                "pipeline://acme/ci/run-7#build",
                MinorUnits(0),
                Some(3600),
            )),
            PipelineStage::job(CiStage::new(
                "test",
                "pipeline://acme/ci/run-7#test",
                MinorUnits(0),
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

/// The dispatch `idem_token` for the Nth CI runner stage (each stage = 2 command positions).
fn stage_token(stage_idx: usize) -> String {
    job_idem_token(CI_RUN, &format!("{CI_PIPELINE_WF_TYPE}:{}", stage_idx * 2))
}

/// Deliver a CI runner stage's `job.done` carrying the verdict marker (the stage passed/failed).
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

/// **Drive CI's REAL `ci.pipeline` body to terminal and return its committed `ci.check.updated`
/// payloads (the per-context X-1 producer facts) + the `ci.result` BUS event payload.** Both stages'
/// `job.done` are buffered green/red so one drive runs the whole pipeline; the producer facts are
/// harvested off the committed outbox — this is CI's ACTUAL producer, not a fixture.
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

/// CI's harvested REAL producer facts (off the committed outbox): the run verdict, the per-context
/// `ci.check.updated` payloads, and the `ci.result` BUS-event payload.
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

/// Begin Git's merge-queue durable workflow WfCtx, pre-loaded with the signal store CI delivers into.
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

/// **GIT-D10 / CI-D8 — the FULL aggregate, driven by CI's REAL producer end-to-end (the dated green
/// artifact: 1 current row/key, merge-count == 1, fork-success-neutral, monotonic supersession).**
///
/// (a) CI's REAL body produces the `build`+`test` success facts; out-of-order/dup delivery into Git's
/// projection supersedes monotonically (1 current row per key). (b) the `test` context was a fork run
/// (untrusted) → it is NEUTRAL for gating until (c) a maintainer endorses it. CI's REAL `ci.result`
/// rollup signal is delivered TWICE (at-least-once) → (d) the merge-queue workflow wakes EXACTLY ONCE,
/// merges EXACTLY ONCE (0 double-merge).
#[test]
fn git_d10_ci_d8_full_seam_gate_from_ci_real_producer() {
    // ── CI's REAL producer: drive the actual ci.pipeline body to a GREEN terminal verdict.
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

    // ── (a) SUPERSESSION: feed CI's REAL facts into Git's projection OUT-OF-ORDER + DUPLICATED, plus a
    // stale LOWER-attempt re-delivery (the at-least-once transport). Each context collapses to exactly
    // one CURRENT row at the highest run_attempt.
    let mut proj = CheckStatusProjection::new();
    // build: a stale FAILURE at attempt 1 (the old run), then CI's real SUCCESS — but delivered
    // out-of-order (the higher attempt FIRST, the stale lower attempt LATE) + duplicated.
    let build_fact = decode_real(&produced, "build");
    proj.apply(&bump_attempt(
        &build_fact,
        2,
        GitState::Success,
        GitTier::Trusted,
    )); // attempt 2 first
    proj.apply(&bump_attempt(
        &build_fact,
        1,
        GitState::Failure,
        GitTier::Trusted,
    )); // stale lower → DROPPED
    proj.apply(&bump_attempt(
        &build_fact,
        2,
        GitState::Success,
        GitTier::Trusted,
    )); // duplicate → idempotent
        // test: CI's real success, but the run was an untrusted FORK (the trust_tier rides the fact).
    let test_fact = decode_real(&produced, "test");
    proj.apply(&bump_attempt(
        &test_fact,
        1,
        GitState::Success,
        GitTier::UntrustedFork,
    ));

    // EXACTLY 1 current row per (commit_oid, context); build's current is the highest-attempt success.
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

    // ── CI's REAL rollup SIGNAL: derive over Git's required set + DELIVER TWICE (at-least-once). The
    // rollup keys on Git's QUALIFIED required-context vocabulary (`ci/build`/`ci/test`, the merge
    // request's required_contexts) — the same names the merge-queue reconciles against (§6.5).
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

    // ── (b)→(c): the merge gate over Git's projection. `test` is an un-endorsed fork → BLOCK first.
    let outbox = OutboxStore::new();
    let ci = RecordingCi::default();
    let merges = Cell::new(0u32);
    {
        // (b) NOT endorsed: the fork's `test` success is NEUTRAL → the gate BLOCKS → DEQUEUE.
        let merger = GitMergePerformer::new(&proj, GitOid(COMMIT.into()), policy(), vec![], |_r| {
            merges.set(merges.get() + 1);
            Ok("should-not-merge".into())
        });
        let mut wf = begin_merge_queue(&outbox, mq_signals.clone());
        let out = wf
            .run_merge_attempt(&request(), &ci, &merger, None, MinorUnits(0), vec![])
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
            "(b) fork self-green is NEUTRAL — merge-count == 0"
        );
    }

    // ── (c) the maintainer ENDORSES the fork's `test` run + (d) the rollup is delivered TWICE again;
    // the gate ADMITS → MERGE EXACTLY ONCE (0 double-merge).
    let outbox2 = OutboxStore::new();
    let ci2 = RecordingCi::default();
    let merges2 = Cell::new(0u32);
    let mq_signals2 = SignalStore::new();
    let producer2 = CiResultSignal::new(&mq_signals2, tenant(), region(), MQ_RUN);
    // CI re-delivers its REAL rollup (same idem_token) TWICE — the at-least-once double delivery.
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
        vec![CheckContext::ci("test")], // the maintainer endorsed the fork's `test` run
        |r| {
            merges2.set(merges2.get() + 1);
            Ok(format!("merged-{}", r.speculative_commit_oid))
        },
    );
    let mut wf = begin_merge_queue(&outbox2, mq_signals2.clone());
    let out = wf
        .run_merge_attempt(&request(), &ci2, &merger, None, MinorUnits(0), vec![])
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

    // ── THE DATED GREEN ARTIFACT ──
    assert_eq!(
        merges2.get(),
        1,
        "(d) GIT-D10/CI-D8: 0 double-merge — merge-count == 1"
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

/// **A CI `failure` rollup (a required context failed) DEQUEUES with a humanised reason — no merge.**
/// CI's REAL body fails at `build`; its rollup is `overall: failure`; the merge queue dequeues (CI
/// reports the fact, Git gates — CI never merges).
#[test]
fn ci_d8_failure_rollup_dequeues_no_merge() {
    let attempt = merge_attempt_id(MQ_RUN, "merge.queue:0");
    let run = ci_run(&attempt, "trusted");
    // build FAILS → the body fails fast (test never dispatched), and emits failure facts per context.
    let produced = drive_ci_body(&run, false, false);
    assert_eq!(
        produced.verdict,
        RunVerdict::Failed {
            stage: "build".into()
        },
        "CI's real body failed at build"
    );

    // Git's projection: build is a FAILURE (CI's real fact).
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

    // CI's REAL rollup: overall failure (a required context failed).
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
        .run_merge_attempt(&request(), &ci, &merger, None, MinorUnits(0), vec![])
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

// ---------------------------------------------------------------------------
// Helpers: read CI's REAL producer facts into Git's typed CheckStatus
// ---------------------------------------------------------------------------

/// Decode the named context's CI-produced `ci.check.updated` payload into Git's frozen `CheckStatus`
/// (the REAL consumer decode — the seam's no-drift property). The body emits `cost_settled: false`
/// (terminal-but-not-settled); the test re-stamps attempt/state/trust for the supersession scenarios.
fn decode_real(produced: &CiProducerFacts, context: &str) -> CheckStatus {
    for payload in &produced.check_payloads {
        if payload["context"]["name"] == context {
            return CheckStatusConsumer::decode(payload)
                .expect("CI's real producer payload decodes into Git's frozen 5.9 CheckStatus");
        }
    }
    panic!("CI did not produce a ci.check.updated for context `{context}`");
}

/// Re-stamp a decoded terminal fact for a supersession scenario. The workflow producer emits its
/// terminal fact before the external accounting bookend, so the fixture must model the later settled
/// projection explicitly; an unsettled success is correctly neutral at the merge gate.
fn bump_attempt(base: &CheckStatus, attempt: u32, state: GitState, trust: GitTier) -> CheckStatus {
    let mut f = base.clone();
    f.run_attempt = attempt;
    f.state = state;
    f.trust_tier = trust;
    f.cost_settled = true;
    f.run = myelin_tenancy::ArtifactRef(format!("myelin://acme/ci/run/{attempt}"));
    f
}

/// The `(commit_oid, context)` projection key for a context name.
fn key(context: &str) -> myelin_git::check_status::CheckKey {
    myelin_git::check_status::CheckKey {
        commit_oid: GitOid(COMMIT.into()),
        context: CheckContext::ci(context),
    }
}

/// The CURRENT per-context verdicts off Git's projection (did each context's current row succeed?) —
/// the post-supersession truth CI rolls up over the required set. Each entry is `(qualified_name,
/// projection_context)`: the rollup keys on Git's QUALIFIED required-context vocabulary
/// (`ci/build`/`ci/test`, the merge request's required_contexts) while reading the verdict off Git's
/// projection keyed on the bare context (`build`/`test`).
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
