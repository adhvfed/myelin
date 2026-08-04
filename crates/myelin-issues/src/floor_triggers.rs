#[derive(Clone, Copy, Debug, Default)]
pub struct MaterialisedRollupTrigger;

impl MaterialisedRollupTrigger {
    pub const MATERIALISE_THRESHOLD: u64 = 1_000;

    #[must_use]
    pub fn should_materialise(leaf_descendants: u64) -> bool {
        leaf_descendants >= Self::MATERIALISE_THRESHOLD
    }

    pub const TRIGGER: &'static str =
        "read-time rollup (small subtree) → materialise-on-measured-large (R-4 / KN-3 / OQ-C, ISS-P32 / P-495)";
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DistributedSqlTrigger;

impl DistributedSqlTrigger {
    pub const SHARD_ROW_THRESHOLD: u64 = 1_000_000_000;

    #[must_use]
    pub fn should_migrate(shard_rows: u64) -> bool {
        shard_rows >= Self::SHARD_ROW_THRESHOLD
    }

    pub const TRIGGER: &'static str =
        "PG-hybrid sharded-by-tenant → distributed-SQL on a measured shard outgrowing PG (R-6, ISS-P32 / P-495)";
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MonteCarloForecastTrigger;

impl MonteCarloForecastTrigger {
    pub const VARIANCE_THRESHOLD: f64 = 0.5;

    #[must_use]
    pub fn should_promote(mean: f64, stddev: f64) -> bool {
        if mean <= 0.0 {
            return false;
        }
        (stddev / mean) >= Self::VARIANCE_THRESHOLD
    }

    pub const TRIGGER: &'static str =
        "linear forecast (remaining ÷ velocity) → Monte-Carlo agent on measured high throughput variance \
         (R-5 / ADR-08, ISS-P32 / P-495)";
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ColumnStoreTrigger;

impl ColumnStoreTrigger {
    pub const VOLUME_THRESHOLD: u64 = 100_000_000;

    #[must_use]
    pub fn should_add_seam(stream_event_count: u64) -> bool {
        stream_event_count >= Self::VOLUME_THRESHOLD
    }

    pub const TRIGGER: &'static str =
        "row-store OLAP feed → event-volume column-store seam on measured high stream volume \
         (EI-04 §5, ISS-P32 / P-495)";
}

pub struct Iss32FloorRegister;

impl Iss32FloorRegister {
    pub const MOVE_CRDT: &'static str = crate::move_crdt::MoveCrdtFloors::MEASURED_TRIGGER;

    pub const CROSS_CELL_ROLLUP: &'static str =
        crate::cross_cell_rollup::CrossCellRollupFloors::CROSS_CELL_ROLLUP_RESOLVED;

    pub const DSR_FAN_OUT: &'static str =
        crate::cross_cell_rollup::CrossCellRollupFloors::DSR_FAN_OUT_RESOLVED;

    pub const MATERIALISED_ROLLUP: &'static str = MaterialisedRollupTrigger::TRIGGER;

    pub const DISTRIBUTED_SQL: &'static str = DistributedSqlTrigger::TRIGGER;

    pub const MONTE_CARLO: &'static str = MonteCarloForecastTrigger::TRIGGER;

    pub const COLUMN_STORE: &'static str = ColumnStoreTrigger::TRIGGER;

    pub const REAL_LLM_RUNTIME_POST_M5: &'static str =
        "the LlmAgentRuntime real-LLM swap is the post-M5 follow-on (R-10)";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_floor_promotes_only_on_its_measured_crossing() {
        assert!(!MaterialisedRollupTrigger::should_materialise(50));
        assert!(MaterialisedRollupTrigger::should_materialise(
            MaterialisedRollupTrigger::MATERIALISE_THRESHOLD
        ));

        assert!(!DistributedSqlTrigger::should_migrate(10_000));
        assert!(DistributedSqlTrigger::should_migrate(
            DistributedSqlTrigger::SHARD_ROW_THRESHOLD
        ));

        assert!(
            !MonteCarloForecastTrigger::should_promote(100.0, 10.0),
            "low variance (cov 0.1) stays on the linear floor"
        );
        assert!(
            MonteCarloForecastTrigger::should_promote(100.0, 80.0),
            "high variance (cov 0.8) promotes to Monte-Carlo"
        );
        assert!(
            !MonteCarloForecastTrigger::should_promote(0.0, 5.0),
            "no samples (zero mean) never promotes - the floor stands"
        );

        assert!(!ColumnStoreTrigger::should_add_seam(1_000));
        assert!(ColumnStoreTrigger::should_add_seam(
            ColumnStoreTrigger::VOLUME_THRESHOLD
        ));
    }

    #[test]
    fn the_floor_register_names_every_follow_on() {
        for named in [
            Iss32FloorRegister::MOVE_CRDT,
            Iss32FloorRegister::CROSS_CELL_ROLLUP,
            Iss32FloorRegister::DSR_FAN_OUT,
            Iss32FloorRegister::MATERIALISED_ROLLUP,
            Iss32FloorRegister::DISTRIBUTED_SQL,
            Iss32FloorRegister::MONTE_CARLO,
            Iss32FloorRegister::COLUMN_STORE,
            Iss32FloorRegister::REAL_LLM_RUNTIME_POST_M5,
        ] {
            assert!(
                !named.is_empty(),
                "every follow-on is named with its trigger"
            );
        }
        assert_eq!(
            Iss32FloorRegister::MOVE_CRDT,
            crate::move_crdt::MoveCrdtFloors::MEASURED_TRIGGER
        );
    }
}
