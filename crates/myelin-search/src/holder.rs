//! Search as a `PersonalDataHolder` (H7 `SearchIndex`) — the STUB surface + the harness
//! auto-registration (SRCH-P02 / P-122; contract 10.1 + 1.4).
//!
//! **Architecture:** search-and-indexing.md §1 ("Search is a true holder whose `erase` is a real
//! purge"), §3.4 (the per-tenant residency-pinned index tier — ONE index store), §4.8 (the holder
//! surface `locate/export/rectify/restrict/erase`; the crypto-shred layering: the per-tenant index
//! DEK is the tenant-decommission shred unit + the backup/immutable-segment backstop; the
//! **PRIMARY** per-subject erasure is **purge + reindex**, landing in SRCH-P15). The exhaustive
//! H1–H18 catalog ([`myelin_substrate::Holder`]) names Search **H7 (`SearchIndex`)**.
//!
//! ## The ONE Search store (the holder's surface)
//! Search owns exactly ONE store — the **per-tenant index** (§3.4: full-text-inverted +
//! structured/columnar + vector HNSW, all in one `(tenant, region)`-keyed, doc-id space). Its
//! [`myelin_substrate::StoreKind::SearchIndex`] class classifies **structurally** to
//! [`myelin_substrate::Holder::H7SearchIndex`] (gdpr §3.2: ONE platform-wide search-index holder —
//! no per-store `StoreClassifier` declaration is needed, exactly like blob→H6 / cache→H9). Search
//! holds **only derived, reconstructible** state (architecture §0/§1); the export source of truth
//! is always the owning subsystem, never the index.
//!
//! ## Why a STUB surface now (the named floor)
//! No index exists yet at M1 — the encrypted-from-birth per-tenant index layout is **SRCH-P03**
//! (M2), the IndexBackend/Tantivy/vector shapes SRCH-P04/P05, the incremental indexer SRCH-P06. So
//! the holder is **registered + classified + callable**, but its bodies return
//! **empty-but-correct** results (a tenant with no index has no located docs/vectors) and `erase`
//! is a well-defined no-op (nothing to purge) returning a content-addressed receipt. The REAL
//! body — `locate` → docs/fields/vectors referencing the subject; `erase` → **purge + reindex**
//! (delete the docs, tombstone+compact the vectors, reindex from the source's now-tombstoned
//! projection, §4.8); `restrict` → suppress indexing/RAG/notification — lands in **SRCH-P15** (M2),
//! and the per-tenant index DEK that crypto-shreds the whole tenant index on decommission +
//! backstops backups is reserved by [`crate::dek`] in THIS prompt. The point of registering NOW:
//! the M5 DSAR fan-out cannot silently miss Search (10.1 exhaustiveness).

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, Result as DsrResult, RestrictReceipt, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreKind, classify_store};

/// The stable, PII-free name of the Search **per-tenant index** store (the holder's H7 store).
/// Frozen here so the SRCH-P03 index layout, the data-map (P-GA-09), and the DSR fan-out all
/// address exactly this store. PII-free: a store identifier, never personal data.
pub const SEARCH_INDEX_STORE: &str = "search_index";

/// The typed receipt that the Search store was auto-registered as a [`PersonalDataHolder`] — the
/// proof the registration fired (mirrors `myelin_substrate::HolderRegistration`). The harness
/// collects these; the holder-registered architecture test reads them to assert the Search index
/// did not escape registration. PII-free: a (kind, name) tag.
pub type SearchHolderRegistration = HolderRegistration;

/// **Register Search's (future) index store as a `PersonalDataHolder` through the harness
/// auto-registration (contract 1.4).** Opens the per-tenant index store through the substrate
/// [`HolderRegistry`] — the ONE door — so it is a registered holder by construction. Returns the
/// registry (carrying the receipt) so a caller / test can assert exactly which store registered +
/// that it classifies to its H-holder (H7 search index).
///
/// At M1 this is the REGISTRATION only — `serve` will open the real index store (re-running this
/// exact classification) when the encrypted-from-birth layout lands (SRCH-P03+); registering now
/// makes "the DSAR fan-out forgot Search" structurally impossible (10.1 exhaustiveness). The
/// search index classifies STRUCTURALLY (kind=SearchIndex → H7), so no `StoreClassifier`
/// declaration is required (gdpr §3.2 — one platform-wide search-index holder).
pub fn register_search_holder() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    // The per-tenant index store (H7) — classifies structurally by its SearchIndex kind.
    registry.open(StoreKind::SearchIndex, SEARCH_INDEX_STORE);
    registry
}

