use std::sync::Arc;

use myelin_events::reindex::ReferenceReindexSource;
use myelin_events::{
    Actor, EmitContextBase, OutboxStore, Region, ReindexSource, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::FieldType;
use myelin_search::{
    AclFilter, EmbeddingAdapter, IncrementalIndexer, IndexSpec, MockEmbeddingAdapter,
    ProjectFetchError, ProjectFetcher, ReindexJob, SearchProjection, SearchReindexer,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
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
    bodies: Mutex<HashMap<String, String>>,
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
        ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.bodies.lock().unwrap().get(&ref_.0) {
            Some(b) => Ok(SearchProjection {
                text: b.clone(),
                fields: BTreeMap::new(),
                lang: None,
            }),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

fn page_spec() -> IndexSpec {
    IndexSpec::new("knowledge", "page", BTreeMap::<String, FieldType>::new()).semantic()
}

fn snapshot_ref(agg: &str) -> String {
    format!("myelin://acme/knowledge/page/{agg}")
}

#[test]
fn reindex_provides_a_job_that_rebuilds_through_the_live_consumer() {
    let mut src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
    src.upsert("home", 1, serde_json::json!({ "kind": "page" }));
    src.upsert("guide", 1, serde_json::json!({ "kind": "page" }));

    let fetcher = Arc::new(OwnerProjection::default());
    fetcher.put(&snapshot_ref("home"), "home page about raft consensus");
    fetcher.put(&snapshot_ref("guide"), "a guide about paxos");

    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let ix = Arc::new(IncrementalIndexer::new(
        vec![page_spec()],
        fetcher,
        embedder,
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region());

    let scope = SnapshotScope::new("knowledge", "page:all");
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];

    let job = reindexer
        .reindex(&tenant(), &scope, None, sources, &mut outbox, ctx_base())
        .expect("reindex returns a job");

    assert!(
        matches!(job, ReindexJob::Done(_)),
        "the rebuild completes (under the batch cap)"
    );
    assert_eq!(
        job.progress().snapshots_emitted,
        2,
        "two pages re-emitted via the bus (2.6)"
    );
    assert_eq!(
        job.progress().docs_indexed,
        2,
        "both driven through the LIVE indexer"
    );

    assert_eq!(ix.live_count(&tenant(), &region()), 2);
    let raft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
        .expect("ft");
    assert_eq!(
        raft.len(),
        1,
        "the rebuilt home page is searchable (cold == live)"
    );
}

#[test]
fn reindex_of_an_unknown_owner_is_loud() {
    let src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
    let fetcher = Arc::new(OwnerProjection::default());
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let ix = Arc::new(IncrementalIndexer::new(
        vec![page_spec()],
        fetcher,
        embedder,
    ));
    let reindexer = SearchReindexer::new(ix, region());

    let unknown = SnapshotScope::new("refs", "edge:all");
    let mut outbox = OutboxStore::new();
    let err = reindexer
        .reindex(&tenant(), &unknown, None, &[&src], &mut outbox, ctx_base())
        .expect_err("an unknown owner is a loud error");
    assert!(
        matches!(err, myelin_search::ReindexError::Bus(_)),
        "the reindex seam refuses an unknown owner loudly (never a silent empty rebuild)"
    );
}
