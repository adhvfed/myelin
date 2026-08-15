use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{EmitContextBase, OutboxStore, ReindexSource, SnapshotScope};
use myelin_gdpr::SubjectRef;
use myelin_tenancy::{Region, TenantId};

use crate::erase::SearchEraseHolder;
use crate::reindex::{ReindexError, SearchReindexer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasedSubjectEntry {
    pub subject_id: String,
    pub subject: SubjectRef,
    pub erased_at: String,
}

#[derive(Clone)]
pub struct SearchErasureLedger {
    tenant: TenantId,
    region: Region,
    entries: Arc<Mutex<BTreeMap<String, ErasedSubjectEntry>>>,
}

impl SearchErasureLedger {
    pub fn new(tenant: TenantId, region: Region) -> SearchErasureLedger {
        SearchErasureLedger {
            tenant,
            region,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn region(&self) -> &Region {
        &self.region
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, ErasedSubjectEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn record(&self, subject: &SubjectRef, erased_at: &str) {
        let subject_id = subject.principal.principal_id.0.clone();
        let mut g = self.lock();
        g.entry(subject_id.clone())
            .or_insert_with(|| ErasedSubjectEntry {
                subject_id,
                subject: subject.clone(),
                erased_at: erased_at.to_string(),
            });
    }

    pub fn is_erased(&self, subject_id: &str) -> bool {
        self.lock().contains_key(subject_id)
    }

    pub fn entries(&self) -> Vec<ErasedSubjectEntry> {
        self.lock().values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRestoreArtifact {
    pub tenant: TenantId,
    pub region: Region,
    pub restored_to_offset: Option<u64>,
    pub live_doc_count: u64,
    pub live_vector_count: usize,
    pub re_erased_subjects: usize,
    pub docs_resurrected_by_restore: usize,
    pub resurrected_docs: usize,
    pub row_doc_vector_mismatches: usize,
    pub orphan_embeddings: bool,
    pub ran_at: String,
}

impl SearchRestoreArtifact {
    pub fn is_green(&self) -> bool {
        self.resurrected_docs == 0 && self.row_doc_vector_mismatches == 0 && !self.orphan_embeddings
    }

    pub fn summary(&self) -> String {
        format!(
            "search restore-verify PASS (SRCH-D9): restored index to offset={:?} via \
             reindex-from-source - {} live docs / {} live vectors (parity), re-erased {} ledger \
             subject(s), {} doc(s) resurrected-by-restore then re-purged; resurrected_docs={}, \
             row_doc_vector_mismatches={}, orphan_embeddings={} (all 0/false). cold==live by \
             construction.",
            self.restored_to_offset,
            self.live_doc_count,
            self.live_vector_count,
            self.re_erased_subjects,
            self.docs_resurrected_by_restore,
            self.resurrected_docs,
            self.row_doc_vector_mismatches,
            self.orphan_embeddings,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchRestoreFailure {
    RestoreFailed(ReindexError),
    ReEraseFailed(String),
    ErasureResurrected {
        subject_id: String,
        surviving_docs: usize,
    },
    RowDocVectorMismatch {
        live_docs: u64,
        live_vectors: usize,
    },
    OrphanEmbedding,
}

impl core::fmt::Display for SearchRestoreFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SearchRestoreFailure::RestoreFailed(e) => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL - the restore (reindex-from-source) failed: {e}"
            ),
            SearchRestoreFailure::ReEraseFailed(e) => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL - the post-restore re-erasure failed: {e}"
            ),
            SearchRestoreFailure::ErasureResurrected {
                subject_id,
                surviving_docs,
            } => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL - ERASURE RESURRECTED: subject {subject_id} was erased \
                 before the backup but has {surviving_docs} live doc(s) after the restore + \
                 re-erasure - a restored backup resurrected an erased subject. THE GRAVEST FAILURE: \
                 it un-erases a person"
            ),
            SearchRestoreFailure::RowDocVectorMismatch {
                live_docs,
                live_vectors,
            } => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL - ROW↔DOC↔VECTOR MISMATCH: {live_docs} live docs but \
                 {live_vectors} live vectors - the restored index is NOT at one consistent point"
            ),
            SearchRestoreFailure::OrphanEmbedding => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL - ORPHAN EMBEDDING: a tombstoned vector's bytes survived \
                 the re-erasure compaction (embeddings are personal data, §3.3)"
            ),
        }
    }
}

impl std::error::Error for SearchRestoreFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a Search restore-verify verdict must be checked - a dropped RED is a SWALLOWED \
              resurrected-erased-subject / silent-corruption failure (the permanent gate, EI-01 §5: \
              loud-never-swallowed)"]
pub enum SearchRestoreVerdict {
    Green(SearchRestoreArtifact),
    Red(SearchRestoreFailure),
}

impl SearchRestoreVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, SearchRestoreVerdict::Green(_))
    }

    pub fn artifact(&self) -> Option<&SearchRestoreArtifact> {
        match self {
            SearchRestoreVerdict::Green(a) => Some(a),
            SearchRestoreVerdict::Red(_) => None,
        }
    }

    pub fn failure(&self) -> Option<&SearchRestoreFailure> {
        match self {
            SearchRestoreVerdict::Red(f) => Some(f),
            SearchRestoreVerdict::Green(_) => None,
        }
    }
}