/// Search's **per-tenant index** AS a [`PersonalDataHolder`] (H7; contract 10.1). At M1 a STUB:
/// no index exists, so `locate`/`export` return **empty-but-correct** receipts (a tenant with no
/// index has no located docs/vectors), `restrict`/`rectify` are well-defined no-ops, and `erase`
/// is a no-op (nothing to purge) — each returning a content-addressed receipt. The REAL bodies
/// (purge + reindex; vectors compacted; restrict suppression) land in SRCH-P15 (§4.8).
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchIndexHolder;

impl SearchIndexHolder {
    /// Register this holder through the substrate registry (the `serve`-called auto-registration
    /// seam), returning the receipt — the proof the index store registered as holder H7.
    pub fn register(&self, registry: &mut HolderRegistry) -> SearchHolderRegistration {
        registry.open(StoreKind::SearchIndex, SEARCH_INDEX_STORE)
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) —
    /// never a name/email. This is the `<pseudonym>@<tenant>.noreply` posture (§4.8 / EI-04 §1).
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for SearchIndexHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        // EMPTY-BUT-CORRECT: no index exists, so the subject has no located Search data. The
        // receipt attests the locate completed over an empty surface (NOT an error — the holder is
        // a real, callable stub). The real doc/field/vector walk lands in SRCH-P15.
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "no-search-data (SRCH-P02 stub: index lands SRCH-P03; locate body SRCH-P15)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        // EMPTY-BUT-CORRECT: an empty portable bundle. The index is DERIVED + reconstructible
        // (architecture §0/§1) — it is NEVER the export source of truth (the owning subsystem is);
        // an export over the index is empty by design, here doubly so (no index exists yet).
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "empty-bundle (SRCH-P02 stub: index derived/reconstructible — never the export source)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // The index is derived; rectification is via reindex-from-source over the corrected
        // projection (§4.9), a no-op now (no index). The real rectify-by-reindex lands in SRCH-P15.
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (SRCH-P02 stub: index derived; rectify via reindex-from-source SRCH-P15)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // Restriction (Art. 18/21) suppresses indexing/RAG/analytics/notification for the subject
        // pending erasure (§4.8 — the suppression the X-7 posture relies on). No index exists yet —
        // a well-defined no-op now; the real suppression into the index lands in GA-D7 (P-152) /
        // SRCH-P15 once the index exists.
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                "",
                &format!("no-op on={on} (SRCH-P02 stub: no index yet; suppression SRCH-P15 / GA-D7 P-152)"),
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // No-op purge: no index exists, so there is nothing to purge or compact. The real
        // structural erasure — PURGE + REINDEX (delete docs/fields, tombstone+compact vectors,
        // reindex the surviving artifact from the source's now-tombstoned projection, §4.8) — is
        // the PRIMARY per-subject erasure and lands in SRCH-P15. The per-tenant index DEK
        // (crate::dek) crypto-shreds the whole tenant index on decommission + backstops backups;
        // it is reserved in THIS prompt but is NOT the whole erasure answer (named floor).
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                SEARCH_INDEX_STORE,
                &subject_id,
                &tenant,
                "no-op (SRCH-P02 stub: no index to purge; PRIMARY purge+reindex SRCH-P15; index DEK reserved here)",
                None,
                0,
            ),
        })
    }
}

