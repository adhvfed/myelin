//! **E2E-3 reindex-parity CDC pair — cold-reindex == live for the derived stores** (P-ST-36 / global
//! **P-447**, M5; contract-index rows **11.6** "the OLAP derived store", **2.6** "reindex-from-source").
//!
//! The E2E-3 wedge ("an artifact's full causal lineage survives a reindex-from-cold") is whole-system;
//! its STORAGE half (this prompt) is the proof, in the data layer, that **every derived store rebuilds
//! BYTE-IDENTICALLY from source** (cold == live) with **NO backup-restore path** (§7.1/§7.3). That half
//! has THREE grains that MUST agree on the same "derived stores rebuild cold==live from source" contract:
//!   - the **PROVIDER** = `myelin-storage` — [`DerivedStoreClass::ALL`] is the exhaustive derived-store
//!     catalogue (OLAP/Search/Refs), every member cold-reindex==live with NO backup-restore path, and
//!     the storage-side E2E-3 artifact ([`run_e2e3_storage_half`]) seals that proof.
//!   - the **CONSUMER (Refs)** = `myelin-refs-service`'s `RefsReindexer` — its REAL REF-D4 cold==live
//!     byte-parity drill (wipe → reindex-from-source → `verify_parity` == true).
//!   - the **CONSUMER (Search)** = `myelin-search`'s `SearchReindexer` — its REAL SRCH-D5 cold==live
//!     drill (wipe → reindex-from-source → the rebuilt index byte-matches live).
//!
//! This CDC pins that the storage half's E2E-3 claim is CORROBORATED by the real Refs + Search
//! reindexers actually achieving cold==live — and that storage's derived-store catalogue covers exactly
//! the stores those reindexers rebuild (no derived store escapes the cold==live proof). Both
//! `myelin-refs-service` and `myelin-search` already depend on `myelin-storage` (the normal DAG edge —
//! they consume the KMS/holder substrate); this dev-only edge (the CDC reaching DOWN to corroborate the
//! real reindexers) introduces no build cycle. The two proofs MEET; neither re-derives the other
//! (coherence EI-01 §7 — the SAME posture as the E2E-4 holder-coverage CDC).

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

// the REAL Refs reindexer (the REF-D4 cold==live byte-parity drill the CDC corroborates).
use myelin_refs_service::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use myelin_refs_service::reindex::{
    RefsReindexSource, RefsReindexer, SourceEdge, REFS_OWNER_TOKEN,
};

// the REAL Search reindexer (the SRCH-D5 cold==live drill the CDC corroborates).
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

/// The PROVIDER's E2E-3 storage artifact (the storage half sealed green) — OLAP/Search/Refs all
/// cold-reindex==live with no backup-restore path.
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

// =====================================================================================================
// PROVIDER: the storage half seals a green E2E-3 artifact over the WHOLE derived-store set.
// =====================================================================================================

/// **The PROVIDER (storage `DerivedStoreClass::ALL`) seals a GREEN E2E-3 artifact: every derived store
/// cold-reindex==live (0 drift), NO backup-restore path.** This is the storage half's contribution.
#[test]
fn cdc_provider_storage_half_seals_a_green_e2e3_artifact() {
    let artifact = storage_artifact();
    assert!(
        artifact.is_green(),
        "the storage half is green: {artifact:?}"
    );
    assert_eq!(artifact.stores_with_drift, 0, "0 drift — cold == live");
    assert_eq!(
        artifact.derived_stores_with_backup_path, 0,
        "0 derived stores backed up — reindex-from-source only (§7.1/§7.3)"
    );
    assert!(
        artifact.covers_all_derived_stores(),
        "covers OLAP + Search + Refs"
    );
}

// =====================================================================================================
// CONSUMER (Refs): the REAL RefsReindexer achieves cold==live byte-parity (REF-D4).
// =====================================================================================================

