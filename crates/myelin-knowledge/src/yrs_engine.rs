use crate::block_tree::BlockId;
use crate::transport::{DocOp, OpId, OpKind};
use yrs::updates::decoder::Decode;
use yrs::{
    Array, ArrayRef, Doc, GetString, Map, MapRef, ReadTxn, StateVector, Text, TextPrelim, TextRef,
    Transact, Update,
};

const BLOCKS_ROOT: &str = "blocks";
const CONTENT_ROOT: &str = "content";

const SEED_CLIENT_ID: u64 = 0;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DocSnapshot {
    pub blocks: Vec<(BlockId, String)>,
}

impl DocSnapshot {
    pub fn new() -> DocSnapshot {
        DocSnapshot::default()
    }

    pub fn push_block(&mut self, block_id: BlockId, inline: impl Into<String>) {
        self.blocks.push((block_id, inline.into()));
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

fn new_seed_doc() -> Doc {
    let mut options = yrs::Options::with_client_id(yrs::block::ClientID::new(SEED_CLIENT_ID));
    options.skip_gc = true;
    Doc::with_options(options)
}

pub struct YrsDoc {
    doc: Doc,
    blocks: ArrayRef,
    content: MapRef,
}

impl YrsDoc {
    pub fn seed_from_snapshot(snapshot: &DocSnapshot) -> YrsDoc {
        let doc = new_seed_doc();
        let blocks = doc.get_or_insert_array(BLOCKS_ROOT);
        let content = doc.get_or_insert_map(CONTENT_ROOT);
        {
            let mut txn = doc.transact_mut();
            for (block_id, inline) in &snapshot.blocks {
                blocks.push_back(&mut txn, block_id.as_str());
                content.insert(&mut txn, block_id.as_str(), TextPrelim::new(inline.clone()));
            }
        }
        YrsDoc {
            doc,
            blocks,
            content,
        }
    }

    pub fn from_state(bytes: &[u8]) -> Result<YrsDoc, YrsError> {
        let doc = Doc::new();
        let blocks = doc.get_or_insert_array(BLOCKS_ROOT);
        let content = doc.get_or_insert_map(CONTENT_ROOT);
        let me = YrsDoc {
            doc,
            blocks,
            content,
        };
        me.apply_update(bytes)?;
        Ok(me)
    }

    pub fn encode_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&StateVector::default())
    }

    pub fn apply_update(&self, bytes: &[u8]) -> Result<(), YrsError> {
        let update = Update::decode_v1(bytes).map_err(|_| YrsError::MalformedUpdate)?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|_| YrsError::MalformedUpdate)
    }

    pub fn encode_diff(&self, since: &StateVector) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_diff_v1(since)
    }

    pub fn state_vector(&self) -> StateVector {
        self.doc.transact().state_vector()
    }

    pub fn edit_block_text(
        &self,
        block_id: &BlockId,
        index: u32,
        chunk: &str,
    ) -> Result<Vec<u8>, YrsError> {
        let text = self.block_text(block_id)?;
        let before = self.state_vector();
        {
            let mut txn = self.doc.transact_mut();
            text.insert(&mut txn, index, chunk);
        }
        Ok(self.encode_diff(&before))
    }

    pub fn insert_block(
        &self,
        block_id: &BlockId,
        index: u32,
        inline: &str,
    ) -> Result<Vec<u8>, YrsError> {
        let before = self.state_vector();
        {
            let mut txn = self.doc.transact_mut();
            let len = self.blocks.len(&txn);
            let at = index.min(len);
            self.blocks.insert(&mut txn, at, block_id.as_str());
            self.content.insert(
                &mut txn,
                block_id.as_str(),
                TextPrelim::new(inline.to_string()),
            );
        }
        Ok(self.encode_diff(&before))
    }

    pub fn move_block(&self, block_id: &BlockId, to_index: u32) -> Result<Vec<u8>, YrsError> {
        let before = self.state_vector();
        let from = self
            .block_ordinal(block_id)
            .ok_or_else(|| YrsError::NoSuchBlock(block_id.clone()))?;
        {
            let mut txn = self.doc.transact_mut();
            self.blocks.remove(&mut txn, from);
            let len = self.blocks.len(&txn);
            let at = to_index.min(len);
            self.blocks.insert(&mut txn, at, block_id.as_str());
        }
        Ok(self.encode_diff(&before))
    }

    fn block_ordinal(&self, block_id: &BlockId) -> Option<u32> {
        let txn = self.doc.transact();
        for (i, out) in self.blocks.iter(&txn).enumerate() {
            if let yrs::Out::Any(yrs::Any::String(s)) = out {
                if s.as_ref() == block_id.as_str() {
                    return Some(i as u32);
                }
            }
        }
        None
    }

    fn block_text(&self, block_id: &BlockId) -> Result<TextRef, YrsError> {
        let txn = self.doc.transact();
        match self.content.get(&txn, block_id.as_str()) {
            Some(yrs::Out::YText(t)) => Ok(t),
            _ => Err(YrsError::NoSuchBlock(block_id.clone())),
        }
    }

    pub fn block_content(&self, block_id: &BlockId) -> Result<String, YrsError> {
        let text = self.block_text(block_id)?;
        let txn = self.doc.transact();
        Ok(text.get_string(&txn))
    }

    pub fn block_order(&self) -> Vec<BlockId> {
        let txn = self.doc.transact();
        self.blocks
            .iter(&txn)
            .filter_map(|out| match out {
                yrs::Out::Any(yrs::Any::String(s)) => Some(BlockId(s.to_string())),
                _ => None,
            })
            .collect()
    }

    pub fn derived_order_keys(&self) -> Vec<(BlockId, u64)> {
        self.block_order()
            .into_iter()
            .enumerate()
            .map(|(i, b)| (b, i as u64))
            .collect()
    }

    pub fn merge_peer(&self, peer: &YrsDoc) -> Result<(), YrsError> {
        self.apply_update(&peer.encode_state())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YrsError {
    NoSuchBlock(BlockId),
    MalformedUpdate,
}

impl std::fmt::Display for YrsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            YrsError::NoSuchBlock(b) => write!(f, "no Yrs content for block {}", b.as_str()),
            YrsError::MalformedUpdate => write!(f, "malformed Yrs update bytes"),
        }
    }
}

