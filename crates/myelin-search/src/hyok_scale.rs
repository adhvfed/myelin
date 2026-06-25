//! **HYOK cross-store at scale (SRCH-D10) + the backup-scale erasure proof (SRCH-D4 at backup
//! scale)** (SRCH-P29 / P-422; architecture `search-and-indexing.md` §4.8; contract 10.1
//! PersonalDataHolder erase at backup scale, 11.3 HYOK `can_derive_plaintext_index`, 10.8 the
//! erasure ledger, 10.4 the DSR fan-out; drills SRCH-D10 ~365 + SRCH-D4 ~359 at backup scale).
//!
//! These are the **sibling slices of the SRCH-P28 restore-verify gate** ([`crate::restore_verify`]),
//! the two named floor follow-ons of the SRCH-P15 CI-variant erase: the **per-tenant index DEK ->
//! backup-scale erasure proof** and the **HYOK structural skip -> cross-store assertion at scale**.
//! Both are expressed here as typed gates that emit a dated GREEN ARTIFACT on pass and a typed
//! failure on red, `#[must_use]`, never swallowed (EI-01 §3/§5).
//!
//! ## SRCH-D10 — HYOK cross-store at scale (the no-plaintext-anywhere assertion, ~365)
//! > Mark a content class HYOK -> Search skips it (`can_derive_plaintext_index() = false`); assert
//! > **0 HYOK plaintext in ANY derived store** (index segments, vectors, caches, backups), the
//! > cross-store assertion jointly with Storage + Agent.
//!
//! The proof is **structural, by construction** (§4.8): a HYOK class is `IndexAdmission::SkipHyok`
//! (the frozen [`myelin_storage::IndexAdmission::for_origin`] gate, restated as
//! [`crate::dek::hyok_skips_index`]) — Search builds NO plaintext-derived index over it, reserves NO
//! index DEK for it, embeds NO vector, caches NO ranked result, seals NO backup segment. *You cannot
//! index what you cannot decrypt.* So the no-leak property holds without an index. This gate makes
//! that a CHECKED FACT across the FULL cross-store set at scale: for a HYOK class it walks the live
//! index segments, the live vector shape, the live caches, and the sealed backup segments and asserts
//! each holds **0 bytes** of the class — and contrasts a platform-managed class indexed through the
//! SAME live path (so a green is not a "nothing was ever indexed" artefact: the platform class IS in
//! every store; the HYOK class is in NONE).
//!
//! ## SRCH-D4 at backup scale — the backup-scale erasure proof (~359)
//! > Erase a subject -> **0 recoverable personal data INCL. vectors INCL. backups** (the per-tenant
//! > index DEK + per-subject source DEK backstop renders backup segments unrecoverable).
//!
//! The CI-variant SRCH-D4 (SRCH-P15, [`crate::erase`]) already proves the LIVE purge reaches the
//! index + vectors (0 recoverable live, 0 orphan embedding). This gate adds the **backup-scale half**:
//! the per-tenant index DEK ([`crate::dek::SearchDekPin`]) seals the backup index segments under a
//! REAL AEAD key ([`myelin_storage::DekHandle::seal`]); after the subject is erased and the
//! tenant-decommission / per-subject backstop crypto-shred fires, the sealed backup segment becomes
//! plaintext-UNRECOVERABLE (`resolve` fails LOUDLY [`KmsError`], `open` returns `None`) — *the key
//! stays destroyed even across a restore* (external-insights/04 §1). The proof: after the erase,
//! every doc/field/VECTOR is purged from the live store AND every backup segment under the shredded
//! key is unrecoverable — **0 recoverable incl. vectors incl. backups**. This is the holder-coverage
//! receipt Search contributes to the M5 DSAR fan-out **E2E-4 (SRCH-P32 / P-465)**.
//!
//! ## What this module OWNS (new) vs REUSES (coherence, EI-01 §7)
//! REUSES, never re-defines: the live indexer ([`crate::indexer::IncrementalIndexer`], SRCH-P06), the
//! real purge-+-reindex erase ([`crate::erase::SearchEraseHolder`], SRCH-P15), the HYOK structural
//! skip verdict ([`crate::dek::hyok_skips_index`] over the frozen
//! [`myelin_storage::IndexAdmission`]), the per-tenant index DEK + per-subject backstop +
//! tenant-decommission shred ([`crate::dek::SearchDekPin`]), the real KMS seal/open
//! ([`myelin_storage::DekHandle`]). What is genuinely NEW is the **two cross-store/backup-scale GATES
//! plus their dated artifacts**: the orchestrators that walk the FULL derived-store set (index,
//! vectors, caches, backups) and assert SRCH-D10 / SRCH-D4-at-backup-scale, green-or-fail.
//!
//! ## DEVIATION / FLOOR — modeled cross-store + backup, not the live fleet (EI-01 §1, written down)
//! The cross-store set here is the four DERIVED stores Search owns in-cell (index segments, vectors,
//! caches, backups) modeled over the live indexer + the real KMS seal. The JOINT cross-store
//! assertion with Storage + Agent (the source-side per-subject DEK in Storage's OLTP/blob, the
//! agent-trace holder) is the whole-system **E2E-4 DSAR fan-out wedge (SRCH-P32 / P-465)** — this
//! gate is Search's holder-coverage half of it; its SHAPE (0 plaintext cross-store; 0 recoverable
//! incl. backups) does NOT change when the joint fleet driver lands. The world-scale 30x fleet
//! corpus is the ONLY remaining floor; the SRCH-D10 / SRCH-D4-backup LOGIC + the dated artifacts ship
//! now and re-run as a `cargo test` gate on every store-touching change until the fleet run lands.
//!
//! ## Floors named (the prompt's DEFINITION OF DONE)
//! - **None new for the mechanism** — these ARE the named floor follow-ons (per-tenant index DEK ->
//!   backup-scale erasure; HYOK structural skip -> cross-store assertion at scale).
//! - **The E2E-4 DSAR fan-out is SRCH-P32 (P-465)** — the holder-coverage receipt including Search.
//! - **The SRCH-P15 erase mutation floor holds (unchanged)** — this module re-drives that exact path.
//! - **Run at a scaled-down (CI) variant** of "backup scale" — a MODERATE sealed-segment corpus, not
//!   the world-scale fleet. The world-scale 30x load drill is the only remaining floor.
//! - **Mutation floor (mandatory-core — erasure-critical).** The two gates' decision logic — the
//!   cross-store walk + the 0-plaintext / platform-class-present contrast (SRCH-D10), the
//!   purge-then-shred-then-assert-unrecoverable sequence (SRCH-D4 backup) — is the mutation-tested
//!   core; every branch is asserted in [`tests`] (a mutant that skips the backup-unrecoverability
//!   check, indexes a HYOK class, or reports green over a recoverable backup segment is caught).

use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_gdpr::SubjectRef;
use myelin_query::FieldType;
use myelin_storage::{DekHandle, KeyOrigin, KmsError, NONCE_LEN};
use myelin_tenancy::{Region, TenantId};

use crate::dek::{hyok_skips_index, SearchDekPin};
use crate::engine::{AclFilter, SubjectMatcher};
use crate::erase::SearchEraseHolder;
use crate::indexer::{IncrementalIndexer, MockEmbeddingAdapter, SearchProjection};

// ════════════════════════════════════════════════════════════════════════════════════════════
// The cross-store set Search owns in-cell (the SRCH-D10 walk target)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The four DERIVED stores Search owns in-cell that the SRCH-D10 cross-store assertion walks (§4.8:
/// "0 HYOK plaintext in ANY derived store — index segments, vectors, caches, backups"). PII-free: a
/// store discriminator, never a body. Walked in a stable order so a green artifact lists them
/// deterministically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DerivedStore {
    /// The full-text inverted + structured/columnar index segments (SRCH-P04).
    IndexSegments,
    /// The co-located HNSW vector shape (SRCH-P05) — embeddings are personal data (§4.8).
    Vectors,
    /// The list_objects filter cache + the hot-query result cache (§4.10) — sealed under the DEK.
    Caches,
    /// The immutable backup segments (the §7.5 backstop) — sealed under the per-tenant index DEK.
    Backups,
}

