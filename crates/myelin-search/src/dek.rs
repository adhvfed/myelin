//! The Search per-tenant **index DEK** pin into the KMS hierarchy + the per-subject source-DEK
//! backstop + the HYOK structural-skip reference (SRCH-P02 / P-122; contracts 11.3 / 11.4 consumed).
//!
//! **Owning architecture doc:** `search-and-indexing.md` §3.4 (the per-tenant residency-pinned
//! index tier; per-tenant index directories give "residency + crypto-shred-per-index for free"),
//! §4.8 ("Crypto-shred layering, change #9": the **per-tenant index DEK** (`pii_key_ref`)
//! crypto-shreds the whole tenant index on tenant-decommission + **backstops** backups/immutable
//! segments; the Phase-5 **per-subject SOURCE DEK** (contract 11.4) is an **additional backstop**
//! on the source side; Search's PRIMARY per-subject erasure is **purge + reindex**, NOT the DEK),
//! and the **HYOK structural skip** (§4.8: `can_derive_plaintext_index() = false` ⇒ Search builds
//! NO plaintext index — there is nothing to encrypt or erase for that class).
//! **Reconciliation / VISION §3:** GDPR-safe by construction; name-your-floors.
//!
//! **Contract-index rows 11.3 / 11.4 (consumed, NOT re-implemented):** the three-level KMS
//! hierarchy `per-cell root (L0) → per-tenant KEK (L1) → per-tenant DEK (L2)` + the per-subject DEK
//! backstop + crypto-shred — all frozen in [`myelin_storage::KmsEngine`] (storage P-058). The
//! `KeyOrigin` trait + the [`myelin_storage::IndexAdmission`] HYOK gate are frozen in storage P-094.
//! SRCH-P02 PINS the Search index key classes INTO that one engine. It does **not** stand up a
//! second KMS (no parallel crypto): one cell root governs every store's keys, so Search reserves
//! ITS classes in the SAME engine the cell's other stores resolve through.
//!
//! ## What SRCH-P02 (P-122) ships — the per-tenant index DEK reservation
//! No index exists yet (the encrypted-from-birth per-tenant index layout is SRCH-P03, the
//! IndexBackend SRCH-P04, the indexer SRCH-P06). So this prompt **reserves the key CLASS** so the
//! S-M2 index is **encrypted-from-birth** and proves **destroy is callable** on it:
//! - [`SearchDekPin::reserve`] provisions, under the cell's [`KmsEngine`], the per-`(tenant, region)`
//!   KEK (L1) and the **per-tenant index DEK** (L2, [`KeyClass::Tenant`]) — THE Search
//!   tenant-decommission crypto-shred + backup-backstop unit. The returned [`PiiKeyRef`]
//!   (`kms://<tenant>/<epoch>/tenant`) is the `pii_key_ref` the SRCH-P03 per-tenant index layout
//!   travels with every encrypted segment (the `encrypted-from-birth` anchor).
//! - The **per-subject SOURCE DEK backstop** ([`KeyClass::Subject`], §4.8 / 11.4) is RESERVED on
//!   demand: [`SearchDekPin::reserve_subject_source_backstop`] provisions a distinct per-subject DEK
//!   so a subject's source-side payload (e.g. a CI log segment naming them) is crypto-shred-able at
//!   subject grain WITHOUT touching the tenant index DEK (the GD-4 individual lever). It is the
//!   ADDITIONAL backstop §4.8 names — the PRIMARY per-subject erasure is still purge + reindex.
//! - **Destroy is callable** on both classes — [`SearchDekPin::destroy_tenant_index_dek`] (the
//!   tenant-decommission shred of the whole index) and [`SearchDekPin::destroy_subject_backstop`]
//!   (the per-subject source shred) — proven STRUCTURALLY here (the key class exists + destroy
//!   returns true once + is then unrecoverable). The *real* crypto-shred OVER REAL INDEX DATA
//!   (SRCH-D4: 0 recoverable incl. vectors) is SRCH-P15; here the check is that the lever fires.
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **THE floor is the per-tenant index DEK** as the tenant-decommission crypto-shred + backup
//!   backstop UNIT — reserved + destroyable here. The **PRIMARY per-subject erasure by purge +
//!   reindex** is the follow-on, landing in **SRCH-P15** once the index exists. The DEK is NOT the
//!   whole erasure answer (a tenant-decommission shred erases the WHOLE tenant index, not one
//!   person) — it is the key class that makes the index encrypted-from-birth + crypto-shred-able.
//! - **No index / layout / migration / real ciphertext / real vectors ship here.** The index
//!   layout is SRCH-P03, the backends SRCH-P04/P05, the indexer SRCH-P06, the real purge+reindex
//!   erase SRCH-P15, the world-scale 0-recoverable shred drill (SRCH-D4) SRCH-P15. This prompt
//!   reserves keys; nothing is encrypted yet (there is no index to encrypt).
//! - **The HYOK structural skip is a REFERENCE here, wired in SRCH-P03/P15.** [`hyok_skips_index`]
//!   restates the §4.8 invariant in code (a `can_derive_plaintext_index()=false` class is NOT
//!   indexed — there is no plaintext to embed, so no index DEK is ever reserved for it). The LIVE
//!   index-builder consult of [`IndexAdmission`] lands when the indexer does (SRCH-P06/P15).
//!
//! ## The inherited M1 platform gates (named as the SRCH-P03 precondition — GATE/DRILLS)
//! Search does NOT re-prove the platform's M1 gates; it INHERITS them and cannot begin its M2 index
//! engine (SRCH-P03) over a red one. They are named here as machine-readable facts
//! ([`InheritedGate`] / [`srch_p03_inherited_gates`]) so the SRCH-P03 agent reads the precondition
//! list in code, not prose: STOR-D1/D2 (restore-verify), ID-D3 (cross-tenant 0), ID-D2
//! (fail-static), ID-D1 (disabled-user N≥5 min), CP-D2/CP-D3 (misroute + residency-pin). The
//! `DEPENDS-ON` edge (SRCH-P03 → SRCH-P02 + M1 fully green) makes this concrete; Search reads them,
//! never re-runs them.

