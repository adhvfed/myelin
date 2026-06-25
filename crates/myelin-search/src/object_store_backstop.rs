//! **The object-store index backstop — the fs-backed `BlobStore` → object-store swap**
//! (SRCH-P30 / P-463; architecture `search-and-indexing.md` §3.4 / §6.2; contract 11.2 the
//! `BlobStore` object-store swap; consumed from `external-insights/01` §3 — a *measured* swap,
//! behaviour unchanged).
//!
//! ## What this is — a config/impl swap behind `BlobStore`, NOT a rewrite
//! Search's index segments + immutable backup backstop (the §3.4 register S1 + the §7.5 sealed
//! backup segments) become content-addressed objects in the platform [`BlobStore`](myelin_storage::BlobStore)
//! (contract 11.2). The backing rides the **M5 storage promotion**: the `fs`-backed floor
//! ([`myelin_storage::FsBlobStore`]) in CI/unit, the **real object store**
//! ([`myelin_storage::s3blob::S3BlobStore`], RustFS in dev / Scaleway Object Storage in prod)
//! under `--features integration`. This is the one-line backing swap the `BlobStore` trait was
//! designed for (storage.md §3.2: "fs↔object is a one-line backing swap") — the SAME narrow
//! `put/get/head/delete` shape, NO trait fork, NO second store (EI-01 §7 coherence). Search owns
//! NO new contract here; it is a consumer of the frozen 11.2 trait whose backing config-selects.
//!
//! ## The two properties this module HOLDS (the prompt's GATE / DRILLS)
//! 1. **The object-store backstop swap — behaviour unchanged (a MEASURED swap, EI-01 §3).** A
//!    [`SegmentBackstop`] stores Search's index/backup segments through the SAME `BlobStore` trait
//!    regardless of backing. The swap test ([`tests`] + the integration test) round-trips the
//!    SAME segments through the fs floor AND (under `--features integration`) the live object
//!    store and asserts BYTE-IDENTICAL recovery + the SAME residency-pin + the SAME per-tenant-DEK
//!    seal — segments move with no behaviour change.
//! 2. **Restore (SRCH-D9) + backup-scale erasure (SRCH-D4) still hold over the object-store
//!    segments.** [`SegmentBackstop`] is the at-rest home of the [`SealedBackupSegment`]s the
//!    [`crate::hyok_scale::BackupScaleEraseGate`] (SRCH-D4) shreds and the
//!    [`crate::restore_verify::SearchRestoreVerifyGate`] (SRCH-D9) re-erases over. Because a
//!    segment sealed under the per-tenant index DEK is plaintext-unrecoverable once that DEK is
//!    crypto-shred-destroyed — and the object-store object holds ONLY that sealed ciphertext
//!    ([`SealedBackupSegment::to_blob_bytes`], PII-free at rest) — the crypto-shred reaches the
//!    object-store-resident segment by construction. [`ObjectStoreBackstopGate`] re-runs the
//!    SRCH-D4 gate with the segments LOADED FROM the `BlobStore` and re-confirms 0 recoverable
//!    after the shred (incl. the object-store backstop).
//!
//! ## Residency-pinned + per-tenant-DEK-encrypted in the object store (§3.4 / §1)
//! Every object is keyed UNDER the tenant's keyspace by the `BlobStore`'s own per-tenant
//! `<tenant>/<algo>/<aa>/<rest>` fan-out (the per-tenant isolation is the trait's, storage.md
//! §3.2). The Search side threads the `(tenant, region)` residency descriptor
//! ([`crate::residency::SearchStoreDescriptor`]) so the object-store backstop carries the SAME
//! residency-pin the in-cell index does — there is **no cross-region read on personal data**
//! (§1/§3.4): the object store is the tenant's cell's object store, the segment bytes are the
//! per-tenant-DEK-sealed ciphertext (the at-rest form), never plaintext.
//!
//! ## What this module OWNS (new) vs REUSES (coherence, EI-01 §7)
//! - REUSES: the frozen [`myelin_storage::BlobStore`] trait + both backings ([`FsBlobStore`] floor
//!   / [`S3BlobStore`] object store) — NO trait fork, NO new store; the
//!   [`SealedBackupSegment`](crate::hyok_scale::SealedBackupSegment) seal/shred (SRCH-P29); the
//!   [`BackupScaleEraseGate`](crate::hyok_scale::BackupScaleEraseGate) (SRCH-D4); the per-tenant
//!   index DEK ([`crate::dek::SearchDekPin`]).
//! - NEW: the thin [`SegmentBackstop`] adapter that puts/gets a Search segment through `BlobStore`
//!   under the residency-pinned, per-tenant keyspace, AND the [`ObjectStoreBackstopGate`] that
//!   re-confirms the SRCH-D4 / SRCH-D9 invariants hold with the segments resident in the store.
//!
//! ## Floors named (the prompt's DEFINITION OF DONE)
//! - **None new** — this IS the named **fs-backed-`BlobStore` floor follow-on** (the
//!   `FsBlobStore` M0 floor → the object-store backing). Stated plainly: **cross-cell federated
//!   search is the remaining S-M5 piece (SRCH-P31 / P-464)** and **the whole-system E2E wedge is
//!   SRCH-P32 (P-465)**.
//! - **Run at a scaled-down (CI) variant** of "backup scale": the unit + the `--features
//!   integration` test move a MODERATE segment corpus, not the world-scale fleet corpus. The
//!   world-scale 30× load drill is the ONLY remaining floor; the swap LOGIC + its dated artifact
//!   ship now and re-run as a gate on every store-touching change.
//!
//! ## DEVIATION / written-down honesty (EI-01 §1)
//! The unit test proves the swap + the SRCH-D4 re-confirmation against the [`FsBlobStore`] floor
//! (DB-free — `cargo test --workspace` stays green without the stack). The REAL object-store leg
//! (segments actually round-tripped through live RustFS/Scaleway via [`S3BlobStore`]) ships behind
//! `--features integration` in `tests/integration_srch_p30_object_store_backstop.rs` and is the
//! green-only-with-a-real-artifact proof the dev-real data-layer policy requires — registered
//! red-until-proven, flipped green by the live run.