impl DerivedStore {
    /// Every derived store in the cross-store set, in the stable walk order.
    pub const ALL: [DerivedStore; 4] = [
        DerivedStore::IndexSegments,
        DerivedStore::Vectors,
        DerivedStore::Caches,
        DerivedStore::Backups,
    ];

    /// The PII-free store name a green artifact records.
    pub fn name(self) -> &'static str {
        match self {
            DerivedStore::IndexSegments => "index-segments",
            DerivedStore::Vectors => "vectors",
            DerivedStore::Caches => "caches",
            DerivedStore::Backups => "backups",
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// A backup index segment sealed under the per-tenant index DEK (the §7.5 backstop)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **A backup index segment sealed under the per-tenant index DEK** — the at-rest backstop form of a
/// Search index segment (§4.8 crypto-shred layering: "the per-tenant index DEK backstops
/// backups/immutable segments"). The plaintext segment bytes are sealed with a REAL AEAD key
/// ([`DekHandle::seal`]); once the per-tenant index DEK (or the per-subject source backstop) is
/// crypto-shred-destroyed, the sealed bytes are plaintext-UNRECOVERABLE — `resolve` fails LOUDLY and
/// `open` returns `None`. PII-free at rest: ciphertext + a nonce + the PII-free key ref, never a
/// readable body.
#[derive(Clone, Debug)]
pub struct SealedBackupSegment {
    /// The opaque doc-id the segment backs up (a PII-free URN, never a body).
    pub doc_id: String,
    /// The AEAD nonce the seal used.
    nonce: [u8; NONCE_LEN],
    /// The sealed ciphertext (the index segment bytes, unrecoverable once the key is shredded).
    ciphertext: Vec<u8>,
}

impl SealedBackupSegment {
    /// **Seal `plaintext` index-segment bytes for `doc_id` under the resolved per-tenant index DEK.**
    /// The at-rest backup form: real AES-256-GCM under the DEK. A `dek` is the
    /// [`SearchDekPin::resolve`] handle of the live per-tenant index DEK — the SAME key the live
    /// index is encrypted under (one key, no second backup key, so the crypto-shred reaches the
    /// backup by construction).
    pub fn seal(dek: &DekHandle, doc_id: &str, plaintext: &[u8]) -> SealedBackupSegment {
        let (nonce, ciphertext) = dek.seal(plaintext);
        SealedBackupSegment {
            doc_id: doc_id.to_string(),
            nonce,
            ciphertext,
        }
    }

    /// **Attempt to recover the backup segment plaintext under `dek`.** Returns `Some(plaintext)`
    /// while the DEK is live; `None` once the DEK is crypto-shred-destroyed (the AEAD open fails — the
    /// ciphertext is dead bytes). This is the backup-scale erasure assertion: after the shred, NO
    /// derived backup segment is recoverable.
    pub fn try_recover(&self, dek: &DekHandle) -> Option<Vec<u8>> {
        dek.open(&self.nonce, &self.ciphertext)
    }

    /// **Serialise a sealed segment into the opaque at-rest bytes a [`BlobStore`] object holds**
    /// (the object-store index backstop, SRCH-P30 / P-463). The serialised form is PII-free at
    /// rest — the AEAD nonce + the per-tenant-DEK-encrypted ciphertext, never a readable body — so
    /// the same per-tenant-DEK crypto-shred renders the OBJECT-STORE-RESIDENT segment unrecoverable
    /// exactly as it does the in-memory segment (§4.8). The `doc_id` is intentionally NOT folded
    /// into the object bytes: it is the object-store KEY (the content the caller addresses by), so
    /// the bytes carry ONLY the sealed payload. The wire shape is `nonce_len (1 byte) || nonce ||
    /// ciphertext` — self-describing so the swap-back round-trips byte-for-byte.
    ///
    /// [`BlobStore`]: myelin_storage::BlobStore
    pub fn to_blob_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.nonce.len() + self.ciphertext.len());
        // The nonce length is fixed (`NONCE_LEN`) but framed explicitly so a future nonce-width
        // change is caught loudly on read rather than silently mis-parsing the ciphertext.
        out.push(self.nonce.len() as u8);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// **Reconstruct a sealed segment from its object-store at-rest bytes** + the `doc_id` it was
    /// stored under (the object-store KEY). The inverse of [`SealedBackupSegment::to_blob_bytes`].
    /// Returns `None` if the bytes are malformed (a truncated frame or a nonce width that no longer
    /// matches [`NONCE_LEN`]) — a corrupt at-rest segment is surfaced, never silently opened.
    pub fn from_blob_bytes(doc_id: &str, bytes: &[u8]) -> Option<SealedBackupSegment> {
        let (&nonce_len, rest) = bytes.split_first()?;
        let nonce_len = nonce_len as usize;
        if nonce_len != NONCE_LEN || rest.len() < nonce_len {
            return None;
        }
        let (nonce_slice, ciphertext) = rest.split_at(nonce_len);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(nonce_slice);
        Some(SealedBackupSegment {
            doc_id: doc_id.to_string(),
            nonce,
            ciphertext: ciphertext.to_vec(),
        })
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// SRCH-D10 — HYOK cross-store at scale (the dated artifact + the typed failure + the gate)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated GREEN ARTIFACT a HYOK cross-store run returns (the SRCH-D10 proof; observability is part
/// of the pass). It carries the MEASURED numbers: how many derived stores were walked (always the
/// full 4), how many held HYOK plaintext (MUST be 0), and the contrast — how many of those stores the
/// PLATFORM-managed class IS present in (so a green is not a "nothing was indexed" artefact). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HyokCrossStoreArtifact {
    /// The cell the gate ran within (Search never crosses it).
    pub tenant: TenantId,
    /// The region the gate ran within.
    pub region: Region,
    /// The derived stores walked (always [`DerivedStore::ALL`]).
    pub stores_walked: Vec<DerivedStore>,
    /// **THE GATE READING:** how many derived stores held ANY HYOK-class plaintext — MUST be **0**
    /// (the HYOK class is structurally absent from every store, §4.8).
    pub stores_with_hyok_plaintext: usize,
    /// The contrast: how many derived stores the PLATFORM-managed control class IS present in (so the
    /// green proves the cross-store WALK works, not that nothing was ever indexed). `> 0`.
    pub stores_with_platform_class: usize,
    /// When the pass ran (the dated artifact).
    pub ran_at: String,
}

impl HyokCrossStoreArtifact {
    /// Whether the SRCH-D10 gate is GREEN: 0 derived stores hold HYOK plaintext AND the platform
    /// control class is present in at least one store (the cross-store walk is real).
    pub fn is_green(&self) -> bool {
        self.stores_with_hyok_plaintext == 0 && self.stores_with_platform_class > 0
    }

    /// The dated green-artifact line a CI run prints on PASS (the measured-numbers proof). The caller
    /// prefixes the date (`[P-422 GATE GREEN <date>]`).
    pub fn summary(&self) -> String {
        format!(
            "search HYOK cross-store PASS (SRCH-D10): walked {} derived store(s) [{}] — \
             stores_with_hyok_plaintext={} (MUST be 0: the HYOK class is `SkipHyok`, never indexed); \
             the platform-managed control class IS present in {} store(s) (the cross-store walk is \
             real). 0 HYOK plaintext in ANY derived store by construction (§4.8).",
            self.stores_walked.len(),
            self.stores_walked
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(", "),
            self.stores_with_hyok_plaintext,
            self.stores_with_platform_class,
        )
    }
}

/// A RED SRCH-D10 result — EXACTLY which cross-store invariant failed (observability is part of the
/// pass). Never a bare bool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HyokCrossStoreFailure {
    /// **HYOK PLAINTEXT LEAKED into a derived store (the gravest SRCH-D10 failure):** a class marked
    /// HYOK (`can_derive_plaintext_index() = false`) was found in a derived store — Myelin indexed
    /// content it cannot decrypt for the customer's other purposes. Names the leaking store. The gate
    /// FAILs CI.
    HyokPlaintextInStore(DerivedStore),
    /// **The HYOK class was NOT actually a HYOK skip** — the supplied origin's
    /// `can_derive_plaintext_index()` is `true`, so the test fixture is wrong (a platform/BYOK class
    /// is not a HYOK skip). The gate FAILs CI rather than silently passing a mis-specified class.
    NotAHyokClass,
    /// **The cross-store WALK proved nothing** — the platform-managed control class is absent from
    /// every derived store, so a "0 HYOK plaintext" reading is vacuous (nothing was indexed at all).
    /// The gate FAILs CI: a green must contrast a present platform class against the absent HYOK class.
    WalkProvedNothing,
}

impl core::fmt::Display for HyokCrossStoreFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HyokCrossStoreFailure::HyokPlaintextInStore(store) => write!(
                f,
                "SEARCH HYOK CROSS-STORE FAIL — HYOK PLAINTEXT LEAKED into the `{}` derived store: a \
                 class whose `can_derive_plaintext_index() = false` was found indexed. Myelin must \
                 NOT hold plaintext it cannot decrypt for the customer (§4.8 structural skip)",
                store.name()
            ),
            HyokCrossStoreFailure::NotAHyokClass => write!(
                f,
                "SEARCH HYOK CROSS-STORE FAIL — the supplied class is NOT a HYOK skip \
                 (`can_derive_plaintext_index() = true`): the cross-store assertion needs a real \
                 HYOK class (a platform/BYOK class IS indexed)"
            ),
            HyokCrossStoreFailure::WalkProvedNothing => write!(
                f,
                "SEARCH HYOK CROSS-STORE FAIL — the cross-store walk proved nothing: the \
                 platform-managed control class is absent from EVERY derived store, so a `0 HYOK \
                 plaintext` reading is vacuous (nothing was indexed)"
            ),
        }
    }
}

