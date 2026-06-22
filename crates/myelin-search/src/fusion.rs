//! **Reciprocal Rank Fusion (RRF) — hybrid lexical + semantic fusion** (SRCH-P11 / P-174;
//! architecture `search-and-indexing.md` §4.5).
//!
//! Hybrid search runs two (or more) ranked branches — the **BM25 full-text** branch and the
//! **vector / semantic** branch — over the SAME one-doc-id space (§3.2). Their scores live on
//! incomparable scales (BM25 is an unbounded relevance sum; cosine similarity is `[0, 2]`), so a
//! naive score-add would let one branch's scale dominate and would need per-corpus calibration.
//! **RRF fuses on RANK, not score** (Cormack, Clarke & Büttcher, SIGIR 2009): each doc's fused
//! score is the sum, over the branches it appears in, of `1 / (k + rank)` where `rank` is its
//! 1-based position in that branch's ranked list and `k` is a smoothing constant (the conventional
//! `60`). This is **score-scale-free** — no per-corpus calibration — exactly the §4.5 property.
//!
//! ## The leak invariant (the SRCH-D1 vector/RAG half — the reason fusion is correct-by-construction)
//! RRF is a pure function of the RANK LISTS it is handed. It can only ever emit a `doc_id` that
//! appears in at least one input list. Because **both** branches are produced by the SAME conjoined
//! ACL filter (the FT branch via the posting-list pre-filter, the vector branch via
//! filter-during-traversal — `crate::pipeline::execute`), every input list is already
//! permission-correct, so **fusion can never introduce a hidden doc** (§4.5). The fusion layer
//! holds NO ACL state of its own — it cannot widen a result. This is asserted by
//! [`tests::rrf_never_introduces_a_doc_absent_from_every_branch`].
//!
//! ## What this is NOT (named floor)
//! The *tuned* re-rank — learning-to-rank, a cross-encoder semantic re-rank, per-tenant weighting —
//! is the post-M5 floor (SRCH-P26 / the BM25-default re-rank floor). RRF with the conventional
//! `k=60` is the score-scale-free v1 fusion; it is correct (no leak, deterministic, scale-free), not
//! yet tuned-at-scale.

/// The conventional RRF smoothing constant `k` (Cormack et al. 2009). Larger `k` flattens the
/// rank-position weighting (later ranks contribute relatively more); `60` is the standard default
/// and the score-scale-free v1 value. Tuning it per-tenant/per-corpus is the SRCH-P26 floor.
pub const RRF_K: f32 = 60.0;

/// One branch's **ranked result list** — the `doc_id`s in descending relevance order (rank 1 =
/// most relevant = position 0). Built from a branch's hits BEFORE fusion (the branch's own scores
/// are discarded by RRF — only the order matters, which is what makes fusion score-scale-free).
/// A branch contributes a doc at most once (deduped within the branch upstream).
#[derive(Clone, Debug, Default)]
pub struct RankedList {
    /// `doc_id`s in descending relevance order. The index is the 0-based rank.
    pub doc_ids: Vec<String>,
}

impl RankedList {
    /// An empty ranked list (a branch that produced no hits).
    pub fn new() -> RankedList {
        RankedList {
            doc_ids: Vec::new(),
        }
    }

    /// Build a ranked list from `doc_id`s already in descending-relevance order.
    pub fn from_ranked(doc_ids: impl IntoIterator<Item = impl Into<String>>) -> RankedList {
        RankedList {
            doc_ids: doc_ids.into_iter().map(Into::into).collect(),
        }
    }

    /// The number of ranked docs in this branch.
    pub fn len(&self) -> usize {
        self.doc_ids.len()
    }

    /// Whether the branch produced no hits.
    pub fn is_empty(&self) -> bool {
        self.doc_ids.is_empty()
    }
}

/// A **fused result row** — the `doc_id` and its accumulated RRF score (the sum of `1/(k+rank)`
/// across the branches it appeared in). Higher is more relevant. The fused list is the public,
/// score-scale-free hybrid ranking.
#[derive(Clone, Debug, PartialEq)]
pub struct FusedHit {
    /// The fused document's `doc_id` (the one-doc-id-space key, §3.2).
    pub doc_id: String,
    /// The accumulated RRF score (`Σ 1/(RRF_K + rank)` over the branches the doc appears in).
    pub score: f32,
}

/// **Reciprocal Rank Fusion of N ranked branches (§4.5).** For every `doc_id` that appears in any
/// branch, accumulate `1 / (RRF_K + rank)` (rank 1-based) across the branches it appears in, then
/// sort by the accumulated score descending (ties broken by `doc_id` ascending — a deterministic,
/// stable order so fusion is reproducible). Score-scale-free: the branch scores are NOT consulted,
/// only their rank positions, so no per-corpus calibration is needed.
///
/// **Leak-safe by construction:** the output `doc_id` set is exactly the UNION of the input lists —
/// RRF holds no ACL state and cannot emit a doc absent from every branch. The caller (the query
/// pipeline) guarantees every input list is already ACL-filtered (the same conjoined filter on both
/// branches), so the fused list is permission-correct (§4.5; the SRCH-D1 vector/RAG half).
pub fn reciprocal_rank_fusion(branches: &[RankedList]) -> Vec<FusedHit> {
    fuse_with_k(branches, RRF_K)
}

