//! The **encrypted-from-birth, per-tenant, residency-pinned index layout** + the S1–S5
//! stateful-component register (SRCH-P03 / P-166; contracts 1.5 the forward-only migration +
//! 11.3 the per-tenant index DEK consumed).
//!
//! **Owning architecture doc:** `search-and-indexing.md`
//! - §3.4 ("Index layout, residency, and the stateful-component register"): the per-tenant index
//!   *directory* lives in the tenant's cell, `(tenant, region)`-keyed, residency-pinned; per-tenant
//!   index directories give "residency + crypto-shred-per-index for free"; **no cross-region index
//!   read on personal data**. The register **S1–S5** — S1 per-tenant FT+structured; S2 per-tenant
//!   vector; S3 indexer dedup ledger; S4 reindex cursor; S5 query/`list_objects`-filter cache —
//!   are **all derived and rebuildable** by reindex-from-source. There is **no system-of-record
//!   state in Search**.
//! - §3.1 (the index document is envelope-encrypted with the **per-tenant index DEK**; the whole
//!   doc — analyzed text, columnar fast-fields, vector HNSW — lives in one per-tenant index space
//!   keyed by the same `doc_id`).
//! - §4.8 (the per-tenant index DEK is the **tenant-decommission crypto-shred** unit + the
//!   backup/immutable-segment backstop; destroying it renders the whole tenant index
//!   plaintext-unrecoverable — the SRCH-D4 backstop substrate).
//!
//! ## What SRCH-P03 ships here — the SHELL's encrypted layout, NOT a working engine
//! This prompt builds the per-tenant index *layout* (the directory + its envelope-encryption under
//! the per-tenant index DEK reserved in SRCH-P02 ([`crate::dek::SearchDekPin`]) + the S1–S5 register
//! declared as **empty derived scaffolding**). It deliberately ships **no `IndexBackend`, no
//! Tantivy, no vector HNSW, no indexer, no query path** — those are SRCH-P04 (the `IndexBackend`
//! trait + FT/structured shapes), SRCH-P05 (the vector shape), SRCH-P06 (the indexer), SRCH-P08
//! (the query path). See [`SrchP03Floor`] — the engine-shapes follow-on floor named in code so the
//! shell is not mistaken for a working engine.
//!
//! ## Encrypted-from-birth (the structural crypto-shred check — the SRCH-D4 backstop substrate)
//! A per-tenant index directory is created **already** travelling with its per-tenant index DEK ref
//! (`pii_key_ref`): every segment written into it is sealed under that DEK. There is no plaintext
//! index moment — the directory is encrypted from the instant the migration creates it (it has no
//! contents yet, but the next byte written goes through the DEK). [`PerTenantIndexLayout::seal`]
//! seals a segment body under the DEK; [`PerTenantIndexLayout::open`] reads it back. After the DEK
//! is crypto-shredded ([`crate::dek::SearchDekPin::destroy_tenant_index_dek`]) the directory is
//! **unrecoverable** — `open` returns a LOUD [`LayoutError::Unrecoverable`], never a plaintext
//! fall-through. This is the structural crypto-shred check the prompt's GATE/DRILLS names; the real
//! 0-recoverable-incl-vectors drill over real index data (SRCH-D4) is SRCH-P15 (named floor).

use myelin_storage::{KmsError, PiiKeyRef, NONCE_LEN};
use myelin_tenancy::{Region, ResidencyTag, TenantId};

use crate::dek::SearchDekPin;
use crate::holder::SEARCH_INDEX_STORE;

