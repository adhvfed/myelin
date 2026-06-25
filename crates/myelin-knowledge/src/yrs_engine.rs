//! # The Yrs CRDT engine (Layer 3b) — the merge promotion over the unchanged transport (KN-P29 / P-484, M5)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §3.3 (Layer-3b CRDT promotion: a **per-block content CRDT** `Y.Text`/`Y.XmlFragment` for inline
//! runs + a **tree/move CRDT** for block structure — the hybrid granularity; rich-text marks →
//! Peritext; the op-log becomes Yrs updates, the transport unchanged; the server stays a "dumb
//! relay + persistence + authority"), §3.4 (the online CAS→CRDT migration per-doc, no stop-the-world:
//! quiesce-lite snapshot → **deterministic** Yrs seed → a single `engine_promote` cutover op at the
//! next `op_seq` → reconcile in-flight CAS edits; reversible from the pre-cutover snapshot), §3.5 (the
//! move-CRDT's list type owns sibling ordering — `order_key` becomes a derived OLTP-index hint).
//!
//! **Canon (read in full first):** VISION §3 (name-your-floors — the CRDT is the NAMED M5 promotion of
//! the [`crate::merge`] CAS floor) + §4 (cite proven structures — Yrs/Yjs is the cited CRDT, never a
//! hand-rolled merge); external-insights/04-hard-problems.md §2 (CRDT-after-CAS — the trigger is the
//! first true concurrent-edit conflict; the CRDT slots into the transport WITHOUT touching the data
//! model); external-insights/01 §3 (prove-it: KN-D1 re-runs green ACROSS the `engine_promote`
//! boundary — the floor's promotion is itself drilled).
//!
//! **Contract-index:** row **3.5** the firehose resume-cursor transport — **CONSUMED, UNCHANGED.** The
//! Yrs UPDATE bytes ride [`crate::transport::DocOp::payload`] exactly as the CAS deltas did; the
//! resume cursor, the idempotent `UNIQUE(op_id)` apply, and the per-doc monotone `op_seq` are
//! identical (the CRDT is a Layer-3 PAYLOAD swap, not a transport rewrite — that is why the transport
//! was built first, KN-1).
//!
//! ## What this module ships (KN-P29's owned work — the Layer-3 swap)
//! - **[`YrsDoc`]** — the per-doc Yrs CRDT engine. The HYBRID granularity (§3.3):
//!   - a root **`blocks` `Array`** (the **tree/move CRDT** list, §3.5) owns SIBLING ORDERING — a block
//!     id at each ordinal; a move is a remove+insert in the convergent list type, so the bespoke
//!     LexoRank jitter/rebalance ([`crate::block_tree`]) RETIRES and `order_key` becomes a derived
//!     index hint recomputed from CRDT state ([`YrsDoc::derived_order_keys`]).
//!   - a root **`content` `Map`** maps `block_id → Y.Text` (the **per-block content CRDT**, §3.3) —
//!     two clients editing the SAME block's inline run MERGE (no blend lost), not conflict.
//! - **[`YrsDoc::seed_from_snapshot`]** — the **deterministic** Yrs seed from a quiesce-lite snapshot
//!   (§3.4 step 2): the same [`DocSnapshot`] always produces byte-identical CRDT update bytes (a fixed
//!   `client_id`, GC off, a fixed block order) — reproducible + replay-safe (the seed is the
//!   `engine_promote` payload, so it MUST be deterministic for the cutover to be replayable).
//! - **[`YrsDoc::encode_state`] / [`YrsDoc::apply_update`]** — the Yrs UPDATE bytes that ride the
//!   transport: `encode_state` is the full-state update a reconnecting client loads once across the
//!   `engine_promote` boundary; `apply_update` is the idempotent merge of a peer's update bytes (Yrs
//!   updates are commutative + idempotent — re-applying one is a no-op, the SAME at-least-once
//!   property the transport's `UNIQUE(op_id)` gives, now ALSO at the merge layer).
//! - **[`EnginePromotion`]** — the online per-doc CAS→CRDT migration (§3.4): a [`DocSnapshot`] +
//!   the deterministic seed + the cutover `op_seq` + the pre-cutover snapshot for REVERSIBILITY. The
//!   in-flight-CAS reconcile at the boundary is [`EnginePromotion::reconcile_inflight_cas`].
//!
//! ## FLOORS RESOLVED (the named KN-P13 follow-ons — VISION §3)
//! - **CAS → CRDT (RESOLVED).** [`crate::merge`] named "CAS — NO MERGE" with this as the follow-on.
//!   The Yrs `Y.Text` per-block content CRDT now MERGES two concurrent same-block edits convergently
//!   ([`YrsDoc::merge_peer`] + the convergence gate `tests/drill_kn_p29_*`), so the loser is no longer
//!   rejected — both edits survive.
//! - **Offline-first (RESOLVED).** The CAS floor's "offline = read + queued light-edit" ([`crate::merge::OfflineQueue`])
//!   is promoted to FULL offline-first: two long-offline DIVERGENT edits to one block MERGE on
//!   reconnect (the CRDT's convergent merge over the accumulated update bytes), instead of one losing.
//! - **KQ-6 (editable-in-place synced blocks) DISPOSITION — NAMED POST-M5.** The CRDT ENABLES KQ-6
//!   (a synced/transcluded block becomes editable-in-place because its `Y.Text` is the same CRDT
//!   node wherever it appears). The full sync_block read-projection edit path ([`crate::sync_block`])
//!   is wired post-M5 (the cross-cell op fan-out KN-P30 + the materialisation KN-P31 land first) — it
//!   is NAMED here, not pulled in (this prompt is the Yrs promotion HALF; cross-cell collab is KN-P30).
//!
//! ## MANDATORY-CORE MUTATION FLOOR (the KN-P29 cargo-mutants gate — TESTS field)
//! Mandatory-core: the **deterministic seed** ([`YrsDoc::seed_from_snapshot`] — the block-order +
//! per-block-text construction that the `engine_promote` payload depends on) and the **cutover op_seq
//! continuity** ([`EnginePromotion::new`] / [`EnginePromotion::cutover_seq`] — the seed is appended at
//! `head + 1`, before it CAS, after it Yrs). The stated floor: **100% mutation score on the
//! seed-determinism + cutover-continuity path** — a mutated block order, a dropped text seed, or an
//! off-by-one cutover seq all flip the determinism assertion (two seeds from one snapshot diverge) or
//! the across-boundary KN-D1 0-lost/0-dup assertion. The convergence property itself is proven by Yrs
//! (the cited structure), not re-derived; the gate is that OUR seed + cutover are deterministic +
//! continuous. Run: `cargo mutants -p myelin-knowledge -f yrs_engine.rs`.

