//! # `floor_followons` — the CI-M5 measured-trigger-gated floor follow-ons (CI-P29 → global P-489).
//!
//! **Status note (DATED 2026-06-25; re-date on any change — a claim that outlives its verification
//! misleads the next agent, VISION §3 / EI-01 §1).** This module is a *gap-report*: it NAMES the
//! two **measured-trigger-gated** non-gate promotions CI-M5 schedules — the **time-series /
//! wide-column log tier** and the **hierarchical scheduler** — and records, for EACH, whether its
//! measured trigger has FIRED. Per VISION §3 (name-your-floors — promotion is *triggered*, never
//! premature) and EI-04 §5 ("an append-everything event log can outgrow a general-purpose
//! database; keep a seam … but don't add it before the volume is *measured*"), a promotion whose
//! trigger has NOT fired stays a **named floor — it is NOT built speculatively**, and its trigger
//! status is recorded here, dated.
//!
//! ## Why a gap-report, not a build (the trigger ordering)
//! Both triggers are produced by **CI-P30** (global **P-490**) — the world-scale-hardening prompt
//! that runs the 30× CI surge family (CI-D2), tunes the DRR weights / shed budget, and *measures*:
//! - the **per-`fair_key` wait-time/starvation histogram** under surge (contract 1.8) — the signal
//!   the hierarchical-scheduler promotion is gated on (open question 07#1);
//! - the **event volume of the firehose log stream** vs the OLTP-indexed object-segment tier — the
//!   signal the time-series-tier promotion is gated on (EI-04 §5).
//!
//! In the global ledger, **P-490 (CI-P30) runs AFTER this prompt (P-489 / CI-P29)** — the
//! architecture index itself places CI-P29 "after CI-P30's measurements", but the run-table orders
//! P-489 before P-490. At THIS prompt's execution (2026-06-25) CI-P30 has **not run**, so:
//! - **no per-`fair_key` starvation histogram has been measured under the 30× surge** (the
//!   scheduler today exposes the queue-depth signal, [`crate::scheduler`] 1.8, but the
//!   surge-driven wait-time histogram + its threshold are CI-P30's measurement);
//! - **no firehose log-volume measurement vs the OLTP index has been taken** (the log pipeline
//!   today seals into the object-segment T3 tier + the `(job, step, byte-range)` OLTP index,
//!   [`crate::log_pipeline`] 11.8; the volume-vs-DB measurement is CI-P30's).
//!
//! Therefore BOTH triggers are **`NotFired` (red-until-proven)** and BOTH promotions **remain named
//! floors**. Building either now would be exactly the "add it before the volume is measured"
//! anti-pattern EI-04 §5 forbids and the "floor that masquerades as done" VISION §3 forbids. This
//! module records that honestly; it does NOT build the promotions. When CI-P30 fires a trigger, the
//! follow-on agent flips that row's [`TriggerStatus`] to `Fired` (with the dated measured evidence)
//! and ships the promotion behind the SAME contract (11.8 for the log tier; the DRR claim path the
//! hierarchical scheduler refines) — a config/algorithm swap behind a frozen seam, NOT a rewrite.
//!
//! ## What is already BUILT (the seams the promotions swap behind — no rewrite)
//! - **Log tier (11.8):** the firehose → sealed T2 content-addressed segment → `log_segment` +
//!   `log_anchor` `(job, step, byte-range)` OLTP index ([`crate::log_pipeline`]). The time-series
//!   tier promotion **preserves this addressability contract**: the `details_ref` still resolves
//!   `(job, step, byte-range) → bytes`, the migration loses 0 log bytes — only the *index/storage
//!   engine behind the contract* changes.
//! - **Scheduler (DRR):** flat Deficit-Round-Robin fair-share at claim time over `fair_key`
//!   ([`crate::fairness`], [`crate::scheduler`]). The hierarchical promotion **refines the same
//!   claim-time fairness predicate** (per-tenant → per-project → per-pipeline) — the claim seam is
//!   unchanged.
//!
//! ## The deferred-by-design floors (named here by reference; NOT this prompt's promotions)
//! Two further CI-M5 floors are designed-not-built and named by reference (the prompt requires
//! they be named, not built here):
//! - **Cross-cell-spanning pipelines** — a pipeline whose jobs span cells of a multi-cell tenant;
//!   inherits the **12.6 cross-cell PII-free pointer bridge** when OQ-I lifts in M5's shared work.
//! - **SLSA L3+ / hermetic provenance** — demand-triggered (the v1 ships SLSA L1–L2 + SBOM).
//!
//! ## The gap-report invariant (this prompt's gate)
//! Each follow-on below is recorded as a [`FloorFollowOn`] with a NON-EMPTY trigger, follow-on,
//! preserved-contract, and a [`TriggerStatus`]. The `floor_followons_gap_report` test asserts **0
//! invisible gaps** (each fully recorded) AND the **honest-floor invariant**: while a trigger is
//! `NotFired`, the promotion MUST be a named floor (`built == false`) — a promotion built without a
//! fired trigger is the premature-promotion failure EI-04 §5 / VISION §3 forbid. The machine-
//! checkable manifest below is the single source of truth the gap-report cross-checks; keep it in
//! sync with the prose above.

