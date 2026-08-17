use myelin_events::{
    Firehose, FirehoseScope, FrameDraft, RetentionTuning, StreamClass, DEFAULT_INFLIGHT_CAP,
};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};

fn scope(s: &str) -> FirehoseScope {
    FirehoseScope::parse(s).expect("a bounded scope")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApplyEngine {
    Cas,
    Crdt,
}

fn op_frame(engine: ApplyEngine, op_idx: u64) -> FrameDraft {
    let tag = match engine {
        ApplyEngine::Cas => "cas",
        ApplyEngine::Crdt => "crdt",
    };
    FrameDraft::new(format!("op:{tag}:{op_idx}"))
}

fn engine_promote_frame() -> FrameDraft {
    FrameDraft::new("op:engine_promote")
}

#[test]
fn d10_reconnect_across_engine_promote_loses_zero_ops() {
    let mut fh = Firehose::for_stream_class(StreamClass::CollabOp);
    let stream = "kn-ops";
    let s = scope("doc:hot-design");

    let breaker = DependencyBreaker::new();
    let sub = fh
        .subscribe(stream, &s, None)
        .expect("bounded scope subscribes");

    for i in 1..=3u64 {
        fh.publish(stream, &s, op_frame(ApplyEngine::Cas, i))
            .expect("the fixture publishes a valid frame");
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seen,
        vec![1, 2, 3],
        "the client saw the CAS ops while connected"
    );
    let last_seq = sub.last_seq();
    assert_eq!(last_seq, 3, "its resume cursor is the last delivered seq");

    breaker.break_dependency(Dependency::Firehose, Scope::Global);
    assert!(
        breaker.is_broken(&Dependency::Firehose, &Scope::Global),
        "the connection is down across the boundary"
    );
    fh.publish(stream, &s, op_frame(ApplyEngine::Cas, 4))
        .expect("the fixture publishes a valid frame");
    fh.publish(stream, &s, op_frame(ApplyEngine::Cas, 5))
        .expect("the fixture publishes a valid frame");
    fh.publish(stream, &s, engine_promote_frame())
        .expect("the fixture publishes a valid frame");
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 7))
        .expect("the fixture publishes a valid frame");
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 8))
        .expect("the fixture publishes a valid frame");

    breaker.restore_dependency(Dependency::Firehose, Scope::Global);
    let resumed = fh
        .resume(stream, &s, last_seq)
        .expect("an in-window resume backfills the boundary-spanning gap");
    let backfilled = resumed.drain_ready();
    let backfill_seqs: Vec<u64> = backfilled.iter().map(|f| f.seq).collect();
    assert_eq!(
        backfill_seqs,
        vec![4, 5, 6, 7, 8],
        "the gap STRADDLING the engine_promote is replayed - 0 ops lost across the boundary"
    );

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
        "the same transport carried CAS bytes, the cutover, and CRDT bytes - byte-opaque, unchanged"
    );

    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 9))
        .expect("the fixture publishes a valid frame");
    let live: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        live,
        vec![9],
        "live (CRDT) continues contiguously after the boundary"
    );

    let mut total = seen;
    total.extend(backfill_seqs);
    total.extend(live);
    assert_eq!(
        total,
        (1..=9).collect::<Vec<u64>>(),
        "across the engine_promote reconnect: 0 lost, 0 duplicate"
    );

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
        .expect_green();
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", stream),
            Label::new("scope", s.selector()),
        ],
        Predicate::Eq(0),
    )
    .expect_green();
}

#[test]
fn d10_post_promotion_reconnect_loses_zero_ops_unchanged() {
    let mut fh = Firehose::for_stream_class(StreamClass::CollabOp);
    let stream = "kn-ops";
    let s = scope("doc:post-promote");

    fh.publish(stream, &s, engine_promote_frame())
        .expect("the fixture publishes a valid frame");
    let sub = fh
        .subscribe(stream, &s, None)
        .expect("subscribe post-promotion");
    for i in 2..=5u64 {
        fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, i))
            .expect("the fixture publishes a valid frame");
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(seen, vec![2, 3, 4, 5], "the client saw the CRDT ops live");

    for i in 6..=9u64 {
        fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, i))
            .expect("the fixture publishes a valid frame");
    }
    let resumed = fh
        .resume(stream, &s, sub.last_seq())
        .expect("a post-promotion resume backfills");
    let gap: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        gap,
        vec![6, 7, 8, 9],
        "the post-promotion gap is replayed - 0 ops lost (the property is unchanged after the cutover)"
    );
}

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
        let fh = Firehose::for_stream_class(class);
        let s = scope("doc:probe");
        if t.window_frames <= StreamClass::ChatLive.window_frames() {
            let mut fh = fh;
            for _ in 0..(t.window_frames + 1) {
                fh.publish("kn-ops", &s, FrameDraft::new("f"))
                    .expect("the fixture publishes a valid frame");
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

#[test]
fn out_of_window_reconnect_across_boundary_still_raises_resync_required() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let stream = "kn-ops";
    let s = scope("doc:tiny-window");

    let last_seq = 1u64;
    fh.publish(stream, &s, op_frame(ApplyEngine::Cas, 1))
        .expect("the fixture publishes a valid frame");
    fh.publish(stream, &s, op_frame(ApplyEngine::Cas, 2))
        .expect("the fixture publishes a valid frame");
    fh.publish(stream, &s, engine_promote_frame())
        .expect("the fixture publishes a valid frame");
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 4))
        .expect("the fixture publishes a valid frame");
    fh.publish(stream, &s, op_frame(ApplyEngine::Crdt, 5))
        .expect("the fixture publishes a valid frame");

    let err = fh
        .resume(stream, &s, last_seq)
        .expect_err("an out-of-window reconnect spanning the boundary cannot backfill");
    assert!(
        err.is_resync_required(),
        "the over-window cursor RAISES resync_required across the boundary (NAMED, → *.snapshot EB-22)"
    );
}