use crate::block_tree::BlockId;
use crate::transport::{DocOp, OpId, OpKind};
use yrs::updates::decoder::Decode;
use yrs::{
    Array, ArrayRef, Doc, GetString, Map, MapRef, ReadTxn, StateVector, Text, TextPrelim, TextRef,
    Transact, Update,
};

/// The fixed root name of the **tree/move CRDT list** that owns sibling ordering (§3.5 — the move-CRDT
/// list type; a block id at each ordinal). A move is a convergent remove+insert in THIS list.
const BLOCKS_ROOT: &str = "blocks";
/// The fixed root name of the **per-block content map** (`block_id → Y.Text`, §3.3 — the per-block
/// content CRDT). A same-block concurrent edit merges in the block's `Y.Text`.
const CONTENT_ROOT: &str = "content";

/// **The fixed `client_id` of the DETERMINISTIC seed doc (§3.4 step 2 — reproducible + replay-safe).**
/// The Yrs seed MUST be byte-identical for the same snapshot (it IS the `engine_promote` cutover
/// payload, which a reconnecting client loads ACROSS the boundary — a non-deterministic seed would
/// make the cutover unreplayable). A FIXED `client_id` (the server is the seeding authority — the
/// "dumb relay + persistence + authority", §3.3) + GC off ([`new_seed_doc`]) makes the seed update
/// bytes a pure function of the snapshot. Live client edits AFTER the seed use their own `client_id`.
const SEED_CLIENT_ID: u64 = 0;