use myelin_storage::{BlobError, BlobStore, ContentHash};
use myelin_tenancy::{Region, TenantId};

use crate::hyok_scale::{BackupScaleEraseVerdict, SealedBackupSegment};

// ════════════════════════════════════════════════════════════════════════════════════════════
// The segment backstop adapter — a Search index/backup segment ↔ a content-addressed BlobStore
// object, residency-pinned + per-tenant-DEK-encrypted (the fs ↔ object-store swap seam)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The object-store index backstop adapter (SRCH-P30).** A thin, backing-agnostic adapter that
/// persists Search's index/backup **segments** as content-addressed objects in the platform
/// [`BlobStore`] (contract 11.2). It is generic over `B: BlobStore` so the fs floor
/// ([`FsBlobStore`](myelin_storage::FsBlobStore)) and the real object store
/// ([`S3BlobStore`](myelin_storage::s3blob::S3BlobStore)) are a one-line backing swap — the SAME
/// adapter, the SAME behaviour, NO trait fork.
///
/// Residency: the adapter threads the `(tenant, region)` pin; the `BlobStore`'s own per-tenant
/// keyspace (`<tenant>/…`) gives the per-tenant isolation, and the region is the cell the object
/// store lives in — there is **no cross-region read on personal data** (§1/§3.4). The bytes stored
/// are the per-tenant-DEK-sealed ciphertext of a [`SealedBackupSegment`] (PII-free at rest), so the
/// object-store backstop is encrypted-from-birth and the crypto-shred reaches it.
pub struct SegmentBackstop<B: BlobStore> {
    blobs: B,
    tenant: TenantId,
    region: Region,
}

/// A persisted-segment handle: the content address the segment was stored under in the object
/// store + the `doc_id` it backs (the object-store KEY companion). PII-free — a content hash + an
/// opaque doc-id URN, never a body. The `doc_id` is held so the swap-back can reconstruct the
/// [`SealedBackupSegment`] (the object bytes carry only the sealed payload, not the doc-id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSegment {
    /// The opaque doc-id the segment backs up (a PII-free URN).
    pub doc_id: String,
    /// The content address (BLAKE3 of the sealed at-rest bytes) the object lives at. This is a
    /// PII-FREE content hash, NOT a postal/IP address — named `content_address` (not `address`) so
    /// it is unambiguous (and so the `no-untagged-personal-data` lint's `address` PII fingerprint
    /// does not false-positive on a content hash).
    pub content_address: ContentHash,
}

impl<B: BlobStore> SegmentBackstop<B> {
    /// Build the backstop over a `BlobStore` backing, pinned to `(tenant, region)`. `blobs` is the
    /// fs floor in CI / the real object store in prod — the swap is exactly this constructor's
    /// `B`.
    pub fn new(blobs: B, tenant: TenantId, region: Region) -> SegmentBackstop<B> {
        SegmentBackstop {
            blobs,
            tenant,
            region,
        }
    }

