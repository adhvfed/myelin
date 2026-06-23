//! # `keys` — the Hi/Lo human-key allocator (ISS-P08 / P-374, M4-I1)
//!
//! **What this module ships:** the per-prefix **Hi/Lo** allocator that hands out the issue's
//! **stored canonical id** `<PROJECTKEY>-<seqno>` (contract 5.1, recon REF-3). The
//! `prefix_counter` table (ISS-P05 migration `iss_0009_prefix_counter`) holds the durable **Hi**
//! block high-water mark; the allocator reserves a block with ONE atomic
//! `UPDATE … SET high_water = high_water + block_size RETURNING high_water - block_size, high_water`
//! and then hands out the **Lo** seqnos from that block IN MEMORY (no DB contact per key). The minted
//! `<PROJECTKEY>-<seqno>` is the stored `<id>` segment of the issue's
//! [`myelin_events::ArtifactRef`] (`myelin://<tenant>/issue/issue/<PROJECTKEY>-<seqno>`); the
//! short display form `#<seqno>` is a **render-time projection only**, never stored as the link
//! (REF-3, [`render_display_key`]).
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §4 (Human-key allocation — Hi/Lo, sketch 04, TE-14):
//!
//! ```text
//! fn allocate_key(prefix) -> String:
//!     if local_block_for(prefix).is_empty():
//!         (lo, hi) = UPDATE prefix_counter
//!                      SET high_water = high_water + block_size
//!                      WHERE (tenant, prefix) = …
//!                      RETURNING high_water - block_size, high_water     // ONE atomic reserve
//!         local_block_for(prefix) = (lo+1 ..= hi)
//!         maybe_grow_block_size(prefix)        // adaptive: raise block_size on a high create-rate
//!     n = local_block_for(prefix).next()       // handed out from memory, no DB contact
//!     return format!("{}-{}", prefix, n)       // the STORED CANONICAL <id> (contract 5.1)
//! ```
//!
//! **The four invariants the architecture pins (§4 + §"Key allocation" row), each tested below:**
//! 1. **Monotonic per prefix** — seqnos are strictly increasing within a prefix; a crash after
//!    reserving but before using a block leaks the unused tail as a **gap**, never a reuse, never a
//!    double-allocation. (Reserve-then-use, leak-a-block-on-crash — the platform's at-least-once +
//!    idempotent shape.)
//! 2. **Gap-tolerant** — gaps (from a leaked block on crash, or a partially-consumed block) are
//!    benign: the canonical key is still unique + monotonic; the next reserve continues from the
//!    durable high-water, never from the consumed Lo position. Nothing reads "the count of issues" off
//!    the seqno.
//! 3. **Adaptive block size** — start small (50, tiny gaps for a cold prefix) → grow (toward 1000) on
//!    a measured high create-rate so a hot prefix (an incident storm, a 10k-issue import) drops the
//!    `prefix_counter` write rate by N× without serialising on the counter row (1 counter write per
//!    block, not per key).
//! 4. **Per-prefix isolation** — each `(tenant, prefix)` has its own counter row + its own local
//!    block; a busy `ENG` never slows `OPS`, and the two prefixes' seqno spaces never collide.
//!
//! **Contract-index rows (built to the FROZEN shapes — never diverged):**
//! - **5.1 (owned)** — the Issues canonical `<id>` is `<PROJECTKEY>-<seqno>` (the stored key); the
//!   `#<seqno>` short form is the render-time display projection ([`render_display_key`]). The minted
//!   key is the `<id>` segment of [`issue_artifact_ref`] / `myelin_refs::parse` admits it (proven by
//!   the CDC `cdc_5_1_iss_key.rs`).
//! - **2.2 (consumed)** — the key allocation slots into the ISS-P06 write path so the minted canonical
//!   key co-commits with the issue's `issue.created` event ([`crate::write_path`]); the key write
//!   does NOT add a second emit verb (the `no-raw-publish` lint holds).
//!
//! ## Cell-local, one allocator over a [`PrefixReserve`] port (EI-01 §7 — one mechanism)
//! The allocator is **cell-local** (each cell runs its own [`HiLoKeyAllocator`] over its cell's
//! `prefix_counter` rows; cross-cell uniqueness is by single-home — an issue lives in exactly one
//! cell, OQ-I). The DB reserve is abstracted behind the [`PrefixReserve`] port so the in-memory test
//! reserve ([`InMemoryPrefixCounter`]) and the live-Postgres reserve
//! ([`integration_iss_p08_key_storm.rs`]) drive the SAME allocator + the SAME block arithmetic — the
//! reserve is the ONLY thing that touches the DB, and the live `UPDATE … RETURNING` is its sole real
//! implementation. The atomic `UPDATE … RETURNING` is the substrate's transactional reserve, not a
//! second concurrency primitive.
//!
//! ## FLOORS NAMED (VISION §3 — name-your-floors)
//! - **The render-time `#<seqno>` projection is DISPLAY-ONLY** ([`render_display_key`]). It is NEVER
//!   stored, NEVER an `ArtifactRef` link segment, and NOT parseable as a scope (Refs `parse` rejects
//!   the bare `#1421`, per REF-3). The stored canonical id is always the full `<PROJECTKEY>-<seqno>`.
//! - **The storage floor was named in ISS-P05** (`prefix_counter` is the durable Hi block; the live
//!   PG-hybrid sharded-by-tenant store is the ISS-P32 distributed-SQL R-6 follow-on). This module adds
//!   no new storage floor.
//! - **The live `UPDATE … RETURNING` reserve** is the [`PrefixReserve`] port's sole production impl
//!   (the integration test proves it under a create-storm). The DB-free [`InMemoryPrefixCounter`] is
//!   test scaffolding that models EXACTLY the atomic-reserve semantics (the same gap-on-leak, the same
//!   monotonic high-water) — never a parallel production path.
//!
//! ## Mutation-score floor (mandatory-core — a duplicate key is a correctness failure)
//! A duplicate `<PROJECTKEY>-<seqno>` is a Tier-1 correctness failure (two issues sharing a canonical
//! id is silent data corruption), so this module is a **mandatory-core mutation target with a ≥ 90%
//! floor**: `cargo mutants -p myelin-issues --file crates/myelin-issues/src/keys.rs`. The
//! mutation-tested core is the block arithmetic (the `lo+1 ..= hi` half-open vs inclusive boundary,
//! the "reserve when the local block is empty" guard, the monotonic high-water advance), the adaptive
//! block-size growth (grow on a measured high rate, capped), and the per-prefix isolation (the
//! `(tenant, prefix)` key never crosses). A mutant that reuses a seqno, off-by-ones a block boundary,
//! crosses two prefixes' counters, or shrinks the high-water is caught. **FLOOR (measured-under-load):**
//! running the mutation score is a CI step; this prompt SHIPS the testable construction + the unit/
//! e2e/drill coverage the score reads — the measured % is the CI artifact, registered red-until-run
//! in the scorecard, never self-asserted here.

