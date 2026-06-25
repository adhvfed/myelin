//! # `move_crdt` — the measured move-CRDT promotion of the `order_key` CAS floor (ISS-P32 / P-495, M5)
//!
//! **The R-3 floor follow-on — promoted on a MEASURED trigger, never premature (VISION §3 /
//! external-insights/01 §7 / external-insights/04 §2 "CRDT-after-CAS").** The conflict engine the
//! [`crate::reorder`] floor ships is the **server-arbitrated CAS** (the loser re-bases honestly). This
//! module ships the **move-CRDT** — the named M5 follow-on — as a convergent Yrs list over the
//! **byte-identical** [`OrderKey`]: a board's sibling ordering is a `yrs::Array` of issue ids (a
//! convergent list, the cited structure — Yrs/Yjs, VISION §4, the SAME `yrs` crate Knowledge's
//! `yrs_engine` reuses for its block move-CRDT, KN-P29 / P-484). A move is a convergent remove+insert
//! in that list, so two concurrent reorders of DIFFERENT issues both survive (no loser, no clobber);
//! the `order_key` becomes a **derived OLTP index hint** recomputed from the convergent list order
//! ([`MoveCrdtBoard::derived_order_keys`]), NOT a bespoke LexoRank jitter the writers contend on.
//!
//! ## The promotion swaps the conflict ENGINE, not the data model (arch §5 "Floor → follow-on")
//! Because the frozen `order_key` is already byte-identical (contract 13.3, co-owned with Knowledge),
//! the promotion is a Layer-3 ENGINE swap, exactly like the Knowledge CAS→Yrs promotion (KN-P29):
//! - the **data model is unchanged** — the displayed order is still `(order_key, created_at, ulid)`
//!   ([`crate::reorder::RankedIssue`]); the CRDT derives the `order_key` index hints from its list
//!   order ([`MoveCrdtBoard::derived_ranked`]), it does not introduce a second ordering token;
//! - the **transport is unchanged** — a move still co-commits an `issue.reordered` event over the ONE
//!   shared outbox (the [`crate::reorder`] emit path is reused verbatim by the live binding; this
//!   module owns the convergent ENGINE, the emit seam is the floor's);
//! - the resume-cursor firehose (contract 3.5) is the SAME — the CRDT update bytes ride
//!   [`MoveCrdtBoard::encode_state`] / [`MoveCrdtBoard::apply_update`] exactly as the CAS deltas did.
//!
//! ## The MEASURED trigger ([`ReorderPressure`]) — the floor stands until the signal fires (VISION §3)
//! The promotion is **NOT** speculative. [`ReorderPressure`] is the measurement seam: it counts the
//! CAS re-base churn on a board (the loser-re-bases the CAS floor produces under concurrent same-region
//! drag). The move-CRDT is promoted for a board ONLY once its measured re-base rate crosses the named
//! trigger ([`ReorderPressure::PROMOTE_THRESHOLD`] — the default-to-beat, calibrated by the
//! at-scale surge family ISS-P33). Below the trigger the CAS floor is the complete, correct engine
//! (it is cheaper — no per-board CRDT doc); above it, the convergent engine eliminates the re-base
//! churn. This is the doctrine's measured-promotion: ship the seam + the measurement, promote on the
//! signal.
//!
//! ## ISS-D5 re-greens ACROSS the engine-promote boundary (the GATE — TESTS field)
//! The drill catalogue row ISS-D5 (0 silent clobber) was written to SURVIVE the swap. This module's
//! tests + `tests/drill_iss_d5_move_crdt_re_green.rs` re-run the ISS-D5 scenario THROUGH the move-CRDT
//! engine and assert the SAME 0-clobber property — now STRONGER: two concurrent distinct-issue moves
//! into the same region BOTH survive convergently (the CAS floor accepted them serially; the CRDT
//! merges them with no serialisation), and no reorder is lost. The order_key data model is byte-for-
//! byte the same across the boundary (the derived hints sort identically to the CAS ranks).
//!
//! ## MANDATORY-CORE MUTATION FLOOR (the ISS-P32 cargo-mutants gate — TESTS field)
//! The move-CRDT conflict engine is correctness-bearing → mandatory-core when promoted. The stated
//! floor: **100% mutation score on the convergence + derived-order path** ([`MoveCrdtBoard::move_issue`]
//! / [`MoveCrdtBoard::merge_peer`] / [`MoveCrdtBoard::derived_order_keys`]) — a mutated list ordinal, a
//! dropped merge, or an off-by-one derived hint flips the convergence assertion (two replicas diverge)
//! or the across-boundary ISS-D5 0-clobber assertion. The convergence property itself is proven by Yrs
//! (the cited structure, VISION §4), not re-derived; the gate is that OUR move + derived-order are
//! convergent + continuous. Run: `cargo mutants -p myelin-issues -f move_crdt.rs`.

