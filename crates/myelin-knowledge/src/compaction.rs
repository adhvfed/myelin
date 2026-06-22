//! # Version history + op-log compaction → content-addressed snapshots + op-log GC (KN-P11 → P-301, M3)
//!
//! **Owning architecture docs:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §3 (the `doc_op` op-log + the `doc_snapshot` metadata table; **"op-logs grow unbounded; periodic
//! compaction → a content-addressed snapshot in the object tier; the op-log table keeps the live
//! tail"**; the compaction cadence + the **GC rule: delete `doc_op` rows ≤ `snap_seq` EXCEPT those an
//! open client's resume cursor still trails — the cursor is the GC watermark that makes KD-1 survive
//! compaction**) +
//! `02-internals-and-algorithms.md` §3.4 step 1 (**"at a compaction boundary, snapshot the doc's
//! current materialised state"**; **"construct … deterministically from that snapshot … Deterministic
//! ⇒ reproducible + replay-safe"**) + §2.2 (the log-as-source-of-truth: the op-log is authoritative,
//! the live state a projection; `resync_required → *.snapshot` is the named cold-rebuild path).
//!
//! **Contract-index:** row **11.2** the content-addressed [`BlobStore`] (BLAKE3) — **CONSUMED** (the
//! compacted snapshot is stored content-addressed in the fs-backed-floor [`FsBlobStore`](myelin_storage::blob::FsBlobStore); the
//! object-store swap is KN-P31). Row **2.6** the block-granular `*.snapshot` replay target shape —
//! **REFERENCED** (the snapshot this module mints is what the KN-P20 replay path re-emits as
//! `knowledge.page.snapshot` block-granular; here it is minted, not re-emitted).
//!
//! ## What this module ships (KN-P11's deliverable)
//! - **[`materialize`]** — the deterministic materialised-state projection of a doc from its op-log
//!   (arch §2.2: the op-log is authoritative, the live state a projection). Replays the applied ops
//!   in `op_seq` order into a single canonical byte string. **Deterministic**: the same ordered op
//!   sequence always materialises to the same bytes (the precondition for a deterministic
//!   content-address). Engine-agnostic — the v1 payload is opaque CAS bytes; the SAME projection
//!   carries Yrs update bytes after KN-P29 (the byte layout is the op-log's, not this module's).
//! - **[`SnapshotCompactor::compact`]** — op-log compaction to a content-addressed (BLAKE3) snapshot:
//!   materialise the doc up to `up_to_seq`, store the bytes in the [`BlobStore`] (the content address
//!   IS the BLAKE3 of the materialised state), and return the [`DocSnapshot`] metadata (the
//!   `doc_snapshot` row: `snap_seq` + `blob_hash` + an optional `named_label`). **The live op-log tail
//!   (`op_seq > snap_seq`) is KEPT** — compaction never touches the tail (arch §3: "the op-log table
//!   keeps the live tail").
//! - **[`SnapshotCompactor::gc`]** — op-log GC: prune `doc_op` rows `≤ snap_seq` **EXCEPT** those an
//!   open client's resume cursor still trails. The cursor is the **GC watermark** (arch §3) — a row at
//!   or below the lowest open cursor is retained so a reconnecting client never loses an op (KD-1
//!   survives compaction). The `op_seq` counter survives the prune (it is monotone, never reset).
//! - **[`SnapshotCompactor::reconstruct_at`]** — version-history read: reconstruct a page at a prior version (`op_seq`)
//!   from the **nearest snapshot at-or-below the target + the op-log tail up to the target**, and
//!   prove it is **byte-identical** to the pre-compaction materialised state at that version.
//!
//! ## The deterministic content-address (the determinism gate)
//! The snapshot content-address is `BLAKE3(materialize(ops ≤ snap_seq))` — derived from the
//! **materialised state**, which is a pure function of the ordered op sequence. So **the same state
//! compacts to the same content-address** (arch §3.4: "Deterministic ⇒ reproducible"). Two compactions
//! of the same `(aggregate, version)` — i.e. the same `(page_id, snap_seq)` over the same ops — mint
//! the SAME `blob_hash`. This is the determinism gate (see the module tests) and the property the KN-P20 replay
//! path relies on (a re-emitted snapshot is content-identical, so a derived store dedups it).
//!
//! ## FLOORS NAMED (VISION §3 — stubbed / deferred + the filling prompt)
//! - **fs-backed BlobStore for snapshots (11.2) is the M1 floor.** The compacted snapshot is stored
//!   in [`FsBlobStore`](myelin_storage::blob::FsBlobStore) (content-addressed, BLAKE3, per-tenant-keyed). The **object-store BlobStore
//!   swap is KN-P31 (M5)** — a one-line backing change behind the [`BlobStore`] trait (the trait is
//!   the seam; this module is unchanged). Named here in writing.
//! - **The per-subject DEK wrap of PII-bearing inline runs inside a snapshot (the crypto-shred unit
//!   boundary, 01 §3) is the BlobStore's [`ContentWrap`](myelin_storage::blob::ContentWrap) seam (identity on the M0 floor; the real DEK
//!   wrap is P-ST-08 / the GDPR M1 crypto-shred is KN-P26/P-316).** The content-address is computed
//!   from the PLAINTEXT materialised state (so it is stable across the wrap) — exactly the storage
//!   §3.2 "address by plaintext hash, store ciphertext" rule. Named here.
//! - **The compaction cadence triggers (op-count / quiescence / named-version, the measured KQ-4
//!   thresholds, 01 §3) are the caller's policy, not this module's.** This module is the MECHANISM
//!   (compact at a given `snap_seq`, GC below a watermark); the live relay decides WHEN to fire it.
//!   The named-version (non-GC'd restore-point) trigger rides the `named_label` field. Named here.
//!
//! ## Mutation floor (mandatory-core — TESTS field)
//! The COMPACTION-ROUND-TRIP path is mandatory-core: [`materialize`] (the deterministic projection),
//! [`SnapshotCompactor::compact`] (the materialise → content-address → store), and [`SnapshotCompactor::reconstruct_at`]
//! (snapshot + tail → byte-identical) — plus the [`SnapshotCompactor::gc`] watermark branch. The
//! stated floor is **100% mutation score on the compaction-round-trip + GC-watermark path**: every
//! arithmetic/comparison/branch mutant is killed by the round-trip byte-equality assertion (a mutated
//! `op_seq ≤ snap_seq` boundary, a swapped materialise order, a dropped watermark guard, or a flipped
//! determinism comparison all change the 0-mismatch / byte-identical / cursor-retained assertion).
//! Run: `cargo mutants -p myelin-knowledge -f compaction.rs`. The telemetry accessors / Display arms
//! are not core; the round-trip + GC-watermark path is — that is what the compaction + determinism
//! gates prove.