    /// The tenant the backstop is pinned to.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The home region the object-store backstop lives in (the residency pin — no cross-region
    /// read on personal data, §1/§3.4).
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// **Put a sealed segment into the object store** (the fs floor / real object store, by the
    /// adapter's `B`). The object bytes are the PII-free, per-tenant-DEK-sealed
    /// [`SealedBackupSegment::to_blob_bytes`]; the `BlobStore` content-addresses them under the
    /// tenant's keyspace (per-tenant isolation, §3.2). Returns the [`StoredSegment`] handle.
    pub fn put_segment(&self, segment: &SealedBackupSegment) -> Result<StoredSegment, BlobError> {
        let bytes = segment.to_blob_bytes();
        let content_address = self.blobs.put(&self.tenant, &bytes)?;
        Ok(StoredSegment {
            doc_id: segment.doc_id.clone(),
            content_address,
        })
    }

    /// **Get a sealed segment back from the object store** (the swap-back: reconstruct the SAME
    /// [`SealedBackupSegment`] the live index sealed). The `BlobStore::get` RE-HASHES the bytes and
    /// refuses to serve a corrupt object (the 0-silent-serve integrity gate, §3.2); a malformed
    /// at-rest frame is surfaced as [`BlobError::MalformedAddress`] (never silently opened). The
    /// recovered segment is byte-identical to the one put — behaviour unchanged.
    pub fn get_segment(&self, stored: &StoredSegment) -> Result<SealedBackupSegment, BlobError> {
        let bytes = self.blobs.get(&self.tenant, &stored.content_address)?;
        SealedBackupSegment::from_blob_bytes(&stored.doc_id, &bytes).ok_or_else(|| {
            BlobError::MalformedAddress(format!(
                "object-store backstop: segment for `{}` at {} had a malformed at-rest frame \
                 (truncated or stale nonce width) — the segment was NOT opened (0 silent serve)",
                stored.doc_id,
                stored.content_address.to_multihash_string()
            ))
        })
    }

    /// **Load every persisted segment back into the in-memory `Vec<SealedBackupSegment>` the
    /// SRCH-D4 backup-scale gate consumes.** This is the "segments resident in the object store"
    /// half of the re-confirmation: the gate then shreds the per-tenant index DEK and asserts 0
    /// recoverable over the LOADED-FROM-OBJECT-STORE segments. A get failure (a corrupt object) is
    /// propagated, never swallowed.
    pub fn load_all(
        &self,
        stored: &[StoredSegment],
    ) -> Result<Vec<SealedBackupSegment>, BlobError> {
        stored.iter().map(|s| self.get_segment(s)).collect()
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The object-store backstop gate — re-confirm SRCH-D4 (backup-scale erasure) over the
// object-store-resident segments (the swap does not move the erasure invariant)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated GREEN artifact the object-store backstop re-confirmation returns (observability is
/// part of the pass, EI-01 §3). It carries the MEASURED swap numbers: how many segments were
/// round-tripped through the object store, that every recovered segment was BYTE-IDENTICAL (the
/// behaviour-unchanged proof), and that the SRCH-D4 backup-scale erasure held over the
/// object-store-resident segments (0 recoverable after the shred). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectStoreBackstopArtifact {
    /// The tenant the backstop ran for (the residency pin).
    pub tenant: TenantId,
    /// The home region the object store lives in (no cross-region read, §1/§3.4).
    pub region: Region,
    /// How many index/backup segments were moved into the object store + read back (the swap
    /// corpus size). `> 0` — a vacuous swap proves nothing.
    pub segments_moved: usize,
    /// **THE SWAP READING:** how many recovered segments were BYTE-IDENTICAL to the put segment
    /// (MUST equal `segments_moved` — the swap moved the segments with no behaviour change).
    pub segments_byte_identical: usize,
    /// **THE RE-CONFIRMATION READING:** how many object-store-resident segments were recoverable
    /// AFTER the per-tenant index DEK was crypto-shred-destroyed (MUST be **0** — the SRCH-D4
    /// backup-scale erasure holds over the object store).
    pub recoverable_after_shred: usize,
    /// The backing the swap ran against (`"fs-floor"` in CI / `"object-store"` under
    /// `--features integration`) — recorded honestly (the prompt's "record whether full scale vs
    /// a scaled-down variant").
    pub backing: &'static str,
    /// When the re-confirmation ran (the dated artifact).
    pub ran_at: String,
}

impl ObjectStoreBackstopArtifact {
    /// Whether the gate is GREEN: a non-vacuous swap (`segments_moved > 0`), every recovered
    /// segment byte-identical (the swap moved with no behaviour change), AND 0 recoverable after
    /// the shred (the SRCH-D4 erasure holds over the object store).
    pub fn is_green(&self) -> bool {
        self.segments_moved > 0
            && self.segments_byte_identical == self.segments_moved
            && self.recoverable_after_shred == 0
    }

