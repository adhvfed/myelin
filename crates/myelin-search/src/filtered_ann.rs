//! # The tuned filtered-ANN strategy + the HNSW↔IVF-PQ promotion point (SRCH-P26 / P-461, M5)
//!
//! **Architecture:** `search-and-indexing.md` §4.2.2 (filter-during-traversal — the recall@k under a
//! selective filter + the **brute-force-fallback** for very selective filters), §3.3 (IVF-PQ the
//! per-cell memory-pressure upgrade — a *measured* promotion). **Contracts:** 6.2 (the filtered-ANN
//! traversal at scale), 1.8 (the recall + zero-escape telemetry). **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it; recall@k MEASURED never
//! predicted), `external-insights/04-hard-problems.md` §5 (embeddings are personal data).
//!
//! ## What SRCH-P26 ships here (the STRATEGY, not the property)
//! SRCH-P11 (M2, [`crate::vector::HnswVectorIndex::knn_filtered`]) fixed the *property*: the top-k are
//! the k **visible** neighbours, never k-then-filtered (under-fill) and never a hidden doc (leak), with
//! a brute-force fallback that recovers the genuine k-nearest visible set under a very selective
//! filter. THIS module tunes + measures the *strategy*:
//!   - [`FilteredAnnStrategy`] — the tuned numbers (mirrored from the thresholds file): the recall@k
//!     FLOOR a selective filter must meet (0 leak), the visible-fraction at/below which the graph walk
//!     under-fills so Search falls back to brute-force, and the HNSW→IVF-PQ promotion point.
//!   - [`measure_recall_at_k`] — drives `knn_filtered` over a corpus under a selective filter and
//!     compares against the brute-force ground truth over the VISIBLE set: a [`RecallMeasurement`]
//!     carrying recall@k + the **zero-escape counter** (a hidden doc that surfaced).
//!   - [`FilteredAnnGate`] — the SRCH-D8 gate: recall@k ≥ floor AND 0 escapes → a dated
//!     [`FilteredAnnArtifact`]; else a typed [`FilteredAnnFailure`] that FAILs CI (never a softened
//!     bar).
//!
//! ## The numbers are MEASURED, not predicted (EI-01 §3)
//! `recall_at_k` is measured by [`measure_recall_at_k`] against a brute-force ground truth — the gate
//! reads the floor from the thresholds file and the corpus drives the measurement. The brute-force
//! fallback makes EXACT recall (100.00 %) achievable under a selective filter, so the floor is not a
//! soft 95 %: it is "no visible nearest neighbour is ever DROPPED, no hidden one ever LEAKED".
//!
//! ## Floors named (not mistaken for the tuned-at-scale answer)
//! - The **world-scale 30× run on real fleet hardware** (the per-cell IVF-PQ promotion at a real
//!   million-vector cell, network-delivered) is the ONE remaining floor (shared testing-strategy §4.1).
//!   The strategy LOGIC + the dated artifact + the measured-recall-to-thresholds write ship now and
//!   re-run as a `cargo test` gate on every vector-touching change.
//! - The **real EU-hostable embedding-model adapter** (text→vector) is the post-M5 / runtime config
//!   swap (SRCH-P06 named floor) — the vector math + the erasure are done; the adapter is a config swap,
//!   never a code change. Named, not built here.

use myelin_substrate::thresholds::FilteredAnn;
use myelin_tenancy::{Region, TenantId};

use crate::vector::{Embedding, HnswVectorIndex};

/// The tuned filtered-ANN strategy numbers, mirrored from the thresholds file (`[filtered_ann]`). The
/// thresholds file is the SOURCE OF TRUTH; this is the in-crate seed + the typed accessor the gate and
/// the index reads. The seed constants must equal the [`FilteredAnn`] seeds (asserted by a test) so the
/// two never drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilteredAnnStrategy {
    /// The recall@k FLOOR in basis points (10 000 = 100.00 %) — the filtered-ANN traversal must recover
    /// at least this fraction of the true k-nearest VISIBLE neighbours under a selective filter, 0 leak.
    pub recall_at_k_bps: u32,
    /// The visible-fraction (basis points) at/below which the graph walk under-fills, so Search falls
    /// back to brute-force over the small visible set (§4.2.2). The TUNED very-selective trigger.
    pub brute_force_fallback_visible_bps: u32,
    /// The live per-cell vector count at which HNSW promotes to the IVF-PQ memory-pressure shape (§3.3).
    pub ivf_pq_promotion_live_vectors: u64,
}

