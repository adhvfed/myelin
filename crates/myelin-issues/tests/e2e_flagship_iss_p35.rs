use myelin_issues::ci_done_guard;
use myelin_issues::{
    per_effect_idem_key as iss_per_effect_idem_key, plan_agent_ci_gated_transition,
    AgentTransitionOutcome, IssueContext, LinkedPrCheck, StateCategory, Workflow, WorkflowState,
    WorkflowTransition, CHECK_STATE_SUCCESS,
};

use myelin_flow::{
    apply_approved_effects, per_effect_idem_key as flow_per_effect_idem_key, ApprovalCard,
    ApprovalDecision, EffectOutcome, GatedEffect, SignalRow, SignalStore, APPROVAL_SIGNAL_NAME,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

const CLOSE_CARD_ID: &str = "card:triage:close-eng-1421";

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn region() -> Region {
    Region("fr-par".into())
}

fn triage_workflow() -> Workflow {
    Workflow {
        states: vec![
            WorkflowState {
                name: "In Review".into(),
                category: StateCategory::Started,
            },
            WorkflowState {
                name: "Done".into(),
                category: StateCategory::Completed,
            },
        ],
        transitions: vec![WorkflowTransition {
            from: "In Review".into(),
            to: "Done".into(),
            guards: vec![ci_done_guard()],
            required_fields: vec![],
            post_actions: vec![],
        }],
    }
}

#[test]
fn the_governed_close_is_withheld_zero_pre_approval_mutation() {
    let wf = triage_workflow();
    let green = LinkedPrCheck::trusted(CHECK_STATE_SUCCESS);
    let outcome =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &green);
    assert!(
        outcome.is_withheld(),
        "the permitted governed close is WITHHELD for approval"
    );
    assert!(!outcome.is_blocked());
    assert_eq!(
        outcome.pre_approval_mutations(),
        0,
        "0 mutation before approval"
    );
    if let AgentTransitionOutcome::WithheldForApproval { plan } = outcome {
        assert_eq!(plan.to_category, StateCategory::Completed);
        assert_eq!(plan.to, "Done");
    } else {
        panic!("expected WithheldForApproval");
    }
}

#[test]
fn zero_effect_outside_the_intersection() {
    let wf = triage_workflow();
    let green = LinkedPrCheck::trusted(CHECK_STATE_SUCCESS);
    let undeclared =
        plan_agent_ci_gated_transition(&wf, "In Review", "Canceled", IssueContext::new(), &green);
    assert!(undeclared.is_blocked(), "an undeclared edge is BLOCKED");
    assert_eq!(
        undeclared.pre_approval_mutations(),
        0,
        "0 mutation on a block"
    );
    let red = LinkedPrCheck::trusted("failure");
    let red_close =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &red);
    assert!(
        red_close.is_blocked(),
        "a CI-red close is BLOCKED for the agent"
    );
    assert_eq!(red_close.pre_approval_mutations(), 0);
}

#[test]
fn a_declined_governed_transition_is_withheld_zero_mutation() {
    let signals = SignalStore::new();
    let run_id = "run:merge-queue:declined";
    let effect_ref = "myelin://acme/issues/issue/ENG-9/transition";
    let key = flow_per_effect_idem_key(CLOSE_CARD_ID, 0, 1);
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: run_id.to_string(),
        signal_name: APPROVAL_SIGNAL_NAME.to_string(),
        idem_key: key,
        payload: vec![],
        payload_key_ref: Some(myelin_flow::DECLINE_MARKER.to_string()),
        consumed_seq: None,
        received_unix_ms: 0,
    });
    let card = ApprovalCard {
        run_id: run_id.to_string(),
        card_id: CLOSE_CARD_ID.to_string(),
        effects: vec![GatedEffect {
            effect_ref: ArtifactRef(effect_ref.to_string()),
            decision: ApprovalDecision::Decline,
        }],
    };
    let applied = std::cell::Cell::new(false);
    let applier = |_r: &ArtifactRef| -> Result<String, String> {
        applied.set(true);
        Ok("must-not-run".into())
    };
    let res = apply_approved_effects(&signals, &tenant(), &card, &applier);
    match res[0].as_ref().expect("a buffered decision").as_ref() {
        Ok(EffectOutcome::Withheld(_)) => {}
        other => panic!("a declined transition must be WITHHELD, got {other:?}"),
    }
    assert!(
        !applied.get(),
        "the apply seam is NEVER reached for a declined transition (0 mutation)"
    );
}

#[test]
fn the_per_effect_idem_key_rule_is_byte_identical_across_subsystems() {
    for (card, idx, total) in [
        ("card-7", 0, 1),
        ("card-7", 0, 3),
        ("card-7", 1, 3),
        ("card-7", 2, 3),
        (CLOSE_CARD_ID, 0, 1),
    ] {
        assert_eq!(
            iss_per_effect_idem_key(card, idx, total),
            flow_per_effect_idem_key(card, idx, total),
            "the Issues + flow per-effect idem_key derivations must be byte-identical (OQ-F)"
        );
    }
    assert_eq!(iss_per_effect_idem_key(CLOSE_CARD_ID, 0, 1), CLOSE_CARD_ID);
}

#[test]
fn the_check_status_guard_reads_off_the_fact_under_the_scenario() {
    assert!(LinkedPrCheck::trusted(CHECK_STATE_SUCCESS).is_acceptable());
    assert!(!LinkedPrCheck::trusted("failure").is_acceptable());
    assert!(!LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false).is_acceptable());
    assert!(LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, true).is_acceptable());
    let wf = triage_workflow();
    let red = plan_agent_ci_gated_transition(
        &wf,
        "In Review",
        "Done",
        IssueContext::new(),
        &LinkedPrCheck::trusted("failure"),
    );
    assert!(red.is_blocked());
    let green = plan_agent_ci_gated_transition(
        &wf,
        "In Review",
        "Done",
        IssueContext::new(),
        &LinkedPrCheck::trusted(CHECK_STATE_SUCCESS),
    );
    assert!(green.is_withheld());
}