use myelin_storage::blob::{BlobStore, ContentHash};
use myelin_tenancy::TenantId;

use crate::transport::{DocOpLog, PageSnapshot, PersistedOp};

/// **A compacted-snapshot metadata row (the `doc_snapshot` row, 01 §3).** The content-addressed
/// pointer at the materialised state in the object tier (the BlobStore), plus the `snap_seq` it
/// includes up to and an optional `named_label`. PII-free: a `(page_id, snap_seq, blob_hash)` pointer
/// — never the materialised bytes (those live in the BlobStore, per-tenant-keyed + DEK-wrapped, K6).
///
/// This is the durable record a [`SnapshotCompactor::compact`] mints; a [`SnapshotCompactor::reconstruct_at`] reads it to
/// find the nearest snapshot at-or-below a target version, and the KN-P20 replay path re-emits the
/// referenced blob as a block-granular `knowledge.page.snapshot` (contract 2.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocSnapshot {
    /// The doc (aggregate) this snapshot materialises (the firehose `scope = doc:<page_id>`).
    pub page_id: String,
    /// **The `op_seq` this snapshot includes up to** (the version boundary; a `reconstruct_at(v)` uses
    /// the nearest snapshot with `snap_seq ≤ v`). The live tail is `op_seq > snap_seq`.
    pub snap_seq: u64,
    /// **The content-addressed (BLAKE3) snapshot blob handle** — `BLAKE3(materialize(ops ≤ snap_seq))`.
    /// Deterministic: the same `(page_id, snap_seq)` over the same ops mints the SAME hash. Opaque
    /// here (the bytes live in the BlobStore).
    pub blob_hash: ContentHash,
    /// `None` = an auto-compaction snapshot (GC-eligible once superseded); `Some(label)` = a named
    /// version (a non-GC'd restore point — user "save version" / publish, 01 §3). The named-version
    /// trigger is the caller's policy (NAMED floor).
    pub named_label: Option<String>,
}

impl DocSnapshot {
    /// Lower this snapshot to the transport's [`PageSnapshot`] seed shape (the `resync_required` cold
    /// seed, contract 2.6). The transport's [`crate::transport::CollabTransport::install_snapshot`]
    /// takes this to advance the resume cursor past the compacted range — the SAME snapshot serving
    /// the resync cold path AND the version-history restore point (the "one format, three masters",
    /// 01 §3). EI-01 §7 — one primitive (the snapshot the compactor mints IS the resync seed).
    pub fn as_page_snapshot(&self) -> PageSnapshot {
        PageSnapshot {
            snap_seq: self.snap_seq,
            blob_hash: self.blob_hash.to_multihash_string(),
        }
    }
}

/// **The deterministic materialised-state projection of a doc from a slice of its op-log (arch §2.2 —
/// the op-log is authoritative, the live state a projection).** Replays the ops (assumed already in
/// `op_seq` order, as the op-log keeps them) into ONE canonical byte string: a length-prefixed,
/// `op_seq`-ordered concatenation of each op's `(op_seq, op_id.wire, op_kind, payload)`.
///
/// **Deterministic** — the SAME ordered op sequence always materialises to the SAME bytes. This is the
/// precondition for a deterministic content-address (the same state ⇒ the same BLAKE3). The projection
/// is **engine-agnostic**: it does not interpret the opaque `payload` (CAS bytes now, Yrs bytes after
/// KN-P29) — it canonically frames the op-log itself, so the materialised state is a pure replay of
/// the authoritative log (the log-as-source-of-truth pattern, Kreps 2013).
///
/// Length-prefixing each variable-length field makes the framing **injective** (no two distinct op
/// sequences collide into the same bytes — a delimiter-free concat could), so byte-equality of the
/// materialised state is true state-equality (the round-trip gate's correctness rests on this).
pub fn materialize(ops: &[PersistedOp]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in ops {
        // op_seq (8 bytes, big-endian — the per-doc monotone cursor; fixed-width, no length prefix).
        out.extend_from_slice(&p.op_seq.to_be_bytes());
        // each variable-length field is length-prefixed (u32 BE) so the framing is injective.
        push_lp(&mut out, p.op.op_id.wire().as_bytes());
        push_lp(&mut out, p.op.kind.as_str().as_bytes());
        push_lp(&mut out, &p.op.payload);
    }
    out
}

