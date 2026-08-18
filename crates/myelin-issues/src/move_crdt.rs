use crate::reorder::RankedIssue;
use myelin_query::field::OrderKey;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use yrs::updates::decoder::Decode;
use yrs::{Array, ArrayRef, Doc, ReadTxn, StateVector, Transact, Update};

const ITEMS_ROOT: &str = "items";

const SEED_CLIENT_ID: u64 = 0;

#[derive(Debug)]
pub struct ReorderPressure {
    attempts: AtomicU64,
    rebases: AtomicU64,
}

impl ReorderPressure {
    pub const PROMOTE_THRESHOLD: f64 = 0.25;

    pub const MIN_ATTEMPTS: u64 = 8;

    #[must_use]
    pub fn new() -> ReorderPressure {
        ReorderPressure::default()
    }

    pub fn observe_cas_outcome(&self, lost_cas: bool) {
        self.attempts.fetch_add(1, AtomicOrdering::SeqCst);
        if lost_cas {
            self.rebases.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    #[must_use]
    pub fn attempts(&self) -> u64 {
        self.attempts.load(AtomicOrdering::SeqCst)
    }

    #[must_use]
    pub fn rebases(&self) -> u64 {
        self.rebases.load(AtomicOrdering::SeqCst)
    }

    #[must_use]
    pub fn rebase_rate(&self) -> f64 {
        let attempts = self.attempts();
        if attempts == 0 {
            0.0
        } else {
            self.rebases() as f64 / attempts as f64
        }
    }

    #[must_use]
    pub fn should_promote(&self) -> bool {
        self.attempts() >= Self::MIN_ATTEMPTS && self.rebase_rate() >= Self::PROMOTE_THRESHOLD
    }
}

fn new_seed_doc() -> Doc {
    let mut options = yrs::Options::with_client_id(yrs::block::ClientID::new(SEED_CLIENT_ID));
    options.skip_gc = true;
    Doc::with_options(options)
}

pub struct MoveCrdtBoard {
    doc: Doc,
    items: ArrayRef,
}

impl MoveCrdtBoard {
    #[must_use]
    pub fn seed_from_order(seed_order: &[String]) -> MoveCrdtBoard {
        let doc = new_seed_doc();
        let items = doc.get_or_insert_array(ITEMS_ROOT);
        {
            let mut txn = doc.transact_mut();
            for issue_id in seed_order {
                items.push_back(&mut txn, issue_id.as_str());
            }
        }
        MoveCrdtBoard { doc, items }
    }

    pub fn from_state(bytes: &[u8]) -> Result<MoveCrdtBoard, MoveCrdtError> {
        let doc = Doc::new();
        let items = doc.get_or_insert_array(ITEMS_ROOT);
        let me = MoveCrdtBoard { doc, items };
        me.apply_update(bytes)?;
        Ok(me)
    }

    #[must_use]
    pub fn encode_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&StateVector::default())
    }

    #[must_use]
    pub fn encode_diff(&self, since: &StateVector) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_diff_v1(since)
    }

    #[must_use]
    pub fn state_vector(&self) -> StateVector {
        self.doc.transact().state_vector()
    }

    pub fn apply_update(&self, bytes: &[u8]) -> Result<(), MoveCrdtError> {
        let update = Update::decode_v1(bytes).map_err(|_| MoveCrdtError::MalformedUpdate)?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|_| MoveCrdtError::MalformedUpdate)
    }

    pub fn move_issue(&self, issue_id: &str, to_index: u32) -> Result<Vec<u8>, MoveCrdtError> {
        let before = self.state_vector();
        let from = self
            .ordinal(issue_id)
            .ok_or_else(|| MoveCrdtError::NoSuchIssue(issue_id.to_string()))?;
        {
            let mut txn = self.doc.transact_mut();
            self.items.remove(&mut txn, from);
            let len = self.items.len(&txn);
            let at = to_index.min(len);
            self.items.insert(&mut txn, at, issue_id);
        }
        Ok(self.encode_diff(&before))
    }

    pub fn insert_issue(&self, issue_id: &str, at_index: u32) -> Result<Vec<u8>, MoveCrdtError> {
        let before = self.state_vector();
        {
            let mut txn = self.doc.transact_mut();
            let len = self.items.len(&txn);
            let at = at_index.min(len);
            self.items.insert(&mut txn, at, issue_id);
        }
        Ok(self.encode_diff(&before))
    }

    fn ordinal(&self, issue_id: &str) -> Option<u32> {
        let txn = self.doc.transact();
        for (i, out) in self.items.iter(&txn).enumerate() {
            if let yrs::Out::Any(yrs::Any::String(s)) = out {
                if s.as_ref() == issue_id {
                    return Some(i as u32);
                }
            }
        }
        None
    }

    #[must_use]
    pub fn order(&self) -> Vec<String> {
        let txn = self.doc.transact();
        self.items
            .iter(&txn)
            .filter_map(|out| match out {
                yrs::Out::Any(yrs::Any::String(s)) => Some(s.to_string()),
                _ => None,
            })
            .collect()
    }

    #[must_use]
    pub fn derived_order_keys(&self) -> Vec<(String, OrderKey)> {
        let mut out = Vec::new();
        let mut prev: Option<OrderKey> = None;
        for issue_id in self.order() {
            let key = match &prev {
                None => OrderKey::rank_first(myelin_query::field::Jitter::ZERO),
                Some(p) => OrderKey::rank_last(Some(p), myelin_query::field::Jitter::ZERO),
            };
            prev = Some(key.clone());
            out.push((issue_id, key));
        }
        out
    }

    #[must_use]
    pub fn derived_ranked(
        &self,
        meta: &impl Fn(&str) -> Option<(String, String)>,
    ) -> Vec<RankedIssue> {
        self.derived_order_keys()
            .into_iter()
            .filter_map(|(issue_id, order_key)| {
                let (created_at, ulid) = meta(&issue_id)?;
                Some(RankedIssue {
                    issue_id,
                    order_key,
                    version: 0,
                    created_at,
                    ulid,
                })
            })
            .collect()
    }

    pub fn merge_peer(&self, peer: &MoveCrdtBoard) -> Result<(), MoveCrdtError> {
        self.apply_update(&peer.encode_state())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        let txn = self.doc.transact();
        self.items.len(&txn) as usize
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveCrdtError {
    NoSuchIssue(String),
    MalformedUpdate,
}

impl std::fmt::Display for MoveCrdtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoveCrdtError::NoSuchIssue(id) => write!(f, "move-CRDT: unknown issue `{id}`"),
            MoveCrdtError::MalformedUpdate => write!(f, "move-CRDT: malformed update bytes"),
        }
    }
}

