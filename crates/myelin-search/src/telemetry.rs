use crate::cache::CacheStats;
use crate::consistency::ConsistencyStats;
use crate::pipeline::QueryStats;

pub mod signal {
    pub const INDEX_LAG: &str = "search.index_lag";
    pub const QUERY_RATE: &str = "search.query_rate";
    pub const QUERY_ERRORS: &str = "search.query_errors";
    pub const QUERY_DURATION_MS: &str = "search.query_duration_ms";
    pub const LIST_OBJECTS_RATE: &str = "search.list_objects_rate";
    pub const FILTER_MODE_IDS: &str = "search.filter_mode.ids";
    pub const FILTER_MODE_FILTER: &str = "search.filter_mode.filter";
    pub const CACHE_HIT_RATIO_PCT: &str = "search.cache_hit_ratio_pct";
    pub const ZERO_ESCAPE_ZOOKIE_BYPASS: &str = "search.zero_escape.zookie_bypass";
    pub const ZERO_ESCAPE_STALE_EXCLUDED: &str = "search.zero_escape.stale_excluded";
    pub const REINDEX_PARITY_HASH: &str = "search.reindex.parity_hash";
    pub const ERASE_RECEIPTS: &str = "search.erase_receipts";
    pub const VECTOR_COMPACTION_LAG: &str = "search.vector_compaction_lag";
    pub const CONSUMER_LAG: &str = "search.consumer_lag";
    pub const IN_FLIGHT: &str = "search.in_flight";
    pub const SHED_COUNT: &str = "search.shed_count";

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

pub const CACHE_RATIO_ABSENT: i64 = -1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelledSignal {
    pub name: &'static str,
    pub labels: Vec<(String, String)>,
    pub value: i64,
}

impl LabelledSignal {
    fn new(name: &'static str, labels: Vec<(String, String)>, value: i64) -> LabelledSignal {
        let mut labels = labels;
        labels.sort();
        LabelledSignal {
            name,
            labels,
            value,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTelemetry {
    pub index_lag: i64,
    pub list_objects_rate: i64,
    pub filter_mode_ids: i64,
    pub filter_mode_filter: i64,
    pub cache_hit_ratio_pct: i64,
    pub zero_escape_zookie_bypass: i64,
    pub zero_escape_stale_excluded: i64,
    pub reindex_parity_hash: i64,
    pub erase_receipts: i64,
    pub labelled: Vec<LabelledSignal>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedLabels<'a> {
    pub kind: &'a str,
    pub tenant: &'a str,
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

    pub fn set_reindex_parity_hash(&mut self, hash: i64) {
        self.reindex_parity_hash = hash;
    }

    pub fn set_erase_receipts(&mut self, count: u64) {
        self.erase_receipts = count as i64;
    }

    pub fn record_red(&mut self, labels: RedLabels<'_>, rate: u64, errors: u64, duration_ms: u64) {
        let l = labels.to_labels();
        self.set_labelled(signal::QUERY_RATE, l.clone(), rate as i64);
        self.set_labelled(signal::QUERY_ERRORS, l.clone(), errors as i64);
        self.set_labelled(signal::QUERY_DURATION_MS, l, duration_ms as i64);
    }

    pub fn set_consumer_lag(&mut self, consumer: &str, lag: u64) {
        self.set_labelled(
            signal::CONSUMER_LAG,
            vec![("consumer".to_string(), consumer.to_string())],
            lag as i64,
        );
    }

    pub fn set_vector_compaction_lag(&mut self, tenant: &str, lag: u64) {
        self.set_labelled(
            signal::VECTOR_COMPACTION_LAG,
            vec![("tenant".to_string(), tenant.to_string())],
            lag as i64,
        );
    }

    pub fn set_in_flight(&mut self, tenant: &str, in_flight: u64) {
        self.set_labelled(
            signal::IN_FLIGHT,
            vec![("tenant".to_string(), tenant.to_string())],
            in_flight as i64,
        );
    }

    pub fn set_shed_count(&mut self, tenant: &str, shed: u64) {
        self.set_labelled(
            signal::SHED_COUNT,
            vec![("tenant".to_string(), tenant.to_string())],
            shed as i64,
        );
    }

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

    pub fn labelled_value(&self, name: &str, labels: &[(String, String)]) -> Option<i64> {
        let mut want = labels.to_vec();
        want.sort();
        self.labelled
            .iter()
            .find(|s| s.name == name && s.labels == want)
            .map(|s| s.value)
    }

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

    #[test]
    fn the_signal_name_set_is_exhaustive_and_distinct() {
        let mut sorted = signal::ALL.to_vec();
        let n = sorted.len();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            n,
            "every §4.11 Search signal name is distinct"
        );
        assert_eq!(n, 16, "the full §4.11 set is named exhaustively");
    }

    #[test]
    fn from_stats_folds_the_per_slice_counters() {
        let q = QueryStats::new();
        let c = ConsistencyStats::new();
        let cache = CacheStats::new();
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
            assert!(
                t.scalar(name).is_some(),
                "scalar `{name}` is emitted from boot"
            );
        }
    }

    #[test]
    fn red_is_labelled_per_kind_tenant_surface() {
        let mut t = SearchTelemetry::empty();
        t.record_red(
            RedLabels {
                kind: "human",
                tenant: "acme",
                surface: "ft",
            },
            120,
            0,
            7,
        );
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
        assert_eq!(
            t.labelled_value(signal::QUERY_DURATION_MS, &labels),
            Some(7)
        );
    }

    #[test]
    fn labelled_overwrites_in_place() {
        let mut t = SearchTelemetry::empty();
        t.set_consumer_lag("search-indexer", 5);
        t.set_consumer_lag("search-indexer", 0);
        let lag = t.labelled_value(
            signal::CONSUMER_LAG,
            &[("consumer".to_string(), "search-indexer".to_string())],
        );
        assert_eq!(
            lag,
            Some(0),
            "the consumer lag drained to 0 (in-place update)"
        );
        assert_eq!(
            t.labelled
                .iter()
                .filter(|s| s.name == signal::CONSUMER_LAG)
                .count(),
            1,
            "no duplicate key on re-observation"
        );
    }
}
