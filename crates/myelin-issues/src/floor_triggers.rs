//! # `floor_triggers` — the MEASURED promotion triggers for the remaining floor follow-ons (ISS-P32 / P-495)
//!
//! **VISION §3 / external-insights/01 §7: promote a floor ONLY on a MEASURED trigger, never premature.**
//! The move-CRDT (R-3, [`crate::move_crdt`]) and the cross-cell portfolio rollup (R-7,
//! [`crate::cross_cell_rollup`]) ship a CONCRETE promotion in this prompt. The four REMAINING floor
//! follow-ons named for M5 ship the **MEASUREMENT SEAM, not the migration** — each is promoted only
//! when its measured signal fires, and below the trigger the floor stands. This module is the ONE place
//! those measured triggers live (the floor register made executable): a deployment wires the live signal
//! into each [`*Trigger::should_promote`] and the floor is promoted per-collection / per-tenant only on
//! the measured crossing.
//!
//! This is the doctrine's "name your floors WITH their measured trigger" made into code: the trigger is
//! a TYPE with a quantified threshold + a `should_promote` decision, not a prose note. The actual
//! migration (the materialised row, the distributed-SQL shard, the Monte-Carlo agent, the column-store
//! table) is NOT shipped speculatively — only the measurement that would fire it. Each `const TRIGGER`
//! names the floor and the prompt that fills it on the signal.
//!
//! ## The four measured triggers
//! - **Materialised rollup (R-4 / KN-3 / OQ-C).** [`MaterialisedRollupTrigger`] — materialise a
//!   subtree's rollup row only when the subtree is MEASURED large (the read-time floor,
//!   [`crate::rollup`], stays for small subtrees). Trigger: leaf-descendant count past the threshold.
//! - **Distributed-SQL (R-6).** [`DistributedSqlTrigger`] — migrate a tenant shard off PG only if it
//!   is MEASURED to outgrow PG (the PG-hybrid floor, [`crate::migrations`], is the complete v1).
//!   Trigger: shard size past the threshold. Never premature — ship the measurement, not the migration.
//! - **Monte-Carlo forecast (R-5 / ADR-08).** [`MonteCarloForecastTrigger`] — promote the linear
//!   `remaining ÷ velocity` forecast ([`crate::olap_feed`]) to a Monte-Carlo agent only when the
//!   throughput sample variance is MEASURED high enough that the linear point-estimate is misleading.
//!   The swap is a STRATEGY change reading the SAME OLAP samples, not a rewrite.
//! - **Event-volume column-store (EI-04 §5).** [`ColumnStoreTrigger`] — add a column-store seam for
//!   Issues' highest-volume streams (`issue.updated`, the change-log) only once the per-stream event
//!   VOLUME is MEASURED past the threshold. Added only on measured volume, not before.

/// **The materialised-rollup measured trigger (R-4 / KN-3 / OQ-C — `crate::rollup`).** The read-time
/// rollup floor recomputes a subtree's aggregate on every read — always fresh, cheap for SMALL
/// subtrees. This trigger MEASURES the subtree's leaf-descendant count; a subtree is materialised (a
/// derived rollup ROW persisted, refreshed on child change) only once it crosses
/// [`Self::MATERIALISE_THRESHOLD`]. Below it the read-time floor stands (the always-fresh common case).
#[derive(Clone, Copy, Debug, Default)]
pub struct MaterialisedRollupTrigger;

impl MaterialisedRollupTrigger {
    /// **The named measured trigger (the default-to-beat, calibrated by the ISS-D8a 10k-import drill +
    /// the at-scale surge family ISS-P33).** A subtree is materialised once its MEASURED leaf-descendant
    /// count is at or above this — the read-time recompute of a subtree this large is the measured cost
    /// the materialised row amortises. Below it the read-time floor is cheaper (no stale row to refresh).
    pub const MATERIALISE_THRESHOLD: u64 = 1_000;

    /// **The promotion decision — materialise iff the subtree is MEASURED large.** `true` iff
    /// `leaf_descendants >= MATERIALISE_THRESHOLD`. Below the threshold the read-time floor stands
    /// (VISION §3 — no premature materialisation).
    #[must_use]
    pub fn should_materialise(leaf_descendants: u64) -> bool {
        leaf_descendants >= Self::MATERIALISE_THRESHOLD
    }

    /// The floor + the prompt that fills it on the measured signal (the named follow-on).
    pub const TRIGGER: &'static str =
        "read-time rollup (small subtree) → materialise-on-measured-large (R-4 / KN-3 / OQ-C, ISS-P32 / P-495)";
}

/// **The distributed-SQL measured trigger (R-6 — `crate::migrations`).** The PG-hybrid (typed core +
/// JSONB + projection feeder) sharded-by-tenant store is the v1 floor. This trigger MEASURES a single
/// tenant's shard size; the distributed-SQL migration is provisioned ONLY if a single tenant's shard is
/// MEASURED to outgrow PG (past [`Self::SHARD_ROW_THRESHOLD`]). Never premature — ship the measurement,
/// not the migration, unless the trigger fires.
#[derive(Clone, Copy, Debug, Default)]
pub struct DistributedSqlTrigger;