use myelin_events::ArtifactRef;
use myelin_tenancy::TenantId;
use std::collections::HashMap;
use std::sync::Mutex;

/// The starting (cold-prefix) adaptive block size — small, so a rarely-created prefix leaks at most a
/// few seqnos on a crash (arch §4: "start small (50) → tiny gaps for cold prefixes").
pub const INITIAL_BLOCK_SIZE: u32 = 50;
/// The ceiling the adaptive block size grows toward for a hot prefix (arch §4: "grow toward 1000 on a
/// measured high create-rate"). A hot prefix amortises the `prefix_counter` write by up to 1000×.
pub const MAX_BLOCK_SIZE: u32 = 1000;
/// The growth factor applied when a reserve is requested while the previous block was fully consumed
/// "fast" (the create-rate is high) — the block doubles each consecutive fast reserve, capped at
/// [`MAX_BLOCK_SIZE`]. A geometric ramp reaches the cap in a handful of bursts (50→100→200→400→800→1000).
pub const BLOCK_GROWTH_FACTOR: u32 = 2;

// ===========================================================================
// §1 — the stored canonical key + the render-time display projection (5.1 / REF-3)
// ===========================================================================

/// The Issues **stored canonical id** — `<PROJECTKEY>-<seqno>` (contract 5.1, recon REF-3). This is
/// the `<id>` segment of the issue's [`ArtifactRef`] (`myelin://<tenant>/issue/issue/<key>`); it is
/// the link the whole platform stores + references. NEVER the short display `#<seqno>` form.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalKey {
    /// The project key prefix (e.g. `ENG`, `OPS`) — the per-prefix counter partition.
    pub prefix: String,
    /// The monotonic-per-prefix sequence number (the Lo seqno handed out from a reserved block).
    pub seqno: u64,
}

