//! Unit tests for the CI-P21 resume-cursor live-tail viewer + the `details_ref` resolution.
//! (CI-P21 → P-364, M4)
//!
//! Proves: the resume backfill `(last_seq, now]` math (0 lost lines), the `resync_required` →
//! range-read fallback, the bounded-scope rejection of `*`, and the `#step-<n>` → byte-range
//! resolution (0 dangling anchors). The CI-D11 drill scenario is `tests/drills_ci_p21_live_tail.rs`.

use myelin_ci_controlplane::{
    read_range_from_archive, AnchorStatus, CoalesceBudget, DetailsRefError, DetailsRefResolver,
    LiveTail, LogAnchorRow, LogCoord, LogPipeline, LogSegmentRow, ResumeOutcome, SealThreshold,
    SecretRedactor, SegmentIndex, CI_LOG_STREAM,
};
use myelin_events::firehose::{Firehose, FrameDraft};
use myelin_storage::{BlobStore, FsBlobStore};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn region() -> Region {
    Region("fr-par".into())
}

fn anchor(
    run: &str,
    job: &str,
    step: &str,
    start: i64,
    end: Option<i64>,
    status: AnchorStatus,
) -> LogAnchorRow {
    LogAnchorRow {
        tenant_id: "01J0ACME".into(),
        region: "fr-par".into(),
        run_id: run.into(),
        job_id: job.into(),
        step_id: step.into(),
        byte_start: start,
        byte_end: end,
        status,
    }
}

// =================================================================================================
// 1. The resume-cursor backfill — (last_seq, now] then live, 0 lost lines.
// =================================================================================================

/// **A reconnect backfills `(last_seq, now]` then goes live, losing ZERO lines (CI-D11 core).** A
/// viewer saw up to seq 2; lines 3,4,5 ship while it is disconnected; `resume(last_seq = 2)` delivers
/// EXACTLY {3,4,5} then any subsequent live frame — contiguous, no gap, no duplicate.
#[test]
fn resume_backfills_the_gap_then_goes_live_losing_zero_lines() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded run:<id>");

    // The viewer is connected and sees lines 1,2 live, then the connection drops.
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-1"));
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-2"));
    // While disconnected, 3,4,5 ship (the gap).
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-3"));
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-4"));
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-5"));

    let mut tail = LiveTail::new(&mut fh, SegmentIndex::default());
    let outcome = tail.resume(&coord, 2, 0).expect("in-window resume");
    assert!(
        outcome.is_live(),
        "an in-window resume backfills (never resync)"
    );
    let ResumeOutcome::Live(sub) = outcome else {
        panic!("expected a live resume");
    };
    let backfilled: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        backfilled,
        vec![3, 4, 5],
        "the gap (last_seq, now] is replayed — ZERO lines lost"
    );
    assert_eq!(sub.last_seq(), 5, "the resume cursor advanced to the head");

    // A LIVE frame after the resume is delivered gap-free (no duplicate of the backfill).
    let f6 = fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-6"));
    assert_eq!(f6.seq, 6);
    let live: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        live,
        vec![6],
        "live continues gap-free across the reconnect"
    );
}

/// **A `cursor = None` subscribe starts live from now (no backfill).** A fresh viewer joining a hot
/// run sees only frames published AFTER it subscribed (the live-from-head case).
#[test]
fn subscribe_with_no_cursor_starts_live_from_now() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded");
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("old-1"));
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("old-2"));

    let mut tail = LiveTail::new(&mut fh, SegmentIndex::default());
    let sub = tail.subscribe(&coord, None).expect("bounded subscribe");
    assert!(sub.drain_ready().is_empty(), "no backfill on a None cursor");

    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("new-3"));
    let live: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        live,
        vec![3],
        "only post-subscribe live frames are delivered"
    );
}

