//! # The CDC pair for contract 3.5 — the firehose transport + resume-cursor protocol (EB-21 / P-141)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 3.5
//! (Firehose transport + the resume-cursor subscription protocol — `publish`/`tail`;
//! `subscribe(stream, scope, cursor?)`; frames carry a per-`(stream, scope)` monotonic `seq`;
//! `resume(stream, scope, last_seq)` backfills `(last_seq, now]` then live (reconnect loses zero
//! ops); `resync_required → *.snapshot`; scope a bounded selector, never `*`). Owning architecture:
//! `event-bus.md` §4.3 (the protocol — the AUTHORITY), §5.5 (the contract surface). Reconciliation:
//! `00-reconciliation-decisions.md` OQ-J.
//!
//! ## The contract this pair pins (one resume-cursor protocol; board/doc/channel use it identically)
//! Row 3.5 is the **owned-seam** between the side that PUBLISHES firehose frames on a bounded
//! `(stream, scope)` (the **PROVIDER** — CI logs, KN collab op-streams, Chat live delivery) and the
//! side that SUBSCRIBES / RESUMES over the protocol (the **CONSUMER** — the connection tier / each
//! live surface). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER `publish`es to a BOUNDED `(stream, scope)` and the transport assigns the
//!   per-`(stream, scope)` MONOTONIC `seq` (the producer never mints its own seq — the cursor is the
//!   transport's invariant);
//! - the CONSUMER `subscribe`s on a BOUNDED scope (`*` is REJECTED) and, on reconnect, `resume`s with
//!   its `last_seq` to backfill `(last_seq, now]` then go live — **0 ops lost, 0 duplicate**; an
//!   out-of-window `last_seq` yields a `resync_required` it falls back on (NAMED, not silent).
//!
//! This is the dedicated 3.5 provider+consumer pair the EB-21 TESTS field names; the focused
//! per-mechanism unit tests live in `firehose.rs::tests`, the D-10 drill in
//! `tests/drills_eb21_firehose_d10.rs`.

use myelin_events::{
    Firehose, FirehoseError, FirehoseScope, Frame, FrameDraft, DEFAULT_INFLIGHT_CAP,
};

/// **PROVIDER side of 3.5** — a producing subsystem publishes a frame on a bounded `(stream, scope)`.
/// The provider's promise: it publishes to a BOUNDED scope and lets the transport assign the
/// per-`(stream, scope)` monotonic `seq` (it returns the assigned [`Frame`] so the cursor is visible).
fn provider_publishes(
    fh: &mut Firehose,
    stream: &str,
    scope: &FirehoseScope,
    payload: &str,
) -> Frame {
    fh.publish(stream, scope, FrameDraft::new(payload))
}

/// **CONSUMER side of 3.5** — the connection tier subscribes/resumes over the protocol. Returns the
/// frames it received in order (the consumer's promise: it sees the gap then live, contiguously).
fn consumer_resume_drains(
    fh: &mut Firehose,
    stream: &str,
    scope: &FirehoseScope,
    last_seq: u64,
) -> Result<Vec<u64>, FirehoseError> {
    let sub = fh.resume(stream, scope, last_seq)?;
    Ok(sub.drain_ready().iter().map(|f| f.seq).collect())
}

/// The 3.5 pair, end-to-end: a PROVIDER publishes frames on a bounded `(stream, scope)` assigning a
/// monotonic seq; the CONSUMER subscribes live, the connection drops, the provider keeps publishing,
/// and the consumer `resume`s — backfilling the gap then going live, **0 lost, 0 duplicate**.
#[test]
fn cdc_3_5_provider_publishes_consumer_resumes_loses_zero_ops() {
    let mut fh = Firehose::new();
    let stream = "kn-ops";
    let scope = FirehoseScope::parse("doc:design").expect("bounded scope");

    // PROVIDER publishes 1,2,3 with a transport-assigned monotonic seq.
    for (i, p) in ["op-1", "op-2", "op-3"].iter().enumerate() {
        let f = provider_publishes(&mut fh, stream, &scope, p);
        assert_eq!(
            f.seq,
            (i + 1) as u64,
            "the transport assigns the monotonic seq, not the producer"
        );
    }

    // CONSUMER was live up to seq 2, the connection drops, the provider publishes 3,4,5 (already had
    // 3; add 4,5), and the consumer resumes from 2 → gets {3,4,5}.
    provider_publishes(&mut fh, stream, &scope, "op-4");
    provider_publishes(&mut fh, stream, &scope, "op-5");
    let gap = consumer_resume_drains(&mut fh, stream, &scope, 2).expect("in-window resume");
    assert_eq!(
        gap,
        vec![3, 4, 5],
        "the consumer backfills (last_seq, now] — 0 lost, 0 dup"
    );
}

/// The 3.5 pair, the RESYNC leg: the PROVIDER outran the bounded retention window while the CONSUMER
/// was down; the consumer's `resume` yields `resync_required` (the consumer falls back to a
/// `*.snapshot` replay — EB-22). The seam's NAMED cold-rebuild signal, never a silent partial replay.
#[test]
fn cdc_3_5_out_of_window_resume_is_a_named_resync_required() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let stream = "chat-live";
    let scope = FirehoseScope::parse("channel:eng").expect("bounded scope");

    // the provider publishes 8 frames; the window holds only the last 3.
    for _ in 0..8 {
        provider_publishes(&mut fh, stream, &scope, "msg");
    }
    // the consumer (last_seq=2, far behind the window) resumes → resync_required.
    let err = consumer_resume_drains(&mut fh, stream, &scope, 2).expect_err("out-of-window resume");
    assert!(
        err.is_resync_required(),
        "the consumer gets a NAMED resync_required (→ *.snapshot, EB-22)"
    );
}

/// The 3.5 pair, the SCOPE leg: the CONSUMER may only subscribe on a BOUNDED scope; the transport
/// REJECTS an over-broad scope (`*`). The whitelist-not-`*` rule (BUS-3) generalised to the firehose.
#[test]
fn cdc_3_5_consumer_over_broad_scope_is_rejected() {
    let mut fh = Firehose::new();
    let err = fh
        .subscribe_raw("chat-live", "*", None)
        .expect_err("the transport rejects an over-broad scope at subscribe");
    assert!(
        err.is_over_broad_scope(),
        "scope = * is rejected (the protocol's bounded-scope invariant)"
    );
    // the positive control: a bounded scope subscribes.
    assert!(fh.subscribe_raw("chat-live", "doc:x", None).is_ok());
}

/// The 3.5 pair pins the per-`(stream, scope)` seq INDEPENDENCE: two scopes on the same stream have
/// independent monotonic sequences, so a board/doc/channel never shares another's cursor (the
/// head-of-line discipline — a per-view cursor).
#[test]
fn cdc_3_5_per_stream_scope_seq_is_independent() {
    let mut fh = Firehose::new();
    let a = FirehoseScope::parse("board:a").expect("bounded");
    let b = FirehoseScope::parse("board:b").expect("bounded");
    assert_eq!(provider_publishes(&mut fh, "issues", &a, "x").seq, 1);
    assert_eq!(
        provider_publishes(&mut fh, "issues", &b, "y").seq,
        1,
        "b has its own seq"
    );
    assert_eq!(
        provider_publishes(&mut fh, "issues", &a, "z").seq,
        2,
        "a's seq is independent of b"
    );
}
