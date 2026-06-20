//! # SUB-D11 — the firehose hot-stream slow-consumer drill (P-S28 → global P-135)
//!
//! **Drill catalogue:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §11 row **D-11** (*Firehose reconnect-loses-zero-ops*): "Drop a firehose subscription mid-stream on
//! a hot board/doc/channel; assert … a slow consumer is dropped (not buffered unboundedly)." This is
//! the **substrate's half** — the bounded-queue half (§7.7): the substrate makes a slow consumer drop
//! to `resync_required` with bounded memory + accurate survival signals. The **Bus owns the
//! zero-loss-replay assertion** (`resume(last_seq)` backfills the gap with zero ops lost) — **P-141
//! (EB-21)**, named here, not asserted (the Bus firehose protocol has not landed yet — this prompt is
//! the substrate half ahead of the interleaved Bus half).
//!
//! It is the EI-01 §3 drill shape: *inject a fault (the P-S03 `break_dependency` drops a firehose
//! subscription on a hot stream), drive one unit of load (a fast producer racing a slow consumer), read
//! one telemetry assertion that reads green (the P-S04 library).* Here:
//!   - **inject** — `break_dependency(Dependency::Firehose, …)` drops a firehose connection mid-stream
//!     on a HOT stream (the realistic D-11 condition).
//!   - **load** — a fast producer on a hot `channel:` floods a consumer that has stalled (a slow
//!     consumer); plus a second, keeping-up consumer on the same stream (so the drill proves the drop
//!     is per-CONNECTION, not stream-wide — a slow consumer never sheds a keeping-up neighbour).
//!   - **assert** — the two §10.2 firehose survival signals read green: `firehose_frame_lag` is
//!     **BOUNDED** (≤ the slow-consumer ceiling — memory never grew unboundedly) for every
//!     `(stream,scope)`, and `resync_required_count == 1` (the slow consumer was dropped, NAMED, and
//!     counted accurately — exactly one drop). The slow connection holds 0 buffered frames after the
//!     drop (memory released); the keeping-up connection is untouched.
//!
//! The full D-11 reconnect-loses-zero-ops proof (zero ops lost across the reconnect) re-runs with the
//! Bus firehose protocol (P-141) + the M4 connection tier (the M4 connection-storm re-confirm is P-S31
//! / P-326); this drill proves the *substrate* bounded-and-sheds machinery against synthetic hot load.

use myelin_harness::{
    Dependency, DrillContext, DrillRegistry, DrillResult, DrillScenario, Label, Predicate, Scope,
    SignalName,
};
use myelin_substrate::{FirehoseScope, FirehoseSignals, Frame, FrameBuffer, FrameClass, PushOutcome};

fn scope(s: &str) -> FirehoseScope {
    FirehoseScope(s.to_string())
}

fn human(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::HumanDelivery)
}

