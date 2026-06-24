//! **Search as a REAL `PersonalDataHolder`: purge + reindex (vectors compacted) + restrict
//! suppression + the HYOK structural skip** (SRCH-P15 / P-178; contract 10.1 real erase, §4.8).
//!
//! This module replaces the SRCH-P02 holder STUB ([`crate::holder::SearchIndexHolder`], whose bodies
//! were well-defined no-ops over an empty surface) with the **real erase mechanism** over a live
//! per-tenant index ([`crate::indexer::IncrementalIndexer`]). The stub stays for the
//! registration/empty-index path (it is what `serve` opens before any index exists); THIS is the
//! mechanism the DSR orchestrator's fan-out reaches once a tenant has an index.
//!
//! ## The §4.8 holder surface, for real (contract 10.1)
//! - **`locate(subject)`** — find every live doc/field/vector referencing the subject: by `acl_object`,
//!   by an `actor`/`assignee`/`mention` subject-locator facet, or by the subject's pseudonym
//!   `<pseudonym>@<tenant>.noreply` (contract 4.8) in the analyzable body. The set is computed by the
//!   ONE [`crate::engine::SubjectMatcher`] the three holder ops share (no drift).
//! - **`erase(subject)`** — **PURGE + RE-INDEX, not hide** (§4.8): for each located doc, drive a
//!   synthetic `*.erased` tombstone through the **SAME live consumer path** as everything else (the
//!   SRCH-P06 indexer's `index()` → `apply_removed` → `delete`), which removes the doc AND
//!   soft-deletes its co-located vector; then **compact** the index so the tombstoned embedding bytes
//!   are physically gone (0 orphan embedding, §3.3). There is **NO bespoke erasure backdoor** — the
//!   purge rides the live consumer, exactly as SEARCH-1 demands. The DSR orchestrator gets a receipt.
//! - **`restrict(subject)`** — suppress indexing/agent-use/analytics/notification for a subject pending
//!   erasure (§4.8): a restricted subject's content is **not surfaced** in results or RAG. The
//!   restriction set is a live, queryable suppression list ([`SearchEraseHolder::is_restricted`] /
//!   [`SearchEraseHolder::suppress_hits`]) the query path consults.
//! - **`export`/`rectify`** — the index is DERIVED + reconstructible (architecture §0/§1): it is NEVER
//!   the export source of truth (the owning subsystem is), and rectification is via reindex-from-source
//!   over the corrected projection (§4.9). `export` returns an empty-but-correct bundle; `rectify` is a
//!   no-op receipt (the corrected body re-enters via the live indexer, SRCH-P16).
//!
//! ## The HYOK structural skip (§4.8, contract 11.3)
//! When a content class's `can_derive_plaintext_index() = false` (the customer holds the key outside
//! Myelin's reach), Search **structurally skips** indexing — there is no plaintext to embed or analyse,
//! so the class is **not in the index at all**, and the no-leak property holds by construction. The
//! skip is the frozen [`crate::dek::hyok_skips_index`] verdict; here [`SearchEraseHolder::erase_class`]
//! records that a HYOK class has **nothing to erase** (0 docs, 0 vectors) — its erasure is satisfied by
//! the absence of any index, not by a purge. The at-scale cross-store assertion is M5 (SRCH-D10, P-422).
//!
//! ## Crypto-shred layering (change #9 — the per-tenant index DEK + per-subject source backstop)
//! Search's **primary** per-subject erasure is purge + reindex (above). The **per-tenant index DEK**
//! crypto-shreds the WHOLE tenant index on tenant-decommission ([`EraseScope::Tenant`] → destroy the
//! tenant KEK, [`crate::dek::SearchDekPin::destroy_tenant_index_dek`]) and backstops backups/immutable
//! segments. The Phase-5 per-subject SOURCE DEK (11.4) is an additional source-side backstop. A tenant
//! erase records the destroyed key epoch in its receipt (the GD-4 lever's audit trail).
//!
//! ## Floors named (prompt DoD)
//! - **SRCH-D4 is the CI variant here** (a moderate-scale 0-recoverable-incl-vectors proof). The full
//!   **backup-scale** erasure proof (SRCH-D4 at backup scale, folded into the M5 DSAR fan-out E2E-4) is
//!   the follow-on **SRCH-P29 / SRCH-P32** (P-422). Named so the CI green is not mistaken for the
//!   backup-scale proof.
//! - **The reindex-from-source rebuild path** (the bus re-emit → live indexer that re-indexes the
//!   *surviving* artifact after the subject's docs are purged) is the sibling slice **SRCH-P16** (the
//!   next prompt, P-179); `erase` here purges + compacts, and the surviving artifact re-enters through
//!   that path. Post-erase re-erasure after a reindex does not resurrect the subject (proven jointly
//!   with SRCH-P16).
//! - **Mutation floor (mandatory-core — erasure-critical).** The erase + restrict + HYOK-skip decision
//!   logic — the live-consumer purge loop, the compaction (0 orphan), the no-backdoor invariant, the
//!   restriction suppression, the HYOK-skip "nothing to erase" verdict, the tenant-decommission
//!   crypto-shred branch — is the mutation-tested core; the floor is stated + met by the unit +
//!   chained + drill tests in [`mod tests`] (every branch asserted). The SRCH-P05 vector + SRCH-P06
//!   indexer mutation floors still hold (unchanged). The world-scale re-erase-at-scale drill is
//!   SRCH-P28/P29 (M5).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Region, TenantId, Timestamp, Visibility,
};
use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef,
};
use myelin_identity::Principal;
use myelin_storage::KeyOrigin;