use std::sync::Arc;

use myelin_storage::{DekId, IndexAdmission, KekId, KeyClass, KeyOrigin, KmsEngine, KmsError, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};

/// The Search index-DEK pin — reserves the Search key classes in the cell's one [`KmsEngine`]
/// (contract 11.3 / 11.4) so the (future) per-tenant index is **encrypted-from-birth** and the
/// per-tenant index DEK is a **destroyable** tenant-decommission crypto-shred + backup-backstop
/// unit.
///
/// It holds an `Arc<KmsEngine>` — the SAME engine the cell's other stores resolve DEKs through
/// (there is ONE cell root governing one hierarchy; Search does not own a second KMS). At SRCH-P02
/// no store opens at runtime; `serve` will hold this pin and provision the index DEK when the
/// encrypted-from-birth layout lands (SRCH-P03+). The pin proves the key class exists + destroy
/// fires.
#[derive(Clone)]
pub struct SearchDekPin {
    /// The cell-wide KMS engine (the per-cell root → per-tenant KEK → per-tenant DEK hierarchy,
    /// storage P-058). Shared, not Search-owned — the one engine the whole cell uses.
    kms: Arc<KmsEngine>,
}

impl SearchDekPin {
    /// Build a Search DEK pin over the cell's KMS engine (the SAME `Arc<KmsEngine>` the other cell
    /// stores share — one cell root, one hierarchy).
    pub fn new(kms: Arc<KmsEngine>) -> SearchDekPin {
        SearchDekPin { kms }
    }

    /// The Search **per-tenant index DEK class** (contract 11.4) — [`KeyClass::Tenant`]. The whole
    /// per-tenant index (full-text + structured/columnar + vector HNSW, all in one doc-id space,
    /// §3.4) seals under the per-tenant index DEK; destroying it is **tenant-decommission
    /// crypto-shred** of the entire tenant index (the tenant-offboard lever) AND the backup /
    /// immutable-segment backstop. THE floor unit this prompt reserves.
    pub fn tenant_index_dek_class() -> KeyClass {
        KeyClass::Tenant
    }