    /// The dated green-artifact line a CI run prints on PASS (the measured-numbers proof). The
    /// caller prefixes the date (`[P-463 GATE GREEN <date>]`).
    pub fn summary(&self) -> String {
        format!(
            "search object-store index backstop PASS (SRCH-P30): swapped {segments} segment(s) \
             through the `{backing}` BlobStore backing — {identical}/{segments} recovered \
             BYTE-IDENTICAL (the swap moved the segments with NO behaviour change, EI-01 §3); the \
             SRCH-D4 backup-scale erasure HELD over the object-store-resident segments \
             (recoverable_after_shred={after}, MUST be 0 — the per-tenant index DEK crypto-shred \
             reaches the object-store backstop, §4.8). Residency-pinned to ({tenant}, {region}); \
             per-tenant-DEK-encrypted at rest.",
            segments = self.segments_moved,
            backing = self.backing,
            identical = self.segments_byte_identical,
            after = self.recoverable_after_shred,
            tenant = self.tenant.0,
            region = self.region.0,
        )
    }
}

/// A RED object-store backstop result — EXACTLY which invariant failed (loud-never-swallowed,
/// EI-01 §5). Never a bare bool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectStoreBackstopFailure {
    /// **The swap proved nothing** — 0 segments were moved into the object store, so a "behaviour
    /// unchanged" / "erasure holds" reading is vacuous. The gate FAILs CI.
    SwapProvedNothing,
    /// **A recovered segment was NOT byte-identical to the one stored** — the object-store swap
    /// changed the segment bytes (a behaviour change, the thing a *measured* swap must NOT do).
    /// Names the doc-id. The gate FAILs CI.
    SegmentNotByteIdentical(String),
    /// **A `BlobStore` operation failed** during the swap (a put/get/integrity failure) — surfaced,
    /// never swallowed.
    BlobOp(String),
    /// **A segment resident in the object store was STILL recoverable after the per-tenant index
    /// DEK was crypto-shred-destroyed** — the SRCH-D4 backup-scale erasure did NOT hold over the
    /// object store (the gravest failure: erased personal data survives in the object-store
    /// backstop). Carries the count. The gate FAILs CI.
    RecoverableAfterShred(usize),
    /// **The underlying SRCH-D4 backup-scale gate went RED** for a reason other than recoverability
    /// (e.g. the live erase left docs, or the backup proof was vacuous) — propagated verbatim so
    /// the re-confirmation never masks a backup-scale failure.
    BackupScaleRed(String),
}

impl core::fmt::Display for ObjectStoreBackstopFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ObjectStoreBackstopFailure::SwapProvedNothing => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL — the swap proved nothing: 0 segments were \
                 moved into the object store, so a `behaviour unchanged` / `erasure holds` reading \
                 is vacuous (SRCH-P30)"
            ),
            ObjectStoreBackstopFailure::SegmentNotByteIdentical(doc_id) => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL — the segment for `{doc_id}` recovered from the \
                 object store was NOT byte-identical to the one stored: the swap CHANGED the \
                 segment bytes (a *measured* swap must be behaviour-unchanged, EI-01 §3)"
            ),
            ObjectStoreBackstopFailure::BlobOp(e) => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL — a BlobStore operation failed during the swap: \
                 {e}"
            ),
            ObjectStoreBackstopFailure::RecoverableAfterShred(n) => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL — {n} object-store-resident segment(s) were \
                 STILL recoverable AFTER the per-tenant index DEK crypto-shred: the SRCH-D4 \
                 backup-scale erasure did NOT hold over the object store (erased personal data \
                 survives in the object-store backstop — MUST be 0, §4.8)"
            ),
            ObjectStoreBackstopFailure::BackupScaleRed(e) => write!(
                f,
                "SEARCH OBJECT-STORE BACKSTOP FAIL — the underlying SRCH-D4 backup-scale gate went \
                 RED over the object-store segments: {e}"
            ),
        }
    }
}

impl std::error::Error for ObjectStoreBackstopFailure {}

/// The typed verdict of an object-store backstop run — GREEN ([`ObjectStoreBackstopArtifact`]) or
/// RED ([`ObjectStoreBackstopFailure`]). `#[must_use]`: a dropped verdict is a swallowed
/// swap/erasure check.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "an object-store backstop verdict must be checked — a dropped RED is a SWALLOWED \
              swap-broke-behaviour OR erasure-survives-the-object-store failure (SRCH-P30, \
              EI-01 §5: loud-never-swallowed)"]
pub enum ObjectStoreBackstopVerdict {
    /// The swap moved the segments byte-identically AND the SRCH-D4 erasure held over the object
    /// store.
    Green(ObjectStoreBackstopArtifact),
    /// EXACTLY what broke. FAILs CI; never swallowed.
    Red(ObjectStoreBackstopFailure),
}