/// The measured status of a promotion's trigger. Red-until-proven: a promotion is built ONLY once
/// its trigger is `Fired` with dated, measured evidence (never speculatively).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerStatus {
    /// The measured trigger has **not** fired — the promotion REMAINS a named floor (not built).
    /// Carries the dated note recording *why* it is unfired (what measurement is still owed + who
    /// owes it). EI-04 §5: don't add it before the measurement.
    NotFired {
        /// The dated note (e.g. `"2026-06-25: CI-P30 (P-490) not yet run — no surge measurement"`).
        as_of: &'static str,
    },
    /// The measured trigger HAS fired — the promotion is unblocked. Carries the dated measured
    /// evidence (the histogram/volume reading that crossed the threshold). When this is set, the
    /// follow-on agent ships the promotion behind the preserved contract.
    Fired {
        /// The dated measured evidence (e.g. `"2026-06-30: p99 wait 47s on tenant X under 30x"`).
        evidence: &'static str,
    },
}

impl TriggerStatus {
    /// True iff the trigger has fired (the promotion may be built). While false, the promotion
    /// MUST remain a named floor.
    pub fn has_fired(&self) -> bool {
        matches!(self, TriggerStatus::Fired { .. })
    }

    /// The dated note/evidence string (non-empty in both arms — an undated status is itself a gap).
    pub fn dated(&self) -> &'static str {
        match self {
            TriggerStatus::NotFired { as_of } => as_of,
            TriggerStatus::Fired { evidence } => evidence,
        }
    }
}

/// One measured-trigger-gated floor follow-on row, machine-checked by the gap-report.
///
/// The gap-report asserts every must-be-non-empty field IS non-empty (no invisible gap) AND the
/// honest-floor invariant: `built` is true ONLY when `trigger.has_fired()` (no premature
/// promotion).
#[derive(Clone, Copy, Debug)]
pub struct FloorFollowOn {
    /// A short stable id (e.g. `"time-series-log-tier"`).
    pub id: &'static str,
    /// One line: what this promotion IS (the thing the floor is promoted TO).
    pub what: &'static str,
    /// What is already BUILT — the seam the promotion swaps behind (so it is a swap, not a rewrite).
    pub built_seam: &'static str,
    /// The contract the promotion MUST preserve across the swap (e.g. 11.8 addressability). The
    /// migration changes the engine behind the contract, never the contract. MUST be non-empty.
    pub preserved_contract: &'static str,
    /// The MEASURED trigger that must fire to start the work (which prompt measures it). Non-empty.
    pub trigger: &'static str,
    /// The measured status of that trigger (red-until-proven).
    pub status: TriggerStatus,
    /// What the follow-on actually delivers once the trigger fires. MUST be non-empty.
    pub follow_on: &'static str,
    /// The gate that must be green to call the promotion done (only relevant once built). Non-empty.
    pub promotion_gate: &'static str,
    /// `true` iff the promotion has been BUILT. The honest-floor invariant requires this be `false`
    /// while `status` is `NotFired` — a built promotion with an unfired trigger is a premature
    /// promotion (EI-04 §5 / VISION §3 forbid it).
    pub built: bool,
}

impl FloorFollowOn {
    /// True iff this row is fully recorded — no invisible gap. A follow-on named without a trigger,
    /// a preserved contract, a follow-on, or a promotion gate is an invisible gap.
    pub fn is_fully_recorded(&self) -> bool {
        !self.id.is_empty()
            && !self.what.is_empty()
            && !self.built_seam.is_empty()
            && !self.preserved_contract.is_empty()
            && !self.trigger.is_empty()
            && !self.status.dated().is_empty()
            && !self.follow_on.is_empty()
            && !self.promotion_gate.is_empty()
    }

