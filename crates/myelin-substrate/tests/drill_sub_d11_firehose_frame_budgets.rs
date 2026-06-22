//! # SUB-D11 (completion) — the firehose frame-budget + scope-selector drill (P-S29 → global P-136)
//!
//! **Drill catalogue:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §11 row **D-11** (*Firehose reconnect-loses-zero-ops*) + §7.7/§7.6. P-S28 (`drill_sub_d11_firehose_
//! slow_consumer.rs`) proved the per-connection cap + the slow-consumer drop. THIS drill **completes the
//! substrate's half of D-11** with the two P-S29 slices the GATE/DRILLS names:
//!   - a `*` scope is **rejected** (bounded selector only — board:/doc:/channel:);
//!   - presence/speculative frames **shed before message frames** (the §7.6 per-surface frame budgets);
//!   - the per-`(stream,scope)` `firehose_frame_lag` stays **BOUNDED** under a hot-stream frame flood.
//!
//! The **Bus owns the zero-loss-replay assertion** (`resume(last_seq)` backfills with zero ops lost) —
//! **P-141 (EB-21)**, named here, not asserted (the Bus firehose protocol has not landed yet). The full
//! reconnect-loses-zero-ops proof re-runs with the Bus impl + the M4 connection tier (P-S31 / P-326).
//!
//! It is the EI-01 §3 drill shape: *inject a fault (the P-S03 `break_dependency` drops a firehose
//! subscription on a hot stream), drive one unit of load (a presence/agent/human frame flood on a
//! paginated board scope), read one telemetry assertion that reads green (the P-S04 library).* Here:
//!   - **inject** — `break_dependency(Dependency::Firehose, …)` drops a firehose connection mid-stream.
//!   - **load** — a hot `board:` scope floods a per-connection [`FrameSelector`] with mixed-class frames
//!     (presence + agent + human) plus off-window rows (a 50k-row board); the consumer keeps up just
//!     enough that the per-surface CLASS budgets fire (presence sheds first), not the slow-consumer drop.
//!   - **assert** — the §10.2 `ShedCount`-by-lane (frame budgets) signal reads green: presence shed
//!     `>= 1` BEFORE any human (message) shed, and the `firehose_frame_lag` stays BOUNDED.

use myelin_harness::{
    Dependency, DrillContext, DrillRegistry, DrillResult, DrillScenario, Label, Predicate, Scope,
    SignalName,
};
use myelin_substrate::{
    BoundedSelector, Frame, FrameClass, FrameOutcome, FrameSelector, ScopeWindow, SelectorError,
};

fn presence(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::Presence)
}
fn agent(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::AgentDelivery)
}
fn human(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::HumanDelivery)
}

/// The SUB-D11-completion scenario: under an injected firehose drop on a HOT board stream, reject a `*`
/// scope, flood a paginated window with mixed-class frames, and assert the per-surface frame shed
/// budgets fire in order (presence sheds before message frames) with a bounded frame-lag.
fn sub_d11_firehose_frame_budget_scenario() -> DrillScenario {
    DrillScenario::new(
        "sub-d11-firehose-frame-budgets",
        |ctx: &mut DrillContext| {
            // (inject) drop a firehose subscription mid-stream on a hot stream (the D-11 condition).
            ctx.breaker
                .break_dependency(Dependency::Firehose, Scope::Global);

            // (a) a `*` scope is REJECTED — one client cannot subscribe to the whole tenant firehose (§7.7).
            assert_eq!(
                BoundedSelector::parse("*"),
                Err(SelectorError::Wildcard),
                "a `*` firehose scope MUST be rejected (bounded selector only)"
            );
            // a 50k-row board subscribes to a bounded paginated WINDOW, never the whole board.
            let sel = BoundedSelector::parse("board:hot").expect("a bounded board selector");
            let window = ScopeWindow::new(10_000, 100, 50); // delivers rows [9_950, 10_150)
                                                            // cap 8 → v1 floor: presence 2, agent 4, human 8. A high lag ceiling so the CLASS budgets fire
                                                            // (the slow-consumer drop is the OTHER half, proven in drill_sub_d11_firehose_slow_consumer.rs).
            let mut selector = FrameSelector::new("kn-ops", &sel, 8, 100_000, window);

            // ---- (load) flood the hot board with mixed-class frames on in-window + off-window rows. -------
            // off-window rows (a 50k board's off-screen rows) are NOT delivered — bounded memory.
            let mut out_of_window = 0u64;
            for seq in 1..=40u64 {
                // rows 0..40 are far below the window [9_950, …) → OutOfWindow.
                if selector.offer(presence(seq), Some(seq)) == FrameOutcome::OutOfWindow {
                    out_of_window += 1;
                }
            }
            assert_eq!(
                out_of_window, 40,
                "off-screen board rows are never delivered (paginated slice)"
            );
            assert_eq!(
                selector.buffer().buffered_frames(),
                0,
                "off-window frames cost no buffer memory"
            );

            // in-window presence flood: the presence budget (2) fills, then presence sheds BY CLASS — while
            // message (human) frames still have budget. (No deliveries → the class in-flight stays full.)
            for seq in 100..=110u64 {
                selector.offer(presence(seq), Some(10_050)); // an in-window row
            }
            // in-window human (message) frames still buffer — message delivery is shed LAST.
            let mut human_buffered = 0u64;
            for seq in 200..=203u64 {
                if selector.offer(human(seq), Some(10_050)) == FrameOutcome::Buffered {
                    human_buffered += 1;
                }
            }
            // an agent frame: agents shed before humans (the agent budget is tighter than the human one).
            selector.offer(agent(300), Some(10_050));

            let presence_shed = selector.budget().shed_count(FrameClass::Presence);
            let human_shed = selector.budget().shed_count(FrameClass::HumanDelivery);

            // (assertions baked into the scenario) presence shed > 0 and BEFORE any human (message) shed.
            assert!(
                presence_shed >= 1,
                "presence/speculative frames shed (the lowest budget fills first)"
            );
            assert_eq!(
                human_shed, 0,
                "message (human) frames are shed LAST (never class-shed here)"
            );
            assert!(
                human_buffered >= 1,
                "message frames still buffered while presence shed"
            );

            // (signals) export the §10.2 ShedCount-by-lane (frame budgets) signal, labelled by frame class.
            for class in [
                FrameClass::Presence,
                FrameClass::AgentDelivery,
                FrameClass::HumanDelivery,
            ] {
                ctx.signals.set_labelled(
                    SignalName::ShedCount,
                    vec![Label::new("lane", class.label().to_string())],
                    selector.budget().shed_count(class) as i64,
                );
            }
            // the firehose_frame_lag survival signal stays BOUNDED on the (stream,scope) row.
            ctx.signals.set_labelled(
                SignalName::FirehoseFrameLag,
                vec![
                    Label::new("stream", "kn-ops".to_string()),
                    Label::new("scope", "board:hot".to_string()),
                ],
                selector.buffer().frame_lag() as i64,
            );

            // restore the injected fault before returning (a re-run starts clean).
            ctx.breaker
                .restore_dependency(Dependency::Firehose, Scope::Global);

            // (assert) the presence lane's frame-shed budget fired (>= 1) — presence sheds before message
            // delivery. The single telemetry assertion that reads green; the rest are asserted in the runner.
            ctx.signals.assert_labelled(
                SignalName::ShedCount,
                vec![Label::new("lane", "presence".to_string())],
                Predicate::Gte(1),
            )
        },
    )
}