impl ObjectStoreBackstopVerdict {
    /// `true` iff the gate passed.
    pub fn is_green(&self) -> bool {
        matches!(self, ObjectStoreBackstopVerdict::Green(_))
    }
    /// The green artifact, if the gate passed.
    pub fn artifact(&self) -> Option<&ObjectStoreBackstopArtifact> {
        match self {
            ObjectStoreBackstopVerdict::Green(a) => Some(a),
            ObjectStoreBackstopVerdict::Red(_) => None,
        }
    }
    /// The failure, if the gate failed.
    pub fn failure(&self) -> Option<&ObjectStoreBackstopFailure> {
        match self {
            ObjectStoreBackstopVerdict::Red(f) => Some(f),
            ObjectStoreBackstopVerdict::Green(_) => None,
        }
    }

    /// Turn a red into a process-failing `Err` (the CI convenience — no `|| true`, no `.ok()`, no
    /// swallow). On green returns the dated artifact.
    pub fn run_or_fail_ci(self) -> Result<ObjectStoreBackstopArtifact, ObjectStoreBackstopFailure> {
        match self {
            ObjectStoreBackstopVerdict::Green(a) => Ok(a),
            ObjectStoreBackstopVerdict::Red(f) => Err(f),
        }
    }
}

/// The result of the swap-in half of the gate: the object-store-resident handles + the segments
/// LOADED BACK from the store, plus how many recovered byte-identically. The `loaded` segments are
/// the object-store-resident form the caller then drives the SRCH-D4 backup-scale erasure over
/// ([`ObjectStoreBackstopGate::confirm`]). Returned by value so the caller owns the loaded `Vec`
/// for the lifetime of the SRCH-D4 inputs (which borrow it).
#[derive(Clone, Debug)]
pub struct SwappedSegments {
    /// The object-store handles every segment was stored under.
    pub stored: Vec<StoredSegment>,
    /// The segments READ BACK from the object store (the object-store-resident form — the SRCH-D4
    /// re-confirmation runs the shred over THESE).
    pub loaded: Vec<SealedBackupSegment>,
    /// How many recovered byte-identically to the put segment (equals `loaded.len()` on a clean
    /// swap; the gate only returns `Ok` once every segment matched).
    pub byte_identical: usize,
}

/// **The object-store index backstop gate (SRCH-P30).** A zero-sized orchestrator over two phases
/// so the SRCH-D4 backup-scale gate is REUSED verbatim (EI-01 §7) rather than re-implemented, and
/// the loaded segments outlive the SRCH-D4 inputs that borrow them:
///
/// 1. [`ObjectStoreBackstopGate::swap_in`] — move the segments into the object store + read them
///    back BYTE-IDENTICAL (the *measured* swap, EI-01 §3). Returns the [`SwappedSegments`] the
///    caller owns.
/// 2. The caller runs the frozen [`BackupScaleEraseGate`](crate::hyok_scale::BackupScaleEraseGate)
///    over `swapped.loaded` (the object-store-resident segments).
/// 3. [`ObjectStoreBackstopGate::confirm`] — fold that SRCH-D4 verdict into the dated
///    [`ObjectStoreBackstopArtifact`] (0 recoverable after the shred ⇒ the erasure held over the
///    object store).
///
/// It REUSES the [`SegmentBackstop`] adapter (the swap), the [`SealedBackupSegment`] seal/shred
/// (SRCH-P29), and the [`BackupScaleEraseGate`](crate::hyok_scale::BackupScaleEraseGate) (SRCH-D4).
#[derive(Clone, Copy, Debug, Default)]
pub struct ObjectStoreBackstopGate;

impl ObjectStoreBackstopGate {
    /// A new gate (stateless).
    pub fn new() -> ObjectStoreBackstopGate {
        ObjectStoreBackstopGate
    }