pub struct SearchRestoreInputs<'a> {
    pub reindexer: &'a SearchReindexer,
    pub erase_holder: &'a SearchEraseHolder,
    pub ledger: &'a SearchErasureLedger,
    pub tenant: TenantId,
    pub scope: SnapshotScope,
    pub restore_to_offset: Option<u64>,
    pub sources: &'a [&'a dyn ReindexSource],
    pub outbox: &'a mut OutboxStore,
    pub ctx_base: EmitContextBase,
    pub now: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SearchRestoreVerifyGate;

impl SearchRestoreVerifyGate {
    pub fn new() -> SearchRestoreVerifyGate {
        SearchRestoreVerifyGate
    }

    pub fn run(&self, inputs: &mut SearchRestoreInputs<'_>) -> SearchRestoreVerdict {
        let tenant = inputs.tenant.clone();
        let region = inputs.reindexer.region().clone();

        if let Err(e) = inputs.reindexer.reindex(
            &tenant,
            &inputs.scope,
            inputs.restore_to_offset,
            inputs.sources,
            inputs.outbox,
            inputs.ctx_base.clone(),
        ) {
            return SearchRestoreVerdict::Red(SearchRestoreFailure::RestoreFailed(e));
        }

        let entries = inputs.ledger.entries();

        let mut docs_resurrected_by_restore = 0usize;
        for entry in &entries {
            docs_resurrected_by_restore +=
                self.live_docs_for(inputs.erase_holder, &entry.subject, &tenant);
        }

        for entry in &entries {
            if let Err(e) = inputs.erase_holder.erase_subject(&entry.subject, &tenant) {
                return SearchRestoreVerdict::Red(SearchRestoreFailure::ReEraseFailed(format!(
                    "{e:?}"
                )));
            }
        }

        let mut resurrected_docs = 0usize;
        for entry in &entries {
            let surviving = self.live_docs_for(inputs.erase_holder, &entry.subject, &tenant);
            if surviving > 0 {
                return SearchRestoreVerdict::Red(SearchRestoreFailure::ErasureResurrected {
                    subject_id: entry.subject_id.clone(),
                    surviving_docs: surviving,
                });
            }
            resurrected_docs += surviving;
        }

        let live_docs = match Self::verification_live_count(
            inputs.reindexer.try_indexer_live_count(&tenant, &region),
        ) {
            Ok(count) => count,
            Err(failure) => return SearchRestoreVerdict::Red(failure),
        };
        let live_vectors = inputs.reindexer.indexer_live_vector_count(&tenant, &region);
        if (live_docs as usize) != live_vectors {
            return SearchRestoreVerdict::Red(SearchRestoreFailure::RowDocVectorMismatch {
                live_docs,
                live_vectors,
            });
        }

        let orphan = inputs
            .reindexer
            .indexer_has_orphan_embedding(&tenant, &region);
        if orphan {
            return SearchRestoreVerdict::Red(SearchRestoreFailure::OrphanEmbedding);
        }

        SearchRestoreVerdict::Green(SearchRestoreArtifact {
            tenant,
            region,
            restored_to_offset: inputs.restore_to_offset,
            live_doc_count: live_docs,
            live_vector_count: live_vectors,
            re_erased_subjects: entries.len(),
            docs_resurrected_by_restore,
            resurrected_docs,
            row_doc_vector_mismatches: 0,
            orphan_embeddings: false,
            ran_at: inputs.now.clone(),
        })
    }