use crate::reorder::RankedIssue;
use myelin_query::field::OrderKey;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use yrs::updates::decoder::Decode;
use yrs::{Array, ArrayRef, Doc, ReadTxn, StateVector, Transact, Update};

/// The fixed root name of the **move-CRDT sibling list** — a convergent `yrs::Array` of issue ids, one
/// id at each ordinal. A reorder is a convergent remove+insert in THIS list (§5 "Floor → follow-on").
const ITEMS_ROOT: &str = "items";

/// **The fixed `client_id` of the DETERMINISTIC seed doc** (mirrors the Knowledge `yrs_engine`
/// SEED_CLIENT_ID, §3.4 step 2 — reproducible + replay-safe). The seed Yrs bytes MUST be a pure
/// function of the seeding board order so the `engine_promote` cutover is replayable: a fixed
/// `client_id` (the server is the seeding authority) + GC off makes the encoded seed bytes
/// byte-identical for the same board. Live replica edits AFTER the seed use their own `client_id`.
const SEED_CLIENT_ID: u64 = 0;

// ===========================================================================
// §1 — the MEASURED promotion trigger (the floor stands until the signal fires)
// ===========================================================================

/// **The MEASURED concurrent-reorder pressure trigger (VISION §3 — promote on a measured signal,
/// never premature).** The [`crate::reorder`] CAS floor produces re-base churn under concurrent
/// same-region drag: a loser is returned the authoritative order and re-bases. This counter MEASURES
/// that churn per board; the move-CRDT is promoted for a board ONLY once the measured re-base rate
/// crosses [`Self::PROMOTE_THRESHOLD`]. Below the trigger the CAS floor is the complete, correct,
/// cheaper engine — there is NO per-board CRDT doc until the measurement says it is worth it.
///
/// This is the seam, not the migration: a deployment wires [`Self::observe_cas_outcome`] into the
/// reorder path and reads [`Self::should_promote`] to decide, per board, whether to seed the CRDT.
#[derive(Debug)]
pub struct ReorderPressure {
    /// Total reorder attempts observed on this board (CAS wins + CAS losses).
    attempts: AtomicU64,
    /// CAS losses observed (a loser was returned the authoritative order to re-base) — the churn the
    /// move-CRDT eliminates by merging instead of serialising.
    rebases: AtomicU64,
}

impl ReorderPressure {
    /// **The named promotion trigger (the default-to-beat, calibrated by the at-scale surge family
    /// ISS-P33).** The move-CRDT is promoted for a board once at least [`Self::MIN_ATTEMPTS`] reorders
    /// have been observed AND the measured re-base rate (`rebases / attempts`) is at or above this
    /// fraction. `0.25` = a quarter of reorders losing their CAS is the measured "concurrent-reorder
    /// pain" the doctrine names as the move-CRDT trigger. Below it, the CAS floor stands.
    pub const PROMOTE_THRESHOLD: f64 = 0.25;

    /// The minimum sample size before the rate is trusted (a single contended drag must not promote a
    /// whole board — the trigger is a MEASURED rate, not a one-off).
    pub const MIN_ATTEMPTS: u64 = 8;

