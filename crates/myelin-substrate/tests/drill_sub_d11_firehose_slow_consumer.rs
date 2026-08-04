use myelin_harness::{
    Dependency, DrillContext, DrillRegistry, DrillResult, DrillScenario, Label, Predicate, Scope,
    SignalName,
};
use myelin_substrate::{
    FirehoseScope, FirehoseSignals, Frame, FrameBuffer, FrameClass, PushOutcome,
};

fn scope(s: &str) -> FirehoseScope {
    FirehoseScope(s.to_string())
}

fn human(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::HumanDelivery)
}

fn sub_d11_firehose_slow_consumer_scenario() -> DrillScenario {
    DrillScenario::new(
        "sub-d11-firehose-slow-consumer",
        |ctx: &mut DrillContext| {
            ctx.breaker
                .break_dependency(Dependency::Firehose, Scope::Global);

            let mut keeping_up = FrameBuffer::new("chat-live", scope("channel:hot-fast"), 4, 8);
            let mut slow = FrameBuffer::new("chat-live", scope("channel:hot-slow"), 4, 8);

            for seq in 1..=64u64 {
                assert!(
                    keeping_up.offer(human(seq)).is_buffered(),
                    "the keeping-up consumer never sheds (it keeps pace)"
                );
                keeping_up.deliver(human(seq));
            }

            let mut drop_seen = false;
            for seq in 1..=20u64 {
                match slow.offer(human(seq)) {
                    PushOutcome::ResyncRequired => drop_seen = true,
                    PushOutcome::Buffered | PushOutcome::Shed => {
                        assert!(
                            slow.buffered_frames() <= slow.capacity(),
                            "memory NEVER exceeds the per-connection cap (Little's Law): seq={seq}"
                        );
                    }
                }
            }
            assert!(
                drop_seen,
                "the slow consumer must be dropped to resync_required"
            );
            assert_eq!(
                slow.buffered_frames(),
                0,
                "a dropped connection releases its buffer (bounded memory)"
            );
            assert_eq!(
                slow.frame_lag(),
                0,
                "a dropped connection holds no gap (it is in *.snapshot replay)"
            );
            assert_eq!(
                slow.resync_required_count(),
                1,
                "the resync_required drop is counted EXACTLY once"
            );

            assert!(
                !keeping_up.resync_required(),
                "a slow consumer never drops a keeping-up neighbour"
            );

            let sig = FirehoseSignals::snapshot([&keeping_up, &slow]);
            for row in &sig.frame_lag {
                ctx.signals.set_labelled(
                    SignalName::FirehoseFrameLag,
                    vec![
                        Label::new("stream", row.stream.clone()),
                        Label::new("scope", row.scope.clone()),
                    ],
                    row.lag as i64,
                );
            }
            ctx.signals.set_scalar(
                SignalName::ResyncRequiredCount,
                sig.resync_required_count as i64,
            );

            ctx.breaker
                .restore_dependency(Dependency::Firehose, Scope::Global);

            ctx.signals.assert_labelled(
                SignalName::FirehoseFrameLag,
                vec![
                    Label::new("stream", "chat-live".to_string()),
                    Label::new("scope", "channel:hot-slow".to_string()),
                ],
                Predicate::Lte(8),
            )
        },
    )
}

#[test]
fn sub_d11_firehose_slow_consumer_is_dropped_green_artifact() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(sub_d11_firehose_slow_consumer_scenario());

    let results = registry.run_all();
    assert_eq!(results.len(), 1);
    let result = &results[0];

    assert!(
        result.is_pass(),
        "SUB-D11: a slow firehose consumer must be DROPPED to resync_required with bounded memory + a \
         bounded frame-lag (the firehose survival signals read green): {result:?}"
    );

    let row = result.artifact_row("2026-06-20");
    assert_eq!(
        row,
        "[2026-06-20] PASS  drill=sub-d11-firehose-slow-consumer  (inject → load → assert green)"
    );
    println!("{row}");
}

#[test]
fn sub_d11_both_firehose_survival_signals_read_green() {
    let scenario = sub_d11_firehose_slow_consumer_scenario();
    let result = scenario.run_once();
    match result {
        DrillResult::Pass { .. } => {
            let mut keeping_up = FrameBuffer::new("chat-live", scope("channel:hot-fast"), 4, 8);
            let mut slow = FrameBuffer::new("chat-live", scope("channel:hot-slow"), 4, 8);
            for seq in 1..=64u64 {
                keeping_up.offer(human(seq));
                keeping_up.deliver(human(seq));
            }
            for seq in 1..=20u64 {
                slow.offer(human(seq));
            }
            let sig = FirehoseSignals::snapshot([&keeping_up, &slow]);
            let mut ctx = DrillContext::new();
            ctx.signals.set_scalar(
                SignalName::ResyncRequiredCount,
                sig.resync_required_count as i64,
            );
            ctx.signals
                .assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(1))
                .expect_green();
            assert!(
                sig.max_frame_lag() <= 8,
                "every (stream,scope) frame-lag is BOUNDED"
            );
        }
        other => panic!("SUB-D11 must pass: {other:?}"),
    }
}
