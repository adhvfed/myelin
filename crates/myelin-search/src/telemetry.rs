//! # The Search telemetry contract — the full §4.11 signal set on the metrics-health port
//! (SRCH-P14 → global P-177, M2; contract 1.8).
//!
//! **Owning architecture doc:** `search-and-indexing.md`
//! §4.11 (the telemetry contract — the §4.11 signal set Search exports on its metrics-health
//! port): *index lag; query latency RED per principal-kind+tenant (FT/structured/vector/hybrid);
//! `list_objects` call rate + cache hit ratio + the filter-mode split (`Ids` vs `Filter`/`TupleSet`);
//! zero-escape assertion counters (zookie-bypass, stale-served); reindex progress + cold-vs-live
//! parity hash; erase receipts + vector-tombstone/compaction lag; consumer lag (`num_pending`);
//! per-tenant in-flight + shed counts.*
//!
//! **Contract-index:** row 1.8 (the telemetry signal set + the filter-mode split). Search owns NO
//! contract crate — 1.8 is the harness's; Search **emits** the signals. The harness's
//! telemetry-assertion library (`myelin_harness::telemetry::SignalSource`, P-S04) is the consumer
//! that reads each signal back — every later Search drill (SRCH-D1/D2/D3/D4/D5/D6/D7/…) asserts
//! against this surface, so a **missing signal is a failed drill** (observability is part of the
//! pass, EI-01 §3: "a system that survives a drill but emits no signal has FAILED it").
//!
//! ## What this slice ships — the AGGREGATOR, not new mechanism
//! The individual survival counters already live on each slice's stats type (the slices that
//! produce them, EI-01 §7 — never re-define a signal):
//!   - **index lag** + **consumer lag (`num_pending`)** → [`crate::indexer::IncrementalIndexer`]
//!     (SRCH-P06): `index_lag()` is the named `search.index_lag`; the consumer lag is the runtime's
//!     [`myelin_events::Consumer::lag`] the indexer is driven by.
//!   - **`list_objects` call rate** + the **filter-mode split (`Ids` vs `Filter`/`TupleSet`)** +
//!     the reverse-index JOIN count → [`crate::pipeline::QueryStats`] (SRCH-P08/P09).
//!   - **zero-escape counters** (excluded-stale / fail-static bypass vs served) →
//!     [`crate::consistency::ConsistencyStats`] (SRCH-P09/P10).
//!   - **cache hit ratio** → [`crate::cache::CacheStats`] (SRCH-P13).
//!   - **vector-tombstone / compaction lag** → [`crate::vector::HnswVectorIndex`] (SRCH-P05):
//!     `physical_len() - live_len()` (tombstoned-until-compact vectors).
//!
//! This module is the **one place** those scattered counters are folded into the single §4.11
//! snapshot ([`SearchTelemetry`]) the metrics-health port exports, named to the contract-1.8 set
//! so the harness reads them by the frozen names. It is the green-artifact source every Search
//! drill reads.
//!
//! ## The producer / consumer split (why the snapshot is a value, not a `SignalSource`)
//! `myelin-harness` is the TEST-SUPPORT failure-injection harness — its own crate docs are explicit
//! that *"nothing depends on myelin-harness; it must never appear in a production crate's
//! dependencies"*. So the **producer** side (this prod module) is a self-contained typed snapshot:
//! it exposes each §4.11 signal as a named integer (`i64`, the predicate-readable shape the harness
//! asserts on) plus its frozen NAME constant. The **consumer** side — the telemetry-assertion
//! library `SignalSource` — reads the snapshot in the test (`tests/telemetry_srch_p14_signal_set.rs`),
//! populating one `SignalSource` per signal and `assert_signal`/`assert_labelled`-ing each. The names
//! and units match the harness's `SignalName` set, so when the real OTLP metrics-health export lands
//! (the producer-transport floor, below) the SAME names are exported. This is the identical
//! producer/consumer split the substrate `serve::Telemetry` uses (architecture §3.5).
//!
//! ## FLOOR named (SRCH-P14 — "none new", per the prompt) — the signal SHAPE is full at M2; some
//! producers are not yet fully exercised:
//! - **reindex progress + cold-vs-live parity hash** is the SHAPE here; the producer that fully
//!   exercises it (a real reindex that re-emits from source and compares cold==live byte-parity) is
//!   **SRCH-P16** (reindex-from-source) — the parity hash field defaults to the not-yet-reindexed
//!   sentinel until then.
//! - **erase receipts + the at-scale vector-tombstone/compaction lag** is the SHAPE here; the
//!   producer that fully exercises it (the real `erase` = purge + reindex, vectors compacted,
//!   restrict suppression) is **SRCH-P15** — the erase-receipt count defaults to 0 until a real
//!   erase runs. The vector-tombstone/compaction lag IS live now (a soft-delete bumps it; a compact
//!   clears it) — SRCH-P05's `physical_len()-live_len()`.
//! - **per-tenant in-flight + shed counts** is the SHAPE here; the at-scale surge that drives the
//!   shed lane (the protected-human-lane shed order under 30× load) is **SRCH-P25** (SRCH-D6). The
//!   in-flight gauge is live now (a query in the pipeline increments it); the shed count is 0 until a
//!   surge sheds.
//! - **the real OpenTelemetry/OTLP export on the metrics-health LISTENER** is the substrate
//!   producer-transport floor (P-S13/P-S14): here the snapshot is exported in-process by the same
//!   typed-meter shape `serve::Telemetry` uses, exercising the signal NAMES end-to-end now.
//!
//! State this so the signal set is **not mistaken for fully-exercised** before the
//! erasure/reindex/surge slices (SRCH-P15..SRCH-P32) land. The SHAPE is complete and emitted now.