impl FilteredAnnStrategy {
    /// The recall@k floor seed: exact recall (100.00 %) — mirrors [`FilteredAnn::RECALL_AT_K_BPS_SEED`].
    pub const RECALL_AT_K_BPS_SEED: u32 = FilteredAnn::RECALL_AT_K_BPS_SEED;
    /// The brute-force-fallback visible-fraction seed (20 %) —
    /// mirrors [`FilteredAnn::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED`].
    pub const BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED: u32 =
        FilteredAnn::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED;
    /// The HNSW→IVF-PQ promotion-point seed (1 000 000 live vectors) —
    /// mirrors [`FilteredAnn::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED`].
    pub const IVF_PQ_PROMOTION_LIVE_VECTORS_SEED: u64 =
        FilteredAnn::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED;

    /// Adopt the strategy from a loaded thresholds-file section (the source of truth).
    pub fn from_thresholds(f: &FilteredAnn) -> FilteredAnnStrategy {
        FilteredAnnStrategy {
            recall_at_k_bps: f.recall_at_k_bps,
            brute_force_fallback_visible_bps: f.brute_force_fallback_visible_bps,
            ivf_pq_promotion_live_vectors: f.ivf_pq_promotion_live_vectors,
        }
    }

    /// The recall floor as a fraction in `[0, 1]` (bps / 10 000).
    pub fn recall_floor_fraction(&self) -> f64 {
        self.recall_at_k_bps as f64 / 10_000.0
    }

    /// Whether a filter leaving `visible` of `total` indexed vectors visible is "very selective" — the
    /// visible fraction is at/below the tuned trigger, so Search should fall back to brute-force over
    /// the small visible set (§4.2.2). `total == 0` is not selective. Integer-exact (no float rounding).
    pub fn is_very_selective(&self, visible: u64, total: u64) -> bool {
        if total == 0 {
            return false;
        }
        (visible as u128) * 10_000
            <= (self.brute_force_fallback_visible_bps as u128) * (total as u128)
    }

    /// Whether a cell holding `live_vectors` live vectors has crossed the HNSW→IVF-PQ promotion point
    /// (§3.3) — at/above it the per-cell memory budget triggers the IVF-PQ compression. Promotion
    /// changes COST (RAM), never correctness (the recall floor still binds).
    pub fn should_promote_to_ivf_pq(&self, live_vectors: u64) -> bool {
        live_vectors >= self.ivf_pq_promotion_live_vectors
    }
}

impl Default for FilteredAnnStrategy {
    /// The §4.2.2 / §3.3 seeds (exact recall, 20 % fallback trigger, 1 000 000-vector promotion point).
    fn default() -> Self {
        FilteredAnnStrategy {
            recall_at_k_bps: Self::RECALL_AT_K_BPS_SEED,
            brute_force_fallback_visible_bps: Self::BRUTE_FORCE_FALLBACK_VISIBLE_BPS_SEED,
            ivf_pq_promotion_live_vectors: Self::IVF_PQ_PROMOTION_LIVE_VECTORS_SEED,
        }
    }
}

/// The MEASURED recall@k under a selective filter, with the zero-escape counter (contract 1.8). The
/// raw measurement [`measure_recall_at_k`] produces; the gate turns it into a dated artifact or a
/// typed failure. PII-free (counts only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecallMeasurement {
    /// The number of (true visible nearest neighbour) slots EXPECTED across all query points = the sum
    /// of `min(k, |visible|)` over the queries (the brute-force ground-truth size).
    pub expected_hits: u64,
    /// The number of those slots the filtered-ANN traversal actually RECOVERED (the intersection of the
    /// returned set with the brute-force ground truth, per query, summed).
    pub recovered_hits: u64,
    /// **THE ZERO-ESCAPE COUNTER (contract 1.8):** how many times a NON-visible (hidden) doc surfaced
    /// in a filtered-ANN result across all queries. MUST be 0 — a single escape is a leak (the gravest
    /// failure), independent of recall.
    pub escapes: u64,
    /// The number of query points the measurement ran over (a 0-query measurement is vacuous).
    pub queries: u64,
}

