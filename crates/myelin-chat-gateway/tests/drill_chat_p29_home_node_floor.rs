use myelin_chat_gateway::home_node::{
    home_node_floor_gap_report, MeasuredFanOut, SubjectFanOutBudget, BEAM_GATEWAY_SIBLING_FLOOR,
    GATEWAY_MEASURED_TRIGGER_FLOORS, HOME_NODE_FLOOR,
};

#[test]
fn chat_p29_not_triggered_dated_gap_report_row_is_honest() {
    home_node_floor_gap_report()
        .expect("CHAT-P29 gap-report: the not-triggered branch is an honest dated row");

    let ids: Vec<&str> = GATEWAY_MEASURED_TRIGGER_FLOORS
        .iter()
        .map(|f| f.id)
        .collect();
    assert_eq!(
        ids,
        vec!["channel-sharded-home-node", "beam-phoenix-gateway"]
    );

    let home_node = std::hint::black_box(HOME_NODE_FLOOR);
    let beam = std::hint::black_box(BEAM_GATEWAY_SIBLING_FLOOR);
    assert!(
        !home_node.status.has_fired(),
        "the subscriber-count-vs-budget trigger has NOT fired at this prompt's execution"
    );
    assert!(
        !home_node.built,
        "the home-node is a NAMED FLOOR - not built speculatively"
    );
    assert!(!beam.status.has_fired());
    assert!(!beam.built);
}

#[test]
fn chat_p29_trigger_fires_on_a_measured_subscriber_count_crossing_the_budget() {
    let budget = SubjectFanOutBudget::NAMED;
    let cap = budget.max_subscribers_per_subject;

    assert!(
        !MeasuredFanOut {
            subscriber_count: cap
        }
        .fires(&budget),
        "at-budget keeps the flat subject fan-out (the trigger is exclusive `>`)"
    );
    assert!(
        MeasuredFanOut {
            subscriber_count: cap + 1
        }
        .fires(&budget),
        "a measured subscriber count over budget fires the home-node promotion (R-5)"
    );
}

#[test]
fn chat_p29_promotion_gate_is_chat_d1_re_run_across_the_unchanged_firehose_protocol() {
    let home_node = std::hint::black_box(HOME_NODE_FLOOR);
    assert!(
        home_node.promotion_gate.contains("CHAT-D1") && home_node.promotion_gate.contains("0 lost"),
        "the home-node promotion gate is CHAT-D1 re-run (0 lost / 0 dup) across the escalation"
    );
    assert!(
        home_node.preserved_contract.contains("3.5"),
        "the escalation rides the UNCHANGED firehose resume-cursor protocol (3.5) - a swap, not a \
         protocol redesign; CHAT-D1 was written to survive it"
    );
    assert!(
        home_node.built_seam.contains("resume-cursor seq"),
        "the resume-cursor seq CHAT-D1 pins is carried the same way across the shard split"
    );
}
