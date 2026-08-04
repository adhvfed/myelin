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
fn srch_d8_filtered_ann_recall_meets_floor_with_zero_leak() {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let strategy = FilteredAnnStrategy::from_thresholds(&t.filtered_ann);

    let (corpus, idx) = corpus_and_index(500, 6, 0xD8_5EED_C0FF_EE11);
    let visible_ids: Vec<String> = ["d3", "d61", "d130", "d199", "d255", "d310", "d404", "d489"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let visible = |doc: &str| visible_ids.iter().any(|v| v == doc);

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

    assert!(
        artifact.measured_recall_bps >= artifact.recall_floor_bps,
        "recall@k {}bps must meet the {}bps floor",
        artifact.measured_recall_bps,
        artifact.recall_floor_bps
    );
    assert_eq!(artifact.escapes, 0, "0 escapes - no hidden doc leaked");
    assert!(
        artifact.measured,
        "recall was MEASURED vs brute-force ground truth, not a default-to-beat"
    );
    assert!(
        artifact.visible_fraction_bps <= strategy.brute_force_fallback_visible_bps,
        "the filter is very selective ({}bps visible ≤ the {}bps trigger)",
        artifact.visible_fraction_bps,
        strategy.brute_force_fallback_visible_bps
    );
    assert_eq!(
        artifact.ivf_pq_promotion_live_vectors,
        strategy.ivf_pq_promotion_live_vectors
    );
    assert!(artifact.is_green());

    println!("[P-461 GATE GREEN 2026-06-25] {}", artifact.summary());
}

#[test]
fn srch_d8_recorded_floor_is_achievable() {
    let t = Thresholds::load_canonical().expect("load");
    let strategy = FilteredAnnStrategy::from_thresholds(&t.filtered_ann);
    assert!(
        strategy.recall_at_k_bps >= FilteredAnnStrategy::RECALL_AT_K_BPS_SEED,
        "the recorded recall floor must be at-or-above the exact-recall seed (a looser bar is weakened)"
    );

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
        "the MEASURED recall {}bps must be at-or-above the recorded floor {}bps - the thresholds-file \
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

#[test]
fn srch_d8_hnsw_to_ivf_pq_promotion_point_is_recorded() {
    let t = Thresholds::load_canonical().expect("load");
    let strategy = FilteredAnnStrategy::from_thresholds(&t.filtered_ann);

    let promo = strategy.ivf_pq_promotion_live_vectors;
    assert!(promo > 0, "the promotion point is a real per-cell count");
    assert!(
        !strategy.should_promote_to_ivf_pq(promo - 1),
        "below the point the in-RAM HNSW shape holds"
    );
    assert!(
        strategy.should_promote_to_ivf_pq(promo),
        "at the point the per-cell memory budget triggers the IVF-PQ promotion (§3.3)"
    );
    assert_eq!(
        strategy.recall_floor_fraction(),
        1.0,
        "exact recall - promotion changes RAM cost, never the recall floor"
    );
    println!(
        "[P-461 SRCH-D8] HNSW→IVF-PQ promotion point = {} live vectors/cell (§3.3, cost-not-correctness)",
        promo
    );
}