    /// The honest-floor invariant (EI-04 §5 / VISION §3): a promotion is built ONLY once its
    /// trigger has fired. While the trigger is `NotFired`, the promotion MUST remain a named floor.
    /// Returns `true` when the invariant holds.
    pub fn honours_no_premature_promotion(&self) -> bool {
        // built ⇒ trigger fired. (Equivalently: ¬fired ⇒ ¬built.)
        !self.built || self.status.has_fired()
    }
}

/// The TWO measured-trigger-gated promotions CI-P29 (P-489) schedules. Both stay named floors
/// until CI-P30 (P-490) measures their triggers; the gap-report enforces no premature promotion.
pub const MEASURED_TRIGGER_FLOORS: &[FloorFollowOn] = &[
    FloorFollowOn {
        id: "time-series-log-tier",
        what: "a dedicated time-series / wide-column log tier promoted from the object-segment T3 \
               floor — the storage/index engine for the highest-volume firehose log stream",
        built_seam: "the firehose -> sealed T2 content-addressed segment -> log_segment + \
                     log_anchor (job, step, byte-range) OLTP index (log_pipeline, 11.8); the \
                     details_ref jump-to-failure resolves (job, step, byte-range) -> bytes today",
        preserved_contract: "11.8 — the (job, step, byte-range) addressability: the details_ref \
                             still resolves through the new tier (0 dangling step anchors); the \
                             migration loses 0 log bytes. Only the engine behind the contract \
                             changes, never the contract",
        trigger: "CI-P30 (P-490) MEASURES firehose log-stream event volume outgrowing the \
                  OLTP-indexed object-segment tier (EI-04 §5 — not before measured)",
        status: TriggerStatus::NotFired {
            as_of: "2026-06-25: CI-P30 (P-490) has not run — no firehose log-volume-vs-OLTP \
                    measurement exists; per EI-04 §5 the tier is NOT built before the measurement. \
                    Floor remains named.",
        },
        follow_on: "swap the (job, step, byte-range) index/storage engine to a time-series / \
                    wide-column tier behind the unchanged 11.8 contract; migrate sealed segments \
                    losing 0 bytes; re-point details_ref resolution at the new tier",
        promotion_gate: "the (job, step, byte-range) details_ref still resolves through the new \
                         tier (0 dangling step anchors) AND the migration loses 0 log bytes — CI",
        built: false,
    },
    FloorFollowOn {
        id: "hierarchical-scheduler",
        what: "a richer hierarchical (per-tenant -> per-project -> per-pipeline) scheduler \
               promoted from flat DRR fair-share at claim time",
        built_seam: "flat Deficit-Round-Robin fair-share over fair_key at claim time \
                     (fairness + scheduler); the no-starvation property is proven by the \
                     fairness_no_starvation drill; the claim-time fairness predicate is the seam \
                     the hierarchy refines",
        preserved_contract: "the claim-time fairness predicate (the DRR claim path) + 1.8 (the \
                             per-fair_key wait-time histogram telemetry); the hierarchy refines \
                             the same claim seam per-tenant -> per-project -> per-pipeline",
        trigger: "CI-P30 (P-490) MEASURES a per-fair_key starvation signal under the 30x surge — \
                  the wait-time histogram crossing its tuned threshold (open question 07#1)",
        status: TriggerStatus::NotFired {
            as_of: "2026-06-25: CI-P30 (P-490) has not run — no per-fair_key starvation histogram \
                    has been measured under the 30x surge; flat DRR holds no-starvation today. \
                    Floor remains named.",
        },
        follow_on: "refine the flat DRR claim predicate into a per-tenant -> per-project -> \
                    per-pipeline hierarchical DRR behind the unchanged claim seam, only on the \
                    measured starvation signal",
        promotion_gate: "the per-fair_key starvation histogram improves vs flat DRR under the \
                         same surge (the measured starvation signal clears) — CI",
        built: false,
    },
];