impl DistributedSqlTrigger {
    /// **The named measured trigger (the default-to-beat — the single-shard PG ceiling).** A tenant
    /// shard is a candidate for distributed-SQL once its MEASURED row count is at or above this (the
    /// point a single PG shard's hot-path indexes are measured to degrade). This is a per-tenant
    /// measurement; the vast majority of tenants NEVER cross it (the PG floor is the whole requirement).
    pub const SHARD_ROW_THRESHOLD: u64 = 1_000_000_000;

    /// **The promotion decision — migrate iff a single tenant's shard is MEASURED to outgrow PG.**
    /// `true` iff `shard_rows >= SHARD_ROW_THRESHOLD`. Below it the PG-hybrid floor stands (VISION §3 —
    /// never a premature, speculative distributed-SQL migration).
    #[must_use]
    pub fn should_migrate(shard_rows: u64) -> bool {
        shard_rows >= Self::SHARD_ROW_THRESHOLD
    }

    /// The floor + the prompt that fills it on the measured signal (the named follow-on).
    pub const TRIGGER: &'static str =
        "PG-hybrid sharded-by-tenant → distributed-SQL on a measured shard outgrowing PG (R-6, ISS-P32 / P-495)";
}

/// **The Monte-Carlo forecast measured trigger (R-5 / ADR-08 — `crate::olap_feed`).** The linear
/// `remaining ÷ velocity` forecast over the OLAP throughput samples is the floor. This trigger MEASURES
/// the throughput sample variance (the coefficient of variation); the Monte-Carlo agent is promoted
/// only when the variance is MEASURED high enough that the linear POINT estimate is misleading (a wide
/// confidence interval the linear floor cannot express). The swap is a STRATEGY change reading the SAME
/// OLAP samples, not a rewrite ([`crate::olap_feed`] feeds both).
#[derive(Clone, Copy, Debug, Default)]
pub struct MonteCarloForecastTrigger;

impl MonteCarloForecastTrigger {
    /// **The named measured trigger (the default-to-beat — the variance ceiling the linear floor can
    /// honestly express).** The Monte-Carlo agent is promoted once the MEASURED throughput coefficient
    /// of variation (`stddev / mean`) is at or above this: below it the throughput is stable enough that
    /// the linear point estimate is a fair forecast; above it the variance demands a distribution (the
    /// Monte-Carlo confidence interval), not a point. `0.5` = the measured "the linear forecast is
    /// misleading" threshold.
    pub const VARIANCE_THRESHOLD: f64 = 0.5;

    /// **The promotion decision — promote iff the throughput variance is MEASURED high.** Given the
    /// throughput samples' `mean` and `stddev`, `true` iff the coefficient of variation
    /// (`stddev / mean`) is at or above [`Self::VARIANCE_THRESHOLD`]. A zero/empty mean never promotes
    /// (no samples → the floor stands). Below the threshold the linear floor stands (VISION §3).
    #[must_use]
    pub fn should_promote(mean: f64, stddev: f64) -> bool {
        if mean <= 0.0 {
            return false;
        }
        (stddev / mean) >= Self::VARIANCE_THRESHOLD
    }

    /// The floor + the prompt that fills it on the measured signal (the named follow-on).
    pub const TRIGGER: &'static str =
        "linear forecast (remaining ÷ velocity) → Monte-Carlo agent on measured high throughput variance \
         (R-5 / ADR-08, ISS-P32 / P-495)";
}

/// **The event-volume column-store measured trigger (EI-04 §5).** Issues' highest-volume streams
/// (`issue.updated`, the change-log) are served by the row-store OLAP feed ([`crate::olap_feed`]) by
/// default. This trigger MEASURES the per-stream event VOLUME; a column-store seam for a stream is added
/// only once its measured volume crosses [`Self::VOLUME_THRESHOLD`] (the point columnar scan-pruning
/// measurably beats the row store for the analytics queries). Added only on measured volume, not before.
#[derive(Clone, Copy, Debug, Default)]
pub struct ColumnStoreTrigger;

impl ColumnStoreTrigger {
    /// **The named measured trigger (the default-to-beat — the per-stream event-volume ceiling).** A
    /// column-store seam for a stream is added once its MEASURED cumulative event count is at or above
    /// this. Below it the row-store OLAP feed is the complete, cheaper analytics path (no second store
    /// to keep consistent).
    pub const VOLUME_THRESHOLD: u64 = 100_000_000;

    /// **The promotion decision — add a column-store seam iff the stream volume is MEASURED high.**
    /// `true` iff `stream_event_count >= VOLUME_THRESHOLD`. Below it the row-store OLAP floor stands
    /// (VISION §3 / EI-04 §5 — only once volume is MEASURED, not before).
    #[must_use]
    pub fn should_add_seam(stream_event_count: u64) -> bool {
        stream_event_count >= Self::VOLUME_THRESHOLD
    }