/// **A quiesce-lite materialised snapshot of a doc (§3.4 step 1 — the CAS-era state the Yrs seed is
/// built from).** The block tree (ordered sibling list) + each block's inline content string — the
/// EXACT shape [`crate::merge::CasStore`] + [`crate::block_tree::BlockTree`] materialise. The snapshot
/// PREDATES the cutover, so a botched promotion rolls back to it (§3.4 reversibility). Deterministic
/// input ⇒ deterministic seed: the block ORDER is load-bearing (the move-CRDT list ordinal).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DocSnapshot {
    /// The blocks in SIBLING ORDER (the move-CRDT list ordinals; the materialised LexoRank order at
    /// quiesce time). `(block_id, inline_content)` — the inline string seeds the block's `Y.Text`.
    pub blocks: Vec<(BlockId, String)>,
}

impl DocSnapshot {
    /// A fresh empty snapshot (a brand-new doc — the seed is the empty CRDT).
    pub fn new() -> DocSnapshot {
        DocSnapshot::default()
    }

    /// Push a block (in sibling order) with its inline content. Order is load-bearing — call in the
    /// materialised LexoRank order (the move-CRDT list ordinal the seed reproduces).
    pub fn push_block(&mut self, block_id: BlockId, inline: impl Into<String>) {
        self.blocks.push((block_id, inline.into()));
    }

    /// The number of blocks in the snapshot.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// `true` iff the snapshot has no blocks.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Build a fresh Yrs `Doc` with the DETERMINISTIC seed parameters (§3.4 step 2): a fixed `client_id`
/// (the seeding authority) and **GC disabled** (`skip_gc`), so the encoded seed update bytes are a
/// pure function of the construction order — byte-identical for the same snapshot. GC-off matters: GC
/// timing is non-deterministic, which would make the seed bytes vary; the live doc may GC later, the
/// SEED must not (its bytes are the replayable cutover payload).
fn new_seed_doc() -> Doc {
    let mut options = yrs::Options::with_client_id(yrs::block::ClientID::new(SEED_CLIENT_ID));
    options.skip_gc = true;
    Doc::with_options(options)
}

/// **The per-doc Yrs CRDT engine (Layer 3b, §3.3 — the hybrid-granularity merge engine).** Wraps a
/// `yrs::Doc` with the two root types: the `blocks` `Array` (the tree/move CRDT list owning sibling
/// ordering, §3.5) and the `content` `Map` (`block_id → Y.Text`, the per-block content CRDT, §3.3).
/// Engine-agnostic at the transport seam: its UPDATE bytes ride [`crate::transport::DocOp::payload`]
/// exactly as the CAS deltas did (the transport is unchanged).
pub struct YrsDoc {
    doc: Doc,
    blocks: ArrayRef,
    content: MapRef,
}

impl YrsDoc {
    /// **Seed a Yrs doc DETERMINISTICALLY from a quiesce-lite [`DocSnapshot`] (§3.4 step 2).** Builds
    /// the block tree → the move-CRDT `blocks` list (a block id at each ordinal, in snapshot order) +
    /// each block's inline string → a `Y.Text` in the `content` map. The SAME snapshot always yields
    /// byte-identical [`Self::encode_state`] bytes (the fixed seed `client_id` + GC off + the fixed
    /// construction order) — so the seed is the replayable `engine_promote` cutover payload.
    pub fn seed_from_snapshot(snapshot: &DocSnapshot) -> YrsDoc {
        let doc = new_seed_doc();
        let blocks = doc.get_or_insert_array(BLOCKS_ROOT);
        let content = doc.get_or_insert_map(CONTENT_ROOT);
        {
            let mut txn = doc.transact_mut();
            for (block_id, inline) in &snapshot.blocks {
                // The move-CRDT list ordinal: append the block id in snapshot (sibling) order.
                blocks.push_back(&mut txn, block_id.as_str());
                // The per-block content CRDT: a Y.Text seeded with the block's inline run.
                content.insert(&mut txn, block_id.as_str(), TextPrelim::new(inline.clone()));
            }
        }
        YrsDoc {
            doc,
            blocks,
            content,
        }
    }