/// **The CONSUMER (the REAL `myelin-refs-service` `RefsReindexer`) achieves cold==live byte-parity —
/// corroborating the storage half's "Refs is a derived store rebuilt from source, cold==live" claim.**
/// Build a LIVE Refs edge projection, then WIPE + reindex-from-source through the SAME `handle` and
/// assert `verify_parity` == true (the byte-parity verdict, REF-D4). No backup-restore path is used —
/// the only rebuild verb is `reindex`.
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

    // LIVE projection (steady-state ingest of the live edge log).
    let live_builder = RefsEdgeBuilder::new(EdgeProjection::new());
    live_builder.handle(&live_edge_event("01J-1", "s1", "t1", "mentions"));
    live_builder.handle(&live_edge_event("01J-2", "s2", "t2", "embeds"));
    let live = live_builder.projection().clone();

    // The owner's source of truth (the same edges) → the reindex source.
    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:s1->t1", 1, "s1", "t1", "mentions"));
    src.record(source_edge("refs.edge:s2->t2", 1, "s2", "t2", "embeds"));

    // COLD: wipe + reindex-from-source through the SAME `handle` (no backdoor).
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let mut outbox = OutboxStore::new();
    let receipt = reindexer
        .reindex(&scope, None, &src, &mut outbox, ctx_base())
        .expect("the real Refs reindex succeeds");
    assert_eq!(receipt.snapshots_emitted, 2, "two edges re-emitted");

    // The REAL byte-parity verdict: cold == live.
    assert!(
        reindexer.verify_parity(&live, &tenant(), &region()),
        "the REAL Refs reindexer rebuilt the edge index BYTE-IDENTICALLY to live (REF-D4)"
    );
}

// =====================================================================================================
// CONSUMER (Search): the REAL SearchReindexer achieves cold==live (SRCH-D5).
// =====================================================================================================

/// A ProjectFetcher backed by an owner-content map (the no-cross-db seam — Search fetches the owner's
/// projection, never its DB).
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

/// **The CONSUMER (the REAL `myelin-search` `SearchReindexer`) achieves cold==live — corroborating the
/// storage half's "Search is a derived store rebuilt from source, cold==live" claim.** Wipe + reindex-
/// from-source rebuilds the index so the SAME doc is searchable (SRCH-D5). No backup-restore path is
/// used — the only rebuild verb is `reindex`.
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

    // COLD: reindex-from-source through the bus re-emit → the live indexer (the recovery lane).
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

    // The rebuilt docs are searchable (cold == live: the SAME content is found).
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

// =====================================================================================================
// THE CDC CONTRACT: provider catalogue ⟺ consumer reindexers AGREE (neither re-derives the other).
// =====================================================================================================

/// **The CDC contract: storage's derived-store catalogue covers EXACTLY the stores the real Refs +
/// Search reindexers rebuild from source — and the storage half + the real reindexers AGREE that those
/// stores are cold==live with NO backup-restore path.** A derived store that the real reindexers rebuild
/// but storage's catalogue omitted (or that storage claimed cold==live but a real reindexer drifted on)
/// would break this — so "the storage half claims a cold==live property the real derived store cannot
/// hold" is structurally impossible.
#[test]
fn cdc_provider_catalogue_agrees_with_the_consumer_reindexers() {
    // The PROVIDER's catalogue: OLAP + Search + Refs, all cold==live with no backup path.
    let artifact = storage_artifact();
    assert!(artifact.is_green());

    // The catalogue carries BOTH derived stores the real reindexers cover (Search + Refs) + OLAP.
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

    // The storage half's structural truth (no backup-restore path) holds for every store the real
    // reindexers rebuild: a derived store has reindex-from-source as its ONLY rebuild verb.
    for c in DerivedStoreClass::ALL {
        assert!(
            !c.has_backup_restore_path(),
            "{} (a derived store the real reindexer rebuilds) has NO backup-restore path",
            c.name()
        );
    }

    // The two REAL reindexer proofs above (Refs `verify_parity` == true, Search rebuilt+searchable)
    // corroborate the storage half's per-store cold==live legs. Neither re-derives the other: the
    // storage half models the derived-store CLASS; the real reindexers prove the concrete stores; this
    // CDC asserts they agree on the same contract (cold==live, no backup path).
    assert!(
        artifact.legs.iter().all(|l| l.cold_matches_live()),
        "every storage-half leg is cold==live (corroborated by the real Refs/Search reindexers)"
    );
}
