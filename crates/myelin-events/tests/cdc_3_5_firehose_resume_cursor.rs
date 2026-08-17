use myelin_events::{
    Firehose, FirehoseError, FirehoseScope, Frame, FrameDraft, DEFAULT_INFLIGHT_CAP,
};

fn provider_publishes(
    fh: &mut Firehose,
    stream: &str,
    scope: &FirehoseScope,
    payload: &str,
) -> Frame {
    fh.publish(stream, scope, FrameDraft::new(payload))
        .expect("the fixture publishes a valid frame")
}

fn consumer_resume_drains(
    fh: &mut Firehose,
    stream: &str,
    scope: &FirehoseScope,
    last_seq: u64,
) -> Result<Vec<u64>, FirehoseError> {
    let sub = fh.resume(stream, scope, last_seq)?;
    Ok(sub.drain_ready().iter().map(|f| f.seq).collect())
}

#[test]
fn cdc_3_5_provider_publishes_consumer_resumes_loses_zero_ops() {
    let mut fh = Firehose::new();
    let stream = "kn-ops";
    let scope = FirehoseScope::parse("doc:design").expect("bounded scope");

    for (i, p) in ["op-1", "op-2", "op-3"].iter().enumerate() {
        let f = provider_publishes(&mut fh, stream, &scope, p);
        assert_eq!(
            f.seq,
            (i + 1) as u64,
            "the transport assigns the monotonic seq, not the producer"
        );
    }

    provider_publishes(&mut fh, stream, &scope, "op-4");
    provider_publishes(&mut fh, stream, &scope, "op-5");
    let gap = consumer_resume_drains(&mut fh, stream, &scope, 2).expect("in-window resume");
    assert_eq!(
        gap,
        vec![3, 4, 5],
        "the consumer backfills (last_seq, now] - 0 lost, 0 dup"
    );
}

#[test]
fn cdc_3_5_out_of_window_resume_is_a_named_resync_required() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let stream = "chat-live";
    let scope = FirehoseScope::parse("channel:eng").expect("bounded scope");

    for _ in 0..8 {
        provider_publishes(&mut fh, stream, &scope, "msg");
    }
    let err = consumer_resume_drains(&mut fh, stream, &scope, 2).expect_err("out-of-window resume");
    assert!(
        err.is_resync_required(),
        "the consumer gets a NAMED resync_required (→ *.snapshot, EB-22)"
    );
}

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
    assert!(fh.subscribe_raw("chat-live", "doc:x", None).is_ok());
}

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