impl std::error::Error for YrsError {}

#[derive(Clone, Debug)]
pub struct EnginePromotion {
    snapshot: DocSnapshot,
    seed_bytes: Vec<u8>,
    cutover_seq: u64,
}

impl EnginePromotion {
    pub fn new(snapshot: DocSnapshot, head_seq: u64) -> EnginePromotion {
        let seed = YrsDoc::seed_from_snapshot(&snapshot);
        let seed_bytes = seed.encode_state();
        EnginePromotion {
            snapshot,
            seed_bytes,
            cutover_seq: head_seq + 1,
        }
    }

    pub fn cutover_seq(&self) -> u64 {
        self.cutover_seq
    }

    pub fn seed_bytes(&self) -> &[u8] {
        &self.seed_bytes
    }

    pub fn snapshot(&self) -> &DocSnapshot {
        &self.snapshot
    }

    pub fn cutover_op(&self) -> DocOp {
        DocOp::cas(
            OpId::new("server", self.cutover_seq),
            "actor-server",
            OpKind::EnginePromote,
            self.seed_bytes.clone(),
        )
    }

    pub fn seeded_doc(&self) -> YrsDoc {
        YrsDoc::seed_from_snapshot(&self.snapshot)
    }

    pub fn reconcile_inflight_cas(
        &self,
        doc: &YrsDoc,
        inflight: &[(BlockId, u32, String)],
    ) -> Vec<Result<Vec<u8>, YrsError>> {
        inflight
            .iter()
            .map(|(block_id, index, chunk)| doc.edit_block_text(block_id, *index, chunk))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bid(s: &str) -> BlockId {
        BlockId(s.to_string())
    }

    fn snapshot() -> DocSnapshot {
        let mut s = DocSnapshot::new();
        s.push_block(bid("b1"), "hello");
        s.push_block(bid("b2"), "world");
        s
    }

    #[test]
    fn seed_is_deterministic_byte_identical() {
        let snap = snapshot();
        let a = YrsDoc::seed_from_snapshot(&snap).encode_state();
        let b = YrsDoc::seed_from_snapshot(&snap).encode_state();
        assert_eq!(
            a, b,
            "the same snapshot seeds byte-identical Yrs update bytes"
        );
        assert!(!a.is_empty(), "the seed is non-empty for a non-empty doc");
    }

    #[test]
    fn seed_materialises_snapshot_faithfully() {
        let doc = YrsDoc::seed_from_snapshot(&snapshot());
        assert_eq!(
            doc.block_order(),
            vec![bid("b1"), bid("b2")],
            "the move-CRDT list reproduces the sibling order"
        );
        assert_eq!(doc.block_content(&bid("b1")).unwrap(), "hello");
        assert_eq!(doc.block_content(&bid("b2")).unwrap(), "world");
    }

    #[test]
    fn block_order_is_load_bearing_in_the_seed() {
        let mut reordered = DocSnapshot::new();
        reordered.push_block(bid("b2"), "world");
        reordered.push_block(bid("b1"), "hello");
        let doc = YrsDoc::seed_from_snapshot(&reordered);
        assert_eq!(
            doc.block_order(),
            vec![bid("b2"), bid("b1")],
            "the seed honours the snapshot's sibling order"
        );
    }

    #[test]
    fn concurrent_same_block_edits_converge_no_blend_lost() {
        let snap = {
            let mut s = DocSnapshot::new();
            s.push_block(bid("b1"), "");
            s
        };
        let seed = YrsDoc::seed_from_snapshot(&snap).encode_state();
        let a = YrsDoc::from_state(&seed).unwrap();
        let b = YrsDoc::from_state(&seed).unwrap();

        let ua = a.edit_block_text(&bid("b1"), 0, "AAA").unwrap();
        let ub = b.edit_block_text(&bid("b1"), 0, "BBB").unwrap();

        a.apply_update(&ub).unwrap();
        b.apply_update(&ua).unwrap();

        let ca = a.block_content(&bid("b1")).unwrap();
        let cb = b.block_content(&bid("b1")).unwrap();
        assert_eq!(ca, cb, "the two replicas converge to one state");
        assert!(
            ca.contains("AAA") && ca.contains("BBB"),
            "both edits survived: {ca}"
        );
        assert_eq!(ca.len(), 6, "exactly both inserts, no duplication");
    }

    #[test]
    fn n_client_same_block_edits_converge() {
        let snap = {
            let mut s = DocSnapshot::new();
            s.push_block(bid("b1"), "");
            s
        };
        let seed = YrsDoc::seed_from_snapshot(&snap).encode_state();
        let replicas: Vec<YrsDoc> = (0..4).map(|_| YrsDoc::from_state(&seed).unwrap()).collect();

        let updates: Vec<Vec<u8>> = replicas
            .iter()
            .enumerate()
            .map(|(i, r)| r.edit_block_text(&bid("b1"), 0, &format!("<{i}>")).unwrap())
            .collect();

        for r in &replicas {
            for u in &updates {
                r.apply_update(u).unwrap();
            }
        }

        let states: Vec<String> = replicas
            .iter()
            .map(|r| r.block_content(&bid("b1")).unwrap())
            .collect();
        let first = &states[0];
        for s in &states {
            assert_eq!(s, first, "all replicas converge to ONE identical state");
        }
        for i in 0..4 {
            assert!(
                first.contains(&format!("<{i}>")),
                "replica {i}'s edit survived: {first}"
            );
        }
    }

    #[test]
    fn applying_an_update_twice_is_idempotent() {
        let doc = YrsDoc::seed_from_snapshot(&snapshot());
        let u = doc.edit_block_text(&bid("b1"), 5, "!!").unwrap();
        let peer = YrsDoc::seed_from_snapshot(&snapshot());
        peer.apply_update(&u).unwrap();
        let once = peer.block_content(&bid("b1")).unwrap();
        peer.apply_update(&u).unwrap();
        let twice = peer.block_content(&bid("b1")).unwrap();
        assert_eq!(
            once, twice,
            "re-applying the same update is a no-op (idempotent)"
        );
        assert_eq!(twice, "hello!!");
    }

    #[test]
    fn move_crdt_owns_ordering_order_key_derived() {
        let mut snap = DocSnapshot::new();
        snap.push_block(bid("b1"), "one");
        snap.push_block(bid("b2"), "two");
        snap.push_block(bid("b3"), "three");
        let doc = YrsDoc::seed_from_snapshot(&snap);
        doc.move_block(&bid("b3"), 0).unwrap();
        assert_eq!(
            doc.block_order(),
            vec![bid("b3"), bid("b1"), bid("b2")],
            "the move-CRDT list owns the new order"
        );
        let keys = doc.derived_order_keys();
        assert_eq!(
            keys,
            vec![(bid("b3"), 0), (bid("b1"), 1), (bid("b2"), 2)],
            "order_key is a derived hint from CRDT state, not a bespoke LexoRank jitter"
        );
    }

    #[test]
    fn concurrent_moves_converge() {
        let mut snap = DocSnapshot::new();
        for n in ["b1", "b2", "b3", "b4"] {
            snap.push_block(bid(n), n);
        }
        let seed = YrsDoc::seed_from_snapshot(&snap).encode_state();
        let a = YrsDoc::from_state(&seed).unwrap();
        let b = YrsDoc::from_state(&seed).unwrap();
        let ua = a.move_block(&bid("b4"), 0).unwrap();
        let ub = b.move_block(&bid("b1"), 3).unwrap();
        a.apply_update(&ub).unwrap();
        b.apply_update(&ua).unwrap();
        assert_eq!(
            a.block_order(),
            b.block_order(),
            "concurrent moves converge to one ordering"
        );
        assert_eq!(
            a.block_order().len(),
            4,
            "no block lost in concurrent moves"
        );
    }

    #[test]
    fn cutover_is_at_head_plus_one() {
        let promo = EnginePromotion::new(snapshot(), 7);
        assert_eq!(promo.cutover_seq(), 8, "the cutover op_seq is head + 1");
        let op = promo.cutover_op();
        assert_eq!(
            op.kind,
            OpKind::EnginePromote,
            "it is the engine_promote op"
        );
        assert_eq!(
            op.payload,
            promo.seed_bytes(),
            "it carries the deterministic seed bytes"
        );
    }

    #[test]
    fn promotion_retains_reversibility_snapshot() {
        let snap = snapshot();
        let promo = EnginePromotion::new(snap.clone(), 3);
        assert_eq!(
            promo.snapshot(),
            &snap,
            "the pre-cutover snapshot is retained for rollback"
        );
        let doc = promo.seeded_doc();
        assert_eq!(doc.block_order(), vec![bid("b1"), bid("b2")]);
    }

    #[test]
    fn inflight_cas_edits_reconcile_across_cutover() {
        let promo = EnginePromotion::new(snapshot(), 0);
        let doc = promo.seeded_doc();
        let results = promo.reconcile_inflight_cas(&doc, &[(bid("b1"), 5, "!".to_string())]);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_ok(),
            "the in-flight CAS edit reconciled (not dropped)"
        );
        assert_eq!(
            doc.block_content(&bid("b1")).unwrap(),
            "hello!",
            "the in-flight edit survived the swap, merged into the CRDT"
        );
    }