    /// **Load a Yrs doc from previously-[`Self::encode_state`]d update bytes (the reconnect path,
    /// §3.4 step 3).** A client resuming ACROSS the `engine_promote` boundary loads the seeded Yrs
    /// state ONCE from these bytes, then applies the live tail. Errors loudly on malformed bytes
    /// (never a silent half-load).
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

    /// **Encode the FULL CRDT state as Yrs update bytes (the seed / reconnect payload, §3.4).** This
    /// is the `engine_promote` cutover payload (from this `op_seq` forward the op-log carries Yrs
    /// bytes) and the full-state a reconnecting client loads once. For a seed doc built via
    /// [`Self::seed_from_snapshot`] these bytes are a pure function of the snapshot (determinism).
    pub fn encode_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&StateVector::default())
    }

    /// **Apply a peer's Yrs UPDATE bytes idempotently (the merge, §3.3).** Yrs updates are both
    /// commutative and idempotent: applying the same update twice is a no-op, and updates from
    /// different clients merge to one convergent state regardless of arrival order. This is the
    /// at-least-once → effectively-once property at the MERGE layer (mirroring `UNIQUE(op_id)`).
    pub fn apply_update(&self, bytes: &[u8]) -> Result<(), YrsError> {
        let update = Update::decode_v1(bytes).map_err(|_| YrsError::MalformedUpdate)?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|_| YrsError::MalformedUpdate)
    }

    /// **The DIFF a peer needs to catch up from its `state_vector` (the incremental update, §3.3).**
    /// A client that has seen part of the doc sends its state vector; this returns ONLY the ops it is
    /// missing (the bounded delta, not the full state) — the per-op Yrs update bytes that ride the
    /// transport after the cutover. The bytes are commutative + idempotent (see [`Self::apply_update`]).
    pub fn encode_diff(&self, since: &StateVector) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_diff_v1(since)
    }

    /// The doc's current state vector (what this replica has seen — the basis a peer diffs against).
    pub fn state_vector(&self) -> StateVector {
        self.doc.transact().state_vector()
    }

    /// **Insert text into a block's `Y.Text` content CRDT (a content edit, §3.3).** The per-block
    /// content CRDT: a concurrent same-block insert from another replica MERGES (no blend lost) when
    /// the two replicas exchange updates. Returns the update bytes the edit produced (the transport
    /// payload). A block with no `content` entry is a loud error (edit before seed/insert).
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

    /// **Insert a NEW block into the tree (a structural edit — the move-CRDT list + a fresh `Y.Text`,
    /// §3.3/§3.5).** Appends the block id at `index` in the `blocks` list (the convergent sibling
    /// ordering) + seeds an empty `Y.Text`. Returns the update bytes.
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

    /// **Move a block within the sibling list (the move-CRDT, §3.5 — the move-CRDT's list type owns
    /// ordering; `order_key` retires).** A remove+insert in the convergent `blocks` list. Returns the
    /// update bytes. A block not in the list is a loud error.
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

    /// The current ordinal of a block in the sibling list, if present (the move-CRDT ordinal).
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

    /// The `Y.Text` ref for a block, or a loud error if the block has no content entry.
    fn block_text(&self, block_id: &BlockId) -> Result<TextRef, YrsError> {
        let txn = self.doc.transact();
        match self.content.get(&txn, block_id.as_str()) {
            Some(yrs::Out::YText(t)) => Ok(t),
            _ => Err(YrsError::NoSuchBlock(block_id.clone())),
        }
    }

    /// **The materialised inline content of a block (the convergent render — `render(parse(md)) == md`
    /// holds regardless of engine, §3.6).** Reads the block's `Y.Text` as a string.
    pub fn block_content(&self, block_id: &BlockId) -> Result<String, YrsError> {
        let text = self.block_text(block_id)?;
        let txn = self.doc.transact();
        Ok(text.get_string(&txn))
    }

    /// **The blocks in SIBLING ORDER (the move-CRDT list materialised — the convergent ordering).**
    /// This is the order the OLTP `order_key` is DERIVED from (§3.5), not a bespoke LexoRank jitter.
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

    /// **The DERIVED `order_key` hints recomputed from CRDT state (§3.5 — `order_key` becomes a derived
    /// OLTP-index ordering hint, the bespoke jitter/rebalance retires).** A monotone hint per block in
    /// the move-CRDT list order; the CRDT list is the source of truth for ordering, this is only the
    /// index hint the OLTP read path uses to return siblings in order (recomputed, never authoritative).
    pub fn derived_order_keys(&self) -> Vec<(BlockId, u64)> {
        self.block_order()
            .into_iter()
            .enumerate()
            .map(|(i, b)| (b, i as u64))
            .collect()
    }

    /// **Merge a peer replica's full state into this one (the convergence operation, §3.3).** Exchanges
    /// the peer's update bytes; after a bidirectional merge both replicas hold the SAME convergent
    /// state (no blend lost, no divergence) — the CRDT's defining property the convergence gate
    /// asserts. A convenience over [`Self::apply_update`] of the peer's [`Self::encode_state`].
    pub fn merge_peer(&self, peer: &YrsDoc) -> Result<(), YrsError> {
        self.apply_update(&peer.encode_state())
    }
}