    /// The Search **per-subject SOURCE-DEK backstop class** (contract 11.4, §4.8) — `subject:<id>`,
    /// DISTINCT from the per-tenant index DEK. The ADDITIONAL backstop §4.8 names: a source-side
    /// payload that names a subject (e.g. a CI log segment, SRCH-P22) is sealed under THAT subject's
    /// DEK, so erasing the subject crypto-shreds exactly their source-side ciphertext (the GD-4
    /// individual lever) without touching the tenant index DEK. The id is the pseudonymous opaque
    /// principal id (never a name — EI-04 §1). Search's PRIMARY per-subject erasure remains purge +
    /// reindex (SRCH-P15); this is the backstop, not the primary mechanism.
    pub fn subject_source_dek_class(subject_id: &str) -> KeyClass {
        KeyClass::Subject(subject_id.to_string())
    }

    /// **Reserve the per-tenant Search index DEK in the KMS hierarchy (11.3 / 11.4).** Provisions
    /// the L1 per-`(tenant, region)` KEK (idempotently) and the L2 per-tenant index DEK
    /// ([`KeyClass::Tenant`]) under it, returning the [`PiiKeyRef`] (`kms://<tenant>/<epoch>/tenant`)
    /// that the SRCH-P03 per-tenant index layout travels with every encrypted segment (the
    /// **encrypted-from-birth** anchor — the `pii_key_ref` the index doc carries, §4.8). Idempotent:
    /// a second call returns the same key ref (the DEK is not silently rotated — that would orphan
    /// existing index ciphertext).
    ///
    /// This is the floor unit — the per-tenant index DEK as the tenant-decommission crypto-shred +
    /// backup-backstop unit. No data is encrypted yet (no index exists); this RESERVES the key so
    /// the index is encrypted-from-birth and the tenant-offboard lever is in place from day one.
    pub fn reserve(&self, tenant: &TenantId, region: &Region) -> Result<PiiKeyRef, KmsError> {
        // L1: provision the per-(tenant, region) KEK (idempotent — never rotates an existing one).
        self.kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        // L2: reserve the PER-TENANT INDEX DEK under it — the Search tenant-decommission shred unit.
        // The returned ref is what the SRCH-P03 index layout keys on (encrypted-from-birth).
        self.kms.ensure_dek(tenant, region, Self::tenant_index_dek_class())
    }

    /// **Reserve the per-subject source-DEK backstop for a subject (§4.8 / 11.4 ADDITIONAL
    /// backstop).** Provisions a DISTINCT per-subject DEK so a subject's source-side payload is
    /// crypto-shred-able at subject grain (the GD-4 individual lever) — without touching the tenant
    /// index DEK. Requires the L1 KEK (provisioned via [`Self::reserve`] or here). Idempotent per
    /// `(tenant, subject)`. This is the ADDED backstop, NOT Search's primary erasure (purge+reindex,
    /// SRCH-P15).
    pub fn reserve_subject_source_backstop(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
    ) -> Result<PiiKeyRef, KmsError> {
        self.kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        self.kms.ensure_dek(tenant, region, Self::subject_source_dek_class(subject_id))
    }

    /// **Tenant-decommission crypto-shred (destroy is callable on the per-tenant index key class).**
    /// Destroys the tenant KEK ⇒ EVERY Search DEK under it (the per-tenant index DEK + every
    /// per-subject source backstop) is unrecoverable, live AND in every backup (§7.5: a shredded key
    /// is excluded from backup, so a restore never resurrects it — the backup-backstop half). The
    /// whole tenant index becomes plaintext-unrecoverable. Returns `true` if a KEK was present.
    ///
    /// This is the STRUCTURAL proof SRCH-P02 owes: the lever EXISTS and FIRES. The real crypto-shred
    /// over real index data incl. vectors (SRCH-D4, 0 recoverable) is SRCH-P15 — it cannot be proven
    /// here because no index data exists yet (named floor).
    pub fn destroy_tenant_index_dek(&self, tenant: &TenantId, region: &Region) -> bool {
        self.kms.destroy_kek(&KekId::new(tenant.clone(), region.clone()))
    }