/// The SUB-D11 scenario: under an injected firehose-subscription drop on a HOT stream, race a fast
/// producer against a slow consumer (and a keeping-up neighbour), then assert the two firehose
/// survival signals read green (frame-lag bounded + resync_required accurate).
fn sub_d11_firehose_slow_consumer_scenario() -> DrillScenario {
    DrillScenario::new("sub-d11-firehose-slow-consumer", |ctx: &mut DrillContext| {
        // (inject) drop a firehose subscription mid-stream on a hot stream (the D-11 condition).
        ctx.breaker
            .break_dependency(Dependency::Firehose, Scope::Global);

        // The hot stream carries two connections on two scopes: one consumer keeps up, one stalls.
        // cap 4 per connection, slow-consumer ceiling 8 (a connection cannot be 'slow' before it has
        // filled its buffer; once its lag reaches 8 it is structurally too slow → dropped).
        let mut keeping_up = FrameBuffer::new("chat-live", scope("channel:hot-fast"), 4, 8);
        let mut slow = FrameBuffer::new("chat-live", scope("channel:hot-slow"), 4, 8);

        // ---- (load) a fast producer floods the hot stream. ------------------------------------------
        // The keeping-up consumer delivers each frame right after it is offered → its lag stays ~0,
        // it never sheds, never drops.
        for seq in 1..=64u64 {
            assert!(
                keeping_up.offer(human(seq)).is_buffered(),
                "the keeping-up consumer never sheds (it keeps pace)"
            );
            keeping_up.deliver(human(seq));
        }

        // The slow consumer never delivers (a fully-stalled subscription, the dropped one). The lag
        // climbs: the first 4 buffer, 5..7 shed (over the per-connection cap — memory stays bounded at
        // the cap), and at seq 8 the lag reaches the slow-consumer ceiling → it is DROPPED to
        // resync_required. We keep offering past the drop to prove the connection STAYS dropped + the
        // count does NOT double-increment.
        let mut drop_seen = false;
        for seq in 1..=20u64 {
            match slow.offer(human(seq)) {
                PushOutcome::ResyncRequired => drop_seen = true,
                PushOutcome::Buffered | PushOutcome::Shed => {
                    // below the ceiling, memory stays bounded at the cap as the lag climbs.
                    assert!(
                        slow.buffered_frames() <= slow.capacity(),
                        "memory NEVER exceeds the per-connection cap (Little's Law): seq={seq}"
                    );
                }
            }
        }
        assert!(drop_seen, "the slow consumer must be dropped to resync_required");
        // bounded memory: the dropped connection holds NOTHING (it did not buffer the gap).
        assert_eq!(slow.buffered_frames(), 0, "a dropped connection releases its buffer (bounded memory)");
        assert_eq!(slow.frame_lag(), 0, "a dropped connection holds no gap (it is in *.snapshot replay)");
        assert_eq!(slow.resync_required_count(), 1, "the resync_required drop is counted EXACTLY once");

        // the keeping-up connection is UNTOUCHED by its slow neighbour's drop (per-connection isolation).
        assert!(!keeping_up.resync_required(), "a slow consumer never drops a keeping-up neighbour");

        // (signals) snapshot the two §10.2 firehose survival signals off the open buffers (the
        // producer side wires this off the real connection tier at Chat M4; here the drill records it).
        let sig = FirehoseSignals::snapshot([&keeping_up, &slow]);
        // firehose_frame_lag is labelled by {stream, scope} — record each (stream,scope) row.
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
        // resync_required_count is scalar.
        ctx.signals
            .set_scalar(SignalName::ResyncRequiredCount, sig.resync_required_count as i64);

        // restore the injected fault before returning (a re-run starts clean).
        ctx.breaker
            .restore_dependency(Dependency::Firehose, Scope::Global);

        // (assert) the per-(stream,scope) frame-lag is BOUNDED — assert the hot-slow scope's lag is
        // ≤ the slow-consumer ceiling (memory never grew unboundedly). This is the single telemetry
        // assertion that reads green; the resync_required_count is asserted in the runner below.
        ctx.signals.assert_labelled(
            SignalName::FirehoseFrameLag,
            vec![
                Label::new("stream", "chat-live".to_string()),
                Label::new("scope", "channel:hot-slow".to_string()),
            ],
            // ≤ 8 (the slow-consumer ceiling): a dropped connection reads 0; a live one ≤ ceiling.
            Predicate::Lte(8),
        )
    })
}

/// **THE SUB-D11 DRILL** — the dated green artifact the P-S28 GATE/DRILLS names. Register it (it joins
/// the permanent every-incident suite) AND run it; assert the firehose frame-lag survival signal reads
/// green (bounded).
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

    // The dated green-artifact row (the prompt's named DEFINITION-OF-DONE artifact).
    let row = result.artifact_row("2026-06-20");
    assert_eq!(
        row,
        "[2026-06-20] PASS  drill=sub-d11-firehose-slow-consumer  (inject → load → assert green)"
    );
    println!("{row}");
}

/// The drill, run directly, asserting BOTH firehose survival signals — the frame-lag is bounded AND
/// the `resync_required` count is accurate (`== 1`, the slow consumer dropped exactly once). The full
/// SUB-D11 green pair, not just the first assertion.
#[test]
fn sub_d11_both_firehose_survival_signals_read_green() {
    let scenario = sub_d11_firehose_slow_consumer_scenario();
    let result = scenario.run_once();
    match result {
        DrillResult::Pass { .. } => {
            // re-derive the signals to assert the resync_required half explicitly (the scenario's own
            // assertion is the frame-lag-bounded half).
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
            ctx.signals
                .set_scalar(SignalName::ResyncRequiredCount, sig.resync_required_count as i64);
            // the resync_required count is accurate: exactly one drop (NAMED, not silent).
            ctx.signals
                .assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(1))
                .expect_green();
            // and the frame-lag is bounded (memory never grew unboundedly).
            assert!(sig.max_frame_lag() <= 8, "every (stream,scope) frame-lag is BOUNDED");
        }
        other => panic!("SUB-D11 must pass: {other:?}"),
    }
}