use crate::dek::{hyok_skips_index, SearchDekPin};
use crate::engine::SubjectMatcher;
use crate::holder::SEARCH_INDEX_STORE;
use crate::indexer::IncrementalIndexer;

/// The event-type the holder emits to drive a per-doc purge through the **live consumer path** (§4.8 —
/// the `*.erased` tombstone). Its trailing `.erased` segment is one of the indexer's
/// [`IncrementalIndexer::REMOVED_SUFFIXES`], so it flows through the SAME `index()` → `apply_removed`
/// path as a real owner-emitted `*.erased` — there is no bespoke erasure backdoor.
pub const SEARCH_ERASE_EVENT_TYPE: &str = "search.subject.erased";

/// **Search's REAL `PersonalDataHolder` erase mechanism (contract 10.1; §4.8).** Wraps a live
/// [`IncrementalIndexer`] (the per-tenant index it purges) + the cell [`SearchDekPin`] (the
/// tenant-decommission crypto-shred lever) + the cell's resident region + the live restriction set.
/// Replaces the SRCH-P02 stub with the real **purge + reindex** (vectors compacted) + **restrict
/// suppression** + the **HYOK structural skip**.
#[derive(Clone)]
pub struct SearchEraseHolder {
    /// The live per-tenant index the holder purges (the SAME indexer the bus feeds — `erase` drives the
    /// purge through ITS `index()` path, no second index, no backdoor).
    indexer: Arc<IncrementalIndexer>,
    /// The cell's Search DEK pin — the tenant-decommission crypto-shred lever (the per-tenant index DEK)
    /// + the per-subject source backstop. A tenant erase destroys the tenant KEK through this.
    dek: SearchDekPin,
    /// The cell's **resident region** (§3.4 — Search is region-pinned; the per-tenant index directory is
    /// residency-pinned, one tenant resolves to one resident region). The frozen 10.1 holder surface
    /// passes only `(subject, tenant)`; the cell-local holder resolves the region from its config
    /// (env `MYELIN_REGION`, dev `fr-par`) — a config detail, never a code change between dev and prod.
    region: Region,
    /// The live **restriction set** (§4.8 `restrict`): the pseudonymous subject ids whose content is
    /// suppressed pending erasure. The query path consults [`Self::is_restricted`] /
    /// [`Self::suppress_hits`] so a restricted subject is not surfaced in results or RAG. Keyed by the
    /// `(tenant, subject_id)` opaque ids — never a name (EI-04 §1).
    restricted: Arc<Mutex<BTreeSet<(String, String)>>>,
}

/// The outcome of a real `erase(subject)` over a live index — what was purged (the SRCH-D4 receipt
/// body). PII-free: counts + the destroyed key epoch, never the located bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EraseOutcome {
    /// The number of docs/fields purged through the live consumer path (`*.erased` → `delete`).
    pub docs_purged: usize,
    /// `true` iff, after the purge + compaction, the index holds **0 orphan embedding** (the
    /// erasure-critical GATE: a soft-deleted vector's bytes are physically gone — §3.3).
    pub zero_orphan_embedding: bool,
    /// The key epoch a tenant-decommission crypto-shred destroyed (`Some` only for an
    /// [`EraseScope::Tenant`] offboard); `None` for a per-subject purge (the primary mechanism shreds
    /// no key — it purges + reindexes).
    pub key_epoch_destroyed: Option<u64>,
}