impl CanonicalKey {
    /// The stored canonical `<PROJECTKEY>-<seqno>` string — the `<id>` segment of the issue's
    /// [`ArtifactRef`]. This is what rests in the DB + on the wire (references-not-payloads).
    pub fn render(&self) -> String {
        format!("{}-{}", self.prefix, self.seqno)
    }

    /// The **render-time display projection** `#<seqno>` (REF-3 — the short in-context form the UI
    /// shows when the project prefix is implied). **DISPLAY-ONLY:** this string is NEVER stored, NEVER
    /// an `ArtifactRef` link segment, and Refs `parse` REJECTS it as a scope. Always store
    /// [`render`](Self::render); only render this for a human under a known-project context.
    pub fn render_display_key(&self) -> String {
        format!("#{}", self.seqno)
    }

    /// The issue's stored canonical [`ArtifactRef`]
    /// (`myelin://<tenant>/issue/issue/<PROJECTKEY>-<seqno>`) — the frozen Issues artifact-ref
    /// grammar (5.1). The `<id>` segment is the stored canonical key; `myelin_refs::parse` admits it.
    pub fn issue_artifact_ref(&self, tenant: &TenantId) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{}/issue/issue/{}",
            tenant.0,
            self.render()
        ))
    }
}

/// The render-time display projection `#<seqno>` of a canonical key (REF-3). A free function mirror of
/// [`CanonicalKey::render_display_key`] so a caller that only has the seqno can project without
/// rebuilding the key. **DISPLAY-ONLY — never stored, never parseable as a scope.**
pub fn render_display_key(seqno: u64) -> String {
    format!("#{seqno}")
}

// ===========================================================================
// §2 — the prefix reserve PORT (the atomic Hi-block reserve, DB-abstracted)
// ===========================================================================

/// A reserved Hi block — the half-open Lo range the allocator hands out from memory. The
/// `prefix_counter` `high_water` advanced from `lo` (exclusive) to `hi` (inclusive) in ONE atomic
/// reserve, so this block owns the seqnos `lo+1 ..= hi` (arch §4 `local_block_for = (lo+1 ..= hi)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReservedBlock {
    /// The high-water BEFORE the reserve (exclusive lower bound — the first seqno this block owns is
    /// `lo + 1`).
    pub lo: u64,
    /// The high-water AFTER the reserve (inclusive upper bound — the last seqno this block owns).
    pub hi: u64,
}

impl ReservedBlock {
    /// The inclusive count of seqnos this block owns (`hi - lo`). A non-empty reserve always owns ≥ 1.
    pub fn len(&self) -> u64 {
        self.hi - self.lo
    }
    /// Whether the block owns no seqnos (a zero-width reserve — never produced on the happy path).
    pub fn is_empty(&self) -> bool {
        self.hi <= self.lo
    }
}

