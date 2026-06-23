//! # The CDC pair for CI's live-tail VIEWER (CI-P21 → P-364, M4) — the CONSUMED rows 3.5 + 11.8
//!
//! **Contracts (CONSUMED — the viewer half's no-drift proof):**
//! - **3.5** the FIREHOSE resume-cursor protocol (`myelin_events::firehose::{subscribe, resume}`) —
//!   CI's live-tail VIEWER rides this; this CDC proves the viewer `subscribe`/`resume` decode through
//!   the REAL transport (the bounded `run:<id>` scope, the per-`(stream, scope)` monotone seq, the
//!   `(last_seq, now]` backfill that loses 0 ops, the `resync_required` over-window verdict).
//! - **11.8** the T3 `(job, step, byte-range)` index — the `details_ref` jump-to-failure resolution
//!   (the X-1 / OQ-D `#step-<n>` → byte range path) decodes through the consumed `log_anchor` →
//!   `log_segment` → byte-range index. 0 dangling step anchors.
//!
//! ## What this CDC pins (the provider ↔ consumer no-drift property)
//! - **provider side** — CI-P20's `LogPipeline` (driven IN this file) is the PROVIDER: it publishes
//!   the firehose frames on the bounded `run:<id>` scope and writes the `(job, step, byte-range)`
//!   index (`log_anchor` / `log_segment`). The provider's frozen 3.5 / 11.8 shapes are produced here
//!   by the real pipeline, not a fixture.
//! - **consumer side** — CI-P21's `LiveTail` / `DetailsRefResolver` is the CONSUMER: it
//!   `subscribe`/`resume`s the firehose and resolves the `details_ref` through the same index. If the
//!   provider shape ever drifted from the frozen 3.5 / 11.8 shapes, the consumer decode here would
//!   FAIL (a loud contract break). The sibling producer-side firehose CDC is
//!   `cdc_log_pipeline_3_5_11_8_11_2.rs` (CI-P20); this is the resume-cursor-viewer pair (CI-P21).

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

/// **CONSUMED 3.5 — the viewer `resume` decodes through the REAL firehose, backfilling `(last_seq,
/// now]` with 0 lost ops.** The producer publishes onto the firehose; the viewer resumes from a
/// `last_seq` and the transport replays exactly the gap — the consumer half's no-drift proof for the
/// resume-cursor protocol (if CI's viewer drifted from the frozen `resume` shape, this would fail).
#[test]
fn cdc_3_5_viewer_resume_backfills_the_gap_through_the_real_firehose() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord
        .firehose_scope()
        .expect("the bounded run:<id> the transport admits");

    // The producer publishes onto the SAME bounded scope the viewer subscribes on (no-drift on the key).
    for i in 1..=6u64 {
        let f = fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")));
        assert_eq!(f.seq, i, "the per-(stream, scope) monotone resume cursor");
    }

    // The viewer resumes from last_seq = 3 → backfills (3, now] = {4,5,6} (0 lost).
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

/// **CONSUMED 3.5 — an out-of-window `last_seq` decodes the `resync_required` verdict.** The viewer
/// presents a cursor older than the retention window; the transport raises `resync_required` and the
/// viewer falls back to the sealed-segment range-read — the consumer half decodes the frozen
/// over-window verdict, never a silent partial replay.
#[test]
fn cdc_3_5_viewer_decodes_resync_required_over_window() {
    let mut fh = Firehose::with_limits(3, myelin_events::firehose::DEFAULT_INFLIGHT_CAP);
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded");
    for i in 0..6u64 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("l-{i}")));
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

/// **CONSUMED 11.8 — the `details_ref` jump-to-failure decodes through the `(job, step, byte-range)`
/// index.** CI-P20's producer writes the `log_anchor` + `log_segment` index; CI-P21's resolver decodes
/// a `CheckStatus.details_ref = …/ci/run/<id>#step-<n>` through it to the byte range — the consumer
/// half's no-drift proof for the index shape (0 dangling step anchors).
#[test]
fn cdc_11_8_details_ref_decodes_through_the_job_step_byte_range_index() {
    // Drive CI's REAL producer to write the index.
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
    let c = LogCoord::new("01J0RUN", "01J0JOB", "9");
    for _ in 0..5 {
        p.ship_line(&c, "0123456789").expect("ship");
    }
    p.close_step(&c, AnchorStatus::Failed).expect("close");
    p.flush_job("01J0RUN", "01J0JOB", "9").expect("flush");

    // The viewer decodes the details_ref through the SAME index the producer wrote (no-drift).
    let anchors = p.anchor_rows().into_iter().cloned().collect();
    let segments = p.segment_rows().to_vec();
    let resolver = DetailsRefResolver::new(anchors, segments);

    let details_ref = c.details_ref().0;
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
        "0 dangling step anchors — the index is byte-consistent (11.8)"
    );
}
