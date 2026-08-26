use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use myelin_events::reindex::{reindex as bus_reindex, ReindexError as BusReindexError};
use myelin_events::{
    EmitContextBase, OutboxStore, ReindexReceipt as BusReindexReceipt, ReindexSource, SnapshotScope,
};
use myelin_tenancy::{Region, TenantId};

use crate::indexer::{IncrementalIndexer, IndexEventError};

pub const DEFAULT_BATCH_CAP: usize = 1024;

pub const DEFAULT_MAX_IN_FLIGHT_PER_TENANT: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CursorKey {
    tenant: String,
    region: String,
    scope: String,
}

impl CursorKey {
    fn new(tenant: &TenantId, region: &Region, scope: &SnapshotScope) -> CursorKey {
        CursorKey {
            tenant: tenant.0.clone(),
            region: region.0.clone(),
            scope: scope.as_key(),
        }
    }
}

#[derive(Clone)]
pub struct ReindexCursorStore {
    inner: Arc<Mutex<CursorInner>>,
    batch_cap: usize,
    max_in_flight_per_tenant: usize,
}

#[derive(Default)]
struct CursorInner {
    cursors: BTreeMap<CursorKey, u64>,
    applied: BTreeMap<CursorKey, BTreeSet<String>>,
    in_flight: BTreeMap<String, usize>,
}

impl Default for ReindexCursorStore {
    fn default() -> ReindexCursorStore {
        ReindexCursorStore::new()
    }
}

impl ReindexCursorStore {
    pub fn new() -> ReindexCursorStore {
        ReindexCursorStore::with_budget(DEFAULT_BATCH_CAP, DEFAULT_MAX_IN_FLIGHT_PER_TENANT)
    }

    pub fn with_budget(batch_cap: usize, max_in_flight_per_tenant: usize) -> ReindexCursorStore {
        ReindexCursorStore {
            inner: Arc::new(Mutex::new(CursorInner::default())),
            batch_cap: batch_cap.max(1),
            max_in_flight_per_tenant: max_in_flight_per_tenant.max(1),
        }
    }

    pub fn batch_cap(&self) -> usize {
        self.batch_cap
    }

    pub fn max_in_flight_per_tenant(&self) -> usize {
        self.max_in_flight_per_tenant
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CursorInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn cursor(&self, tenant: &TenantId, region: &Region, scope: &SnapshotScope) -> Option<u64> {
        self.lock()
            .cursors
            .get(&CursorKey::new(tenant, region, scope))
            .copied()
    }

    pub fn is_applied(
        &self,
        tenant: &TenantId,
        region: &Region,
        scope: &SnapshotScope,
        event_id: &str,
    ) -> bool {
        self.lock()
            .applied
            .get(&CursorKey::new(tenant, region, scope))
            .is_some_and(|set| set.contains(event_id))
    }

    pub fn try_acquire(&self, tenant: &TenantId) -> bool {
        let cap = self.max_in_flight_per_tenant;
        let mut g = self.lock();
        let n = g.in_flight.entry(tenant.0.clone()).or_insert(0);
        if *n >= cap {
            false
        } else {
            *n += 1;
            true
        }
    }

    pub fn release(&self, tenant: &TenantId) {
        let mut g = self.lock();
        if let Some(n) = g.in_flight.get_mut(&tenant.0) {
            *n = n.saturating_sub(1);
        }
    }

    pub fn in_flight(&self, tenant: &TenantId) -> usize {
        self.lock().in_flight.get(&tenant.0).copied().unwrap_or(0)
    }

    pub fn reset_scope(&self, tenant: &TenantId, region: &Region, scope: &SnapshotScope) {
        let key = CursorKey::new(tenant, region, scope);
        let mut g = self.lock();
        g.cursors.remove(&key);
        g.applied.remove(&key);
    }

    fn record_applied(
        &self,
        tenant: &TenantId,
        region: &Region,
        scope: &SnapshotScope,
        event_id: &str,
        version: u64,
    ) -> bool {
        let key = CursorKey::new(tenant, region, scope);
        let mut g = self.lock();
        let fresh = g
            .applied
            .entry(key.clone())
            .or_default()
            .insert(event_id.to_string());
        if fresh {
            let cur = g.cursors.entry(key).or_insert(0);
            *cur = (*cur).max(version);
        }
        fresh
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexError {
    Bus(String),
    Index(String),
    AtCapacity(String),
}

impl std::fmt::Display for ReindexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReindexError::Bus(e) => write!(f, "reindex: bus re-emit failed: {e}"),
            ReindexError::Index(e) => write!(f, "reindex: live indexer rejected a snapshot: {e}"),
            ReindexError::AtCapacity(e) => {
                write!(f, "reindex: per-tenant in-flight cap reached: {e}")
            }
        }
    }
}