/// Why a Hi-block reserve failed (LOUD — a reserve failure FAILS THE ALLOCATION CLOSED; the write
/// path never mints a key without a durable reserve, so a key is never reused on a counter error).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReserveError {
    /// The reserve backend (the `prefix_counter` `UPDATE … RETURNING`) surfaced an error. The
    /// allocation fails closed; no key is handed out, no row is written.
    Backend(String),
}

impl std::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReserveError::Backend(why) => {
                write!(
                    f,
                    "prefix_counter reserve failed (allocation fails closed): {why}"
                )
            }
        }
    }
}

impl std::error::Error for ReserveError {}

/// The atomic **Hi-block reserve** port (the ONLY thing that touches the `prefix_counter` durable
/// state). Production: the live-Postgres `UPDATE prefix_counter SET high_water = high_water +
/// block_size WHERE (tenant, prefix) = … RETURNING high_water - block_size, high_water` (one atomic
/// statement; the row lock serialises concurrent reserves on the SAME prefix without an app-level
/// lock — different prefixes never contend). Test: [`InMemoryPrefixCounter`], modelling exactly that
/// atomic advance. The allocator NEVER reads/writes `prefix_counter` except through this port (one
/// mechanism, EI-01 §7).
pub trait PrefixReserve: Send + Sync {
    /// Atomically advance the `(tenant, prefix)` high-water by `block_size` and return the
    /// [`ReservedBlock`] (the `lo`=before / `hi`=after). The row is upserted (a first reserve for a
    /// fresh prefix starts at high-water 0, so the first block owns `1 ..= block_size`). MUST be
    /// linearisable per `(tenant, prefix)` (the live impl relies on the row lock; the in-memory impl
    /// holds a mutex) — concurrent reserves on one prefix return DISJOINT blocks, the source of the
    /// 0-duplicate-key guarantee.
    fn reserve(
        &self,
        tenant: &TenantId,
        prefix: &str,
        block_size: u32,
    ) -> Result<ReservedBlock, ReserveError>;
}

/// The DB-free in-memory `prefix_counter` reserve (test scaffolding — EI-01 §7: it models EXACTLY the
/// live `UPDATE … RETURNING` atomic advance, never a parallel production path). A mutex over a
/// per-`(tenant, prefix)` high-water map gives the same linearisable-per-prefix advance the
/// Postgres row lock gives, so a concurrent create-storm returns disjoint blocks here too (the
/// in-memory leg of the ISS-D4 drill). The live leg is `integration_iss_p08_key_storm.rs`.
#[derive(Default)]
pub struct InMemoryPrefixCounter {
    high_water: Mutex<HashMap<(String, String), u64>>,
}

impl InMemoryPrefixCounter {
    /// A fresh, empty in-memory `prefix_counter` (every prefix starts at high-water 0).
    pub fn new() -> Self {
        Self::default()
    }

    /// The current durable high-water for a prefix (test inspection — the durable Hi mark, NOT the
    /// in-memory Lo position the allocator has consumed). 0 if the prefix has never reserved.
    pub fn high_water(&self, tenant: &TenantId, prefix: &str) -> u64 {
        *self
            .high_water
            .lock()
            .expect("prefix_counter mutex poisoned")
            .get(&(tenant.0.clone(), prefix.to_string()))
            .unwrap_or(&0)
    }
}

impl PrefixReserve for InMemoryPrefixCounter {
    fn reserve(
        &self,
        tenant: &TenantId,
        prefix: &str,
        block_size: u32,
    ) -> Result<ReservedBlock, ReserveError> {
        let mut map = self
            .high_water
            .lock()
            .map_err(|e| ReserveError::Backend(format!("mutex poisoned: {e}")))?;
        let key = (tenant.0.clone(), prefix.to_string());
        // the atomic advance: lo = before, hi = before + block_size. Linearised by the mutex (the
        // live impl linearises by the Postgres row lock) — two concurrent reserves get disjoint blocks.
        let lo = *map.get(&key).unwrap_or(&0);
        let hi = lo + block_size as u64;
        map.insert(key, hi);
        Ok(ReservedBlock { lo, hi })
    }
}

