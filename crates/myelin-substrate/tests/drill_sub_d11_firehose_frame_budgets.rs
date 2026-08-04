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

fn sub_d11_firehose_frame_budget_scenario() -> DrillScenario {
    DrillScenario::new(
        "sub-d11-firehose-frame-budgets",
        |ctx: &mut DrillContext| {
            ctx.breaker
                .break_dependency(Dependency::Firehose, Scope::Global);

            assert_eq!(
                BoundedSelector::parse("*"),
                Err(SelectorError::Wildcard),
                "a `*` firehose scope MUST be rejected (bounded selector only)"
            );
            let sel = BoundedSelector::parse("board:hot").expect("a bounded board selector");
            let window = ScopeWindow::new(10_000, 100, 50);
            let mut selector = FrameSelector::new("kn-ops", &sel, 8, 100_000, window);

            let mut out_of_window = 0u64;
            for seq in 1..=40u64 {
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

            for seq in 100..=110u64 {
                selector.offer(presence(seq), Some(10_050));
            }
            let mut human_buffered = 0u64;
            for seq in 200..=203u64 {
                if selector.offer(human(seq), Some(10_050)) == FrameOutcome::Buffered {
                    human_buffered += 1;
                }
            }
            selector.offer(agent(300), Some(10_050));

            let presence_shed = selector.budget().shed_count(FrameClass::Presence);
            let human_shed = selector.budget().shed_count(FrameClass::HumanDelivery);

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
            ctx.signals.set_labelled(
                SignalName::FirehoseFrameLag,
                vec![
                    Label::new("stream", "kn-ops".to_string()),
                    Label::new("scope", "board:hot".to_string()),
                ],
                selector.buffer().frame_lag() as i64,
            );

            ctx.breaker
                .restore_dependency(Dependency::Firehose, Scope::Global);

            ctx.signals.assert_labelled(
                SignalName::ShedCount,
                vec![Label::new("lane", "presence".to_string())],
                Predicate::Gte(1),
            )
        },
    )
}

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

#[test]
fn sub_d11_frame_shed_order_is_presence_before_message_green() {
    let scenario = sub_d11_firehose_frame_budget_scenario();
    match scenario.run_once() {
        DrillResult::Pass { .. } => {
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
            ctx.signals
                .assert_labelled(
                    SignalName::ShedCount,
                    vec![Label::new("lane", "human".to_string())],
                    Predicate::Eq(0),
                )
                .expect_green();
            assert!(
                selector.buffer().frame_lag() <= 100_000,
                "the (stream,scope) frame-lag stays BOUNDED"
            );
        }
        other => panic!("SUB-D11 (completion) must pass: {other:?}"),
    }
}