    /// **Phase 1 — swap the segments into the object store + read them back BYTE-IDENTICAL.** Puts
    /// every supplied [`SealedBackupSegment`] through the [`SegmentBackstop`] adapter (the fs floor
    /// / real object store, by the adapter's backing), reads each back, and asserts byte-identical
    /// recovery (the swap moved them with no behaviour change). 0 segments is a vacuous swap
    /// ([`SwapProvedNothing`]); a mismatch is [`SegmentNotByteIdentical`]; a `BlobStore` failure is
    /// [`BlobOp`]. On success returns the [`SwappedSegments`] (the caller owns the loaded `Vec`).
    ///
    /// [`SwapProvedNothing`]: ObjectStoreBackstopFailure::SwapProvedNothing
    /// [`SegmentNotByteIdentical`]: ObjectStoreBackstopFailure::SegmentNotByteIdentical
    /// [`BlobOp`]: ObjectStoreBackstopFailure::BlobOp
    pub fn swap_in<B: BlobStore>(
        &self,
        backstop: &SegmentBackstop<B>,
        segments: &[SealedBackupSegment],
    ) -> Result<SwappedSegments, ObjectStoreBackstopFailure> {
        if segments.is_empty() {
            return Err(ObjectStoreBackstopFailure::SwapProvedNothing);
        }
        let mut stored = Vec::with_capacity(segments.len());
        for seg in segments {
            let s = backstop
                .put_segment(seg)
                .map_err(|e| ObjectStoreBackstopFailure::BlobOp(e.to_string()))?;
            stored.push(s);
        }

        let mut byte_identical = 0usize;
        let mut loaded = Vec::with_capacity(stored.len());
        for (orig, s) in segments.iter().zip(stored.iter()) {
            let recovered = backstop
                .get_segment(s)
                .map_err(|e| ObjectStoreBackstopFailure::BlobOp(e.to_string()))?;
            if recovered.to_blob_bytes() == orig.to_blob_bytes() && recovered.doc_id == orig.doc_id
            {
                byte_identical += 1;
            } else {
                return Err(ObjectStoreBackstopFailure::SegmentNotByteIdentical(
                    orig.doc_id.clone(),
                ));
            }
            loaded.push(recovered);
        }

        Ok(SwappedSegments {
            stored,
            loaded,
            byte_identical,
        })
    }

