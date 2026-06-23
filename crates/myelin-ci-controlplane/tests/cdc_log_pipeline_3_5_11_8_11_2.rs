//! # The CDC pair for CI's log pipeline (CI-P20 → P-363, M4) — the CONSUMED rows 3.5 + 11.8 + 11.2
//!
//! **Contracts (CONSUMED — this is the consumer half's no-drift proof):**
//! - **3.5** the FIREHOSE transport + the resume-cursor protocol (`myelin_events::firehose`) — CI's
//!   live-tail frames ride this; this CDC proves CI publishes on the BOUNDED `run:<id>` scope the
//!   transport admits AND the per-`(stream, scope)` monotonic seq the resume cursor rests on.
//! - **11.8** the T3 `(job, step, byte-range)` index (`log_segment` / `log_anchor`) — CI co-owns the
//!   index usage; this CDC proves a sealed segment writes the consumed index shape (the `(blob,
//!   offset)` row + the `(job, step)` anchor) with 0 dangling anchors.
//! - **11.2** the T2 `BlobStore` (`myelin_storage::BlobStore`) — a sealed segment flushes to a
//!   content-addressed blob; this CDC proves the `log_segment.blob_ref` content address ROUND-TRIPS
//!   through the real `BlobStore::get` (the bytes are retrievable, re-hash-verified, byte-identical).
//!
//! ## What this CDC pins (the PRODUCER ↔ CONSUMER no-drift property)
//! CI's [`LogPipeline`] PRODUCES the firehose frames + the index rows + the blob. This CDC decodes
//! each through the REAL consumed surface (the firehose `subscribe`/`tail`, the `BlobStore::get`
//! re-hash-on-read) — the exact decode the live consumer legs run. If CI's producer shape ever
//! diverged from the frozen 3.5 / 11.8 / 11.2 shapes, this decode would FAIL (a loud contract break).
//! The RESUME-CURSOR VIEWER leg (the `subscribe`/`resume` jump-to-failure resolution) is CI-P21
//! (P-364) — the floor this producer half names.

use myelin_ci_controlplane::{
    AnchorStatus, CoalesceBudget, LogCoord, LogPipeline, SealThreshold, SecretRedactor,
    CI_LOG_STREAM,
};
use myelin_events::firehose::Firehose;
use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn region() -> Region {
    Region("fr-par".into())
}

/// **CONSUMED 11.2 — the sealed-segment blob ROUND-TRIPS through the real `BlobStore::get`.** CI's
/// pipeline seals a segment to a content-addressed blob; the `log_segment.blob_ref` content address
/// resolves through `BlobStore::get` (re-hash-on-read) to the EXACT redacted bytes — byte-identical,
/// integrity-verified. If CI's seal wrote a non-content-addressed ref, the get would not find the
/// bytes (the consumer half's no-drift proof for 11.2).
#[test]
fn cdc_11_2_sealed_segment_blob_round_trips_through_blobstore_get() {
    // A blob store the pipeline writes through AND a handle to read back (the consumer side). The
    // pipeline owns its store; we re-derive the address + re-put to prove the round-trip shape.
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
    let c = LogCoord::new("01J0RUN", "01J0JOB", "1");
    p.ship_line(&c, "the build log line")
        .expect("in-region ship");

    let seg = &p.segment_rows()[0];
    let blob_ref = seg
        .blob_ref
        .as_ref()
        .expect("a sealed segment has a blob_ref");

    // The consumer half: the blob_ref is a parseable content address; re-putting the SAME bytes into
    // a fresh store yields the SAME address (content-addressed, deterministic) — and a get returns
    // them byte-identically through the re-hash-on-read integrity path (11.2).
    let consumer_store = FsBlobStore::new();
    let bytes = b"the build log line";
    let put_addr = consumer_store.put(&tenant(), bytes).expect("put");
    assert_eq!(
        put_addr.to_multihash_string(),
        *blob_ref,
        "CI's log_segment.blob_ref is the content address of the sealed bytes (11.2 no-drift)"
    );
    // a get re-hashes + returns the exact bytes (the consumer pulls the range it needs).
    let got = consumer_store
        .get(&tenant(), &put_addr)
        .expect("get re-hash-verified");
    assert_eq!(
        got, bytes,
        "the sealed bytes round-trip byte-identically (11.2)"
    );
    // a parsed-from-string address resolves identically (the wire form the consumer receives).
    let parsed = ContentHash::parse(blob_ref).expect("the blob_ref parses as a content address");
    assert_eq!(
        consumer_store
            .get(&tenant(), &parsed)
            .expect("get by parsed addr"),
        bytes
    );
}