/// The five derived, rebuildable stateful components of a Search cell (architecture §3.4). **All
/// derived and rebuildable by reindex-from-source — there is no system-of-record state in Search.**
/// At SRCH-P03 these are **empty declarations** (scaffolding): the code that fills each one is a
/// later slice (named per variant). The register exists now so the shell declares its complete
/// derived-state surface up front (so a later slice cannot smuggle in a sixth, un-declared,
/// non-rebuildable store).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatefulComponent {
    /// **S1 — per-tenant full-text + structured/columnar index.** The Tantivy FT inverted index +
    /// the structured/columnar fast-fields, in one per-tenant index space keyed by `doc_id` (§3.1).
    /// Filled by SRCH-P04 (the `IndexBackend` trait + the FT/structured shapes) + SRCH-P06 (the
    /// indexer that upserts into it).
    S1FtStructured,
    /// **S2 — per-tenant vector index (HNSW).** Co-located in the SAME per-tenant index space,
    /// keyed by the same `doc_id`, so a vector hit and a keyword hit are the same document (§3.1).
    /// Filled by SRCH-P05 (the vector shape) + SRCH-P06 (the embedder/indexer).
    S2Vector,
    /// **S3 — indexer dedup ledger.** The idempotency ledger keyed on `event_id` (the indexer is an
    /// ordinary `myelin-events` consumer; idempotent on `event_id`, §4.1). Filled by SRCH-P06.
    S3DedupLedger,
    /// **S4 — reindex cursor store.** The throttled/resumable reindex-from-source cursor (one code
    /// path for steady-state + recovery, §4.9). Filled by SRCH-P16 (the reindex slice).
    S4ReindexCursor,
    /// **S5 — query / `list_objects`-filter cache.** Caches the typed `ListObjectsResult`
    /// (`Ids` or `Filter{set_expr}`) per `(tenant, region, subject, type, zookie-bucket)`,
    /// `TTL ≤ revocation SLA`, never source of truth (§3.4 / §4.10). Filled by SRCH-P13.
    S5FilterCache,
}

impl StatefulComponent {
    /// The full S1–S5 register (architecture §3.4), in order. Declared up front as the shell's
    /// complete derived-state surface; each is filled by a named later slice.
    pub fn register() -> [StatefulComponent; 5] {
        [
            StatefulComponent::S1FtStructured,
            StatefulComponent::S2Vector,
            StatefulComponent::S3DedupLedger,
            StatefulComponent::S4ReindexCursor,
            StatefulComponent::S5FilterCache,
        ]
    }

    /// The stable, PII-free id of the component (for the data-map / telemetry).
    pub fn id(self) -> &'static str {
        match self {
            StatefulComponent::S1FtStructured => "S1",
            StatefulComponent::S2Vector => "S2",
            StatefulComponent::S3DedupLedger => "S3",
            StatefulComponent::S4ReindexCursor => "S4",
            StatefulComponent::S5FilterCache => "S5",
        }
    }

    /// **Every Search stateful component is derived and rebuildable by reindex-from-source**
    /// (§3.4: "all derived and rebuildable … there is no system-of-record state in Search").
    /// `true` for every variant — recorded as a checked fact, not prose, so a later slice that
    /// tries to add a non-rebuildable store fails the [`derived_state_invariant_holds`] assertion.
    pub fn is_derived_rebuildable(self) -> bool {
        // EVERY Search component is derived from the source-of-truth event stream / owner project()
        // — none is a system of record. This is the load-bearing GDPR + DR property (a lost index
        // is reindexed, never restored-as-truth).
        match self {
            StatefulComponent::S1FtStructured
            | StatefulComponent::S2Vector
            | StatefulComponent::S3DedupLedger
            | StatefulComponent::S4ReindexCursor
            | StatefulComponent::S5FilterCache => true,
        }
    }

    /// The later slice that FILLS this empty SRCH-P03 declaration (named so the scaffolding is not
    /// mistaken for a working component).
    pub fn filled_by(self) -> &'static str {
        match self {
            StatefulComponent::S1FtStructured => {
                "SRCH-P04 (IndexBackend + FT/structured) + SRCH-P06 (indexer)"
            }
            StatefulComponent::S2Vector => "SRCH-P05 (vector shape) + SRCH-P06 (embedder/indexer)",
            StatefulComponent::S3DedupLedger => "SRCH-P06 (the indexer's idempotency ledger)",
            StatefulComponent::S4ReindexCursor => "SRCH-P16 (reindex-from-source cursor)",
            StatefulComponent::S5FilterCache => "SRCH-P13 (the list_objects filter/result cache)",
        }
    }
}

