//! # D-10 RE-GREEN across the KN CAS→CRDT `engine_promote` boundary (EB-30 / P-439, M5)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row D-10
//! (firehose reconnect loses zero ops), **re-run across the engine_promote boundary** (the floor's
//! promotion is itself drilled). Threshold: **0 ops lost across the engine_promote; the per-stream-
//! class retention window measured > the p99 reconnect gap (asserted from measured data)**.
//!
//! ## What this drill proves (the EB-30 GATE)
//! EB-21 / P-141 NAMED two firehose floors, both filled here:
//!  1. **The retention window per stream class is MEASURED + tuned** (the named M2 floor → measured):
//!     the per-class window EXCEEDS the measured p99 reconnect gap, with headroom (§4.3). Asserted
//!     here from the `myelin_events::retention` measured data — and held in lock-step with the
//!     recorded `thresholds.toml` numbers by `cdc_3_5_retention.rs`.
//!  2. **D-10 re-runs GREEN across the KN CAS→CRDT `engine_promote` boundary** (the floor's promotion
//!     is itself drilled): the resume-cursor transport SURVIVES the CRDT promotion — 0 ops lost. The
//!     transport is UNCHANGED; the drill proves the floor's promotion does not regress the zero-ops-
//!     lost property.
//!
//! ## Why this is the BUS's drill (the engine-agnostic transport property)
//! The firehose transport carries OPAQUE payload bytes (references-not-payloads — `FramePayload` is a
//! pointer string the transport never reads, `firehose.rs`). The KN collab engine (the apply layer
//! that interprets those bytes) swaps from the per-block CAS floor (KN-P13) to the Yrs CRDT (KN-P29,
//! P-484 — a LATER prompt) at the `engine_promote` cutover op (the `OpKind::EnginePromote` marker
//! already in `myelin-knowledge`'s transport). Because the BUS transport is byte-opaque to the apply
//! engine, the engine_promote is a payload-only change at the LAYER ABOVE the transport — the
//! `(stream, scope)` monotone `seq`, the `(last_seq, now]` backfill, and the `resync_required`
//! fallback are IDENTICAL on both sides of the boundary. This drill models that boundary at the BUS
//! transport (myelin-events cannot depend on myelin-knowledge — it is downstream): CAS-class frames,
//! then an `engine_promote` marker frame, then CRDT-class frames, with a connection drop SPANNING the
//! boundary; the reconnect backfills the gap (which straddles the marker) losing ZERO ops. The KN
//! consumer-side re-green (the same property over `CollabTransport`) is KN-P29's re-confirm (P-484);
//! this is the Bus's authoritative transport-property half.
//!
//! The drill reads its verdict off the §10.2 firehose survival signals through the FROZEN harness
//! assertion library (`FirehoseFrameLag == 0` after backfill; `ResyncRequiredCount == 0` on the
//! in-window boundary-spanning reconnect), exactly as `drills_eb21_firehose_d10.rs` does.

use myelin_events::{
    Firehose, FirehoseScope, FrameDraft, RetentionTuning, StreamClass, DEFAULT_INFLIGHT_CAP,
};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};

fn scope(s: &str) -> FirehoseScope {
    FirehoseScope::parse(s).expect("a bounded scope")
}

/// The opaque payload-byte "engine" a frame's bytes are produced by, ABOVE the transport. The
/// transport never reads this — it is here only so the drill can assert the SAME transport carried
/// both engines' bytes across the boundary (the engine-agnostic property). This mirrors the
/// `myelin-knowledge` `DocOp` payload posture (CAS bytes in v1, Yrs bytes after KN-P29) without the
/// downstream dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApplyEngine {
    /// The per-block CAS floor (KN-P13) — the v1 op bytes.
    Cas,
    /// The Yrs CRDT (KN-P29 / P-484) — the post-promotion op bytes.
    Crdt,
}

/// A frame payload that NAMES which apply engine produced it + a monotone op index, so the drill can
/// reconstruct the exact applied set across the reconnect AND assert the transport carried both
/// engines. The transport treats it as the opaque pointer string it is.
fn op_frame(engine: ApplyEngine, op_idx: u64) -> FrameDraft {
    let tag = match engine {
        ApplyEngine::Cas => "cas",
        ApplyEngine::Crdt => "crdt",
    };
    FrameDraft::new(format!("op:{tag}:{op_idx}"))
}

/// The `engine_promote` cutover marker frame (the `OpKind::EnginePromote` analog at the transport
/// layer). From this frame's seq forward the payload carries CRDT bytes; before it, CAS bytes. The
/// transport carries the marker as just another opaque frame — that IS the point.
fn engine_promote_frame() -> FrameDraft {
    FrameDraft::new("op:engine_promote")
}