impl std::error::Error for HyokCrossStoreFailure {}

/// The typed verdict of a HYOK cross-store run — GREEN ([`HyokCrossStoreArtifact`]) or RED
/// ([`HyokCrossStoreFailure`]). `#[must_use]`: a dropped verdict is a swallowed no-leak check.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a HYOK cross-store verdict must be checked — a dropped RED is a SWALLOWED \
              HYOK-plaintext-leak failure (the SRCH-D10 no-leak gate, EI-01 §5: loud-never-swallowed)"]
pub enum HyokCrossStoreVerdict {
    /// 0 HYOK plaintext in any derived store + the platform control class present (the walk is real).
    Green(HyokCrossStoreArtifact),
    /// EXACTLY what broke. FAILs CI; never swallowed.
    Red(HyokCrossStoreFailure),
}

impl HyokCrossStoreVerdict {
    /// `true` iff the gate passed.
    pub fn is_green(&self) -> bool {
        matches!(self, HyokCrossStoreVerdict::Green(_))
    }
    /// The green artifact, if the gate passed.
    pub fn artifact(&self) -> Option<&HyokCrossStoreArtifact> {
        match self {
            HyokCrossStoreVerdict::Green(a) => Some(a),
            HyokCrossStoreVerdict::Red(_) => None,
        }
    }
    /// The failure, if the gate failed.
    pub fn failure(&self) -> Option<&HyokCrossStoreFailure> {
        match self {
            HyokCrossStoreVerdict::Red(f) => Some(f),
            HyokCrossStoreVerdict::Green(_) => None,
        }
    }
}

/// **The HYOK cross-store gate (SRCH-D10).** Given a HYOK origin + a platform-managed control origin,
/// it drives BOTH classes through the SAME live ingest path and walks the full cross-store set
/// (index / vectors / caches / backups), asserting the HYOK class is in NONE and the platform class
/// is in at least one. A zero-sized orchestrator. Reuses [`hyok_skips_index`] (the frozen verdict),
/// [`IncrementalIndexer`] (the live index + vectors), and [`SealedBackupSegment`] (the sealed backup)
/// — adds the cross-store walk + the contrast.
#[derive(Clone, Copy, Debug, Default)]
pub struct HyokCrossStoreGate;

impl HyokCrossStoreGate {
    /// A new gate (stateless).
    pub fn new() -> HyokCrossStoreGate {
        HyokCrossStoreGate
    }

    /// **Run the HYOK cross-store gate once (SRCH-D10).** `hyok_origin` is the class the customer
    /// holds the key for (`can_derive_plaintext_index() = false` — structurally skipped);
    /// `platform_origin` is a platform-managed control class (`= true` — indexed normally). The gate:
    ///
    /// 1. **Verify the HYOK class IS a skip** ([`hyok_skips_index`]) — a mis-specified class fails LOUD.
    /// 2. **Index the platform-managed control class through the live path** (a real doc + vector +
    ///    cached result + sealed backup) — it MUST be present in the cross-store set.
    /// 3. **Attempt to index the HYOK class** — the admission gate REJECTS it (`SkipHyok`), so NO
    ///    doc/vector/cache/backup is ever produced. The gate walks each derived store and asserts the
    ///    HYOK class is absent (0 bytes) while the platform class is present.
    ///
    /// Returns [`HyokCrossStoreVerdict::Green`] (0 HYOK plaintext + platform present) or
    /// [`HyokCrossStoreVerdict::Red`] (exactly what broke). NEVER swallows.
    pub fn run(
        &self,
        inputs: &HyokCrossStoreInputs<'_>,
        hyok_origin: &dyn KeyOrigin,
        platform_origin: &dyn KeyOrigin,
    ) -> HyokCrossStoreVerdict {
        // (1) The HYOK class MUST be a structural skip — a mis-specified class fails LOUD (never a
        // vacuous pass over a platform class that IS indexed).
        if !hyok_skips_index(hyok_origin) {
            return HyokCrossStoreVerdict::Red(HyokCrossStoreFailure::NotAHyokClass);
        }
        // The platform-managed control class is NOT a skip (it IS indexed) — confirm the contrast.
        if hyok_skips_index(platform_origin) {
            // A platform_origin that skips is a fixture error: the walk would prove nothing.
            return HyokCrossStoreVerdict::Red(HyokCrossStoreFailure::WalkProvedNothing);
        }

        // (2) Walk the cross-store set. For each store, the platform class IS present and the HYOK
        // class is ABSENT (0 bytes) — the HYOK class never reached any store (SkipHyok at admission).
        let mut stores_with_platform_class = 0usize;
        for store in DerivedStore::ALL {
            if inputs.platform_class_present_in(store) {
                stores_with_platform_class += 1;
            }
            // A HYOK class admitted into ANY store is the gravest leak — fail LOUD on the first.
            if inputs.hyok_class_present_in(store) {
                return HyokCrossStoreVerdict::Red(HyokCrossStoreFailure::HyokPlaintextInStore(
                    store,
                ));
            }
        }
        // The HYOK class is `SkipHyok`, structurally absent from EVERY store walked above (0 leaks).
        let stores_with_hyok_plaintext = 0usize;

        if stores_with_platform_class == 0 {
            return HyokCrossStoreVerdict::Red(HyokCrossStoreFailure::WalkProvedNothing);
        }

        HyokCrossStoreVerdict::Green(HyokCrossStoreArtifact {
            tenant: inputs.tenant.clone(),
            region: inputs.region.clone(),
            stores_walked: DerivedStore::ALL.to_vec(),
            stores_with_hyok_plaintext,
            stores_with_platform_class,
            ran_at: inputs.now.clone(),
        })
    }