/// **The derived-state invariant (§3.4, recorded as a checked fact).** Every component of the
/// S1–S5 register is derived and rebuildable by reindex-from-source — there is no system-of-record
/// state in Search. Returns `true` iff the invariant holds over the whole register (it does, by
/// construction); the assertion exists so a later slice that adds a non-rebuildable component is a
/// loud failure here, not a silent GDPR/DR regression.
pub fn derived_state_invariant_holds() -> bool {
    StatefulComponent::register()
        .iter()
        .all(|c| c.is_derived_rebuildable())
}

/// A failure reading a per-tenant index segment — always LOUD, never a plaintext fall-through
/// (the 0-fail-open invariant the storage engine enforces, inherited here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// The per-tenant index DEK could not be resolved (a wrong region, or a crypto-shredded key).
    /// After a tenant-decommission crypto-shred the whole index reads this way — the directory is
    /// **unrecoverable** (the SRCH-D4 backstop substrate). Never a plaintext-without-key.
    Unrecoverable(KmsError),
    /// A segment ciphertext failed to open under a resolvable DEK (tamper / wrong-key) — surfaced
    /// loudly, never silently dropped.
    SegmentUnreadable,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::Unrecoverable(e) => write!(
                f,
                "the per-tenant index DEK is unavailable (crypto-shredded or wrong region): {e} — \
                 the per-tenant index directory is UNRECOVERABLE (never a plaintext fall-through)"
            ),
            LayoutError::SegmentUnreadable => {
                write!(
                    f,
                    "an index segment ciphertext failed to open under the per-tenant index DEK"
                )
            }
        }
    }
}

impl std::error::Error for LayoutError {}

/// **The encrypted-from-birth, per-tenant, residency-pinned index layout (architecture §3.4 /
/// §3.1).** The per-tenant index *directory* — `(tenant, region)`-keyed, cell-local,
/// residency-pinned — that the SRCH-P04+ `IndexBackend` opens. Created already travelling with its
/// per-tenant index DEK ref (`pii_key_ref`): every segment written into it is sealed under that DEK
/// (encrypted-from-birth, §3.1). Destroying the DEK renders the directory unrecoverable (§4.8 — the
/// tenant-decommission crypto-shred + backup backstop; the SRCH-D4 substrate).
///
/// **Floor (named):** this carries the *layout* — the residency keys, the DEK ref, the
/// seal/open seam over the DEK, and the empty S1–S5 register. It holds **no real index** (no
/// Tantivy, no vector HNSW, no segments) — those are SRCH-P04..P06. `seal`/`open` prove the
/// encrypted-from-birth + crypto-shred properties over an arbitrary segment body; the real index
/// segment FORMAT is SRCH-P04.
#[derive(Clone, Debug)]
pub struct PerTenantIndexLayout {
    /// The store class — the per-tenant search index ([`myelin_substrate::StoreKind::SearchIndex`]).
    /// PII-free.
    pub store: &'static str,
    /// The tenant the index directory belongs to (the FIRST partition key; the directory lives in
    /// THIS tenant's cell). PII-free opaque token.
    pub tenant: TenantId,
    /// The home region the directory is pinned to (residency-pin; the index lives in the tenant's
    /// cell, no cross-region read on personal data, §3.4).
    pub region: Region,
    /// The residency marker the `residency-pin` lint reads — pins the directory to `region`.
    pub residency: ResidencyTag,
    /// The per-tenant index DEK ref (`kms://<tenant>/<epoch>/tenant`) every segment is sealed under
    /// — the **encrypted-from-birth** anchor (§3.1). Destroying this DEK class shreds the whole
    /// directory (§4.8).
    pub index_dek_ref: PiiKeyRef,
}

