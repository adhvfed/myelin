//! Refs as a `PersonalDataHolder` (H12) — the STUB surface + the harness auto-registration
//! (REF-P3 / P-120; contract 10.1 + 1.4).
//!
//! **Architecture:** reference-graph.md §3 (every Refs store is a `PersonalDataHolder`
//! auto-registered by the harness — substrate §3.4 / contract 1.4), §3.6 (the projection cache is
//! itself a bounded invalidatable holder), §4.6 (the small structural erasure surface). The
//! exhaustive H1–H18 catalog ([`myelin_substrate::Holder`]) names Refs **H12
//! (`ReferenceGraph`)**.
//!
//! ## The two Refs stores (the holder's surface)
//! Refs owns exactly two stores, both **future** at M1 (no migration ships here):
//! 1. the **edge inverse-index** — an OLTP table (REF-P5 schema); its H-holder is **H12** (declared
//!    here through the substrate [`myelin_substrate::StoreClassifier`], the data-map's "this OLTP
//!    store is holder H12" fact);
//! 2. the **R2 projection cache** — a Valkey-class cache namespace (REF-P11/REF-P12); a cache
//!    classifies **structurally** to [`myelin_substrate::Holder::H9Caches`] (gdpr §3.2: ONE
//!    platform-wide caches holder), and §3.6 additionally treats it as a Refs-owned invalidatable
//!    holder — both are true (the cache is platform-cache-class for the catalog AND Refs invalidates
//!    it on `*.erased`).
//!
//! ## Why a STUB surface now (the named floor)
//! No edge index exists yet (REF-P5 is M2), so there is nothing to locate/export/purge. The holder
//! is therefore **registered + classified + callable** — but its bodies return **empty-but-correct**
//! results (a tenant with no edges has no located data) and `erase` is a well-defined no-op (nothing
//! to shred) returning a content-addressed receipt. The REAL bodies — `locate` → edges/cache entries
//! naming the subject; `erase` → purge R2-cache PII + rely on Identity's pseudonym shred for
//! `origin_actor` + `*.erased` tombstoning (§4.6) — land in **REF-P15** (M2), and the per-tenant DEK
//! that makes the cache crypto-shred-able lands in **REF-P4** (the next prompt). The point of
//! registering NOW: the M5 DSAR fan-out cannot silently miss Refs (10.1 exhaustiveness).

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, Result as DsrResult, RestrictReceipt, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};

/// The stable, PII-free name of the Refs **edge inverse-index** OLTP store (the holder's H12
/// store). Frozen here so the REF-P5 migration, the data-map (P-GA-09), and the DSR fan-out all
/// address exactly this store. PII-free: a store identifier, never personal data.
pub const REFS_EDGE_STORE: &str = "refs_edge_index";

/// The stable, PII-free name of the Refs **R2 projection cache** namespace (the §3.6 invalidatable
/// holder). Classifies structurally to H9 (caches); §3.6 also has Refs invalidate it on `*.erased`.
pub const REFS_CACHE_STORE: &str = "refs_projection_cache";

/// The typed receipt that a Refs store was auto-registered as a [`PersonalDataHolder`] — the proof
/// the registration fired for a given store (mirrors `myelin_substrate::HolderRegistration`, the
/// substrate-side receipt). The harness collects these; the holder-registered architecture test
/// reads them to assert no Refs store escaped registration. PII-free: a (kind, name) tag.
pub type RefsHolderRegistration = HolderRegistration;

/// Build the Refs [`myelin_substrate::StoreClassifier`] — the data-map declaration that the Refs
/// edge OLTP store belongs to holder **H12 (`ReferenceGraph`)**. The R2 cache classifies
/// structurally to H9 (a cache), so it needs no per-store declaration here. The substrate
/// completeness assertion joins the harness's [`HolderRegistry`] against this classifier: every
/// opened Refs store must map to an H-holder, or it is an orphan (contract 1.4 + gdpr §3.2).
pub fn refs_store_classifier() -> StoreClassifier {
    StoreClassifier::of([myelin_substrate::StoreHolder::new(
        StoreKind::Oltp,
        REFS_EDGE_STORE,
        Holder::H12ReferenceGraph,
    )])
}

