use std::sync::Arc;

use myelin_ci_controlplane::{
    AnchorStatus, CoalesceBudget, LogAvailablePointer, LogCoord, LogPipeline, SealThreshold,
    SecretRedactor, CI_LOG_STREAM,
};
use myelin_events::firehose::Firehose;
use myelin_events::{derive_envelope, Actor, EmitContext, EventId, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::{
    ci_log_index_specs, ci_log_search_projection, AclFilter, CiLogProjectionInput,
    IncrementalIndexer, MapFetcher, MockEmbeddingAdapter,
};
use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn region() -> Region {
    Region("fr-par".into())
}

#[test]
fn cdc_11_2_sealed_segment_blob_round_trips_through_blobstore_get() {
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
    let c = LogCoord::new("01J0RUN", "01J0JOB", 1);
    p.ship_line(&c, "the build log line")
        .expect("in-region ship");

    let seg = &p.segment_rows()[0];
    let blob_ref = seg
        .blob_ref
        .as_ref()
        .expect("a sealed segment has a blob_ref");

    let consumer_store = FsBlobStore::new();
    let bytes = b"the build log line";
    let put_addr = consumer_store.put(&tenant(), bytes).expect("put");
    assert_eq!(
        put_addr.to_multihash_string(),
        *blob_ref,
        "CI's log_segment.blob_ref is the content address of the sealed bytes (11.2 no-drift)"
    );
    let got = consumer_store
        .get(&tenant(), &put_addr)
        .expect("get re-hash-verified");
    assert_eq!(
        got, bytes,
        "the sealed bytes round-trip byte-identically (11.2)"
    );
    let parsed = ContentHash::parse(blob_ref).expect("the blob_ref parses as a content address");
    assert_eq!(
        consumer_store
            .get(&tenant(), &parsed)
            .expect("get by parsed addr"),
        bytes
    );
}

#[test]
fn cdc_11_8_sealed_segment_writes_the_job_step_byte_range_index() {
    let mut p = LogPipeline::new(
        tenant(),
        region(),
        FsBlobStore::new(),
        SecretRedactor::default(),
    )
    .with_thresholds(
        CoalesceBudget::default(),
        SealThreshold { seal_at_bytes: 40 },
    );
    let c = LogCoord::new("01J0RUN", "01J0JOB", 7);
    for _ in 0..8 {
        p.ship_line(&c, "0123456789").expect("ship");
    }
    p.close_step(&c, AnchorStatus::Failed)
        .expect("close the step");

    assert!(
        !p.segment_rows().is_empty(),
        "a segment sealed → a log_segment index row"
    );
    let anchors = p.anchor_rows();
    assert_eq!(
        anchors.len(),
        1,
        "the step has ONE log_anchor (the (job, step) index key)"
    );
    let anchor = anchors[0];

    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "0 dangling anchors - the index is byte-consistent"
    );
    assert_eq!(anchor.status, AnchorStatus::Failed);
    assert!(
        anchor.byte_end.is_some(),
        "a terminal anchor closes its byte_end"
    );
    assert_eq!(
        c.details_ref(&tenant()).expect("canonical details ref").0,
        "myelin://01J0ACME/ci/run/01J0RUN#step-7",
        "the details_ref is the canonical tenant-bound jump-to-failure path"
    );
    for seg in p.segment_rows() {
        assert!(
            seg.byte_end > seg.byte_start,
            "the segment covers a real byte range"
        );
        assert!(
            seg.blob_ref.is_some(),
            "a sealed segment names its (blob, offset)"
        );
    }
}