use crate::cache::CacheStats;
use crate::consistency::ConsistencyStats;
use crate::pipeline::QueryStats;

/// The frozen contract-1.8 / §4.11 Search signal NAMES, exported on the metrics-health port. Each
/// is the name the harness's telemetry-assertion library reads the value under (and the name the
/// real OTLP meter exports when the transport floor lands). Public so a drill can name the signal it
/// asserts (observability is part of the pass — a drill that does not name its signal is not a pass).
pub mod signal {
    /// `search.index_lag` — events delivered to the indexer but not yet projected (§4.11 row 1;
    /// SRCH-P06). The drill asserts it returns to 0 in steady state.
    pub const INDEX_LAG: &str = "search.index_lag";
    /// `search.query_rate` — the query RED *rate* leg, per principal-kind + tenant + surface
    /// (FT/structured/vector/hybrid) (§4.11 row 2). Labelled.
    pub const QUERY_RATE: &str = "search.query_rate";
    /// `search.query_errors` — the query RED *errors* leg, per principal-kind + tenant + surface
    /// (§4.11 row 2). Labelled. The human lane must hold at 0 while a surge sheds the agent lane.
    pub const QUERY_ERRORS: &str = "search.query_errors";
    /// `search.query_duration_ms` — the query RED *duration* leg (a representative latency the drill
    /// reads), per principal-kind + tenant + surface (§4.11 row 2). Labelled.
    pub const QUERY_DURATION_MS: &str = "search.query_duration_ms";
    /// `search.list_objects_rate` — the `list_objects` (4.3) call rate (§4.11 row 3). The no-N+1
    /// invariant: exactly ONE call per query (never one authz call per result).
    pub const LIST_OBJECTS_RATE: &str = "search.list_objects_rate";
    /// `search.filter_mode.ids` — the `Ids` leg of the filter-mode split (§4.11 row 3; the
    /// materialised S4 allow-set path). Read against [`FILTER_MODE_FILTER`].
    pub const FILTER_MODE_IDS: &str = "search.filter_mode.ids";
    /// `search.filter_mode.filter` — the `Filter`/`TupleSet` leg of the filter-mode split (§4.11
    /// row 3; the pushed-down `SetExpr` path, incl. the relational reverse-index JOINs).
    pub const FILTER_MODE_FILTER: &str = "search.filter_mode.filter";
    /// `search.cache_hit_ratio_pct` — the result/filter cache hit ratio, 0..=100 (§4.11 row 3;
    /// SRCH-P13). `-1` before any cacheable read (no ratio over zero — never a fabricated 100).
    pub const CACHE_HIT_RATIO_PCT: &str = "search.cache_hit_ratio_pct";
    /// `search.zero_escape.zookie_bypass` — the zero-escape counter: strong (zookie-stamped) reads
    /// that BYPASSED the fail-static cache (§4.11 row 4; SRCH-P10). The SRCH-D2 green artifact.
    pub const ZERO_ESCAPE_ZOOKIE_BYPASS: &str = "search.zero_escape.zookie_bypass";
    /// `search.zero_escape.stale_served` — the zero-escape counter: stale candidates EXCLUDED under
    /// staleness (a just-revoked grant kept out — §4.11 row 4; SRCH-P09). The SRCH-D1/D2 green
    /// artifact. The "stale-served" *leak* this counts is the EXCLUSIONS (kept-out), never a served
    /// stale grant — a non-zero exclusion count is the new-enemy being kept out, the property holding.
    pub const ZERO_ESCAPE_STALE_EXCLUDED: &str = "search.zero_escape.stale_excluded";
    /// `search.reindex.parity_hash` — the cold-vs-live parity hash (§4.11 row 5; the reindex
    /// recovery path's byte-parity proof, SRCH-P16). A 64-bit hash; `0` = no reindex has run yet.
    pub const REINDEX_PARITY_HASH: &str = "search.reindex.parity_hash";
    /// `search.erase_receipts` — the count of holder `erase` receipts produced (§4.11 row 6; the
    /// real purge+reindex erase is SRCH-P15). `0` until a real erase runs.
    pub const ERASE_RECEIPTS: &str = "search.erase_receipts";
    /// `search.vector_compaction_lag` — vector-tombstone/compaction lag: soft-deleted (tombstoned)
    /// vectors not yet physically removed by a compact (§4.11 row 6; SRCH-P05). Returns to 0 after a
    /// compact (no orphan embedding survives a compaction). Labelled by `{tenant}`.
    pub const VECTOR_COMPACTION_LAG: &str = "search.vector_compaction_lag";
    /// `search.consumer_lag` — the indexer's `num_pending` un-acked backlog (§4.11 row 7; the
    /// runtime's [`myelin_events::Consumer::lag`]). Labelled by `{consumer}`.
    pub const CONSUMER_LAG: &str = "search.consumer_lag";
    /// `search.in_flight` — per-tenant in-flight query count (§4.11 row 8). Labelled by `{tenant}`.
    pub const IN_FLIGHT: &str = "search.in_flight";
    /// `search.shed_count` — per-tenant/per-lane shed count under load (§4.11 row 8; the
    /// protected-human-lane shed order, fully driven at 30× in SRCH-P25). Labelled by `{tenant}`.
    pub const SHED_COUNT: &str = "search.shed_count";