    /// The floor + the prompt that fills it on the measured signal (the named follow-on).
    pub const TRIGGER: &'static str =
        "row-store OLAP feed → event-volume column-store seam on measured high stream volume \
         (EI-04 §5, ISS-P32 / P-495)";
}

/// **The complete measured-promotion register for ISS-P32 (VISION §3 — every follow-on named WITH its
/// trigger).** A single place a reviewer reads to confirm every measured floor follow-on is named with
/// its measured trigger + the prompt that fills it. The two follow-ons that ship a CONCRETE promotion
/// in this prompt (move-CRDT R-3, cross-cell rollup R-7) are named here alongside the four that ship
/// the measurement seam (materialised R-4, distributed-SQL R-6, Monte-Carlo R-5, column-store EI-04 §5).
pub struct Iss32FloorRegister;

impl Iss32FloorRegister {
    /// The move-CRDT — PROMOTED (the conflict engine swap over the byte-identical order_key), gated by
    /// the measured concurrent-reorder trigger [`crate::move_crdt::ReorderPressure`].
    pub const MOVE_CRDT: &'static str = crate::move_crdt::MoveCrdtFloors::MEASURED_TRIGGER;

    /// The cross-cell portfolio rollup — PROMOTED (the cell-local rollup over the PII-free bridge).
    pub const CROSS_CELL_ROLLUP: &'static str =
        crate::cross_cell_rollup::CrossCellRollupFloors::CROSS_CELL_ROLLUP_RESOLVED;

    /// The DSR fan-out across member cells — PROMOTED (GA-D1 / CP-D7 / CP-D8).
    pub const DSR_FAN_OUT: &'static str =
        crate::cross_cell_rollup::CrossCellRollupFloors::DSR_FAN_OUT_RESOLVED;

    /// The materialised rollup — MEASUREMENT SEAM shipped (promoted on the measured-large trigger).
    pub const MATERIALISED_ROLLUP: &'static str = MaterialisedRollupTrigger::TRIGGER;

    /// Distributed-SQL — MEASUREMENT SEAM shipped (promoted on the measured shard-outgrows-PG trigger).
    pub const DISTRIBUTED_SQL: &'static str = DistributedSqlTrigger::TRIGGER;

    /// The Monte-Carlo forecast — MEASUREMENT SEAM shipped (promoted on the measured variance trigger).
    pub const MONTE_CARLO: &'static str = MonteCarloForecastTrigger::TRIGGER;

    /// The event-volume column-store — MEASUREMENT SEAM shipped (promoted on the measured volume trigger).
    pub const COLUMN_STORE: &'static str = ColumnStoreTrigger::TRIGGER;

    /// **The post-M5 follow-on (R-10) — named, not pulled in.** The real-LLM `LlmAgentRuntime` swap (the
    /// Monte-Carlo forecast agent's real-LLM runtime) is the post-M5 follow-on; the measured triggers
    /// above do not depend on it (they are pure strategy/measurement seams).
    pub const REAL_LLM_RUNTIME_POST_M5: &'static str =
        "the LlmAgentRuntime real-LLM swap is the post-M5 follow-on (R-10)";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every measured trigger promotes ONLY on its measured crossing (VISION §3 — the floor stands
    /// until the signal fires).** Below each threshold the floor is the complete, correct, cheaper
    /// path; at/above it the follow-on is promoted.
    #[test]
    fn each_floor_promotes_only_on_its_measured_crossing() {
        // materialised rollup: small subtree stays read-time; a measured-large one materialises.
        assert!(!MaterialisedRollupTrigger::should_materialise(50));
        assert!(MaterialisedRollupTrigger::should_materialise(
            MaterialisedRollupTrigger::MATERIALISE_THRESHOLD
        ));

        // distributed-SQL: a normal shard stays on PG; a measured-outgrowing one migrates.
        assert!(!DistributedSqlTrigger::should_migrate(10_000));
        assert!(DistributedSqlTrigger::should_migrate(
            DistributedSqlTrigger::SHARD_ROW_THRESHOLD
        ));

        // Monte-Carlo: stable throughput stays linear; high measured variance promotes.
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
            "no samples (zero mean) never promotes — the floor stands"
        );

        // column-store: a normal-volume stream stays row-store; a measured-high one adds the seam.
        assert!(!ColumnStoreTrigger::should_add_seam(1_000));
        assert!(ColumnStoreTrigger::should_add_seam(
            ColumnStoreTrigger::VOLUME_THRESHOLD
        ));
    }

    /// **The floor register names every follow-on WITH its trigger (the executable floor register).**
    /// The two promoted follow-ons + the DSR leg + the four measurement seams + the post-M5 R-10 are
    /// all named — the register a reviewer reads to confirm no floor is unnamed.
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
        // the promoted follow-ons reference the SAME named trigger their owning module names (no drift).
        assert_eq!(
            Iss32FloorRegister::MOVE_CRDT,
            crate::move_crdt::MoveCrdtFloors::MEASURED_TRIGGER
        );
    }
}