    /// **Phase 3 — fold the SRCH-D4 backup-scale erasure verdict (run over the object-store-resident
    /// `swapped.loaded` segments) into the dated [`ObjectStoreBackstopArtifact`].** The caller has
    /// already run the frozen [`BackupScaleEraseGate`](crate::hyok_scale::BackupScaleEraseGate) over
    /// the loaded segments (so the per-tenant index DEK crypto-shred ran over the
    /// object-store-resident form). This method asserts 0 recoverable after the shred — the SRCH-D4
    /// erasure held over the object store — and emits the green artifact, or surfaces the SRCH-D4
    /// red verbatim (never masked).
    ///
    /// `backing` + `ran_at` are caller-supplied so the SAME logic runs against the fs floor (unit)
    /// and the live object store (`--features integration`), recording the backing honestly.
    pub fn confirm<B: BlobStore>(
        &self,
        backstop: &SegmentBackstop<B>,
        swapped: &SwappedSegments,
        srch_d4: &BackupScaleEraseVerdict,
        backing: &'static str,
        ran_at: impl Into<String>,
    ) -> ObjectStoreBackstopVerdict {
        let recoverable_after_shred = match srch_d4 {
            BackupScaleEraseVerdict::Green(a) => a.backup_segments_recoverable_after_shred,
            BackupScaleEraseVerdict::Red(f) => {
                // A red for "still recoverable after the shred" is the object-store-specific
                // failure; any other red is propagated verbatim (never masked).
                use crate::hyok_scale::BackupScaleEraseFailure as F;
                return match f {
                    F::BackupRecoverableAfterShred(n) => ObjectStoreBackstopVerdict::Red(
                        ObjectStoreBackstopFailure::RecoverableAfterShred(*n),
                    ),
                    other => ObjectStoreBackstopVerdict::Red(
                        ObjectStoreBackstopFailure::BackupScaleRed(other.to_string()),
                    ),
                };
            }
        };
        if recoverable_after_shred != 0 {
            return ObjectStoreBackstopVerdict::Red(
                ObjectStoreBackstopFailure::RecoverableAfterShred(recoverable_after_shred),
            );
        }

        ObjectStoreBackstopVerdict::Green(ObjectStoreBackstopArtifact {
            tenant: backstop.tenant().clone(),
            region: backstop.region().clone(),
            segments_moved: swapped.loaded.len(),
            segments_byte_identical: swapped.byte_identical,
            recoverable_after_shred,
            backing,
            ran_at: ran_at.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::FsBlobStore;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    /// A sealed segment built WITHOUT a live DEK is impossible (seal needs the key); for the
    /// adapter-level swap tests we model a sealed segment by its at-rest bytes directly through
    /// `from_blob_bytes` so the round-trip is testable DB-free without standing up the full KMS.
    /// (The DEK-real seal/shred path is exercised end-to-end in the integration test + the
    /// hyok_scale unit tests.)
    fn fake_sealed(doc_id: &str, nonce_byte: u8, ct: &[u8]) -> SealedBackupSegment {
        // nonce_len || nonce(NONCE_LEN bytes) || ciphertext
        let mut bytes = Vec::new();
        bytes.push(myelin_storage::NONCE_LEN as u8);
        bytes.extend(std::iter::repeat_n(nonce_byte, myelin_storage::NONCE_LEN));
        bytes.extend_from_slice(ct);
        SealedBackupSegment::from_blob_bytes(doc_id, &bytes).expect("well-formed at-rest frame")
    }

    /// **The swap round-trips a segment BYTE-IDENTICALLY through the fs `BlobStore` floor** — the
    /// behaviour-unchanged core (the real object store is the `--features integration` leg).
    #[test]
    fn swap_round_trips_segment_byte_identical_over_fs_floor() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let seg = fake_sealed("myelin://acme/kn/page/p1", 0xAB, b"sealed-segment-bytes");

        let stored = backstop.put_segment(&seg).expect("put");
        // The address is the BLAKE3 of the at-rest bytes (content-addressed).
        assert_eq!(
            stored.content_address,
            ContentHash::blake3(&seg.to_blob_bytes())
        );

        let recovered = backstop.get_segment(&stored).expect("get");
        assert_eq!(
            recovered.to_blob_bytes(),
            seg.to_blob_bytes(),
            "the recovered segment is byte-identical — the swap moved it with no behaviour change"
        );
        assert_eq!(recovered.doc_id, seg.doc_id);
    }

    /// **Per-tenant isolation holds over the swap** — a DIFFERENT tenant's backstop cannot read
    /// the first tenant's object (the `BlobStore` per-tenant keyspace, §3.2). The object-store
    /// backstop inherits the trait's residency/isolation.
    #[test]
    fn per_tenant_isolation_holds_over_the_swap() {
        // One shared fs backing, two tenant-pinned backstops over it (the Arc<B> shared-backing
        // blanket impl from myelin-storage).
        let blobs = std::sync::Arc::new(FsBlobStore::new());
        let acme = SegmentBackstop::new(std::sync::Arc::clone(&blobs), tenant(), region());
        let globex = SegmentBackstop::new(
            std::sync::Arc::clone(&blobs),
            TenantId("globex".into()),
            region(),
        );
        let seg = fake_sealed("myelin://acme/kn/page/p1", 0x01, b"acme-only");
        let stored = acme.put_segment(&seg).expect("acme put");

        // globex addresses the SAME content hash but in ITS keyspace → a miss (per-tenant dedup,
        // no cross-tenant share).
        let cross = globex.get_segment(&stored);
        assert!(
            matches!(cross, Err(BlobError::NotFound { .. })),
            "a different tenant must NOT read this tenant's object-store segment, got {cross:?}"
        );
    }

    /// **A corrupt at-rest frame is surfaced, never silently opened** (0 silent serve). A
    /// truncated object yields a malformed-frame error from `get_segment`.
    #[test]
    fn corrupt_at_rest_frame_is_surfaced() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        // A frame whose declared nonce_len does not match NONCE_LEN → from_blob_bytes returns None.
        let bad_bytes = vec![0u8]; // nonce_len = 0, no nonce, no ciphertext
        let content_address = backstop.blobs.put(&tenant(), &bad_bytes).expect("put raw");
        let stored = StoredSegment {
            doc_id: "myelin://acme/kn/page/bad".into(),
            content_address,
        };
        let got = backstop.get_segment(&stored);
        assert!(
            matches!(got, Err(BlobError::MalformedAddress(_))),
            "a malformed at-rest frame must be surfaced (0 silent serve), got {got:?}"
        );
    }

    /// **`load_all` reconstructs the full segment set** (the SRCH-D4 re-confirmation's
    /// load-from-the-store step). Every loaded segment is byte-identical to what was put.
    #[test]
    fn load_all_reconstructs_the_segment_set() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let segs = [
            fake_sealed("myelin://acme/kn/page/p1", 0x10, b"one"),
            fake_sealed("myelin://acme/kn/page/p2", 0x20, b"two"),
            fake_sealed("myelin://acme/kn/page/p3", 0x30, b"three"),
        ];
        let stored: Vec<_> = segs
            .iter()
            .map(|s| backstop.put_segment(s).expect("put"))
            .collect();
        let loaded = backstop.load_all(&stored).expect("load_all");
        assert_eq!(loaded.len(), segs.len());
        for (a, b) in loaded.iter().zip(segs.iter()) {
            assert_eq!(a.to_blob_bytes(), b.to_blob_bytes());
            assert_eq!(a.doc_id, b.doc_id);
        }
    }

    /// **The artifact's green decision is exactly the conjunction (mutation floor).** A vacuous
    /// swap, a non-byte-identical recovery, or any recoverable-after-shred is RED.
    #[test]
    fn artifact_green_is_the_full_conjunction() {
        let green = ObjectStoreBackstopArtifact {
            tenant: tenant(),
            region: region(),
            segments_moved: 3,
            segments_byte_identical: 3,
            recoverable_after_shred: 0,
            backing: "fs-floor",
            ran_at: "2026-06-25".into(),
        };
        assert!(green.is_green());

        // vacuous swap
        let mut vacuous = green.clone();
        vacuous.segments_moved = 0;
        vacuous.segments_byte_identical = 0;
        assert!(!vacuous.is_green(), "0 segments moved is vacuous → RED");

        // a recovery that was not byte-identical
        let mut drifted = green.clone();
        drifted.segments_byte_identical = 2;
        assert!(!drifted.is_green(), "a non-byte-identical recovery is RED");

        // a segment recoverable after the shred
        let mut leaked = green.clone();
        leaked.recoverable_after_shred = 1;
        assert!(
            !leaked.is_green(),
            "a segment recoverable after the shred is RED (erasure must hold over the object store)"
        );
    }