    /// A fresh pressure meter for a board (no attempts observed — the CAS floor is the engine).
    #[must_use]
    pub fn new() -> ReorderPressure {
        ReorderPressure::default()
    }

    /// **Observe one reorder outcome (the measurement seam).** `lost_cas` is `true` iff the reorder
    /// LOST its CAS and re-based (the churn signal). Wired into the reorder path so the measured rate
    /// is live; never an estimate.
    pub fn observe_cas_outcome(&self, lost_cas: bool) {
        self.attempts.fetch_add(1, AtomicOrdering::SeqCst);
        if lost_cas {
            self.rebases.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    /// The total reorder attempts observed (CAS wins + losses).
    #[must_use]
    pub fn attempts(&self) -> u64 {
        self.attempts.load(AtomicOrdering::SeqCst)
    }

    /// The CAS re-bases observed (the churn the move-CRDT eliminates).
    #[must_use]
    pub fn rebases(&self) -> u64 {
        self.rebases.load(AtomicOrdering::SeqCst)
    }

    /// The measured re-base RATE (`rebases / attempts`), `0.0` when no attempts have been observed.
    #[must_use]
    pub fn rebase_rate(&self) -> f64 {
        let attempts = self.attempts();
        if attempts == 0 {
            0.0
        } else {
            self.rebases() as f64 / attempts as f64
        }
    }

    /// **The promotion decision (VISION §3 — promote on the MEASURED signal).** `true` iff the board
    /// has crossed the named trigger: at least [`Self::MIN_ATTEMPTS`] reorders AND a re-base rate at or
    /// above [`Self::PROMOTE_THRESHOLD`]. Until this fires the CAS floor is the engine (the floor stands
    /// until its measured signal fires — it is not promoted speculatively).
    #[must_use]
    pub fn should_promote(&self) -> bool {
        self.attempts() >= Self::MIN_ATTEMPTS && self.rebase_rate() >= Self::PROMOTE_THRESHOLD
    }
}

// ===========================================================================
// §2 — the move-CRDT board (the convergent engine over the byte-identical order_key)
// ===========================================================================

/// Build a fresh Yrs `Doc` with DETERMINISTIC seed parameters (mirrors the Knowledge `yrs_engine`
/// seed doc): a fixed `client_id` + GC off, so the encoded seed bytes are a pure function of the
/// seeding board order (the replayable `engine_promote` cutover payload — §3.4 step 2).
fn new_seed_doc() -> Doc {
    let mut options = yrs::Options::with_client_id(yrs::block::ClientID::new(SEED_CLIENT_ID));
    options.skip_gc = true;
    Doc::with_options(options)
}

/// **The per-board move-CRDT engine (the R-3 promotion of the CAS floor — arch §5).** Wraps a
/// `yrs::Doc` whose `items` `Array` is the convergent sibling list of issue ids. A move is a
/// convergent remove+insert in that list; two concurrent moves of DIFFERENT issues both survive (no
/// loser). The `order_key` is DERIVED from the convergent list order ([`Self::derived_order_keys`]) —
/// the data model is unchanged across the engine swap (the displayed order is still the frozen
/// `(order_key, created_at, ulid)` tuple, recomputed from the CRDT list).
pub struct MoveCrdtBoard {
    doc: Doc,
    items: ArrayRef,
}

impl MoveCrdtBoard {
    /// **Seed the move-CRDT DETERMINISTICALLY from the current displayed board order (the
    /// `engine_promote` cutover — §3.4 step 2).** The `seed_order` is the CAS-era displayed order
    /// (issue ids in `(order_key, created_at, ulid)` sequence — exactly [`crate::reorder::BoardRanking::displayed_order`]
    /// materialised); each id is appended to the convergent `items` list at its ordinal. The SAME
    /// seed order always yields byte-identical [`Self::encode_state`] bytes (the fixed seed client_id +
    /// GC off), so the promotion cutover is replayable + reversible from the pre-cutover order.
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

    /// **Load a move-CRDT board from previously-[`Self::encode_state`]d bytes (the reconnect path
    /// across the `engine_promote` boundary — §3.4 step 3).** A replica resuming over the firehose
    /// loads the seeded CRDT state once from these bytes, then applies the live tail. Errors LOUDLY on
    /// malformed bytes (never a silent half-load — EI-01 §5).
    pub fn from_state(bytes: &[u8]) -> Result<MoveCrdtBoard, MoveCrdtError> {
        let doc = Doc::new();
        let items = doc.get_or_insert_array(ITEMS_ROOT);
        let me = MoveCrdtBoard { doc, items };
        me.apply_update(bytes)?;
        Ok(me)
    }

    /// **Encode the FULL CRDT state as Yrs update bytes (the seed / reconnect payload — §3.4).** This
    /// is the `engine_promote` cutover payload and the full-state a reconnecting replica loads once.
    /// For a board seeded via [`Self::seed_from_order`] these bytes are a pure function of the seed
    /// order (determinism — the replayable cutover).
    #[must_use]
    pub fn encode_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&StateVector::default())
    }

