//! # CDC 6.4 — `reindex(scope) -> job` (Search), the consumer-driven-contract pair (SRCH-P16 / P-179)
//!
//! Contract-index row **6.4** (`reindex(scope) -> job (Search)`) is **OWNED** by Search. This is its
//! CDC pair — the two sides of the §4.9 reindex-from-source seam, the ONLY rebuild path (SEARCH-1):
//!
//! - **PROVIDER side** (Search's [`myelin_search::SearchReindexer::reindex`]): `reindex(scope)` drives
//!   the bus re-emit protocol (contract 2.6 CONSUMED, [`myelin_events::reindex`]) → `*.snapshot` events
//!   through the outbox → the live indexer's `index()` step. It returns a `job` (here a
//!   [`myelin_search::ReindexJob`]) — `Done` or throttled `InProgress` with a resume cursor. No "load
//!   the index from Postgres" backdoor: the rebuild re-drives the SAME live consumer step a `*.created`
//!   takes.
//! - **CONSUMER side** (the live indexer, SRCH-P06): a `*.snapshot` carries the SAME envelope shape as a
//!   live event; the indexer cannot tell cold from live, so the cold-rebuilt index == the live index
//!   (SRCH-D5). The deterministic snapshot `event_id` ([`myelin_events::snapshot_event_id`]) makes a
//!   re-run idempotent.
//!
//! The two sides agree on the FROZEN shapes: the [`myelin_events::SnapshotScope`] selector (the §4.9
//! sub-artifact-granular scope), the `*.snapshot` envelope, and the deterministic event_id. This file
//! pins both so a drift on EITHER side breaks the build/test (EI-01 §7).
//!
//! FLOOR named: the per-owner real `replay` bodies are EB-26 / per-owner M3/M4; the full-scale
//! reindex-parity (SRCH-D5 at scale, E2E-3) is SRCH-P32 (M5). This CDC is the CI-variant seam pair.

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

/// The owner's `project(ref, viewer)` (contract 5.6) — Search fetches the owner's projection per
/// `*.snapshot`, NEVER the owner DB (the no-cross-db floor). Backed by an in-memory `ref -> body`.
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
    format!("myelin://t/knowledge/page/{agg}")
}

/// **CDC 6.4 — PROVIDER+CONSUMER: `reindex(scope) -> job` drives the bus re-emit through the live
/// indexer and rebuilds the index (the §4.9 ONLY rebuild path).** The provider returns a `job`; the
/// consumer (the live indexer) ends up with every page searchable.
#[test]
fn reindex_provides_a_job_that_rebuilds_through_the_live_consumer() {
    let mut src = ReferenceReindexSource::new("knowledge", "page");
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

    // PROVIDER: the job reports Done + the totals (the 6.4 receipt body).
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

    // CONSUMER: the rebuilt docs are searchable through the ordinary FT path (cold == live).
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

/// **CDC 6.4 — the seam is the ONLY rebuild path (SEARCH-1): a reindex of an UNKNOWN owner is a LOUD
/// error, never a silent empty rebuild.** The provider bubbles the bus's `NoSourceForOwner`.
#[test]
fn reindex_of_an_unknown_owner_is_loud() {
    let src = ReferenceReindexSource::new("knowledge", "page");
    let fetcher = Arc::new(OwnerProjection::default());
    let embedder: Arc<dyn EmbeddingAdapter> = Arc::new(MockEmbeddingAdapter::new(8));
    let ix = Arc::new(IncrementalIndexer::new(
        vec![page_spec()],
        fetcher,
        embedder,
    ));
    let reindexer = SearchReindexer::new(ix, region());

    let unknown = SnapshotScope::new("refs", "edge:all"); // no `refs` source registered.
    let mut outbox = OutboxStore::new();
    let err = reindexer
        .reindex(&tenant(), &unknown, None, &[&src], &mut outbox, ctx_base())
        .expect_err("an unknown owner is a loud error");
    assert!(
        matches!(err, myelin_search::ReindexError::Bus(_)),
        "the reindex seam refuses an unknown owner loudly (never a silent empty rebuild)"
    );
}
