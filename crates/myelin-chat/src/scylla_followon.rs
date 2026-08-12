#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerStatus {
    NotFired { as_of: &'static str },
    Fired { evidence: &'static str },
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

pub const SCYLLA_HOT_TIER_FLOOR: FloorFollowOn = FloorFollowOn {
    id: "scylla-hot-tier",
    what: "a ScyllaDB (wide-column) message hot tier promoted from the Postgres-partitioned v1 \
           floor - the proven infinite-scale chat-log shape (Discord's Cassandra->ScyllaDB), \
           residency-pinned + crypto-shred-capable per cell",
    built_seam: "the MessageStore trait (store::MessageStore) - the hot-engine swap seam (arch 01 \
                 §3.1): append/range/revise/tombstone/resync_from is identical under any hot engine, \
                 and the cold tier (store::ColdSegments, now object-store-backed) is \
                 engine-independent; MemHotTier + PgMessageStore are the two v1 impls",
    preserved_contract: "11.4 - the per-subject DEK crypto-shred the Scylla tier must preserve; \
                         12.1/12.4 - the (tenant, region) partition + residency-pin per cell; the \
                         MessageStore trait surface (0 behavioural divergence). Only the hot engine \
                         behind the trait changes, never the contract",
    trigger: "a cell's MEASURED per-cell message-store write/partition volume crossing the hot-tier \
              budget (R-C6/R-5); the measure-before-shard mandate (ADR-10) - not before measured",
    status: TriggerStatus::NotFired {
        as_of: "2026-06-25: NO per-cell message-store write/partition-volume measurement against a \
                hot-tier budget exists. The chat M5 surge family (CHAT-P26 / P-500) measured the \
                gateway SHED budgets (ConnectionTier/AgentMention lanes), NOT the message-store \
                write/partition volume crossing a hot-tier budget; a cell bounds the scale (one \
                region's tenants, ADR-11, not the planet), so the Postgres-partitioned hot tier is \
                correct. Floor remains named (measured-not-predicted: the measurement is owed).",
    },
    follow_on: "add a third MessageStore impl (ScyllaMessageStore) behind the unchanged trait, \
                residency-pinned + crypto-shred-capable per cell; migrate the hot partitions; the \
                object-segment cold tier is unchanged (engine-independent)",
    promotion_gate: "CHAT-D2 (per-conversation total order) + CHAT-D8 (0 recoverable PII) re-run \
                     GREEN across the swap - the order-violation + recoverable-PII signals = 0 \
                     post-swap (the drills were written to survive the swap) - SCHED",
    built: false,
};

pub const MEASURED_TRIGGER_FLOORS: &[FloorFollowOn] = &[SCYLLA_HOT_TIER_FLOOR];

pub fn scylla_floor_gap_report() -> Result<(), String> {
    gap_report_over(MEASURED_TRIGGER_FLOORS)
}

fn gap_report_over(floors: &[FloorFollowOn]) -> Result<(), String> {
    for f in floors {
        if !f.is_fully_recorded() {
            return Err(format!(
                "floor follow-on `{}` is an invisible gap (a must-be-non-empty field is empty)",
                f.id
            ));
        }
        if !f.honours_no_premature_promotion() {
            return Err(format!(
                "floor follow-on `{}` is a PREMATURE promotion - built with an unfired trigger \
                 (EI-04 §5 forbids adding it before the measurement)",
                f.id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scylla_floor_is_recorded_with_no_invisible_gap() {
        let ids: Vec<&str> = MEASURED_TRIGGER_FLOORS.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["scylla-hot-tier"]);
        let floor = std::hint::black_box(SCYLLA_HOT_TIER_FLOOR);
        assert!(
            floor.is_fully_recorded(),
            "the Scylla hot-tier floor must be fully recorded (no must-be-non-empty field empty)"
        );
    }

    #[test]
    fn no_premature_promotion_trigger_unfired_stays_a_floor() {
        let floor = std::hint::black_box(SCYLLA_HOT_TIER_FLOOR);
        assert!(
            !floor.status.has_fired(),
            "the measured per-cell write/partition-volume trigger has NOT fired at this prompt's \
             execution"
        );
        assert!(
            !floor.built,
            "the Scylla hot-tier promotion is a NAMED FLOOR - not built speculatively"
        );
        assert!(
            floor.honours_no_premature_promotion(),
            "the honest-floor invariant holds: ¬fired ⇒ ¬built"
        );
        scylla_floor_gap_report().expect("the gap-report is honest - 0 invisible gaps");
    }

    #[test]
    fn the_trigger_names_the_measured_signal_and_is_dated() {
        let floor = std::hint::black_box(SCYLLA_HOT_TIER_FLOOR);
        assert!(floor.trigger.contains("write/partition volume"));
        assert!(floor.trigger.contains("ADR-10"));
        assert!(!floor.status.dated().is_empty());
        assert!(floor.preserved_contract.contains("11.4"));
        assert!(floor.preserved_contract.contains("residency-pin"));
    }

    fn good_floor() -> FloorFollowOn {
        FloorFollowOn {
            id: "x",
            what: "x",
            built_seam: "x",
            preserved_contract: "x",
            trigger: "x",
            status: TriggerStatus::NotFired { as_of: "x" },
            follow_on: "x",
            promotion_gate: "x",
            built: false,
        }
    }

    #[test]
    fn is_fully_recorded_catches_each_empty_field() {
        assert!(good_floor().is_fully_recorded());
        let breakers: Vec<(&str, FloorFollowOn)> = vec![
            (
                "id",
                FloorFollowOn {
                    id: "",
                    ..good_floor()
                },
            ),
            (
                "what",
                FloorFollowOn {
                    what: "",
                    ..good_floor()
                },
            ),
            (
                "built_seam",
                FloorFollowOn {
                    built_seam: "",
                    ..good_floor()
                },
            ),
            (
                "preserved_contract",
                FloorFollowOn {
                    preserved_contract: "",
                    ..good_floor()
                },
            ),
            (
                "trigger",
                FloorFollowOn {
                    trigger: "",
                    ..good_floor()
                },
            ),
            (
                "status.dated",
                FloorFollowOn {
                    status: TriggerStatus::NotFired { as_of: "" },
                    ..good_floor()
                },
            ),
            (
                "follow_on",
                FloorFollowOn {
                    follow_on: "",
                    ..good_floor()
                },
            ),
            (
                "promotion_gate",
                FloorFollowOn {
                    promotion_gate: "",
                    ..good_floor()
                },
            ),
        ];
        for (field, broken) in breakers {
            assert!(
                !broken.is_fully_recorded(),
                "an empty `{field}` must make the row an invisible gap (the conjunct is load-bearing)"
            );
        }
    }

    #[test]
    fn honest_floor_invariant_and_has_fired_are_load_bearing() {
        let unfired = TriggerStatus::NotFired { as_of: "d" };
        let fired = TriggerStatus::Fired { evidence: "e" };
        assert!(!unfired.has_fired());
        assert!(fired.has_fired());
        assert_eq!(unfired.dated(), "d");
        assert_eq!(fired.dated(), "e");

        let premature = FloorFollowOn {
            built: true,
            status: unfired,
            ..good_floor()
        };
        assert!(!premature.honours_no_premature_promotion());
        let honest_built = FloorFollowOn {
            built: true,
            status: fired,
            ..good_floor()
        };
        assert!(honest_built.honours_no_premature_promotion());
        assert!(good_floor().honours_no_premature_promotion());
    }

    #[test]
    fn gap_report_verdict_distinguishes_honest_from_broken() {
        assert!(scylla_floor_gap_report().is_ok());
        assert!(gap_report_over(&[good_floor()]).is_ok());
        let invisible_gap = FloorFollowOn {
            id: "gap-row",
            trigger: "",
            ..good_floor()
        };
        let err = gap_report_over(&[invisible_gap]).expect_err("an invisible gap is an Err");
        assert!(err.contains("gap-row") && err.contains("invisible gap"));
        let premature = FloorFollowOn {
            id: "premature-row",
            built: true,
            status: TriggerStatus::NotFired { as_of: "d" },
            ..good_floor()
        };
        let err = gap_report_over(&[premature]).expect_err("a premature promotion is an Err");
        assert!(err.contains("premature-row") && err.contains("PREMATURE"));
    }
}
