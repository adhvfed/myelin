use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::dek::SearchDekPin;
use crate::engine::SubjectMatcher;
use crate::indexer::IncrementalIndexer;
use crate::store::SEARCH_INDEX_STORE;
use chrono::{SecondsFormat, Utc};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Region, TenantId, Timestamp, Visibility,
};
use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef,
};
use myelin_identity::Principal;

pub const SEARCH_ERASE_EVENT_TYPE: &str = "search.subject.erased";

pub trait ErasureEventClock: Send + Sync {
    fn now(&self) -> Timestamp;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemErasureEventClock;

impl ErasureEventClock for SystemErasureEventClock {
    fn now(&self) -> Timestamp {
        Timestamp(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true))
    }
}

#[derive(Clone)]
pub struct SearchEraseHolder {
    indexer: Arc<IncrementalIndexer>,
    dek: SearchDekPin,
    region: Region,
    restricted: Arc<Mutex<BTreeSet<(String, String)>>>,
    clock: Arc<dyn ErasureEventClock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EraseOutcome {
    pub docs_purged: usize,
    pub zero_orphan_embedding: bool,
    pub key_epoch_destroyed: Option<u64>,
}

impl SearchEraseHolder {
    pub fn new(
        indexer: Arc<IncrementalIndexer>,
        dek: SearchDekPin,
        region: Region,
    ) -> SearchEraseHolder {
        Self::with_clock(indexer, dek, region, Arc::new(SystemErasureEventClock))
    }

    pub fn with_clock(
        indexer: Arc<IncrementalIndexer>,
        dek: SearchDekPin,
        region: Region,
        clock: Arc<dyn ErasureEventClock>,
    ) -> SearchEraseHolder {
        SearchEraseHolder {
            indexer,
            dek,
            region,
            restricted: Arc::new(Mutex::new(BTreeSet::new())),
            clock,
        }
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    fn pseudonym_of(subject: &SubjectRef, tenant: &TenantId) -> Option<String> {
        use myelin_identity::PseudonymHandle;
        PseudonymHandle::new(&subject.principal.principal_id.0, &tenant.0).map(|h| h.render())
    }

    fn matcher(subject: &SubjectRef, tenant: &TenantId) -> SubjectMatcher {
        SubjectMatcher::new(
            Self::subject_id(subject),
            Self::pseudonym_of(subject, tenant),
        )
    }

    pub fn locate_doc_count(&self, subject: &SubjectRef, tenant: &TenantId) -> usize {
        let matcher = Self::matcher(subject, tenant);
        self.indexer
            .locate_subject(tenant, &self.region, &matcher)
            .len()
    }

    pub fn is_restricted(&self, tenant: &TenantId, subject_id: &str) -> bool {
        self.restricted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(tenant.0.clone(), subject_id.to_string()))
    }

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

    pub fn erase_subject(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
    ) -> Result<EraseOutcome, crate::engine::IndexError> {
        let region = self.region.clone();
        let matcher = Self::matcher(subject, tenant);
        let located = self.indexer.locate_subject(tenant, &region, &matcher);
        let docs_purged = located.len();

        for doc_id in &located {
            let ev = self.erased_event(tenant, &region, &subject.principal, doc_id);
            self.indexer.index(&ev).map_err(|e| {
                crate::engine::IndexError::Engine(format!("erase purge failed: {e:?}"))
            })?;
        }

        self.indexer.compact(tenant, &region)?;
        let zero_orphan_embedding = !self.indexer.has_orphan_embedding(tenant, &region);

        Ok(EraseOutcome {
            docs_purged,
            zero_orphan_embedding,
            key_epoch_destroyed: None,
        })
    }

    pub fn erase_tenant(
        &self,
        tenant: &TenantId,
    ) -> Result<EraseOutcome, crate::engine::IndexError> {
        let shredded = self
            .dek
            .destroy_tenant_index_dek(tenant, &self.region)
            .map_err(|error| crate::engine::IndexError::Engine(error.to_string()))?;
        Ok(EraseOutcome {
            docs_purged: 0,
            zero_orphan_embedding: true,
            key_epoch_destroyed: shredded.then_some(0),
        })
    }