/// A resume at the CURRENT head (a caught-up viewer) backfills nothing and just continues live — the
/// no-op reconnect (the viewer never actually fell behind).
#[test]
fn resume_at_head_is_a_no_op_backfill() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded");
    for i in 0..5 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("l-{i}")));
    }
    let mut tail = LiveTail::new(&mut fh, SegmentIndex::default());
    let outcome = tail.resume(&coord, 5, 0).expect("caught-up resume");
    let ResumeOutcome::Live(sub) = outcome else {
        panic!("a caught-up resume is live, never resync");
    };
    assert!(
        sub.drain_ready().is_empty(),
        "a caught-up resume backfills nothing"
    );
}

// =================================================================================================
// 2. The resync_required → range-read fallback (an out-of-window last_seq).
// =================================================================================================

/// **An out-of-window `last_seq` → `resync_required` → a clean range-read of the sealed segments
/// (CI-D11 resync leg).** A SMALL retention window (3 frames); the viewer's `last_seq` is older than
/// the window floor → the live window cannot replay the gap → `resync_required` → the viewer reads the
/// gap from the SEALED segments (the durable archive). The bytes are recovered, never lost.
#[test]
fn out_of_window_last_seq_yields_resync_required_then_range_reads_the_archive() {
    // A window holding only the most-recent 3 frames (the drill forces eviction).
    let mut fh = Firehose::with_limits(3, myelin_events::firehose::DEFAULT_INFLIGHT_CAP);
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded");
    // 6 frames publish → the window holds {4,5,6}; 1,2,3 evicted.
    for i in 0..6 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("l-{i}")));
    }

    // The sealed archive: two segments covering the evicted bytes [0, 60).
    let archive = SegmentIndex::from_rows(
        "01J0RUN",
        "01J0JOB",
        &[
            LogSegmentRow {
                tenant_id: "01J0ACME".into(),
                region: "fr-par".into(),
                run_id: "01J0RUN".into(),
                job_id: "01J0JOB".into(),
                segment_seq: 0,
                blob_ref: Some("blob-a".into()),
                byte_start: 0,
                byte_end: 30,
                pii_key_ref: "kms://01J0ACME/0/tenant".into(),
            },
            LogSegmentRow {
                tenant_id: "01J0ACME".into(),
                region: "fr-par".into(),
                run_id: "01J0RUN".into(),
                job_id: "01J0JOB".into(),
                segment_seq: 1,
                blob_ref: Some("blob-b".into()),
                byte_start: 30,
                byte_end: 60,
                pii_key_ref: "kms://01J0ACME/0/tenant".into(),
            },
        ],
    );

    let mut tail = LiveTail::new(&mut fh, archive);
    // A viewer at last_seq = 2 needs op 3 first — but 3 was evicted → resync_required.
    let outcome = tail
        .resume(&coord, 2, 60)
        .expect("resume succeeds with a verdict");
    assert!(
        outcome.is_resync_required(),
        "an out-of-window last_seq raises resync_required (NAMED, never a silent gap)"
    );
    let ResumeOutcome::ResyncRequired {
        window_floor,
        range_read,
    } = outcome
    else {
        panic!("expected resync_required");
    };
    assert_eq!(window_floor, 4, "the window floor is the oldest held seq");
    // The range-read recovers the evicted bytes from the SEALED archive (both segments cover [0,60)).
    assert_eq!(
        range_read.len(),
        2,
        "both sealed segments cover the gap [0, 60)"
    );
    assert_eq!(range_read[0].blob_ref, "blob-a");
    assert_eq!(range_read[1].blob_ref, "blob-b");
    // The segments are byte_start-ordered (the viewer reads them in order — 0 lost bytes).
    assert!(range_read[0].byte_start < range_read[1].byte_start);
}

/// An in-window `last_seq` at the EXACT window-floor boundary backfills (never a premature resync) —
/// the off-by-one boundary the firehose floor check pins, exercised through the CI viewer.
#[test]
fn the_window_floor_boundary_backfills_not_resyncs() {
    let mut fh = Firehose::with_limits(3, myelin_events::firehose::DEFAULT_INFLIGHT_CAP);
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded");
    for i in 0..6 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("l-{i}")));
    }
    // window holds {4,5,6}, floor = 4. last_seq = 3 → first-missing 4 == floor → IN-WINDOW.
    let mut tail = LiveTail::new(&mut fh, SegmentIndex::default());
    let outcome = tail.resume(&coord, 3, 60).expect("boundary resume");
    assert!(
        outcome.is_live(),
        "first-missing == floor is in-window (backfills, not resyncs)"
    );
    let ResumeOutcome::Live(sub) = outcome else {
        panic!()
    };
    assert_eq!(
        sub.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
        vec![4, 5, 6]
    );
}