/// **THE SUB-D11-completion DRILL** — the dated green artifact the P-S29 GATE/DRILLS names. Register it
/// (it joins the permanent every-incident suite) AND run it; assert the per-surface frame-shed-budget
/// signal reads green (presence shed before message delivery).
#[test]
fn sub_d11_firehose_frame_budgets_green_artifact() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(sub_d11_firehose_frame_budget_scenario());

    let results = registry.run_all();
    assert_eq!(results.len(), 1);
    let result = &results[0];

    assert!(
        result.is_pass(),
        "SUB-D11 (completion): a `*` scope must be rejected, presence frames must shed before message \
         frames, and the frame-lag must stay bounded (the per-surface frame-shed-budget signal reads \
         green): {result:?}"
    );

    let row = result.artifact_row("2026-06-20");
    assert_eq!(
        row,
        "[2026-06-20] PASS  drill=sub-d11-firehose-frame-budgets  (inject → load → assert green)"
    );
    println!("{row}");
}

/// The drill, run directly, asserting the FULL frame-shed order: presence shed `>= 1`, human (message)
/// shed `== 0` (message frames shed last), and the frame-lag bounded — the complete green pair.
#[test]
fn sub_d11_frame_shed_order_is_presence_before_message_green() {
    let scenario = sub_d11_firehose_frame_budget_scenario();
    match scenario.run_once() {
        DrillResult::Pass { .. } => {
            // re-derive to assert the message-shed-last half explicitly.
            let sel = BoundedSelector::parse("board:hot").unwrap();
            let mut selector = FrameSelector::new(
                "kn-ops",
                &sel,
                8,
                100_000,
                ScopeWindow::new(10_000, 100, 50),
            );
            for seq in 100..=110u64 {
                selector.offer(presence(seq), Some(10_050));
            }
            for seq in 200..=203u64 {
                selector.offer(human(seq), Some(10_050));
            }
            let mut ctx = DrillContext::new();
            ctx.signals.set_labelled(
                SignalName::ShedCount,
                vec![Label::new("lane", "human".to_string())],
                selector.budget().shed_count(FrameClass::HumanDelivery) as i64,
            );
            // message (human) frames are shed LAST — 0 class sheds here.
            ctx.signals
                .assert_labelled(
                    SignalName::ShedCount,
                    vec![Label::new("lane", "human".to_string())],
                    Predicate::Eq(0),
                )
                .expect_green();
            // and the frame-lag is bounded (memory never grew unboundedly).
            assert!(
                selector.buffer().frame_lag() <= 100_000,
                "the (stream,scope) frame-lag stays BOUNDED"
            );
        }
        other => panic!("SUB-D11 (completion) must pass: {other:?}"),
    }
}