/// **The typed LOUD error surface of the Yrs engine (never a silent merge failure — EI-01 §5).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YrsError {
    /// An edit/move named a `block_id` with no `Y.Text`/list entry (an edit before the seed/insert).
    NoSuchBlock(BlockId),
    /// Update bytes failed to decode/apply (a corrupt payload — surfaced, never silently dropped).
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

/// **The online per-doc CAS→CRDT `engine_promote` migration (§3.4 — no stop-the-world).** Carries the
/// quiesce-lite [`DocSnapshot`] (the reversibility seed, predating the cutover), the DETERMINISTIC Yrs
/// seed bytes (the cutover payload), and the cutover `op_seq` (the boundary: before it CAS deltas,
/// from it forward Yrs bytes). Built at a compaction boundary; the cutover is a SINGLE
/// [`OpKind::EnginePromote`] op appended at the next `op_seq` — an ordinary op on the unchanged
/// transport (the resume cursor straddles it; KN-D1 re-runs green ACROSS it).
#[derive(Clone, Debug)]
pub struct EnginePromotion {
    /// The pre-cutover quiesce-lite snapshot (the REVERSIBILITY seed, §3.4 — a botched promotion rolls
    /// back to this; it predates the cutover op).
    snapshot: DocSnapshot,
    /// The deterministic Yrs seed update bytes (the `engine_promote` cutover op payload — pure
    /// function of `snapshot`).
    seed_bytes: Vec<u8>,
    /// The `op_seq` the cutover op was appended at (`head + 1` at promotion time). Before it: CAS
    /// deltas; from it forward: Yrs update bytes. The boundary KN-D1 straddles.
    cutover_seq: u64,
}

impl EnginePromotion {
    /// **Plan the `engine_promote` cutover from a quiesce-lite snapshot at the current op-log head
    /// (§3.4 steps 1–3).** Seeds the deterministic Yrs doc from the snapshot, encodes its state as the
    /// cutover payload, and sets the cutover `op_seq = head + 1` (the next op_seq — op_seq CONTINUITY
    /// across the swap is the gated property). The snapshot is retained for reversibility.
    pub fn new(snapshot: DocSnapshot, head_seq: u64) -> EnginePromotion {
        let seed = YrsDoc::seed_from_snapshot(&snapshot);
        let seed_bytes = seed.encode_state();
        EnginePromotion {
            snapshot,
            seed_bytes,
            cutover_seq: head_seq + 1,
        }
    }

