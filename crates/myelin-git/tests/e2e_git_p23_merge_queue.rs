use myelin_events::{
    Actor, CausedBy, CiOverall, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore,
    Timestamp as EvTs,
};
use myelin_flow::{
    merge_attempt_id, ActivityError, CiDispatch, CiDispatcher, MergeOutcome, MergeRequest,
    MockCiResultProducer, SignalStore, WfCtx, WfJournal,
};
use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use myelin_git::merge_gate::MergeGatePolicy;
use myelin_git::merge_queue::GitMergePerformer;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::reserve_settle::MicroUsd;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const HEAD: &str = "deadbeefcafe";
const REPO: &str = "myelin://acme/git/repo/core";
const RUN: &str = "R1";

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn fact(context: &str, attempt: u32, state: CheckState, trust: TrustTier) -> CheckStatus {
    CheckStatus {
        tenant: tenant(),
        repo: ArtifactRef(REPO.into()),
        commit_oid: GitOid(HEAD.into()),
        context: CheckContext::ci(context),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://acme/ci/run/{attempt}")),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: ArtifactRef(format!("myelin://acme/ci/run/{attempt}#step-2")),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: Default::default(),
        },
        started_at: Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
        cost_settled: true,
    }
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
        occurred_at: EvTs("2026-06-21T00:00:00Z".into()),
        recorded_at: EvTs("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn minter() -> std::sync::Arc<dyn IdMinter> {
    std::sync::Arc::new(MonotonicMinter::new())
}

fn request() -> MergeRequest {
    MergeRequest {
        pr_ref: format!("{REPO}#pr-7"),
        target_ref: "refs/heads/main".into(),
        speculative_commit_oid: HEAD.into(),
        required_contexts: vec!["ci/build".into(), "ci/test".into()],
    }
}

fn policy() -> MergeGatePolicy {
    MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap()
}

#[derive(Default)]
struct RecordingCi {
    dispatched: Mutex<Vec<CiDispatch>>,
    calls: AtomicUsize,
}
impl CiDispatcher for RecordingCi {
    fn dispatch(&self, ci: &CiDispatch) -> Result<(), ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.dispatched.lock().unwrap().push(ci.clone());
        Ok(())
    }
}

#[test]
fn git_d10_full_aggregate_doubly_delivered_ci_result_merges_exactly_once() {
    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 2, CheckState::Success, TrustTier::Trusted));
    proj.apply(&fact("build", 1, CheckState::Failure, TrustTier::Trusted));
    proj.apply(&fact("build", 2, CheckState::Success, TrustTier::Trusted));
    proj.apply(&fact(
        "test",
        1,
        CheckState::Success,
        TrustTier::UntrustedFork,
    ));
    let endorsed = vec![CheckContext::ci("test")];

    let signals = SignalStore::new();
    let producer = MockCiResultProducer::new(&signals, tenant(), region(), RUN);
    let attempt = merge_attempt_id(RUN, "merge.queue:0");
    let first = producer.deliver(
        &attempt,
        HEAD,
        CiOverall::Success,
        vec!["ci/build".into(), "ci/test".into()],
    );
    let second = producer.deliver(
        &attempt,
        HEAD,
        CiOverall::Success,
        vec!["ci/build".into(), "ci/test".into()],
    );
    assert!(first, "first ci.result delivery is new");
    assert!(
        !second,
        "the at-least-once DOUBLE delivery deduped (wf_signal PK)"
    );
    assert_eq!(
        signals.buffered_depth(),
        1,
        "ONE buffered ci.result row (woke ONCE)"
    );

    let merges = Cell::new(0u32);
    let merger = GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), endorsed, |r| {
        merges.set(merges.get() + 1);
        Ok(format!("merged-{}", r.speculative_commit_oid))
    });

    let outbox = OutboxStore::new();
    let ci = RecordingCi::default();
    let mut wf = WfCtx::begin(
        &outbox,
        minter(),
        WfJournal::new(),
        ctx_base(),
        RUN,
        "merge.queue",
        "2026-06-21T00:00:00Z",
        42,
    )
    .with_signals(signals);

    let out = wf
        .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
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
        other => panic!("expected Merged, got {other:?}"),
    }
    assert_eq!(
        merges.get(),
        1,
        "GIT-D10 (d): 0 double-merge - merge-count == 1"
    );
    assert_eq!(
        wf.consumed_signals().len(),
        1,
        "the doubly-delivered ci.result woke the workflow ONCE"
    );
    assert_eq!(wf.staged_emit_len(), 1, "EXACTLY one git.pr.merged emitted");
    assert_eq!(
        ci.calls.load(Ordering::SeqCst),
        1,
        "CI dispatched exactly once"
    );
}