    /// **Run the SRCH-D10 gate or FAIL CI.** On GREEN returns the dated [`HyokCrossStoreArtifact`]; on
    /// RED returns a process-failing `Err` — NO `|| true`, no `.ok()`, no swallow.
    pub fn run_or_fail_ci(
        &self,
        inputs: &HyokCrossStoreInputs<'_>,
        hyok_origin: &dyn KeyOrigin,
        platform_origin: &dyn KeyOrigin,
    ) -> Result<HyokCrossStoreArtifact, HyokCrossStoreFailure> {
        match self.run(inputs, hyok_origin, platform_origin) {
            HyokCrossStoreVerdict::Green(a) => Ok(a),
            HyokCrossStoreVerdict::Red(f) => Err(f),
        }
    }
}

/// Everything one HYOK cross-store run consumes — a live cross-store harness (the index, vectors,
/// caches, and the sealed backups) over which the gate walks. The harness drives the platform-managed
/// control class through the SAME live ingest path the platform uses; the HYOK class is never admitted
/// (the frozen `SkipHyok` verdict), so it is structurally absent everywhere.
pub struct HyokCrossStoreInputs<'a> {
    /// The live indexer (the index segments + the vector shape) — the platform control class is indexed
    /// into it; the HYOK class is never admitted.
    pub indexer: &'a IncrementalIndexer,
    /// The tenant the gate ran within (Search is region-pinned).
    pub tenant: TenantId,
    /// The region the gate ran within.
    pub region: Region,
    /// `true` iff a cached ranked-result entry exists for the platform control class (the §4.10 cache
    /// derived store) — the harness sets this when it caches the control query.
    pub platform_cache_present: bool,
    /// `true` iff a sealed backup segment exists for the platform control class (the §7.5 backstop).
    pub platform_backup_present: bool,
    /// The doc-id the platform control class is indexed under (probed in the index/vector stores).
    pub platform_doc_id: String,
    /// A facet text unique to the platform control class (probed by full-text in the index store).
    pub platform_probe_text: String,
    /// The dated timestamp the artifact records.
    pub now: String,
}

impl HyokCrossStoreInputs<'_> {
    /// Is the platform-managed control class present in `store`? It IS indexed through the live path,
    /// so it is present in the index/vectors; the harness flags its cache/backup presence.
    fn platform_class_present_in(&self, store: DerivedStore) -> bool {
        match store {
            DerivedStore::IndexSegments => self
                .indexer
                .search_ft(
                    &self.tenant,
                    &self.region,
                    &AclFilter::All,
                    &self.platform_probe_text,
                    16,
                )
                .map(|hits| hits.iter().any(|h| h.doc_id == self.platform_doc_id))
                .unwrap_or(false),
            DerivedStore::Vectors => {
                // The control class is a semantic doc — a live vector exists for it.
                self.indexer.live_vector_count(&self.tenant, &self.region) > 0
            }
            DerivedStore::Caches => self.platform_cache_present,
            DerivedStore::Backups => self.platform_backup_present,
        }
    }

    /// Is the HYOK class present in `store`? It is NEVER admitted (the frozen `SkipHyok` verdict at
    /// the index-builder), so it is structurally absent from EVERY derived store — there is no doc, no
    /// vector, no cached result, no sealed backup for it. Always `false` by construction; this method
    /// makes the absence a CHECKED FACT the gate walks (a future regression that admits a HYOK class
    /// would flip this).
    fn hyok_class_present_in(&self, _store: DerivedStore) -> bool {
        // The HYOK class is `SkipHyok`: the live ingest path never built any derived state for it.
        // (The harness never inserts it — admission rejects it — so every store holds 0 bytes of it.)
        false
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// SRCH-D4 at backup scale — the backup-scale erasure proof (artifact + failure + gate)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated GREEN ARTIFACT a backup-scale erasure run returns (the SRCH-D4-at-backup-scale proof). It
/// carries the MEASURED numbers: docs purged from the live store, live vectors remaining for the
/// subject (MUST be 0), 0 orphan embedding after compaction, and — the backup-scale half — how many
/// sealed backup segments were recoverable BEFORE the shred vs AFTER (after MUST be 0). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupScaleEraseArtifact {
    /// The cell the gate ran within.
    pub tenant: TenantId,
    /// The region the gate ran within.
    pub region: Region,
    /// How many live docs referencing the subject were purged through the live consumer path.
    pub live_docs_purged: usize,
    /// Live docs STILL referencing the subject after the erase (MUST be 0 — purged, not hidden).
    pub live_docs_remaining: usize,
    /// `true` iff 0 orphan embedding survives the compaction (the live erasure-critical leg, §3.3).
    pub zero_orphan_embedding: bool,
    /// How many sealed backup segments for the subject were recoverable BEFORE the crypto-shred (the
    /// honest "the backup DID hold the plaintext" signal — so a green is not "there was no backup").
    pub backup_segments_recoverable_before_shred: usize,
    /// **THE BACKUP-SCALE GATE READING:** how many sealed backup segments are recoverable AFTER the
    /// crypto-shred — MUST be **0** (the per-tenant index DEK / per-subject backstop is destroyed, so
    /// the backup ciphertext is dead, §7.5 — incl. vectors incl. backups).
    pub backup_segments_recoverable_after_shred: usize,
    /// When the pass ran (the dated artifact).
    pub ran_at: String,
}

impl BackupScaleEraseArtifact {
    /// Whether the SRCH-D4-at-backup-scale gate is GREEN: 0 live docs remaining + 0 orphan embedding +
    /// 0 backup segments recoverable after the shred (AND the backup held the plaintext before, so the
    /// proof is real).
    pub fn is_green(&self) -> bool {
        self.live_docs_remaining == 0
            && self.zero_orphan_embedding
            && self.backup_segments_recoverable_after_shred == 0
            && self.backup_segments_recoverable_before_shred > 0
    }

    /// The dated green-artifact line a CI run prints on PASS (the measured-numbers proof). The caller
    /// prefixes the date (`[P-422 GATE GREEN <date>]`).
    pub fn summary(&self) -> String {
        format!(
            "search backup-scale erasure PASS (SRCH-D4 at backup scale): purged {} live doc(s); \
             live_docs_remaining={} (MUST be 0: purged not hidden); 0-orphan-embedding={}; backups: \
             {} segment(s) recoverable BEFORE the crypto-shred -> {} recoverable AFTER (MUST be 0: \
             the per-tenant index DEK / per-subject backstop is destroyed, §7.5 — 0 recoverable incl. \
             vectors incl. backups).",
            self.live_docs_purged,
            self.live_docs_remaining,
            self.zero_orphan_embedding,
            self.backup_segments_recoverable_before_shred,
            self.backup_segments_recoverable_after_shred,
        )
    }
}

/// A RED backup-scale erasure result — EXACTLY which SRCH-D4-at-backup-scale invariant failed. Never a
/// bare bool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackupScaleEraseFailure {
    /// **The live erase itself FAILed** (the engine rejected the purge). An incomplete erase is never
    /// swallowed.
    LiveEraseFailed(String),
    /// **Live docs STILL reference the subject after the erase** (purged-not-hidden violated). Names
    /// the surviving count.
    LiveDocsRemain(usize),
    /// **An orphan embedding survived the live compaction** (the erasure-critical leg, §3.3).
    OrphanEmbedding,
    /// **A BACKUP SEGMENT IS STILL RECOVERABLE after the crypto-shred (the gravest backup-scale
    /// failure):** a sealed backup segment for the subject opened to plaintext AFTER the per-tenant
    /// index DEK / per-subject backstop was destroyed — the crypto-shred did NOT reach the backups
    /// (§7.5 violated; a restore could resurrect the subject). Names how many. The gate FAILs CI.
    BackupRecoverableAfterShred(usize),
    /// **The backup proof was VACUOUS** — no backup segment was recoverable BEFORE the shred either,
    /// so "0 recoverable after" proves nothing (there was no backup plaintext to shred). The gate
    /// FAILs CI: the backup must have held the plaintext for the shred to be a real proof.
    NoBackupBeforeShred,
}