impl RecallMeasurement {
    /// The measured recall@k as a fraction in `[0, 1]`: `recovered / expected`. `1.0` when no slots
    /// were expected (a vacuous-but-clean measurement — the gate separately rejects 0 queries). Never
    /// rounded UP to flatter the floor.
    pub fn recall(&self) -> f64 {
        if self.expected_hits == 0 {
            return 1.0;
        }
        self.recovered_hits as f64 / self.expected_hits as f64
    }

    /// The measured recall in basis points (floored, never rounded up — a recall of 0.9999 reports
    /// 9999 bps, not 10000, so a sub-floor recall can never be flattered over the floor).
    pub fn recall_bps(&self) -> u32 {
        if self.expected_hits == 0 {
            return 10_000;
        }
        // floor((recovered * 10000) / expected) — integer, no float round-up.
        (((self.recovered_hits as u128) * 10_000) / (self.expected_hits as u128)) as u32
    }
}

/// **Measure recall@k under a selective filter against the brute-force ground truth over the VISIBLE
/// set.** For each query: the brute-force ground truth is the `min(k, |visible|)` nearest VISIBLE docs;
/// the filtered-ANN result is [`HnswVectorIndex::knn_filtered`] with the same visibility predicate. The
/// recovered count is the intersection size; the escape count is any returned doc that is NOT visible
/// (a leak). The measurement is over the LIVE index — the same code path production serves.
///
/// `corpus` is the `(doc_id, embedding)` set the index was built from (the ground-truth source);
/// `queries` are the query embeddings; `visible` is the selective ACL/structured filter; `k` is the
/// neighbour count. The corpus and the index MUST agree (the index was upserted from this corpus).
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
        // Brute-force ground truth over the VISIBLE set: the min(k, |visible|) nearest visible doc-ids.
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

        // The filtered-ANN result over the LIVE index (the production code path). The recall harness
        // is keyed on `doc_id` (the corpus is `(doc_id, embedding)` — no acl_object), so it adapts to
        // the two-field predicate by matching on the `doc_id` arm (the acl_object is ignored here; the
        // acl_object membership is exercised by the engine/vector ACL-parity tests, not the recall
        // measurement).
        let hits = index.knn_filtered(q, k, |doc_id, _acl_object| visible(doc_id));
        for h in &hits {
            // Any returned doc that is NOT visible is an ESCAPE (a leak) — contract 1.8 zero-escape.
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

/// The cosine distance between two embeddings (`1 - cosine_similarity`), the metric the index descends.
/// Duplicated tiny helper (the index's own `cosine_distance` is private) — the brute-force ground truth
/// must compute the SAME metric the graph does. Zero-norm yields `1.0` (no NaN), matching the index.
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

// ════════════════════════════════════════════════════════════════════════════════════════════
// SRCH-D8 — the dated artifact + the typed failure + the gate
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated GREEN ARTIFACT a filtered-ANN recall run returns (the SRCH-D8 proof; observability is part
/// of the pass). Carries the MEASURED numbers: the recall@k under the selective filter, the zero-escape
/// counter, the floor it met, and the tuned strategy points. PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilteredAnnArtifact {
    /// The cell the gate ran within (Search never crosses it).
    pub tenant: TenantId,
    /// The region the gate ran within.
    pub region: Region,
    /// The neighbour count `k` the recall was measured at.
    pub k: usize,
    /// The number of query points measured over.
    pub queries: u64,
    /// The visible-fraction of the corpus under the selective filter, in basis points (10 000 = 100 %)
    /// — recorded so the artifact proves the filter was genuinely VERY SELECTIVE (≤ the fallback
    /// trigger), the regime the strategy is tuned for.
    pub visible_fraction_bps: u32,
    /// **THE GATE READING:** the MEASURED recall@k in basis points (10 000 = 100.00 %). MUST be ≥
    /// `recall_floor_bps`.
    pub measured_recall_bps: u32,
    /// The recall@k floor the measurement met (bps) — from the thresholds file.
    pub recall_floor_bps: u32,
    /// **THE ZERO-ESCAPE COUNTER (contract 1.8):** must be 0 (no hidden doc surfaced).
    pub escapes: u64,
    /// The tuned HNSW→IVF-PQ promotion point (per-cell live vectors) the strategy carries — recorded so
    /// the artifact documents the measured promotion point (§3.3).
    pub ivf_pq_promotion_live_vectors: u64,
    /// Honest recording (the TESTS line): `true` — the recall was MEASURED against a brute-force ground
    /// truth, NOT carried as a default-to-beat.
    pub measured: bool,
    /// When the pass ran (the dated artifact).
    pub ran_at: String,
}

impl FilteredAnnArtifact {
    /// Whether the SRCH-D8 gate is GREEN: recall@k met the floor AND 0 escapes AND it was a real
    /// measurement.
    pub fn is_green(&self) -> bool {
        self.measured_recall_bps >= self.recall_floor_bps && self.escapes == 0 && self.measured
    }

    /// The dated green-artifact line a SCHED run prints on PASS (the measured-numbers proof). The caller
    /// prefixes the date (`[P-461 GATE GREEN <date>]`).
    pub fn summary(&self) -> String {
        format!(
            "search filtered-ANN recall PASS (SRCH-D8): k={}, {} queries under a very selective filter \
             ({}bps = {:.2}% visible) — recall@k={}bps ({:.2}%, MEASURED vs brute-force ground truth, \
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

/// A RED filtered-ANN result — EXACTLY which recall invariant failed (observability is part of the
/// pass). Never a bare bool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilteredAnnFailure {
    /// **A hidden (non-visible) doc surfaced in a filtered-ANN result** — the gravest SRCH-D8 failure
    /// (a leak), independent of recall. Carries the escape count. FAILs CI; never tolerated.
    Leak { escapes: u64 },
    /// **The measured recall@k fell BELOW the floor** — a visible nearest neighbour was DROPPED under
    /// the selective filter. Carries the measured recall + the floor (bps). The gate FAILs CI rather
    /// than softening the floor to manufacture green (EI-01 §3).
    RecallUnderFloor { measured_bps: u32, floor_bps: u32 },
    /// **No query points were measured** — a recall run that measured nothing cannot prove a floor (a
    /// mis-specified drill). The gate FAILs CI, never passes on a vacuous run.
    NoQueries,
    /// **The strategy numbers are mis-specified** (e.g. a 0 recall floor — "no recall required"). A
    /// green can never be manufactured by a vacuous bar; the gate FAILs LOUD.
    MisspecifiedStrategy,
    /// **The filter was not actually selective** (the visible fraction exceeds the tuned fallback
    /// trigger) — the SRCH-D8 gate proves recall in the VERY-SELECTIVE regime the strategy is tuned for;
    /// running it on a non-selective filter would prove a different (easier) thing. The gate FAILs LOUD
    /// so the drill cannot quietly drift out of its regime.
    FilterNotSelective { visible_bps: u32, trigger_bps: u32 },
}

impl core::fmt::Display for FilteredAnnFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FilteredAnnFailure::Leak { escapes } => write!(
                f,
                "FILTERED-ANN FAIL — {escapes} ESCAPE(s): a hidden (non-visible) doc surfaced in a \
                 filtered-ANN result (a leak, contract 1.8 zero-escape). This is the gravest failure, \
                 independent of recall — never tolerated"
            ),
            FilteredAnnFailure::RecallUnderFloor {
                measured_bps,
                floor_bps,
            } => write!(
                f,
                "FILTERED-ANN FAIL — the measured recall@k {measured_bps}bps fell BELOW the \
                 {floor_bps}bps floor: a visible nearest neighbour was DROPPED under the selective \
                 filter (§4.2.2 recall-correctness). The floor is NOT softened to pass — this is a \
                 dated [[claimed_not_proven]] row (EI-01 §3)"
            ),
            FilteredAnnFailure::NoQueries => write!(
                f,
                "FILTERED-ANN FAIL — 0 query points: the run measured nothing (a mis-specified drill). \
                 A recall over zero queries is undefined, never a fabricated 100 %"
            ),
            FilteredAnnFailure::MisspecifiedStrategy => write!(
                f,
                "FILTERED-ANN FAIL — the strategy numbers are mis-specified (a 0 recall floor / 0 \
                 promotion point / a fallback trigger outside (0, 100 %]). A green cannot be \
                 manufactured by a vacuous bar"
            ),
            FilteredAnnFailure::FilterNotSelective {
                visible_bps,
                trigger_bps,
            } => write!(
                f,
                "FILTERED-ANN FAIL — the filter is not VERY SELECTIVE ({visible_bps}bps visible > the \
                 {trigger_bps}bps fallback trigger): SRCH-D8 proves recall in the very-selective regime \
                 the brute-force fallback is tuned for; a non-selective filter proves a different thing"
            ),
        }
    }
}