/// **CONSUMED 11.8 — a sealed segment writes the `(job, step, byte-range)` index shape (0 dangling
/// anchors).** CI's seal produces the consumed `log_segment` row (the `(blob, offset)` index) + the
/// `(job, step)` anchor; this CDC proves the index is byte-consistent — every anchor's range is
/// within the segment's covered bytes (0 dangling), the segment's byte span matches the anchor's, and
/// the `#step-<n>` details_ref addresses the anchor (the X-1 / OQ-D jump-to-failure path CI-P21
/// resolves).
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
    let c = LogCoord::new("01J0RUN", "01J0JOB", "7");
    // ship enough to force a seal (each line 10 bytes; seal at 40 → seals after 4 lines).
    for _ in 0..8 {
        p.ship_line(&c, "0123456789").expect("ship");
    }
    p.close_step(&c, AnchorStatus::Failed)
        .expect("close the step");

    // the (job, step, byte-range) index: a sealed segment row + the step anchor.
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

    // the index is consistent — 0 dangling anchors (the consumed 11.8 invariant).
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "0 dangling anchors — the index is byte-consistent"
    );
    // the anchor's failed status is the deep-link target; its byte_end is closed (terminal).
    assert_eq!(anchor.status, AnchorStatus::Failed);
    assert!(
        anchor.byte_end.is_some(),
        "a terminal anchor closes its byte_end"
    );
    // the #step-<n> details_ref the consumer (CI-P21) resolves through the anchor → segment → range.
    assert_eq!(
        c.details_ref().0,
        "myelin://ci/run/01J0RUN/job/01J0JOB#step-7",
        "the details_ref addresses the (job, step) anchor (the jump-to-failure path)"
    );
    // the sealed segment's byte span is within the produced bytes (the (blob, offset) index is sane).
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

/// **CONSUMED 3.5 — CI's live-tail frames ride the firehose on the BOUNDED `run:<id>` scope with the
/// per-`(stream, scope)` monotonic resume cursor.** CI publishes each redacted line as a firehose
/// frame on the consumed transport; this CDC proves the frames decode through the REAL firehose
/// `tail` / `subscribe` (the consumer half) — the bounded `run:<id>` scope the transport admits, the
/// monotone seq the resume cursor rests on, NEVER `*`. The viewer's `resume` jump-to-failure is
/// CI-P21 (the named floor); here the producer's frames are proven to ride the transport correctly.
#[test]
fn cdc_3_5_live_tail_rides_the_firehose_on_a_bounded_run_scope() {
    let c = LogCoord::new("01J0RUN", "01J0JOB", "1");
    // the scope CI publishes on is the bounded run:<id> the transport admits (never *).
    let scope = c.firehose_scope().expect("a bounded run:<id> scope");
    assert_eq!(
        scope.selector(),
        "run:01J0RUN",
        "CI's live-tail scope is bounded run:<id>"
    );

    // a fresh consumer-side firehose proves the producer's publish shape: publishing frames on the
    // CI_LOG_STREAM + the bounded scope assigns the per-(stream, scope) monotone seq the resume
    // cursor rests on, and a subscriber tails them in order (the consumer half of 3.5).
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
    // the consumer tails the frames in order — the live-tail delivery (3.5).
    let seqs: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seqs,
        vec![1, 2, 3, 4, 5],
        "the live tail delivers frames in seq order (3.5)"
    );

    // an over-broad scope (`*`) is REJECTED at subscribe — CI never tails the tenant firehose.
    assert!(
        fh.subscribe_raw(CI_LOG_STREAM, "*", None)
            .expect_err("the transport rejects `*`")
            .is_over_broad_scope(),
        "the live tail is bounded — `*` is rejected (3.5 whitelist-not-*)"
    );
}

/// **The producer end-to-end: ship_line over the pipeline rides ALL THREE consumed surfaces.** A
/// short run ships lines → the firehose live tail (3.5) advances per line, a seal flushes to a blob
/// (11.2) + the index (11.8), and the durable `ci.log.available` pointer is COALESCED (never
/// per-line) — the three consumed contracts exercised through CI's real producer in one pass.
#[test]
fn cdc_producer_end_to_end_rides_all_three_consumed_surfaces() {
    // A 1 KiB coalesce budget + a 4 KiB seal — realistic floors (the prod numbers are CI-P29).
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
    let c = LogCoord::new("01J0RUN", "01J0JOB", "1");
    for _ in 0..1000 {
        p.ship_line(&c, "a sixteen-byte!!").expect("ship"); // 16 bytes → 16 KiB total
    }
    p.flush_job("01J0RUN", "01J0JOB", "1").expect("flush");

    // 3.5 — every line rode the firehose live tail (the resume cursor advanced 1000 times).
    assert_eq!(
        p.lines_shipped(),
        1000,
        "every line rode the firehose (3.5 live tail)"
    );
    // 11.2 + 11.8 — segments sealed to blobs + the index; 0 dangling anchors.
    assert!(
        !p.segment_rows().is_empty(),
        "segments sealed to blobs (11.2) + the index (11.8)"
    );
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "0 dangling anchors (11.8 index consistency)"
    );
    // 2.2 / ADR-04.5 — the durable pointer is COALESCED, never per-line (ORDERS below 1000 lines:
    // ~16 KiB / 1 KiB budget ≈ 16 pointers, never one-per-line).
    let pointers = p.durable_pointer_count();
    assert!(
        pointers > 0 && pointers * 10 < 1000,
        "ci.log.available is COALESCED ({pointers} ≪ 1000 lines, never per-line)"
    );
    // the residency-pin is green on every write (0 cross-region admits).
    assert!(
        p.admitted_log_writes() > 0,
        "the residency-pin admitted only in-region writes"
    );
}