    /// The cutover `op_seq` (before it CAS, from it forward Yrs). `head + 1` — op_seq continuity.
    pub fn cutover_seq(&self) -> u64 {
        self.cutover_seq
    }

    /// The deterministic Yrs seed bytes (the cutover op payload).
    pub fn seed_bytes(&self) -> &[u8] {
        &self.seed_bytes
    }

    /// The pre-cutover snapshot (the reversibility seed — §3.4 rollback target).
    pub fn snapshot(&self) -> &DocSnapshot {
        &self.snapshot
    }

    /// **The single [`OpKind::EnginePromote`] cutover op (§3.4 step 3 — appended at `cutover_seq` on
    /// the UNCHANGED transport).** An ordinary [`DocOp`] carrying the deterministic Yrs seed bytes as
    /// its payload; the transport assigns it `op_seq` and fans it out like any op. A reconnecting
    /// client resumes ACROSS it, loads the seed once, and applies the tail (KN-D1 re-greens here).
    /// `actor` is the server (the seeding authority, §3.3) at a fixed `op_id` (one cutover per doc).
    pub fn cutover_op(&self) -> DocOp {
        DocOp::cas(
            OpId::new("server", self.cutover_seq),
            "actor-server",
            OpKind::EnginePromote,
            self.seed_bytes.clone(),
        )
    }

    /// **Materialise the seeded Yrs doc (the post-cutover engine state, §3.4).** A client loads THIS
    /// across the boundary then applies the live tail. Deterministic: the same promotion always yields
    /// the same doc state.
    pub fn seeded_doc(&self) -> YrsDoc {
        YrsDoc::seed_from_snapshot(&self.snapshot)
    }

    /// **Reconcile in-flight CAS edits that straddle the cutover (§3.4 step 4 — no silent drop).** A
    /// CAS edit that was in flight when the cutover landed is replayed INTO the Yrs engine as a
    /// content edit (the editor client switches its merge module at the boundary, §3.4) — so it
    /// survives the swap (merged convergently), never dropped. Returns the Yrs update bytes per
    /// reconciled edit (so the caller can fan them out on the transport after the cutover op).
    ///
    /// Each in-flight edit is `(block_id, index, chunk)` — the CAS content delta re-expressed as a
    /// Yrs `Y.Text` insert. An edit to a block absent from the seed surfaces its error LOUDLY (it is
    /// collected per-edit, never a silent drop — the §3.4 "no silent drop" property).
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

    // ── the deterministic seed (mandatory-core) ──────────────────────────────────────────────────

    /// **THE DETERMINISM GATE (§3.4 step 2 — mandatory-core): the SAME snapshot seeds byte-identical
    /// Yrs update bytes.** The seed is the `engine_promote` cutover payload a reconnecting client loads
    /// across the boundary — it MUST be reproducible (a non-deterministic seed makes the cutover
    /// unreplayable). The fixed seed `client_id` + GC off + the fixed construction order guarantee it.
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

    /// **The seed materialises the snapshot faithfully: block order + per-block content round-trip
    /// (§3.6 — `render(parse(md)) == md` regardless of engine).**
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

    /// **A reordered snapshot seeds DIFFERENT bytes (the block ORDER is load-bearing — the move-CRDT
    /// list ordinal). Kills a "drop the order" mutant.**
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

    // ── the per-block content CRDT MERGES (the CAS-NO-MERGE floor RESOLVED) ───────────────────────