// =================================================================================================
// 3. Scope is bounded, never * (the whitelist-not-* rule at the viewer seam).
// =================================================================================================

/// **The viewer's scope is BOUNDED `run:<id>`, never `*`.** The viewer subscribes/resumes on exactly
/// `LogCoord::firehose_scope()` — a bounded `run:<id>`; the type cannot represent `*` and the
/// connection-tier `subscribe_raw` rejects an over-broad scope. A viewer can ONLY tail one bounded run.
#[test]
fn the_viewer_scope_is_bounded_run_id_never_star() {
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded run:<id>");
    assert_eq!(
        scope.selector(),
        "run:01J0RUN",
        "the viewer scope is bounded run:<id>"
    );

    // The transport rejects `*` at the raw connection-tier entry (the viewer can never tail the
    // tenant firehose) — the whitelist-not-* rule (BUS-3) at the CI live-tail seam.
    let mut fh = Firehose::new();
    assert!(
        fh.subscribe_raw(CI_LOG_STREAM, "*", None)
            .expect_err("`*` is rejected")
            .is_over_broad_scope(),
        "the CI live-tail rejects `*` (bounded scope, never the tenant firehose)"
    );
    for over_broad in ["*", "run:*", "", "01J0RUN", "all"] {
        assert!(
            fh.subscribe_raw(CI_LOG_STREAM, over_broad, None).is_err(),
            "over-broad scope `{over_broad}` is rejected at the viewer seam"
        );
    }
}

// =================================================================================================
// 4. The details_ref jump-to-failure resolution (#step-<n> → byte range; 0 dangling anchors).
// =================================================================================================

/// **A `#step-<n>` details_ref resolves through `log_anchor` → the byte range (the X-1 / OQ-D
/// jump-to-failure path).** A failed step's `details_ref = …/ci/run/<id>/job/<id>#step-7` resolves to
/// its `[byte_start, byte_end)` span + the sealed segments covering it. 0 dangling anchors.
#[test]
fn details_ref_resolves_through_the_anchor_to_a_byte_range() {
    let anchors = vec![
        anchor(
            "01J0RUN",
            "01J0JOB",
            "5",
            0,
            Some(100),
            AnchorStatus::Passed,
        ),
        anchor(
            "01J0RUN",
            "01J0JOB",
            "7",
            100,
            Some(250),
            AnchorStatus::Failed,
        ),
    ];
    let segments = vec![LogSegmentRow {
        tenant_id: "01J0ACME".into(),
        region: "fr-par".into(),
        run_id: "01J0RUN".into(),
        job_id: "01J0JOB".into(),
        segment_seq: 0,
        blob_ref: Some("blob-z".into()),
        byte_start: 0,
        byte_end: 256,
        pii_key_ref: "kms://01J0ACME/0/tenant".into(),
    }];
    let resolver = DetailsRefResolver::new(anchors, segments);

    // The failed step's details_ref (the canonical CI form CI-P20's LogCoord mints).
    let details_ref = LogCoord::new("01J0RUN", "01J0JOB", "7").details_ref().0;
    assert_eq!(details_ref, "myelin://ci/run/01J0RUN/job/01J0JOB#step-7");

    let range = resolver
        .resolve(&details_ref)
        .expect("the #step-7 anchor resolves");
    assert_eq!(range.run_id, "01J0RUN");
    assert_eq!(range.job_id, "01J0JOB");
    assert_eq!(range.step_id, "7");
    assert_eq!(
        range.byte_start, 100,
        "the failed step's bytes START at 100"
    );
    assert_eq!(
        range.byte_end,
        Some(250),
        "the failed step's bytes END at 250"
    );
    assert_eq!(
        range.status, "failed",
        "the jump-to-failure target is the failed step"
    );
    assert_eq!(
        range.segments.len(),
        1,
        "the covering sealed segment is named"
    );
    assert_eq!(range.segments[0].blob_ref, "blob-z");
}

