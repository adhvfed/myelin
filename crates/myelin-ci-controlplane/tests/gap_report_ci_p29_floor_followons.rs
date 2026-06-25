//! CI-P29 → global **P-489**: the measured-trigger-gated **floor-follow-on gap-report**.
//!
//! This prompt's deliverable is the HONEST NAMING (VISION §3 — name-your-floors) of the two
//! measured-trigger-gated CI-M5 promotions — the **time-series / wide-column log tier** and the
//! **hierarchical scheduler** — plus the two **deferred-by-reference** floors (cross-cell-spanning
//! pipelines, SLSA L3+). Neither promotion is built: their triggers are produced by **CI-P30
//! (global P-490)**, which runs AFTER this prompt, so at execution (2026-06-25) NO surge-driven
//! starvation histogram and NO firehose-log-volume-vs-OLTP measurement exists. Per EI-04 §5
//! ("don't add it before the volume is *measured*") both promotions REMAIN named floors, with
//! their trigger status recorded dated and `built == false`.
//!
//! The gate has two halves (mirrors the AG-P25 seam-floor gap-report, global P-481):
//! 1. **0 invisible gaps** — every named floor is recorded with a NON-EMPTY trigger + follow-on +
//!    preserved-contract + promotion-gate + a dated status
//!    ([`FloorFollowOn::is_fully_recorded`]).
//! 2. **No premature promotion** — the honest-floor invariant: while a measured trigger is
//!    `NotFired`, the promotion MUST stay a named floor (`built == false`). A promotion built with
//!    an unfired trigger is the "add it before it is measured" failure EI-04 §5 / VISION §3 forbid
//!    ([`FloorFollowOn::honours_no_premature_promotion`]).
//!
//! No new core module behaviour is built here beyond the gap-report manifest itself; these are its
//! assertions over the public `floor_followons` surface.

use myelin_ci_controlplane::floor_followons::{all_floor_followons, TriggerStatus};
use myelin_ci_controlplane::{
    FloorFollowOn, DEFERRED_BY_REFERENCE_FLOORS, MEASURED_TRIGGER_FLOORS,
};

/// The four floor follow-ons are exactly: the two measured-trigger promotions + the two
/// deferred-by-reference floors. None may go missing (a missing floor is the silent-skip failure
/// VISION §3 forbids).
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

/// Gate half 1 — **0 invisible gaps**: every row is fully recorded.
#[test]
fn zero_invisible_gaps() {
    let all: Vec<FloorFollowOn> = all_floor_followons();
    assert_eq!(all.len(), 4, "all four floor follow-ons accounted for");
    for f in &all {
        assert!(
            f.is_fully_recorded(),
            "floor follow-on `{}` is an invisible gap — a must-be-non-empty field is empty \
             (trigger / follow-on / preserved-contract / promotion-gate / dated-status)",
            f.id
        );
    }
}

/// Gate half 2 — **no premature promotion** (the honest-floor invariant, EI-04 §5 / VISION §3).
/// At 2026-06-25, CI-P30 (P-490) has not run, so every measured trigger is `NotFired` and every
/// promotion MUST remain a named floor (`built == false`). This is the load-bearing assertion: it
/// proves this prompt did NOT speculatively build a promotion before its measurement.
#[test]
fn no_promotion_built_before_its_measured_trigger_fired() {
    for f in all_floor_followons() {
        assert!(
            f.honours_no_premature_promotion(),
            "`{}` is a PREMATURE promotion — built before its measured trigger fired",
            f.id
        );
        assert!(
            !f.status.has_fired(),
            "`{}` trigger recorded FIRED — but CI-P30 (P-490) has not run at 2026-06-25; \
             re-date the manifest only when a real measurement fires it",
            f.id
        );
        assert!(
            !f.built,
            "`{}` recorded BUILT — but its measured trigger has not fired (no speculative build)",
            f.id
        );
    }
}

/// Every trigger status is DATED (a claim that outlives its verification misleads the next agent,
/// VISION §3). The 2026-06-25 dated note records the red-until-proven state for the next agent.
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
            "`{}` trigger status must carry a date — `{}`",
            f.id,
            f.status.dated()
        );
    }
}

/// The two measured-trigger promotions each preserve their FROZEN contract across the swap — the
/// migration changes the engine behind the contract, never the contract shape. The log tier
/// preserves 11.8 addressability; the hierarchical scheduler preserves the claim-time fairness
/// predicate. This proves the swap is a config/engine change, not a contract divergence.
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