    fn erased_event(
        &self,
        tenant: &TenantId,
        region: &Region,
        actor: &Principal,
        doc_id: &str,
    ) -> EventEnvelope {
        let now = self.clock.now();
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
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: now.clone(),
            recorded_at: now,
            payload: serde_json::json!({ "ref": doc_id }),
        }
    }
}

impl PersonalDataHolder for SearchEraseHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
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
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "empty-bundle (index derived/reconstructible - never the export source of truth, §0/§1)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
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
        let (subject_id, tenant, outcome) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                let outcome = self
                    .erase_subject(subject, tenant)
                    .map_err(|e| DsrError(format!("Search erase failed: {e}")))?;
                (Self::subject_id(subject), tenant.0.clone(), outcome)
            }
            EraseScope::Tenant(tenant) => {
                let outcome = self
                    .erase_tenant(tenant)
                    .map_err(|e| DsrError(format!("Search tenant erase failed: {e}")))?;
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
    use myelin_storage::KmsEngine;
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

    fn page_spec() -> IndexSpec {
        let mut fields = BTreeMap::new();
        fields.insert("actor".to_string(), FieldType::Principal);
        fields.insert("assignee".to_string(), FieldType::Principal);
        IndexSpec::new("knowledge", "page", fields).semantic()
    }

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

    #[derive(Clone, Copy)]
    struct FixedErasureEventClock;

    impl ErasureEventClock for FixedErasureEventClock {
        fn now(&self) -> Timestamp {
            Timestamp("2026-08-26T02:48:00Z".into())
        }
    }

    fn holder_over_at_fixed_time(ix: Arc<IncrementalIndexer>) -> SearchEraseHolder {
        let pin = SearchDekPin::new(Arc::new(KmsEngine::new()));
        pin.reserve(&tenant(), &region())
            .expect("reserve the per-tenant index DEK");
        SearchEraseHolder::with_clock(ix, pin, region(), Arc::new(FixedErasureEventClock))
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

    #[test]
    fn pseudonym_renders_the_frozen_noreply_grammar() {
        let subj = subject("anon-7f3a");
        assert_eq!(
            SearchEraseHolder::pseudonym_of(&subj, &tenant()).as_deref(),
            Some("anon-7f3a@acme.noreply"),
            "the frozen pseudonym grammar is `<pseudonym>@<tenant>.noreply` keyed on the opaque id"
        );
        let bad = subject("a@b");
        assert!(
            SearchEraseHolder::pseudonym_of(&bad, &tenant()).is_none(),
            "a grammar-breaking id renders no handle"
        );
    }

    #[test]
    fn body_mention_match_is_the_exact_pseudonym() {
        let subj = subject("u-77");
        let real = SearchEraseHolder::pseudonym_of(&subj, &tenant()).expect("renders");
        assert_eq!(
            real, "u-77@acme.noreply",
            "the exact handle the body must contain"
        );

        let docs = vec![
            (
                "myelin://acme/knowledge/page/hit",
                proj("cc u-77@acme.noreply please", BTreeMap::new()),
            ),
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

    #[test]
    fn erase_drives_the_live_consumer_path_no_backdoor() {
        let last = SEARCH_ERASE_EVENT_TYPE.rsplit('.').next().unwrap();
        assert!(
            IncrementalIndexer::REMOVED_SUFFIXES.contains(&last),
            "the holder's erase event is a `*.erased` REMOVED_SUFFIX - it rides the live consumer path"
        );

        let r = "myelin://acme/knowledge/page/p1";
        let ix = indexer_with(&[(r, proj("body", BTreeMap::new()))]);
        assert_eq!(ix.live_count(&tenant(), &region()), 1);
        let holder = holder_over_at_fixed_time(ix.clone());
        let erase_ev = holder.erased_event(&tenant(), &region(), &subject("u-1").principal, r);
        assert_eq!(
            erase_ev.occurred_at,
            Timestamp("2026-08-26T02:48:00Z".into()),
            "an erasure event records when the privacy operation actually happened"
        );
        assert_eq!(
            erase_ev.recorded_at, erase_ev.occurred_at,
            "the synchronous local projection records the erasure at the observed operation time"
        );
        ix.index(&erase_ev)
            .expect("the erase event flows through the live index() path");
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            0,
            "the doc was removed via the live consumer path"
        );
    }

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
            "re-erase purges nothing (already gone - no resurrection)"
        );
        assert!(second.zero_orphan_embedding, "still 0 orphan embedding");
    }

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