    #[test]
    fn inflight_edit_to_missing_block_errors_loudly() {
        let promo = EnginePromotion::new(snapshot(), 0);
        let doc = promo.seeded_doc();
        let results = promo.reconcile_inflight_cas(&doc, &[(bid("ghost"), 0, "x".to_string())]);
        assert_eq!(results[0], Err(YrsError::NoSuchBlock(bid("ghost"))));
    }

    #[test]
    fn state_round_trips_through_update_bytes() {
        let doc = YrsDoc::seed_from_snapshot(&snapshot());
        doc.edit_block_text(&bid("b2"), 5, "!").unwrap();
        let bytes = doc.encode_state();
        let loaded = YrsDoc::from_state(&bytes).unwrap();
        assert_eq!(loaded.block_order(), doc.block_order());
        assert_eq!(loaded.block_content(&bid("b2")).unwrap(), "world!");
    }

    #[test]
    fn malformed_update_bytes_error_loudly() {
        let doc = YrsDoc::seed_from_snapshot(&snapshot());
        assert_eq!(
            doc.apply_update(&[0xff, 0xff, 0xff, 0xff]),
            Err(YrsError::MalformedUpdate)
        );
    }

    #[test]
    fn merge_peer_converges_replicas() {
        let seed = YrsDoc::seed_from_snapshot(&snapshot()).encode_state();
        let doc_a = YrsDoc::from_state(&seed).unwrap();
        let doc_b = YrsDoc::from_state(&seed).unwrap();
        doc_a.edit_block_text(&bid("b1"), 5, " from A").unwrap();
        doc_b.edit_block_text(&bid("b2"), 5, " from B").unwrap();
        doc_a.merge_peer(&doc_b).unwrap();
        doc_b.merge_peer(&doc_a).unwrap();
        assert_eq!(
            doc_a.block_content(&bid("b1")).unwrap(),
            doc_b.block_content(&bid("b1")).unwrap()
        );
        assert_eq!(
            doc_a.block_content(&bid("b2")).unwrap(),
            doc_b.block_content(&bid("b2")).unwrap()
        );
        assert_eq!(doc_a.block_content(&bid("b1")).unwrap(), "hello from A");
        assert_eq!(doc_a.block_content(&bid("b2")).unwrap(), "world from B");
    }
}
