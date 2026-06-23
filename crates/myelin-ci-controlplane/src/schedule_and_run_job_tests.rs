//! Unit tests for the `SCHEDULE_AND_RUN_JOB` dispatch handshake (CI-P16 → P-359, M4).
//!
//! These prove CI's HALF of the frozen handshake (arch 02 §3.3): the dispatch ENQUEUES into
//! `job_queue` idempotently on the engine-minted `idem_token`, the scheduling terms ride the row, and
//! the terminal `job.done` completion is idempotent. The engine half (the deterministic `idem_token`
//! mint, the park, the `wf_signal` PK dedup) is proven in `myelin-flow/src/job.rs`; the END-TO-END
//! effectively-once drill (kill runner + control plane mid-run) is `tests/drills_ci_p16_*.rs`.

use super::*;
use crate::scheduler::{ClaimRequest, JobState, Lane, SchedulerState, TrustTier};
use myelin_flow::{JobKind, JobRunner, JobSpec};

fn terms() -> JobScheduleTerms {
    JobScheduleTerms::new(
        "acme",
        "fr-par",
        "run-pr-7",
        Lane::Interactive,
        TrustTier::Trusted,
        "acme",
    )
}

fn runner() -> SchedulerJobRunner {
    SchedulerJobRunner::new(
        std::sync::Arc::new(std::sync::Mutex::new(SchedulerState::new())),
        terms(),
    )
}

/// A `JobSpec` as the engine hands it to the runner: the deterministic `idem_token` already stamped.
fn dispatched_spec(idem_token: &str, target: &str) -> JobSpec {
    JobSpec {
        kind: JobKind::Ci,
        target: target.into(),
        idem_token: idem_token.into(),
    }
}

/// **A dispatch ENQUEUES one `job_queue` row with the run's scheduling terms (arch §3.3 step 1).** The
/// engine-minted `idem_token` is BOTH the `job_id` and the `jq_idem` key; the lane/trust/fair-key ride
/// the row (the claim orders/filters on them).
#[test]
fn dispatch_enqueues_one_job_queue_row_with_the_scheduling_terms() {
    let r = runner();
    r.dispatch(&dispatched_spec(
        "run-pr-7/ci.pipeline:0/job",
        "pipeline://acme/ci/pr-7#build",
    ))
    .expect("the dispatch enqueues");

    let sched = r.scheduler().lock().unwrap();
    let jobs = sched.jobs();
    assert_eq!(jobs.len(), 1, "ONE job enqueued");
    let job = &jobs[0];
    assert_eq!(
        job.job_id, "run-pr-7/ci.pipeline:0/job",
        "the engine-minted idem_token IS the job id"
    );
    assert_eq!(
        job.idem_token, "run-pr-7/ci.pipeline:0/job",
        "... and the jq_idem idempotency key"
    );
    assert_eq!(job.lane, Lane::Interactive, "the run's lane rides the row");
    assert_eq!(
        job.trust_tier,
        TrustTier::Trusted,
        "the trust tier stamped at trigger time rides the row (X-1, never recomputed)"
    );
    assert_eq!(job.fair_key, "acme", "the DRR fair-key rides the row");
    assert_eq!(job.state, JobState::Queued, "the job is claimable");
}

/// **The effectively-once floor: a RE-DISPATCH on the SAME `idem_token` is ONE row (the `jq_idem`
/// unique, arch §3.3 step 4 / CI-D1).** A control-plane replay re-derives the SAME deterministic
/// `idem_token` and re-dispatches; the enqueue is idempotent → still exactly one job (0 double-run).
#[test]
fn a_re_dispatch_on_the_same_idem_token_is_one_row() {
    let r = runner();
    let spec = dispatched_spec(
        "run-pr-7/ci.pipeline:0/job",
        "pipeline://acme/ci/pr-7#build",
    );

    // First dispatch: inserts.
    r.dispatch(&spec).expect("dispatch 1");
    // Control-plane replay re-dispatches the SAME idem_token (the engine re-derived it).
    r.dispatch(&spec).expect("dispatch 2 (replay)");
    // A redundant third (the activity retry under at-least-once).
    r.dispatch(&spec).expect("dispatch 3 (retry)");

    let sched = r.scheduler().lock().unwrap();
    assert_eq!(
        sched.jobs().len(),
        1,
        "three dispatches on the SAME idem_token = ONE job_queue row (effectively-once, CI-D1)"
    );
}