    /// **The DIFF a peer replica needs to catch up from its `state_vector` (the incremental update).**
    /// Only the ops the peer is missing — the bounded delta the resume-cursor firehose carries after
    /// the cutover (contract 3.5, the transport unchanged). Commutative + idempotent (see
    /// [`Self::apply_update`]).
    #[must_use]
    pub fn encode_diff(&self, since: &StateVector) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_diff_v1(since)
    }

    /// This replica's current state vector (what it has seen — the basis a peer diffs against).
    #[must_use]
    pub fn state_vector(&self) -> StateVector {
        self.doc.transact().state_vector()
    }

    /// **Apply a peer's Yrs UPDATE bytes idempotently (the merge — the convergence operation).** Yrs
    /// updates are commutative + idempotent: re-applying one is a no-op, and updates from different
    /// replicas merge to ONE convergent order regardless of arrival order. This is the at-least-once →
    /// effectively-once property at the MERGE layer (mirroring the transport's `UNIQUE(op_id)`). A
    /// corrupt payload is a LOUD error, never a silent drop.
    pub fn apply_update(&self, bytes: &[u8]) -> Result<(), MoveCrdtError> {
        let update = Update::decode_v1(bytes).map_err(|_| MoveCrdtError::MalformedUpdate)?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|_| MoveCrdtError::MalformedUpdate)
    }

    /// **Move an issue within the sibling list (the move-CRDT — the R-3 promotion's headline).** A
    /// convergent remove+insert in the `items` list. Two concurrent moves of DIFFERENT issues BOTH
    /// survive when the replicas exchange updates (no loser re-bases — the CAS floor's churn is gone).
    /// Returns the update bytes the move produced (the transport payload). An issue not on the board is
    /// a LOUD error.
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

    /// **Insert a NEW issue into the board at an ordinal (a create — the move-CRDT list grows).**
    /// Returns the update bytes.
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

    /// The current ordinal of an issue in the sibling list, if present (the move-CRDT ordinal).
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

    /// **The issue ids in convergent sibling order (the move-CRDT list materialised).** This is the
    /// order the OLTP `order_key` index hints are DERIVED from — the CRDT list is the source of truth
    /// for ordering, not a bespoke LexoRank jitter.
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

    /// **The DERIVED `order_key` hints recomputed from the convergent list order (arch §5 — the
    /// `order_key` becomes a derived OLTP-index hint, the bespoke jitter/rebalance retires).** Each
    /// issue gets a frozen [`OrderKey`] in list order, spaced evenly via the contract-13.3 codec
    /// (`rank_first` then `rank_last` after the previous) — the SAME byte-identical encoding the CAS
    /// floor wrote, so the displayed order is unchanged across the engine swap. The CRDT list is the
    /// source of truth; this is only the index hint the OLTP read path returns siblings in order by
    /// (recomputed, never authoritative).
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

    /// **The full displayed [`RankedIssue`] rows derived from the convergent order (the data model is
    /// UNCHANGED across the engine swap — arch §5).** Joins the derived `order_key` hints onto the
    /// per-issue `(created_at, ulid)` secondaries (the caller supplies the issue metadata — the CRDT
    /// owns only ordering, never the row body). `version` is `0` here (the CRDT engine has no CAS
    /// version — convergence replaces the optimistic token); a binding that still surfaces a version
    /// derives it from the op count. Issues absent from `meta` are skipped (a stale list entry).
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

    /// **Merge a peer replica's full state into this one (the convergence operation).** Exchanges the
    /// peer's update bytes; after a bidirectional merge both replicas hold the SAME convergent order
    /// (no reorder lost, no divergence) — the CRDT's defining property the ISS-D5 re-green asserts. A
    /// convenience over [`Self::apply_update`] of the peer's [`Self::encode_state`].
    pub fn merge_peer(&self, peer: &MoveCrdtBoard) -> Result<(), MoveCrdtError> {
        self.apply_update(&peer.encode_state())
    }

    /// The number of issues on the board (the convergent list length).
    #[must_use]
    pub fn len(&self) -> usize {
        let txn = self.doc.transact();
        self.items.len(&txn) as usize
    }

    /// `true` iff the board has no issues.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// **The typed LOUD error surface of the move-CRDT engine (never a silent merge failure — EI-01 §5).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveCrdtError {
    /// A move named an `issue_id` not on the board (a stale client / a deleted issue) — surfaced, never
    /// a silent no-op.
    NoSuchIssue(String),
    /// Update bytes failed to decode/apply (a corrupt payload — surfaced, never silently dropped).
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