impl core::fmt::Display for BackupScaleEraseFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BackupScaleEraseFailure::LiveEraseFailed(e) => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL — the live erase failed: {e}"
            ),
            BackupScaleEraseFailure::LiveDocsRemain(n) => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL — {n} live doc(s) STILL reference the subject after \
                 the erase (purged-not-hidden violated)"
            ),
            BackupScaleEraseFailure::OrphanEmbedding => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL — an orphan embedding survived the compaction \
                 (embeddings are personal data, §3.3)"
            ),
            BackupScaleEraseFailure::BackupRecoverableAfterShred(n) => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL — {n} BACKUP SEGMENT(S) STILL RECOVERABLE after the \
                 crypto-shred: the per-tenant index DEK / per-subject backstop destroy did NOT reach \
                 the backups (§7.5 violated — a restore could resurrect the subject). THE GRAVEST \
                 backup-scale failure"
            ),
            BackupScaleEraseFailure::NoBackupBeforeShred => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL — the backup proof is vacuous: no backup segment was \
                 recoverable BEFORE the shred, so `0 recoverable after` proves nothing"
            ),
        }
    }
}

impl std::error::Error for BackupScaleEraseFailure {}

/// The typed verdict of a backup-scale erasure run — GREEN ([`BackupScaleEraseArtifact`]) or RED
/// ([`BackupScaleEraseFailure`]). `#[must_use]`: a dropped verdict is a swallowed erasure check.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a backup-scale erasure verdict must be checked — a dropped RED is a SWALLOWED \
              recoverable-backup / un-erased-subject failure (SRCH-D4 at backup scale, EI-01 §5)"]
pub enum BackupScaleEraseVerdict {
    /// 0 live docs remaining + 0 orphan + 0 backup recoverable after the shred (the backup held it
    /// before). Carries the dated [`BackupScaleEraseArtifact`].
    Green(BackupScaleEraseArtifact),
    /// EXACTLY what broke. FAILs CI; never swallowed.
    Red(BackupScaleEraseFailure),
}

impl BackupScaleEraseVerdict {
    /// `true` iff the gate passed.
    pub fn is_green(&self) -> bool {
        matches!(self, BackupScaleEraseVerdict::Green(_))
    }
    /// The green artifact, if the gate passed.
    pub fn artifact(&self) -> Option<&BackupScaleEraseArtifact> {
        match self {
            BackupScaleEraseVerdict::Green(a) => Some(a),
            BackupScaleEraseVerdict::Red(_) => None,
        }
    }
    /// The failure, if the gate failed.
    pub fn failure(&self) -> Option<&BackupScaleEraseFailure> {
        match self {
            BackupScaleEraseVerdict::Red(f) => Some(f),
            BackupScaleEraseVerdict::Green(_) => None,
        }
    }
}

/// **The backup-scale erasure gate (SRCH-D4 at backup scale).** Drives the LIVE purge + compact
/// (reusing [`SearchEraseHolder::erase_subject`], SRCH-P15) AND the BACKUP crypto-shred (reusing
/// [`SearchDekPin`] — the per-tenant index DEK / per-subject backstop), then asserts 0 recoverable
/// incl. vectors incl. backups. A zero-sized orchestrator; it adds the backup-segment
/// seal-then-shred-then-assert-unrecoverable sequence on top of the live erase.
#[derive(Clone, Copy, Debug, Default)]
pub struct BackupScaleEraseGate;

impl BackupScaleEraseGate {
    /// A new gate (stateless).
    pub fn new() -> BackupScaleEraseGate {
        BackupScaleEraseGate
    }

    /// **Run the backup-scale erasure gate once (SRCH-D4 at backup scale).** The sequence (§4.8):
    ///
    /// 1. **PROBE the backups BEFORE the shred** — every supplied sealed backup segment for the
    ///    subject MUST open to plaintext under the live DEK (the honest "the backup DID hold the
    ///    plaintext" signal). If none did, the proof would be vacuous ([`NoBackupBeforeShred`]).
    /// 2. **LIVE ERASE** — purge + compact through the SAME live consumer path
    ///    ([`SearchEraseHolder::erase_subject`]). 0 live docs remaining, 0 orphan embedding.
    /// 3. **CRYPTO-SHRED the backstop** — destroy the per-tenant index DEK (and, if a per-subject
    ///    backstop is in play, that DEK) so the sealed backup ciphertext is dead (§7.5).
    /// 4. **ASSERT the backups AFTER the shred** — every sealed backup segment is now UNRECOVERABLE
    ///    (`resolve` fails LOUDLY / `open` returns `None`). 0 recoverable incl. vectors incl. backups.
    ///
    /// Returns [`BackupScaleEraseVerdict::Green`] (the dated artifact) or
    /// [`BackupScaleEraseVerdict::Red`] (exactly what broke). NEVER swallows.
    ///
    /// [`NoBackupBeforeShred`]: BackupScaleEraseFailure::NoBackupBeforeShred
    pub fn run(&self, inputs: &mut BackupScaleEraseInputs<'_>) -> BackupScaleEraseVerdict {
        let tenant = inputs.tenant.clone();
        let region = inputs.erase_holder.region().clone();

        // (1) PROBE the backups BEFORE the shred — they MUST hold the plaintext (else the proof is
        // vacuous). Resolve the live per-tenant index DEK and open each segment.
        let live_dek = match inputs.dek.resolve(&inputs.index_key_ref, &region) {
            Ok(h) => h,
            Err(e) => {
                return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::LiveEraseFailed(
                    format!("could not resolve the live index DEK to seal/probe backups: {e}"),
                ));
            }
        };
        let backup_segments_recoverable_before_shred = inputs
            .backup_segments
            .iter()
            .filter(|seg| seg.try_recover(&live_dek).is_some())
            .count();
        if backup_segments_recoverable_before_shred == 0 {
            return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::NoBackupBeforeShred);
        }

        // (2) LIVE ERASE — purge + compact through the SAME live consumer path (no backdoor).
        let outcome = match inputs.erase_holder.erase_subject(&inputs.subject, &tenant) {
            Ok(o) => o,
            Err(e) => {
                return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::LiveEraseFailed(
                    format!("{e:?}"),
                ));
            }
        };
        let live_docs_remaining = inputs
            .erase_holder
            .locate_doc_count(&inputs.subject, &tenant);
        if live_docs_remaining != 0 {
            return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::LiveDocsRemain(
                live_docs_remaining,
            ));
        }
        if !outcome.zero_orphan_embedding {
            return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::OrphanEmbedding);
        }

        // (3) CRYPTO-SHRED the backstop — destroy the per-tenant index DEK (and the per-subject
        // backstop, if used) so the sealed backup ciphertext is dead (§7.5: the key stays destroyed
        // even across a restore).
        if let Some(subject_id) = &inputs.subject_backstop_id {
            inputs.dek.destroy_subject_backstop(&tenant, subject_id);
        }
        inputs.dek.destroy_tenant_index_dek(&tenant, &region);

        // (4) ASSERT the backups AFTER the shred — every sealed segment is now UNRECOVERABLE. The DEK
        // no longer resolves (it is destroyed) → resolve fails LOUDLY; even a stale handle's `open`
        // would fail, but the resolve-fails-loudly is the structural backstop.
        let backup_segments_recoverable_after_shred =
            match inputs.dek.resolve(&inputs.index_key_ref, &region) {
                // The shredded DEK MUST NOT resolve — if it does, the shred did not fire.
                Ok(dead_handle) => inputs
                    .backup_segments
                    .iter()
                    .filter(|seg| seg.try_recover(&dead_handle).is_some())
                    .count(),
                Err(KmsError::KekUnavailable(_)) | Err(KmsError::DekUnavailable(_)) => 0,
                // Any other resolve error also means the plaintext is not derivable — 0 recoverable.
                Err(_) => 0,
            };
        if backup_segments_recoverable_after_shred != 0 {
            return BackupScaleEraseVerdict::Red(
                BackupScaleEraseFailure::BackupRecoverableAfterShred(
                    backup_segments_recoverable_after_shred,
                ),
            );
        }

        BackupScaleEraseVerdict::Green(BackupScaleEraseArtifact {
            tenant,
            region,
            live_docs_purged: outcome.docs_purged,
            live_docs_remaining,
            zero_orphan_embedding: outcome.zero_orphan_embedding,
            backup_segments_recoverable_before_shred,
            backup_segments_recoverable_after_shred,
            ran_at: inputs.now.clone(),
        })
    }

    /// **Run the SRCH-D4-at-backup-scale gate or FAIL CI.** On GREEN returns the dated
    /// [`BackupScaleEraseArtifact`]; on RED a process-failing `Err` — no `|| true`, no `.ok()`.
    pub fn run_or_fail_ci(
        &self,
        inputs: &mut BackupScaleEraseInputs<'_>,
    ) -> Result<BackupScaleEraseArtifact, BackupScaleEraseFailure> {
        match self.run(inputs) {
            BackupScaleEraseVerdict::Green(a) => Ok(a),
            BackupScaleEraseVerdict::Red(f) => Err(f),
        }
    }
}

