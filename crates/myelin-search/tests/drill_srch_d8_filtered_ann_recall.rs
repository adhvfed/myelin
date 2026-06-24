//! # Drill — SRCH-D8 filtered-ANN recall@k under a selective filter (SRCH-P26 → global P-461, M5)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D8
//! (filtered-ANN recall: a selective filter → the k nearest VISIBLE neighbours; recall@k ≥ threshold,
//! 0 leak). **Architecture:** `search-and-indexing.md` §4.2.2 (filter-during-traversal + the
//! brute-force fallback for very selective filters), §3.3 (the HNSW↔IVF-PQ promotion point). **Contracts:**
//! 6.2 (the filtered-ANN traversal at scale), 1.8 (the recall + zero-escape telemetry). **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §3 (recall@k MEASURED never predicted).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-25)
//! A large deterministic HNSW corpus is built; a VERY selective ACL/structured filter (a handful of
//! scattered visible docs of a 500-doc corpus, ≤ the tuned 20 % fallback trigger) is applied; the
//! filtered-ANN top-k ([`myelin_search::HnswVectorIndex::knn_filtered`], the production code path) is
//! measured against the BRUTE-FORCE ground truth over the VISIBLE set across many query points
//! ([`myelin_search::measure_recall_at_k`]). The drill then asserts (via
//! [`myelin_search::FilteredAnnGate`]):
//!   1. the MEASURED recall@k meets the floor in the canonical thresholds file
//!      (`[filtered_ann] recall_at_k_bps` = exact recall) — the brute-force fallback recovers the
//!      genuine k-nearest VISIBLE neighbours under the selective filter;
//!   2. the **zero-escape counter is 0** (no hidden doc ever surfaced — contract 1.8);
//!   3. the filter was genuinely VERY SELECTIVE (≤ the tuned fallback trigger — the regime the strategy
//!      is tuned for), and the HNSW↔IVF-PQ promotion point is recorded;
//!   4. the floor the file records is ACHIEVABLE, never a softened bar (EI-01 §3).
//!
//! ## Honest recording (the TESTS line)
//! recall@k is **MEASURED against a brute-force ground truth over the visible set** (500-doc corpus,
//! many query points), NOT carried as a default-to-beat. The brute-force fallback makes EXACT recall
//! (100.00 %) achievable under the selective filter — the SRCH-P11 mutation floor on the leak-critical
//! exclusion still holds (this tunes the STRATEGY, it does not touch the property).
//!
//! ## Floors named
//! - The **world-scale 30× run on real fleet hardware** (the IVF-PQ promotion at a real million-vector
//!   cell, network-delivered) is the ONE remaining floor (shared testing-strategy §4.1). The strategy
//!   LOGIC + the dated artifact + the measured-recall-to-thresholds write ship now and re-run as a
//!   `cargo test` gate on every vector-touching change.
//! - The **real EU-hostable embedding-model adapter** (text→vector) is the post-M5 / runtime config swap
//!   (SRCH-P06 named floor) — the vector math + the erasure are done; the adapter is a config swap.

use myelin_search::{
    measure_recall_at_k, Embedding, FilteredAnnGate, FilteredAnnStrategy, FilteredAnnVerdict,
    HnswVectorIndex, ModelRef, VectorRecord,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

/// Build a deterministic `dim`-d corpus of `n` vectors + the matching live HNSW index (the corpus the
/// brute-force ground truth is computed from, and the index the production path queries).
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
            embedding: e,
            model_ref: ModelRef("m@1".into()),
        })
        .unwrap();
    }
    (corpus, idx)
}