// ===========================================================================
// §3 — the named floors (VISION §3 — the measured trigger + the post-M5 follow-on)
// ===========================================================================

/// **The named move-CRDT floors (VISION §3 — name the trigger, name the post-M5 follow-on).**
pub struct MoveCrdtFloors;

impl MoveCrdtFloors {
    /// **The MEASURED promotion trigger.** The move-CRDT is promoted for a board ONLY once
    /// [`ReorderPressure::should_promote`] fires (a measured re-base rate ≥
    /// [`ReorderPressure::PROMOTE_THRESHOLD`] over ≥ [`ReorderPressure::MIN_ATTEMPTS`]). Below the
    /// trigger the [`crate::reorder`] CAS floor is the complete, correct, cheaper engine.
    pub const MEASURED_TRIGGER: &'static str =
        "order_key + server-arbitrated CAS → move-CRDT (Yrs list) on measured concurrent-reorder pain \
         (R-3, arch §5, ISS-P32 / P-495)";

    /// **The real-LLM runtime (R-10) is the post-M5 follow-on** (named here so the measured-promotion
    /// register is complete). The move-CRDT promotion does not depend on it — it is a pure ordering
    /// engine swap.
    pub const REAL_LLM_RUNTIME_POST_M5: &'static str =
        "the LlmAgentRuntime real-LLM swap is the post-M5 follow-on (R-10)";
}

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

    /// **The measured trigger stands the floor until the signal fires (VISION §3).** Below
    /// `MIN_ATTEMPTS`, and below the rate threshold, `should_promote` is FALSE — the CAS floor is the
    /// engine. Only a measured re-base rate at/above the trigger over enough samples promotes.
    #[test]
    fn reorder_pressure_promotes_only_on_the_measured_trigger() {
        let p = ReorderPressure::new();
        // Below MIN_ATTEMPTS, even an all-rebase board does NOT promote (the rate is not yet trusted).
        for _ in 0..(ReorderPressure::MIN_ATTEMPTS - 1) {
            p.observe_cas_outcome(true);
        }
        assert!(
            !p.should_promote(),
            "below MIN_ATTEMPTS the floor stands (the rate is not trusted)"
        );

        // A low-churn board over enough samples does NOT promote (the floor is the cheaper engine).
        let calm = ReorderPressure::new();
        for i in 0..40 {
            calm.observe_cas_outcome(i % 20 == 0); // ~5% re-base rate, below the 25% trigger
        }
        assert!(calm.rebase_rate() < ReorderPressure::PROMOTE_THRESHOLD);
        assert!(
            !calm.should_promote(),
            "a calm board stays on the CAS floor (no premature promotion)"
        );

        // A measured high-churn board over enough samples DOES promote.
        let hot = ReorderPressure::new();
        for i in 0..40 {
            hot.observe_cas_outcome(i % 2 == 0); // 50% re-base rate, above the trigger
        }
        assert!(hot.rebase_rate() >= ReorderPressure::PROMOTE_THRESHOLD);
        assert!(
            hot.should_promote(),
            "a measured concurrent-reorder-pain board promotes to the move-CRDT"
        );
    }

    /// **The promotion swaps the conflict ENGINE, not the data model (arch §5).** The move-CRDT seeded
    /// from a displayed order produces derived `order_key` hints that sort the issues in the SAME
    /// order — the byte-identical encoding, the unchanged data model across the boundary.
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
        // The derived hints are in list order and STRICTLY increasing (the frozen codec) — the same
        // displayed order, recomputed from the convergent list, not a bespoke jitter.
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

    /// **The convergence property (the move-CRDT headline) — two concurrent distinct-issue moves BOTH
    /// survive, no loser, no clobber (the CAS floor's re-base churn is gone).** Two replicas seed from
    /// the same order; replica A moves I3 to the front, replica B moves I0 to the back, concurrently;
    /// they exchange updates and CONVERGE to the SAME order with BOTH moves applied.
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

        // concurrent, no coordination: A moves I3 to front, B moves I0 to back.
        a.move_issue("I3", 0).expect("A moves I3 to front");
        b.move_issue("I0", 4).expect("B moves I0 to back");

        // exchange updates (the firehose carries the diffs) — both directions.
        a.merge_peer(&b).expect("A merges B");
        b.merge_peer(&a).expect("B merges A");

        // CONVERGENCE: both replicas hold the SAME order, with BOTH moves applied (no loser).
        assert_eq!(a.order(), b.order(), "the two replicas converge");
        let order = a.order();
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(
            pos("I3") < pos("I0"),
            "I3 moved to front, I0 moved to back — both survive"
        );
    }

    /// **The merge is idempotent (effectively-once at the merge layer).** Re-applying the same peer
    /// update is a no-op — the convergent order is unchanged (mirroring the transport's `UNIQUE(op_id)`).
    #[test]
    fn merge_is_idempotent() {
        let seed = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let x = MoveCrdtBoard::seed_from_order(&seed);
        let y = MoveCrdtBoard::from_state(&x.encode_state()).unwrap();
        let upd = y.move_issue("C", 0).unwrap();
        x.apply_update(&upd).unwrap();
        let once = x.order();
        // re-apply the SAME update — idempotent, no change.
        x.apply_update(&upd).unwrap();
        assert_eq!(
            x.order(),
            once,
            "re-applying a move is a no-op (idempotent merge)"
        );
    }

    /// A malformed update is a LOUD error (never a silent drop — EI-01 §5).
    #[test]
    fn malformed_update_is_a_loud_error() {
        let board = MoveCrdtBoard::seed_from_order(&["A".to_string()]);
        let err = board.apply_update(&[0xde, 0xad, 0xbe, 0xef]).unwrap_err();
        assert_eq!(err, MoveCrdtError::MalformedUpdate);
    }

    /// A move of an unknown issue is a LOUD error (never a silent no-op).
    #[test]
    fn move_unknown_issue_is_a_loud_error() {
        let board = MoveCrdtBoard::seed_from_order(&["A".to_string()]);
        let err = board.move_issue("ghost", 0).unwrap_err();
        assert_eq!(err, MoveCrdtError::NoSuchIssue("ghost".to_string()));
    }

    /// **The deterministic seed (the replayable cutover — §3.4 step 2).** The same seed order always
    /// produces byte-identical state bytes (the fixed seed client_id + GC off) — the `engine_promote`
    /// cutover is replayable.
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
