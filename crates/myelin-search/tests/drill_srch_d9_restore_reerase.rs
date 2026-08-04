use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_events::reindex::ReferenceReindexSource;
use myelin_events::{
    Actor, ArtifactRef, EmitContextBase, OutboxStore, Region, ReindexSource, SnapshotScope,
    TenantId, Timestamp,
};
use myelin_gdpr::SubjectRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};

use myelin_search::{
    AclFilter, IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError,
    ProjectFetcher, SearchDekPin, SearchEraseHolder, SearchErasureLedger, SearchProjection,
    SearchReindexer, SearchRestoreFailure, SearchRestoreInputs, SearchRestoreVerifyGate,
};
use myelin_storage::KmsEngine;

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
fn pseudonym(id: &str) -> String {
    PseudonymHandle::new(id, &tenant().0)
        .expect("pseudonym renders")
        .render()
}
fn snapshot_ref(agg: &str) -> String {
    format!("myelin://t/knowledge/page/{agg}")
}
fn scope() -> SnapshotScope {
    SnapshotScope::new("knowledge", "page:all")
}

#[derive(Default)]
struct Fetcher {
    bodies: std::sync::Mutex<std::collections::HashMap<String, String>>,
}
impl Fetcher {
    fn put(&self, ref_: &str, body: &str) {
        self.bodies
            .lock()
            .unwrap()
            .insert(ref_.to_string(), body.to_string());
    }
}
impl ProjectFetcher for Fetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
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
    IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()
}

#[allow(clippy::type_complexity)]
fn cell() -> (
    Arc<IncrementalIndexer>,
    Arc<Fetcher>,
    SearchReindexer,
    SearchEraseHolder,
) {
    let fetcher = Arc::new(Fetcher::default());
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

#[test]
fn srch_d9_restore_reerase_zero_resurrected_zero_mismatch() {
    let (ix, fetcher, reindexer, holder) = cell();

    let erased_ids = ["u-erased-1", "u-erased-2", "u-erased-3"];
    let ledger = SearchErasureLedger::new(tenant(), region());

    let mut owner = ReferenceReindexSource::new("knowledge", "page");
    for (i, id) in erased_ids.iter().enumerate() {
        let agg = format!("owned-{i}");
        owner.upsert(&agg, 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(
            &snapshot_ref(&agg),
            &format!("a page mentioning {} about topic {i} raft", pseudonym(id)),
        );
        ledger.record(&subject(id), "2026-06-20T00:00:00Z");
    }
    for i in 0..9 {
        let agg = format!("free-{i}");
        owner.upsert(&agg, 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(
            &snapshot_ref(&agg),
            &format!("an unrelated page {i} about paxos"),
        );
    }
    assert_eq!(ledger.len(), 3, "three subjects recorded erased (PII-free)");

    let mut outbox = OutboxStore::new();
    let srcs: &[&dyn ReindexSource] = &[&owner];
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

    let artifact = SearchRestoreVerifyGate::new()
        .run_or_fail_ci(&mut inputs)
        .expect("SRCH-D9 must GREEN - 0 resurrected, 0 mismatch");

    assert_eq!(
        artifact.re_erased_subjects, 3,
        "three ledger subjects replayed"
    );
    assert_eq!(
        artifact.docs_resurrected_by_restore, 3,
        "the restore brought the three erased subjects' docs back (then re-erased)"
    );
    assert_eq!(
        artifact.resurrected_docs, 0,
        "0 resurrected erased docs (the GATE)"
    );
    assert_eq!(
        artifact.row_doc_vector_mismatches, 0,
        "0 row↔doc↔vector mismatch (the GATE)"
    );
    assert!(
        !artifact.orphan_embeddings,
        "0 orphan embedding (the GATE, §3.3)"
    );
    assert_eq!(artifact.live_doc_count, 9, "nine survivors remain");
    assert_eq!(
        artifact.live_vector_count, 9,
        "nine live vectors - exact doc↔vector parity"
    );
    assert!(artifact.is_green());

    assert_eq!(ix.live_count(&tenant(), &region()), 9);
    assert_eq!(
        ix.live_vector_count(&tenant(), &region()),
        9,
        "no orphan / no missing vector"
    );
    let raft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 50)
        .expect("ft raft");
    assert!(
        raft.is_empty(),
        "no erased subject's page resurrected (raft was their topic)"
    );
    let paxos = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 50)
        .expect("ft paxos");
    assert_eq!(paxos.len(), 9, "all nine survivors searchable");

    let s = artifact.summary();
    assert!(s.contains("search restore-verify PASS (SRCH-D9)"));
    assert!(s.contains("re-erased 3 ledger subject"));
    println!("[P-421 SRCH-D9 GATE GREEN 2026-06-24] {s}");
}

#[test]
fn srch_d9_restore_failure_is_loud() {
    let (_ix, _f, reindexer, holder) = cell();
    let src = ReferenceReindexSource::new("knowledge", "page");
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
        .expect_err("a restore that cannot reach a consistent point MUST fail CI");
    assert!(matches!(err, SearchRestoreFailure::RestoreFailed(_)));
    assert!(err.to_string().contains("SEARCH RESTORE-VERIFY FAIL"));
}