    /// **THE CONVERGENCE GATE (§3.3 — the CAS→CRDT floor resolved): two replicas editing the SAME
    /// block concurrently MERGE to one convergent state (no blend lost, no divergence).** Under the CAS
    /// floor one writer would lose; under the CRDT both edits survive and both replicas converge.
    #[test]
    fn concurrent_same_block_edits_converge_no_blend_lost() {
        // Two replicas seeded from the SAME snapshot (the post-cutover state).
        let snap = {
            let mut s = DocSnapshot::new();
            s.push_block(bid("b1"), "");
            s
        };
        let seed = YrsDoc::seed_from_snapshot(&snap).encode_state();
        let a = YrsDoc::from_state(&seed).unwrap();
        let b = YrsDoc::from_state(&seed).unwrap();

        // CONCURRENTLY: A prepends "A", B prepends "B" to the SAME block (no coordination).
        let ua = a.edit_block_text(&bid("b1"), 0, "AAA").unwrap();
        let ub = b.edit_block_text(&bid("b1"), 0, "BBB").unwrap();

        // Exchange the concurrent updates (each applies the other's).
        a.apply_update(&ub).unwrap();
        b.apply_update(&ua).unwrap();

        let ca = a.block_content(&bid("b1")).unwrap();
        let cb = b.block_content(&bid("b1")).unwrap();
        // CONVERGENCE: both replicas hold the SAME state (no divergence).
        assert_eq!(ca, cb, "the two replicas converge to one state");
        // NO BLEND LOST: both authors' text survives (not one losing as under CAS).
        assert!(
            ca.contains("AAA") && ca.contains("BBB"),
            "both edits survived: {ca}"
        );
        assert_eq!(ca.len(), 6, "exactly both inserts, no duplication");
    }

    /// **N-client convergence (the convergence property at scale): 4 replicas each edit the same block
    /// concurrently, then all exchange — all converge to ONE identical state, all 4 edits present.**
    #[test]
    fn n_client_same_block_edits_converge() {
        let snap = {
            let mut s = DocSnapshot::new();
            s.push_block(bid("b1"), "");
            s
        };
        let seed = YrsDoc::seed_from_snapshot(&snap).encode_state();
        let replicas: Vec<YrsDoc> = (0..4).map(|_| YrsDoc::from_state(&seed).unwrap()).collect();

        // each replica makes a distinct concurrent edit to the SAME block.
        let updates: Vec<Vec<u8>> = replicas
            .iter()
            .enumerate()
            .map(|(i, r)| r.edit_block_text(&bid("b1"), 0, &format!("<{i}>")).unwrap())
            .collect();

        // full mesh exchange: every replica applies every other's update (idempotent if self-applied).
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

    /// **Yrs updates are IDEMPOTENT: applying the same update twice is a no-op (the merge-layer
    /// at-least-once → effectively-once, mirroring the transport's UNIQUE(op_id)).**
    #[test]
    fn applying_an_update_twice_is_idempotent() {
        let doc = YrsDoc::seed_from_snapshot(&snapshot());
        let u = doc.edit_block_text(&bid("b1"), 5, "!!").unwrap();
        let peer = YrsDoc::seed_from_snapshot(&snapshot());
        peer.apply_update(&u).unwrap();
        let once = peer.block_content(&bid("b1")).unwrap();
        peer.apply_update(&u).unwrap(); // re-apply — no double-insert.
        let twice = peer.block_content(&bid("b1")).unwrap();
        assert_eq!(
            once, twice,
            "re-applying the same update is a no-op (idempotent)"
        );
        assert_eq!(twice, "hello!!");
    }

    // ── the move-CRDT (LexoRank retires, §3.5) ───────────────────────────────────────────────────

    /// **The move-CRDT list owns sibling ordering; `order_key` becomes a DERIVED hint (§3.5).** A move
    /// is a convergent remove+insert in the list; the derived order keys are recomputed from CRDT
    /// state, not a bespoke LexoRank jitter.
    #[test]
    fn move_crdt_owns_ordering_order_key_derived() {
        let mut snap = DocSnapshot::new();
        snap.push_block(bid("b1"), "one");
        snap.push_block(bid("b2"), "two");
        snap.push_block(bid("b3"), "three");
        let doc = YrsDoc::seed_from_snapshot(&snap);
        // move b3 to the front.
        doc.move_block(&bid("b3"), 0).unwrap();
        assert_eq!(
            doc.block_order(),
            vec![bid("b3"), bid("b1"), bid("b2")],
            "the move-CRDT list owns the new order"
        );
        // the DERIVED order keys are recomputed from the CRDT list (monotone in list order).
        let keys = doc.derived_order_keys();
        assert_eq!(
            keys,
            vec![(bid("b3"), 0), (bid("b1"), 1), (bid("b2"), 2)],
            "order_key is a derived hint from CRDT state, not a bespoke LexoRank jitter"
        );
    }

    /// **Concurrent block MOVES converge (the move-CRDT — no key-collision reorder, §3.5).** Two
    /// replicas move different blocks concurrently; after exchange both converge to one ordering.
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
        // all four blocks still present (none lost in the reorder).
        assert_eq!(
            a.block_order().len(),
            4,
            "no block lost in concurrent moves"
        );
    }