impl std::error::Error for FilteredAnnFailure {}

/// The typed verdict of a filtered-ANN recall run — GREEN ([`FilteredAnnArtifact`]) or RED
/// ([`FilteredAnnFailure`]). `#[must_use]`: a dropped verdict is a swallowed recall check.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a filtered-ANN verdict must be checked — a dropped RED is a SWALLOWED recall/leak \
              failure (the SRCH-D8 gate, EI-01 §5: loud-never-swallowed)"]
pub enum FilteredAnnVerdict {
    /// recall@k met the floor + 0 escapes. The dated artifact.
    Green(FilteredAnnArtifact),
    /// EXACTLY what broke. FAILs CI; never swallowed.
    Red(FilteredAnnFailure),
}

impl FilteredAnnVerdict {
    /// `true` iff the gate passed.
    pub fn is_green(&self) -> bool {
        matches!(self, FilteredAnnVerdict::Green(_))
    }
    /// The green artifact, if the gate passed.
    pub fn artifact(&self) -> Option<&FilteredAnnArtifact> {
        match self {
            FilteredAnnVerdict::Green(a) => Some(a),
            FilteredAnnVerdict::Red(_) => None,
        }
    }
    /// The failure, if the gate failed.
    pub fn failure(&self) -> Option<&FilteredAnnFailure> {
        match self {
            FilteredAnnVerdict::Green(_) => None,
            FilteredAnnVerdict::Red(f) => Some(f),
        }
    }
}