    /// The full §4.11 Search signal-NAME set — the exhaustive list the metrics-health port exports.
    /// A drill / the telemetry-assertion test reads EACH of these; a missing one fails the gate
    /// (observability is part of the pass). 16 names: index-lag + RED×3 (rate/errors/duration) +
    /// list_objects-rate + filter-mode×2 + cache-hit + zero-escape×2 + reindex-parity +
    /// erase-receipts + vector-compaction-lag + consumer-lag + in-flight + shed.
    pub const ALL: [&str; 16] = [
        INDEX_LAG,
        QUERY_RATE,
        QUERY_ERRORS,
        QUERY_DURATION_MS,
        LIST_OBJECTS_RATE,
        FILTER_MODE_IDS,
        FILTER_MODE_FILTER,
        CACHE_HIT_RATIO_PCT,
        ZERO_ESCAPE_ZOOKIE_BYPASS,
        ZERO_ESCAPE_STALE_EXCLUDED,
        REINDEX_PARITY_HASH,
        ERASE_RECEIPTS,
        VECTOR_COMPACTION_LAG,
        CONSUMER_LAG,
        IN_FLIGHT,
        SHED_COUNT,
    ];
}

/// The sentinel for a cache hit ratio that has no ratio yet (no cacheable read) — `-1`, never a
/// fabricated `100`. The harness reads this as "absent ratio" (a drill that needs the ratio fails
/// loudly on `-1` rather than silently passing on a made-up value).
pub const CACHE_RATIO_ABSENT: i64 = -1;

