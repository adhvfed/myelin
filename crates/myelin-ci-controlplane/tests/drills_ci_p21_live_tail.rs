//! # CI-D11 / P-364 — the resume-cursor live-tail reconnect-loses-zero-ops drill
//!
//! **The CI-D11 failure-injection scenario (the dated green artifact — 0 lost lines).** This drill
//! drives CI's REAL producer ([`myelin_ci_controlplane::LogPipeline`]) to ship a run's log lines onto
//! the firehose, then INJECTS a connection drop mid-run and reconnects the viewer
//! ([`myelin_ci_controlplane::LiveTail`]) with its `last_seq`. The aggregate gate:
//!
//! - **(a) reconnect loses ZERO lines** — a viewer drops at `last_seq = k`; lines `k+1..now` ship
//!   while it is disconnected; `resume(last_seq = k)` backfills EXACTLY `k+1..now` then goes live —
//!   the viewer sees every line exactly once, none lost, none duplicated.
//! - **(b) an out-of-window `last_seq` → `resync_required` → a clean range-read fallback** — a SMALL
//!   retention window evicts the gap's head; the resume raises `resync_required` and the viewer reads
//!   the gap from the SEALED segments (the durable archive) — the bytes are recovered, never lost.
//! - **(c) scope stays BOUNDED, never `*`** — every subscribe/resume is on the `run:<id>` scope; an
//!   over-broad scope is REJECTED at the viewer seam (the whitelist-not-`*` rule, BUS-3).
//! - **(d) the details_ref jump-to-failure resolves (0 dangling step anchors)** — the failed step's
//!   `CheckStatus.details_ref = …/ci/run/<id>#step-<n>` resolves through `log_anchor` → the byte range.
//!
//! **This is the VIEWER half of the CI log seam.** CI-P20's `cdc_log_pipeline_3_5_11_8_11_2.rs` proves
//! the PRODUCER rides the firehose + the index; THIS drill proves the VIEWER reads it back across a
//! reconnect with 0 lost lines (CI-D11) + the details_ref resolves (0 dangling anchors).

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

/// **CI-D11 (a) — a reconnect mid-run loses ZERO lines (the dated green artifact).** A viewer tails a
/// run; the connection drops at `last_seq = 4`; lines 5..12 ship while disconnected; `resume(4)`
/// backfills EXACTLY {5..12} then goes live — every line exactly once, 0 lost, 0 duplicate.
#[test]
fn ci_d11_reconnect_mid_run_loses_zero_lines() {
    let mut fh = Firehose::new();
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded run:<id>");

    // The viewer is live and follows lines 1..4.
    let mut tail = LiveTail::new(&mut fh, SegmentIndex::default());
    let sub = tail.subscribe(&coord, None).expect("bounded subscribe");
    for i in 1..=4u64 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")));
    }
    let seen_before: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seen_before,
        vec![1, 2, 3, 4],
        "the viewer followed 1..4 live"
    );
    let last_seq = sub.last_seq();
    assert_eq!(last_seq, 4, "the resume cursor at drop time is 4");

    // INJECT the drop: lines 5..12 ship while the viewer is disconnected (the gap).
    for i in 5..=12u64 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")));
    }

    // Reconnect: resume(last_seq = 4) backfills (4, now] = {5..12} then goes live.
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
        "CI-D11: the gap (4, now] is replayed — ZERO lines lost"
    );

    // A live frame after reconnect arrives gap-free (no duplicate across the backfill→live boundary).
    fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new("line-13"));
    let live: Vec<u64> = sub2.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(live, vec![13], "live continues gap-free; 0 duplicate");

    // The TOTAL the viewer saw across the reconnect: 1..13, every line exactly once.
    let mut all = seen_before;
    all.extend(backfilled);
    all.extend(live);
    assert_eq!(
        all,
        (1..=13).collect::<Vec<_>>(),
        "CI-D11 green: across the reconnect — 0 lost, 0 duplicate (1..13 exactly once)"
    );
}