/// Length-prefix a field (u32 big-endian length, then the bytes) into `out` — the injective framing
/// [`materialize`] relies on. A `u32` length is ample for a single op payload (bounded well under 4GiB).
fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// **The content-address of the materialised state at a version (the determinism gate's core).**
/// `BLAKE3(materialize(ops ≤ snap_seq))` — a pure function of the ordered op sequence, so the same
/// state always yields the same address (arch §3.4: "Deterministic ⇒ reproducible + replay-safe").
/// Computed from the PLAINTEXT materialised state (stable across the BlobStore's DEK wrap, storage
/// §3.2: "address by plaintext hash, store ciphertext"). A cited proven structure (BLAKE3) via the
/// frozen [`ContentHash::blake3`] — never a hand-rolled hash (VISION §4).
pub fn content_address(materialized: &[u8]) -> ContentHash {
    ContentHash::blake3(materialized)
}

/// **Why a compaction / reconstruction failed (the typed LOUD verdicts — never a silent gap).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompactionError {
    /// A `compact(up_to_seq)` named an `up_to_seq` beyond the op-log head (no such version exists yet).
    /// Carries the requested `up_to_seq` and the actual head.
    BeyondHead {
        /// The `up_to_seq` the caller asked to compact up to.
        requested: u64,
        /// The op-log's current head `op_seq` (the highest version that exists).
        head: u64,
    },
    /// A `reconstruct_at(target)` could not produce the version: the op-log tail it needed was GC'd
    /// below the nearest snapshot (a true gap — the snapshot the target needs was pruned). Carries the
    /// target version and the lowest `op_seq` still available. **This is NEVER a silent wrong answer**
    /// — a reconstruction that cannot be byte-exact errors LOUDLY (0 silent wrong-version serve).
    UnreconstructableGap {
        /// The target version that could not be reconstructed.
        target: u64,
        /// The lowest `op_seq` still in the op-log (the GC floor — ops below this were pruned).
        lowest_available: u64,
    },
    /// The BlobStore put/get failed (an integrity fail or a not-found on the snapshot blob). Carries
    /// the underlying blob-error message (a corrupt snapshot is refused, never silently served — the
    /// storage §3.2 re-hash-on-read integrity floor propagates here).
    Blob(String),
}

impl core::fmt::Display for CompactionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CompactionError::BeyondHead { requested, head } => write!(
                f,
                "cannot compact up to op_seq {requested}: beyond the op-log head {head}"
            ),
            CompactionError::UnreconstructableGap {
                target,
                lowest_available,
            } => write!(
                f,
                "cannot reconstruct version {target}: ops below {lowest_available} were GC'd \
                 (the snapshot the target needs was pruned) — refusing a non-exact reconstruction"
            ),
            CompactionError::Blob(e) => write!(f, "snapshot blob error: {e}"),
        }
    }
}

impl std::error::Error for CompactionError {}

/// **The op-log compactor (KN-P11) — the MECHANISM over one doc's [`DocOpLog`] + the content-addressed
/// [`BlobStore`].** Compacts an op-log range to a content-addressed snapshot (keeping the live tail),
/// GCs the compacted range below the open-cursor watermark, and reconstructs a prior version from a
/// snapshot + tail (byte-identically). Engine-agnostic: it frames the op-log, never interprets the op
/// payload (CAS now, Yrs after KN-P29), so it is unchanged across the `engine_promote` cutover.
///
/// The compactor pins ONE doc (`tenant`, `page_id`) to the BlobStore's per-tenant keyspace (the
/// snapshot blob is stored under `tenant`'s key path, K6). Generic over the [`BlobStore`] so the
/// fs-backed floor swaps for the object store (KN-P31) with no change here.
pub struct SnapshotCompactor<'b, B: BlobStore> {
    tenant: TenantId,
    page_id: String,
    blobs: &'b B,
}