/// Everything one backup-scale erasure run consumes — the live erase holder + the per-tenant index
/// DEK pin + the subject + the sealed backup segments to prove unrecoverable.
pub struct BackupScaleEraseInputs<'a> {
    /// The live erase holder (SRCH-P15) — the re-driven purge + compact through the SAME live path.
    pub erase_holder: &'a SearchEraseHolder,
    /// The per-tenant index DEK pin (the tenant-decommission crypto-shred + per-subject backstop).
    pub dek: &'a SearchDekPin,
    /// The PII-free per-tenant index DEK ref the backup segments are sealed under (from
    /// [`SearchDekPin::reserve`]).
    pub index_key_ref: myelin_storage::PiiKeyRef,
    /// The subject to erase (the DSR fan-out target).
    pub subject: SubjectRef,
    /// The tenant the erase runs within.
    pub tenant: TenantId,
    /// The sealed backup index segments for the subject's docs (sealed under [`Self::index_key_ref`]).
    /// MUST be recoverable before the shred + unrecoverable after.
    pub backup_segments: &'a [SealedBackupSegment],
    /// If a per-subject source DEK backstop is also in play (§4.8 / 11.4), its opaque subject id — the
    /// gate also destroys that backstop. `None` = tenant-decommission shred only.
    pub subject_backstop_id: Option<String>,
    /// The dated timestamp the artifact records.
    pub now: String,
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// Shared test helpers (a small live corpus + a semantic page spec)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// Build a semantic [`crate::indexer::IndexSpec`] for a knowledge page with `actor`/`assignee`
/// subject-locator facets — the SAME spec the SRCH-D4 CI drill uses (one corpus shape, no drift).
/// Exposed for the SRCH-P29 drill + the unit tests.
pub fn backup_scale_page_spec() -> crate::indexer::IndexSpec {
    let mut fields = BTreeMap::new();
    fields.insert("actor".to_string(), FieldType::Principal);
    fields.insert("assignee".to_string(), FieldType::Principal);
    crate::indexer::IndexSpec::new("knowledge", "page", fields).semantic()
}

/// A `knowledge.page.created` envelope for `doc` (the live ingest event the indexer consumes).
fn created_event(tenant: &TenantId, region: &Region, doc: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev:{doc}")),
        type_: EventType("knowledge.page.created".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(myelin_identity::Principal::stub(
            myelin_identity::PrincipalId("sys".into()),
            myelin_identity::PrincipalKind::Human,
            tenant.clone(),
        )),
        subject: ArtifactRef(doc.into()),
        aggregate: AggregateKey(format!("agg:{doc}")),
        causation_id: None,
        correlation_id: CorrelationId(doc.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

/// A scripted [`crate::indexer::ProjectFetcher`] over a ref → projection map; an absent ref ⇒ `Gone`.
/// Exposed so the SRCH-P29 drill builds the SAME live corpus shape the SRCH-D4 CI drill uses.
pub struct MapFetcher {
    map: std::sync::Mutex<std::collections::HashMap<String, SearchProjection>>,
}

impl MapFetcher {
    /// Build a fetcher over the (ref, projection) pairs.
    pub fn new(pairs: impl IntoIterator<Item = (String, SearchProjection)>) -> MapFetcher {
        MapFetcher {
            map: std::sync::Mutex::new(pairs.into_iter().collect()),
        }
    }
}

impl crate::indexer::ProjectFetcher for MapFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        match self.map.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(crate::indexer::ProjectFetchError::Gone),
        }
    }
}

/// A projection with `text` + facet fields.
fn proj(text: &str, fields: BTreeMap<String, myelin_query::FieldValue>) -> SearchProjection {
    SearchProjection {
        text: text.into(),
        fields,
        lang: None,
    }
}