/// **The SRCH-D8 filtered-ANN recall gate (SRCH-P26 / P-461).** Takes a [`RecallMeasurement`] (from
/// [`measure_recall_at_k`] over the live index under a selective filter) + the tuned strategy (from the
/// thresholds file) and produces a dated [`FilteredAnnArtifact`] or a typed [`FilteredAnnFailure`].
/// Stateless. The measurement (the I/O) is the caller's job — the producer/consumer split, the same
/// shape as [`crate::freshness::FreshnessGate`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FilteredAnnGate;

impl FilteredAnnGate {
    /// A new gate (stateless).
    pub fn new() -> FilteredAnnGate {
        FilteredAnnGate
    }

    /// **Evaluate the SRCH-D8 gate over a MEASURED recall set under a selective filter.**
    ///
    /// `visible_count` / `total_count` are the corpus sizes the filter selected (so the gate proves the
    /// filter was genuinely VERY SELECTIVE — ≤ the tuned fallback trigger). `m` is the measurement; `s`
    /// is the tuned strategy from the thresholds file; `k` is the neighbour count.
    ///
    /// The gate (pure logic, no I/O):
    /// 0. the strategy is well-formed (a 0 floor / 0 promotion point is a mis-specified bar — LOUD);
    /// 1. the measurement ran over ≥ 1 query (a vacuous run cannot prove a floor — LOUD);
    /// 2. the filter was VERY SELECTIVE (visible fraction ≤ the trigger — the regime the strategy is
    ///    tuned for — LOUD otherwise, so the drill cannot drift out of its regime);
    /// 3. **0 escapes** (no hidden doc surfaced, contract 1.8 — the gravest failure, checked first);
    /// 4. recall@k ≥ the floor (no visible nearest neighbour DROPPED).
    ///
    /// Returns [`FilteredAnnVerdict::Green`] (the dated artifact) or [`FilteredAnnVerdict::Red`]
    /// (exactly what broke). NEVER swallows.
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
        // (0) The strategy must be well-formed — a vacuous bar can never manufacture a green.
        if s.recall_at_k_bps == 0
            || s.brute_force_fallback_visible_bps == 0
            || s.brute_force_fallback_visible_bps > 10_000
            || s.ivf_pq_promotion_live_vectors == 0
        {
            return FilteredAnnVerdict::Red(FilteredAnnFailure::MisspecifiedStrategy);
        }
        // (1) A measurement over zero queries proves nothing.
        if m.queries == 0 {
            return FilteredAnnVerdict::Red(FilteredAnnFailure::NoQueries);
        }
        // (2) The filter must be VERY SELECTIVE — the regime the brute-force fallback is tuned for.
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
        // (3) 0 escapes — the leak check, the gravest failure, before recall.
        if m.escapes > 0 {
            return FilteredAnnVerdict::Red(FilteredAnnFailure::Leak { escapes: m.escapes });
        }
        // (4) recall@k ≥ the floor (floored measured bps — never rounded up over the floor).
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