/// The H-holder the Search index store classifies to (H7 `SearchIndex`) — a convenience over
/// [`classify_store`] for the structural classification of the ONE Search store. Returns the
/// holder (always `Some(H7SearchIndex)` for the SearchIndex kind, gdpr §3.2) so a caller can pin
/// the classification without rebuilding an empty `StoreClassifier`.
pub fn search_index_holder() -> Option<Holder> {
    // SearchIndex classifies structurally — no per-store declaration needed (gdpr §3.2).
    classify_store(StoreKind::SearchIndex, SEARCH_INDEX_STORE, &Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::assert_holder_completeness;

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

    /// **Search registers its index store as a holder through the one door (contract 1.4).** The
    /// per-tenant index is opened through the substrate registry, so it is a registered holder by
    /// construction — 0 stores escape registration.
    #[test]
    fn search_registers_its_index_store_as_a_holder() {
        let registry = register_search_holder();
        assert!(registry.is_registered(StoreKind::SearchIndex, SEARCH_INDEX_STORE));
        assert_eq!(registry.len(), 1, "exactly the one Search index store registered");
    }

    /// **Re-registration is idempotent** — `serve` re-running the registration on a restart records
    /// the Search store exactly once (the registry is idempotent on (kind, name)).
    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_search_holder();
        SearchIndexHolder.register(&mut registry);
        assert_eq!(registry.len(), 1, "re-opening the same Search store does not double-register");
    }

    /// **The Search store classifies to H7 — 0 orphans (contract 1.4 + gdpr §3.2).** The index
    /// store maps structurally to **H7 (`SearchIndex`)** (kind=SearchIndex). The substrate
    /// completeness assertion is GREEN — the Search store is inside the exhaustive H1–H18 list, so
    /// the M5 DSAR fan-out cannot miss Search.
    #[test]
    fn search_store_classifies_to_h7_no_orphan() {
        let registry = register_search_holder();
        assert_eq!(
            search_index_holder(),
            Some(Holder::H7SearchIndex),
            "the per-tenant index is holder H7"
        );
        // An empty classifier suffices — the search index classifies structurally, never via a
        // per-store OLTP declaration (gdpr §3.2). The completeness assertion is GREEN.
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &Default::default()),
            Ok(()),
            "the Search index store is in the exhaustive H1–H18 list — 0 orphan stores"
        );
    }

    /// **The holder stub returns empty-but-correct `locate`/`export` for a tenant with no index
    /// (the SRCH-P02 TESTS requirement).** No index exists, so the subject has no located Search
    /// data — the holder responds with a content-addressed receipt over an EMPTY surface (NOT an
    /// error; it is a real, callable stub). The bodies are deterministic + PII-free.
    #[test]
    fn holder_stub_returns_empty_but_correct_locate_and_export() {
        let holder = SearchIndexHolder;
        let subj = subject("u-1");
        let locate = holder.locate(&subj, tenant()).expect("locate over empty surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(locate.receipt.key_epoch_destroyed.is_none(), "locate shreds no key");

        let export = holder.export(&subj, tenant()).expect("export of empty bundle succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    /// **`erase` is a well-defined no-op now (nothing to purge) returning a receipt — never a
    /// panic.** The stub names its SRCH-P15 follow-on (purge+reindex, the PRIMARY erasure) in the
    /// outcome (a named floor, not a hidden gap). Idempotent: the same scope yields the same
    /// content-addressed receipt.
    #[test]
    fn holder_stub_erase_is_a_no_op_receipt_and_idempotent() {
        let holder = SearchIndexHolder;
        let scope = EraseScope::Subject { subject: subject("u-1"), tenant: tenant() };
        let r1 = holder.erase(scope.clone()).expect("stub erase succeeds (no-op)");
        let r2 = holder.erase(scope).expect("stub erase is idempotent");
        assert_eq!(r1, r2, "the same erase scope yields the identical content-addressed receipt");
        assert!(r1.receipt.key_epoch_destroyed.is_none(), "no key shredded (no index exists)");
    }

    /// **`restrict` is a well-defined no-op now** (no index to suppress) — the suppression the X-7
    /// posture relies on (§4.8) lands in SRCH-P15 / GA-D7. Both `on` and `off` succeed.
    #[test]
    fn holder_stub_restrict_surface() {
        let holder = SearchIndexHolder;
        let subj = subject("u-2");
        assert!(holder.restrict(&subj, true).is_ok(), "restrict on succeeds (no-op stub)");
        assert!(holder.restrict(&subj, false).is_ok(), "restrict off succeeds (no-op stub)");
    }

    /// **The holder is object-safe** — held behind `dyn PersonalDataHolder` exactly as the DSR
    /// orchestrator / holder registry need (a heterogeneous holder set, contract 10.1).
    #[test]
    fn holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(SearchIndexHolder)];
        let subj = subject("u-3");
        for h in &holders {
            assert!(h.locate(&subj, tenant()).is_ok(), "the holder responds to the contract");
        }
    }
}
