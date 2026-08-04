pub const RRF_K: f32 = 60.0;

#[derive(Clone, Debug, Default)]
pub struct RankedList {
    pub doc_ids: Vec<String>,
}

impl RankedList {
    pub fn new() -> RankedList {
        RankedList {
            doc_ids: Vec::new(),
        }
    }

    pub fn from_ranked(doc_ids: impl IntoIterator<Item = impl Into<String>>) -> RankedList {
        RankedList {
            doc_ids: doc_ids.into_iter().map(Into::into).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.doc_ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.doc_ids.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FusedHit {
    pub doc_id: String,
    pub score: f32,
}

pub fn reciprocal_rank_fusion(branches: &[RankedList]) -> Vec<FusedHit> {
    fuse_with_k(branches, RRF_K)
}

pub fn fuse_with_k(branches: &[RankedList], k: f32) -> Vec<FusedHit> {
    use std::collections::HashMap;
    let mut scores: HashMap<&str, f32> = HashMap::new();
    for branch in branches {
        for (rank0, doc_id) in branch.doc_ids.iter().enumerate() {
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

    #[test]
    fn a_doc_ranked_by_both_branches_outranks_a_one_branch_doc() {
        let ft = RankedList::from_ranked(["a", "c", "d"]);
        let vec = RankedList::from_ranked(["b", "a", "e"]);
        let fused = reciprocal_rank_fusion(&[ft, vec]);
        assert_eq!(
            fused[0].doc_id, "a",
            "the doc both branches rank surfaces first (the RRF boost)"
        );
        let a = fused.iter().find(|h| h.doc_id == "a").unwrap().score;
        let b = fused.iter().find(|h| h.doc_id == "b").unwrap().score;
        assert!(
            a > b,
            "two-branch agreement ({a}) beats single-branch top ({b})"
        );
    }

    #[test]
    fn fusion_is_score_scale_free_depends_only_on_rank() {
        let ft = RankedList::from_ranked(["x", "y", "z"]);
        let vec = RankedList::from_ranked(["z", "x", "y"]);
        let a = reciprocal_rank_fusion(&[ft.clone(), vec.clone()]);
        let b = reciprocal_rank_fusion(&[ft, vec]);
        assert_eq!(
            ids(&a),
            ids(&b),
            "fusion depends only on rank - reproducible, scale-free"
        );
    }

    #[test]
    fn rrf_never_introduces_a_doc_absent_from_every_branch() {
        let ft = RankedList::from_ranked(["a", "b"]);
        let vec = RankedList::from_ranked(["b", "c"]);
        let fused = reciprocal_rank_fusion(&[ft, vec]);
        let got: BTreeSet<&str> = fused.iter().map(|h| h.doc_id.as_str()).collect();
        let union: BTreeSet<&str> = ["a", "b", "c"].into_iter().collect();
        assert_eq!(
            got, union,
            "the fused set is EXACTLY the union of the branch lists - no new doc"
        );
        assert!(
            !got.contains("secret"),
            "a doc in no branch never appears (no fusion-introduced leak)"
        );
    }

    #[test]
    fn fusion_is_deterministic_with_doc_id_tiebreak() {
        let ft = RankedList::from_ranked(["m"]);
        let vec = RankedList::from_ranked(["k"]);
        let fused = reciprocal_rank_fusion(&[ft.clone(), vec.clone()]);
        assert_eq!(
            ids(&fused),
            vec!["k", "m"],
            "tie broken by doc_id ascending"
        );
        let swapped = reciprocal_rank_fusion(&[vec, ft]);
        assert_eq!(
            ids(&swapped),
            vec!["k", "m"],
            "stable regardless of branch order"
        );
    }

    #[test]
    fn weighting_is_additive_one_based_k_plus_rank() {
        let ft = RankedList::from_ranked(["a", "z"]);
        let vec = RankedList::from_ranked(["w", "z"]);
        let fused = reciprocal_rank_fusion(&[ft, vec]);
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
        let expected_a = 1.0 / (RRF_K + 1.0);
        assert!(
            (a - expected_a).abs() < 1e-6,
            "rank-1 contribution is exactly 1/(k+1) = {expected_a}"
        );
    }

    #[test]
    fn ranked_list_len_and_is_empty() {
        let l = RankedList::from_ranked(["a", "b", "c"]);
        assert_eq!(l.len(), 3, "three ranked docs");
        assert!(!l.is_empty(), "a populated list is not empty");
        let e = RankedList::new();
        assert_eq!(e.len(), 0, "an empty list has length 0");
        assert!(e.is_empty(), "an empty list is empty");
    }

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
        assert!(reciprocal_rank_fusion(&[]).is_empty());
        assert!(reciprocal_rank_fusion(&[RankedList::new()]).is_empty());
    }

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
