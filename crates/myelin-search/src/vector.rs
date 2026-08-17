use std::collections::{BTreeMap, BinaryHeap, HashSet};

use myelin_storage::DekHandle;

use crate::engine::IndexError;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelRef(pub String);

impl ModelRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S: Into<String>> From<S> for ModelRef {
    fn from(s: S) -> Self {
        ModelRef(s.into())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Embedding(pub Vec<f32>);

impl Embedding {
    pub fn new(v: impl Into<Vec<f32>>) -> Embedding {
        Embedding(v.into())
    }

    pub fn dim(&self) -> usize {
        self.0.len()
    }

    fn cosine_distance(&self, other: &Embedding) -> f32 {
        let mut dot = 0.0f32;
        let mut na = 0.0f32;
        let mut nb = 0.0f32;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            dot += a * b;
            na += a * a;
            nb += b * b;
        }
        if na == 0.0 || nb == 0.0 {
            return 1.0;
        }
        let sim = dot / (na.sqrt() * nb.sqrt());
        1.0 - sim.clamp(-1.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VectorRecord {
    pub doc_id: String,
    pub acl_object: String,
    pub embedding: Embedding,
    pub model_ref: ModelRef,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VectorHit {
    pub doc_id: String,
    pub similarity: f32,
    pub model_ref: ModelRef,
}

#[derive(Clone, Debug)]
struct Node {
    record: VectorRecord,
    neighbours: Vec<Vec<usize>>,
    tombstoned: bool,
}

pub struct HnswVectorIndex {
    dim: Option<usize>,
    nodes: Vec<Node>,
    doc_index: BTreeMap<String, usize>,
    entry: Option<usize>,
    m: usize,
    m0: usize,
    ef_construction: usize,
    rng_state: u64,
}

impl HnswVectorIndex {
    pub fn open() -> HnswVectorIndex {
        HnswVectorIndex {
            dim: None,
            nodes: Vec::new(),
            doc_index: BTreeMap::new(),
            entry: None,
            m: 16,
            m0: 32,
            ef_construction: 64,
            rng_state: 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn live_len(&self) -> usize {
        self.nodes.iter().filter(|n| !n.tombstoned).count()
    }

    pub fn physical_len(&self) -> usize {
        self.nodes.len()
    }

    pub fn contains(&self, doc_id: &str) -> bool {
        self.doc_index
            .get(doc_id)
            .is_some_and(|&id| !self.nodes[id].tombstoned)
    }

    pub fn model_ref_of(&self, doc_id: &str) -> Option<&ModelRef> {
        self.doc_index
            .get(doc_id)
            .filter(|&&id| !self.nodes[id].tombstoned)
            .map(|&id| &self.nodes[id].record.model_ref)
    }

    fn next_rand(&mut self) -> u64 {
        self.rng_state = self.rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn random_layer(&mut self) -> usize {
        let u = (self.next_rand() >> 11) as f64 / (1u64 << 53) as f64;
        let u = u.max(f64::MIN_POSITIVE);
        let ml = 1.0 / (self.m as f64).ln();
        (-u.ln() * ml).floor() as usize
    }

    pub fn upsert(&mut self, record: VectorRecord) -> Result<(), IndexError> {
        self.validate_embedding(&record.embedding)?;
        let dim = record.embedding.dim();
        if self.dim.is_none() {
            self.dim = Some(dim);
        }

        self.soft_delete(&record.doc_id);

        let layer = self.random_layer();
        let new_id = self.nodes.len();
        self.nodes.push(Node {
            record: record.clone(),
            neighbours: vec![Vec::new(); layer + 1],
            tombstoned: false,
        });
        self.doc_index.insert(record.doc_id.clone(), new_id);

        let entry = match self.entry {
            None => {
                self.entry = Some(new_id);
                return Ok(());
            }
            Some(e) => e,
        };

        let top_layer = self.nodes[entry].neighbours.len() - 1;
        let query = record.embedding.clone();

        let mut ep = entry;
        let mut l = top_layer;
        while l > layer {
            ep = self.greedy_descend(&query, ep, l);
            if l == 0 {
                break;
            }
            l -= 1;
        }

        let mut cur_ep = ep;
        for cur_l in (0..=layer.min(top_layer)).rev() {
            let candidates = self.search_layer(&query, cur_ep, cur_l, self.ef_construction);
            let m = if cur_l == 0 { self.m0 } else { self.m };
            let selected = self.select_neighbours(&query, &candidates, m);
            for &nbr in &selected {
                self.nodes[new_id].neighbours[cur_l].push(nbr);
                self.nodes[nbr].neighbours[cur_l].push(new_id);
                self.prune(nbr, cur_l);
            }
            cur_ep = candidates.first().map(|&(id, _)| id).unwrap_or(cur_ep);
        }

        if layer > top_layer {
            self.entry = Some(new_id);
        }
        Ok(())
    }

    pub(crate) fn validate_embedding(&self, embedding: &Embedding) -> Result<(), IndexError> {
        let dim = embedding.dim();
        if dim == 0 {
            return Err(IndexError::Engine(
                "a vector embedding must be non-empty".into(),
            ));
        }
        if let Some(expected) = self.dim {
            if expected != dim {
                return Err(IndexError::Engine(format!(
                    "vector dimensionality {dim} does not match the index dimensionality {expected} \
                     (a model swap must reindex, never mix dimensions - §3.3)"
                )));
            }
        }
        Ok(())
    }

    fn greedy_descend(&self, query: &Embedding, ep: usize, l: usize) -> usize {
        let mut best = ep;
        let mut best_d = self.nodes[best].record.embedding.cosine_distance(query);
        loop {
            let mut improved = false;
            if l < self.nodes[best].neighbours.len() {
                for &nbr in &self.nodes[best].neighbours[l] {
                    let d = self.nodes[nbr].record.embedding.cosine_distance(query);
                    if d < best_d {
                        best_d = d;
                        best = nbr;
                        improved = true;
                    }
                }
            }
            if !improved {
                return best;
            }
        }
    }

    fn search_layer(&self, query: &Embedding, ep: usize, l: usize, ef: usize) -> Vec<(usize, f32)> {
        let mut visited: HashSet<usize> = HashSet::new();
        let mut frontier: BinaryHeap<std::cmp::Reverse<Cand>> = BinaryHeap::new();
        let mut results: BinaryHeap<Cand> = BinaryHeap::new();

        let d0 = self.nodes[ep].record.embedding.cosine_distance(query);
        visited.insert(ep);
        frontier.push(std::cmp::Reverse(Cand { id: ep, dist: d0 }));
        if !self.nodes[ep].tombstoned {
            results.push(Cand { id: ep, dist: d0 });
        }

        while let Some(std::cmp::Reverse(cur)) = frontier.pop() {
            if results.len() >= ef {
                if let Some(worst) = results.peek() {
                    if cur.dist > worst.dist {
                        break;
                    }
                }
            }
            if l < self.nodes[cur.id].neighbours.len() {
                for &nbr in &self.nodes[cur.id].neighbours[l] {
                    if !visited.insert(nbr) {
                        continue;
                    }
                    let d = self.nodes[nbr].record.embedding.cosine_distance(query);
                    let worst = results.peek().map(|c| c.dist).unwrap_or(f32::INFINITY);
                    if results.len() < ef || d < worst {
                        frontier.push(std::cmp::Reverse(Cand { id: nbr, dist: d }));
                        if !self.nodes[nbr].tombstoned {
                            results.push(Cand { id: nbr, dist: d });
                            if results.len() > ef {
                                results.pop();
                            }
                        }
                    }
                }
            }
        }

        let mut out: Vec<(usize, f32)> = results.into_iter().map(|c| (c.id, c.dist)).collect();
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    fn select_neighbours(
        &self,
        _query: &Embedding,
        candidates: &[(usize, f32)],
        m: usize,
    ) -> Vec<usize> {
        candidates.iter().take(m).map(|&(id, _)| id).collect()
    }

    fn prune(&mut self, node: usize, l: usize) {
        let cap = if l == 0 { self.m0 } else { self.m };
        if self.nodes[node].neighbours[l].len() <= cap {
            return;
        }
        let base = self.nodes[node].record.embedding.clone();
        let mut nbrs: Vec<usize> = self.nodes[node].neighbours[l].clone();
        nbrs.sort_by(|&a, &b| {
            let da = self.nodes[a].record.embedding.cosine_distance(&base);
            let db = self.nodes[b].record.embedding.cosine_distance(&base);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
        nbrs.truncate(cap);
        self.nodes[node].neighbours[l] = nbrs;
    }

    pub fn knn(&self, query: &Embedding, k: usize) -> Vec<VectorHit> {
        self.knn_filtered(query, k, |_, _| true)
    }

    pub fn knn_filtered(
        &self,
        query: &Embedding,
        k: usize,
        visible: impl Fn(&str, &str) -> bool,
    ) -> Vec<VectorHit> {
        let entry = match self.entry {
            None => return Vec::new(),
            Some(e) => e,
        };
        if k == 0 {
            return Vec::new();
        }
        let top_layer = self.nodes[entry].neighbours.len() - 1;
        let mut ep = entry;
        let mut l = top_layer;
        while l > 0 {
            ep = self.greedy_descend(query, ep, l);
            l -= 1;
        }
        let ef = k.max(self.ef_construction);
        let candidates = self.search_layer(query, ep, 0, ef);
        let mut hits: Vec<VectorHit> = Vec::with_capacity(k);
        for (id, dist) in candidates {
            let node = &self.nodes[id];
            if node.tombstoned {
                continue;
            }
            if !visible(&node.record.doc_id, &node.record.acl_object) {
                continue;
            }
            hits.push(VectorHit {
                doc_id: node.record.doc_id.clone(),
                similarity: 1.0 - dist,
                model_ref: node.record.model_ref.clone(),
            });
            if hits.len() == k {
                break;
            }
        }

        if hits.len() < k && self.visible_live_count(&visible) > hits.len() {
            return self.brute_force_visible(query, k, &visible);
        }
        hits
    }

    fn visible_live_count(&self, visible: &impl Fn(&str, &str) -> bool) -> usize {
        self.nodes
            .iter()
            .filter(|n| !n.tombstoned && visible(&n.record.doc_id, &n.record.acl_object))
            .count()
    }

    fn brute_force_visible(
        &self,
        query: &Embedding,
        k: usize,
        visible: &impl Fn(&str, &str) -> bool,
    ) -> Vec<VectorHit> {
        let mut scored: Vec<(f32, &Node)> = self
            .nodes
            .iter()
            .filter(|n| !n.tombstoned && visible(&n.record.doc_id, &n.record.acl_object))
            .map(|n| (n.record.embedding.cosine_distance(query), n))
            .collect();
        scored.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.record.doc_id.cmp(&b.1.record.doc_id))
        });
        scored
            .into_iter()
            .take(k)
            .map(|(dist, n)| VectorHit {
                doc_id: n.record.doc_id.clone(),
                similarity: 1.0 - dist,
                model_ref: n.record.model_ref.clone(),
            })
            .collect()
    }

    pub fn soft_delete(&mut self, doc_id: &str) -> bool {
        if let Some(&id) = self.doc_index.get(doc_id) {
            if !self.nodes[id].tombstoned {
                self.nodes[id].tombstoned = true;
                self.doc_index.remove(doc_id);
                return true;
            }
        }
        false
    }

    pub fn compact(&mut self) {
        let survivors: Vec<VectorRecord> = self
            .nodes
            .iter()
            .filter(|n| !n.tombstoned)
            .map(|n| n.record.clone())
            .collect();

        self.nodes.clear();
        self.doc_index.clear();
        self.entry = None;
        self.rng_state = 0x9E37_79B9_7F4A_7C15;
        for rec in survivors {
            let _ = self.upsert(rec);
        }
    }

    pub fn has_orphan_embedding(&self) -> bool {
        self.nodes.iter().any(|n| n.tombstoned)
    }

    pub fn seal_segment(&self, dek: &DekHandle) -> ([u8; 12], Vec<u8>) {
        let plaintext = self.serialize_live();
        dek.seal(&plaintext)
    }

    pub fn open_segment(
        dek: &DekHandle,
        nonce: &[u8; 12],
        ciphertext: &[u8],
    ) -> Result<Option<HnswVectorIndex>, IndexError> {
        let Some(plaintext) = dek.open(nonce, ciphertext) else {
            return Ok(None);
        };
        let records = Self::deserialize(&plaintext)?;
        let mut idx = HnswVectorIndex::open();
        for rec in records {
            idx.upsert(rec)?;
        }
        Ok(Some(idx))
    }

    fn serialize_live(&self) -> Vec<u8> {
        let mut out = String::new();
        for node in self.nodes.iter().filter(|n| !n.tombstoned) {
            let dims = node
                .record
                .embedding
                .0
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&node.record.doc_id);
            out.push('\t');
            out.push_str(node.record.model_ref.as_str());
            out.push('\t');
            out.push_str(&dims);
            out.push('\t');
            out.push_str(&node.record.acl_object);
            out.push('\n');
        }
        out.into_bytes()
    }

    fn deserialize(bytes: &[u8]) -> Result<Vec<VectorRecord>, IndexError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| IndexError::Engine(format!("sealed vector segment is not utf-8: {e}")))?;
        let mut out = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            let doc_id = parts
                .next()
                .ok_or_else(|| IndexError::Engine("vector segment line has no doc_id".into()))?;
            let model_ref = parts
                .next()
                .ok_or_else(|| IndexError::Engine("vector segment line has no model_ref".into()))?;
            let dims = parts
                .next()
                .ok_or_else(|| IndexError::Engine("vector segment line has no dims".into()))?;
            let acl_object = parts.next().unwrap_or(doc_id).to_string();
            let v: Result<Vec<f32>, _> = dims.split(',').map(|s| s.parse::<f32>()).collect();
            let v = v.map_err(|e| IndexError::Engine(format!("vector dim parse: {e}")))?;
            out.push(VectorRecord {
                doc_id: doc_id.to_string(),
                acl_object,
                embedding: Embedding(v),
                model_ref: ModelRef(model_ref.to_string()),
            });
        }
        Ok(out)
    }
}

#[derive(Clone, Copy, Debug)]
struct Cand {
    id: usize,
    dist: f32,
}

impl PartialEq for Cand {
    fn eq(&self, other: &Self) -> bool {
        self.dist == other.dist
    }
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Cand {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.dist
            .partial_cmp(&other.dist)
            .unwrap_or(std::cmp::Ordering::Greater)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dek() -> DekHandle {
        use myelin_storage::{KekId, KmsEngine};
        use myelin_tenancy::{Region, TenantId};
        use std::sync::Arc;
        let kms = Arc::new(KmsEngine::new());
        let t = TenantId("acme".into());
        let r = Region("fr-par".into());
        kms.ensure_kek(&KekId::new(t.clone(), r.clone()))
            .expect("seed the in-memory KEK");
        let key_ref = kms
            .ensure_dek(&t, &r, myelin_storage::KeyClass::Tenant)
            .expect("dek");
        kms.resolve_dek(&key_ref, &r).expect("resolve")
    }

    fn rec(doc: &str, v: Vec<f32>, model: &str) -> VectorRecord {
        VectorRecord {
            doc_id: doc.into(),
            acl_object: doc.into(),
            embedding: Embedding(v),
            model_ref: ModelRef(model.into()),
        }
    }

    fn rec_acl(doc: &str, acl_object: &str, v: Vec<f32>, model: &str) -> VectorRecord {
        VectorRecord {
            doc_id: doc.into(),
            acl_object: acl_object.into(),
            embedding: Embedding(v),
            model_ref: ModelRef(model.into()),
        }
    }

    #[test]
    fn incremental_insert_and_knn() {
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec("a", vec![1.0, 0.0, 0.0], "m@1")).unwrap();
        idx.upsert(rec("b", vec![0.0, 1.0, 0.0], "m@1")).unwrap();
        idx.upsert(rec("c", vec![0.0, 0.0, 1.0], "m@1")).unwrap();
        idx.upsert(rec("d", vec![0.9, 0.1, 0.0], "m@1")).unwrap();
        assert_eq!(idx.live_len(), 4);

        let hits = idx.knn(&Embedding(vec![1.0, 0.05, 0.0]), 2);
        assert_eq!(hits.len(), 2, "k=2 nearest");
        let ids: HashSet<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids.contains("a") && ids.contains("d"),
            "a and d are nearest, got {ids:?}"
        );
        assert!(
            hits[0].similarity >= hits[1].similarity,
            "sorted by similarity desc"
        );
        assert!(
            hits[0].similarity > 0.99,
            "the near-identical vector is highly similar"
        );
    }

    #[test]
    fn model_ref_is_carried_on_every_vector() {
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec("a", vec![1.0, 0.0], "text-embed-3@1"))
            .unwrap();
        assert_eq!(
            idx.model_ref_of("a"),
            Some(&ModelRef("text-embed-3@1".into()))
        );

        let hits = idx.knn(&Embedding(vec![1.0, 0.0]), 1);
        assert_eq!(
            hits[0].model_ref,
            ModelRef("text-embed-3@1".into()),
            "the hit carries model_ref"
        );

        idx.upsert(rec("a", vec![0.0, 1.0], "text-embed-4@1"))
            .unwrap();
        assert_eq!(
            idx.model_ref_of("a"),
            Some(&ModelRef("text-embed-4@1".into())),
            "new model"
        );
        assert_eq!(
            idx.live_len(),
            1,
            "the same doc_id replaced, not duplicated (one doc-id space)"
        );
    }

    #[test]
    fn soft_delete_then_compact_zero_orphan_embedding() {
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec("keep1", vec![1.0, 0.0], "m@1")).unwrap();
        idx.upsert(rec("erase", vec![0.0, 1.0], "m@1")).unwrap();
        idx.upsert(rec("keep2", vec![0.0, 0.0], "m@1")).unwrap();
        assert_eq!(idx.physical_len(), 3);

        assert!(idx.soft_delete("erase"), "a live vector was tombstoned");
        assert!(
            !idx.contains("erase"),
            "the erased vector no longer surfaces"
        );
        assert!(
            idx.has_orphan_embedding(),
            "its bytes are still physically present (tombstoned)"
        );
        assert_eq!(
            idx.physical_len(),
            3,
            "still physically there until compaction"
        );
        assert_eq!(idx.live_len(), 2, "two live");

        let hits = idx.knn(&Embedding(vec![0.0, 1.0]), 3);
        assert!(
            !hits.iter().any(|h| h.doc_id == "erase"),
            "the soft-deleted vector never surfaces, even as the nearest"
        );

        idx.compact();
        assert!(
            !idx.has_orphan_embedding(),
            "0 orphan embedding after compaction (the GATE)"
        );
        assert_eq!(
            idx.physical_len(),
            2,
            "the tombstoned bytes are physically gone"
        );
        assert_eq!(idx.live_len(), 2, "the live set is intact after compaction");
        assert!(
            idx.contains("keep1") && idx.contains("keep2"),
            "survivors kept"
        );
        let hits = idx.knn(&Embedding(vec![1.0, 0.0]), 1);
        assert_eq!(hits[0].doc_id, "keep1", "k-NN works post-compaction");
    }

    #[test]
    fn soft_delete_idempotent_and_reinsert() {
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec("d", vec![1.0, 0.0], "m@1")).unwrap();
        assert!(idx.soft_delete("d"));
        assert!(!idx.soft_delete("d"), "second soft-delete is a no-op");
        assert!(
            !idx.soft_delete("absent"),
            "deleting an absent doc is a no-op"
        );

        idx.upsert(rec("d", vec![0.0, 1.0], "m@2")).unwrap();
        assert!(idx.contains("d"), "re-inserted");
        assert_eq!(idx.model_ref_of("d"), Some(&ModelRef("m@2".into())));
    }