    /// **`swap_in` goes RED on a vacuous swap (no segments)** — `SwapProvedNothing`, never a
    /// green over nothing.
    #[test]
    fn gate_red_on_vacuous_swap() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let got = ObjectStoreBackstopGate::new().swap_in(&backstop, &[]);
        assert!(matches!(
            got,
            Err(ObjectStoreBackstopFailure::SwapProvedNothing)
        ));
    }

    /// **`swap_in` then `confirm` is GREEN over the fs floor** when the SRCH-D4 verdict reports 0
    /// recoverable after the shred — the two-phase happy path (the integration test drives the SAME
    /// shape over the live object store with a real DEK).
    #[test]
    fn swap_in_then_confirm_is_green_with_zero_recoverable() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let segs = vec![
            fake_sealed("myelin://acme/kn/page/p1", 0x11, b"one"),
            fake_sealed("myelin://acme/kn/page/p2", 0x22, b"two"),
        ];
        let gate = ObjectStoreBackstopGate::new();
        let swapped = gate.swap_in(&backstop, &segs).expect("swap_in");
        assert_eq!(swapped.loaded.len(), 2);
        assert_eq!(swapped.byte_identical, 2);

        // A green SRCH-D4 verdict with 0 recoverable after the shred (the shape the real gate emits
        // when the crypto-shred reached the object-store-resident segments).
        let d4 = green_d4(0);
        let verdict = gate.confirm(&backstop, &swapped, &d4, "fs-floor", "2026-06-25");
        let artifact = verdict.artifact().expect("green");
        assert!(artifact.is_green());
        assert_eq!(artifact.recoverable_after_shred, 0);
        assert_eq!(artifact.segments_moved, 2);
    }

    /// **`confirm` surfaces a recoverable-after-shred SRCH-D4 verdict as RED** — the erasure did
    /// NOT hold over the object store (the gravest failure, never masked).
    #[test]
    fn confirm_red_when_segment_recoverable_after_shred() {
        let backstop = SegmentBackstop::new(FsBlobStore::new(), tenant(), region());
        let segs = vec![fake_sealed("myelin://acme/kn/page/p1", 0x11, b"one")];
        let gate = ObjectStoreBackstopGate::new();
        let swapped = gate.swap_in(&backstop, &segs).expect("swap_in");
        // A SRCH-D4 verdict reporting 1 segment STILL recoverable after the shred.
        let d4 = green_d4(1);
        let verdict = gate.confirm(&backstop, &swapped, &d4, "fs-floor", "2026-06-25");
        assert!(matches!(
            verdict.failure(),
            Some(ObjectStoreBackstopFailure::RecoverableAfterShred(1))
        ));
    }

    /// Build a SRCH-D4 green verdict that reports `recoverable_after` recoverable backups (a test
    /// fixture for the `confirm` fold — the real verdict comes from the live `BackupScaleEraseGate`).
    fn green_d4(recoverable_after: usize) -> BackupScaleEraseVerdict {
        BackupScaleEraseVerdict::Green(crate::hyok_scale::BackupScaleEraseArtifact {
            tenant: tenant(),
            region: region(),
            live_docs_purged: 1,
            live_docs_remaining: 0,
            zero_orphan_embedding: true,
            backup_segments_recoverable_before_shred: 1,
            backup_segments_recoverable_after_shred: recoverable_after,
            ran_at: "2026-06-25".into(),
        })
    }

    /// The verdict is `#[must_use]` + `run_or_fail_ci` turns a red into an `Err` (no swallow).
    #[test]
    fn run_or_fail_ci_propagates_red() {
        let red =
            ObjectStoreBackstopVerdict::Red(ObjectStoreBackstopFailure::RecoverableAfterShred(2));
        assert!(red.run_or_fail_ci().is_err());
        let green = ObjectStoreBackstopVerdict::Green(ObjectStoreBackstopArtifact {
            tenant: tenant(),
            region: region(),
            segments_moved: 1,
            segments_byte_identical: 1,
            recoverable_after_shred: 0,
            backing: "fs-floor",
            ran_at: "2026-06-25".into(),
        });
        assert!(green.run_or_fail_ci().is_ok());
    }
}