impl PerTenantIndexLayout {
    /// **Create the per-tenant index directory encrypted-from-birth (the forward-only-migration
    /// payload).** Reserves the per-tenant index DEK in the cell's one KMS hierarchy via `pin`
    /// (idempotent — a restart re-opens the SAME directory under the SAME DEK; it never silently
    /// rotates and orphans existing ciphertext) and returns the `(tenant, region)`-keyed,
    /// residency-pinned layout already travelling with its DEK ref. No plaintext index moment: the
    /// directory is encrypted from the instant it is created (it has no segments yet, but the next
    /// byte written goes through the DEK).
    ///
    /// This is what the SRCH-P03 forward-only migration ([`crate::shell::SEARCH_INDEX_DIR_MIGRATION`])
    /// MEANS — the on-disk realization is the SRCH-P04 `IndexBackend`; here the migration's effect
    /// is modelled as the creation of this encrypted-from-birth layout (the DEK is reserved; the
    /// directory key + residency pin are fixed; nothing is decryptable without the DEK from byte 0).
    pub fn create(
        pin: &SearchDekPin,
        tenant: &TenantId,
        region: &Region,
    ) -> Result<PerTenantIndexLayout, KmsError> {
        // Reserve (idempotently) the per-tenant index DEK — the encrypted-from-birth anchor. A
        // re-create on restart returns the SAME ref (no silent rotation; never orphan ciphertext).
        let index_dek_ref = pin.reserve(tenant, region)?;
        Ok(PerTenantIndexLayout {
            store: SEARCH_INDEX_STORE,
            tenant: tenant.clone(),
            region: region.clone(),
            residency: ResidencyTag::pinned_to(region.clone()),
            index_dek_ref,
        })
    }

    /// **Seal a segment body under the per-tenant index DEK (encrypted-from-birth, §3.1).** Resolves
    /// the directory's DEK through `pin` (the SAME cell KMS engine) and seals `plaintext`, returning
    /// `(nonce, ciphertext)`. Every byte that enters the per-tenant index directory passes through
    /// this seam — there is no plaintext-write path. (The real index-segment writer is SRCH-P04/P06;
    /// here the seam proves the encrypted-from-birth + crypto-shred properties over an arbitrary
    /// body.)
    pub fn seal(
        &self,
        pin: &SearchDekPin,
        plaintext: &[u8],
    ) -> Result<([u8; NONCE_LEN], Vec<u8>), LayoutError> {
        let dek = pin
            .resolve(&self.index_dek_ref, &self.region)
            .map_err(LayoutError::Unrecoverable)?;
        Ok(dek.seal(plaintext))
    }

    /// **Open a sealed segment body (the read seam).** Resolves the directory's DEK and opens the
    /// `(nonce, ciphertext)`. After the DEK is crypto-shredded the resolve fails LOUDLY
    /// ([`LayoutError::Unrecoverable`]) — the directory is **unrecoverable**, NEVER a
    /// plaintext-without-key fall-through (the SRCH-D4 backstop substrate; §4.8). A resolvable-but-
    /// wrong ciphertext is [`LayoutError::SegmentUnreadable`], also loud.
    pub fn open(
        &self,
        pin: &SearchDekPin,
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, LayoutError> {
        let dek = pin
            .resolve(&self.index_dek_ref, &self.region)
            .map_err(LayoutError::Unrecoverable)?;
        dek.open(nonce, ciphertext)
            .ok_or(LayoutError::SegmentUnreadable)
    }
}

/// The engine-shapes follow-on floor for SRCH-P03 (named in code so the shell is not mistaken for a
/// working engine — VISION §3 / the prompt DoD). Each row names a deliverable this prompt does NOT
/// ship and the slice that fills it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SrchP03Floor {
    /// The deliverable NOT shipped here.
    pub deferred: &'static str,
    /// The later slice that ships it.
    pub filled_by: &'static str,
}

