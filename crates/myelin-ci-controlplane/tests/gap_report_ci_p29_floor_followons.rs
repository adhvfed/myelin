use myelin_ci_controlplane::floor_followons::{all_floor_followons, TriggerStatus};
use myelin_ci_controlplane::{
    FloorFollowOn, DEFERRED_BY_REFERENCE_FLOORS, MEASURED_TRIGGER_FLOORS,
};

#[test]
fn all_four_floor_followons_present_and_correctly_partitioned() {
    assert_eq!(
        MEASURED_TRIGGER_FLOORS.len(),
        2,
        "exactly TWO measured-trigger-gated promotions (time-series log tier + hierarchical sched)"
    );
    assert_eq!(
        DEFERRED_BY_REFERENCE_FLOORS.len(),
        2,
        "exactly TWO deferred-by-reference floors (cross-cell pipelines + SLSA L3+)"
    );

    let measured: Vec<&str> = MEASURED_TRIGGER_FLOORS.iter().map(|f| f.id).collect();
    assert_eq!(
        measured,
        vec!["time-series-log-tier", "hierarchical-scheduler"],
        "the two promotions must be the time-series log tier + the hierarchical scheduler"
    );

    let deferred: Vec<&str> = DEFERRED_BY_REFERENCE_FLOORS.iter().map(|f| f.id).collect();
    assert_eq!(
        deferred,
        vec!["cross-cell-spanning-pipelines", "slsa-l3-plus-hermetic"],
        "the two deferred floors must be cross-cell pipelines + SLSA L3+"
    );
}

#[test]
fn zero_invisible_gaps() {
    let all: Vec<FloorFollowOn> = all_floor_followons();
    assert_eq!(all.len(), 4, "all four floor follow-ons accounted for");
    for f in &all {
        assert!(
            f.is_fully_recorded(),
            "floor follow-on `{}` is an invisible gap - a must-be-non-empty field is empty \
             (trigger / follow-on / preserved-contract / promotion-gate / dated-status)",
            f.id
        );
    }
}

#[test]
fn no_promotion_built_before_its_measured_trigger_fired() {
    for f in all_floor_followons() {
        assert!(
            f.honours_no_premature_promotion(),
            "`{}` is a PREMATURE promotion - built before its measured trigger fired",
            f.id
        );
        assert!(
            !f.status.has_fired(),
            "`{}` trigger recorded FIRED - but CI-P30 (P-490) has not run at 2026-06-25; \
             re-date the manifest only when a real measurement fires it",
            f.id
        );
        assert!(
            !f.built,
            "`{}` recorded BUILT - but its measured trigger has not fired (no speculative build)",
            f.id
        );
    }
}

#[test]
fn every_trigger_status_is_dated() {
    for f in all_floor_followons() {
        assert!(
            matches!(f.status, TriggerStatus::NotFired { .. }),
            "`{}` must be NotFired at this prompt's execution",
            f.id
        );
        assert!(
            f.status.dated().contains("2026-"),
            "`{}` trigger status must carry a date - `{}`",
            f.id,
            f.status.dated()
        );
    }
}

#[test]
fn measured_promotions_preserve_their_frozen_contracts() {
    let log_tier = MEASURED_TRIGGER_FLOORS
        .iter()
        .find(|f| f.id == "time-series-log-tier")
        .expect("time-series log tier row present");
    assert!(
        log_tier.preserved_contract.contains("11.8")
            && log_tier
                .preserved_contract
                .contains("(job, step, byte-range)"),
        "the log-tier promotion must preserve the 11.8 (job, step, byte-range) addressability"
    );
    assert!(
        log_tier.promotion_gate.contains("0 dangling")
            && log_tier.promotion_gate.contains("0 log bytes"),
        "the log-tier promotion gate must be: details_ref resolves (0 dangling) + 0 log bytes lost"
    );

    let sched = MEASURED_TRIGGER_FLOORS
        .iter()
        .find(|f| f.id == "hierarchical-scheduler")
        .expect("hierarchical scheduler row present");
    assert!(
        sched.preserved_contract.contains("claim")
            && sched.preserved_contract.contains("per-tenant"),
        "the scheduler promotion must preserve the claim-time fairness predicate (per-tenant -> ..)"
    );
    assert!(
        sched.promotion_gate.contains("starvation histogram")
            && sched.promotion_gate.contains("vs flat DRR"),
        "the scheduler promotion gate must be the starvation histogram improving vs flat DRR"
    );
}