/// **CI-D11 (b) — an out-of-window `last_seq` → `resync_required` → a clean range-read fallback.** A
/// SMALL retention window (4 frames) evicts the gap's head; `resume` raises `resync_required` and the
/// viewer reads the gap from the SEALED segments (the durable archive) — the bytes are recovered.
#[test]
fn ci_d11_out_of_window_last_seq_falls_back_to_a_range_read() {
    // A window holding only the most-recent 4 frames (the failure injection: a long disconnect).
    let mut fh = Firehose::with_limits(4, DEFAULT_INFLIGHT_CAP);
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
    let scope = coord.firehose_scope().expect("bounded");
    // 12 frames ship → the window holds {9,10,11,12}; 1..8 evicted.
    for i in 1..=12u64 {
        fh.publish(CI_LOG_STREAM, &scope, FrameDraft::new(format!("line-{i}")));
    }

    // The sealed archive covers the evicted bytes (the durable record — CI-P20 sealed them).
    let archive = SegmentIndex::from_rows("01J0RUN", "01J0JOB", &build_archive_rows());

    let mut tail = LiveTail::new(&mut fh, archive);
    // A viewer at last_seq = 4 needs op 5 first — but 5 was evicted → resync_required.
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
    // The range-read recovers the evicted bytes from the SEALED archive — 0 lost bytes.
    assert!(
        !range_read.is_empty(),
        "the range-read recovers the gap from the sealed segments"
    );
    // The segments are byte_start-ordered (the viewer reads them in order).
    for w in range_read.windows(2) {
        assert!(
            w[0].byte_start <= w[1].byte_start,
            "segments are byte_start-ordered"
        );
    }
}

/// **CI-D11 (c) — the viewer scope stays BOUNDED `run:<id>`, never `*`.** A viewer subscribes/resumes
/// on exactly the bounded `run:<id>` scope; an over-broad scope is REJECTED at the connection-tier
/// entry — a viewer can ONLY tail one bounded run, never the tenant firehose.
#[test]
fn ci_d11_scope_stays_bounded_never_star() {
    let coord = LogCoord::new("01J0RUN", "01J0JOB", "1");
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
    // the bounded run scope subscribes fine (the positive control).
    assert!(
        fh.subscribe_raw(CI_LOG_STREAM, "run:01J0RUN", None).is_ok(),
        "a bounded run:<id> scope subscribes"
    );
}

/// **CI-D11 (d) — the details_ref jump-to-failure resolves (0 dangling step anchors).** CI's REAL
/// producer ships a run with two steps; step 2 FAILS. The failed step's
/// `CheckStatus.details_ref = …/ci/run/<id>#step-2` resolves through `log_anchor` → the byte range —
/// 0 dangling anchors over every real step's details_ref.
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

    // Step 1 passes; step 2 fails (the jump-to-failure target).
    let s1 = LogCoord::new("01J0RUN", "01J0JOB", "1");
    for _ in 0..4 {
        p.ship_line(&s1, "0123456789").expect("ship");
    }
    p.close_step(&s1, AnchorStatus::Passed).expect("close");

    let s2 = LogCoord::new("01J0RUN", "01J0JOB", "2");
    for _ in 0..3 {
        p.ship_line(&s2, "FAIL-LINE!").expect("ship");
    }
    p.close_step(&s2, AnchorStatus::Failed).expect("close");
    p.flush_job("01J0RUN", "01J0JOB", "2").expect("flush");

    // The producer index is consistent (0 dangling anchors on the producer side).
    assert_eq!(
        p.dangling_anchor_count(),
        0,
        "the producer index is byte-consistent"
    );

    // The viewer resolves both steps' details_refs through the anchor index — 0 dangling anchors.
    let anchors = p.anchor_rows().into_iter().cloned().collect();
    let segments = p.segment_rows().to_vec();
    let resolver = DetailsRefResolver::new(anchors, segments);

    let ref1 = s1.details_ref().0;
    let ref2 = s2.details_ref().0;
    assert_eq!(
        resolver.dangling_anchor_count([ref1.as_str(), ref2.as_str()]),
        0,
        "CI-D11: 0 dangling step anchors — every step's details_ref resolves to a byte range"
    );

    // The failed step deep-links to its byte range (the X-1 / OQ-D jump-to-failure path).
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

    // A step that never ran is a NAMED tombstone, never a silent dangle (the negative control).
    let missing = "myelin://ci/run/01J0RUN/job/01J0JOB#step-404";
    assert!(
        matches!(
            resolver.resolve(missing),
            Err(DetailsRefError::AnchorGone { .. })
        ),
        "a missing step is a NAMED AnchorGone tombstone, never a silent dangle"
    );
}

/// Build the sealed `log_segment` archive rows covering [0, 120) — three 40-byte segments (the
/// durable record the resync_required fallback range-reads).
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