/// One labelled signal value (e.g. `query_errors{kind=human, tenant=acme}` = 0). The label set is
/// PII-free identifiers only (a tenant id, a principal-KIND, a surface name, a consumer name, a
/// lane name) — a telemetry label is `control-plane-pii-free` by construction (the same discipline
/// as the substrate dependency-break labels). `value` is the predicate-readable `i64`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelledSignal {
    /// The frozen signal NAME (one of [`signal`]).
    pub name: &'static str,
    /// The `(key, value)` label set, sorted so a read is order-independent.
    pub labels: Vec<(String, String)>,
    /// The signal value (count / age-secs / ratio-pct / parity-hash). Predicate-readable `i64`.
    pub value: i64,
}

impl LabelledSignal {
    fn new(name: &'static str, labels: Vec<(String, String)>, value: i64) -> LabelledSignal {
        let mut labels = labels;
        labels.sort();
        LabelledSignal { name, labels, value }
    }
}

/// **The full §4.11 Search telemetry snapshot — the contract-1.8 signal set exported on the
/// metrics-health port (SRCH-P14).** The single typed value the harness's telemetry-assertion
/// library reads every signal off. Scalar signals are named fields; the per-(principal-kind,
/// tenant, surface) / per-tenant / per-consumer signals are the [`Self::labelled`] vector.
///
/// Built by folding the per-slice stats ([`SearchTelemetry::from_stats`]) + the live gauges into the
/// one snapshot, so the scattered counters become the one observable surface every drill reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTelemetry {
    /// `search.index_lag` (scalar) — events delivered to the indexer but not yet projected.
    pub index_lag: i64,
    /// `search.list_objects_rate` (scalar) — the count of `list_objects` calls (the no-N+1
    /// invariant reads this against the query count: exactly one per query).
    pub list_objects_rate: i64,
    /// `search.filter_mode.ids` (scalar) — the `Ids` leg of the filter-mode split.
    pub filter_mode_ids: i64,
    /// `search.filter_mode.filter` (scalar) — the `Filter`/`TupleSet` leg of the filter-mode split.
    pub filter_mode_filter: i64,
    /// `search.cache_hit_ratio_pct` (scalar, 0..=100, or [`CACHE_RATIO_ABSENT`]).
    pub cache_hit_ratio_pct: i64,
    /// `search.zero_escape.zookie_bypass` (scalar) — strong reads that bypassed the fail-static cache.
    pub zero_escape_zookie_bypass: i64,
    /// `search.zero_escape.stale_excluded` (scalar) — stale candidates excluded (new-enemy kept out).
    pub zero_escape_stale_excluded: i64,
    /// `search.reindex.parity_hash` (scalar) — the cold-vs-live parity hash (`0` = no reindex yet).
    pub reindex_parity_hash: i64,
    /// `search.erase_receipts` (scalar) — holder `erase` receipts produced (`0` until SRCH-P15 erase).
    pub erase_receipts: i64,
    /// The labelled signals: RED (rate/errors/duration) per `{kind,tenant,surface}`; consumer lag
    /// per `{consumer}`; vector-compaction-lag / in-flight / shed per `{tenant}`.
    pub labelled: Vec<LabelledSignal>,
}

/// The labels a RED (rate/errors/duration) signal is read under (contract 1.8: RED **per
/// principal-kind per tenant**, plus the Search surface FT/structured/vector/hybrid). PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedLabels<'a> {
    /// The principal KIND (`human` / `agent` / `service` / `ci`) — never a principal id (PII-free).
    pub kind: &'a str,
    /// The tenant id (PII-free identifier; the partition key).
    pub tenant: &'a str,
    /// The Search surface: `ft` / `structured` / `vector` / `hybrid` (§4.11 RED-per-surface).
    pub surface: &'a str,
}