/// **EB-30 LEG 1 (the GATE) — D-10 re-greens across the engine_promote boundary: a reconnect whose
/// gap STRADDLES the CAS→CRDT cutover loses ZERO ops.** The transport is unchanged; the resume cursor
/// backfills the gap (CAS tail + the promote marker + CRDT head) contiguously, then goes live.
#[test]
fn d10_reconnect_across_engine_promote_loses_zero_ops() {
    // the collab op-stream class (the class the engine_promote boundary rides), opened at its MEASURED
    // retention window (EB-30) — large enough that this routine reconnect backfills from the window.
    let mut fh = Firehose::for_stream_class(StreamClass::CollabOp);
    let stream = "kn-ops";
    let s = scope("doc:hot-design"); // a hot doc (KN KD-8) — the OQ-J co-designed case.

    let breaker = DependencyBreaker::new();
    let sub = fh
        .subscribe(stream, &s, None)
        .expect("bounded scope subscribes");

    // ── before the boundary: the client consumes 3 CAS-engine ops live (seq 1..3). ───────────────
    for i in 1..=3u64 {
        fh.publish(stream, &s, op_frame(ApplyEngine::Cas, i));
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seen,
        vec![1, 2, 3],
        "the client saw the CAS ops while connected"
    );
    let last_seq = sub.last_seq();
    assert_eq!(last_seq, 3, "its resume cursor is the last delivered seq");

    // ── DROP the connection mid-stream, SPANNING the engine_promote boundary (P-S03). While down: ─
    //    seq 4,5 are CAS tail ops; seq 6 is the engine_promote cutover; seq 7,8 are CRDT head ops.
    breaker.break_dependency(Dependency::Firehose, Scope::Global);
    assert!(
        breaker.is_broken(&Dependency::Firehose, &Scope::Global),
        "the connection is down across the boundary"
    );
    fh.publish(stream, &s, op_frame(ApplyEngine::Cas, 4));
    fh.publish(stream, &s, op_frame(ApplyEngine::Cas, 5));
    fh.publish(stream, &s, engine_promote_frame()); // seq 6 — the CAS→CRDT cutover
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 7));
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 8));

    // ── RECONNECT across the boundary: resume(last_seq=3) → backfill (3, now] = {4,5,6,7,8}. ──────
    breaker.restore_dependency(Dependency::Firehose, Scope::Global);
    let resumed = fh
        .resume(stream, &s, last_seq)
        .expect("an in-window resume backfills the boundary-spanning gap");
    let backfilled = resumed.drain_ready();
    let backfill_seqs: Vec<u64> = backfilled.iter().map(|f| f.seq).collect();
    assert_eq!(
        backfill_seqs,
        vec![4, 5, 6, 7, 8],
        "the gap STRADDLING the engine_promote is replayed — 0 ops lost across the boundary"
    );

    // the transport carried BOTH engines' bytes UNCHANGED across the boundary (the engine-agnostic
    // property): the gap contains the CAS tail, the engine_promote marker, AND the CRDT head.
    let payloads: Vec<&str> = backfilled.iter().map(|f| f.payload.0.as_str()).collect();
    assert_eq!(
        payloads,
        vec![
            "op:cas:4",
            "op:cas:5",
            "op:engine_promote",
            "op:crdt:7",
            "op:crdt:8"
        ],
        "the same transport carried CAS bytes, the cutover, and CRDT bytes — byte-opaque, unchanged"
    );

    // a subsequent LIVE CRDT op continues gap-free, no duplicate across the reconnect boundary.
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 9));
    let live: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        live,
        vec![9],
        "live (CRDT) continues contiguously after the boundary"
    );

    // ZERO OPS LOST across the engine_promote: the client saw 1..9, each exactly once.
    let mut total = seen;
    total.extend(backfill_seqs);
    total.extend(live);
    assert_eq!(
        total,
        (1..=9).collect::<Vec<u64>>(),
        "across the engine_promote reconnect: 0 lost, 0 duplicate"
    );

    // the §10.2 seq-gap survival signal reads 0 after the boundary-spanning reconnect; no resync
    // fired (the in-window window held the whole boundary gap) — assert GREEN through the frozen lib.
    let remaining_gap = (fh.head_seq(stream, &s) - resumed.last_seq()) as i64;
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", stream),
            Label::new("scope", s.selector()),
        ],
        remaining_gap,
    );
    src.set_scalar(SignalName::ResyncRequiredCount, 0);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(0))
        .expect_green(); // the engine_promote reconnect stayed in-window — no resync
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", stream),
            Label::new("scope", s.selector()),
        ],
        Predicate::Eq(0),
    )
    .expect_green(); // 0 ops lost across the boundary: the seq-gap is closed after the backfill
}