/// **A reaper RE-QUEUE + a redundant `SCHEDULE_AND_RUN_JOB` re-dispatch is ONE row (arch §3.3 step
/// 4).** The runner died mid-job (its lease expired); the reaper re-queues the row; the workflow (on
/// resume) redundantly re-dispatches the SAME `idem_token` — the `jq_idem` unique collapses it. The
/// job is claimable exactly once (0 duplicate publish).
#[test]
fn a_reaper_requeue_plus_a_re_dispatch_is_one_row() {
    let r = runner();
    let spec = dispatched_spec(
        "run-pr-7/ci.pipeline:0/job",
        "pipeline://acme/ci/pr-7#build",
    );
    r.dispatch(&spec).expect("dispatch");

    {
        let mut sched = r.scheduler().lock().unwrap();
        // A runner claims + leases the job, then DIES (the lease expires).
        let claim = ClaimRequest {
            cell_region: "fr-par".into(),
            runner_labels: vec![],
            runner_allowed_tiers: vec![TrustTier::Trusted],
            lease_owner: "runner-1".into(),
            lease_ttl: 10,
        };
        let claimed = sched.claim(&claim).expect("the job claims");
        assert_eq!(claimed.job_id, "run-pr-7/ci.pipeline:0/job");
        // The runner dies: advance past the lease + reap.
        sched.advance(20);
        let reaped = sched.reap();
        assert_eq!(reaped.len(), 1, "the dead runner's lease was reaped");
        assert_eq!(
            sched.state_of("acme", "run-pr-7/ci.pipeline:0/job"),
            Some(JobState::Queued),
            "the reaped job is claimable again (ONE row, re-queued in place)"
        );
    }

    // The workflow resumes and redundantly re-dispatches the SAME idem_token.
    r.dispatch(&spec).expect("the resume re-dispatch");
    let sched = r.scheduler().lock().unwrap();
    assert_eq!(
        sched.jobs().len(),
        1,
        "reaper re-queue + re-dispatch = ONE row (the job runs once, never twice — CI-D1)"
    );
}

/// **The terminal `job.done` completion is IDEMPOTENT (arch §3.3 step 3 / CI-D1).** The runner reports
/// `job.done`; `complete_job` moves the row to `terminal` ONCE. A double-delivered `job.done`
/// (at-least-once) re-completes a no-op → the row terminates once (0 double-effect).
#[test]
fn the_terminal_job_done_completion_is_idempotent() {
    let r = runner();
    let spec = dispatched_spec(
        "run-pr-7/ci.pipeline:0/job",
        "pipeline://acme/ci/pr-7#build",
    );
    r.dispatch(&spec).expect("dispatch");

    // First job.done: terminates the row.
    let first =
        complete_job(r.scheduler(), "acme", "run-pr-7/ci.pipeline:0/job").expect("complete");
    assert!(first, "the first job.done moves the row to terminal");
    // Double-delivered job.done (at-least-once): a no-op.
    let second =
        complete_job(r.scheduler(), "acme", "run-pr-7/ci.pipeline:0/job").expect("re-complete");
    assert!(
        !second,
        "a double-delivered job.done re-completes a no-op (the row terminates ONCE — CI-D1)"
    );

    let sched = r.scheduler().lock().unwrap();
    assert_eq!(
        sched.state_of("acme", "run-pr-7/ci.pipeline:0/job"),
        Some(JobState::Terminal),
        "the job is terminal (the reaper never re-queues a completed job)"
    );
}

/// **A completed job is NEVER re-queued by the reaper (the `job.done` ↔ reaper interplay, CI-D1).** A
/// job that completed (`job.done` delivered) is `terminal`; even with an expired lease cleared, the
/// reaper does not touch a terminal row — so a completed job's effect is never re-run.
#[test]
fn a_completed_job_is_never_re_queued_by_the_reaper() {
    let r = runner();
    let spec = dispatched_spec(
        "run-pr-7/ci.pipeline:0/job",
        "pipeline://acme/ci/pr-7#build",
    );
    r.dispatch(&spec).expect("dispatch");

    let mut sched = r.scheduler().lock().unwrap();
    let claim = ClaimRequest {
        cell_region: "fr-par".into(),
        runner_labels: vec![],
        runner_allowed_tiers: vec![TrustTier::Trusted],
        lease_owner: "runner-1".into(),
        lease_ttl: 10,
    };
    sched.claim(&claim).expect("claim");
    // The runner COMPLETES the job (job.done), then we advance past where its lease WOULD have expired.
    assert!(sched.complete_job("acme", "run-pr-7/ci.pipeline:0/job"));
    sched.advance(100);
    let reaped = sched.reap();
    assert!(
        reaped.is_empty(),
        "the reaper does NOT re-queue a COMPLETED job (0 double-run on a finished job — CI-D1)"
    );
    assert_eq!(
        sched.state_of("acme", "run-pr-7/ci.pipeline:0/job"),
        Some(JobState::Terminal),
        "the completed job stays terminal"
    );
}