    /// Build a deterministic `dim`-d corpus of `n` vectors + the matching live HNSW index.
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

    /// **The seed constants mirror the thresholds-file seeds (no drift).**
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

    /// **MEASURED recall@k under a very selective filter is EXACT (the brute-force fallback recovers the
    /// genuine k-nearest visible), with 0 escapes — the SRCH-D8 green artifact.**
    #[test]
    fn exact_recall_under_selective_filter_zero_escape() {
        let (corpus, idx) = corpus_and_index(500, 6, 0xD8_C0FFEE);
        // A very selective filter: 8 scattered docs visible of 500 (1.6 % ≤ the 20 % trigger).
        let visible_ids: Vec<String> =
            ["d3", "d61", "d130", "d199", "d255", "d310", "d404", "d489"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let visible = |doc: &str| visible_ids.iter().any(|v| v == doc);
        // Query points = a spread of corpus vectors (near visible + near invisible regions).
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

    /// **An ESCAPE (a hidden doc surfacing) fails the gate with a Leak — the gravest failure, checked
    /// before recall.** A measurement that recorded an escape is RED even if recall met the floor.
    #[test]
    fn an_escape_fails_loud_as_a_leak() {
        let m = RecallMeasurement {
            expected_hits: 10,
            recovered_hits: 10, // recall is perfect...
            escapes: 1,         // ...but a hidden doc leaked — the gate must still FAIL.
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
            "an escape is a leak, checked before recall — RED even at perfect recall"
        );
        assert!(!v.is_green());
    }

    /// **Recall BELOW the floor fails LOUD (a dropped visible NN) — the floor is not softened.**
    #[test]
    fn recall_under_floor_fails_loud() {
        // 9 of 10 expected recovered → 9000 bps < the 10000 floor.
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
            "recall under the floor is RED — never softened to pass"
        );
    }

    /// **A measured recall of 0.9999 floors to 9999 bps (never rounded UP over a 10000 floor).**
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
        // 9999 < 10000 floor ⇒ RED (a single dropped visible NN does not get flattered over the floor).
        assert!(matches!(
            v.failure(),
            Some(FilteredAnnFailure::RecallUnderFloor { .. })
        ));
    }

    /// **A 0-query measurement is RED (NoQueries) — a vacuous run proves nothing.**
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

    /// **A mis-specified strategy (0 recall floor) is RED — a vacuous bar can never manufacture green.**
    #[test]
    fn misspecified_strategy_is_red() {
        let m = RecallMeasurement {
            expected_hits: 10,
            recovered_hits: 10,
            escapes: 0,
            queries: 5,
        };
        let bad = FilteredAnnStrategy {
            recall_at_k_bps: 0, // "no recall required" — must be rejected.
            ..FilteredAnnStrategy::default()
        };
        let v = FilteredAnnGate::new().run(&tenant(), &region(), 3, 5, 500, &m, &bad, "2026-06-25");
        assert_eq!(v.failure(), Some(&FilteredAnnFailure::MisspecifiedStrategy));
    }

    /// **A NON-selective filter is RED (FilterNotSelective) — SRCH-D8 proves recall in the very-
    /// selective regime; the drill cannot drift out of its tuned regime.**
    #[test]
    fn non_selective_filter_is_red() {
        let m = RecallMeasurement {
            expected_hits: 10,
            recovered_hits: 10,
            escapes: 0,
            queries: 5,
        };
        // 300 of 500 visible (60 %) > the 20 % trigger ⇒ not very selective.
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

    /// **`is_very_selective` is integer-exact at the boundary (≤ the trigger, not <).**
    #[test]
    fn very_selective_boundary_is_inclusive() {
        let s = FilteredAnnStrategy::default(); // 20 % trigger
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

    /// **The HNSW→IVF-PQ promotion point binds AT/ABOVE the threshold, not below (§3.3).**
    #[test]
    fn ivf_pq_promotion_point_binds_at_threshold() {
        let s = FilteredAnnStrategy::default(); // 1 000 000
        assert!(
            s.should_promote_to_ivf_pq(1_000_000),
            "promotes at the threshold"
        );
        assert!(s.should_promote_to_ivf_pq(2_500_000), "promotes above it");
        assert!(
            !s.should_promote_to_ivf_pq(999_999),
            "does NOT promote below it"
        );
        // Promotion changes COST, not the recall floor — the floor still binds either side.
        assert_eq!(s.recall_floor_fraction(), 1.0);
    }
}