/// **Register Refs' (future) stores as `PersonalDataHolder`s through the harness auto-registration
/// (contract 1.4).** Opens both Refs stores through the substrate [`HolderRegistry`] — the ONE door
/// — so each is a registered holder by construction. Returns the registry (carrying the two
/// receipts) so a caller / test can assert exactly which stores registered + that they classify to
/// their H-holders (H12 edge index, H9 cache).
///
/// At M1 this is the REGISTRATION only — `serve` will open the real stores (re-running this exact
/// classification) when the edge schema lands (REF-P5+); registering now makes "the DSAR fan-out
/// forgot Refs" structurally impossible (10.1 exhaustiveness).
pub fn register_refs_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    // The edge inverse-index OLTP store (H12) — declared in the classifier above.
    registry.open(StoreKind::Oltp, REFS_EDGE_STORE);
    // The R2 projection cache namespace (§3.6) — a cache, structurally H9.
    registry.open(StoreKind::Cache, REFS_CACHE_STORE);
    registry
}

/// The DSR-body floor marker note (PII-free) — names where the real body lands so the stub is never
/// mistaken for the whole erasure answer (VISION §3 name-your-floors). The stub bodies are
/// **empty-but-correct**, never panicking, so the registration + fan-out path is exercisable now.
fn floor_note(op: &str) -> String {
    format!(
        "Refs {op} is the REF-P3 STUB (no edge index exists yet; REF-P5 ships the schema). The real \
         body — locate over edges/cache + erase = purge R2-cache PII + Identity pseudonym shred for \
         origin_actor + *.erased tombstoning (§4.6) — lands in REF-P15; the per-tenant DEK in REF-P4"
    )
}

/// Refs' **edge inverse-index** AS a [`PersonalDataHolder`] (H12; contract 10.1). At M1 a STUB:
/// no edge index exists, so `locate`/`export` return **empty-but-correct** receipts (a tenant with
/// no edges has no located data), `restrict`/`rectify` are well-defined no-ops, and `erase` is a
/// no-op (nothing to shred) — each returning a content-addressed receipt. The REAL bodies land in
/// REF-P15 (§4.6).
#[derive(Clone, Copy, Debug, Default)]
pub struct RefsEdgeHolder;

impl RefsEdgeHolder {
    /// Register this holder through the substrate registry (the `serve`-called auto-registration
    /// seam), returning the receipt — the proof the edge store registered as holder H12.
    pub fn register(&self, registry: &mut HolderRegistry) -> RefsHolderRegistration {
        registry.open(StoreKind::Oltp, REFS_EDGE_STORE)
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) —
    /// never a name/email. This is the `origin_actor` pseudonym posture (§4.6 / EI-04 §1).
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for RefsEdgeHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        // EMPTY-BUT-CORRECT: no edge index exists, so the subject has no located Refs data. The
        // receipt attests the locate completed over an empty surface (NOT an error — the holder is
        // a real, callable stub). The real edge/cache walk lands in REF-P15.
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                REFS_EDGE_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "no-refs-data (REF-P3 stub: edge index lands REF-P5; body REF-P15)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        // EMPTY-BUT-CORRECT: an empty portable bundle (no edges name the subject yet).
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                REFS_EDGE_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "empty-bundle (REF-P3 stub: edge index lands REF-P5; body REF-P15)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // Refs holds no free-text bodies (references-not-payloads); rectification is a no-op now
        // (rectify via reindex-from-source over edges lands in REF-P15 / GA-D2 P-151).
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                REFS_EDGE_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (REF-P3 stub; references-not-payloads — nothing to rectify; body REF-P15)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // Restriction (Art. 18/21) suppresses Refs-derived projections for the subject. No
        // projections exist yet — a well-defined no-op now; the real suppression into the derived
        // stores lands in GA-D7 (P-152) once the cache exists.
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                REFS_EDGE_STORE,
                &Self::subject_id(subject),
                "",
                &format!("no-op on={on} (REF-P3 stub: no projections yet; suppression GA-D7 P-152)"),
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // No-op crypto-shred: no edge index / R2 cache exists, so there is nothing to purge or
        // shred. The real structural erasure — purge R2-cache PII + rely on Identity's pseudonym
        // shred for origin_actor (the edge keeps the opaque id; the human becomes unresolvable) +
        // *.erased tombstoning (§4.6) — lands in REF-P15; the per-tenant DEK in REF-P4.
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let _ = floor_note("erase"); // the floor is named in the receipt outcome below.
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                REFS_EDGE_STORE,
                &subject_id,
                &tenant,
                "no-op (REF-P3 stub: no edge index/R2 cache to shred; structural erase REF-P15; DEK REF-P4)",
                None,
                0,
            ),
        })
    }
}