impl<'a> RedLabels<'a> {
    fn to_labels(self) -> Vec<(String, String)> {
        vec![
            ("kind".to_string(), self.kind.to_string()),
            ("tenant".to_string(), self.tenant.to_string()),
            ("surface".to_string(), self.surface.to_string()),
        ]
    }
}

impl SearchTelemetry {
    /// An empty snapshot — every scalar at its initial zero, the cache ratio absent, no labelled
    /// signals. The metrics-health port starts emitting the moment the service boots (observability
    /// is part of the pass — a fresh service still emits the set, every value at its baseline).
    pub fn empty() -> SearchTelemetry {
        SearchTelemetry {
            index_lag: 0,
            list_objects_rate: 0,
            filter_mode_ids: 0,
            filter_mode_filter: 0,
            cache_hit_ratio_pct: CACHE_RATIO_ABSENT,
            zero_escape_zookie_bypass: 0,
            zero_escape_stale_excluded: 0,
            reindex_parity_hash: 0,
            erase_receipts: 0,
            labelled: Vec::new(),
        }
    }

    /// **Fold the per-slice stats into the §4.11 snapshot (the aggregator, EI-01 §7 — reuse the
    /// existing counters, never re-define them).** Reads the scalar §4.11 signals off the slices
    /// that produce them:
    ///   - `index_lag` ← the indexer (SRCH-P06).
    ///   - the `list_objects` rate + the filter-mode split ← [`QueryStats`] (SRCH-P08/P09).
    ///   - the zero-escape counters ← [`ConsistencyStats`] (SRCH-P09/P10).
    ///   - the cache hit ratio ← [`CacheStats`] (SRCH-P13).
    ///
    /// The labelled signals (RED, consumer-lag, vector-compaction-lag, in-flight, shed) are added
    /// by the live emitters via [`Self::record_red`] / [`Self::set_consumer_lag`] /
    /// [`Self::set_vector_compaction_lag`] / [`Self::set_in_flight`] / [`Self::set_shed_count`].
    pub fn from_stats(
        index_lag: u64,
        qstats: &QueryStats,
        cstats: &ConsistencyStats,
        cache: &CacheStats,
    ) -> SearchTelemetry {
        SearchTelemetry {
            index_lag: index_lag as i64,
            list_objects_rate: qstats.list_objects_calls() as i64,
            filter_mode_ids: qstats.ids_mode_count() as i64,
            filter_mode_filter: qstats.filter_mode_count() as i64,
            cache_hit_ratio_pct: cache
                .hit_ratio_pct()
                .map(|p| p as i64)
                .unwrap_or(CACHE_RATIO_ABSENT),
            zero_escape_zookie_bypass: cstats.fail_static_bypassed() as i64,
            zero_escape_stale_excluded: cstats.excluded_stale() as i64,
            reindex_parity_hash: 0,
            erase_receipts: 0,
            labelled: Vec::new(),
        }
    }

    /// Set the cold-vs-live reindex parity hash (§4.11 row 5; the SRCH-P16 producer). `0` is the
    /// "no reindex has run yet" sentinel.
    pub fn set_reindex_parity_hash(&mut self, hash: i64) {
        self.reindex_parity_hash = hash;
    }

    /// Set the holder erase-receipt count (§4.11 row 6; the SRCH-P15 producer).
    pub fn set_erase_receipts(&mut self, count: u64) {
        self.erase_receipts = count as i64;
    }