    #[test]
    fn knn_filtered_returns_k_visible_neighbours() {
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec("secret", vec![1.0, 0.0, 0.0], "m@1"))
            .unwrap();
        idx.upsert(rec("v1", vec![0.95, 0.05, 0.0], "m@1")).unwrap();
        idx.upsert(rec("v2", vec![0.9, 0.1, 0.0], "m@1")).unwrap();
        idx.upsert(rec("v3", vec![0.85, 0.15, 0.0], "m@1")).unwrap();

        let visible = |doc: &str, _acl: &str| doc != "secret";
        let hits = idx.knn_filtered(&Embedding(vec![1.0, 0.0, 0.0]), 2, visible);
        assert_eq!(
            hits.len(),
            2,
            "two VISIBLE neighbours (the hidden one didn't waste a slot)"
        );
        assert!(
            !hits.iter().any(|h| h.doc_id == "secret"),
            "the hidden vector never surfaces (no post-filter leak/under-fill)"
        );
    }

    #[test]
    fn knn_filtered_matches_doc_id_or_acl_object() {
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec_acl(
            "page/secret#b1",
            "page/secret",
            vec![1.0, 0.0, 0.0],
            "m@1",
        ))
        .unwrap();

        let by_parent = |_doc: &str, acl: &str| acl == "page/secret";
        let hits = idx.knn_filtered(&Embedding(vec![1.0, 0.0, 0.0]), 5, by_parent);
        assert_eq!(
            hits.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(),
            vec!["page/secret#b1"],
            "a grant on the parent acl_object admits the sub-doc's vector (acl_object arm)"
        );

        let deny_parent = |_doc: &str, acl: &str| acl != "page/secret";
        assert!(
            idx.knn_filtered(&Embedding(vec![1.0, 0.0, 0.0]), 5, deny_parent)
                .is_empty(),
            "a deny on the parent acl_object excludes the sub-doc's vector (no semantic leak)"
        );

        let deny_docid = |doc: &str, _acl: &str| doc != "page/secret#b1";
        assert!(
            idx.knn_filtered(&Embedding(vec![1.0, 0.0, 0.0]), 5, deny_docid)
                .is_empty(),
            "a deny on the sub-precise doc_id excludes the sub-doc's vector (doc_id arm intact)"
        );
    }

    #[test]
    fn very_selective_filter_falls_back_to_brute_force_over_visible_set() {
        let mut idx = HnswVectorIndex::open();
        let mut s: u64 = 0xC0FF_EE11;
        let mut gen = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f32 / (1u64 << 31) as f32) - 1.0
        };
        let mut corpus: Vec<(String, Vec<f32>)> = Vec::new();
        for i in 0..400 {
            let v: Vec<f32> = (0..6).map(|_| gen()).collect();
            corpus.push((format!("d{i}"), v.clone()));
            idx.upsert(rec(&format!("d{i}"), v, "m@1")).unwrap();
        }
        let visible_ids: Vec<String> = ["d3", "d97", "d180", "d255", "d399"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let visible = |doc: &str, _acl: &str| visible_ids.iter().any(|v| v == doc);

        let q = Embedding(corpus[255].1.clone());
        let hits = idx.knn_filtered(&q, 3, visible);

        let mut truth: Vec<(f32, String)> = corpus
            .iter()
            .filter(|(id, _)| visible(id, id))
            .map(|(id, v)| (Embedding(v.clone()).cosine_distance(&q), id.clone()))
            .collect();
        truth.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then_with(|| a.1.cmp(&b.1)));
        let truth_ids: Vec<&str> = truth.iter().take(3).map(|(_, id)| id.as_str()).collect();

        let got: Vec<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            got, truth_ids,
            "the k-nearest VISIBLE neighbours (recovered by brute-force)"
        );
        assert_eq!(
            hits.len(),
            3,
            "k visible neighbours, fully filled (not under-filled)"
        );
        assert!(
            hits.iter().all(|h| visible(&h.doc_id, &h.doc_id)),
            "no hidden vector surfaced"
        );
        assert_eq!(
            hits[0].doc_id, "d255",
            "the exact nearest visible neighbour is first"
        );
    }

    #[test]
    fn brute_force_fallback_excludes_tombstoned_and_invisible_and_ranks_exactly() {
        let mut idx = HnswVectorIndex::open();
        let mut s: u64 = 0xBEEF_0042;
        let mut gen = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f32 / (1u64 << 31) as f32) - 1.0
        };
        for i in 0..300 {
            let v: Vec<f32> = (0..5).map(|_| gen()).collect();
            idx.upsert(rec(&format!("filler{i}"), v, "m@1")).unwrap();
        }
        idx.upsert(rec("near_visible", vec![1.0, 0.0, 0.0, 0.0, 0.0], "m@1"))
            .unwrap();
        idx.upsert(rec(
            "near_invisible",
            vec![0.99, 0.01, 0.0, 0.0, 0.0],
            "m@1",
        ))
        .unwrap();
        idx.upsert(rec("near_tombstoned", vec![1.0, 0.0, 0.0, 0.0, 0.0], "m@1"))
            .unwrap();
        idx.upsert(rec("far_visible", vec![-1.0, 0.0, 0.0, 0.0, 0.0], "m@1"))
            .unwrap();
        assert!(idx.soft_delete("near_tombstoned"));

        let visible = |doc: &str, _acl: &str| doc == "near_visible" || doc == "far_visible";
        let q = Embedding(vec![1.0, 0.0, 0.0, 0.0, 0.0]);
        let hits = idx.knn_filtered(&q, 5, visible);

        let ids: Vec<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["near_visible", "far_visible"],
            "exact ascending-distance order over visible set"
        );
        assert!(
            !ids.contains(&"near_tombstoned"),
            "a tombstoned vector never enters the brute-force scan"
        );
        assert!(
            !ids.contains(&"near_invisible"),
            "an invisible-but-near vector never enters the scan (no leak)"
        );
        assert!(
            hits[0].similarity > hits[1].similarity,
            "nearest has the higher similarity (ascending distance)"
        );
        assert!(
            hits[0].similarity > 0.99,
            "the at-query vector is maximally similar"
        );
    }

    #[test]
    fn fallback_does_not_fire_when_graph_walk_fills_k() {
        let mut idx = HnswVectorIndex::open();
        for i in 0..50 {
            let v = vec![(i as f32).sin(), (i as f32).cos()];
            idx.upsert(rec(&format!("d{i}"), v, "m@1")).unwrap();
        }
        let visible = |_: &str, _: &str| true;
        let q = Embedding(vec![1.0, 0.0]);
        let hits = idx.knn_filtered(&q, 3, visible);
        assert_eq!(
            hits.len(),
            3,
            "k visible neighbours filled by the graph walk (no under-fill)"
        );
        let mut truth: Vec<(f32, String)> = (0..50)
            .map(|i| {
                let v = vec![(i as f32).sin(), (i as f32).cos()];
                (Embedding(v).cosine_distance(&q), format!("d{i}"))
            })
            .collect();
        truth.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert_eq!(
            hits[0].doc_id, truth[0].1,
            "the nearest is the true nearest (graph walk correct)"
        );
    }

    #[test]
    fn fallback_returns_all_visible_when_fewer_than_k() {
        let mut idx = HnswVectorIndex::open();
        for i in 0..100 {
            let v = vec![(i as f32).sin(), (i as f32).cos(), (i as f32 * 0.3).sin()];
            idx.upsert(rec(&format!("d{i}"), v, "m@1")).unwrap();
        }
        let visible = |doc: &str, _acl: &str| doc == "d10" || doc == "d50";
        let q = Embedding(vec![0.0, 1.0, 0.0]);
        let hits = idx.knn_filtered(&q, 5, visible);
        assert_eq!(
            hits.len(),
            2,
            "exactly the two visible docs - never padded with a hidden one"
        );
        assert!(hits.iter().all(|h| h.doc_id == "d10" || h.doc_id == "d50"));
    }

    #[test]
    fn zero_norm_vector_has_defined_distance() {
        let zero = Embedding(vec![0.0, 0.0, 0.0]);
        let nonzero = Embedding(vec![1.0, 2.0, 3.0]);
        let d1 = zero.cosine_distance(&nonzero);
        let d2 = nonzero.cosine_distance(&zero);
        let d3 = zero.cosine_distance(&zero);
        for d in [d1, d2, d3] {
            assert!(
                !d.is_nan(),
                "a zero-norm vector must not produce a NaN distance"
            );
            assert_eq!(
                d, 1.0,
                "the zero-norm guard yields the defined sentinel distance 1.0"
            );
        }
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec("a", vec![1.0, 0.0, 0.0], "m@1")).unwrap();
        let hits = idx.knn(&Embedding(vec![0.0, 0.0, 0.0]), 1);
        assert_eq!(
            hits.len(),
            1,
            "a zero query still searches (defined distance), no NaN"
        );
        assert_eq!(
            hits[0].similarity, 0.0,
            "similarity = 1 - 1.0 = 0 for a zero-norm query"
        );
    }

    #[test]
    fn dimension_mismatch_is_loud() {
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec("a", vec![1.0, 0.0, 0.0], "m@1")).unwrap();
        let err = idx
            .upsert(rec("b", vec![1.0, 0.0], "m@1"))
            .expect_err("dim mismatch");
        assert!(
            matches!(err, IndexError::Engine(_)),
            "loud dimension-mismatch error"
        );
        let empty = idx.upsert(rec("c", vec![], "m@1")).expect_err("empty");
        assert!(
            matches!(empty, IndexError::Engine(_)),
            "an empty embedding is rejected"
        );
    }

    #[test]
    fn segment_is_dek_sealed_encrypted_from_birth() {
        let mut idx = HnswVectorIndex::open();
        idx.upsert(rec("a", vec![1.0, 0.0], "m@1")).unwrap();
        idx.upsert(rec("b", vec![0.0, 1.0], "m@1")).unwrap();
        idx.soft_delete("b");

        let key = dek();
        let (nonce, ct) = idx.seal_segment(&key);
        assert!(!ct.is_empty(), "non-empty ciphertext");
        assert!(
            !String::from_utf8_lossy(&ct).contains("m@1"),
            "the model_ref does not appear in the clear (the segment is sealed)"
        );

        let restored = HnswVectorIndex::open_segment(&key, &nonce, &ct)
            .expect("open")
            .expect("the right key opens");
        assert!(restored.contains("a"), "the live vector round-trips");
        assert!(
            !restored.contains("b"),
            "the soft-deleted vector's bytes never reached the segment"
        );
        assert_eq!(
            restored.model_ref_of("a"),
            Some(&ModelRef("m@1".into())),
            "model_ref round-trips"
        );

        let wrong = dek();
        assert!(
            HnswVectorIndex::open_segment(&wrong, &nonce, &ct)
                .expect("no error")
                .is_none(),
            "a wrong/shredded key yields None, never a plaintext leak"
        );
    }

    #[test]
    fn knn_finds_the_true_nearest_neighbour() {
        let mut idx = HnswVectorIndex::open();
        let mut s: u64 = 0x1234_5678;
        let mut gen = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f32 / (1u64 << 31) as f32) - 1.0
        };
        let mut corpus: Vec<(String, Vec<f32>)> = Vec::new();
        for i in 0..200 {
            let v: Vec<f32> = (0..8).map(|_| gen()).collect();
            corpus.push((format!("d{i}"), v.clone()));
            idx.upsert(rec(&format!("d{i}"), v, "m@1")).unwrap();
        }

        let brute_nn = |q: &[f32]| -> String {
            let qe = Embedding(q.to_vec());
            corpus
                .iter()
                .min_by(|a, b| {
                    let da = Embedding(a.1.clone()).cosine_distance(&qe);
                    let db = Embedding(b.1.clone()).cosine_distance(&qe);
                    da.partial_cmp(&db).unwrap()
                })
                .map(|(id, _)| id.clone())
                .unwrap()
        };

        let mut correct = 0;
        let trials = 20;
        for i in 0..trials {
            let base = &corpus[i * 9 % corpus.len()].1;
            let q: Vec<f32> = base.iter().map(|x| x + 0.001).collect();
            let truth = brute_nn(&q);
            let hits = idx.knn(&Embedding(q), 1);
            if hits.first().map(|h| h.doc_id.as_str()) == Some(truth.as_str()) {
                correct += 1;
            }
        }
        assert!(
            correct >= trials - 2,
            "HNSW recall@1 must be near-perfect on this corpus, got {correct}/{trials}"
        );
    }

    #[test]
    fn the_graph_is_multi_layer() {
        let mut idx = HnswVectorIndex::open();
        for i in 0..300 {
            let v = vec![(i as f32).sin(), (i as f32).cos()];
            idx.upsert(rec(&format!("d{i}"), v, "m@1")).unwrap();
        }
        let entry = idx.entry.expect("non-empty");
        assert!(
            idx.nodes[entry].neighbours.len() > 1,
            "the graph has express lanes (the entry node spans multiple layers) - not flat"
        );
    }

    #[test]
    fn compaction_rebuild_is_deterministic() {
        let mut a = HnswVectorIndex::open();
        let mut b = HnswVectorIndex::open();
        for i in 0..20 {
            let v = vec![(i as f32).sin(), (i as f32).cos(), (i as f32 * 0.5).sin()];
            a.upsert(rec(&format!("d{i}"), v.clone(), "m@1")).unwrap();
            b.upsert(rec(&format!("d{i}"), v, "m@1")).unwrap();
        }
        a.compact();
        b.compact();
        let q = Embedding(vec![0.5, 0.5, 0.5]);
        let ha: Vec<String> = a.knn(&q, 5).into_iter().map(|h| h.doc_id).collect();
        let hb: Vec<String> = b.knn(&q, 5).into_iter().map(|h| h.doc_id).collect();
        assert_eq!(ha, hb, "deterministic graph ⇒ identical k-NN order");
    }
}