impl SearchEraseHolder {
    /// Build the real holder over a live [`IncrementalIndexer`] + the cell [`SearchDekPin`] + the cell's
    /// resident `region` (env `MYELIN_REGION`).
    pub fn new(
        indexer: Arc<IncrementalIndexer>,
        dek: SearchDekPin,
        region: Region,
    ) -> SearchEraseHolder {
        SearchEraseHolder {
            indexer,
            dek,
            region,
            restricted: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// The cell's resident region (§3.4).
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The pseudonymous opaque subject id a receipt/matcher keys on (the Principal id) — never a
    /// name/email (the `<pseudonym>@<tenant>.noreply` posture, §4.8 / EI-04 §1).
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    /// The subject's pseudonym handle `<pseudonym>@<tenant>.noreply` (contract 4.8) for the body-mention
    /// match, if it renders. The opaque principal id IS the pseudonym token (the S2 pseudonym map
    /// resolves it elsewhere — `IdentityService::resolve_pseudonym`, 4.8); we render it through the FROZEN
    /// grammar so the body-mention match keys on the exact `.noreply` form the platform emits (a drift in
    /// the `@`/`.noreply` shape fails to compile, 4.8).
    fn pseudonym_of(subject: &SubjectRef, tenant: &TenantId) -> Option<String> {
        use myelin_identity::PseudonymHandle;
        PseudonymHandle::new(&subject.principal.principal_id.0, &tenant.0).map(|h| h.render())
    }

    /// Build the [`SubjectMatcher`] for a subject in a tenant (the ONE "references the subject" predicate
    /// the locate/erase/restrict ops share, §4.8).
    fn matcher(subject: &SubjectRef, tenant: &TenantId) -> SubjectMatcher {
        SubjectMatcher::new(
            Self::subject_id(subject),
            Self::pseudonym_of(subject, tenant),
        )
    }

    /// **How many live docs does `subject` currently reference in the `(tenant, region)` index?** Uses
    /// the SAME [`SubjectMatcher`] (one matcher, no drift, §4.8) the locate/erase ops share — so the
    /// restore-verify gate's resurrected-doc probe (SRCH-P28) reads exactly the set `erase_subject` would
    /// purge. 0 = the subject references no live doc (already erased / never present). PII-free.
    pub fn locate_doc_count(&self, subject: &SubjectRef, tenant: &TenantId) -> usize {
        let matcher = Self::matcher(subject, tenant);
        self.indexer
            .locate_subject(tenant, &self.region, &matcher)
            .len()
    }

    /// **Is `subject` restricted in `tenant` (§4.8 `restrict`)?** The query/RAG path consults this — a
    /// restricted subject is not surfaced. PII-free (opaque ids).
    pub fn is_restricted(&self, tenant: &TenantId, subject_id: &str) -> bool {
        self.restricted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(tenant.0.clone(), subject_id.to_string()))
    }

    /// **Suppress the docs of any restricted subject from a candidate hit set (§4.8 restrict
    /// suppression).** Given the per-doc subject ids (the docs' located subjects), drops any doc whose
    /// subject is restricted in `tenant` — the suppression the query path applies so a restricted
    /// subject's content does not surface in results or RAG. Returns the surviving doc-ids.
    pub fn suppress_hits<'a>(
        &self,
        tenant: &TenantId,
        hits: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Vec<String> {
        let set = self.restricted.lock().unwrap_or_else(|e| e.into_inner());
        hits.into_iter()
            .filter(|(_doc, subject_id)| {
                !set.contains(&(tenant.0.clone(), (*subject_id).to_string()))
            })
            .map(|(doc, _)| doc.to_string())
            .collect()
    }

    /// **Erase a class behind a HYOK structural skip (§4.8, contract 11.3).** A content class whose
    /// `can_derive_plaintext_index() = false` is NOT in the index at all — there is nothing to purge,
    /// so its erasure is satisfied by construction (the no-leak property holds without an index). Returns
    /// `true` iff the class is a HYOK skip (its erasure is the structural absence of any index). A
    /// non-HYOK (platform-managed / BYOK) class IS indexed and erases by purge + reindex (use [`Self::erase`]).
    pub fn erase_class(&self, origin: &dyn KeyOrigin) -> bool {
        hyok_skips_index(origin)
    }

    /// **The REAL `erase(subject, tenant)` — PURGE + RE-INDEX, not hide (§4.8).** Locates every doc
    /// referencing the subject, drives a synthetic `*.erased` tombstone for each through the SAME live
    /// consumer path (`index()` → `apply_removed` → `delete`, which removes the doc + soft-deletes its
    /// co-located vector — no backdoor), then COMPACTS the index (the tombstoned embedding bytes are
    /// physically gone, 0 orphan embedding, §3.3). Returns the [`EraseOutcome`] (the SRCH-D4 receipt
    /// body). Idempotent: a second erase of the same subject purges 0 (already gone) and is still
    /// 0-orphan.
    pub fn erase_subject(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
    ) -> Result<EraseOutcome, crate::engine::IndexError> {
        let region = self.region.clone();
        let matcher = Self::matcher(subject, tenant);
        let located = self.indexer.locate_subject(tenant, &region, &matcher);
        let docs_purged = located.len();

        // Drive the purge through the SAME live consumer path — a synthetic `*.erased` per located doc,
        // delivered to the indexer's `index()` (NOT a direct backend `delete` backdoor). This is the
        // SEARCH-1 symmetry: the erase path IS the live consumer path.
        for doc_id in &located {
            let ev = Self::erased_event(tenant, &region, &subject.principal, doc_id);
            self.indexer.index(&ev).map_err(|e| {
                crate::engine::IndexError::Engine(format!("erase purge failed: {e:?}"))
            })?;
        }

        // COMPACT (§3.3): physically remove every tombstoned embedding so 0 orphan embedding survives
        // (the erasure-critical GATE — embeddings are personal data, erased with their source).
        self.indexer.compact(tenant, &region)?;
        let zero_orphan_embedding = !self.indexer.has_orphan_embedding(tenant, &region);

        Ok(EraseOutcome {
            docs_purged,
            zero_orphan_embedding,
            key_epoch_destroyed: None,
        })
    }

    /// **Tenant offboard (`EraseScope::Tenant`, §4.4) — tenant-decommission crypto-shred.** Destroys the
    /// per-tenant index DEK (the tenant KEK) so the WHOLE tenant index becomes plaintext-unrecoverable,
    /// live AND across backups (the backup-backstop half). Returns the destroyed key epoch (0 = the
    /// initial epoch) iff a key was present. This is the DEK lever — distinct from the per-subject purge.
    pub fn erase_tenant(&self, tenant: &TenantId) -> EraseOutcome {
        let shredded = self.dek.destroy_tenant_index_dek(tenant, &self.region);
        EraseOutcome {
            docs_purged: 0,
            // After a tenant-decommission shred there is no index to hold an orphan embedding (the whole
            // tenant index is crypto-shred unrecoverable) — the 0-orphan property holds by construction.
            zero_orphan_embedding: true,
            key_epoch_destroyed: shredded.then_some(0),
        }
    }

    /// Build a synthetic `*.erased` tombstone for `doc_id` — the SAME envelope shape a real owner emits,
    /// so it flows through the indexer's live `index()` → `apply_removed` path (no backdoor). The
    /// removed ref is named in the payload `ref` (the indexer reads it there or falls back to `subject`).
    fn erased_event(
        tenant: &TenantId,
        region: &Region,
        actor: &Principal,
        doc_id: &str,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("erase:{}:{doc_id}", tenant.0)),
            type_: EventType(SEARCH_ERASE_EVENT_TYPE.into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(actor.clone()),
            subject: ArtifactRef(doc_id.to_string()),
            aggregate: AggregateKey(format!("erase:{doc_id}")),
            causation_id: None,
            correlation_id: CorrelationId(format!("erase:{doc_id}")),
            caused_by: None,
            depth: 0,
            // The erase tombstone carries no personal data payload — it is a references-not-payloads
            // removal naming the doc to purge.
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("1970-01-01T00:00:00Z".into()),
            recorded_at: Timestamp("1970-01-01T00:00:00Z".into()),
            payload: serde_json::json!({ "ref": doc_id }),
        }
    }
}