/// **SRCH-D8: recall@k under a very selective filter meets the floor with 0 leak — the dated GREEN
/// ARTIFACT.** The filtered-ANN top-k is measured against the brute-force ground truth over the visible
/// set; the recall floor read from the canonical thresholds file is met exactly; the zero-escape
/// counter is 0.
#[test]
fn srch_d8_filtered_ann_recall_meets_floor_with_zero_leak() {
    // The canonical thresholds-file strategy (the source of truth). The drill proves the recorded floor
    // is ACHIEVABLE — it does not invent a looser one.
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let strategy = FilteredAnnStrategy::from_thresholds(&t.filtered_ann);

    let (corpus, idx) = corpus_and_index(500, 6, 0xD8_5EED_C0FF_EE11);
    // A VERY selective filter: 8 scattered docs visible of 500 (1.6 % ≤ the 20 % fallback trigger).
    let visible_ids: Vec<String> = ["d3", "d61", "d130", "d199", "d255", "d310", "d404", "d489"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let visible = |doc: &str| visible_ids.iter().any(|v| v == doc);

    // Many query points: a spread across the corpus (near visible regions AND deep in invisible regions
    // where the ANN graph walk would under-fill — forcing the brute-force fallback).
    let queries: Vec<Embedding> = [255, 10, 404, 250, 489, 7, 130, 320, 199, 61]
        .iter()
        .map(|&i| corpus[i].1.clone())
        .collect();

    let k = 3;
    let m = measure_recall_at_k(&idx, &corpus, &queries, visible, k);

    let verdict = FilteredAnnGate::new().run(
        &tenant(),
        &region(),
        k,
        visible_ids.len() as u64,
        corpus.len() as u64,
        &m,
        &strategy,
        "2026-06-25",
    );
    let artifact = match &verdict {
        FilteredAnnVerdict::Green(a) => a,
        FilteredAnnVerdict::Red(f) => panic!("SRCH-D8 RED under the selective filter: {f}"),
    };

    // recall@k met the floor (exact recall recovered by the brute-force fallback).
    assert!(
        artifact.measured_recall_bps >= artifact.recall_floor_bps,
        "recall@k {}bps must meet the {}bps floor",
        artifact.measured_recall_bps,
        artifact.recall_floor_bps
    );
    // The zero-escape counter is 0 — no hidden doc surfaced (contract 1.8).
    assert_eq!(artifact.escapes, 0, "0 escapes — no hidden doc leaked");
    assert!(
        artifact.measured,
        "recall was MEASURED vs brute-force ground truth, not a default-to-beat"
    );
    // The filter was genuinely VERY SELECTIVE (the regime the strategy is tuned for).
    assert!(
        artifact.visible_fraction_bps <= strategy.brute_force_fallback_visible_bps,
        "the filter is very selective ({}bps visible ≤ the {}bps trigger)",
        artifact.visible_fraction_bps,
        strategy.brute_force_fallback_visible_bps
    );
    // The HNSW↔IVF-PQ promotion point is recorded (§3.3).
    assert_eq!(
        artifact.ivf_pq_promotion_live_vectors,
        strategy.ivf_pq_promotion_live_vectors
    );
    assert!(artifact.is_green());

    // The dated green-artifact line (SCHED): observability is part of the pass.
    println!("[P-461 GATE GREEN 2026-06-25] {}", artifact.summary());
}

/// **The recall floor the thresholds file records is ACHIEVABLE under the selective filter (the file's
/// number is honest, never a softened bar — EI-01 §3).** The MEASURED recall is at-or-above the
/// recorded floor across an independently-seeded corpus.
#[test]
fn srch_d8_recorded_floor_is_achievable() {
    let t = Thresholds::load_canonical().expect("load");
    let strategy = FilteredAnnStrategy::from_thresholds(&t.filtered_ann);
    // The recorded floor is at-or-above the seed (the exact-recall bar) — never looser than the seed.
    assert!(
        strategy.recall_at_k_bps >= FilteredAnnStrategy::RECALL_AT_K_BPS_SEED,
        "the recorded recall floor must be at-or-above the exact-recall seed (a looser bar is weakened)"
    );

    // An independently-seeded corpus + a different selective filter → recall still meets the floor.
    let (corpus, idx) = corpus_and_index(400, 8, 0xABCD_1234_5678);
    let visible_ids: Vec<String> = ["d7", "d88", "d150", "d201", "d333"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let visible = |doc: &str| visible_ids.iter().any(|v| v == doc);
    let queries: Vec<Embedding> = [201, 7, 333, 100, 88]
        .iter()
        .map(|&i| corpus[i].1.clone())
        .collect();

    let m = measure_recall_at_k(&idx, &corpus, &queries, visible, 3);
    assert_eq!(m.escapes, 0, "no escape on the second corpus either");
    assert!(
        m.recall_bps() >= strategy.recall_at_k_bps,
        "the MEASURED recall {}bps must be at-or-above the recorded floor {}bps — the thresholds-file \
         number is achievable, never a lowered bar",
        m.recall_bps(),
        strategy.recall_at_k_bps
    );
    println!(
        "[P-461 SRCH-D8] measured recall = {}bps; recorded floor = {}bps; escapes = {}; IVF-PQ \
         promotion @ {} live vectors/cell",
        m.recall_bps(),
        strategy.recall_at_k_bps,
        m.escapes,
        strategy.ivf_pq_promotion_live_vectors,
    );
}

/// **The strategy carries the tuned HNSW↔IVF-PQ promotion point (§3.3) — promotion changes COST, not
/// the recall floor.** The promotion point binds at/above the per-cell threshold; the recall floor is
/// unchanged on either side of the promotion (cost-not-correctness).
#[test]
fn srch_d8_hnsw_to_ivf_pq_promotion_point_is_recorded() {
    let t = Thresholds::load_canonical().expect("load");
    let strategy = FilteredAnnStrategy::from_thresholds(&t.filtered_ann);

    let promo = strategy.ivf_pq_promotion_live_vectors;
    assert!(promo > 0, "the promotion point is a real per-cell count");
    // Below the point: HNSW v1 (full f32 in RAM). At/above: the IVF-PQ compression promotion.
    assert!(
        !strategy.should_promote_to_ivf_pq(promo - 1),
        "below the point the in-RAM HNSW shape holds"
    );
    assert!(
        strategy.should_promote_to_ivf_pq(promo),
        "at the point the per-cell memory budget triggers the IVF-PQ promotion (§3.3)"
    );
    // The recall floor binds either side of the promotion — cost changes, correctness does not.
    assert_eq!(
        strategy.recall_floor_fraction(),
        1.0,
        "exact recall — promotion changes RAM cost, never the recall floor"
    );
    println!(
        "[P-461 SRCH-D8] HNSW→IVF-PQ promotion point = {} live vectors/cell (§3.3, cost-not-correctness)",
        promo
    );
}