#[test]
fn cdc_3_5_live_tail_rides_the_firehose_on_a_bounded_run_scope() {
    let c = LogCoord::new("01J0RUN", "01J0JOB", 1);
    let scope = c.firehose_scope().expect("a bounded run:<id> scope");
    assert_eq!(
        scope.selector(),
        "run:01J0RUN",
        "CI's live-tail scope is bounded run:<id>"
    );

    let mut fh = Firehose::new();
    use myelin_events::firehose::FrameDraft;
    let sub = fh
        .subscribe(CI_LOG_STREAM, &scope, None)
        .expect("a bounded scope subscribes (never *)");
    for i in 1..=5u64 {
        let f = fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")));
        assert_eq!(
            f.seq, i,
            "the per-(stream, scope) monotone seq (the resume cursor)"
        );
    }
    let seqs: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seqs,
        vec![1, 2, 3, 4, 5],
        "the live tail delivers frames in seq order (3.5)"
    );

    assert!(
        fh.subscribe_raw(CI_LOG_STREAM, "*", None)
            .expect_err("the transport rejects `*`")
            .is_over_broad_scope(),
        "the live tail is bounded - `*` is rejected (3.5 whitelist-not-*)"
    );
}

#[test]
fn cdc_producer_end_to_end_rides_all_three_consumed_surfaces() {
    let mut p = LogPipeline::new(
        tenant(),
        region(),
        FsBlobStore::new(),
        SecretRedactor::default(),
    )
    .with_thresholds(
        CoalesceBudget {
            bytes_per_pointer: 1024,
        },
        SealThreshold {
            seal_at_bytes: 4096,
        },
    );
    let c = LogCoord::new("01J0RUN", "01J0JOB", 1);
    for _ in 0..1000 {
        p.ship_line(&c, "a sixteen-byte!!").expect("ship");
    }
    p.flush_job("01J0RUN", "01J0JOB", 1).expect("flush");

    assert_eq!(
        p.lines_shipped(),
        1000,
        "every line rode the firehose (3.5 live tail)"
    );
    assert!(
        !p.segment_rows().is_empty(),
        "segments sealed to blobs (11.2) + the index (11.8)"
    );
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "0 dangling anchors (11.8 index consistency)"
    );
    let pointers = p.durable_pointer_count();
    assert!(
        pointers > 0 && pointers * 10 < 1000,
        "ci.log.available is COALESCED ({pointers} ≪ 1000 lines, never per-line)"
    );
    assert!(
        p.admitted_log_writes() > 0,
        "the residency-pin admitted only in-region writes"
    );
}

#[test]
fn ci_log_available_from_the_real_producer_is_searchable_under_its_parent_run_grant() {
    let pointer = LogAvailablePointer {
        coord: LogCoord::new("01J0RUN", "01J0JOB", 7),
        byte_start: 0,
        byte_end: 21,
        segment_ref: Some("blake3:segment".into()),
    };
    let draft = pointer
        .to_draft(&tenant())
        .expect("the producer mints one canonical tenant-bound event");
    assert_eq!(
        draft.subject.0, "myelin://01J0ACME/ci/log/01J0RUN:01J0JOB:7",
        "the event subject is the searchable log document, not a scope-less deep link"
    );
    assert_eq!(
        draft.payload["details_ref"], "myelin://01J0ACME/ci/run/01J0RUN#step-7",
        "the payload separately carries the human jump-to-step link"
    );

    let projection = ci_log_search_projection(&CiLogProjectionInput {
        run_id: "myelin://01J0ACME/ci/run/01J0RUN".into(),
        job_id: "01J0JOB".into(),
        step_no: 7,
        log_text: "compile failed in scheduler".into(),
        lang: None,
    });
    let fetcher = Arc::new(MapFetcher::new([(draft.subject.0.clone(), projection)]));
    let indexer = IncrementalIndexer::new(
        ci_log_index_specs(),
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    );
    let event = derive_envelope(
        draft,
        EmitContext {
            event_id: EventId("ci-log-available-7".into()),
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("ci-controlplane".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-08-15T00:00:00Z".into()),
            recorded_at: Timestamp("2026-08-15T00:00:00Z".into()),
            caused_by: None,
        },
        None,
    );

    indexer
        .index(&event)
        .expect("Search accepts the exact event CI persists");
    let hits = indexer
        .search_ft(
            &tenant(),
            &region(),
            &AclFilter::ids(["myelin://01J0ACME/ci/run/01J0RUN"]),
            "scheduler",
            10,
        )
        .expect("the parent-authorized query succeeds");
    assert_eq!(hits.len(), 1, "one run grant reveals its failing log step");
    assert_eq!(hits[0].doc_id, event.subject.0);
}