    pub fn run_or_fail_ci(
        &self,
        inputs: &mut SearchRestoreInputs<'_>,
    ) -> Result<SearchRestoreArtifact, SearchRestoreFailure> {
        match self.run(inputs) {
            SearchRestoreVerdict::Green(artifact) => Ok(artifact),
            SearchRestoreVerdict::Red(failure) => Err(failure),
        }
    }

    fn live_docs_for(
        &self,
        holder: &SearchEraseHolder,
        subject: &SubjectRef,
        tenant: &TenantId,
    ) -> usize {
        holder.locate_doc_count(subject, tenant)
    }

    fn verification_live_count(
        count: Result<u64, ReindexError>,
    ) -> Result<u64, SearchRestoreFailure> {
        count.map_err(SearchRestoreFailure::RestoreFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dek::SearchDekPin;
    use crate::engine::AclFilter;
    use crate::indexer::{
        IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
        SearchProjection,
    };
    use myelin_events::reindex::ReferenceReindexSource;
    use myelin_events::{Actor, ArtifactRef, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};
    use myelin_storage::KmsEngine;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    const REGION: &str = "fr-par";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region(REGION.into())
    }
    fn platform() -> Principal {
        Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )
    }
    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(platform()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
            caused_by: None,
        }
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    #[derive(Default)]
    struct OwnerProjection {
        bodies: StdMutex<HashMap<String, String>>,
    }
    impl OwnerProjection {
        fn put(&self, ref_: &str, body: &str) {
            self.bodies
                .lock()
                .unwrap()
                .insert(ref_.to_string(), body.to_string());
        }
        fn remove(&self, ref_: &str) {
            self.bodies.lock().unwrap().remove(ref_);
        }
    }
    impl ProjectFetcher for OwnerProjection {
        fn project(
            &self,
            _t: &TenantId,
            _r: &Region,
            ref_: &ArtifactRef,
        ) -> Result<SearchProjection, ProjectFetchError> {
            match self.bodies.lock().unwrap().get(&ref_.0) {
                Some(body) => Ok(SearchProjection {
                    text: body.clone(),
                    fields: BTreeMap::new(),
                    lang: None,
                }),
                None => Err(ProjectFetchError::Gone),
            }
        }
    }

    fn page_spec() -> IndexSpec {
        IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()
    }

    fn snapshot_ref(agg: &str) -> String {
        format!("myelin://acme/knowledge/page/{agg}")
    }
    fn scope() -> SnapshotScope {
        SnapshotScope::new("knowledge", "page:all")
    }

    #[allow(clippy::type_complexity)]
    fn cell() -> (
        Arc<IncrementalIndexer>,
        Arc<OwnerProjection>,
        SearchReindexer,
        SearchEraseHolder,
    ) {
        let fetcher = Arc::new(OwnerProjection::default());
        let ix = Arc::new(IncrementalIndexer::new(
            vec![page_spec()],
            fetcher.clone(),
            Arc::new(MockEmbeddingAdapter::new(8)),
        ));
        let reindexer = SearchReindexer::new(ix.clone(), region());
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        pin.reserve(&tenant(), &region())
            .expect("reserve index DEK");
        let holder = SearchEraseHolder::new(ix.clone(), pin, region());
        (ix, fetcher, reindexer, holder)
    }