/// **The Refs-canonical `#sub` form (`myelin://<tenant>/ci/run/<id>#step-<n>`) resolves too.** The
/// X-1 `CheckStatus.details_ref` Git renders is the tenant-scoped Refs grammar; the resolver parses the
/// `ci/run/<run>` path + the `#step-<n>` sub-anchor regardless of the tenant prefix / the optional job.
#[test]
fn the_refs_canonical_details_ref_form_resolves() {
    let anchors = vec![anchor(
        "01J0RUN",
        "01J0JOB",
        "3",
        0,
        Some(50),
        AnchorStatus::Failed,
    )];
    let resolver = DetailsRefResolver::new(anchors, vec![]);

    // The run-level Refs form (no /job/ segment, a tenant prefix) — the (run, step) still resolves.
    let range = resolver
        .resolve("myelin://acme/ci/run/01J0RUN#step-3")
        .expect("the run-level #step-3 resolves");
    assert_eq!(range.step_id, "3");
    assert_eq!(range.byte_start, 0);
    assert_eq!(range.byte_end, Some(50));
}

/// **A `details_ref` for a step with NO anchor is a NAMED `AnchorGone` tombstone (0 dangling
/// anchors).** The resolver never silently dangles — a missing anchor is the named tombstone (the
/// viewer shows the parent run). The dangling-anchor count over a set of refs is the GATE.
#[test]
fn a_missing_anchor_is_a_named_tombstone_never_a_silent_dangle() {
    let anchors = vec![anchor(
        "01J0RUN",
        "01J0JOB",
        "5",
        0,
        Some(100),
        AnchorStatus::Passed,
    )];
    let resolver = DetailsRefResolver::new(anchors, vec![]);

    // step-99 has no anchor → AnchorGone (a NAMED tombstone, never a silent dangle).
    let err = resolver
        .resolve("myelin://ci/run/01J0RUN/job/01J0JOB#step-99")
        .expect_err("a missing anchor is a tombstone");
    assert!(
        matches!(err, DetailsRefError::AnchorGone { ref step_id, .. } if step_id == "99"),
        "a missing anchor is the NAMED AnchorGone tombstone, got {err:?}"
    );

    // The GATE: 0 dangling anchors over the resolvable refs; the missing one counts as 1 dangle.
    let real_ref = "myelin://ci/run/01J0RUN/job/01J0JOB#step-5";
    let missing_ref = "myelin://ci/run/01J0RUN/job/01J0JOB#step-99";
    assert_eq!(
        resolver.dangling_anchor_count([real_ref]),
        0,
        "every REAL step's details_ref resolves — 0 dangling anchors (the GATE)"
    );
    assert_eq!(
        resolver.dangling_anchor_count([real_ref, missing_ref]),
        1,
        "a missing anchor is COUNTED as a dangle (the gate observable is exact)"
    );
}

/// **A non-step ref is REJECTED, never guessed (the OQ-D "rejects ambiguity" rule).** A ref with no
/// `#step-<n>` sub-anchor, a non-step sub-anchor, or no `ci/run/<run>` path is a NAMED `NotAStepRef`.
#[test]
fn a_non_step_ref_is_rejected_never_guessed() {
    let resolver = DetailsRefResolver::new(vec![], vec![]);
    for (raw, _why) in [
        ("myelin://ci/run/01J0RUN", "no #sub at all"),
        (
            "myelin://ci/run/01J0RUN#check-build",
            "a non-step sub-anchor",
        ),
        ("myelin://ci/run/01J0RUN#step-", "an empty step id"),
        ("myelin://kn/doc/abc#b123", "not a ci/run path"),
    ] {
        assert!(
            matches!(
                resolver.resolve(raw),
                Err(DetailsRefError::NotAStepRef { .. })
            ),
            "`{raw}` is a NotAStepRef rejection (never guessed)"
        );
    }
}