/// RRF with an explicit smoothing constant (the production path uses [`RRF_K`]; exposed so a test
/// can pin the rank-weighting math against a known `k`).
pub fn fuse_with_k(branches: &[RankedList], k: f32) -> Vec<FusedHit> {
    // A stable accumulation order: iterate branches then ranks, accumulating into a map. We keep
    // first-seen insertion order for determinism of the *accumulation*, but the final sort is by
    // (score desc, doc_id asc) so the public order is fully deterministic regardless of input order.
    use std::collections::HashMap;
    let mut scores: HashMap<&str, f32> = HashMap::new();
    for branch in branches {
        for (rank0, doc_id) in branch.doc_ids.iter().enumerate() {
            // 1-based rank: position 0 ⇒ rank 1 (the most relevant). The contribution is strictly
            // positive and strictly decreasing in rank, so a higher (better) position contributes
            // more — the RRF weighting. `k + rank` is never zero (k >= 0, rank >= 1).
            let rank = (rank0 + 1) as f32;
            *scores.entry(doc_id.as_str()).or_insert(0.0) += 1.0 / (k + rank);
        }
    }
    let mut fused: Vec<FusedHit> = scores
        .into_iter()
        .map(|(doc_id, score)| FusedHit {
            doc_id: doc_id.to_string(),
            score,
        })
        .collect();
    // Sort by fused score DESC, then doc_id ASC (deterministic, reproducible — the tuned re-rank is
    // SRCH-P26). A NaN score (impossible here — all contributions are finite positives) sorts last.
    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });
    fused
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn ids(hits: &[FusedHit]) -> Vec<&str> {
        hits.iter().map(|h| h.doc_id.as_str()).collect()
    }

    /// **A doc ranked highly in BOTH branches beats a doc ranked highly in only one (the core RRF
    /// property).** `a` is rank-1 in FT and rank-2 in vector; `b` is rank-1 in vector only. `a`'s
    /// fused score (two contributions) exceeds `b`'s (one) — the agreement boost.
    #[test]
    fn a_doc_ranked_by_both_branches_outranks_a_one_branch_doc() {
        let ft = RankedList::from_ranked(["a", "c", "d"]);
        let vec = RankedList::from_ranked(["b", "a", "e"]);
        let fused = reciprocal_rank_fusion(&[ft, vec]);
        assert_eq!(
            fused[0].doc_id, "a",
            "the doc both branches rank surfaces first (the RRF boost)"
        );
        // `a`: 1/(60+1) + 1/(60+2); `b`: 1/(60+1) only.
        let a = fused.iter().find(|h| h.doc_id == "a").unwrap().score;
        let b = fused.iter().find(|h| h.doc_id == "b").unwrap().score;
        assert!(
            a > b,
            "two-branch agreement ({a}) beats single-branch top ({b})"
        );
    }

    /// **RRF is score-scale-free: only ranks matter, not the underlying branch scores.** Two fusions
    /// over the SAME rank order produce the SAME fused order regardless of any score scale (RRF never
    /// sees scores — it takes rank lists). Pins that the fusion is calibration-free (§4.5).
    #[test]
    fn fusion_is_score_scale_free_depends_only_on_rank() {
        // Whatever the branch scores were, the rank lists are identical ⇒ identical fusion.
        let ft = RankedList::from_ranked(["x", "y", "z"]);
        let vec = RankedList::from_ranked(["z", "x", "y"]);
        let a = reciprocal_rank_fusion(&[ft.clone(), vec.clone()]);
        let b = reciprocal_rank_fusion(&[ft, vec]);
        assert_eq!(
            ids(&a),
            ids(&b),
            "fusion depends only on rank — reproducible, scale-free"
        );
    }

    /// **RRF NEVER introduces a doc absent from every branch (the leak invariant — SRCH-D1 vector
    /// half).** The fused `doc_id` set is exactly the union of the input lists; fusion holds no ACL
    /// state, so a doc neither branch surfaced can never appear. (The branches are ACL-filtered
    /// upstream; this proves fusion adds no widening.)
    #[test]
    fn rrf_never_introduces_a_doc_absent_from_every_branch() {
        let ft = RankedList::from_ranked(["a", "b"]);
        let vec = RankedList::from_ranked(["b", "c"]);
        let fused = reciprocal_rank_fusion(&[ft, vec]);
        let got: BTreeSet<&str> = fused.iter().map(|h| h.doc_id.as_str()).collect();
        let union: BTreeSet<&str> = ["a", "b", "c"].into_iter().collect();
        assert_eq!(
            got, union,
            "the fused set is EXACTLY the union of the branch lists — no new doc"
        );
        // The hidden doc the ACL filter excluded from both branches ("secret") is, by construction,
        // not in either list — so it cannot be in the fused output (the leak-safe property).
        assert!(
            !got.contains("secret"),
            "a doc in no branch never appears (no fusion-introduced leak)"
        );
    }

    /// **Determinism: ties (equal fused score) break by `doc_id` ascending; the order is stable
    /// regardless of input-branch order.** Two docs that are each rank-1 in exactly one branch tie
    /// on score and order by doc_id.
    #[test]
    fn fusion_is_deterministic_with_doc_id_tiebreak() {
        let ft = RankedList::from_ranked(["m"]);
        let vec = RankedList::from_ranked(["k"]);
        // `m` and `k` each score 1/(60+1) — a tie broken by doc_id asc ⇒ k before m.
        let fused = reciprocal_rank_fusion(&[ft.clone(), vec.clone()]);
        assert_eq!(
            ids(&fused),
            vec!["k", "m"],
            "tie broken by doc_id ascending"
        );
        // Swapping the branch order does not change the deterministic output.
        let swapped = reciprocal_rank_fusion(&[vec, ft]);
        assert_eq!(
            ids(&swapped),
            vec!["k", "m"],
            "stable regardless of branch order"
        );
    }

    /// **The RRF weighting is `1/(k + rank)` (ADDITIVE smoothing, 1-based rank) — pins the exact
    /// arithmetic against the multiplicative / 0-based mutants.** A doc ranked 2nd in BOTH branches
    /// must out-score a doc ranked 1st in only ONE branch — which holds for additive `k+rank`
    /// (`2·1/62 ≈ 0.0323 > 1/61 ≈ 0.0164`) but FAILS for the multiplicative `k·rank` form
    /// (`2·1/120 = 1/60 = 1/60`, a tie that would let the single-branch `a` win on the doc_id
    /// tiebreak). The fused ORDER therefore distinguishes the two arithmetics.
    #[test]
    fn weighting_is_additive_one_based_k_plus_rank() {
        // `a`: rank 1 in FT only. `z`: rank 2 in BOTH branches (a worse position, but in two lists).
        let ft = RankedList::from_ranked(["a", "z"]);
        let vec = RankedList::from_ranked(["w", "z"]);
        let fused = reciprocal_rank_fusion(&[ft, vec]);
        // Additive: z (two rank-2 contributions) out-scores a (one rank-1) ⇒ z first.
        assert_eq!(
            fused[0].doc_id, "z",
            "additive k+rank: two rank-2 hits beat one rank-1 hit"
        );
        let z = fused.iter().find(|h| h.doc_id == "z").unwrap().score;
        let a = fused.iter().find(|h| h.doc_id == "a").unwrap().score;
        assert!(
            z > a,
            "the two-branch rank-2 doc strictly out-scores the one-branch rank-1 doc ({z} > {a})"
        );
        // Pin the exact value so a 0-based (`rank0` instead of `rank0+1`) mutant is caught: `a` is
        // rank 1 ⇒ 1/(60+1) = 1/61.
        let expected_a = 1.0 / (RRF_K + 1.0);
        assert!(
            (a - expected_a).abs() < 1e-6,
            "rank-1 contribution is exactly 1/(k+1) = {expected_a}"
        );
    }

    /// **`RankedList::len` / `is_empty` report the branch size (exercised so the accessors are pinned).**
    #[test]
    fn ranked_list_len_and_is_empty() {
        let l = RankedList::from_ranked(["a", "b", "c"]);
        assert_eq!(l.len(), 3, "three ranked docs");
        assert!(!l.is_empty(), "a populated list is not empty");
        let e = RankedList::new();
        assert_eq!(e.len(), 0, "an empty list has length 0");
        assert!(e.is_empty(), "an empty list is empty");
    }

    /// **An empty branch contributes nothing (a single-branch query fuses to that branch's order).**
    #[test]
    fn empty_branch_is_a_no_op() {
        let ft = RankedList::from_ranked(["a", "b", "c"]);
        let empty = RankedList::new();
        let fused = reciprocal_rank_fusion(&[ft, empty]);
        assert_eq!(
            ids(&fused),
            vec!["a", "b", "c"],
            "the non-empty branch's order is preserved"
        );
        // Fusing nothing yields nothing.
        assert!(reciprocal_rank_fusion(&[]).is_empty());
        assert!(reciprocal_rank_fusion(&[RankedList::new()]).is_empty());
    }

    /// **The RRF contribution is strictly decreasing in rank (a higher position is worth more).**
    /// Pins the `1/(k+rank)` weighting direction — a mutant that flips it (rank-1 worth less) is
    /// caught: the rank-1 doc must out-score the rank-3 doc within a single branch.
    #[test]
    fn earlier_rank_contributes_more() {
        let only = RankedList::from_ranked(["first", "second", "third"]);
        let fused = reciprocal_rank_fusion(&[only]);
        assert_eq!(
            ids(&fused),
            vec!["first", "second", "third"],
            "rank order preserved by RRF"
        );
        assert!(
            fused[0].score > fused[1].score && fused[1].score > fused[2].score,
            "the contribution strictly decreases with rank (1/(k+rank))"
        );
    }
}