    fn pseudonym(id: &str) -> String {
        PseudonymHandle::new(id, &tenant().0)
            .expect("pseudonym renders")
            .render()
    }

    #[test]
    fn the_gate_greens_a_whole_restore_with_re_erasure() {
        let (ix, fetcher, reindexer, holder) = cell();
        let erased = subject("u-erased");
        let pn = pseudonym("u-erased");

        let mut src_before = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src_before.upsert("owned", 1, serde_json::json!({ "kind": "page" }));
        src_before.upsert("other", 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(
            &snapshot_ref("owned"),
            &format!("a page mentioning {pn} about raft"),
        );
        fetcher.put(&snapshot_ref("other"), "an unrelated page about paxos");

        let ledger = SearchErasureLedger::new(tenant(), region());
        ledger.record(&erased, "2026-06-20T00:00:00Z");
        assert!(ledger.is_erased("u-erased"));

        let mut outbox = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src_before];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: scope(),
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };

        let verdict = SearchRestoreVerifyGate::new().run(&mut inputs);
        assert!(
            verdict.is_green(),
            "a whole restore + re-erase must GREEN, got {:?}",
            verdict.failure()
        );
        let a = verdict.artifact().expect("green artifact");
        assert_eq!(a.re_erased_subjects, 1, "the ledger's one subject replayed");
        assert_eq!(
            a.docs_resurrected_by_restore, 1,
            "the restore brought the erased subject's doc back"
        );
        assert_eq!(a.resurrected_docs, 0, "0 resurrected docs post-re-erase");
        assert_eq!(a.row_doc_vector_mismatches, 0);
        assert!(!a.orphan_embeddings, "0 orphan embedding");
        assert_eq!(a.live_doc_count, 1, "only the unrelated page survives");
        assert_eq!(a.live_vector_count, 1, "exactly one live vector (parity)");
        let raft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("ft");
        assert!(
            raft.is_empty(),
            "the erased subject's page is NOT resurrected"
        );
        let paxos = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 10)
            .expect("ft");
        assert_eq!(paxos.len(), 1, "the unrelated page is searchable");
        let s = a.summary();
        assert!(s.contains("search restore-verify PASS (SRCH-D9)"));
        assert!(s.contains("re-erased 1 ledger subject"));
        let _ = fetcher;
    }

    #[test]
    fn run_or_fail_ci_returns_ok_on_green_empty_ledger() {
        let (_ix, fetcher, reindexer, holder) = cell();
        let mut src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src.upsert("a", 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(&snapshot_ref("a"), "a page about consensus");
        let ledger = SearchErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: scope(),
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };
        let a = SearchRestoreVerifyGate::new()
            .run_or_fail_ci(&mut inputs)
            .expect("a whole restore must not fail CI");
        assert_eq!(a.re_erased_subjects, 0, "nothing to re-erase");
        assert_eq!(a.live_doc_count, 1);
        assert_eq!(a.live_vector_count, 1, "parity");
    }

    #[test]
    fn re_erasure_is_load_bearing_a_bare_restore_resurrects() {
        let (ix, fetcher, reindexer, holder) = cell();
        let erased = subject("u-x");
        let pn = pseudonym("u-x");
        let mut src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src.upsert("owned", 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(
            &snapshot_ref("owned"),
            &format!("page mentioning {pn} re raft"),
        );

        let mut outbox = OutboxStore::new();
        reindexer
            .reindex(&tenant(), &scope(), None, &[&src], &mut outbox, ctx_base())
            .expect("bare restore");
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "the bare restore brought the erased doc back (resurrection without re-erase)"
        );

        let ledger = SearchErasureLedger::new(tenant(), region());
        ledger.record(&erased, "2026-06-20T00:00:00Z");
        let mut outbox2 = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: scope(),
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox2,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };
        let verdict = SearchRestoreVerifyGate::new().run(&mut inputs);
        assert!(
            verdict.is_green(),
            "the gate re-erases the resurrected doc: {:?}",
            verdict.failure()
        );
        assert_eq!(verdict.artifact().unwrap().docs_resurrected_by_restore, 1);
        assert_eq!(verdict.artifact().unwrap().resurrected_docs, 0);
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            0,
            "the erased doc is purged again"
        );
    }

    #[test]
    fn re_erasure_is_idempotent_when_owner_tombstoned() {
        let (ix, fetcher, reindexer, holder) = cell();
        let erased = subject("u-gone");
        let mut src_after = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src_after.upsert("other", 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(&snapshot_ref("other"), "unrelated page about paxos");
        fetcher.remove(&snapshot_ref("owned"));

        let ledger = SearchErasureLedger::new(tenant(), region());
        ledger.record(&erased, "2026-06-20T00:00:00Z");
        let mut outbox = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src_after];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: scope(),
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };
        let verdict = SearchRestoreVerifyGate::new().run(&mut inputs);
        let a = verdict.artifact().expect("green");
        assert_eq!(
            a.docs_resurrected_by_restore, 0,
            "the owner tombstoned it - nothing resurrected"
        );
        assert_eq!(a.resurrected_docs, 0);
        assert_eq!(a.live_doc_count, 1, "only the unrelated page");
        assert_eq!(ix.live_count(&tenant(), &region()), 1);
    }

    #[test]
    fn an_unknown_owner_fails_the_gate_loud() {
        let (_ix, _f, reindexer, holder) = cell();
        let src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        let ledger = SearchErasureLedger::new(tenant(), region());
        let unknown = SnapshotScope::new("refs", "edge:all");
        let mut outbox = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: unknown,
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };
        let err = SearchRestoreVerifyGate::new()
            .run_or_fail_ci(&mut inputs)
            .expect_err("an unknown owner must fail CI");
        assert!(
            matches!(
                err,
                SearchRestoreFailure::RestoreFailed(ReindexError::Bus(_))
            ),
            "loud RestoreFailed: {err}"
        );
        assert!(err.to_string().contains("SEARCH RESTORE-VERIFY FAIL"));
    }

    #[test]
    fn live_count_snapshot_failure_fails_the_gate_loud() {
        let source = ReindexError::Index("live-count snapshot failed: disk I/O".into());
        let err = SearchRestoreVerifyGate::verification_live_count(Err(source.clone()))
            .expect_err("a failed snapshot must make restore verification RED");

        assert_eq!(err, SearchRestoreFailure::RestoreFailed(source));
        assert!(err.to_string().contains("live-count snapshot failed"));
    }

    #[test]
    fn ledger_is_pii_free_and_idempotent() {
        let ledger = SearchErasureLedger::new(tenant(), region());
        let s = subject("u-1");
        ledger.record(&s, "2026-06-20T00:00:00Z");
        ledger.record(&s, "2026-06-21T00:00:00Z");
        assert_eq!(ledger.len(), 1, "one subject, idempotent");
        let e = &ledger.entries()[0];
        assert_eq!(
            e.subject_id, "u-1",
            "keyed by the opaque principal id (PII-free)"
        );
        assert_eq!(e.erased_at, "2026-06-20T00:00:00Z", "first timestamp kept");
        assert!(ledger.is_erased("u-1"));
        assert!(!ledger.is_erased("u-2"));
    }

    #[test]
    fn ledger_records_many_subjects_in_deterministic_order() {
        let ledger = SearchErasureLedger::new(tenant(), region());
        ledger.record(&subject("u-c"), "2026-06-20T00:00:00Z");
        ledger.record(&subject("u-a"), "2026-06-20T00:00:00Z");
        ledger.record(&subject("u-b"), "2026-06-20T00:00:00Z");
        let ids: Vec<String> = ledger.entries().into_iter().map(|e| e.subject_id).collect();
        assert_eq!(
            ids,
            vec!["u-a", "u-b", "u-c"],
            "deterministic subject-sorted order"
        );
    }
}