/// **A still-RUNNING step's anchor resolves to an OPEN byte range (`byte_end = None`).** The
/// jump-to-failure works mid-run: the viewer scrolls to the step's start and tails the live window for
/// the open end (the live-tail composes with the resolution).
#[test]
fn a_running_step_resolves_to_an_open_byte_range() {
    let anchors = vec![anchor(
        "01J0RUN",
        "01J0JOB",
        "2",
        40,
        None,
        AnchorStatus::Running,
    )];
    let resolver = DetailsRefResolver::new(anchors, vec![]);
    let range = resolver
        .resolve("myelin://ci/run/01J0RUN/job/01J0JOB#step-2")
        .expect("a running step resolves");
    assert_eq!(range.byte_start, 40);
    assert_eq!(
        range.byte_end, None,
        "a running step has an OPEN byte range (the live cursor)"
    );
    assert_eq!(range.status, "running");
}

// =================================================================================================
// 5. End-to-end: the producer ships, the viewer reads back (the round-trip over CI-P20's index).
// =================================================================================================

/// **The producer ships lines → the viewer resolves the failed step's `details_ref` to the byte range
/// → reads the archived bytes back BYTE-IDENTICALLY.** The full CI-P20 (produce) ↔ CI-P21 (view)
/// round-trip: ship lines for two steps, fail step 2, resolve its `details_ref`, range-read the sealed
/// bytes from the real `BlobStore` — the failed step's bytes come back exactly.
#[test]
fn producer_to_viewer_round_trip_reads_the_failed_steps_bytes() {
    let mut p = LogPipeline::new(
        tenant(),
        region(),
        FsBlobStore::new(),
        SecretRedactor::default(),
    )
    .with_thresholds(
        CoalesceBudget::default(),
        SealThreshold { seal_at_bytes: 1 },
    );

    // Step 1 ships two lines (each 10 bytes), passes.
    let s1 = LogCoord::new("01J0RUN", "01J0JOB", "1");
    p.ship_line(&s1, "AAAAAAAAAA").expect("ship");
    p.ship_line(&s1, "BBBBBBBBBB").expect("ship");
    p.close_step(&s1, AnchorStatus::Passed).expect("close");

    // Step 2 ships one line (10 bytes), FAILS — the jump-to-failure target.
    let s2 = LogCoord::new("01J0RUN", "01J0JOB", "2");
    p.ship_line(&s2, "CCCCCCCCCC").expect("ship");
    p.close_step(&s2, AnchorStatus::Failed).expect("close");
    p.flush_job("01J0RUN", "01J0JOB", "2").expect("flush");

    // 0 dangling anchors (the producer-side index is consistent).
    assert_eq!(p.dangling_anchor_count(), 0);

    // The viewer side: resolve the failed step's details_ref → the byte range.
    let anchors: Vec<LogAnchorRow> = p.anchor_rows().into_iter().cloned().collect();
    let segments: Vec<_> = p.segment_rows().to_vec();
    let resolver = DetailsRefResolver::new(anchors, segments.clone());

    let details_ref = s2.details_ref().0;
    let range = resolver
        .resolve(&details_ref)
        .expect("the failed step resolves");
    assert_eq!(range.status, "failed");
    assert_eq!(
        range.byte_start, 20,
        "step 2's bytes start after step 1's 20 bytes"
    );
    assert_eq!(range.byte_end, Some(30), "step 2 wrote 10 bytes (one line)");

    // Read the failed step's bytes back from the sealed archive — BYTE-IDENTICAL to what shipped.
    let blobs = FsBlobStore::new();
    // re-seed the consumer-side store with the same content-addressed bytes the pipeline sealed.
    for line in ["AAAAAAAAAA", "BBBBBBBBBB", "CCCCCCCCCC"] {
        blobs.put(&tenant(), line.as_bytes()).expect("seed");
    }
    let bytes = read_range_from_archive(
        &blobs,
        &tenant(),
        &range.segments,
        range.byte_start,
        range.byte_end.unwrap(),
    );
    assert_eq!(
        bytes, b"CCCCCCCCCC",
        "the failed step's bytes round-trip byte-identically from the sealed archive"
    );
}
