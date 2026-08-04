#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerStatus {
    NotFired {
        as_of: &'static str,
    },
    Fired {
        evidence: &'static str,
    },
}

impl TriggerStatus {
    pub fn has_fired(&self) -> bool {
        matches!(self, TriggerStatus::Fired { .. })
    }

    pub fn dated(&self) -> &'static str {
        match self {
            TriggerStatus::NotFired { as_of } => as_of,
            TriggerStatus::Fired { evidence } => evidence,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FloorFollowOn {
    pub id: &'static str,
    pub what: &'static str,
    pub built_seam: &'static str,
    pub preserved_contract: &'static str,
    pub trigger: &'static str,
    pub status: TriggerStatus,
    pub follow_on: &'static str,
    pub promotion_gate: &'static str,
    pub built: bool,
}

impl FloorFollowOn {
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

    pub fn honours_no_premature_promotion(&self) -> bool {
        !self.built || self.status.has_fired()
    }
}

pub const MEASURED_TRIGGER_FLOORS: &[FloorFollowOn] = &[
    FloorFollowOn {
        id: "time-series-log-tier",
        what: "a dedicated time-series / wide-column log tier promoted from the object-segment T3 \
               floor - the storage/index engine for the highest-volume firehose log stream",
        built_seam: "the firehose -> sealed T2 content-addressed segment -> log_segment + \
                     log_anchor (job, step, byte-range) OLTP index (log_pipeline, 11.8); the \
                     details_ref jump-to-failure resolves (job, step, byte-range) -> bytes today",
        preserved_contract: "11.8 - the (job, step, byte-range) addressability: the details_ref \
                             still resolves through the new tier (0 dangling step anchors); the \
                             migration loses 0 log bytes. Only the engine behind the contract \
                             changes, never the contract",
        trigger: "CI-P30 (P-490) MEASURES firehose log-stream event volume outgrowing the \
                  OLTP-indexed object-segment tier (EI-04 §5 - not before measured)",
        status: TriggerStatus::NotFired {
            as_of: "2026-06-25: CI-P30 (P-490) has not run - no firehose log-volume-vs-OLTP \
                    measurement exists; per EI-04 §5 the tier is NOT built before the measurement. \
                    Floor remains named.",
        },
        follow_on: "swap the (job, step, byte-range) index/storage engine to a time-series / \
                    wide-column tier behind the unchanged 11.8 contract; migrate sealed segments \
                    losing 0 bytes; re-point details_ref resolution at the new tier",
        promotion_gate: "the (job, step, byte-range) details_ref still resolves through the new \
                         tier (0 dangling step anchors) AND the migration loses 0 log bytes - CI",
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
        trigger: "CI-P30 (P-490) MEASURES a per-fair_key starvation signal under the 30x surge - \
                  the wait-time histogram crossing its tuned threshold (open question 07#1)",
        status: TriggerStatus::NotFired {
            as_of: "2026-06-25: CI-P30 (P-490) HAS RUN - the 30x CI-D2 surge MEASURED the \
                    per-fair_key wait-time histogram (surge::StarvationHistogram); the wait p99 \
                    stayed WITHIN the tuned starvation trigger (ci_surge.starvation_wait_p99_max_ticks \
                    = 32 ticks), so flat DRR fairly interleaves the surging tenant - NO starvation \
                    fired. ci_surge.hierarchical_scheduler_promotion_owed = false. Floor remains \
                    named (measured-not-predicted: the trigger did NOT fire).",
        },
        follow_on: "refine the flat DRR claim predicate into a per-tenant -> per-project -> \
                    per-pipeline hierarchical DRR behind the unchanged claim seam, only on the \
                    measured starvation signal",
        promotion_gate: "the per-fair_key starvation histogram improves vs flat DRR under the \
                         same surge (the measured starvation signal clears) - CI",
        built: false,
    },
];

pub const DEFERRED_BY_REFERENCE_FLOORS: &[FloorFollowOn] = &[
    FloorFollowOn {
        id: "cross-cell-spanning-pipelines",
        what: "a pipeline whose jobs span cells of a multi-cell tenant",
        built_seam: "single-cell pipelines ship v1; the cross-cell PII-free pointer bridge (12.6) \
                     is the inherited mechanism",
        preserved_contract: "12.6 - the cross-cell PII-free pointer bridge frame (a CI run \
                             inherits it; no PII crosses a cell boundary, references only)",
        trigger: "the OQ-I multi-cell bridge (12.6) lifts in M5's shared work + cross-cell demand",
        status: TriggerStatus::NotFired {
            as_of: "2026-06-25: designed-not-built - named by reference; the 12.6 bridge lifts in \
                    M5 shared work. Floor remains named (handled by reference, not this prompt).",
        },
        follow_on: "CI runs inherit the 12.6 bridge so a multi-cell tenant's pipeline can span \
                    cells, references-not-payloads across the boundary",
        promotion_gate: "a cross-cell pipeline run carries no PII across a cell boundary \
                         (references only) and resolves through the 12.6 bridge - CI",
        built: false,
    },
    FloorFollowOn {
        id: "slsa-l3-plus-hermetic",
        what: "SLSA L3+ / hermetic (two-party) provenance",
        built_seam: "SLSA L1-L2 provenance + SBOM ship v1 (supply_chain)",
        preserved_contract: "the provenance/attestation seam (the supply-chain attestation shape) \
                             - L3+ strengthens the isolation/non-falsifiability, not the shape",
        trigger: "customer demand (demand-triggered)",
        status: TriggerStatus::NotFired {
            as_of: "2026-06-25: demand-triggered - named by reference; no demand signal recorded. \
                    Floor remains named (handled by reference, not this prompt).",
        },
        follow_on: "hermetic / two-party (L3+) provenance over the existing attestation seam",
        promotion_gate: "the build provenance meets SLSA L3+ (hermetic, non-falsifiable) - CI",
        built: false,
    },
];

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

    #[test]
    fn the_two_measured_trigger_promotions_are_recorded() {
        let ids: Vec<&str> = MEASURED_TRIGGER_FLOORS.iter().map(|f| f.id).collect();
        assert_eq!(
            ids,
            vec!["time-series-log-tier", "hierarchical-scheduler"],
            "the two CI-P29 promotions must be the time-series log tier + the hierarchical scheduler"
        );
    }

    #[test]
    fn the_two_deferred_floors_are_named_by_reference() {
        let ids: Vec<&str> = DEFERRED_BY_REFERENCE_FLOORS.iter().map(|f| f.id).collect();
        assert_eq!(
            ids,
            vec!["cross-cell-spanning-pipelines", "slsa-l3-plus-hermetic"],
            "the two deferred floors named by reference must be cross-cell pipelines + SLSA L3+"
        );
    }

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

    #[test]
    fn no_premature_promotion_every_trigger_unfired_stays_a_floor() {
        for f in all_floor_followons() {
            assert!(
                f.honours_no_premature_promotion(),
                "floor follow-on `{}` is a PREMATURE promotion - built with an unfired trigger \
                 (EI-04 §5 forbids adding it before the measurement)",
                f.id
            );
            assert!(
                !f.status.has_fired(),
                "trigger for `{}` is recorded FIRED - but CI-P30 (P-490) has not run at 2026-06-25",
                f.id
            );
            assert!(
                !f.built,
                "`{}` is recorded BUILT - but its measured trigger has not fired",
                f.id
            );
        }
    }

    #[test]
    fn every_trigger_status_is_dated() {
        for f in all_floor_followons() {
            let dated = f.status.dated();
            assert!(
                dated.contains("2026-"),
                "trigger status for `{}` is undated - `{}`",
                f.id,
                dated
            );
        }
    }
}