/// The deferred-by-design CI-M5 floors named here BY REFERENCE (the prompt requires they be named,
/// not built in this prompt). Each carries its inheriting bridge / demand trigger.
pub const DEFERRED_BY_REFERENCE_FLOORS: &[FloorFollowOn] = &[
    FloorFollowOn {
        id: "cross-cell-spanning-pipelines",
        what: "a pipeline whose jobs span cells of a multi-cell tenant",
        built_seam: "single-cell pipelines ship v1; the cross-cell PII-free pointer bridge (12.6) \
                     is the inherited mechanism",
        preserved_contract: "12.6 — the cross-cell PII-free pointer bridge frame (a CI run \
                             inherits it; no PII crosses a cell boundary, references only)",
        trigger: "the OQ-I multi-cell bridge (12.6) lifts in M5's shared work + cross-cell demand",
        status: TriggerStatus::NotFired {
            as_of: "2026-06-25: designed-not-built — named by reference; the 12.6 bridge lifts in \
                    M5 shared work. Floor remains named (handled by reference, not this prompt).",
        },
        follow_on: "CI runs inherit the 12.6 bridge so a multi-cell tenant's pipeline can span \
                    cells, references-not-payloads across the boundary",
        promotion_gate: "a cross-cell pipeline run carries no PII across a cell boundary \
                         (references only) and resolves through the 12.6 bridge — CI",
        built: false,
    },
    FloorFollowOn {
        id: "slsa-l3-plus-hermetic",
        what: "SLSA L3+ / hermetic (two-party) provenance",
        built_seam: "SLSA L1-L2 provenance + SBOM ship v1 (supply_chain)",
        preserved_contract: "the provenance/attestation seam (the supply-chain attestation shape) \
                             — L3+ strengthens the isolation/non-falsifiability, not the shape",
        trigger: "customer demand (demand-triggered)",
        status: TriggerStatus::NotFired {
            as_of: "2026-06-25: demand-triggered — named by reference; no demand signal recorded. \
                    Floor remains named (handled by reference, not this prompt).",
        },
        follow_on: "hermetic / two-party (L3+) provenance over the existing attestation seam",
        promotion_gate: "the build provenance meets SLSA L3+ (hermetic, non-falsifiable) — CI",
        built: false,
    },
];

/// Every floor follow-on the gap-report must account for: the two measured-trigger-gated
/// promotions + the two deferred-by-reference floors. The gap-report asserts `0` invisible gaps
/// and the honest-floor invariant over all of them.
pub fn all_floor_followons() -> Vec<FloorFollowOn> {
    MEASURED_TRIGGER_FLOORS
        .iter()
        .chain(DEFERRED_BY_REFERENCE_FLOORS.iter())
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exactly the two measured-trigger-gated promotions are recorded, in order.
    #[test]
    fn the_two_measured_trigger_promotions_are_recorded() {
        let ids: Vec<&str> = MEASURED_TRIGGER_FLOORS.iter().map(|f| f.id).collect();
        assert_eq!(
            ids,
            vec!["time-series-log-tier", "hierarchical-scheduler"],
            "the two CI-P29 promotions must be the time-series log tier + the hierarchical scheduler"
        );
    }

    /// Exactly the two deferred-by-reference floors are named.
    #[test]
    fn the_two_deferred_floors_are_named_by_reference() {
        let ids: Vec<&str> = DEFERRED_BY_REFERENCE_FLOORS.iter().map(|f| f.id).collect();
        assert_eq!(
            ids,
            vec!["cross-cell-spanning-pipelines", "slsa-l3-plus-hermetic"],
            "the two deferred floors named by reference must be cross-cell pipelines + SLSA L3+"
        );
    }

    /// 0 invisible gaps: every row is fully recorded (no field that must be non-empty is empty).
    #[test]
    fn zero_invisible_gaps() {
        for f in all_floor_followons() {
            assert!(
                f.is_fully_recorded(),
                "floor follow-on `{}` is an invisible gap (a must-be-non-empty field is empty)",
                f.id
            );
        }
    }

    /// The honest-floor invariant (EI-04 §5 / VISION §3): NO promotion is built while its measured
    /// trigger has not fired. At 2026-06-25 (CI-P30 not run) every trigger is NotFired, so every
    /// promotion MUST remain a named floor (`built == false`).
    #[test]
    fn no_premature_promotion_every_trigger_unfired_stays_a_floor() {
        for f in all_floor_followons() {
            assert!(
                f.honours_no_premature_promotion(),
                "floor follow-on `{}` is a PREMATURE promotion — built with an unfired trigger \
                 (EI-04 §5 forbids adding it before the measurement)",
                f.id
            );
            // At THIS prompt's execution, CI-P30 has not run: assert each trigger is unfired and
            // each promotion is unbuilt, so the next agent sees the honest red-until-proven state.
            assert!(
                !f.status.has_fired(),
                "trigger for `{}` is recorded FIRED — but CI-P30 (P-490) has not run at 2026-06-25",
                f.id
            );
            assert!(
                !f.built,
                "`{}` is recorded BUILT — but its measured trigger has not fired",
                f.id
            );
        }
    }

    /// Every trigger status is dated (an undated status is a claim that outlives its verification).
    #[test]
    fn every_trigger_status_is_dated() {
        for f in all_floor_followons() {
            let dated = f.status.dated();
            assert!(
                dated.contains("2026-"),
                "trigger status for `{}` is undated — `{}`",
                f.id,
                dated
            );
        }
    }
}