/// Refs' **R2 projection cache** AS a [`PersonalDataHolder`] (§3.6 — a bounded, invalidatable
/// holder). A cache classifies structurally to [`Holder::H9Caches`] for the platform-wide catalog,
/// and §3.6 has Refs invalidate it on `*.erased`. At M1 a STUB (no cache exists; REF-P12 builds it):
/// the bodies are empty-but-correct / no-ops, mirroring [`RefsEdgeHolder`]. The cache's PII is only
/// derived projection titles + the pseudonymous `origin_actor` — purged on erase in REF-P15.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefsCacheHolder;

impl RefsCacheHolder {
    /// Register the cache through the substrate registry (a cache namespace, §3.4), returning the
    /// receipt — the proof the R2 cache registered as a holder (§3.6).
    pub fn register(&self, registry: &mut HolderRegistry) -> RefsHolderRegistration {
        registry.open(StoreKind::Cache, REFS_CACHE_STORE)
    }
}

impl PersonalDataHolder for RefsCacheHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                &tenant.0,
                "no-cache-data (REF-P3 stub: R2 cache lands REF-P12; purge body REF-P15)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                &tenant.0,
                "empty-bundle (REF-P3 stub: R2 cache derived, reconstructible — never the export source)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                "",
                "no-op (REF-P3 stub: cache is derived; rectify via reindex-from-source REF-P15)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                "",
                &format!("no-op on={on} (REF-P3 stub: no cache entries yet; suppression GA-D7 P-152)"),
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // The cache erase IS a purge (derived PII), made crypto-shred-able by the per-tenant DEK
        // (REF-P4). No-op now (no cache exists); real purge in REF-P15.
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                REFS_CACHE_STORE,
                &subject_id,
                &tenant,
                "no-op (REF-P3 stub: no R2 cache to purge; purge body REF-P15; DEK REF-P4)",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::{assert_holder_completeness, classify_store};

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    /// **Refs registers its two stores as holders through the one door (contract 1.4).** Both the
    /// edge OLTP index and the R2 cache are opened through the substrate registry, so each is a
    /// registered holder by construction — 0 stores escape registration.
    #[test]
    fn refs_registers_both_stores_as_holders() {
        let registry = register_refs_holders();
        assert!(registry.is_registered(StoreKind::Oltp, REFS_EDGE_STORE));
        assert!(registry.is_registered(StoreKind::Cache, REFS_CACHE_STORE));
        assert_eq!(registry.len(), 2, "exactly the two Refs stores registered");
    }

    /// **Re-registration is idempotent** — `serve` re-running the registration on a restart records
    /// each Refs store exactly once (the registry is idempotent on (kind, name)).
    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_refs_holders();
        RefsEdgeHolder.register(&mut registry);
        RefsCacheHolder.register(&mut registry);
        assert_eq!(registry.len(), 2, "re-opening the same Refs stores does not double-register");
    }

    /// **The Refs stores classify to their H-holders — 0 orphans (contract 1.4 + gdpr §3.2).** The
    /// edge OLTP store maps to **H12 (`ReferenceGraph`)** via the Refs classifier; the cache maps
    /// structurally to **H9 (`Caches`)**. The substrate completeness assertion is GREEN — no Refs
    /// store falls outside the exhaustive H1–H18 list, so the M5 DSAR fan-out cannot miss Refs.
    #[test]
    fn refs_stores_classify_to_h12_and_h9_no_orphan() {
        let registry = register_refs_holders();
        let classifier = refs_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, REFS_EDGE_STORE, &classifier),
            Some(Holder::H12ReferenceGraph),
            "the edge inverse-index is holder H12"
        );
        assert_eq!(
            classify_store(StoreKind::Cache, REFS_CACHE_STORE, &classifier),
            Some(Holder::H9Caches),
            "the R2 projection cache classifies structurally to H9"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every Refs store is in the exhaustive H1–H18 list — 0 orphan stores"
        );
    }

    /// **The holder stub returns empty-but-correct `locate`/`export` for a tenant with no edges
    /// (the REF-P3 TESTS requirement).** No edge index exists, so the subject has no located Refs
    /// data — the holder responds with a content-addressed receipt over an EMPTY surface (NOT an
    /// error; it is a real, callable stub). The bodies are deterministic + PII-free.
    #[test]
    fn holder_stub_returns_empty_but_correct_locate_and_export() {
        let holder = RefsEdgeHolder;
        let subj = subject("u-1");
        let locate = holder.locate(&subj, tenant()).expect("locate over empty surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(locate.receipt.key_epoch_destroyed.is_none(), "locate shreds no key");

        let export = holder.export(&subj, tenant()).expect("export of empty bundle succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    /// **`erase` is a well-defined no-op now (nothing to shred) returning a receipt — never a
    /// panic.** The stub names its REF-P15 / REF-P4 follow-on in the outcome (a named floor, not a
    /// hidden gap). Idempotent: the same scope yields the same content-addressed receipt.
    #[test]
    fn holder_stub_erase_is_a_no_op_receipt_and_idempotent() {
        let holder = RefsEdgeHolder;
        let scope = EraseScope::Subject { subject: subject("u-1"), tenant: tenant() };
        let r1 = holder.erase(scope.clone()).expect("stub erase succeeds (no-op)");
        let r2 = holder.erase(scope).expect("stub erase is idempotent");
        assert_eq!(r1, r2, "the same erase scope yields the identical content-addressed receipt");
        assert!(r1.receipt.key_epoch_destroyed.is_none(), "no key shredded (no index exists)");
    }

    /// **The cache holder mirrors the stub surface (§3.6).** A cache `erase` is a purge (derived
    /// PII), a no-op now; the body is named REF-P15 / REF-P4.
    #[test]
    fn cache_holder_stub_surface() {
        let holder = RefsCacheHolder;
        let subj = subject("u-2");
        assert!(holder.locate(&subj, tenant()).is_ok());
        let r = holder
            .erase(EraseScope::Tenant(tenant()))
            .expect("cache stub erase succeeds");
        assert_eq!(r.receipt.operation, "erase");
    }

    /// **The holders are object-safe** — held behind `dyn PersonalDataHolder` exactly as the DSR
    /// orchestrator / holder registry need (a heterogeneous holder set, contract 10.1).
    #[test]
    fn holders_are_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> =
            vec![Box::new(RefsEdgeHolder), Box::new(RefsCacheHolder)];
        let subj = subject("u-3");
        for h in &holders {
            assert!(h.locate(&subj, tenant()).is_ok(), "each holder responds to the contract");
        }
    }
}