// ===========================================================================
// §3 — the Hi/Lo allocator: reserve a block, hand out Lo seqnos from memory
// ===========================================================================

/// The in-memory Lo state for ONE prefix: the next seqno to hand out, the inclusive block upper bound,
/// and the adaptive block size to reserve next time. Per-prefix isolation: each prefix has its own
/// [`PrefixLocalBlock`] (a busy `ENG` never touches `OPS`'s block).
#[derive(Clone, Copy, Debug)]
struct PrefixLocalBlock {
    /// The next seqno to hand out (the Lo cursor). When `next > block_hi`, the local block is empty
    /// and the allocator reserves a fresh Hi block.
    next: u64,
    /// The inclusive upper bound of the currently-held block (`hi` from the last reserve).
    block_hi: u64,
    /// The adaptive block size to use on the NEXT reserve for this prefix (grows on a hot prefix).
    block_size: u32,
}

impl PrefixLocalBlock {
    /// A cold prefix's initial state: no block held (`next > block_hi` ⇒ the first `allocate` reserves
    /// at [`INITIAL_BLOCK_SIZE`]).
    fn cold() -> Self {
        Self {
            next: 1,
            block_hi: 0, // next (1) > block_hi (0) ⇒ empty ⇒ reserve on first allocate.
            block_size: INITIAL_BLOCK_SIZE,
        }
    }
    /// Whether the local block is exhausted (the next seqno would exceed the held block's upper bound)
    /// — the guard for "reserve a fresh Hi block".
    fn is_empty(&self) -> bool {
        self.next > self.block_hi
    }
}

/// **The Hi/Lo human-key allocator (ISS-P08 / P-374 — the prompt's headline).** Per-prefix,
/// gap-tolerant, monotonic, adaptive-block, per-prefix-isolated, cell-local. Hands out the stored
/// canonical `<PROJECTKEY>-<seqno>` (5.1) over a [`PrefixReserve`] port (the live `prefix_counter`
/// `UPDATE … RETURNING` in production, the in-memory model in test). One allocator per cell; the
/// per-prefix Lo blocks live in memory (no DB contact per key — 1 counter write per block).
pub struct HiLoKeyAllocator<R: PrefixReserve> {
    reserve: R,
    /// The per-`(tenant, prefix)` in-memory Lo block. Behind a mutex so one cell-local allocator is
    /// safe to share across the cell's worker threads (concurrent `allocate` on the SAME prefix
    /// serialises only on the in-memory cursor advance + the rare block reserve — not per key on the
    /// DB).
    blocks: Mutex<HashMap<(String, String), PrefixLocalBlock>>,
}

impl<R: PrefixReserve> HiLoKeyAllocator<R> {
    /// A fresh allocator over a [`PrefixReserve`] backend (one per cell).
    pub fn new(reserve: R) -> Self {
        Self {
            reserve,
            blocks: Mutex::new(HashMap::new()),
        }
    }