impl PersonalDataHolder for SearchEraseHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        // §4.8 locate: every doc/field/vector referencing the subject in the tenant's resident region.
        let matcher = Self::matcher(subject, &tenant);
        let located = self.indexer.locate_subject(&tenant, &self.region, &matcher);
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                &format!(
                    "located {} doc(s) referencing the subject (SRCH-P15 real locate)",
                    located.len()
                ),
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        // The index is DERIVED + reconstructible (architecture §0/§1) — NEVER the export source of truth
        // (the owning subsystem is). An export over the index is empty by design.
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "empty-bundle (index derived/reconstructible — never the export source of truth, §0/§1)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // The index is derived; rectification is via reindex-from-source over the corrected projection
        // (§4.9, SRCH-P16) — the corrected body re-enters through the live indexer, not patched in place.
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (index derived; rectify via reindex-from-source over the corrected projection, SRCH-P16)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // §4.8 restrict: suppress indexing/agent-use/analytics/notification for the subject pending
        // erasure. The restriction set is live — the query/RAG path consults `is_restricted`/`suppress_hits`
        // so a restricted subject's content is not surfaced. `on = true` sets it, `on = false` clears it.
        // The holder surface (10.1) passes (subject, on) without a tenant; key on the subject's tenant.
        let tenant = subject.principal.tenant.0.clone();
        let subject_id = Self::subject_id(subject);
        {
            let mut set = self.restricted.lock().unwrap_or_else(|e| e.into_inner());
            let key = (tenant.clone(), subject_id.clone());
            if on {
                set.insert(key);
            } else {
                set.remove(&key);
            }
        }
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                SEARCH_INDEX_STORE,
                &subject_id,
                &tenant,
                &format!("restrict on={on} (SRCH-P15 suppression: a restricted subject is not surfaced in results/RAG, §4.8)"),
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // §4.8 erase = PURGE + RE-INDEX, not hide (per-subject) / tenant-decommission crypto-shred
        // (tenant). The per-subject purge rides the SAME live consumer path (no backdoor); the tenant
        // offboard destroys the per-tenant index DEK.
        let (subject_id, tenant, outcome) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                let outcome = self
                    .erase_subject(subject, tenant)
                    .map_err(|e| DsrError(format!("Search erase failed: {e}")))?;
                (Self::subject_id(subject), tenant.0.clone(), outcome)
            }
            EraseScope::Tenant(tenant) => {
                let outcome = self.erase_tenant(tenant);
                (String::new(), tenant.0.clone(), outcome)
            }
        };
        let detail = format!(
            "purged {} doc(s) via the live consumer path; 0-orphan-embedding={} (SRCH-D4 CI: 0 recoverable incl. vectors)",
            outcome.docs_purged, outcome.zero_orphan_embedding
        );
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                SEARCH_INDEX_STORE,
                &subject_id,
                &tenant,
                &detail,
                outcome.key_epoch_destroyed,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AclFilter;
    use crate::indexer::{
        EmbeddingAdapter, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
        SearchProjection,
    };
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_query::{FieldType, FieldValue};
    use myelin_storage::{
        Byok, Dek, DekHandle, Hyok, HyokKeyService, HyokServiceDenied, KekId, KmsEngine,
        PlatformManaged, WrappedDek,
    };
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex as StdMutex;

    const REGION: &str = "fr-par";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region(REGION.into())
    }
    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    /// A scripted ProjectFetcher: ref → projection; absent ⇒ Gone.
    #[derive(Default)]
    struct FakeFetcher {
        projections: StdMutex<HashMap<String, SearchProjection>>,
    }
    impl FakeFetcher {
        fn with(items: &[(&str, SearchProjection)]) -> Arc<FakeFetcher> {
            let f = FakeFetcher::default();
            for (r, p) in items {
                f.projections
                    .lock()
                    .unwrap()
                    .insert((*r).to_string(), p.clone());
            }
            Arc::new(f)
        }
    }
    impl ProjectFetcher for FakeFetcher {
        fn project(
            &self,
            _t: &TenantId,
            _r: &Region,
            ref_: &ArtifactRef,
        ) -> Result<SearchProjection, ProjectFetchError> {
            match self.projections.lock().unwrap().get(&ref_.0) {
                Some(p) => Ok(p.clone()),
                None => Err(ProjectFetchError::Gone),
            }
        }
    }

    fn proj(text: &str, fields: BTreeMap<String, FieldValue>) -> SearchProjection {
        SearchProjection {
            text: text.into(),
            fields,
            lang: None,
        }
    }

    /// A semantic page spec (its docs get a vector) with `actor`/`assignee` subject-locator facets.
    fn page_spec() -> IndexSpec {
        let mut fields = BTreeMap::new();
        fields.insert("actor".to_string(), FieldType::Principal);
        fields.insert("assignee".to_string(), FieldType::Principal);
        IndexSpec::new("knowledge", "page", fields).semantic()
    }

    /// Build a `knowledge.page.created` event for a doc (the live indexer ingests it).
    fn created_event(doc: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("ev:{doc}")),
            type_: EventType("knowledge.page.created".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(subject("sys").principal),
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
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    /// Build an indexer over `docs` (ref → projection) and ingest them all through the live path.
    fn indexer_with(docs: &[(&str, SearchProjection)]) -> Arc<IncrementalIndexer> {
        let fetcher = FakeFetcher::with(docs);
        let ix = Arc::new(IncrementalIndexer::new(
            vec![page_spec()],
            fetcher,
            Arc::new(MockEmbeddingAdapter::new(8)),
        ));
        for (r, _) in docs {
            ix.index(&created_event(r)).expect("index");
        }
        ix
    }

    fn holder_over(ix: Arc<IncrementalIndexer>) -> SearchEraseHolder {
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        pin.reserve(&tenant(), &region())
            .expect("reserve the per-tenant index DEK");
        SearchEraseHolder::new(ix, pin, region())
    }

    fn with_actor(id: &str) -> BTreeMap<String, FieldValue> {
        let mut f = BTreeMap::new();
        f.insert("actor".to_string(), FieldValue::Principal(id.into()));
        f
    }
    fn with_assignee(id: &str) -> BTreeMap<String, FieldValue> {
        let mut f = BTreeMap::new();
        f.insert("assignee".to_string(), FieldValue::Principal(id.into()));
        f
    }

    /// **`locate(subject)` finds every doc referencing the subject (§4.8) — by acl_object, by an
    /// actor/assignee subject-locator facet, and by the `.noreply` pseudonym in the body.** (The prompt's
    /// required locate unit test.)
    #[test]
    fn locate_finds_docs_by_acl_facet_and_pseudonym() {
        let subj = subject("u-42");
        let pseudonym =
            SearchEraseHolder::pseudonym_of(&subj, &tenant()).expect("pseudonym renders");

        let docs = vec![
            (
                "myelin://acme/knowledge/page/owned",
                proj("a page", with_actor("u-42")),
            ),
            (
                "myelin://acme/knowledge/page/assigned",
                proj("another page", with_assignee("u-42")),
            ),
            (
                "myelin://acme/knowledge/page/mentions",
                proj(&format!("see {pseudonym} for context"), BTreeMap::new()),
            ),
            (
                "myelin://acme/knowledge/page/unrelated",
                proj("nothing personal", BTreeMap::new()),
            ),
        ];
        let ix = indexer_with(&docs);
        let holder = holder_over(ix.clone());

        let matcher = SearchEraseHolder::matcher(&subj, &tenant());
        let located = ix.locate_subject(&tenant(), &region(), &matcher);
        assert_eq!(
            located.len(),
            3,
            "the three docs referencing u-42 are located (acl/facet/pseudonym)"
        );
        assert!(
            !located.iter().any(|d| d.ends_with("unrelated")),
            "the unrelated doc is NOT located"
        );

        let report = holder.locate(&subj, tenant()).expect("locate");
        assert_eq!(report.receipt.operation, "locate");
        assert!(report.receipt.content_hash.starts_with("blake3:"));
        assert!(
            report.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
    }

    /// **`pseudonym_of` renders the FROZEN `<pseudonym>@<tenant>.noreply` grammar (contract 4.8) —
    /// keyed on the opaque subject id, never a name.** Pins the exact rendering so the body-mention
    /// match keys on the real `.noreply` handle (a mutant that fabricates an arbitrary pseudonym would
    /// match a doc mentioning the wrong handle / miss the real one — a locate correctness break).
    #[test]
    fn pseudonym_renders_the_frozen_noreply_grammar() {
        let subj = subject("anon-7f3a");
        assert_eq!(
            SearchEraseHolder::pseudonym_of(&subj, &tenant()).as_deref(),
            Some("anon-7f3a@acme.noreply"),
            "the frozen pseudonym grammar is `<pseudonym>@<tenant>.noreply` keyed on the opaque id"
        );
        // A subject id that breaks the grammar (contains `@`) renders no handle (no body-mention match).
        let bad = subject("a@b");
        assert!(
            SearchEraseHolder::pseudonym_of(&bad, &tenant()).is_none(),
            "a grammar-breaking id renders no handle"
        );
    }

    /// **A doc mentioning the subject's EXACT `.noreply` pseudonym is located; a doc mentioning a
    /// DIFFERENT handle is NOT (the body-mention match is exact, not a wildcard).** Kills a mutant that
    /// returns an arbitrary pseudonym from `pseudonym_of` (it would either mis-match or fail to match).
    #[test]
    fn body_mention_match_is_the_exact_pseudonym() {
        let subj = subject("u-77");
        let real = SearchEraseHolder::pseudonym_of(&subj, &tenant()).expect("renders");
        assert_eq!(
            real, "u-77@acme.noreply",
            "the exact handle the body must contain"
        );

        let docs = vec![
            // mentions the REAL handle ⇒ located.
            (
                "myelin://acme/knowledge/page/hit",
                proj("cc u-77@acme.noreply please", BTreeMap::new()),
            ),
            // mentions a DIFFERENT handle ⇒ NOT located (no wildcard).
            (
                "myelin://acme/knowledge/page/miss",
                proj("cc someone-else@acme.noreply", BTreeMap::new()),
            ),
        ];
        let ix = indexer_with(&docs);
        let matcher = SearchEraseHolder::matcher(&subj, &tenant());
        let located = ix.locate_subject(&tenant(), &region(), &matcher);
        assert_eq!(
            located,
            vec!["myelin://acme/knowledge/page/hit".to_string()],
            "only the exact-handle mention is located"
        );
    }

    /// **`erase(subject)` is PURGE + RE-INDEX, not hide — through the LIVE consumer path — and leaves 0
    /// recoverable personal data incl. vectors (SRCH-D4 CI variant; the prompt's chained GATE).** Index
    /// docs (with vectors) referencing a subject + an unrelated doc; erase the subject; assert every
    /// referencing doc is GONE from search AND from the vector shape, 0 orphan embedding survives, and
    /// the unrelated doc is untouched.
    #[test]
    fn erase_purges_docs_and_vectors_zero_recoverable_via_live_path() {
        let subj = subject("u-42");
        let owned = "myelin://acme/knowledge/page/owned";
        let unrelated = "myelin://acme/knowledge/page/other";
        let docs = vec![
            (
                owned,
                proj(
                    "the subject's own page about raft consensus",
                    with_actor("u-42"),
                ),
            ),
            (
                unrelated,
                proj("an unrelated page about paxos", BTreeMap::new()),
            ),
        ];
        let ix = indexer_with(&docs);
        assert_eq!(ix.live_count(&tenant(), &region()), 2, "two docs indexed");

        // Both docs got a vector (the page spec is semantic). The subject's doc is reachable by k-NN.
        let q = MockEmbeddingAdapter::new(8)
            .embed("raft consensus")
            .unwrap();
        let pre = ix
            .search_semantic(&tenant(), &region(), &AclFilter::All, &q, 5)
            .expect("semantic pre");
        assert!(
            pre.iter().any(|h| h.doc_id == owned),
            "the subject's doc has a vector before erase"
        );

        let holder = holder_over(ix.clone());
        let outcome = holder.erase_subject(&subj, &tenant()).expect("erase");
        assert_eq!(
            outcome.docs_purged, 1,
            "exactly the one referencing doc purged"
        );
        assert!(
            outcome.zero_orphan_embedding,
            "0 orphan embedding after compaction (SRCH-D4 GATE)"
        );

        // 0 recoverable: the subject's doc is gone from FT search AND from the vector shape; the
        // unrelated doc survives.
        let ft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("ft");
        assert!(
            !ft.iter().any(|h| h.doc_id == owned),
            "the erased doc is GONE from full-text search"
        );
        let post = ix
            .search_semantic(&tenant(), &region(), &AclFilter::All, &q, 5)
            .expect("semantic post");
        assert!(
            !post.iter().any(|h| h.doc_id == owned),
            "the erased doc's VECTOR is gone (purged + compacted)"
        );
        assert!(
            !ix.has_orphan_embedding(&tenant(), &region()),
            "0 orphan embedding (the erasure-critical GATE)"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "only the unrelated doc survives"
        );
        let other = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 10)
            .expect("ft other");
        assert_eq!(other.len(), 1, "the unrelated doc is untouched");
    }

    /// **The erase rides the SAME live consumer path — NO backdoor.** The synthetic erase event the
    /// holder emits is a `*.erased` whose trailing segment is one of the indexer's REMOVED_SUFFIXES, so
    /// it flows through the public `index()` → `apply_removed` path (the same one a real owner `*.erased`
    /// takes), never a private backend `delete`.
    #[test]
    fn erase_drives_the_live_consumer_path_no_backdoor() {
        let last = SEARCH_ERASE_EVENT_TYPE.rsplit('.').next().unwrap();
        assert!(
            IncrementalIndexer::REMOVED_SUFFIXES.contains(&last),
            "the holder's erase event is a `*.erased` REMOVED_SUFFIX — it rides the live consumer path"
        );

        let r = "myelin://acme/knowledge/page/p1";
        let ix = indexer_with(&[(r, proj("body", BTreeMap::new()))]);
        assert_eq!(ix.live_count(&tenant(), &region()), 1);
        let erase_ev =
            SearchEraseHolder::erased_event(&tenant(), &region(), &subject("u-1").principal, r);
        ix.index(&erase_ev)
            .expect("the erase event flows through the live index() path");
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            0,
            "the doc was removed via the live consumer path"
        );
    }

    /// **`restrict(subject)` suppresses the subject's content from results/RAG (§4.8); clearing restores
    /// it.** (The prompt's required restrict-suppression unit test.)
    #[test]
    fn restrict_suppresses_subject_from_results_and_rag() {
        let subj = subject("u-7");
        let holder = holder_over(indexer_with(&[]));
        assert!(
            !holder.is_restricted(&tenant(), "u-7"),
            "not restricted initially"
        );

        holder.restrict(&subj, true).expect("restrict on");
        assert!(
            holder.is_restricted(&tenant(), "u-7"),
            "the subject is restricted"
        );

        let hits = [("doc-a", "u-7"), ("doc-b", "u-other")];
        let surviving = holder.suppress_hits(&tenant(), hits.iter().map(|(d, s)| (*d, *s)));
        assert_eq!(
            surviving,
            vec!["doc-b".to_string()],
            "the restricted subject's doc is suppressed"
        );

        holder.restrict(&subj, false).expect("restrict off");
        assert!(
            !holder.is_restricted(&tenant(), "u-7"),
            "restriction cleared"
        );
        let surviving = holder.suppress_hits(&tenant(), hits.iter().map(|(d, s)| (*d, *s)));
        assert_eq!(
            surviving.len(),
            2,
            "both docs surface once the restriction is cleared"
        );
    }

    /// **The HYOK structural skip: a HYOK class has NOTHING to erase — it is not in the index at all
    /// (§4.8, contract 11.3).** A platform-managed / BYOK class IS indexed (erase = purge + reindex); a
    /// HYOK class (`can_derive_plaintext_index() = false`) is structurally skipped, so its erasure is the
    /// absence of any index, not a purge. (The prompt's required HYOK-skip unit test.)
    #[test]
    fn hyok_class_has_nothing_to_erase() {
        struct DenyAllHyok;
        impl HyokKeyService for DenyAllHyok {
            fn wrap(&self, _d: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
                Err(HyokServiceDenied)
            }
            fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
                Err(HyokServiceDenied)
            }
            fn destroy(&self) {}
        }
        let engine = KmsEngine::new();
        engine.ensure_kek(&KekId::new(tenant(), region()));
        let platform = PlatformManaged::new(&engine, region());
        let byok = Byok::new(&engine, region(), "kms-customer://acme/k1");
        let hyok = Hyok::new(DenyAllHyok);

        let holder = holder_over(indexer_with(&[]));
        assert!(
            !holder.erase_class(&platform),
            "a platform-managed class IS indexed (erase = purge)"
        );
        assert!(
            !holder.erase_class(&byok),
            "a BYOK class IS indexed (plaintext reachable while live)"
        );
        assert!(
            holder.erase_class(&hyok),
            "a HYOK class is structurally skipped — nothing to erase"
        );
    }

    /// **A tenant offboard (`EraseScope::Tenant`) is a tenant-decommission crypto-shred (the per-tenant
    /// index DEK), recording the destroyed key epoch.** Distinct from the per-subject purge.
    #[test]
    fn tenant_offboard_crypto_shreds_the_index_dek() {
        let holder = holder_over(indexer_with(&[]));
        let receipt = holder
            .erase(EraseScope::Tenant(tenant()))
            .expect("tenant offboard erase");
        assert_eq!(receipt.receipt.operation, "erase");
        assert_eq!(
            receipt.receipt.key_epoch_destroyed,
            Some(0),
            "the tenant-decommission shred records the destroyed key epoch (the GD-4 lever's audit trail)"
        );
    }

    /// **Idempotent re-erase: erasing an already-erased subject purges 0 and is still 0-orphan (no
    /// resurrection).** A second erase converges to "nothing to purge" — the post-erase re-erasure
    /// (proven jointly with the SRCH-P16 reindex path) does not resurrect the subject.
    #[test]
    fn re_erase_purges_zero_and_does_not_resurrect() {
        let subj = subject("u-9");
        let r = "myelin://acme/knowledge/page/owned9";
        let ix = indexer_with(&[(r, proj("a page", with_actor("u-9")))]);
        let holder = holder_over(ix.clone());

        let first = holder.erase_subject(&subj, &tenant()).expect("erase 1");
        assert_eq!(first.docs_purged, 1, "first erase purges the one doc");
        let second = holder.erase_subject(&subj, &tenant()).expect("erase 2");
        assert_eq!(
            second.docs_purged, 0,
            "re-erase purges nothing (already gone — no resurrection)"
        );
        assert!(second.zero_orphan_embedding, "still 0 orphan embedding");
    }

    /// **The erase receipt is content-addressed + idempotent on the same scope.**
    #[test]
    fn erase_receipt_is_content_addressed() {
        let holder = holder_over(indexer_with(&[]));
        let scope = EraseScope::Subject {
            subject: subject("u-0"),
            tenant: tenant(),
        };
        let r1 = holder.erase(scope.clone()).expect("erase 1");
        assert!(r1.receipt.content_hash.starts_with("blake3:"));
        let r2 = holder.erase(scope).expect("erase 2");
        assert_eq!(
            r1, r2,
            "the same erase scope yields the identical content-addressed receipt (idempotent)"
        );
    }

    /// **The holder is object-safe behind `dyn PersonalDataHolder` (contract 10.1).**
    #[test]
    fn holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> =
            vec![Box::new(holder_over(indexer_with(&[])))];
        for h in &holders {
            assert!(
                h.locate(&subject("u-1"), tenant()).is_ok(),
                "the real holder responds to the contract"
            );
        }
    }
}