    /// **Per-subject source crypto-shred (destroy is callable on the per-subject backstop class).**
    /// Destroys ONE subject's source backstop DEK ⇒ that subject's source-side ciphertext is
    /// unrecoverable while the tenant index DEK + every other subject is untouched (the GD-4
    /// individual lever). Returns `true` if the backstop DEK was present to destroy.
    pub fn destroy_subject_backstop(&self, tenant: &TenantId, subject_id: &str) -> bool {
        self.kms.destroy_dek(&DekId::new(tenant.clone(), Self::subject_source_dek_class(subject_id)))
    }

    /// Resolve the per-tenant index DEK named by `key_ref` (the read-path key-resolution step the
    /// SRCH-P03 index read / SRCH-P06 indexer write will call). A destroyed-key / wrong-key resolve
    /// fails LOUDLY ([`KmsError`]) — **never** a plaintext-without-key fall-through (the 0-fail-open
    /// invariant the storage engine enforces). Exposed so the SRCH-P03 path resolves through the
    /// SAME pin.
    pub fn resolve(
        &self,
        key_ref: &PiiKeyRef,
        region: &Region,
    ) -> Result<myelin_storage::DekHandle, KmsError> {
        self.kms.resolve_dek(key_ref, region)
    }

    /// Borrow the underlying shared engine (so `serve` wires the SAME engine into the index store
    /// when SRCH-P03 lands — one cell root, one hierarchy, never a second KMS).
    pub fn engine(&self) -> &Arc<KmsEngine> {
        &self.kms
    }
}

impl std::fmt::Debug for SearchDekPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The engine redacts all key material; this only names the pin (never key bytes).
        f.debug_struct("SearchDekPin").field("kms", &self.kms).finish()
    }
}

/// **The HYOK structural-skip invariant restated in code (§4.8, contract 11.3).** A content class
/// whose [`KeyOrigin::can_derive_plaintext_index`] is `false` (HYOK — the customer holds the key
/// outside Myelin's reach) is **NOT indexed**: Search builds NO plaintext-derived index over it, so
/// there is no plaintext to embed/analyse and NO index DEK is ever reserved for it. *You cannot
/// index what you cannot decrypt.* Returns `true` iff the class is skipped (HYOK) — the same verdict
/// the frozen [`IndexAdmission::for_origin`] gate gives, surfaced here so SRCH-P02 records the
/// no-leak property by reference; the LIVE index-builder consult lands with the indexer
/// (SRCH-P06/P15). PII-free.
pub fn hyok_skips_index(origin: &dyn KeyOrigin) -> bool {
    matches!(IndexAdmission::for_origin(origin), IndexAdmission::SkipHyok)
}

// ───────────────────────── the inherited M1 platform gates (the SRCH-P03 precondition) ───────────

/// One inherited M1 platform gate Search depends on but does NOT re-prove — the precondition for
/// SRCH-P03 (the index engine). Named in code (not prose) so the SRCH-P03 agent reads the
/// precondition list machine-readably. Search cannot build the index over a red one of these (the
/// `DEPENDS-ON` edge makes it concrete). PII-free: a gate id + a one-line description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedGate {
    /// The gate's stable id (e.g. `STOR-D1`, `ID-D3`).
    pub id: &'static str,
    /// What the gate guarantees (the precondition Search leans on).
    pub guarantees: &'static str,
}