impl std::error::Error for MoveCrdtError {}


impl Default for ReorderPressure {
    fn default() -> Self {
        ReorderPressure {
            attempts: AtomicU64::new(0),
            rebases: AtomicU64::new(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorder_pressure_promotes_only_on_the_measured_trigger() {
        let p = ReorderPressure::new();
        for _ in 0..(ReorderPressure::MIN_ATTEMPTS - 1) {
            p.observe_cas_outcome(true);
        }
        assert!(
            !p.should_promote(),
            "below MIN_ATTEMPTS the floor stands (the rate is not trusted)"
        );

        let calm = ReorderPressure::new();
        for i in 0..40 {
            calm.observe_cas_outcome(i % 20 == 0);
        }
        assert!(calm.rebase_rate() < ReorderPressure::PROMOTE_THRESHOLD);
        assert!(
            !calm.should_promote(),
            "a calm board stays on the CAS floor (no premature promotion)"
        );

        let hot = ReorderPressure::new();
        for i in 0..40 {
            hot.observe_cas_outcome(i % 2 == 0);
        }
        assert!(hot.rebase_rate() >= ReorderPressure::PROMOTE_THRESHOLD);
        assert!(
            hot.should_promote(),
            "a measured concurrent-reorder-pain board promotes to the move-CRDT"
        );
    }

    #[test]
    fn derived_order_keys_preserve_the_displayed_order_unchanged_data_model() {
        let seed = vec![
            "I0".to_string(),
            "I1".to_string(),
            "I2".to_string(),
            "I3".to_string(),
        ];
        let board = MoveCrdtBoard::seed_from_order(&seed);
        let derived = board.derived_order_keys();
        let ids: Vec<String> = derived.iter().map(|(i, _)| i.clone()).collect();
        assert_eq!(
            ids, seed,
            "the derived order is the seed order (unchanged model)"
        );
        for w in derived.windows(2) {
            assert!(
                w[0].1 < w[1].1,
                "derived order_key hints are strictly increasing (byte-identical codec)"
            );
        }
    }

    #[test]
    fn two_concurrent_distinct_moves_converge_both_survive() {
        let seed = vec![
            "I0".to_string(),
            "I1".to_string(),
            "I2".to_string(),
            "I3".to_string(),
        ];
        let a = MoveCrdtBoard::seed_from_order(&seed);
        let b = MoveCrdtBoard::from_state(&a.encode_state()).expect("b seeds from a");

        a.move_issue("I3", 0).expect("A moves I3 to front");
        b.move_issue("I0", 4).expect("B moves I0 to back");

        a.merge_peer(&b).expect("A merges B");
        b.merge_peer(&a).expect("B merges A");

        assert_eq!(a.order(), b.order(), "the two replicas converge");
        let order = a.order();
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(
            pos("I3") < pos("I0"),
            "I3 moved to front, I0 moved to back - both survive"
        );
    }

    #[test]
    fn merge_is_idempotent() {
        let seed = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let x = MoveCrdtBoard::seed_from_order(&seed);
        let y = MoveCrdtBoard::from_state(&x.encode_state()).unwrap();
        let upd = y.move_issue("C", 0).unwrap();
        x.apply_update(&upd).unwrap();
        let once = x.order();
        x.apply_update(&upd).unwrap();
        assert_eq!(
            x.order(),
            once,
            "re-applying a move is a no-op (idempotent merge)"
        );
    }

    #[test]
    fn malformed_update_is_a_loud_error() {
        let board = MoveCrdtBoard::seed_from_order(&["A".to_string()]);
        let err = board.apply_update(&[0xde, 0xad, 0xbe, 0xef]).unwrap_err();
        assert_eq!(err, MoveCrdtError::MalformedUpdate);
    }

    #[test]
    fn move_unknown_issue_is_a_loud_error() {
        let board = MoveCrdtBoard::seed_from_order(&["A".to_string()]);
        let err = board.move_issue("ghost", 0).unwrap_err();
        assert_eq!(err, MoveCrdtError::NoSuchIssue("ghost".to_string()));
    }

    #[test]
    fn seed_is_deterministic_replayable_cutover() {
        let seed = vec!["X".to_string(), "Y".to_string(), "Z".to_string()];
        let a = MoveCrdtBoard::seed_from_order(&seed);
        let b = MoveCrdtBoard::seed_from_order(&seed);
        assert_eq!(
            a.encode_state(),
            b.encode_state(),
            "the same seed order yields byte-identical CRDT bytes (replayable cutover)"
        );
    }
}