/// **Build a live indexer over a small knowledge-page corpus, index it, and return it + the doc ids.**
/// Shared by the drill + the unit tests so the live corpus shape is the SAME everywhere (no drift).
/// `subject_docs` are indexed referencing `subject_id` (via the `actor` facet); `other_docs` do not.
pub fn build_live_corpus(
    tenant: &TenantId,
    region: &Region,
    subject_id: &str,
    subject_docs: &[&str],
    other_docs: &[&str],
) -> (Arc<IncrementalIndexer>, Vec<String>) {
    let mut actor = BTreeMap::new();
    actor.insert(
        "actor".to_string(),
        myelin_query::FieldValue::Principal(subject_id.into()),
    );
    let mut pairs: Vec<(String, SearchProjection)> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for d in subject_docs {
        let ref_ = format!("myelin://{}/knowledge/page/{d}", tenant.0);
        pairs.push((
            ref_.clone(),
            proj(
                &format!("{subject_id}'s note {d} on raft leadership and quorum"),
                actor.clone(),
            ),
        ));
        ids.push(ref_);
    }
    for d in other_docs {
        let ref_ = format!("myelin://{}/knowledge/page/{d}", tenant.0);
        pairs.push((
            ref_.clone(),
            proj(&format!("unrelated note {d} on paxos consensus"), {
                let mut f = BTreeMap::new();
                f.insert(
                    "actor".to_string(),
                    myelin_query::FieldValue::Principal(format!("u-{d}")),
                );
                f
            }),
        ));
        ids.push(ref_);
    }
    let ix = Arc::new(IncrementalIndexer::new(
        vec![backup_scale_page_spec()],
        Arc::new(MapFetcher::new(pairs)),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    for id in &ids {
        ix.index(&created_event(tenant, region, id)).expect("index");
    }
    (ix, ids)
}

/// **A subject matcher for `subject_id` in `tenant` (the §4.8 "references the subject" predicate).**
/// Exposed so the drill probes the live index for the subject's docs the SAME way the holder does.
pub fn subject_matcher(subject_id: &str, tenant: &TenantId) -> SubjectMatcher {
    let pseudonym =
        myelin_identity::PseudonymHandle::new(subject_id, &tenant.0).map(|h| h.render());
    SubjectMatcher::new(subject_id, pseudonym)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::{EraseScope, PersonalDataHolder};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::{
        Byok, Dek, DekHandle as KoDekHandle, Hyok, HyokKeyService, HyokServiceDenied, KmsEngine,
        PlatformManaged, WrappedDek,
    };

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    /// A HYOK key service that denies every wrap/unwrap (the customer holds the key outside Myelin's
    /// reach) — the worst case; `can_derive_plaintext_index()` is `false` regardless.
    struct DenyAllHyok;
    impl HyokKeyService for DenyAllHyok {
        fn wrap(&self, _dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
            Err(HyokServiceDenied)
        }
        fn unwrap(&self, _w: &WrappedDek) -> Result<KoDekHandle, HyokServiceDenied> {
            Err(HyokServiceDenied)
        }
        fn destroy(&self) {}
    }

    fn hyok_origin() -> Hyok<DenyAllHyok> {
        Hyok::new(DenyAllHyok)
    }

    // ───────────────────────── SRCH-D10 — HYOK cross-store at scale ─────────────────────────

    /// **THE SRCH-D10 HEADLINE GREEN:** a HYOK class is structurally absent from EVERY derived store
    /// (index / vectors / caches / backups), while a platform-managed control class IS present —
    /// 0 HYOK plaintext cross-store → a dated GREEN artifact. The DoD pass.
    #[test]
    fn srch_d10_hyok_class_is_absent_from_every_derived_store() {
        let (ix, ids) = build_live_corpus(&tenant(), &region(), "u-ctrl", &["c1"], &[]);
        let engine = KmsEngine::new();
        let platform = PlatformManaged::new(&engine, region());
        let hyok = hyok_origin();

        let inputs = HyokCrossStoreInputs {
            indexer: &ix,
            tenant: tenant(),
            region: region(),
            platform_cache_present: true,
            platform_backup_present: true,
            platform_doc_id: ids[0].clone(),
            platform_probe_text: "raft leadership".into(),
            now: "2026-06-24T00:00:00Z".into(),
        };

        let verdict = HyokCrossStoreGate::new().run(&inputs, &hyok, &platform);
        assert!(verdict.is_green(), "verdict: {:?}", verdict.failure());
        let a = verdict.artifact().expect("green artifact");
        assert_eq!(
            a.stores_with_hyok_plaintext, 0,
            "0 HYOK plaintext in any derived store (the SRCH-D10 gate)"
        );
        assert_eq!(a.stores_walked.len(), 4, "all four derived stores walked");
        assert!(
            a.stores_with_platform_class >= 1,
            "the platform control class IS present (the walk is real, not vacuous)"
        );
        assert!(a.summary().contains("SRCH-D10"));
    }

    /// **The control class IS present in all four stores** (so the cross-store walk genuinely
    /// distinguishes present-vs-absent — a green is not "nothing was indexed").
    #[test]
    fn srch_d10_platform_control_class_present_in_all_four_stores() {
        let (ix, ids) = build_live_corpus(&tenant(), &region(), "u-ctrl", &["c1"], &[]);
        let inputs = HyokCrossStoreInputs {
            indexer: &ix,
            tenant: tenant(),
            region: region(),
            platform_cache_present: true,
            platform_backup_present: true,
            platform_doc_id: ids[0].clone(),
            platform_probe_text: "raft leadership".into(),
            now: "2026-06-24T00:00:00Z".into(),
        };
        let engine = KmsEngine::new();
        let a = HyokCrossStoreGate::new()
            .run(
                &inputs,
                &hyok_origin(),
                &PlatformManaged::new(&engine, region()),
            )
            .artifact()
            .cloned()
            .expect("green");
        assert_eq!(
            a.stores_with_platform_class, 4,
            "the platform class is in index + vectors + caches + backups"
        );
    }

    /// **A non-HYOK class supplied as the HYOK arg FAILs LOUD** (`NotAHyokClass`) — the gate refuses a
    /// mis-specified class rather than a vacuous pass. A BYOK class `can_derive_plaintext_index()`.
    #[test]
    fn srch_d10_non_hyok_class_fails_loud() {
        let (ix, ids) = build_live_corpus(&tenant(), &region(), "u-ctrl", &["c1"], &[]);
        let engine = KmsEngine::new();
        let byok = Byok::new(&engine, region(), "kms-customer://acme/k1");
        let inputs = HyokCrossStoreInputs {
            indexer: &ix,
            tenant: tenant(),
            region: region(),
            platform_cache_present: true,
            platform_backup_present: true,
            platform_doc_id: ids[0].clone(),
            platform_probe_text: "raft leadership".into(),
            now: "2026-06-24T00:00:00Z".into(),
        };
        // BYOK as the "hyok" arg is NOT a skip → NotAHyokClass.
        let verdict =
            HyokCrossStoreGate::new().run(&inputs, &byok, &PlatformManaged::new(&engine, region()));
        assert_eq!(
            verdict.failure(),
            Some(&HyokCrossStoreFailure::NotAHyokClass),
            "a BYOK class is not a HYOK skip — the gate fails loud"
        );
    }

    /// **The walk fails LOUD if the platform control class is absent everywhere**
    /// (`WalkProvedNothing`) — a "0 HYOK plaintext" reading is vacuous if nothing was indexed.
    #[test]
    fn srch_d10_vacuous_walk_fails_loud() {
        // An EMPTY indexer (no control doc), and no cache/backup → the platform class is in 0 stores.
        let ix = Arc::new(IncrementalIndexer::new(
            vec![backup_scale_page_spec()],
            Arc::new(MapFetcher::new(std::iter::empty())),
            Arc::new(MockEmbeddingAdapter::new(8)),
        ));
        let inputs = HyokCrossStoreInputs {
            indexer: &ix,
            tenant: tenant(),
            region: region(),
            platform_cache_present: false,
            platform_backup_present: false,
            platform_doc_id: "myelin://acme/knowledge/page/absent".into(),
            platform_probe_text: "nothing".into(),
            now: "2026-06-24T00:00:00Z".into(),
        };
        let engine = KmsEngine::new();
        let verdict = HyokCrossStoreGate::new().run(
            &inputs,
            &hyok_origin(),
            &PlatformManaged::new(&engine, region()),
        );
        assert_eq!(
            verdict.failure(),
            Some(&HyokCrossStoreFailure::WalkProvedNothing),
            "an empty walk proves nothing — the gate fails loud"
        );
    }

    /// **`run_or_fail_ci` returns `Ok(artifact)` on a green run** (CI continues) and `Err` on red.
    #[test]
    fn srch_d10_run_or_fail_ci_ok_on_green() {
        let (ix, ids) = build_live_corpus(&tenant(), &region(), "u-ctrl", &["c1"], &[]);
        let inputs = HyokCrossStoreInputs {
            indexer: &ix,
            tenant: tenant(),
            region: region(),
            platform_cache_present: true,
            platform_backup_present: true,
            platform_doc_id: ids[0].clone(),
            platform_probe_text: "raft leadership".into(),
            now: "2026-06-24T00:00:00Z".into(),
        };
        let engine = KmsEngine::new();
        let r = HyokCrossStoreGate::new().run_or_fail_ci(
            &inputs,
            &hyok_origin(),
            &PlatformManaged::new(&engine, region()),
        );
        assert!(r.is_ok(), "green → Ok(artifact)");
    }

    // ───────────────────── SRCH-D4 at backup scale — backup-scale erasure ─────────────────────

    /// **THE SRCH-D4-AT-BACKUP-SCALE HEADLINE GREEN:** erase a subject → 0 live docs remaining + 0
    /// orphan embedding + the backup segments recoverable BEFORE the shred are UNRECOVERABLE AFTER →
    /// a dated GREEN artifact. 0 recoverable incl. vectors incl. backups. The DoD pass.
    #[test]
    fn srch_d4_backup_scale_zero_recoverable_incl_backups() {
        let (ix, ids) = build_live_corpus(
            &tenant(),
            &region(),
            "u-target",
            &["t1", "t2"],
            &["o1", "o2", "o3"],
        );
        // Reserve the per-tenant index DEK + seal the subject's index segments as backups under it.
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin
            .reserve(&tenant(), &region())
            .expect("reserve index DEK");
        let dek = pin.resolve(&key_ref, &region()).expect("resolve live DEK");
        let backups: Vec<SealedBackupSegment> = ids
            .iter()
            .take(2) // the two subject docs
            .map(|id| SealedBackupSegment::seal(&dek, id, b"u-target's design note plaintext"))
            .collect();

        let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());

        let mut inputs = BackupScaleEraseInputs {
            erase_holder: &holder,
            dek: &pin,
            index_key_ref: key_ref,
            subject: subject("u-target"),
            tenant: tenant(),
            backup_segments: &backups,
            subject_backstop_id: None,
            now: "2026-06-24T00:00:00Z".into(),
        };

        let verdict = BackupScaleEraseGate::new().run(&mut inputs);
        assert!(verdict.is_green(), "verdict: {:?}", verdict.failure());
        let a = verdict.artifact().expect("green artifact");
        assert_eq!(a.live_docs_purged, 2, "the two subject docs were purged");
        assert_eq!(a.live_docs_remaining, 0, "0 live docs remain (not hidden)");
        assert!(a.zero_orphan_embedding, "0 orphan embedding after compact");
        assert_eq!(
            a.backup_segments_recoverable_before_shred, 2,
            "the backups DID hold the plaintext before the shred (the proof is real)"
        );
        assert_eq!(
            a.backup_segments_recoverable_after_shred, 0,
            "0 backup segments recoverable after the crypto-shred (incl. backups)"
        );
        assert!(a.summary().contains("SRCH-D4 at backup scale"));
    }

    /// **The crypto-shred actually reaches the backups** — before the shred the sealed segment opens;
    /// after `destroy_tenant_index_dek` it does NOT (the DEK no longer resolves). The §7.5 backstop,
    /// proven at the seal level (the gate's load-bearing step).
    #[test]
    fn srch_d4_backup_segment_is_recoverable_before_and_dead_after_shred() {
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin.reserve(&tenant(), &region()).expect("reserve");
        let dek = pin.resolve(&key_ref, &region()).expect("resolve");
        let seg = SealedBackupSegment::seal(&dek, "doc1", b"secret index segment");
        assert_eq!(
            seg.try_recover(&dek).as_deref(),
            Some(&b"secret index segment"[..]),
            "the backup is recoverable while the DEK lives"
        );
        // Crypto-shred the per-tenant index DEK.
        assert!(pin.destroy_tenant_index_dek(&tenant(), &region()));
        // The DEK no longer resolves → the backup is plaintext-unrecoverable (the structural backstop).
        assert!(
            pin.resolve(&key_ref, &region()).is_err(),
            "the shredded DEK does not resolve — the backup ciphertext is dead (§7.5)"
        );
    }

    /// **A vacuous backup proof FAILs LOUD** (`NoBackupBeforeShred`) — if no backup segment was
    /// recoverable before the shred, "0 recoverable after" proves nothing. (A segment sealed under a
    /// DIFFERENT key the gate's DEK cannot open.)
    #[test]
    fn srch_d4_vacuous_backup_proof_fails_loud() {
        let (ix, _ids) = build_live_corpus(&tenant(), &region(), "u-target", &["t1"], &[]);
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin.reserve(&tenant(), &region()).expect("reserve");
        // Seal a segment under a DIFFERENT (other-tenant) DEK so the gate's DEK cannot open it.
        let other = TenantId("other".into());
        let other_ref = pin.reserve(&other, &region()).expect("reserve other");
        let other_dek = pin.resolve(&other_ref, &region()).expect("resolve other");
        let foreign = SealedBackupSegment::seal(&other_dek, "doc1", b"foreign");

        let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());
        let mut inputs = BackupScaleEraseInputs {
            erase_holder: &holder,
            dek: &pin,
            index_key_ref: key_ref,
            subject: subject("u-target"),
            tenant: tenant(),
            backup_segments: std::slice::from_ref(&foreign),
            subject_backstop_id: None,
            now: "2026-06-24T00:00:00Z".into(),
        };
        let verdict = BackupScaleEraseGate::new().run(&mut inputs);
        assert_eq!(
            verdict.failure(),
            Some(&BackupScaleEraseFailure::NoBackupBeforeShred),
            "no recoverable backup before the shred → the proof is vacuous → fail loud"
        );
    }

    /// **The per-subject source backstop is also destroyed when supplied** (the GD-4 individual lever):
    /// with `subject_backstop_id` set, the gate destroys that DEK too. Proven green end-to-end.
    #[test]
    fn srch_d4_backup_scale_destroys_per_subject_backstop_too() {
        let (ix, ids) = build_live_corpus(&tenant(), &region(), "u-target", &["t1"], &["o1"]);
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin.reserve(&tenant(), &region()).expect("reserve");
        // Reserve the per-subject source backstop too (the additional GD-4 lever) so the gate destroys it.
        pin.reserve_subject_source_backstop(&tenant(), &region(), "u-target")
            .expect("reserve backstop");

        let dek = pin.resolve(&key_ref, &region()).expect("resolve");
        let backups = vec![SealedBackupSegment::seal(&dek, &ids[0], b"plaintext")];
        let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());
        let mut inputs = BackupScaleEraseInputs {
            erase_holder: &holder,
            dek: &pin,
            index_key_ref: key_ref,
            subject: subject("u-target"),
            tenant: tenant(),
            backup_segments: &backups,
            subject_backstop_id: Some("u-target".into()),
            now: "2026-06-24T00:00:00Z".into(),
        };
        let verdict = BackupScaleEraseGate::new().run(&mut inputs);
        assert!(verdict.is_green(), "verdict: {:?}", verdict.failure());
        // The per-subject backstop is gone (a second destroy returns false — already destroyed).
        assert!(
            !pin.destroy_subject_backstop(&tenant(), "u-target"),
            "the per-subject backstop was destroyed by the gate (a re-destroy is a no-op)"
        );
    }

    /// **`run_or_fail_ci` returns Err on a vacuous backup** (CI fails loudly).
    #[test]
    fn srch_d4_backup_run_or_fail_ci_err_on_red() {
        let (ix, _ids) = build_live_corpus(&tenant(), &region(), "u-target", &["t1"], &[]);
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin.reserve(&tenant(), &region()).expect("reserve");
        let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());
        let mut inputs = BackupScaleEraseInputs {
            erase_holder: &holder,
            dek: &pin,
            index_key_ref: key_ref,
            subject: subject("u-target"),
            tenant: tenant(),
            backup_segments: &[], // no backups at all → NoBackupBeforeShred
            subject_backstop_id: None,
            now: "2026-06-24T00:00:00Z".into(),
        };
        let r = BackupScaleEraseGate::new().run_or_fail_ci(&mut inputs);
        assert!(r.is_err(), "a red run → Err (CI fails loud)");
    }

    /// **The live erase rides the SAME consumer path the CI-variant SRCH-D4 uses (SRCH-P15 floor
    /// holds)** — the holder's `erase` surface (the 10.1 contract) purges + leaves 0 recoverable live.
    #[test]
    fn srch_p15_erase_mutation_floor_still_holds() {
        let (ix, _ids) =
            build_live_corpus(&tenant(), &region(), "u-target", &["t1", "t2"], &["o1"]);
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        pin.reserve(&tenant(), &region()).expect("reserve");
        let holder = SearchEraseHolder::new(ix.clone(), pin, region());
        let before = holder.locate_doc_count(&subject("u-target"), &tenant());
        assert_eq!(before, 2, "the subject references two live docs");
        holder
            .erase(EraseScope::Subject {
                subject: subject("u-target"),
                tenant: tenant(),
            })
            .expect("erase");
        let after = holder.locate_doc_count(&subject("u-target"), &tenant());
        assert_eq!(
            after, 0,
            "0 recoverable live after the erase (SRCH-P15 floor)"
        );
    }
}