/// **The scheduling terms derive from the snapshot (the labels + concurrency group ride the row).** A
/// `deploy:%` concurrency group + affinity labels are stamped on the enqueued row (the claim
/// serializes deploys + filters affinity on them).
#[test]
fn the_snapshot_scheduling_terms_ride_the_enqueued_row() {
    let terms = JobScheduleTerms::new(
        "acme",
        "fr-par",
        "run-deploy-1",
        Lane::Deploy,
        TrustTier::Trusted,
        "acme:web",
    )
    .with_labels(["linux", "amd64"])
    .with_concurrency_group("deploy:prod");
    let r = SchedulerJobRunner::new(
        std::sync::Arc::new(std::sync::Mutex::new(SchedulerState::new())),
        terms,
    );
    r.dispatch(&dispatched_spec(
        "run-deploy-1/ci.pipeline:0/job",
        "pipeline://acme/ci/deploy-1#deploy",
    ))
    .expect("dispatch");

    let sched = r.scheduler().lock().unwrap();
    let job = &sched.jobs()[0];
    assert_eq!(job.lane, Lane::Deploy);
    assert_eq!(job.labels, vec!["linux".to_string(), "amd64".to_string()]);
    assert_eq!(job.concurrency_group.as_deref(), Some("deploy:prod"));
    assert_eq!(job.fair_key, "acme:web");
}

/// **A `pr:%` concurrency group enqueues SUPERSEDING (the prior head is cancelled, arch §2.3).** Two
/// PR heads with the same `pr:web:42` group: the second supersedes the first (only the latest head is
/// tested). Distinct `idem_token`s (distinct dispatch positions), so they are distinct rows; the
/// supersession cancels the OLD one.
#[test]
fn a_pr_group_enqueues_superseding_the_prior_head() {
    let scheduler = std::sync::Arc::new(std::sync::Mutex::new(SchedulerState::new()));
    let terms_head1 = JobScheduleTerms::new(
        "acme",
        "fr-par",
        "run-pr-42-head1",
        Lane::Interactive,
        TrustTier::Trusted,
        "acme",
    )
    .with_concurrency_group("pr:web:42");
    let terms_head2 = JobScheduleTerms::new(
        "acme",
        "fr-par",
        "run-pr-42-head2",
        Lane::Interactive,
        TrustTier::Trusted,
        "acme",
    )
    .with_concurrency_group("pr:web:42");

    let r1 = SchedulerJobRunner::new(scheduler.clone(), terms_head1);
    let r2 = SchedulerJobRunner::new(scheduler.clone(), terms_head2);

    r1.dispatch(&dispatched_spec(
        "run-pr-42-head1/ci.pipeline:0/job",
        "pipeline://acme/ci/pr-42#a",
    ))
    .expect("head1 dispatch");
    r2.dispatch(&dispatched_spec(
        "run-pr-42-head2/ci.pipeline:0/job",
        "pipeline://acme/ci/pr-42#b",
    ))
    .expect("head2 dispatch (supersedes head1)");

    let sched = scheduler.lock().unwrap();
    assert_eq!(
        sched.state_of("acme", "run-pr-42-head1/ci.pipeline:0/job"),
        Some(JobState::Terminal),
        "the prior PR head was cancelled (superseded)"
    );
    assert_eq!(
        sched.state_of("acme", "run-pr-42-head2/ci.pipeline:0/job"),
        Some(JobState::Queued),
        "only the latest PR head is tested"
    );
}

/// **A `kind=agent` job is REJECTED — CI's runner enqueues kind=ci into `job_queue` only (arch §3.1
/// boundary).** An agent job dispatches into the agent runner, not CI's `job_queue`; a mis-routed
/// kind is a LOUD error (never silently enqueued onto the wrong runner).
#[test]
fn a_kind_agent_job_is_rejected() {
    let r = runner();
    let err = r
        .dispatch(&JobSpec {
            kind: JobKind::Agent,
            target: "agent://acme/job/x".into(),
            idem_token: "run-pr-7/ci.pipeline:0/job".into(),
        })
        .expect_err("a kind=agent job is not a CI job_queue job");
    assert!(
        err.0.contains("kind=ci"),
        "the mis-routed kind is a loud error, got {err:?}"
    );
    assert_eq!(
        r.scheduler().lock().unwrap().jobs().len(),
        0,
        "the agent job was NEVER enqueued into CI's job_queue"
    );
}