impl std::error::Error for ReindexError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexJob {
    Done(ReindexProgress),
    InProgress {
        progress: ReindexProgress,
        resume_since: u64,
    },
}

impl ReindexJob {
    pub fn progress(&self) -> &ReindexProgress {
        match self {
            ReindexJob::Done(p) => p,
            ReindexJob::InProgress { progress, .. } => progress,
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self, ReindexJob::Done(_))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReindexProgress {
    pub snapshots_emitted: usize,
    pub snapshots_skipped_duplicate: usize,
    pub docs_indexed: usize,
    pub docs_skipped_applied: usize,
    pub owners_replayed: Vec<String>,
}

#[derive(Clone)]
pub struct SearchReindexer {
    indexer: Arc<IncrementalIndexer>,
    cursors: ReindexCursorStore,
    region: Region,
}

impl SearchReindexer {
    pub fn new(indexer: Arc<IncrementalIndexer>, region: Region) -> SearchReindexer {
        SearchReindexer {
            indexer,
            cursors: ReindexCursorStore::new(),
            region,
        }
    }

    pub fn with_cursors(
        indexer: Arc<IncrementalIndexer>,
        cursors: ReindexCursorStore,
        region: Region,
    ) -> SearchReindexer {
        SearchReindexer {
            indexer,
            cursors,
            region,
        }
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn cursors(&self) -> &ReindexCursorStore {
        &self.cursors
    }

    pub fn indexer_live_count(&self, tenant: &TenantId, region: &Region) -> u64 {
        self.indexer.live_count(tenant, region)
    }

    pub fn try_indexer_live_count(
        &self,
        tenant: &TenantId,
        region: &Region,
    ) -> Result<u64, ReindexError> {
        self.indexer
            .try_live_count(tenant, region)
            .map_err(|e| ReindexError::Index(format!("live-count snapshot failed: {e:?}")))
    }

    pub fn indexer_live_vector_count(&self, tenant: &TenantId, region: &Region) -> usize {
        self.indexer.live_vector_count(tenant, region)
    }

    pub fn indexer_has_orphan_embedding(&self, tenant: &TenantId, region: &Region) -> bool {
        self.indexer.has_orphan_embedding(tenant, region)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reindex(
        &self,
        tenant: &TenantId,
        scope: &SnapshotScope,
        since: Option<u64>,
        sources: &[&dyn ReindexSource],
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReindexJob, ReindexError> {
        if !self.cursors.try_acquire(tenant) {
            return Err(ReindexError::AtCapacity(format!(
                "tenant `{}` already has {} reindex job(s) in flight (cap {})",
                tenant.0,
                self.cursors.in_flight(tenant),
                self.cursors.max_in_flight_per_tenant()
            )));
        }
        let result = self.reindex_inner(tenant, scope, since, sources, outbox, ctx_base);
        self.cursors.release(tenant);
        result
    }

    fn reindex_inner(
        &self,
        tenant: &TenantId,
        scope: &SnapshotScope,
        since: Option<u64>,
        sources: &[&dyn ReindexSource],
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReindexJob, ReindexError> {
        let region = self.region.clone();

        if since.is_none() {
            self.indexer.wipe(tenant, &region);
            self.cursors.reset_scope(tenant, &region, scope);
        }

        let BusReindexReceipt {
            snapshots_emitted,
            snapshots_skipped_duplicate,
            owners_replayed,
        } = bus_reindex(scope, since, sources, outbox, ctx_base).map_err(map_bus_err)?;

        let mut progress = ReindexProgress {
            snapshots_emitted,
            snapshots_skipped_duplicate,
            owners_replayed,
            ..Default::default()
        };
        let mut highest_applied = since.unwrap_or(0);
        let mut hit_cap = false;

        for source in sources {
            if source.owner_token() != scope.owner {
                continue;
            }
            for draft in source.replay(scope, since) {
                if progress.docs_indexed >= self.cursors.batch_cap() {
                    hit_cap = true;
                    break;
                }
                let event_id = draft.event_id(tenant);
                if !self
                    .cursors
                    .record_applied(tenant, &region, scope, &event_id.0, draft.version)
                {
                    progress.docs_skipped_applied += 1;
                    continue;
                }
                let row = outbox
                    .try_row(&event_id)
                    .map_err(|error| ReindexError::Bus(error.0))?
                    .ok_or_else(|| {
                        ReindexError::Bus(format!(
                            "snapshot {} not found in the outbox (the bus re-emit did not stage it)",
                            event_id.0
                        ))
                    })?;
                self.indexer.index(&row.envelope).map_err(map_index_err)?;
                progress.docs_indexed += 1;
                highest_applied = highest_applied.max(draft.version);
            }
            if hit_cap {
                break;
            }
        }

        if hit_cap {
            Ok(ReindexJob::InProgress {
                progress,
                resume_since: highest_applied,
            })
        } else {
            Ok(ReindexJob::Done(progress))
        }
    }
}

fn map_bus_err(e: BusReindexError) -> ReindexError {
    ReindexError::Bus(e.to_string())
}

fn map_index_err(e: IndexEventError) -> ReindexError {
    match e {
        IndexEventError::Malformed(w)
        | IndexEventError::Engine(w)
        | IndexEventError::Transient(w) => ReindexError::Index(w),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AclFilter;
    use crate::indexer::{
        IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher, SearchProjection,
    };
    use myelin_events::reindex::ReferenceReindexSource;
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId,
        EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    const REGION: &str = "fr-par";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region(REGION.into())
    }
    fn principal() -> Principal {
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
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            caused_by: None,
        }
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

    fn indexer_with(bodies: &[(&str, &str)]) -> (Arc<IncrementalIndexer>, Arc<OwnerProjection>) {
        let fetcher = Arc::new(OwnerProjection::default());
        for (r, b) in bodies {
            fetcher.put(r, b);
        }
        let ix = Arc::new(IncrementalIndexer::new(
            vec![page_spec()],
            fetcher.clone(),
            Arc::new(MockEmbeddingAdapter::new(8)),
        ));
        (ix, fetcher)
    }

    fn snapshot_ref(agg: &str) -> String {
        format!("myelin://acme/knowledge/page/{agg}")
    }

    fn owner_with_three_pages() -> ReferenceReindexSource {
        let mut src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src.upsert("home", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("guide", 2, serde_json::json!({ "kind": "page" }));
        src.upsert("faq", 1, serde_json::json!({ "kind": "page" }));
        src
    }

    fn scope() -> SnapshotScope {
        SnapshotScope::new("knowledge", "page:all")
    }

    #[test]
    fn reindex_rebuilds_the_index_from_the_bus_re_emit_through_the_live_indexer() {
        let src = owner_with_three_pages();
        let (ix, fetcher) = indexer_with(&[]);
        fetcher.put(&snapshot_ref("home"), "the home page about raft");
        fetcher.put(&snapshot_ref("guide"), "a guide about paxos");
        fetcher.put(&snapshot_ref("faq"), "frequently asked questions");

        let reindexer = SearchReindexer::new(ix.clone(), region());
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[&src];

        let job = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("reindex");

        assert!(
            job.is_done(),
            "the full rebuild completes in one pass (under the batch cap)"
        );
        let p = job.progress();
        assert_eq!(
            p.snapshots_emitted, 3,
            "three pages re-emitted as *.snapshot via the bus"
        );
        assert_eq!(
            p.docs_indexed, 3,
            "all three driven through the LIVE indexer"
        );
        assert_eq!(p.owners_replayed, vec!["knowledge".to_string()]);
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            3,
            "the index holds the three rebuilt docs"
        );

        let hits = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("ft");
        assert_eq!(hits.len(), 1, "the rebuilt home page is searchable");
    }

    #[test]
    fn reindex_is_idempotent_on_the_deterministic_snapshot_event_id() {
        let src = owner_with_three_pages();
        let (ix, fetcher) = indexer_with(&[]);
        for agg in ["home", "guide", "faq"] {
            fetcher.put(&snapshot_ref(agg), "body");
        }
        let cursors = ReindexCursorStore::new();
        let reindexer = SearchReindexer::with_cursors(ix.clone(), cursors, region());
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = OutboxStore::new();

        let first = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("first");
        assert_eq!(
            first.progress().snapshots_emitted,
            3,
            "first run emits three snapshots"
        );
        assert_eq!(first.progress().docs_indexed, 3, "first pass indexes three");

        let second = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("second");
        assert_eq!(
            second.progress().snapshots_emitted,
            0,
            "0 NEW snapshots emitted (deterministic id)"
        );
        assert_eq!(
            second.progress().snapshots_skipped_duplicate,
            3,
            "all three skipped at the bus re-emit"
        );
        assert_eq!(
            second.progress().docs_indexed,
            3,
            "the cold rebuild re-applies the three (over a wipe)"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            3,
            "still exactly three docs - idempotent in effect"
        );
    }

    #[test]
    fn chained_index_erase_reindex_does_not_resurrect_the_erased_subject() {
        use crate::dek::SearchDekPin;
        use crate::engine::SubjectMatcher;
        use crate::erase::SearchEraseHolder;
        use myelin_gdpr::SubjectRef;
        use myelin_identity::PseudonymHandle;
        use myelin_storage::KmsEngine;

        let erased = SubjectRef::new(Principal::stub(
            PrincipalId("u-42".into()),
            PrincipalKind::Human,
            tenant(),
        ));
        let pseudonym = PseudonymHandle::new(&erased.principal.principal_id.0, &tenant().0)
            .expect("pseudonym renders")
            .render();
        let owned_ref = snapshot_ref("owned");
        let other_ref = snapshot_ref("other");

        let (ix, fetcher) = indexer_with(&[]);
        fetcher.put(
            &owned_ref,
            &format!("a page mentioning {pseudonym} about raft"),
        );
        fetcher.put(&other_ref, "an unrelated page about paxos");

        let mut src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src.upsert("owned", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("other", 1, serde_json::json!({ "kind": "page" }));

        let reindexer = SearchReindexer::new(ix.clone(), region());
        let mut outbox = OutboxStore::new();
        reindexer
            .reindex(&tenant(), &scope(), None, &[&src], &mut outbox, ctx_base())
            .expect("initial index");
        assert_eq!(ix.live_count(&tenant(), &region()), 2, "both pages indexed");

        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        pin.reserve(&tenant(), &region())
            .expect("reserve the index DEK");
        let holder = SearchEraseHolder::new(ix.clone(), pin, region());
        let outcome = holder.erase_subject(&erased, &tenant()).expect("erase");
        assert_eq!(outcome.docs_purged, 1, "the subject's page is purged");
        assert!(
            outcome.zero_orphan_embedding,
            "0 orphan embedding after the erase"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "only the unrelated page remains"
        );

        let mut src_after = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src_after.upsert("other", 1, serde_json::json!({ "kind": "page" }));
        fetcher.bodies.lock().unwrap().remove(&owned_ref);

        let mut outbox2 = OutboxStore::new();
        let job = reindexer
            .reindex(
                &tenant(),
                &scope(),
                None,
                &[&src_after],
                &mut outbox2,
                ctx_base(),
            )
            .expect("reindex after erase");
        assert!(job.is_done());
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "the rebuilt index holds only the unrelated page"
        );
        let raft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("ft raft");
        assert!(
            raft.is_empty(),
            "the erased subject's page is NOT resurrected by the reindex (X-7)"
        );

        let matcher = SubjectMatcher::new(
            erased.principal.principal_id.0.clone(),
            Some(pseudonym.clone()),
        );
        let located = ix.locate_subject(&tenant(), &region(), &matcher);
        assert!(
            located.is_empty(),
            "the erased subject references 0 docs after the reindex"
        );
        let re = holder.erase_subject(&erased, &tenant()).expect("re-erase");
        assert_eq!(
            re.docs_purged, 0,
            "re-erasure after the reindex purges nothing (no resurrection)"
        );
        assert!(re.zero_orphan_embedding, "still 0 orphan embedding");
    }

    #[test]
    fn reindex_is_throttled_and_resumable_via_the_cursor_store() {
        let mut src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src.upsert("p1", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("p2", 2, serde_json::json!({ "kind": "page" }));
        src.upsert("p3", 3, serde_json::json!({ "kind": "page" }));
        src.upsert("p4", 4, serde_json::json!({ "kind": "page" }));
        let (ix, fetcher) = indexer_with(&[]);
        for agg in ["p1", "p2", "p3", "p4"] {
            fetcher.put(&snapshot_ref(agg), "body");
        }
        let cursors = ReindexCursorStore::with_budget(2, DEFAULT_MAX_IN_FLIGHT_PER_TENANT);
        let reindexer = SearchReindexer::with_cursors(ix.clone(), cursors, region());
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = OutboxStore::new();

        let p1 = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("pass 1");
        match p1 {
            ReindexJob::InProgress {
                progress,
                resume_since,
            } => {
                assert_eq!(
                    progress.docs_indexed, 2,
                    "the cap stops the pass at two docs"
                );
                assert_eq!(
                    resume_since, 2,
                    "the resume cursor is the high-water version applied"
                );
            }
            ReindexJob::Done(_) => panic!("the capped pass must NOT report Done - more remain"),
        }
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            2,
            "only two docs applied so far"
        );

        let p2 = reindexer
            .reindex(
                &tenant(),
                &scope(),
                Some(2),
                sources,
                &mut outbox,
                ctx_base(),
            )
            .expect("pass 2");
        assert!(p2.is_done(), "the resumed pass finishes the rebuild");
        assert_eq!(
            p2.progress().docs_indexed,
            2,
            "the remaining two docs applied"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            4,
            "all four docs rebuilt across the two batches"
        );
    }

    #[test]
    fn incremental_backfill_appends_without_wiping() {
        let mut src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src.upsert("old", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("new", 5, serde_json::json!({ "kind": "page" }));
        let (ix, fetcher) = indexer_with(&[]);
        fetcher.put(&snapshot_ref("old"), "old body");
        fetcher.put(&snapshot_ref("new"), "new body");

        let reindexer = SearchReindexer::new(ix.clone(), region());
        let mut outbox = OutboxStore::new();
        let only_old = {
            let mut s = ReferenceReindexSource::new(tenant(), "knowledge", "page");
            s.upsert("old", 1, serde_json::json!({ "kind": "page" }));
            s
        };
        reindexer
            .reindex(
                &tenant(),
                &scope(),
                None,
                &[&only_old],
                &mut outbox,
                ctx_base(),
            )
            .expect("seed old");
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "the old doc is indexed"
        );

        let mut outbox2 = OutboxStore::new();
        let job = reindexer
            .reindex(
                &tenant(),
                &scope(),
                Some(1),
                &[&src],
                &mut outbox2,
                ctx_base(),
            )
            .expect("backfill");
        assert!(job.is_done());
        assert_eq!(
            job.progress().docs_indexed,
            1,
            "only the new page replays past since=1"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            2,
            "the backfill APPENDED - the old doc survives"
        );
    }

    #[test]
    fn per_tenant_in_flight_cap_refuses_an_over_cap_reindex() {
        let src = owner_with_three_pages();
        let (ix, _f) = indexer_with(&[]);
        let cursors = ReindexCursorStore::with_budget(DEFAULT_BATCH_CAP, 1);
        let reindexer = SearchReindexer::with_cursors(ix, cursors.clone(), region());
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = OutboxStore::new();

        assert!(cursors.try_acquire(&tenant()), "the first slot acquires");
        assert_eq!(cursors.in_flight(&tenant()), 1);

        let err = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect_err("an over-cap reindex is refused");
        assert!(
            matches!(err, ReindexError::AtCapacity(_)),
            "the per-tenant cap sheds the storm"
        );

        cursors.release(&tenant());
        assert_eq!(cursors.in_flight(&tenant()), 0);
        let job = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("reindex succeeds once a slot frees");
        assert!(job.is_done());
        assert_eq!(
            cursors.in_flight(&tenant()),
            0,
            "the reindex released its in-flight slot"
        );
    }

    #[test]
    fn reindex_of_unknown_owner_is_a_loud_error() {
        let src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        let (ix, _f) = indexer_with(&[]);
        let reindexer = SearchReindexer::new(ix, region());
        let unknown = SnapshotScope::new("refs", "edge:all");
        let mut outbox = OutboxStore::new();
        let err = reindexer
            .reindex(&tenant(), &unknown, None, &[&src], &mut outbox, ctx_base())
            .expect_err("unknown owner");
        assert!(
            matches!(err, ReindexError::Bus(_)),
            "an unknown owner is a loud Bus error"
        );
    }

    fn created_event(doc: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("ev:{doc}")),
            type_: EventType("knowledge.page.created".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: ArtifactRef(doc.into()),
            aggregate: AggregateKey(format!("agg:{doc}")),
            causation_id: None,
            correlation_id: CorrelationId(doc.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn srch_d5_cold_equals_live_ci_variant() {
        let mut src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        src.upsert("alpha", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("beta", 1, serde_json::json!({ "kind": "page" }));
        let (ix, fetcher) = indexer_with(&[]);
        fetcher.put(&snapshot_ref("alpha"), "alpha discusses raft consensus");
        fetcher.put(&snapshot_ref("beta"), "beta discusses paxos consensus");

        ix.index(&created_event(&snapshot_ref("alpha")))
            .expect("live alpha");
        ix.index(&created_event(&snapshot_ref("beta")))
            .expect("live beta");
        let live_count = ix.live_count(&tenant(), &region());
        let live_raft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("live raft");
        let live_paxos = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 10)
            .expect("live paxos");
        assert_eq!(live_count, 2, "the live index holds both pages");
        assert_eq!(live_raft.len(), 1);
        assert_eq!(live_paxos.len(), 1);

        let reindexer = SearchReindexer::new(ix.clone(), region());
        let mut outbox = OutboxStore::new();
        let job = reindexer
            .reindex(&tenant(), &scope(), None, &[&src], &mut outbox, ctx_base())
            .expect("reindex cold");
        assert!(job.is_done());

        let cold_count = ix.live_count(&tenant(), &region());
        let cold_raft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("cold raft");
        let cold_paxos = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 10)
            .expect("cold paxos");
        assert_eq!(
            cold_count, live_count,
            "cold-rebuilt doc count == live (SRCH-D5)"
        );
        assert_eq!(
            cold_raft.len(),
            live_raft.len(),
            "the raft page is searchable in the cold rebuild"
        );
        assert_eq!(
            cold_paxos.len(),
            live_paxos.len(),
            "the paxos page is searchable in the cold rebuild"
        );
        assert_eq!(
            cold_raft.first().map(|h| h.doc_id.clone()),
            live_raft.first().map(|h| h.doc_id.clone()),
            "the SAME doc id ranks for the SAME query (cold == live, not just same count)"
        );
    }
}