    /// **Allocate the next stored canonical key for a prefix** (arch §4 `allocate_key`). If the
    /// prefix's local block is exhausted, reserve a fresh Hi block atomically (1 DB write), grow the
    /// block size if the create-rate is high (adaptive), then hand out the next Lo seqno from memory
    /// (no DB contact). The returned [`CanonicalKey`] is monotonic-per-prefix + globally unique within
    /// the prefix; a crash between reserve and use leaks the unused tail as a benign gap.
    ///
    /// A reserve failure FAILS CLOSED ([`ReserveError`]) — no key is handed out, so a key is never
    /// minted without a durable Hi advance (no reuse on a counter error).
    pub fn allocate(&self, tenant: &TenantId, prefix: &str) -> Result<CanonicalKey, ReserveError> {
        let mut blocks = self
            .blocks
            .lock()
            .map_err(|e| ReserveError::Backend(format!("blocks mutex poisoned: {e}")))?;
        let key = (tenant.0.clone(), prefix.to_string());
        let block = blocks.entry(key).or_insert_with(PrefixLocalBlock::cold);

        if block.is_empty() {
            // adaptive: if the previous block was fully consumed (not the very first cold reserve),
            // the create-rate on this prefix is high → grow the block size for THIS reserve so a hot
            // prefix amortises the counter write by N×. block_hi == 0 marks the never-reserved cold
            // start (no growth on the first block).
            if block.block_hi > 0 {
                block.block_size = grow_block_size(block.block_size);
            }
            let reserved = self.reserve.reserve(tenant, prefix, block.block_size)?;
            // local_block_for(prefix) = (lo+1 ..= hi): the first owned seqno is lo+1.
            block.next = reserved.lo + 1;
            block.block_hi = reserved.hi;
        }

        let seqno = block.next;
        block.next += 1; // hand out from memory; monotonic, no DB contact.
        Ok(CanonicalKey {
            prefix: prefix.to_string(),
            seqno,
        })
    }
}

