use myelin_substrate::thresholds::FilteredAnn;
use myelin_tenancy::{Region, TenantId};

use crate::vector::{Embedding, HnswVectorIndex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilteredAnnStrategy {
    pub recall_at_k_bps: u32,
    pub brute_force_fallback_visible_bps: u32,
    pub ivf_pq_promotion_live_vectors: u64,
}

impl FilteredAnnStrategy {
    pub const RECALL_AT_K_BPS_SEED: u32 = FilteredAnn::RECALL_AT_K_BPS_SEED;
    pub const BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED: u32 =
        FilteredAnn::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED;
    pub const IVF_PQ_PROMOTION_LIVE_VECTORS_SEED: u64 =
        FilteredAnn::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED;

    pub fn from_thresholds(f: &FilteredAnn) -> FilteredAnnStrategy {
        FilteredAnnStrategy {
            recall_at_k_bps: f.recall_at_k_bps,
            brute_force_fallback_visible_bps: f.brute_force_fallback_visible_bps,
            ivf_pq_promotion_live_vectors: f.ivf_pq_promotion_live_vectors,
        }
    }

    pub fn recall_floor_fraction(&self) -> f64 {
        self.recall_at_k_bps as f64 / 10_000.0
    }

    pub fn is_very_selective(&self, visible: u64, total: u64) -> bool {
        if total == 0 {
            return false;
        }
        (visible as u128) * 10_000
            <= (self.brute_force_fallback_visible_bps as u128) * (total as u128)
    }

    pub fn should_promote_to_ivf_pq(&self, live_vectors: u64) -> bool {
        live_vectors >= self.ivf_pq_promotion_live_vectors
    }
}

impl Default for FilteredAnnStrategy {
    fn default() -> Self {
        FilteredAnnStrategy {
            recall_at_k_bps: Self::RECALL_AT_K_BPS_SEED,
            brute_force_fallback_visible_bps: Self::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED,
            ivf_pq_promotion_live_vectors: Self::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecallMeasurement {
    pub expected_hits: u64,
    pub recovered_hits: u64,
    pub escapes: u64,
    pub queries: u64,
}

impl RecallMeasurement {
    pub fn recall(&self) -> f64 {
        if self.expected_hits == 0 {
            return 1.0;
        }
        self.recovered_hits as f64 / self.expected_hits as f64
    }

    pub fn recall_bps(&self) -> u32 {
        if self.expected_hits == 0 {
            return 10_000;
        }
        (((self.recovered_hits as u128) * 10_000) / (self.expected_hits as u128)) as u32
    }
}

pub fn measure_recall_at_k(
    index: &HnswVectorIndex,
    corpus: &[(String, Embedding)],
    queries: &[Embedding],
    visible: impl Fn(&str) -> bool,
    k: usize,
) -> RecallMeasurement {
    let mut expected_hits = 0u64;
    let mut recovered_hits = 0u64;
    let mut escapes = 0u64;

    for q in queries {
        let mut scored: Vec<(f32, &str)> = corpus
            .iter()
            .filter(|(id, _)| visible(id))
            .map(|(id, e)| (cosine_distance(e, q), id.as_str()))
            .collect();
        scored.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(b.1))
        });
        let truth: Vec<&str> = scored.iter().take(k).map(|(_, id)| *id).collect();
        expected_hits += truth.len() as u64;

        let hits = index.knn_filtered(q, k, |doc_id, _acl_object| visible(doc_id));
        for h in &hits {
            if !visible(&h.doc_id) {
                escapes += 1;
            } else if truth.contains(&h.doc_id.as_str()) {
                recovered_hits += 1;
            }
        }
    }

    RecallMeasurement {
        expected_hits,
        recovered_hits,
        escapes,
        queries: queries.len() as u64,
    }
}

fn cosine_distance(a: &Embedding, b: &Embedding) -> f32 {
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.0.iter().zip(b.0.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 1.0;
    }
    1.0 - (dot / (na.sqrt() * nb.sqrt())).clamp(-1.0, 1.0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilteredAnnArtifact {
    pub tenant: TenantId,
    pub region: Region,
    pub k: usize,
    pub queries: u64,
    pub visible_fraction_bps: u32,
    pub measured_recall_bps: u32,
    pub recall_floor_bps: u32,
    pub escapes: u64,
    pub ivf_pq_promotion_live_vectors: u64,
    pub measured: bool,
    pub ran_at: String,
}

impl FilteredAnnArtifact {
    pub fn is_green(&self) -> bool {
        self.measured_recall_bps >= self.recall_floor_bps && self.escapes == 0 && self.measured
    }

    pub fn summary(&self) -> String {
        format!(
            "search filtered-ANN recall PASS (SRCH-D8): k={}, {} queries under a very selective filter \
             ({}bps = {:.2}% visible) - recall@k={}bps ({:.2}%, MEASURED vs brute-force ground truth, \
             >= the {}bps floor); zero-escape counter={} (no hidden doc surfaced, contract 1.8). The \
             HNSW↔IVF-PQ promotion point is {} live vectors/cell (§3.3). Numbers written to the \
             thresholds file ([filtered_ann]).",
            self.k,
            self.queries,
            self.visible_fraction_bps,
            self.visible_fraction_bps as f64 / 100.0,
            self.measured_recall_bps,
            self.measured_recall_bps as f64 / 100.0,
            self.recall_floor_bps,
            self.escapes,
            self.ivf_pq_promotion_live_vectors,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilteredAnnFailure {
    Leak { escapes: u64 },
    RecallUnderFloor { measured_bps: u32, floor_bps: u32 },
    NoQueries,
    MisspecifiedStrategy,
    FilterNotSelective { visible_bps: u32, trigger_bps: u32 },
}

impl core::fmt::Display for FilteredAnnFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FilteredAnnFailure::Leak { escapes } => write!(
                f,
                "FILTERED-ANN FAIL - {escapes} ESCAPE(s): a hidden (non-visible) doc surfaced in a \
                 filtered-ANN result (a leak, contract 1.8 zero-escape). This is the gravest failure, \
                 independent of recall - never tolerated"
            ),
            FilteredAnnFailure::RecallUnderFloor {
                measured_bps,
                floor_bps,
            } => write!(
                f,
                "FILTERED-ANN FAIL - the measured recall@k {measured_bps}bps fell BELOW the \
                 {floor_bps}bps floor: a visible nearest neighbour was DROPPED under the selective \
                 filter (§4.2.2 recall-correctness). The floor is NOT softened to pass - this is a \
                 dated [[claimed_not_proven]] row (EI-01 §3)"
            ),
            FilteredAnnFailure::NoQueries => write!(
                f,
                "FILTERED-ANN FAIL - 0 query points: the run measured nothing (a mis-specified drill). \
                 A recall over zero queries is undefined, never a fabricated 100 %"
            ),
            FilteredAnnFailure::MisspecifiedStrategy => write!(
                f,
                "FILTERED-ANN FAIL - the strategy numbers are mis-specified (a 0 recall floor / 0 \
                 promotion point / a fallback trigger outside (0, 100 %]). A green cannot be \
                 manufactured by a vacuous bar"
            ),
            FilteredAnnFailure::FilterNotSelective {
                visible_bps,
                trigger_bps,
            } => write!(
                f,
                "FILTERED-ANN FAIL - the filter is not VERY SELECTIVE ({visible_bps}bps visible > the \
                 {trigger_bps}bps fallback trigger): SRCH-D8 proves recall in the very-selective regime \
                 the brute-force fallback is tuned for; a non-selective filter proves a different thing"
            ),
        }
    }
}

impl std::error::Error for FilteredAnnFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a filtered-ANN verdict must be checked - a dropped RED is a SWALLOWED recall/leak \
              failure (the SRCH-D8 gate, EI-01 §5: loud-never-swallowed)"]
pub enum FilteredAnnVerdict {
    Green(FilteredAnnArtifact),
    Red(FilteredAnnFailure),
}

impl FilteredAnnVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, FilteredAnnVerdict::Green(_))
    }
    pub fn artifact(&self) -> Option<&FilteredAnnArtifact> {
        match self {
            FilteredAnnVerdict::Green(a) => Some(a),
            FilteredAnnVerdict::Red(_) => None,
        }
    }
    pub fn failure(&self) -> Option<&FilteredAnnFailure> {
        match self {
            FilteredAnnVerdict::Green(_) => None,
            FilteredAnnVerdict::Red(f) => Some(f),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FilteredAnnGate;

impl FilteredAnnGate {
    pub fn new() -> FilteredAnnGate {
        FilteredAnnGate
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        tenant: &TenantId,
        region: &Region,
        k: usize,
        visible_count: u64,
        total_count: u64,
        m: &RecallMeasurement,
        s: &FilteredAnnStrategy,
        now: &str,
    ) -> FilteredAnnVerdict {
        if s.recall_at_k_bps == 0
            || s.brute_force_fallback_visible_bps == 0
            || s.brute_force_fallback_visible_bps > 10_000
            || s.ivf_pq_promotion_live_vectors == 0
        {
            return FilteredAnnVerdict::Red(FilteredAnnFailure::MisspecifiedStrategy);
        }
        if m.queries == 0 {
            return FilteredAnnVerdict::Red(FilteredAnnFailure::NoQueries);
        }
        let visible_bps = if total_count == 0 {
            0
        } else {
            (((visible_count as u128) * 10_000) / (total_count as u128)) as u32
        };
        if !s.is_very_selective(visible_count, total_count) {
            return FilteredAnnVerdict::Red(FilteredAnnFailure::FilterNotSelective {
                visible_bps,
                trigger_bps: s.brute_force_fallback_visible_bps,
            });
        }
        if m.escapes > 0 {
            return FilteredAnnVerdict::Red(FilteredAnnFailure::Leak { escapes: m.escapes });
        }
        let measured_bps = m.recall_bps();
        if measured_bps < s.recall_at_k_bps {
            return FilteredAnnVerdict::Red(FilteredAnnFailure::RecallUnderFloor {
                measured_bps,
                floor_bps: s.recall_at_k_bps,
            });
        }

        FilteredAnnVerdict::Green(FilteredAnnArtifact {
            tenant: tenant.clone(),
            region: region.clone(),
            k,
            queries: m.queries,
            visible_fraction_bps: visible_bps,
            measured_recall_bps: measured_bps,
            recall_floor_bps: s.recall_at_k_bps,
            escapes: m.escapes,
            ivf_pq_promotion_live_vectors: s.ivf_pq_promotion_live_vectors,
            measured: true,
            ran_at: now.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::{ModelRef, VectorRecord};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn corpus_and_index(
        n: usize,
        dim: usize,
        seed: u64,
    ) -> (Vec<(String, Embedding)>, HnswVectorIndex) {
        let mut s = seed;
        let mut gen = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f32 / (1u64 << 31) as f32) - 1.0
        };
        let mut corpus = Vec::with_capacity(n);
        let mut idx = HnswVectorIndex::open();
        for i in 0..n {
            let v: Vec<f32> = (0..dim).map(|_| gen()).collect();
            let e = Embedding(v.clone());
            corpus.push((format!("d{i}"), e.clone()));
            idx.upsert(VectorRecord {
                doc_id: format!("d{i}"),
                acl_object: format!("d{i}"),
                embedding: e,
                model_ref: ModelRef("m@1".into()),
            })
            .unwrap();
        }
        (corpus, idx)
    }

    #[test]
    fn seed_constants_mirror_thresholds() {
        assert_eq!(
            FilteredAnnStrategy::RECALL_AT_K_BPS_SEED,
            FilteredAnn::RECALL_AT_K_BPS_SEED
        );
        assert_eq!(
            FilteredAnnStrategy::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED,
            FilteredAnn::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED
        );
        assert_eq!(
            FilteredAnnStrategy::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED,
            FilteredAnn::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED
        );
        let from_thresh = FilteredAnnStrategy::from_thresholds(&FilteredAnn::default());
        assert_eq!(from_thresh, FilteredAnnStrategy::default());
    }

    #[test]
    fn exact_recall_under_selective_filter_zero_escape() {
        let (corpus, idx) = corpus_and_index(500, 6, 0xD8_C0FFEE);
        let visible_ids: Vec<String> =
            ["d3", "d61", "d130", "d199", "d255", "d310", "d404", "d489"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let visible = |doc: &str| visible_ids.iter().any(|v| v == doc);
        let queries: Vec<Embedding> = [255, 10, 404, 250, 489, 7]
            .iter()
            .map(|&i| corpus[i].1.clone())
            .collect();

        let m = measure_recall_at_k(&idx, &corpus, &queries, visible, 3);
        assert_eq!(m.escapes, 0, "no hidden doc surfaced");
        assert_eq!(
            m.recall_bps(),
            10_000,
            "exact recall under the selective filter (brute-force fallback recovers the visible NN)"
        );

        let s = FilteredAnnStrategy::default();
        let v = FilteredAnnGate::new().run(
            &tenant(),
            &region(),
            3,
            visible_ids.len() as u64,
            corpus.len() as u64,
            &m,
            &s,
            "2026-06-25",
        );
        let a = v.artifact().expect("SRCH-D8 green");
        assert!(a.is_green());
        assert_eq!(a.measured_recall_bps, 10_000);
        assert_eq!(a.escapes, 0);
        assert!(a.measured, "the recall was MEASURED, not a default-to-beat");
        assert!(a.visible_fraction_bps <= s.brute_force_fallback_visible_bps);
        println!("[P-461 GATE GREEN 2026-06-25] {}", a.summary());
    }

    #[test]
    fn an_escape_fails_loud_as_a_leak() {
        let m = RecallMeasurement {
            expected_hits: 10,
            recovered_hits: 10,
            escapes: 1,
            queries: 5,
        };
        let v = FilteredAnnGate::new().run(
            &tenant(),
            &region(),
            3,
            5,
            500,
            &m,
            &FilteredAnnStrategy::default(),
            "2026-06-25",
        );
        assert_eq!(
            v.failure(),
            Some(&FilteredAnnFailure::Leak { escapes: 1 }),
            "an escape is a leak, checked before recall - RED even at perfect recall"
        );
        assert!(!v.is_green());
    }

    #[test]
    fn recall_under_floor_fails_loud() {
        let m = RecallMeasurement {
            expected_hits: 10,
            recovered_hits: 9,
            escapes: 0,
            queries: 5,
        };
        let v = FilteredAnnGate::new().run(
            &tenant(),
            &region(),
            3,
            5,
            500,
            &m,
            &FilteredAnnStrategy::default(),
            "2026-06-25",
        );
        assert_eq!(
            v.failure(),
            Some(&FilteredAnnFailure::RecallUnderFloor {
                measured_bps: 9_000,
                floor_bps: 10_000,
            }),
            "recall under the floor is RED - never softened to pass"
        );
    }

    #[test]
    fn measured_recall_is_floored_never_rounded_up() {
        let m = RecallMeasurement {
            expected_hits: 10_000,
            recovered_hits: 9_999,
            escapes: 0,
            queries: 100,
        };
        assert_eq!(m.recall_bps(), 9_999, "floored, not rounded up to 10000");
        let v = FilteredAnnGate::new().run(
            &tenant(),
            &region(),
            3,
            5,
            500,
            &m,
            &FilteredAnnStrategy::default(),
            "2026-06-25",
        );
        assert!(matches!(
            v.failure(),
            Some(FilteredAnnFailure::RecallUnderFloor { .. })
        ));
    }

    #[test]
    fn zero_queries_is_red() {
        let m = RecallMeasurement {
            expected_hits: 0,
            recovered_hits: 0,
            escapes: 0,
            queries: 0,
        };
        let v = FilteredAnnGate::new().run(
            &tenant(),
            &region(),
            3,
            5,
            500,
            &m,
            &FilteredAnnStrategy::default(),
            "2026-06-25",
        );
        assert_eq!(v.failure(), Some(&FilteredAnnFailure::NoQueries));
    }

    #[test]
    fn misspecified_strategy_is_red() {
        let m = RecallMeasurement {
            expected_hits: 10,
            recovered_hits: 10,
            escapes: 0,
            queries: 5,
        };
        let bad = FilteredAnnStrategy {
            recall_at_k_bps: 0,
            ..FilteredAnnStrategy::default()
        };
        let v = FilteredAnnGate::new().run(&tenant(), &region(), 3, 5, 500, &m, &bad, "2026-06-25");
        assert_eq!(v.failure(), Some(&FilteredAnnFailure::MisspecifiedStrategy));
    }

    #[test]
    fn non_selective_filter_is_red() {
        let m = RecallMeasurement {
            expected_hits: 10,
            recovered_hits: 10,
            escapes: 0,
            queries: 5,
        };
        let v = FilteredAnnGate::new().run(
            &tenant(),
            &region(),
            3,
            300,
            500,
            &m,
            &FilteredAnnStrategy::default(),
            "2026-06-25",
        );
        assert_eq!(
            v.failure(),
            Some(&FilteredAnnFailure::FilterNotSelective {
                visible_bps: 6_000,
                trigger_bps: 2_000,
            })
        );
    }

    #[test]
    fn very_selective_boundary_is_inclusive() {
        let s = FilteredAnnStrategy::default();
        assert!(
            s.is_very_selective(2_000, 10_000),
            "exactly 20 % is selective (≤)"
        );
        assert!(
            s.is_very_selective(1_999, 10_000),
            "just under 20 % is selective"
        );
        assert!(!s.is_very_selective(2_001, 10_000), "just over 20 % is not");
        assert!(!s.is_very_selective(5, 0), "total 0 is never selective");
    }

    #[test]
    fn ivf_pq_promotion_point_binds_at_threshold() {
        let s = FilteredAnnStrategy::default();
        assert!(
            s.should_promote_to_ivf_pq(1_000_000),
            "promotes at the threshold"
        );
        assert!(s.should_promote_to_ivf_pq(2_500_000), "promotes above it");
        assert!(
            !s.should_promote_to_ivf_pq(999_999),
            "does NOT promote below it"
        );
        assert_eq!(s.recall_floor_fraction(), 1.0);
    }
}