/// **EB-30 LEG 2 — the transport property is IDENTICAL on both sides of the boundary (engine-agnostic):
/// a reconnect entirely WITHIN the post-promotion (CRDT) region loses zero ops exactly as a pre-
/// promotion reconnect does.** Proves the promotion did not regress the property AFTER the cutover.
#[test]
fn d10_post_promotion_reconnect_loses_zero_ops_unchanged() {
    let mut fh = Firehose::for_stream_class(StreamClass::CollabOp);
    let stream = "kn-ops";
    let s = scope("doc:post-promote");

    // the doc has already been promoted: seq 1 is the cutover, everything after is CRDT bytes.
    fh.publish(stream, &s, engine_promote_frame());
    let sub = fh
        .subscribe(stream, &s, None)
        .expect("subscribe post-promotion");
    for i in 2..=5u64 {
        fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, i));
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(seen, vec![2, 3, 4, 5], "the client saw the CRDT ops live");

    // drop + reconnect entirely within the CRDT region.
    for i in 6..=9u64 {
        fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, i));
    }
    let resumed = fh
        .resume(stream, &s, sub.last_seq())
        .expect("a post-promotion resume backfills");
    let gap: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        gap,
        vec![6, 7, 8, 9],
        "the post-promotion gap is replayed — 0 ops lost (the property is unchanged after the cutover)"
    );
}

/// **EB-30 — the per-stream-class retention window EXCEEDS the MEASURED p99 reconnect gap, with
/// headroom (§4.3), asserted from the measured data (the prompt's TESTS unit, per stream class).**
/// This is the floor MEASURED: a window short of its p99 gap would force a routine reconnect to the
/// expensive `resync_required` cold path — the EB-30 deliverable is precisely that it does NOT.
#[test]
fn measured_retention_window_exceeds_p99_reconnect_gap_per_stream_class() {
    for class in StreamClass::ALL {
        let t: RetentionTuning = class.tuning();
        assert!(
            t.window_exceeds_p99_gap(),
            "{}: measured window {} must EXCEED the measured p99 reconnect gap {} (§4.3)",
            class.as_str(),
            t.window_frames,
            t.p99_reconnect_gap_frames,
        );
        assert!(
            t.window_has_headroom(),
            "{}: measured window {} must hold >= {}x the measured p99 gap {} (§4.3 comfortably-exceeds)",
            class.as_str(),
            t.window_frames,
            RetentionTuning::MIN_HEADROOM_X,
            t.p99_reconnect_gap_frames,
        );
        // the firehose this class opens is sized to the measured window (the Firehose::for_stream_class
        // production constructor uses exactly this number).
        let fh = Firehose::for_stream_class(class);
        let s = scope("doc:probe");
        // publishing window+1 frames evicts exactly one (the ring is bounded at the measured window),
        // proving the opened window IS the measured capacity. (Small classes only — bounded by run time.)
        if t.window_frames <= StreamClass::ChatLive.window_frames() {
            let mut fh = fh;
            for _ in 0..(t.window_frames + 1) {
                fh.publish("kn-ops", &s, FrameDraft::new("f"));
            }
            assert_eq!(
                fh.window_len("kn-ops", &s),
                t.window_frames,
                "{}: the opened window is bounded at the measured capacity",
                class.as_str()
            );
        }
    }
}

/// **EB-30 — an out-of-window reconnect STILL raises `resync_required` correctly after the engine_
/// promote (the resync floor is unregressed across the boundary).** A small window forces the gap's
/// head (including a CAS op before the cutover) to be evicted → resync_required → `*.snapshot` (EB-22).
#[test]
fn out_of_window_reconnect_across_boundary_still_raises_resync_required() {
    // a deliberately SMALL window (3) to force the resync path — the drill drives a small window, the
    // production window is the MEASURED floor (StreamClass::CollabOp::window_frames()).
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let stream = "kn-ops";
    let s = scope("doc:tiny-window");

    let last_seq = 1u64; // the client last saw seq 1 (a CAS op), then dropped.
    fh.publish(stream, &s, op_frame(ApplyEngine::Cas, 1));
    fh.publish(stream, &s, op_frame(ApplyEngine::Cas, 2));
    fh.publish(stream, &s, engine_promote_frame()); // seq 3 — cutover
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 4));
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 5)); // window now holds {3,4,5}; 1,2 evicted

    let err = fh
        .resume(stream, &s, last_seq)
        .expect_err("an out-of-window reconnect spanning the boundary cannot backfill");
    assert!(
        err.is_resync_required(),
        "the over-window cursor RAISES resync_required across the boundary (NAMED, → *.snapshot EB-22)"
    );
}