#[test]
fn git_d10_b_fork_self_green_is_neutral_dequeues() {
    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
    proj.apply(&fact(
        "test",
        1,
        CheckState::Success,
        TrustTier::UntrustedFork,
    ));

    let signals = SignalStore::new();
    let producer = MockCiResultProducer::new(&signals, tenant(), region(), RUN);
    let attempt = merge_attempt_id(RUN, "merge.queue:0");
    producer.deliver(
        &attempt,
        HEAD,
        CiOverall::Success,
        vec!["ci/build".into(), "ci/test".into()],
    );

    let merges = Cell::new(0u32);
    let merger = GitMergePerformer::new(
        &proj,
        GitOid(HEAD.into()),
        policy(),
        vec![],
        |_r| {
            merges.set(merges.get() + 1);
            Ok("should-not-merge".into())
        },
    );

    let outbox = OutboxStore::new();
    let ci = RecordingCi::default();
    let mut wf = WfCtx::begin(
        &outbox,
        minter(),
        WfJournal::new(),
        ctx_base(),
        RUN,
        "merge.queue",
        "2026-06-21T00:00:00Z",
        42,
    )
    .with_signals(signals);

    let out = wf
        .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
        .expect("dispatch + dequeue");

    match out {
        MergeOutcome::Dequeued { reason } => {
            assert!(!reason.is_empty(), "humanised dequeue reason");
            assert!(
                !reason.contains("Blocked"),
                "no raw gate struct in the reason: {reason}"
            );
        }
        other => panic!("expected Dequeued (a fork self-green must not merge), got {other:?}"),
    }
    assert_eq!(
        merges.get(),
        0,
        "GIT-D10 (b): 0 forks self-green their gate - merge-count == 0"
    );
    assert_eq!(
        wf.staged_emit_len(),
        0,
        "no git.pr.merged on a refused fork merge"
    );
}

#[test]
fn git_d10_c_maintainer_endorsement_flips_the_gate_green() {
    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
    proj.apply(&fact(
        "test",
        1,
        CheckState::Success,
        TrustTier::UntrustedFork,
    ));

    let signals = SignalStore::new();
    let producer = MockCiResultProducer::new(&signals, tenant(), region(), RUN);
    let attempt = merge_attempt_id(RUN, "merge.queue:0");
    producer.deliver(
        &attempt,
        HEAD,
        CiOverall::Success,
        vec!["ci/build".into(), "ci/test".into()],
    );

    let merges = Cell::new(0u32);
    let merger = GitMergePerformer::new(
        &proj,
        GitOid(HEAD.into()),
        policy(),
        vec![CheckContext::ci("test")],
        |_r| {
            merges.set(merges.get() + 1);
            Ok("merged".into())
        },
    );

    let outbox = OutboxStore::new();
    let ci = RecordingCi::default();
    let mut wf = WfCtx::begin(
        &outbox,
        minter(),
        WfJournal::new(),
        ctx_base(),
        RUN,
        "merge.queue",
        "2026-06-21T00:00:00Z",
        42,
    )
    .with_signals(signals);

    let out = wf
        .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
        .expect("dispatch + merge");
    assert!(
        matches!(out, MergeOutcome::Merged { .. }),
        "endorsed fork → merge, got {out:?}"
    );
    assert_eq!(
        merges.get(),
        1,
        "GIT-D10 (c): the endorsement flips green - merge-count == 1"
    );
}

#[test]
fn no_cross_sync_cycle_lint_green_over_the_merge_queue() {
    let src = include_str!("../src/merge_queue.rs");
    let lint = myelin_lints::no_cross_sync_cycle();
    let violations = lint.run(src);
    assert!(
        violations.is_empty(),
        "Git's merge queue makes a synchronous cross-subsystem call (it must read its OWN projection, \
         never synchronously call CI): {violations:?}"
    );
    assert!(
        src.contains("CheckStatusProjection"),
        "the merge queue reads Git's own check_status projection"
    );
}