    /// Record a RED labelled triple `(rate, errors, duration_ms)` for a `{kind, tenant, surface}`
    /// (contract 1.8: RED per principal-kind per tenant, per Search surface). The drill asserts
    /// `query_errors{kind=human, …} == 0` while a surge sheds the agent lane.
    pub fn record_red(&mut self, labels: RedLabels<'_>, rate: u64, errors: u64, duration_ms: u64) {
        let l = labels.to_labels();
        self.set_labelled(signal::QUERY_RATE, l.clone(), rate as i64);
        self.set_labelled(signal::QUERY_ERRORS, l.clone(), errors as i64);
        self.set_labelled(signal::QUERY_DURATION_MS, l, duration_ms as i64);
    }

    /// Set the indexer consumer lag (`num_pending`) for a named consumer (§4.11 row 7; the runtime's
    /// [`myelin_events::Consumer::lag`]).
    pub fn set_consumer_lag(&mut self, consumer: &str, lag: u64) {
        self.set_labelled(
            signal::CONSUMER_LAG,
            vec![("consumer".to_string(), consumer.to_string())],
            lag as i64,
        );
    }

    /// Set the per-tenant vector-tombstone/compaction lag (§4.11 row 6; SRCH-P05 —
    /// `physical_len() - live_len()`). Returns to 0 after a compact (no orphan embedding survives).
    pub fn set_vector_compaction_lag(&mut self, tenant: &str, lag: u64) {
        self.set_labelled(
            signal::VECTOR_COMPACTION_LAG,
            vec![("tenant".to_string(), tenant.to_string())],
            lag as i64,
        );
    }

    /// Set the per-tenant in-flight query count (§4.11 row 8).
    pub fn set_in_flight(&mut self, tenant: &str, in_flight: u64) {
        self.set_labelled(
            signal::IN_FLIGHT,
            vec![("tenant".to_string(), tenant.to_string())],
            in_flight as i64,
        );
    }

    /// Set the per-tenant shed count (§4.11 row 8; the protected-human-lane shed order, fully driven
    /// at 30× in SRCH-P25). `0` until a surge sheds.
    pub fn set_shed_count(&mut self, tenant: &str, shed: u64) {
        self.set_labelled(
            signal::SHED_COUNT,
            vec![("tenant".to_string(), tenant.to_string())],
            shed as i64,
        );
    }

    /// Read a scalar signal by its frozen NAME (the harness reads each §4.11 scalar off here).
    /// `None` for a labelled signal name (read those via [`Self::labelled_value`]) or an unknown
    /// name. A scalar that exists always reads a value — never absent (the port emits the set from
    /// boot).
    pub fn scalar(&self, name: &str) -> Option<i64> {
        Some(match name {
            signal::INDEX_LAG => self.index_lag,
            signal::LIST_OBJECTS_RATE => self.list_objects_rate,
            signal::FILTER_MODE_IDS => self.filter_mode_ids,
            signal::FILTER_MODE_FILTER => self.filter_mode_filter,
            signal::CACHE_HIT_RATIO_PCT => self.cache_hit_ratio_pct,
            signal::ZERO_ESCAPE_ZOOKIE_BYPASS => self.zero_escape_zookie_bypass,
            signal::ZERO_ESCAPE_STALE_EXCLUDED => self.zero_escape_stale_excluded,
            signal::REINDEX_PARITY_HASH => self.reindex_parity_hash,
            signal::ERASE_RECEIPTS => self.erase_receipts,
            _ => return None,
        })
    }

    /// Read a labelled signal value by name + label set (order-independent), or `None` if absent.
    pub fn labelled_value(&self, name: &str, labels: &[(String, String)]) -> Option<i64> {
        let mut want = labels.to_vec();
        want.sort();
        self.labelled
            .iter()
            .find(|s| s.name == name && s.labels == want)
            .map(|s| s.value)
    }