impl<'b, B: BlobStore> SnapshotCompactor<'b, B> {
    /// Open a compactor for one doc over `blobs` (the fs-backed-floor BlobStore on M1; the object
    /// store on KN-P31). The `page_id` is the doc (aggregate) the snapshot materialises.
    pub fn new(
        tenant: TenantId,
        page_id: impl Into<String>,
        blobs: &'b B,
    ) -> SnapshotCompactor<'b, B> {
        SnapshotCompactor {
            tenant,
            page_id: page_id.into(),
            blobs,
        }
    }

    /// **Compact the op-log up to `up_to_seq` → a content-addressed snapshot (arch §3 / §3.4 step 1).**
    /// (1) materialise the doc's state from the ops `≤ up_to_seq` ([`materialize`] — deterministic);
    /// (2) compute the content-address (`BLAKE3` of the materialised state — the determinism gate);
    /// (3) store the bytes in the BlobStore (content-addressed, per-tenant-keyed, DEK-wrapped-on-store);
    /// (4) return the [`DocSnapshot`] metadata (the `doc_snapshot` row).
    ///
    /// **The live op-log tail (`op_seq > up_to_seq`) is UNTOUCHED** — compaction never prunes (that is
    /// [`Self::gc`], a separate watermark-guarded step). So a compact followed by a reconstruct of the
    /// pre-compaction state is byte-identical BEFORE any GC (the round-trip gate's first leg).
    ///
    /// `named_label = Some(..)` marks a non-GC'd named version (a restore point, 01 §3); `None` is an
    /// auto-compaction snapshot. Errors [`CompactionError::BeyondHead`] if `up_to_seq` exceeds the head
    /// (no such version exists — never a silent empty snapshot).
    pub fn compact(
        &self,
        log: &DocOpLog,
        up_to_seq: u64,
        named_label: Option<String>,
    ) -> Result<DocSnapshot, CompactionError> {
        if up_to_seq > log.head_seq() {
            return Err(CompactionError::BeyondHead {
                requested: up_to_seq,
                head: log.head_seq(),
            });
        }
        // (1) materialise the state from the ops up to (and including) up_to_seq — deterministic.
        let prefix = log.ops_up_to(up_to_seq);
        let materialized = materialize(&prefix);
        // (2) the content-address IS the BLAKE3 of the materialised state (computed pre-wrap, so it is
        // stable across the DEK wrap — storage §3.2). (3) store the bytes content-addressed.
        let blob_hash = self
            .blobs
            .put(&self.tenant, &materialized)
            .map_err(|e| CompactionError::Blob(e.to_string()))?;
        // The content-address the BlobStore returns MUST equal the address of the materialised state
        // (the put hashes the same plaintext) — a deterministic-address invariant, asserted in tests.
        Ok(DocSnapshot {
            page_id: self.page_id.clone(),
            snap_seq: up_to_seq,
            blob_hash,
            named_label,
        })
    }

    /// **Read back the materialised state a snapshot points at (the BlobStore get — re-hash-on-read
    /// verified, 0 silent corrupt serve, storage §3.2).** Returns the exact materialised bytes the
    /// snapshot was minted from. A corrupt snapshot blob is refused LOUDLY (the integrity error
    /// propagates as [`CompactionError::Blob`]) — never a silent wrong-state restore.
    pub fn load_snapshot_state(&self, snapshot: &DocSnapshot) -> Result<Vec<u8>, CompactionError> {
        self.blobs
            .get(&self.tenant, &snapshot.blob_hash)
            .map_err(|e| CompactionError::Blob(e.to_string()))
    }

    /// **Op-log GC: prune `doc_op` rows `≤ snap_seq` EXCEPT those an open client's resume cursor still
    /// trails (arch §3 — the cursor is the GC watermark that makes KD-1 survive compaction).** A row is
    /// retained iff `op_seq > min(snap_seq, lowest_open_cursor)`: the compacted range is GC-eligible,
    /// but a row at-or-above the lowest open cursor stays so a reconnecting client never loses an op.
    ///
    /// `open_cursors` is the set of resume cursors of currently-connected clients (each is the
    /// `last_durably_applied_op_seq` a client holds). The GC watermark is the LOWEST such cursor: a row
    /// with `op_seq ≤ watermark` is below every open client's resume point (no client still needs it),
    /// so it is safe to prune. With NO open clients the whole compacted range (`≤ snap_seq`) is pruned.
    ///
    /// **The `op_seq` counter survives the prune** (the [`DocOpLog`] `head_seq` is monotone, never
    /// reset by GC) — so a future op continues `head + 1`, and a `seed_from_snapshot` advances the
    /// cursor of a freshly-opened log past the pruned range. Returns the number of rows pruned.
    pub fn gc(&self, log: &mut DocOpLog, snap_seq: u64, open_cursors: &[u64]) -> usize {
        // The GC watermark: the lowest open client cursor caps how far the compacted range can be
        // pruned (a row above a still-open cursor is retained). With no open clients the cap is
        // snap_seq itself (the whole compacted range is GC-eligible).
        let watermark = match open_cursors.iter().copied().min() {
            Some(lowest_cursor) => snap_seq.min(lowest_cursor),
            None => snap_seq,
        };
        log.gc_below(watermark)
    }

    /// **Version-history read: reconstruct the doc's materialised state at version `target` (arch §3 —
    /// the snapshot is the history restore point).** From the nearest snapshot at-or-below `target`
    /// plus the op-log tail `(snap_seq, target]`, materialise the state at `target`. **Byte-identical**
    /// to the pre-compaction materialised state at `target` (the round-trip gate's second leg).
    ///
    /// `snapshots` is the doc's `doc_snapshot` rows (any order); the nearest at-or-below `target` is
    /// the seed. If a GC pruned the tail BELOW that snapshot's `snap_seq` the version is still
    /// reconstructable (the snapshot carries the materialised state up to `snap_seq`, and the tail
    /// `(snap_seq, target]` is above the GC floor by construction). If the tail needed is itself below
    /// the GC floor with NO covering snapshot, it errors [`CompactionError::UnreconstructableGap`] —
    /// 0 silent wrong-version serve.
    ///
    /// Returns the materialised bytes at `target` (compare byte-for-byte to [`materialize`] of the
    /// pre-compaction prefix to prove the round-trip).
    pub fn reconstruct_at(
        &self,
        log: &DocOpLog,
        snapshots: &[DocSnapshot],
        target: u64,
    ) -> Result<Vec<u8>, CompactionError> {
        // The nearest snapshot AT-OR-BELOW the target version (the seed); None if no snapshot covers it.
        let seed = snapshots
            .iter()
            .filter(|s| s.snap_seq <= target)
            .max_by_key(|s| s.snap_seq);

        match seed {
            Some(snapshot) => {
                // Seed = the snapshot's materialised state (read back from the BlobStore, integrity
                // verified). The tail (snap_seq, target] from the op-log is materialised + appended.
                let seed_state = self.load_snapshot_state(snapshot)?;
                let tail = log.ops_in_range(snapshot.snap_seq, target);
                // If the tail is missing rows the snapshot does NOT cover (a GC gap with no covering
                // snapshot for part of (snap_seq, target]), reconstruction is not byte-exact → refuse.
                self.guard_no_gap(log, snapshot.snap_seq, target)?;
                let mut state = seed_state;
                state.extend_from_slice(&materialize(&tail));
                Ok(state)
            }
            None => {
                // No snapshot covers the target → reconstruct purely from the op-log [0, target]. This
                // is byte-exact iff the op-log still holds every op ≤ target (no GC pruned below it).
                self.guard_no_gap(log, 0, target)?;
                let prefix = log.ops_up_to(target);
                Ok(materialize(&prefix))
            }
        }
    }

    /// Refuse a reconstruction that cannot be byte-exact: if the op-log was GC'd ABOVE `from` (so a row
    /// in `(from, target]` the reconstruction needs was pruned with no covering snapshot), error
    /// [`CompactionError::UnreconstructableGap`] rather than return a state missing ops (0 silent
    /// wrong-version serve). The reconstruction is exact iff every `op_seq` in `(from, target]` that
    /// EVER existed is still present — i.e. the op-log's lowest available op is `≤ from + 1` (the seed
    /// covers up to `from`; the tail must start contiguously right after it).
    fn guard_no_gap(&self, log: &DocOpLog, from: u64, target: u64) -> Result<(), CompactionError> {
        // The tail we need is (from, target]. The seed covers ≤ from. So the FIRST tail op we need is
        // op_seq = from + 1. If the op-log's lowest available op is ABOVE from + 1, a needed op was
        // GC'd with nothing covering it → a true gap.
        if target <= from {
            // Empty tail (target is within the seed) — always reconstructable.
            return Ok(());
        }
        let lowest_available = log.lowest_seq();
        // lowest_available == 0 means the log is empty (everything was GC'd); a non-empty log's lowest
        // op is its first retained op_seq. A needed op (from + 1) below the lowest available is a gap.
        let needed_first = from + 1;
        let gap = match lowest_available {
            // empty log: a gap iff we needed any tail op at all (we do — target > from here).
            0 => true,
            lowest => lowest > needed_first,
        };
        if gap {
            return Err(CompactionError::UnreconstructableGap {
                target,
                lowest_available,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{DocOp, OpId, OpKind};
    use myelin_storage::blob::FsBlobStore;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn op(client: &str, lamport: u64, kind: OpKind, payload: &str) -> DocOp {
        DocOp::cas(
            OpId::new(client, lamport),
            "actor-1",
            kind,
            payload.as_bytes().to_vec(),
        )
    }

    /// Build an op-log with `n` ops (op_seq 1..=n), each a distinct payload, and return it.
    fn log_with(n: u64) -> DocOpLog {
        let mut log = DocOpLog::new();
        for i in 1..=n {
            log.persist(op("c1", i, OpKind::Insert, &format!("edit-{i}")));
        }
        log
    }

    // ---- materialize is deterministic -----------------------------------------------------------

    /// **The materialised state is DETERMINISTIC (the precondition for a deterministic address).** The
    /// same ordered op sequence materialises to the SAME bytes, every time.
    #[test]
    fn materialize_is_deterministic() {
        let log = log_with(5);
        let prefix = log.ops_up_to(5);
        let a = materialize(&prefix);
        let b = materialize(&prefix);
        assert_eq!(
            a, b,
            "the same op sequence materialises to the same bytes (deterministic)"
        );
        assert!(
            !a.is_empty(),
            "a non-empty doc materialises to non-empty state"
        );
    }

    /// Distinct op sequences materialise to DISTINCT bytes (the framing is injective — no two states
    /// collide). A shorter prefix is not a prefix-collision of a longer one (the op_seq + length
    /// prefixes make it injective).
    #[test]
    fn distinct_states_materialize_distinctly() {
        let log = log_with(5);
        let s3 = materialize(&log.ops_up_to(3));
        let s5 = materialize(&log.ops_up_to(5));
        assert_ne!(
            s3, s5,
            "distinct versions materialise to distinct state bytes"
        );
    }

    /// **The op CONTENT is in the materialised state (the framing must carry op_id/kind/payload, not
    /// just the op_seq).** Two single-op logs at the SAME op_seq but DIFFERENT payloads materialise
    /// DIFFERENTLY — proving the per-field length-prefixed framing actually serialises the op content
    /// (a framing that dropped the fields would collide these two distinct states into the same bytes).
    #[test]
    fn materialize_carries_op_content_not_just_seq() {
        let mut a = DocOpLog::new();
        a.persist(op("c1", 1, OpKind::Insert, "alpha"));
        let mut b = DocOpLog::new();
        b.persist(op("c1", 1, OpKind::Insert, "omega")); // SAME op_seq (1), different payload
        let ma = materialize(&a.ops_up_to(1));
        let mb = materialize(&b.ops_up_to(1));
        assert_ne!(
            ma, mb,
            "same op_seq + different content must materialise differently (the content is framed)"
        );

        // And the injective framing prevents a field-boundary collision: ("inse","rt-x") vs
        // ("insert","-x")-style ambiguity is impossible because each field is length-prefixed.
        let mut c = DocOpLog::new();
        c.persist(op("ab", 1, OpKind::Insert, "cd")); // op_id wire "ab:1"
        let mut d = DocOpLog::new();
        d.persist(op("ab", 1, OpKind::Insert, "cd"));
        // d carries one MORE op whose fields could only "merge" into c's if framing were delimiter-free.
        d.persist(op("e", 1, OpKind::Insert, "")); // op_id wire "e:1"
        assert_ne!(
            materialize(&c.ops_up_to(2)),
            materialize(&d.ops_up_to(2)),
            "length-prefixed framing is injective — no field-boundary collision"
        );
    }

    // ---- the snapshot-determinism gate (the same state → the same content-address) --------------

    /// **THE SNAPSHOT-DETERMINISM GATE: the same state compacts to the same content-address (BLAKE3).**
    /// Two independent compactions of the SAME `(page_id, snap_seq)` over the SAME ops mint the SAME
    /// `blob_hash` — the deterministic-address property the KN-P20 replay path relies on (a re-emitted
    /// snapshot is content-identical). The green artifact: the two addresses are equal.
    #[test]
    fn snapshot_determinism_same_state_same_content_address() {
        let log = log_with(6);
        let blobs_a = FsBlobStore::new();
        let blobs_b = FsBlobStore::new();
        let comp_a = SnapshotCompactor::new(tenant(), "page-1", &blobs_a);
        let comp_b = SnapshotCompactor::new(tenant(), "page-1", &blobs_b);

        let snap_a = comp_a.compact(&log, 4, None).expect("compact a");
        let snap_b = comp_b.compact(&log, 4, None).expect("compact b");

        assert_eq!(
            snap_a.blob_hash, snap_b.blob_hash,
            "the same state (page-1 up to op_seq 4) mints the SAME content-address (determinism gate)"
        );
        // And the address IS the BLAKE3 of the materialised state (the content address IS the hash).
        assert_eq!(
            snap_a.blob_hash,
            content_address(&materialize(&log.ops_up_to(4))),
            "the content-address is BLAKE3(materialised state)"
        );
    }

    /// A DIFFERENT version (different `snap_seq`) of the same doc mints a DIFFERENT content-address —
    /// the address is content-derived, so a different state is a different address (no false dedup).
    #[test]
    fn different_versions_get_different_content_addresses() {
        let log = log_with(6);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let s3 = comp.compact(&log, 3, None).expect("compact 3");
        let s5 = comp.compact(&log, 5, None).expect("compact 5");
        assert_ne!(
            s3.blob_hash, s5.blob_hash,
            "different versions of the doc mint different content-addresses"
        );
    }

    // ---- the compaction-round-trip gate (compact → GC → reconstruct == byte-identical) ----------

    /// **THE COMPACTION-ROUND-TRIP GATE (0 mismatches): compact a range → GC it → reconstruct from the
    /// snapshot + tail → byte-identical to the pre-compaction state.** This is the headline gate. We
    /// capture the pre-compaction materialised state at version 4, compact up to 4, GC the range (no
    /// open clients → the whole compacted range is pruned), then reconstruct version 4 from the
    /// snapshot + tail and assert it is BYTE-IDENTICAL. The compaction-round-trip counter is the number
    /// of mismatches — here 0.
    #[test]
    fn compaction_round_trip_is_byte_identical_after_gc() {
        let mut log = log_with(8); // op_seq 1..=8; we'll snapshot up to 4, keep the tail 5..=8.
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);

        // The PRE-compaction materialised state at version 4 (the ground truth to round-trip against).
        let pre_compaction_v4 = materialize(&log.ops_up_to(4));

        // Compact up to op_seq 4 → a content-addressed snapshot; the live tail (5..=8) is KEPT.
        let snapshot = comp.compact(&log, 4, None).expect("compact up to 4");
        assert_eq!(snapshot.snap_seq, 4);
        assert_eq!(
            log.head_seq(),
            8,
            "compaction did NOT touch the op_seq counter"
        );
        assert_eq!(
            log.len(),
            8,
            "compaction did NOT prune the op-log (that is GC)"
        );

        // GC the compacted range with NO open clients → prune doc_op rows ≤ 4 (the whole range).
        let pruned = comp.gc(&mut log, 4, &[]);
        assert_eq!(pruned, 4, "the 4 compacted rows (op_seq 1..=4) were GC'd");
        assert_eq!(log.len(), 4, "only the live tail (op_seq 5..=8) remains");
        assert_eq!(
            log.head_seq(),
            8,
            "the op_seq counter SURVIVED the prune (monotone)"
        );

        // Reconstruct version 4 from the snapshot + the (now-pruned) tail → byte-identical.
        let reconstructed = comp
            .reconstruct_at(&log, std::slice::from_ref(&snapshot), 4)
            .expect("reconstruct version 4 from snapshot + tail");
        let mismatches = if reconstructed == pre_compaction_v4 {
            0
        } else {
            1
        };
        assert_eq!(
            mismatches, 0,
            "COMPACTION-ROUND-TRIP: reconstructed version 4 is byte-identical to pre-compaction \
             (0 mismatches)"
        );
    }

    /// **Reconstruct a LATER version (in the live tail) from the snapshot + the kept tail.** After
    /// compacting up to 4 and GC'ing the range, version 7 (in the tail) reconstructs as snapshot(4) +
    /// tail(5..=7), byte-identical to the pre-compaction state at 7.
    #[test]
    fn reconstruct_a_tail_version_from_snapshot_plus_tail() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let pre_v7 = materialize(&log.ops_up_to(7));

        let snapshot = comp.compact(&log, 4, None).expect("compact 4");
        comp.gc(&mut log, 4, &[]);

        let v7 = comp
            .reconstruct_at(&log, std::slice::from_ref(&snapshot), 7)
            .expect("reconstruct version 7");
        assert_eq!(
            v7, pre_v7,
            "version 7 = snapshot(4) + tail(5..=7), byte-identical"
        );
    }

    /// **A GC'd range is reconstructable from the snapshot (the prompt's explicit TEST).** After GC,
    /// the op-log no longer holds op_seq 1..=4; reconstructing version 4 STILL succeeds + is exact
    /// because the snapshot carries that state — the snapshot is the durable record of the pruned range.
    #[test]
    fn a_gcd_range_is_reconstructable_from_the_snapshot() {
        let mut log = log_with(6);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let pre_v4 = materialize(&log.ops_up_to(4));

        let snapshot = comp.compact(&log, 4, None).expect("compact 4");
        comp.gc(&mut log, 4, &[]);
        assert!(
            log.ops_up_to(4).iter().all(|p| p.op_seq > 4),
            "the range ≤ 4 was pruned from the log"
        );

        // Even though the op-log no longer holds ≤ 4, the snapshot reconstructs version 4 exactly.
        let v4 = comp
            .reconstruct_at(&log, std::slice::from_ref(&snapshot), 4)
            .expect("the GC'd range is reconstructable from the snapshot");
        assert_eq!(
            v4, pre_v4,
            "a GC'd range reconstructs byte-identically from the snapshot"
        );
    }

    // ---- the GC watermark (the open-cursor rule that makes KD-1 survive compaction) -------------

    /// **The open-cursor watermark RETAINS rows a connected client still trails (arch §3 — KD-1 survives
    /// compaction).** With an open client at cursor 2, GC'ing the compacted range ≤ 4 prunes only rows
    /// ≤ 2 (below the open cursor) — rows 3, 4 are RETAINED so the client at cursor 2 can still resume
    /// (3, now] without a resync. This is the rule that makes a reconnect lose 0 ops across compaction.
    #[test]
    fn gc_watermark_retains_rows_an_open_cursor_still_trails() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        comp.compact(&log, 4, None).expect("compact 4");

        // An open client holds cursor 2 (it has applied up to op_seq 2). GC the compacted range ≤ 4.
        let pruned = comp.gc(&mut log, 4, &[2]);
        assert_eq!(
            pruned, 2,
            "only rows ≤ 2 (below the open cursor) are pruned"
        );
        // rows 3, 4 are RETAINED (the open client at cursor 2 still needs (2, now]).
        let remaining: Vec<u64> = log.ops_up_to(8).iter().map(|p| p.op_seq).collect();
        assert_eq!(
            remaining,
            vec![3, 4, 5, 6, 7, 8],
            "rows the open cursor trails are retained"
        );

        // The client at cursor 2 can still resume (2, now] = {3..8} — 0 ops lost across compaction.
        let resumed: Vec<u64> = log.ops_since(2).iter().map(|p| p.op_seq).collect();
        assert_eq!(
            resumed,
            vec![3, 4, 5, 6, 7, 8],
            "the open client resumes with 0 ops lost (KD-1)"
        );
    }

    /// The LOWEST open cursor is the watermark (multiple clients): with cursors {5, 2, 6}, the
    /// watermark is min(snap_seq=4, 2) = 2 → only rows ≤ 2 prune (the most-behind client is protected).
    #[test]
    fn gc_watermark_is_the_lowest_open_cursor() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        comp.compact(&log, 4, None).expect("compact 4");
        let pruned = comp.gc(&mut log, 4, &[5, 2, 6]);
        assert_eq!(
            pruned, 2,
            "the lowest cursor (2) is the watermark — the most-behind client wins"
        );
    }

    /// With NO open clients the WHOLE compacted range (≤ snap_seq) is GC'd (no cursor to protect).
    #[test]
    fn gc_with_no_open_clients_prunes_the_whole_compacted_range() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        comp.compact(&log, 4, None).expect("compact 4");
        let pruned = comp.gc(&mut log, 4, &[]);
        assert_eq!(
            pruned, 4,
            "no open client → the whole compacted range ≤ 4 is pruned"
        );
        assert_eq!(log.len(), 4, "the live tail 5..=8 remains");
    }

    // ---- LOUD failure (never a silent wrong answer) ---------------------------------------------

    /// **A compact beyond the head errors LOUDLY (never a silent empty snapshot).** And the BOUNDARY:
    /// compacting EXACTLY at the head (`up_to_seq == head`) SUCCEEDS — it snapshots the whole doc (the
    /// `>` boundary, not `>=`: at-head is the common "compact everything" case, never an error).
    #[test]
    fn compact_beyond_head_errors_loudly() {
        let log = log_with(3);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);

        // One BEYOND the head → loud error.
        let r = comp.compact(&log, 9, None);
        assert!(matches!(
            r,
            Err(CompactionError::BeyondHead {
                requested: 9,
                head: 3
            })
        ));

        // EXACTLY at the head → succeeds (compact the whole doc up to op_seq 3). This pins the `>`
        // boundary: at-head is valid, not BeyondHead.
        let at_head = comp
            .compact(&log, 3, None)
            .expect("compacting exactly at head succeeds");
        assert_eq!(
            at_head.snap_seq, 3,
            "the at-head snapshot covers the whole doc"
        );
        assert_eq!(
            at_head.blob_hash,
            content_address(&materialize(&log.ops_up_to(3))),
            "the at-head snapshot is the BLAKE3 of the whole materialised state"
        );
    }

    /// **An unreconstructable gap errors LOUDLY (0 silent wrong-version serve).** If the op-log was GC'd
    /// above a version with NO covering snapshot, reconstructing that version refuses rather than return
    /// a state missing ops. Here we GC ≤ 4 with no snapshot at all, then try to reconstruct version 3.
    #[test]
    fn reconstruct_into_a_pruned_gap_errors_loudly() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        // GC the range ≤ 4 (no open clients) WITHOUT keeping a snapshot row for the reconstruct.
        comp.compact(&log, 4, None).expect("compact 4"); // snapshot minted but NOT passed to reconstruct
        comp.gc(&mut log, 4, &[]);
        // Reconstruct version 3 with NO snapshots provided → the ops ≤ 3 are gone, no covering snapshot.
        let r = comp.reconstruct_at(&log, &[], 3);
        assert!(
            matches!(r, Err(CompactionError::UnreconstructableGap { target: 3, .. })),
            "a pruned version with no covering snapshot refuses LOUDLY (0 silent wrong-version serve)"
        );
    }

    /// **A corrupt snapshot blob is refused on reconstruct (storage §3.2 re-hash-on-read propagates).**
    /// Corrupting the snapshot's stored bytes makes the BlobStore get fail integrity → the reconstruct
    /// errors `Blob` rather than restore a wrong state (0 silent corrupt restore).
    #[test]
    fn reconstruct_refuses_a_corrupt_snapshot_blob() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let snapshot = comp.compact(&log, 4, None).expect("compact 4");
        comp.gc(&mut log, 4, &[]);

        // Corrupt the snapshot's stored object (bit-rot / tamper).
        assert!(
            blobs.corrupt_for_drill(&tenant(), &snapshot.blob_hash),
            "snapshot blob present"
        );

        let r = comp.reconstruct_at(&log, std::slice::from_ref(&snapshot), 4);
        assert!(
            matches!(r, Err(CompactionError::Blob(_))),
            "a corrupt snapshot blob is refused (0 silent corrupt restore)"
        );
    }

    // ---- the snapshot lowers to the transport's resync seed (one format, three masters) ----------

    /// **The compacted snapshot lowers to the transport's `PageSnapshot` resync seed (01 §3 — one
    /// format serving the resync cold path AND the history restore point).** The `DocSnapshot` the
    /// compactor mints IS the seed the transport installs to advance its resume cursor past the
    /// compacted range (EI-01 §7 — one primitive).
    #[test]
    fn snapshot_lowers_to_the_transport_resync_seed() {
        let log = log_with(6);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let snapshot = comp
            .compact(&log, 4, Some("v1.0".into()))
            .expect("named compact");
        assert_eq!(
            snapshot.named_label.as_deref(),
            Some("v1.0"),
            "a named version (restore point)"
        );

        let seed = snapshot.as_page_snapshot();
        assert_eq!(
            seed.snap_seq, 4,
            "the resync seed carries the snapshot's snap_seq"
        );
        assert_eq!(
            seed.blob_hash,
            snapshot.blob_hash.to_multihash_string(),
            "the resync seed points at the SAME content-addressed blob (one format)"
        );
    }

    /// Errors render loud + specific (diagnosable, EI-01 §3).
    #[test]
    fn errors_display_loud_and_specific() {
        assert!(CompactionError::BeyondHead {
            requested: 9,
            head: 3
        }
        .to_string()
        .contains("beyond the op-log head"));
        assert!(CompactionError::UnreconstructableGap {
            target: 3,
            lowest_available: 5
        }
        .to_string()
        .contains("refusing a non-exact reconstruction"));
        assert!(CompactionError::Blob("boom".into())
            .to_string()
            .contains("snapshot blob error"));
    }
}
