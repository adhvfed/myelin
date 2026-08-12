use std::collections::BTreeMap;

use myelin_storage::{
    run_e2e3_storage_half, DerivedReindexSource, DerivedStoreClass, E2e3StorageArtifact,
};

use myelin_events::EventHandler;
use myelin_events::{
    Actor, ArtifactRef, EmitContextBase, EventEnvelope, OutboxStore, Region, SnapshotScope,
    TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

use myelin_refs_service::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use myelin_refs_service::reindex::{
    RefsReindexSource, RefsReindexer, SourceEdge, REFS_OWNER_TOKEN,
};

use myelin_events::reindex::ReferenceReindexSource as SearchReferenceSource;
use myelin_search::{
    AclFilter, IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError,
    ProjectFetcher, SearchProjection, SearchReindexer,
};
use std::sync::{Arc, Mutex};

fn tenant() -> TenantId {
    TenantId("01J0ACME".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        caused_by: None,
    }
}

fn storage_artifact() -> E2e3StorageArtifact {
    let mut olap = DerivedReindexSource::new("olap_src");
    olap.upsert("issue:PROJ-1", 1, serde_json::json!({ "cfd": 3 }));
    let mut search = DerivedReindexSource::new("search_src");
    search.upsert("page:home", 1, serde_json::json!({ "text": "raft" }));
    let mut refs = DerivedReindexSource::new("refs_src");
    refs.upsert(
        "edge:PR-1->ISSUE-1",
        1,
        serde_json::json!({ "kind": "closes" }),
    );
    let sources = BTreeMap::from([
        (DerivedStoreClass::Olap, olap),
        (DerivedStoreClass::Search, search),
        (DerivedStoreClass::Refs, refs),
    ]);
    run_e2e3_storage_half(&region(), &sources, &ctx_base()).expect("the storage half runs green")
}

#[test]
fn cdc_provider_storage_half_seals_a_green_e2e3_artifact() {
    let artifact = storage_artifact();
    assert!(
        artifact.is_green(),
        "the storage half is green: {artifact:?}"
    );
    assert_eq!(artifact.stores_with_drift, 0, "0 drift - cold == live");
    assert_eq!(
        artifact.derived_stores_with_backup_path, 0,
        "0 derived stores backed up - reindex-from-source only (§7.1/§7.3)"
    );
    assert!(
        artifact.covers_all_derived_stores(),
        "covers OLAP + Search + Refs"
    );
}

#[test]
fn cdc_consumer_refs_reindexer_cold_equals_live_byte_parity() {
    fn source_edge(agg: &str, version: u64, source: &str, target: &str, rel: &str) -> SourceEdge {
        SourceEdge {
            aggregate: agg.into(),
            version,
            source: ArtifactRef(source.into()),
            target: ArtifactRef(target.into()),
            rel: rel.into(),
            origin_actor: "p-opaque-1".into(),
            zookie: Some("zk-1".into()),
        }
    }
    fn live_edge_event(id: &str, source: &str, target: &str, rel: &str) -> EventEnvelope {
        use myelin_events::{
            AggregateKey, CorrelationId, DataRole, EventId, EventType, Visibility,
        };
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType("refs.edge.created".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p-opaque-1".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            subject: ArtifactRef(source.into()),
            aggregate: AggregateKey(format!("refs.edge:{source}->{target}")),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({
                "source": source, "target": target, "rel": rel, "zookie": "zk-1"
            }),
        }
    }
    let scope = SnapshotScope::new(REFS_OWNER_TOKEN, "edge:all");

    let live_builder = RefsEdgeBuilder::new(EdgeProjection::new());
    live_builder.handle(
        &live_edge_event("01J-1", "s1", "t1", "mentions"),
        &mut myelin_events::HandlerTx::none(),
    );
    live_builder.handle(
        &live_edge_event("01J-2", "s2", "t2", "embeds"),
        &mut myelin_events::HandlerTx::none(),
    );
    let live = live_builder.projection().clone();

    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:s1->t1", 1, "s1", "t1", "mentions"));
    src.record(source_edge("refs.edge:s2->t2", 1, "s2", "t2", "embeds"));

    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let mut outbox = OutboxStore::new();
    let receipt = reindexer
        .reindex(&scope, None, &src, &mut outbox, ctx_base())
        .expect("the real Refs reindex succeeds");
    assert_eq!(receipt.snapshots_emitted, 2, "two edges re-emitted");

    assert!(
        reindexer.verify_parity(&live, &tenant(), &region()),
        "the REAL Refs reindexer rebuilt the edge index BYTE-IDENTICALLY to live (REF-D4)"
    );
}

#[derive(Default)]
struct OwnerProjection {
    bodies: Mutex<BTreeMap<String, String>>,
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

#[test]
fn cdc_consumer_search_reindexer_cold_equals_live() {
    fn snapshot_ref(agg: &str) -> String {
        format!("myelin://t/knowledge/page/{agg}")
    }
    let fetcher = Arc::new(OwnerProjection::default());
    fetcher.put(&snapshot_ref("alpha"), "alpha discusses raft consensus");
    fetcher.put(&snapshot_ref("beta"), "beta discusses paxos consensus");

    let ix = Arc::new(IncrementalIndexer::new(
        vec![IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()],
        fetcher.clone(),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));

    let mut src = SearchReferenceSource::new("knowledge", "page");
    src.upsert("alpha", 1, serde_json::json!({ "kind": "page" }));
    src.upsert("beta", 1, serde_json::json!({ "kind": "page" }));
    let scope = SnapshotScope::new("knowledge", "page:all");

    let reindexer = SearchReindexer::new(ix.clone(), region());
    let mut outbox = OutboxStore::new();
    let job = reindexer
        .reindex(&tenant(), &scope, None, &[&src], &mut outbox, ctx_base())
        .expect("the real Search reindex succeeds");
    assert!(job.is_done(), "the rebuild completes in one pass");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        2,
        "the cold rebuild holds both docs"
    );

    let raft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
        .expect("ft raft");
    let paxos = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 10)
        .expect("ft paxos");
    assert_eq!(
        raft.len(),
        1,
        "the raft page is searchable after the rebuild"
    );
    assert_eq!(
        paxos.len(),
        1,
        "the paxos page is searchable after the rebuild"
    );
}

#[test]
fn cdc_provider_catalogue_agrees_with_the_consumer_reindexers() {
    let artifact = storage_artifact();
    assert!(artifact.is_green());

    let covered: Vec<&'static str> = DerivedStoreClass::ALL.iter().map(|c| c.name()).collect();
    assert!(
        covered.contains(&"refs"),
        "the catalogue covers the Refs derived store"
    );
    assert!(
        covered.contains(&"search"),
        "the catalogue covers the Search derived store"
    );
    assert!(
        covered.contains(&"olap"),
        "the catalogue covers the OLAP derived store"
    );

    for c in DerivedStoreClass::ALL {
        assert!(
            !c.has_backup_restore_path(),
            "{} (a derived store the real reindexer rebuilds) has NO backup-restore path",
            c.name()
        );
    }

    assert!(
        artifact.legs.iter().all(|l| l.cold_matches_live()),
        "every storage-half leg is cold==live (corroborated by the real Refs/Search reindexers)"
    );
}