/// The inherited M1 platform gates that must be green before SRCH-P03 (the encrypted-from-birth
/// per-tenant index layout) may begin. Search INHERITS these — it does not re-run them; a
/// regression on any halts SRCH-P03 (the index cannot be built over a red restore-verify /
/// cross-tenant / fail-static / disabled-user / misroute / residency gate). This is the GATE/DRILLS
/// "name these inherited M1 platform gates as the precondition for SRCH-P03" requirement, in code.
pub fn srch_p03_inherited_gates() -> Vec<InheritedGate> {
    vec![
        InheritedGate {
            id: "STOR-D1",
            guarantees: "restore-verify: a restored copy is byte-faithful + the cross-seam \
                         consistency point holds (the permanent store gate; Search cannot build the \
                         index over an unrestorable source-of-truth store — it reindexes from it)",
        },
        InheritedGate {
            id: "STOR-D2",
            guarantees: "cell-kill RTO restore-verify: a cell can be rebuilt within RTO from \
                         backups (the permanent store gate; re-run on every store-touching change)",
        },
        InheritedGate {
            id: "ID-D3",
            guarantees: "cross-tenant authz = 0: no check ever leaks across tenants (Search's \
                         permission-aware query leans on this; a cross-tenant index read is \
                         impossible — SRCH-D3)",
        },
        InheritedGate {
            id: "ID-D2",
            guarantees: "fail-static authz: a KMS/Identity hiccup degrades to bounded-staleness, \
                         never fail-open (Search's DEK resolve + ACL filter inherit this posture)",
        },
        InheritedGate {
            id: "ID-D1",
            guarantees: "disabled-user revocation within N≥5 min: a revoked principal stops \
                         surfacing in results (Search's zookie/consistency path inherits the \
                         revocation SLA — TTL ≤ revocation SLA)",
        },
        InheritedGate {
            id: "CP-D2",
            guarantees: "misroute rejection: a request to the wrong cell is rejected, never served \
                         (the Search index is cell-local; a cross-cell index read is impossible)",
        },
        InheritedGate {
            id: "CP-D3",
            guarantees: "residency-pin: no cross-region read path on personal data (the per-tenant \
                         index directory is residency-pinned; the per-tenant index DEK is \
                         region-scoped via the KEK — §3.4)",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::{
        Byok, Dek, DekHandle, Hyok, HyokKeyService, HyokServiceDenied, PlatformManaged, WrappedDek,
    };

    fn kms() -> Arc<KmsEngine> {
        Arc::new(KmsEngine::new())
    }
    fn t() -> TenantId {
        TenantId("acme".into())
    }
    fn r() -> Region {
        Region("fr-par".into())
    }

    /// **The per-tenant Search index DEK class is reserved in the KMS hierarchy + the ref is the
    /// encrypted-from-birth anchor (11.3).** `reserve` provisions the L1 KEK + the L2 per-tenant
    /// index DEK and returns `kms://<tenant>/<epoch>/tenant` — the `pii_key_ref` the SRCH-P03 index
    /// layout keys on. The DEK is a real, resolvable key (a payload sealed under it round-trips).
    #[test]
    fn per_tenant_index_dek_is_reserved_and_resolvable() {
        let pin = SearchDekPin::new(kms());
        let key_ref = pin.reserve(&t(), &r()).expect("reserve the per-tenant Search index DEK");
        assert_eq!(key_ref.class, KeyClass::Tenant, "the Search index class is per-tenant");
        assert_eq!(key_ref.to_uri(), "kms://acme/0/tenant", "the encrypted-from-birth key ref");

        // The reserved DEK is a REAL key: a payload sealed under it round-trips (encrypted-from-birth
        // is not a stub — the key is usable from the moment the index lands).
        let dek = pin.resolve(&key_ref, &r()).expect("resolve the reserved per-tenant index DEK");
        let (nonce, ct) = dek.seal(b"a future index segment's encrypted body");
        assert_eq!(
            dek.open(&nonce, &ct).as_deref(),
            Some(&b"a future index segment's encrypted body"[..])
        );
    }

    /// **Reserving the per-tenant index DEK is idempotent** — `serve` re-running the reservation on
    /// a restart returns the SAME key ref (it does NOT silently rotate the DEK, which would orphan
    /// existing index ciphertext).
    #[test]
    fn reserve_is_idempotent() {
        let pin = SearchDekPin::new(kms());
        let a = pin.reserve(&t(), &r()).expect("first reserve");
        let b = pin.reserve(&t(), &r()).expect("second reserve");
        assert_eq!(a, b, "re-reserving returns the same per-tenant index DEK ref (no silent rotation)");
    }

    /// **The per-subject source backstop is a DISTINCT key from the per-tenant index DEK (§4.8 /
    /// GD-4).** A source-side payload naming a subject seals under the subject's backstop; a payload
    /// sealed under it does NOT open under the tenant index DEK — so destroying the subject backstop
    /// erases exactly that one subject's source ciphertext, the tenant index untouched. (The PRIMARY
    /// per-subject erasure is still purge+reindex; this is the ADDED backstop.)
    #[test]
    fn per_subject_source_backstop_is_distinct_from_the_tenant_index_dek() {
        let pin = SearchDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant index dek");
        let sk = pin
            .reserve_subject_source_backstop(&t(), &r(), "u-1")
            .expect("subject source backstop");
        assert_ne!(tk, sk, "the per-subject source backstop is a distinct key ref");
        assert_eq!(sk.class, KeyClass::Subject("u-1".into()));

        let tdek = pin.resolve(&tk, &r()).expect("resolve tenant index dek");
        let sdek = pin.resolve(&sk, &r()).expect("resolve subject source dek");
        let (nonce, ct) = sdek.seal(b"a CI log segment naming the subject");
        assert!(
            tdek.open(&nonce, &ct).is_none(),
            "the tenant index DEK must not open a subject-backstop ciphertext (GD-4 subject grain)"
        );
    }

    /// **Destroy is callable on the per-tenant index key class — tenant-decommission crypto-shred
    /// (the SRCH-P02 structural GATE).** After `destroy_tenant_index_dek`, the reserved DEK is
    /// unrecoverable — a resolve fails LOUDLY ([`KmsError`]), NEVER a plaintext fall-through. The
    /// lever exists and fires; the real shred over real index data incl. vectors is SRCH-P15
    /// (SRCH-D4, named floor).
    #[test]
    fn destroy_tenant_index_dek_is_callable_and_renders_the_key_unrecoverable() {
        let pin = SearchDekPin::new(kms());
        let key_ref = pin.reserve(&t(), &r()).expect("reserve");
        assert!(pin.resolve(&key_ref, &r()).is_ok(), "resolvable before the shred");

        // The structural proof: destroy fires exactly once on a present key class.
        assert!(pin.destroy_tenant_index_dek(&t(), &r()), "destroy is callable + a key was present");
        assert!(!pin.destroy_tenant_index_dek(&t(), &r()), "a second destroy reports nothing left");

        // Post-shred the key is unrecoverable — LOUD failure, never plaintext-without-key.
        assert!(
            matches!(pin.resolve(&key_ref, &r()), Err(KmsError::KekUnavailable(_))),
            "a crypto-shredded per-tenant index DEK resolves to a LOUD error, never a plaintext"
        );
    }

    /// **Destroying the tenant KEK shreds every Search subject backstop under it** (tenant offboard
    /// = one operation crypto-shreds the whole tenant index + all its subjects' source backstops).
    #[test]
    fn tenant_decommission_shreds_every_subject_backstop() {
        let pin = SearchDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant index dek");
        let s1 = pin.reserve_subject_source_backstop(&t(), &r(), "u-1").expect("s1");
        let s2 = pin.reserve_subject_source_backstop(&t(), &r(), "u-2").expect("s2");

        assert!(pin.destroy_tenant_index_dek(&t(), &r()), "tenant KEK destroyed");

        for kr in [&tk, &s1, &s2] {
            assert!(
                pin.resolve(kr, &r()).is_err(),
                "every Search DEK under the destroyed tenant KEK is unrecoverable"
            );
        }
    }

    /// **Per-subject source crypto-shred leaves the tenant index + other subjects intact (the GD-4
    /// individual lever; destroy callable on the per-subject backstop).** One person's source
    /// ciphertext is erased; the tenant index DEK and every other subject keep resolving.
    #[test]
    fn destroy_subject_backstop_is_individual_grained() {
        let pin = SearchDekPin::new(kms());
        let tk = pin.reserve(&t(), &r()).expect("tenant index dek");
        let s1 = pin.reserve_subject_source_backstop(&t(), &r(), "u-1").expect("s1");
        let s2 = pin.reserve_subject_source_backstop(&t(), &r(), "u-2").expect("s2");

        assert!(pin.destroy_subject_backstop(&t(), "u-1"), "subject backstop present to destroy");
        assert!(!pin.destroy_subject_backstop(&t(), "u-1"), "a second destroy finds nothing");

        assert!(pin.resolve(&s1, &r()).is_err(), "u-1's source backstop key is shredded");
        assert!(pin.resolve(&tk, &r()).is_ok(), "the tenant index DEK is untouched");
        assert!(pin.resolve(&s2, &r()).is_ok(), "u-2's backstop is untouched");
    }

    /// **A crypto-shredded Search tenant is excluded from the KMS backup snapshot** (§7.5 / STOR-D3
    /// by reference) — the per-tenant index DEK stays dead across a restore (the backup-backstop
    /// half of the floor unit). The live tenant's wrapped DEK is backed up; the shredded one is not.
    #[test]
    fn shredded_search_tenant_is_excluded_from_backup() {
        let kms = kms();
        let pin = SearchDekPin::new(Arc::clone(&kms));
        let live = TenantId("live-co".into());
        let dead = TenantId("offboarded-co".into());
        pin.reserve(&live, &r()).expect("live");
        pin.reserve(&dead, &r()).expect("dead");

        assert!(pin.destroy_tenant_index_dek(&dead, &r()), "offboard the dead tenant");

        let snap = kms.backup_snapshot();
        assert!(snap.iter().any(|(d, _)| d.tenant == live), "live tenant index DEK backed up");
        assert!(
            !snap.iter().any(|(d, _)| d.tenant == dead),
            "a crypto-shredded Search tenant is EXCLUDED from backup (stays dead across restore)"
        );
    }

    /// **The Search key classes share the cell's ONE engine — no second KMS (11.3 one-hierarchy).**
    /// The pin holds the SAME `Arc<KmsEngine>` it was built over; a DEK reserved through the pin is
    /// visible to the cell engine directly (one cell root governs one hierarchy).
    #[test]
    fn search_uses_the_one_cell_engine_not_a_second_kms() {
        let kms = kms();
        let pin = SearchDekPin::new(Arc::clone(&kms));
        let key_ref = pin.reserve(&t(), &r()).expect("reserve through the pin");
        assert!(
            kms.resolve_dek(&key_ref, &r()).is_ok(),
            "the shared cell engine resolves the DEK the Search pin reserved (one hierarchy)"
        );
        assert!(Arc::ptr_eq(pin.engine(), &kms), "the pin holds the very same cell engine");
    }

    /// **The HYOK structural skip holds by reference (§4.8): a HYOK class is NOT indexed.** A
    /// platform-managed / BYOK class is admitted (a plaintext index may be built); a HYOK class is
    /// skipped (no plaintext to embed → no index DEK reserved → the no-leak property holds by
    /// construction). This restates the frozen `IndexAdmission` verdict; the LIVE indexer consult is
    /// SRCH-P06/P15.
    #[test]
    fn hyok_class_is_structurally_skipped_no_index_no_dek() {
        // A minimal HYOK key service that DENIES every unwrap (the customer key is out of Myelin's
        // reach) — the worst case; `can_derive_plaintext_index()` is `false` regardless of the
        // service's answer, so the class is skipped by construction.
        struct DenyAllHyok;
        impl HyokKeyService for DenyAllHyok {
            fn wrap(&self, _dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
                Err(HyokServiceDenied)
            }
            fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
                Err(HyokServiceDenied)
            }
            fn destroy(&self) {}
        }

        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(t(), r()));
        let platform = PlatformManaged::new(&engine, r());
        let byok = Byok::new(&engine, r(), "kms-customer://acme/k1");
        let hyok = Hyok::new(DenyAllHyok);

        assert!(!hyok_skips_index(&platform), "platform-managed class IS indexed (full search)");
        assert!(!hyok_skips_index(&byok), "BYOK class IS indexed (plaintext reachable while live)");
        assert!(hyok_skips_index(&hyok), "a HYOK class is structurally SKIPPED — no plaintext index");
    }

    /// **The SRCH-P03 inherited-gate precondition list is complete + names the load-bearing gates
    /// (the GATE/DRILLS requirement in code).** Search reads these; it never re-runs them. The list
    /// must name the restore-verify / cross-tenant / fail-static / disabled-user / misroute /
    /// residency gates so the SRCH-P03 agent has the precondition machine-readably.
    #[test]
    fn srch_p03_inherited_gates_name_every_precondition() {
        let gates = srch_p03_inherited_gates();
        let ids: Vec<&str> = gates.iter().map(|g| g.id).collect();
        for required in ["STOR-D1", "STOR-D2", "ID-D3", "ID-D2", "ID-D1", "CP-D2", "CP-D3"] {
            assert!(ids.contains(&required), "the SRCH-P03 precondition list names {required}");
        }
        for g in &gates {
            assert!(!g.guarantees.is_empty(), "gate {} states what it guarantees", g.id);
        }
    }
}
