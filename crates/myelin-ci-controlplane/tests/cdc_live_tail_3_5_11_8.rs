use myelin_ci_controlplane::{
    AnchorStatus, CoalesceBudget, DetailsRefResolver, LiveTail, LogCoord, LogPipeline,
    ResumeOutcome, SealThreshold, SecretRedactor, SegmentIndex, CI_LOG_STREAM,
};
use myelin_events::firehose::{Firehose, FrameDraft};
use myelin_storage::FsBlobStore;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}
fn region() -> Region {
    Region("fr-par".into())
}

#[test]
fn cdc_3_5_viewer_resume_backfills_the_gap_through_the_real_firehose() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord
        .firehose_scope()
        .expect("the bounded run:<id> the transport admits");

    for i in 1..=6u64 {
        let f = fh
            .publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")))
            .expect("the fixture publishes a valid frame");
        assert_eq!(f.seq, i, "the per-(stream, scope) monotone resume cursor");
    }

    let mut tail = LiveTail::new(&mut fh, SegmentIndex::default());
    let outcome = tail.resume(&coord, 3, 0).expect("in-window resume decodes");
    let ResumeOutcome::Live(sub) = outcome else {
        panic!("an in-window resume is live");
    };
    assert_eq!(
        sub.drain_ready().iter().map(|f| f.seq).collect::<Vec<_>>(),
        vec![4, 5, 6],
        "the viewer resume backfills the gap through the real firehose (3.5 no-drift)"
    );
}

#[test]
fn cdc_3_5_viewer_decodes_resync_required_over_window() {
    let mut fh = Firehose::with_limits(3, myelin_events::firehose::DEFAULT_INFLIGHT_CAP);
    let coord = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = coord.firehose_scope().expect("bounded");
    for i in 0..6u64 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("l-{i}")))
            .expect("the fixture publishes a valid frame");
    }
    let archive = SegmentIndex::from_rows(
        "01J0RUN",
        "01J0JOB",
        &[myelin_ci_controlplane::LogSegmentRow {
            tenant_id: "01J0ACME".into(),
            region: "fr-par".into(),
            run_id: "01J0RUN".into(),
            job_id: "01J0JOB".into(),
            segment_seq: 0,
            blob_ref: Some("blob-0".into()),
            byte_start: 0,
            byte_end: 60,
            pii_key_ref: "kms://01J0ACME/0/tenant".into(),
        }],
    );
    let mut tail = LiveTail::new(&mut fh, archive);
    let outcome = tail
        .resume(&coord, 1, 60)
        .expect("the resume returns a verdict");
    assert!(
        outcome.is_resync_required(),
        "the viewer decodes the over-window resync_required verdict (3.5 no-drift)"
    );
}

#[test]
fn cdc_11_8_details_ref_decodes_through_the_job_step_byte_range_index() {
    let mut p = LogPipeline::new(
        tenant(),
        region(),
        FsBlobStore::new(),
        SecretRedactor::default(),
    )
    .with_thresholds(
        CoalesceBudget::default(),
        SealThreshold { seal_at_bytes: 20 },
    );
    let c = LogCoord::new("01J0RUN", "01J0JOB", 9);
    for _ in 0..5 {
        p.ship_line(&c, "0123456789").expect("ship");
    }
    p.close_step(&c, AnchorStatus::Failed).expect("close");
    p.flush_job("01J0RUN", "01J0JOB", 9).expect("flush");

    let anchors = p.anchor_rows().into_iter().cloned().collect();
    let segments = p.segment_rows().to_vec();
    let resolver = DetailsRefResolver::new(anchors, segments);

    let details_ref = c.details_ref(&tenant()).expect("canonical details ref").0;
    let range = resolver
        .resolve(&details_ref)
        .expect("the producer-written #step-9 anchor decodes (11.8 no-drift)");
    assert_eq!(range.step_id, "9");
    assert_eq!(range.status, "failed");
    assert_eq!(range.byte_start, 0, "step 9's bytes start at 0");
    assert_eq!(range.byte_end, Some(50), "step 9 wrote 5x10 = 50 bytes");
    assert_eq!(
        resolver.dangling_anchor_count([details_ref.as_str()]),
        0,
        "0 dangling step anchors - the index is byte-consistent (11.8)"
    );
}