    // ── the engine_promote migration (mandatory-core: op_seq continuity) ─────────────────────────

    /// **The `engine_promote` cutover is at `head + 1` (op_seq CONTINUITY — mandatory-core).** The
    /// cutover op gets the next op_seq; before it CAS, from it forward Yrs. Kills an off-by-one mutant.
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

    /// **The promotion is REVERSIBLE: the pre-cutover snapshot is retained (§3.4 — a botched promotion
    /// rolls back to it).** The snapshot predates the cutover; it is the rollback seed.
    #[test]
    fn promotion_retains_reversibility_snapshot() {
        let snap = snapshot();
        let promo = EnginePromotion::new(snap.clone(), 3);
        assert_eq!(
            promo.snapshot(),
            &snap,
            "the pre-cutover snapshot is retained for rollback"
        );
        // the seeded doc materialises the snapshot (the post-cutover state derives from the same seed).
        let doc = promo.seeded_doc();
        assert_eq!(doc.block_order(), vec![bid("b1"), bid("b2")]);
    }

    /// **In-flight CAS edits straddling the cutover are reconciled into the CRDT (§3.4 step 4 — no
    /// silent drop).** A CAS edit in flight at the cutover is replayed as a Yrs content edit, so it
    /// survives the swap (merged), never dropped.
    #[test]
    fn inflight_cas_edits_reconcile_across_cutover() {
        let promo = EnginePromotion::new(snapshot(), 0);
        let doc = promo.seeded_doc();
        // a CAS edit was in flight at the cutover: append "!" to b1 at index 5.
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

    /// **An in-flight edit to a block absent from the seed surfaces its error LOUDLY (no silent drop,
    /// §3.4).**
    #[test]
    fn inflight_edit_to_missing_block_errors_loudly() {
        let promo = EnginePromotion::new(snapshot(), 0);
        let doc = promo.seeded_doc();
        let results = promo.reconcile_inflight_cas(&doc, &[(bid("ghost"), 0, "x".to_string())]);
        assert_eq!(results[0], Err(YrsError::NoSuchBlock(bid("ghost"))));
    }

    // ── the transport seam: state round-trips through the op-log payload ──────────────────────────

    /// **The full state round-trips through update bytes (the reconnect-across-cutover load, §3.4):
    /// encode → from_state reproduces the doc.**
    #[test]
    fn state_round_trips_through_update_bytes() {
        let doc = YrsDoc::seed_from_snapshot(&snapshot());
        doc.edit_block_text(&bid("b2"), 5, "!").unwrap();
        let bytes = doc.encode_state();
        let loaded = YrsDoc::from_state(&bytes).unwrap();
        assert_eq!(loaded.block_order(), doc.block_order());
        assert_eq!(loaded.block_content(&bid("b2")).unwrap(), "world!");
    }

    /// **Malformed update bytes are a LOUD error (never a silent half-load).**
    #[test]
    fn malformed_update_bytes_error_loudly() {
        let doc = YrsDoc::seed_from_snapshot(&snapshot());
        assert_eq!(
            doc.apply_update(&[0xff, 0xff, 0xff, 0xff]),
            Err(YrsError::MalformedUpdate)
        );
    }

    /// **merge_peer converges two replicas (the convenience used by the convergence drill).** The two
    /// replicas load the SHARED seed via [`YrsDoc::from_state`] (distinct live `client_id`s — the real
    /// post-cutover path; two independent live editors), then each edits a DIFFERENT block.
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
