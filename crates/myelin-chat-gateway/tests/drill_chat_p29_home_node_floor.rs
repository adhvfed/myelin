//! # CHAT-P29 (global P-503) — the mega-channel channel-sharded home-node floor: the "If NOT
//! triggered" dated gap-report row + the CHAT-D1-survives-the-swap invariant.
//!
//! The prompt (CHAT-P29 / M5-C-S3) is CONDITIONAL: the channel-sharded home-node is built ONLY if its
//! measured trigger has fired (a channel's subscriber count exceeding the subject-fan-out budget,
//! R-5); otherwise it is a NAMED FLOOR with its measured trigger signal (EI-04 §4, VISION §3,
//! measure-before-shard ADR-10). At this prompt's execution the trigger has NOT fired: the chat M5
//! surge family (CHAT-P26 / P-500) measured the gateway SHED budgets under a 30x agent-message /
//! connection RATE storm, NOT a single mega-channel whose subscriber fan-out crosses the subject
//! budget. So this drill is the recorded gap-report row (the prompt's "If NOT triggered" GATE),
//! machine-checked — an honest named floor, not a floor masquerading as done.
//!
//! It ALSO pins the property that makes the floor a SWAP-not-a-rewrite: the CHAT-D1 resume-cursor
//! backbone (0 lost / 0 dup across a sever→resume) is carried by the UNCHANGED firehose protocol
//! (contract 3.5), so it survives the home-node escalation — the seam the promotion lands on. The full
//! CHAT-D1 drill is `drill_chat_d1_resume.rs`; here we assert the floor record names CHAT-D1 as its
//! promotion gate (the drill written to survive the swap), so when the trigger fires the gate is
//! already pinned.

use myelin_chat_gateway::home_node::{
    home_node_floor_gap_report, MeasuredFanOut, SubjectFanOutBudget, BEAM_GATEWAY_SIBLING_FLOOR,
    GATEWAY_MEASURED_TRIGGER_FLOORS, HOME_NODE_FLOOR,
};

/// **The "If NOT triggered" GATE: the dated gap-report row is honest (0 invisible gaps, no premature
/// promotion).** Both the mega-channel home-node floor and the BEAM-gateway sibling floor are fully
/// recorded with a measured, dated trigger that has NOT fired — so both stay named floors, not built.
#[test]
fn chat_p29_not_triggered_dated_gap_report_row_is_honest() {
    home_node_floor_gap_report()
        .expect("CHAT-P29 gap-report: the not-triggered branch is an honest dated row");

    // the manifest is exactly the two floors the prompt's DoD names: the mega-channel home-node + the
    // BEAM-gateway sibling.
    let ids: Vec<&str> = GATEWAY_MEASURED_TRIGGER_FLOORS
        .iter()
        .map(|f| f.id)
        .collect();
    assert_eq!(
        ids,
        vec!["channel-sharded-home-node", "beam-phoenix-gateway"]
    );

    // the home-node trigger has NOT fired → it is a named floor, NOT built (no premature promotion).
    // Route the consts through black_box so these are RUNTIME assertions on the EXPORTED manifest
    // values (not const-folded) — the same values the rest of the platform reads.
    let home_node = std::hint::black_box(HOME_NODE_FLOOR);
    let beam = std::hint::black_box(BEAM_GATEWAY_SIBLING_FLOOR);
    assert!(
        !home_node.status.has_fired(),
        "the subscriber-count-vs-budget trigger has NOT fired at this prompt's execution"
    );
    assert!(
        !home_node.built,
        "the home-node is a NAMED FLOOR — not built speculatively"
    );
    // and the BEAM sibling (the connection-tier LANGUAGE floor) is separately unfired (Rust retained).
    assert!(!beam.status.has_fired());
    assert!(!beam.built);
}

/// **The trigger is a MEASURED, evaluable predicate — not a hand-typed boolean.** The floor would
/// promote the moment a real telemetry reading's subscriber count crosses the subject-fan-out budget;
/// at/below the budget the flat firehose subject fan-out (the v1 seam) is retained. Proving the
/// predicate is load-bearing is the honest-floor proof: the gate fires on a measurement, never a
/// guess (EI-01 §3).
#[test]
fn chat_p29_trigger_fires_on_a_measured_subscriber_count_crossing_the_budget() {
    let budget = SubjectFanOutBudget::NAMED;
    let cap = budget.max_subscribers_per_subject;

    // a bounded-membership channel (the measured surge load) does NOT fire the trigger.
    assert!(
        !MeasuredFanOut {
            subscriber_count: cap
        }
        .fires(&budget),
        "at-budget keeps the flat subject fan-out (the trigger is exclusive `>`)"
    );
    // a mega-channel over budget WOULD fire it (the escalation the home-node exists for, R-5).
    assert!(
        MeasuredFanOut {
            subscriber_count: cap + 1
        }
        .fires(&budget),
        "a measured subscriber count over budget fires the home-node promotion (R-5)"
    );
}

/// **CHAT-D1 survives the swap: the floor names CHAT-D1 (re-run across the escalation) as its
/// promotion gate, riding the UNCHANGED firehose resume-cursor protocol (3.5).** This is the property
/// that makes the home-node a delivery-topology SWAP behind the unchanged protocol, not a rewrite —
/// so the 0-lost/0-dup resume backbone (pinned green in `drill_chat_d1_resume.rs`) is invariant under
/// the shard split, and the promotion gate is already proven-shaped before the trigger fires.
#[test]
fn chat_p29_promotion_gate_is_chat_d1_re_run_across_the_unchanged_firehose_protocol() {
    let home_node = std::hint::black_box(HOME_NODE_FLOOR);
    assert!(
        home_node.promotion_gate.contains("CHAT-D1") && home_node.promotion_gate.contains("0 lost"),
        "the home-node promotion gate is CHAT-D1 re-run (0 lost / 0 dup) across the escalation"
    );
    assert!(
        home_node.preserved_contract.contains("3.5"),
        "the escalation rides the UNCHANGED firehose resume-cursor protocol (3.5) — a swap, not a \
         protocol redesign; CHAT-D1 was written to survive it"
    );
    assert!(
        home_node.built_seam.contains("resume-cursor seq"),
        "the resume-cursor seq CHAT-D1 pins is carried the same way across the shard split"
    );
}
