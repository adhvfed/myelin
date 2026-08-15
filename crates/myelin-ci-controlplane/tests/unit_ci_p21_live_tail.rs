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

#[test]
fn resume_backfills_the_gap_then_goes_live_losing_zero_lines() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord.firehose_scope().expect("bounded run:<id>");

    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-1"));
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-2"));
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
        "the gap (last_seq, now] is replayed - ZERO lines lost"
    );
    assert_eq!(sub.last_seq(), 5, "the resume cursor advanced to the head");

    let f6 = fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-6"));
    assert_eq!(f6.seq, 6);
    let live: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        live,
        vec![6],
        "live continues gap-free across the reconnect"
    );
}

#[test]
fn subscribe_with_no_cursor_starts_live_from_now() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
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

#[test]
fn resume_at_head_is_a_no_op_backfill() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
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

#[test]
fn out_of_window_last_seq_yields_resync_required_then_range_reads_the_archive() {
    let mut fh = Firehose::with_limits(3, myelin_events::firehose::DEFAULT_INFLIGHT_CAP);
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord.firehose_scope().expect("bounded");
    for i in 0..6 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("l-{i}")));
    }

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
    assert_eq!(
        range_read.len(),
        2,
        "both sealed segments cover the gap [0, 60)"
    );
    assert_eq!(range_read[0].blob_ref, "blob-a");
    assert_eq!(range_read[1].blob_ref, "blob-b");
    assert!(range_read[0].byte_start < range_read[1].byte_start);
}

#[test]
fn the_window_floor_boundary_backfills_not_resyncs() {
    let mut fh = Firehose::with_limits(3, myelin_events::firehose::DEFAULT_INFLIGHT_CAP);
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord.firehose_scope().expect("bounded");
    for i in 0..6 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("l-{i}")));
    }
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

#[test]
fn the_viewer_scope_is_bounded_run_id_never_star() {
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord.firehose_scope().expect("bounded run:<id>");
    assert_eq!(
        scope.selector(),
        "run:01J0RUN",
        "the viewer scope is bounded run:<id>"
    );

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

    let details_ref = LogCoord::new("01J0RUN", "01J0JOB", 7)
        .details_ref(&tenant())
        .expect("canonical details ref")
        .0;
    assert_eq!(details_ref, "myelin://01J0ACME/ci/run/01J0RUN#step-7");

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

    let range = resolver
        .resolve("myelin://acme/ci/run/01J0RUN#step-3")
        .expect("the run-level #step-3 resolves");
    assert_eq!(range.step_id, "3");
    assert_eq!(range.byte_start, 0);
    assert_eq!(range.byte_end, Some(50));
}

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

    let err = resolver
        .resolve("myelin://ci/run/01J0RUN/job/01J0JOB#step-99")
        .expect_err("a missing anchor is a tombstone");
    assert!(
        matches!(err, DetailsRefError::AnchorGone { ref step_id, .. } if step_id == "99"),
        "a missing anchor is the NAMED AnchorGone tombstone, got {err:?}"
    );

    let real_ref = "myelin://ci/run/01J0RUN/job/01J0JOB#step-5";
    let missing_ref = "myelin://ci/run/01J0RUN/job/01J0JOB#step-99";
    assert_eq!(
        resolver.dangling_anchor_count([real_ref]),
        0,
        "every REAL step's details_ref resolves - 0 dangling anchors (the GATE)"
    );
    assert_eq!(
        resolver.dangling_anchor_count([real_ref, missing_ref]),
        1,
        "a missing anchor is COUNTED as a dangle (the gate observable is exact)"
    );
}

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

    let s1 = LogCoord::new("01J0RUN", "01J0JOB", 1);
    p.ship_line(&s1, "AAAAAAAAAA").expect("ship");
    p.ship_line(&s1, "BBBBBBBBBB").expect("ship");
    p.close_step(&s1, AnchorStatus::Passed).expect("close");

    let s2 = LogCoord::new("01J0RUN", "01J0JOB", 2);
    p.ship_line(&s2, "CCCCCCCCCC").expect("ship");
    p.close_step(&s2, AnchorStatus::Failed).expect("close");
    p.flush_job("01J0RUN", "01J0JOB", 2).expect("flush");

    assert_eq!(p.dangling_anchor_count(), 0);

    let anchors: Vec<LogAnchorRow> = p.anchor_rows().into_iter().cloned().collect();
    let segments: Vec<_> = p.segment_rows().to_vec();
    let resolver = DetailsRefResolver::new(anchors, segments.clone());

    let details_ref = s2.details_ref(&tenant()).expect("canonical details ref").0;
    let range = resolver
        .resolve(&details_ref)
        .expect("the failed step resolves");
    assert_eq!(range.status, "failed");
    assert_eq!(
        range.byte_start, 20,
        "step 2's bytes start after step 1's 20 bytes"
    );
    assert_eq!(range.byte_end, Some(30), "step 2 wrote 10 bytes (one line)");

    let blobs = FsBlobStore::new();
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