    /// Set/overwrite a labelled signal value (the live emitter writes through here).
    fn set_labelled(&mut self, name: &'static str, labels: Vec<(String, String)>, value: i64) {
        let l = LabelledSignal::new(name, labels, value);
        if let Some(existing) = self
            .labelled
            .iter_mut()
            .find(|s| s.name == l.name && s.labels == l.labels)
        {
            existing.value = l.value;
        } else {
            self.labelled.push(l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §4.11 NAME set is exhaustive + has no duplicates (a doc/port that omits any of these
    /// fails X-1; observability is part of the pass). The 17 names map the §4.11 rows.
    #[test]
    fn the_signal_name_set_is_exhaustive_and_distinct() {
        let mut sorted = signal::ALL.to_vec();
        let n = sorted.len();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), n, "every §4.11 Search signal name is distinct");
        assert_eq!(n, 16, "the full §4.11 set is named exhaustively");
    }

    /// `from_stats` folds the per-slice counters into the scalar §4.11 signals (the aggregator
    /// reads the existing counters, never re-defines them — EI-01 §7).
    #[test]
    fn from_stats_folds_the_per_slice_counters() {
        let q = QueryStats::new();
        let c = ConsistencyStats::new();
        let cache = CacheStats::new();
        // a fresh fold: list_objects rate 0, both filter-mode legs 0, ratio absent.
        let t = SearchTelemetry::from_stats(0, &q, &c, &cache);
        assert_eq!(t.scalar(signal::LIST_OBJECTS_RATE), Some(0));
        assert_eq!(t.scalar(signal::FILTER_MODE_IDS), Some(0));
        assert_eq!(t.scalar(signal::FILTER_MODE_FILTER), Some(0));
        assert_eq!(
            t.scalar(signal::CACHE_HIT_RATIO_PCT),
            Some(CACHE_RATIO_ABSENT),
            "no cacheable read yet → the absent sentinel, never a fabricated 100"
        );
    }

    /// The empty snapshot still emits every scalar (a fresh service emits the set from boot).
    #[test]
    fn empty_snapshot_emits_every_scalar() {
        let t = SearchTelemetry::empty();
        for name in [
            signal::INDEX_LAG,
            signal::LIST_OBJECTS_RATE,
            signal::FILTER_MODE_IDS,
            signal::FILTER_MODE_FILTER,
            signal::CACHE_HIT_RATIO_PCT,
            signal::ZERO_ESCAPE_ZOOKIE_BYPASS,
            signal::ZERO_ESCAPE_STALE_EXCLUDED,
            signal::REINDEX_PARITY_HASH,
            signal::ERASE_RECEIPTS,
        ] {
            assert!(t.scalar(name).is_some(), "scalar `{name}` is emitted from boot");
        }
    }

    /// RED is read per `{kind, tenant, surface}` and the read is order-independent (a drill cannot
    /// miss a green by passing labels in a different order).
    #[test]
    fn red_is_labelled_per_kind_tenant_surface() {
        let mut t = SearchTelemetry::empty();
        t.record_red(
            RedLabels { kind: "human", tenant: "acme", surface: "ft" },
            120,
            0,
            7,
        );
        // human-lane errors == 0 (the lane holds).
        let labels = vec![
            ("surface".to_string(), "ft".to_string()),
            ("kind".to_string(), "human".to_string()),
            ("tenant".to_string(), "acme".to_string()),
        ];
        assert_eq!(
            t.labelled_value(signal::QUERY_ERRORS, &labels),
            Some(0),
            "RED errors are read per {{kind,tenant,surface}}, order-independent"
        );
        assert_eq!(t.labelled_value(signal::QUERY_RATE, &labels), Some(120));
        assert_eq!(t.labelled_value(signal::QUERY_DURATION_MS, &labels), Some(7));
    }

    /// A labelled set overwrites in place (a re-observation updates, never duplicates the key).
    #[test]
    fn labelled_overwrites_in_place() {
        let mut t = SearchTelemetry::empty();
        t.set_consumer_lag("search-indexer", 5);
        t.set_consumer_lag("search-indexer", 0);
        let lag = t.labelled_value(
            signal::CONSUMER_LAG,
            &[("consumer".to_string(), "search-indexer".to_string())],
        );
        assert_eq!(lag, Some(0), "the consumer lag drained to 0 (in-place update)");
        assert_eq!(
            t.labelled.iter().filter(|s| s.name == signal::CONSUMER_LAG).count(),
            1,
            "no duplicate key on re-observation"
        );
    }
}
