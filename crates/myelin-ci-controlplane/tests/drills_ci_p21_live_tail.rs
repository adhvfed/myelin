use myelin_ci_controlplane::{
    AnchorStatus, CoalesceBudget, DetailsRefError, DetailsRefResolver, LiveTail, LogCoord,
    LogPipeline, ResumeOutcome, SealThreshold, SecretRedactor, SegmentIndex, CI_LOG_STREAM,
};
use myelin_events::firehose::{Firehose, FrameDraft, DEFAULT_INFLIGHT_CAP};
use myelin_storage::FsBlobStore;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn region() -> Region {
    Region("fr-par".into())
}

#[test]
fn ci_d11_reconnect_mid_run_loses_zero_lines() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord.firehose_scope().expect("bounded run:<id>");

    let mut tail = LiveTail::new(&mut fh, SegmentIndex::default());
    let sub = tail.subscribe(&coord, None).expect("bounded subscribe");
    for i in 1..=4u64 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")))
            .expect("the fixture publishes a valid frame");
    }
    let seen_before: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seen_before,
        vec![1, 2, 3, 4],
        "the viewer followed 1..4 live"
    );
    let last_seq = sub.last_seq();
    assert_eq!(last_seq, 4, "the resume cursor at drop time is 4");

    for i in 5..=12u64 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")))
            .expect("the fixture publishes a valid frame");
    }

    let mut tail = LiveTail::new(&mut fh, SegmentIndex::default());
    let outcome = tail.resume(&coord, last_seq, 0).expect("in-window resume");
    assert!(
        outcome.is_live(),
        "an in-window reconnect backfills, never resyncs"
    );
    let ResumeOutcome::Live(sub2) = outcome else {
        panic!("expected a live resume");
    };
    let backfilled: Vec<u64> = sub2.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        backfilled,
        (5..=12).collect::<Vec<_>>(),
        "CI-D11: the gap (4, now] is replayed - ZERO lines lost"
    );

    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-13"))
        .expect("the fixture publishes a valid frame");
    let live: Vec<u64> = sub2.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(live, vec![13], "live continues gap-free; 0 duplicate");

    let mut all = seen_before;
    all.extend(backfilled);
    all.extend(live);
    assert_eq!(
        all,
        (1..=13).collect::<Vec<_>>(),
        "CI-D11 green: across the reconnect - 0 lost, 0 duplicate (1..13 exactly once)"
    );
}

#[test]
fn ci_d11_out_of_window_last_seq_falls_back_to_a_range_read() {
    let mut fh = Firehose::with_limits(4, DEFAULT_INFLIGHT_CAP);
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord.firehose_scope().expect("bounded");
    for i in 1..=12u64 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")))
            .expect("the fixture publishes a valid frame");
    }

    let archive = SegmentIndex::from_rows("01J0RUN", "01J0JOB", &build_archive_rows());

    let mut tail = LiveTail::new(&mut fh, archive);
    let outcome = tail
        .resume(&coord, 4, 120)
        .expect("resume returns a verdict");
    assert!(
        outcome.is_resync_required(),
        "CI-D11: an out-of-window last_seq RAISES resync_required (NAMED, never a silent gap)"
    );
    let ResumeOutcome::ResyncRequired {
        window_floor,
        range_read,
    } = outcome
    else {
        panic!("expected resync_required");
    };
    assert_eq!(
        window_floor, 9,
        "the window floor is the oldest held seq (9)"
    );
    assert!(
        !range_read.is_empty(),
        "the range-read recovers the gap from the sealed segments"
    );
    for w in range_read.windows(2) {
        assert!(
            w[0].byte_start <= w[1].byte_start,
            "segments are byte_start-ordered"
        );
    }
}

#[test]
fn ci_d11_scope_stays_bounded_never_star() {
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord.firehose_scope().expect("bounded run:<id>");
    assert_eq!(
        scope.selector(),
        "run:01J0RUN",
        "the viewer scope is bounded run:<id>"
    );

    let mut fh = Firehose::new();
    for over_broad in ["*", "run:*", "doc:*", "", "01J0RUN", "all"] {
        assert!(
            fh.subscribe_raw(CI_LOG_STREAM, over_broad, None)
                .err()
                .map(|e| e.is_over_broad_scope())
                .unwrap_or(false),
            "CI-D11: over-broad scope `{over_broad}` is REJECTED at the viewer seam"
        );
    }
    assert!(
        fh.subscribe_raw(CI_LOG_STREAM, "run:01J0RUN", None).is_ok(),
        "a bounded run:<id> scope subscribes"
    );
}

#[test]
fn ci_d11_details_ref_jump_to_failure_resolves_zero_dangling_anchors() {
    let mut p = LogPipeline::new(
        tenant(),
        region(),
        FsBlobStore::new(),
        SecretRedactor::default(),
    )
    .with_thresholds(
        CoalesceBudget::default(),
        SealThreshold { seal_at_bytes: 16 },
    );

    let s1 = LogCoord::new("01J0RUN", "01J0JOB", 1);
    for _ in 0..4 {
        p.ship_line(&s1, "0123456789").expect("ship");
    }
    p.close_step(&s1, AnchorStatus::Passed).expect("close");

    let s2 = LogCoord::new("01J0RUN", "01J0JOB", 2);
    for _ in 0..3 {
        p.ship_line(&s2, "FAIL-LINE!").expect("ship");
    }
    p.close_step(&s2, AnchorStatus::Failed).expect("close");
    p.flush_job("01J0RUN", "01J0JOB", 2).expect("flush");

    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "the producer index is byte-consistent"
    );

    let anchors = p.anchor_rows().into_iter().cloned().collect();
    let segments = p.segment_rows().to_vec();
    let resolver = DetailsRefResolver::new(anchors, segments);

    let ref1 = s1.details_ref(&tenant()).expect("canonical step 1 ref").0;
    let ref2 = s2.details_ref(&tenant()).expect("canonical step 2 ref").0;
    assert_eq!(
        resolver.dangling_anchor_count([ref1.as_str(), ref2.as_str()]),
        0,
        "CI-D11: 0 dangling step anchors - every step's details_ref resolves to a byte range"
    );

    let range = resolver.resolve(&ref2).expect("the failed step resolves");
    assert_eq!(
        range.status, "failed",
        "the jump-to-failure target is the failed step"
    );
    assert_eq!(
        range.byte_start, 40,
        "step 2 starts after step 1's 40 bytes"
    );
    assert!(
        range.byte_end.unwrap() > range.byte_start,
        "the failed step covers a real byte range"
    );

    let missing = "myelin://ci/run/01J0RUN/job/01J0JOB#step-404";
    assert!(
        matches!(
            resolver.resolve(missing),
            Err(DetailsRefError::AnchorGone { .. })
        ),
        "a missing step is a NAMED AnchorGone tombstone, never a silent dangle"
    );
}

fn build_archive_rows() -> Vec<myelin_ci_controlplane::LogSegmentRow> {
    (0..3)
        .map(|i| myelin_ci_controlplane::LogSegmentRow {
            tenant_id: "01J0ACME".into(),
            region: "fr-par".into(),
            run_id: "01J0RUN".into(),
            job_id: "01J0JOB".into(),
            segment_seq: i,
            blob_ref: Some(format!("blob-{i}")),
            byte_start: (i as i64) * 40,
            byte_end: (i as i64) * 40 + 40,
            pii_key_ref: "kms://01J0ACME/0/tenant".into(),
        })
        .collect()
}