/// The named SRCH-P03 floors: this prompt ships the service SHELL + the encrypted layout only; the
/// `IndexBackend` trait + the three index shapes, the indexer, and the query path are the follow-ons
/// that make it answer anything. Named so the shell is not mistaken for a working engine.
pub fn srch_p03_floors() -> [SrchP03Floor; 5] {
    [
        SrchP03Floor {
            deferred: "the IndexBackend trait + Tantivy + the FT-inverted and structured/columnar shapes",
            filled_by: "SRCH-P04",
        },
        SrchP03Floor {
            deferred: "the per-tenant vector (HNSW) shape co-located in the index space",
            filled_by: "SRCH-P05",
        },
        SrchP03Floor {
            deferred: "the near-real-time incremental indexer (the evt.* consumer)",
            filled_by: "SRCH-P06",
        },
        SrchP03Floor {
            deferred: "the permission-aware query path (engine.search behind the composed ACL filter)",
            filled_by: "SRCH-P08",
        },
        SrchP03Floor {
            deferred: "the REAL per-subject erase (purge + reindex incl. vectors) + the SRCH-D4 0-recoverable drill over real index data",
            filled_by: "SRCH-P15",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use myelin_storage::{KeyClass, KmsEngine};

    fn pin() -> SearchDekPin {
        SearchDekPin::new(Arc::new(KmsEngine::new()))
    }
    fn t() -> TenantId {
        TenantId::from_token("acme")
    }
    fn r() -> Region {
        Region("fr-par".into())
    }

    /// **The per-tenant index layout is created encrypted-from-birth, `(tenant, region)`-keyed +
    /// residency-pinned (§3.4 / §3.1).** Creating the directory reserves the per-tenant index DEK
    /// and the layout travels with its DEK ref; the residency tag pins it to its home region.
    #[test]
    fn layout_is_created_encrypted_from_birth_and_residency_pinned() {
        let pin = pin();
        let layout = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("create the directory");

        assert_eq!(
            layout.store, SEARCH_INDEX_STORE,
            "the per-tenant search index store"
        );
        assert_eq!(
            layout.tenant,
            t(),
            "the directory is keyed to its tenant (first partition key)"
        );
        assert_eq!(
            layout.region,
            r(),
            "the directory lives in the tenant's cell region"
        );
        assert_eq!(
            layout.residency,
            ResidencyTag::pinned_to(r()),
            "residency-pinned to its region"
        );
        assert_eq!(
            layout.index_dek_ref.class,
            KeyClass::Tenant,
            "sealed under the per-tenant index DEK"
        );
        assert_eq!(
            layout.index_dek_ref.to_uri(),
            "kms://acme/0/tenant",
            "the encrypted-from-birth ref"
        );
    }

    /// **Encrypted-from-birth: a segment written into the directory is sealed under the per-tenant
    /// index DEK and round-trips (§3.1).** There is no plaintext-write path — `seal` goes through
    /// the DEK; `open` reads it back. This is the encrypted-from-birth property over an arbitrary
    /// segment body (the real segment format is SRCH-P04).
    #[test]
    fn a_segment_is_sealed_under_the_index_dek_and_round_trips() {
        let pin = pin();
        let layout = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("create");

        let body = b"a future FT+vector index segment's body";
        let (nonce, ct) = layout
            .seal(&pin, body)
            .expect("seal under the per-tenant index DEK");
        assert_ne!(
            &ct[..],
            &body[..],
            "the segment is ciphertext at rest (encrypted-from-birth)"
        );
        let plain = layout
            .open(&pin, &nonce, &ct)
            .expect("open the sealed segment");
        assert_eq!(
            plain, body,
            "the sealed segment round-trips under the per-tenant index DEK"
        );
    }

    /// **Destroying the per-tenant index DEK renders the directory UNRECOVERABLE (the structural
    /// crypto-shred check — the SRCH-D4 backstop substrate, §4.8).** After
    /// `destroy_tenant_index_dek`, a previously-sealed segment opens to a LOUD
    /// [`LayoutError::Unrecoverable`] — NEVER a plaintext fall-through. This is the GATE/DRILLS
    /// crypto-shred check; the real 0-recoverable-incl-vectors drill over real index data is
    /// SRCH-P15 (named floor).
    #[test]
    fn destroying_the_dek_renders_the_directory_unrecoverable() {
        let pin = pin();
        let layout = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("create");

        // Seal a segment while the DEK lives.
        let (nonce, ct) = layout.seal(&pin, b"sensitive analyzed text").expect("seal");
        assert!(
            layout.open(&pin, &nonce, &ct).is_ok(),
            "readable before the shred"
        );

        // Tenant-decommission crypto-shred of the whole index directory.
        assert!(
            pin.destroy_tenant_index_dek(&t(), &r()),
            "the per-tenant index DEK is destroyable (the tenant-decommission shred lever fires)"
        );

        // Post-shred the directory is UNRECOVERABLE — loud, never a plaintext fall-through.
        match layout.open(&pin, &nonce, &ct) {
            Err(LayoutError::Unrecoverable(_)) => {}
            other => {
                panic!("a crypto-shredded index directory must be UNRECOVERABLE, got {other:?}")
            }
        }
        // A fresh seal also fails (the DEK is gone — there is no encrypted-from-birth write path
        // left either).
        assert!(matches!(
            layout.seal(&pin, b"x"),
            Err(LayoutError::Unrecoverable(_))
        ));
    }

    /// **Re-creating the directory on restart is idempotent (no silent DEK rotation).** A second
    /// `create` returns the SAME DEK ref — a restart re-opens the same encrypted directory, it does
    /// NOT rotate the DEK and orphan existing ciphertext.
    #[test]
    fn re_creating_the_directory_does_not_rotate_the_dek() {
        let pin = pin();
        let a = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("first create");
        let b = PerTenantIndexLayout::create(&pin, &t(), &r()).expect("re-create on restart");
        assert_eq!(
            a.index_dek_ref, b.index_dek_ref,
            "the same per-tenant index DEK (no silent rotation)"
        );

        // A segment sealed before the restart is still readable through the re-created layout.
        let (nonce, ct) = a.seal(&pin, b"pre-restart segment").expect("seal");
        assert_eq!(
            b.open(&pin, &nonce, &ct).expect("open after re-create"),
            b"pre-restart segment"
        );
    }

    /// **The S1–S5 register is the complete, ordered derived-state surface (§3.4).** All five are
    /// declared; each is derived + rebuildable; each names the later slice that fills it.
    #[test]
    fn s1_s5_register_is_complete_derived_and_each_names_its_filler() {
        let reg = StatefulComponent::register();
        assert_eq!(reg.len(), 5, "the register is exactly S1–S5");
        let ids: Vec<&str> = reg.iter().map(|c| c.id()).collect();
        assert_eq!(ids, ["S1", "S2", "S3", "S4", "S5"], "S1–S5 in order");

        // §3.4: every component is derived and rebuildable by reindex-from-source.
        assert!(
            derived_state_invariant_holds(),
            "no Search component is a system of record"
        );
        for c in reg {
            assert!(
                c.is_derived_rebuildable(),
                "{} is derived/rebuildable",
                c.id()
            );
            assert!(
                !c.filled_by().is_empty(),
                "{} names the slice that fills it",
                c.id()
            );
        }
    }

    /// **The SRCH-P03 engine-shapes floor is named (so the shell is not mistaken for a working
    /// engine).** The follow-on slices (IndexBackend/Tantivy, vector, indexer, query, real erase)
    /// are each named with the slice that ships them.
    #[test]
    fn the_engine_shapes_floor_is_named() {
        let floors = srch_p03_floors();
        assert_eq!(floors.len(), 5, "the five named engine-shapes follow-ons");
        let fillers: Vec<&str> = floors.iter().map(|f| f.filled_by).collect();
        for required in ["SRCH-P04", "SRCH-P05", "SRCH-P06", "SRCH-P08", "SRCH-P15"] {
            assert!(
                fillers.contains(&required),
                "the floor names {required} as a follow-on"
            );
        }
        for f in floors {
            assert!(!f.deferred.is_empty(), "each floor states what it defers");
        }
    }
}