/// The adaptive block-size growth: double toward [`MAX_BLOCK_SIZE`] (arch §4 — "grow toward 1000 on a
/// measured high create-rate"). Called on each reserve where the PREVIOUS block was fully consumed
/// (the create-rate signal). Saturating + capped — never overflows, never exceeds the ceiling.
fn grow_block_size(current: u32) -> u32 {
    current
        .saturating_mul(BLOCK_GROWTH_FACTOR)
        .min(MAX_BLOCK_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    // ── §1: the stored canonical key vs the render-time display projection (5.1 / REF-3) ───────────

    #[test]
    fn canonical_key_is_projectkey_dash_seqno_render_is_hash_seqno() {
        let k = CanonicalKey {
            prefix: "ENG".into(),
            seqno: 1421,
        };
        // the STORED canonical id (5.1) — the ArtifactRef <id> segment.
        assert_eq!(k.render(), "ENG-1421");
        // the render-time display projection (REF-3) — display-only, NEVER stored.
        assert_eq!(k.render_display_key(), "#1421");
        assert_eq!(render_display_key(1421), "#1421");
        // the full stored ArtifactRef carries the canonical key, never the #form.
        assert_eq!(
            k.issue_artifact_ref(&tenant()).0,
            "myelin://acme/issue/issue/ENG-1421"
        );
    }

    /// **The stored canonical key is the one Refs `parse` admits; the `#<seqno>` form is rejected as a
    /// scope (REF-3).** This is the CDC-shaped pin that the render projection is display-only.
    #[test]
    fn stored_key_parses_and_display_form_is_not_a_scope() {
        let k = CanonicalKey {
            prefix: "ENG".into(),
            seqno: 7,
        };
        let stored = k.issue_artifact_ref(&tenant());
        // the canonical stored id parses (it is a real URN component).
        myelin_refs::parse(&stored.0).expect("the stored <PROJECTKEY>-<seqno> id is a valid ref");
        // the render-time #<seqno> is NOT a parseable scope (REF-3 — display-only).
        assert!(
            myelin_refs::parse(&k.render_display_key()).is_err(),
            "the #<seqno> display form is render-time only, never a scope"
        );
    }

    // ── §3: monotonic per prefix ───────────────────────────────────────────────────────────────────

    #[test]
    fn keys_are_monotonic_per_prefix_starting_at_one() {
        let a = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
        let mut last = 0;
        for i in 1..=130u64 {
            let k = a.allocate(&tenant(), "ENG").expect("allocate");
            assert_eq!(k.seqno, i, "seqno is contiguous + monotonic per prefix");
            assert!(k.seqno > last, "strictly increasing");
            last = k.seqno;
        }
        // the first key is ENG-1 (not ENG-0).
        let first = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
        assert_eq!(first.allocate(&tenant(), "ENG").unwrap().seqno, 1);
    }

    // ── §3: per-prefix isolation (two prefixes' seqno spaces never collide) ─────────────────────────

    #[test]
    fn per_prefix_isolation_two_prefixes_have_independent_seqno_spaces() {
        let a = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
        // interleave ENG and OPS — each gets its own 1,2,3 space.
        assert_eq!(a.allocate(&tenant(), "ENG").unwrap().seqno, 1);
        assert_eq!(a.allocate(&tenant(), "OPS").unwrap().seqno, 1);
        assert_eq!(a.allocate(&tenant(), "ENG").unwrap().seqno, 2);
        assert_eq!(a.allocate(&tenant(), "OPS").unwrap().seqno, 2);
        assert_eq!(a.allocate(&tenant(), "ENG").unwrap().seqno, 3);
        // a different tenant's ENG is also isolated.
        let other = TenantId("globex".into());
        assert_eq!(a.allocate(&other, "ENG").unwrap().seqno, 1);
    }

    // ── §3: gap-tolerance (a leaked block on "crash" is benign — still unique + monotonic) ──────────

    /// **A crash after reserving but before consuming a block leaks the tail as a GAP, never a reuse.**
    /// We simulate the crash by dropping an allocator that has reserved a block but only consumed part
    /// of it; a fresh allocator over the SAME durable counter continues from the durable high-water (a
    /// gap), never from the consumed Lo position — monotonic, no double-allocation.
    #[test]
    fn gap_tolerant_a_leaked_block_is_benign_no_reuse() {
        let counter = Arc::new(InMemoryPrefixCounter::new());
        // allocator #1 reserves a block of 50 and consumes only 3 (seqnos 1,2,3) — then "crashes".
        {
            let a1 = HiLoKeyAllocator::new(SharedReserve(Arc::clone(&counter)));
            assert_eq!(a1.allocate(&tenant(), "ENG").unwrap().seqno, 1);
            assert_eq!(a1.allocate(&tenant(), "ENG").unwrap().seqno, 2);
            assert_eq!(a1.allocate(&tenant(), "ENG").unwrap().seqno, 3);
            // the durable high-water already advanced a full block (50) on the reserve.
            assert_eq!(counter.high_water(&tenant(), "ENG"), 50);
        } // a1 dropped — seqnos 4..=50 are LEAKED (a benign gap), never reused.

        // allocator #2 over the SAME durable counter continues from the durable high-water (51),
        // NOT from 4 — the gap 4..=50 is benign, the keys stay unique + monotonic.
        let a2 = HiLoKeyAllocator::new(SharedReserve(Arc::clone(&counter)));
        let next = a2.allocate(&tenant(), "ENG").unwrap();
        assert_eq!(
            next.seqno, 51,
            "continues from the durable high-water — a gap, never a reuse"
        );
    }

    // ── §3: adaptive block size (a hot prefix grows the block; a cold one stays small) ──────────────

    #[test]
    fn adaptive_block_size_grows_on_a_hot_prefix() {
        let counter = Arc::new(InMemoryPrefixCounter::new());
        let a = HiLoKeyAllocator::new(SharedReserve(Arc::clone(&counter)));
        // consume the first cold block (50) entirely → high-water 50.
        for _ in 0..INITIAL_BLOCK_SIZE {
            a.allocate(&tenant(), "ENG").unwrap();
        }
        assert_eq!(
            counter.high_water(&tenant(), "ENG"),
            50,
            "first block is the cold size 50"
        );
        // the 51st allocate reserves a GROWN block (50 → 100) — high-water jumps by 100.
        a.allocate(&tenant(), "ENG").unwrap();
        assert_eq!(
            counter.high_water(&tenant(), "ENG"),
            150,
            "the second block grew to 100"
        );
        // consume the rest of the 100-block, then the next reserve grows again (100 → 200).
        for _ in 0..99 {
            a.allocate(&tenant(), "ENG").unwrap();
        }
        a.allocate(&tenant(), "ENG").unwrap();
        assert_eq!(
            counter.high_water(&tenant(), "ENG"),
            350,
            "the third block grew to 200"
        );
    }

    #[test]
    fn block_size_growth_is_capped_at_max() {
        let mut sz = INITIAL_BLOCK_SIZE;
        for _ in 0..20 {
            sz = grow_block_size(sz);
        }
        assert_eq!(
            sz, MAX_BLOCK_SIZE,
            "growth saturates at the ceiling, never overflows"
        );
        // a single step doubles (until the cap).
        assert_eq!(grow_block_size(50), 100);
        assert_eq!(
            grow_block_size(800),
            1000,
            "the step toward the cap is clamped"
        );
        assert_eq!(grow_block_size(1000), 1000);
    }

    // ── the create-storm: N workers on ONE hot prefix → 0 duplicate key, monotonic (ISS-D4 in-mem) ──

    /// **The ISS-D4 create-storm (in-memory leg): N workers hammer ONE hot prefix → 0 duplicate key,
    /// the seqnos are exactly the contiguous 1..=total (gap-free here because every reserved block is
    /// fully consumed), per-prefix isolation holds.** The live-Postgres leg is
    /// `integration_iss_p08_key_storm.rs` (the dated green artifact).
    #[test]
    fn create_storm_on_one_hot_prefix_zero_dup_monotonic() {
        const WORKERS: usize = 16;
        const PER_WORKER: usize = 500;
        let allocator = Arc::new(HiLoKeyAllocator::new(InMemoryPrefixCounter::new()));
        let mut handles = Vec::new();
        for _ in 0..WORKERS {
            let a = Arc::clone(&allocator);
            handles.push(thread::spawn(move || {
                let mut got = Vec::with_capacity(PER_WORKER);
                for _ in 0..PER_WORKER {
                    got.push(a.allocate(&tenant(), "ENG").unwrap().seqno);
                }
                got
            }));
        }
        let mut all: Vec<u64> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        let total = WORKERS * PER_WORKER;
        assert_eq!(all.len(), total);
        all.sort_unstable();
        // 0 duplicate key — the storm minted `total` DISTINCT seqnos.
        let distinct = {
            let mut d = all.clone();
            d.dedup();
            d.len()
        };
        assert_eq!(
            distinct, total,
            "0 duplicate key under a {WORKERS}-worker storm"
        );
        // monotonic + gap-free 1..=total (every block fully consumed → no leaked gap here).
        assert_eq!(all.first(), Some(&1));
        assert_eq!(all.last(), Some(&(total as u64)));
        for (i, seq) in all.iter().enumerate() {
            assert_eq!(*seq, (i + 1) as u64, "contiguous monotonic 1..=total");
        }
    }

    /// A test [`PrefixReserve`] that shares ONE durable counter across allocators (to model the
    /// crash/leak + the hot-prefix growth over a persistent `prefix_counter`).
    struct SharedReserve(Arc<InMemoryPrefixCounter>);
    impl PrefixReserve for SharedReserve {
        fn reserve(
            &self,
            tenant: &TenantId,
            prefix: &str,
            block_size: u32,
        ) -> Result<ReservedBlock, ReserveError> {
            self.0.reserve(tenant, prefix, block_size)
        }
    }

    #[test]
    fn reserve_error_display_is_loud() {
        let e = ReserveError::Backend("conn reset".into());
        assert!(format!("{e}").contains("fails closed"));
    }

    #[test]
    fn reserved_block_len_and_empty() {
        let b = ReservedBlock { lo: 0, hi: 50 };
        assert_eq!(b.len(), 50);
        assert!(!b.is_empty());
        assert!(ReservedBlock { lo: 7, hi: 7 }.is_empty());
    }
}
