use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Region, TenantId, Timestamp, Visibility,
};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};
use myelin_query::{FieldType, FieldValue};
use myelin_storage::KmsEngine;

use myelin_search::{
    AclFilter, EmbeddingAdapter, IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, SearchDekPin,
    SearchEraseHolder, SubjectMatcher,
};

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

fn page_spec() -> IndexSpec {
    let mut fields = BTreeMap::new();
    fields.insert("actor".to_string(), FieldType::Principal);
    fields.insert("assignee".to_string(), FieldType::Principal);
    IndexSpec::new("knowledge", "page", fields).semantic()
}

struct Fetcher {
    map: std::sync::Mutex<std::collections::HashMap<String, myelin_search::SearchProjection>>,
}
impl myelin_search::ProjectFetcher for Fetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<myelin_search::SearchProjection, myelin_search::ProjectFetchError> {
        match self.map.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(myelin_search::ProjectFetchError::Gone),
        }
    }
}

fn proj(text: &str, fields: BTreeMap<String, FieldValue>) -> myelin_search::SearchProjection {
    myelin_search::SearchProjection {
        text: text.into(),
        fields,
        lang: None,
    }
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

fn actor(id: &str) -> BTreeMap<String, FieldValue> {
    let mut f = BTreeMap::new();
    f.insert("actor".to_string(), FieldValue::Principal(id.into()));
    f
}
fn assignee(id: &str) -> BTreeMap<String, FieldValue> {
    let mut f = BTreeMap::new();
    f.insert("assignee".to_string(), FieldValue::Principal(id.into()));
    f
}

#[test]
fn srch_d4_erase_leaves_zero_recoverable_including_vectors() {
    let target = subject("u-target");
    let pseudonym = PseudonymHandle::new("u-target", "acme")
        .expect("pseudonym")
        .render();

    let mut docs: Vec<(String, myelin_search::SearchProjection)> = Vec::new();
    docs.push((
        "myelin://acme/knowledge/page/t-owned".into(),
        proj(
            "u-target's design note on raft leadership and quorum",
            actor("u-target"),
        ),
    ));
    docs.push((
        "myelin://acme/knowledge/page/t-owned-2".into(),
        proj(
            "u-target's second note on log compaction",
            actor("u-target"),
        ),
    ));
    docs.push((
        "myelin://acme/knowledge/page/t-assigned".into(),
        proj(
            "a task assigned to the subject about snapshotting",
            assignee("u-target"),
        ),
    ));
    docs.push((
        "myelin://acme/knowledge/page/t-mentioned".into(),
        proj(
            &format!("ping {pseudonym} re the membership change protocol"),
            BTreeMap::new(),
        ),
    ));
    docs.push((
        "myelin://acme/knowledge/page/t-acl".into(),
        proj("a page owned directly by the subject", BTreeMap::new()),
    ));
    for i in 0..20 {
        docs.push((
            format!("myelin://acme/knowledge/page/u{i}"),
            proj(
                &format!("unrelated page {i} about paxos and consensus"),
                actor(&format!("u-{i}")),
            ),
        ));
    }

    docs.retain(|(d, _)| !d.ends_with("t-acl"));

    let map: std::collections::HashMap<String, myelin_search::SearchProjection> =
        docs.iter().cloned().collect();
    let fetcher = Arc::new(Fetcher {
        map: std::sync::Mutex::new(map),
    });
    let ix = Arc::new(IncrementalIndexer::new(
        vec![page_spec()],
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    for (d, _) in &docs {
        ix.index(&created_event(d)).expect("index");
    }
    let total = docs.len() as u64;
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        total,
        "the whole corpus indexed"
    );

    let matcher = SubjectMatcher::new("u-target", Some(pseudonym.clone()));
    let referencing = ix.locate_subject(&tenant(), &region(), &matcher);
    assert_eq!(
        referencing.len(),
        4,
        "four docs reference u-target (2 actor + assignee + pseudonym)"
    );
    let q = MockEmbeddingAdapter::new(8)
        .embed("raft leadership and quorum")
        .unwrap();
    let pre = ix
        .search_semantic(&tenant(), &region(), &AclFilter::All, &q, 30)
        .expect("semantic pre");
    assert!(
        pre.iter().any(|h| h.doc_id.ends_with("t-owned")),
        "the subject's doc is reachable by k-NN before erase"
    );

    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    pin.reserve(&tenant(), &region())
        .expect("reserve the per-tenant index DEK");
    let holder = SearchEraseHolder::new(ix.clone(), pin, region());

    let receipt = holder
        .erase(EraseScope::Subject {
            subject: target.clone(),
            tenant: tenant(),
        })
        .expect("erase the subject");
    assert_eq!(receipt.receipt.operation, "erase");
    assert!(
        receipt.receipt.content_hash.starts_with("blake3:"),
        "content-addressed receipt"
    );

    let post_located = ix.locate_subject(&tenant(), &region(), &matcher);
    assert!(
        post_located.is_empty(),
        "0 docs reference the subject after erase (purged, not hidden)"
    );
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        total - 4,
        "exactly the four referencing docs were purged; the rest survive"
    );

    let ft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "raft leadership", 30)
        .expect("ft");
    assert!(
        !ft.iter().any(|h| h.doc_id.ends_with("t-owned")),
        "the erased doc is GONE from full-text search (0 recoverable via FT)"
    );

    let post = ix
        .search_semantic(&tenant(), &region(), &AclFilter::All, &q, 30)
        .expect("semantic post");
    for d in ["t-owned", "t-owned-2", "t-assigned", "t-mentioned"] {
        assert!(
            !post.iter().any(|h| h.doc_id.ends_with(d)),
            "the erased doc {d}'s VECTOR is gone (0 recoverable via the vector/RAG path)"
        );
    }

    assert!(
        !ix.has_orphan_embedding(&tenant(), &region()),
        "0 orphan embedding after the erase compaction (embeddings purged with their source, §3.3)"
    );

    let unrelated = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 30)
        .expect("ft unrelated");
    assert_eq!(
        unrelated.len(),
        20,
        "all 20 unrelated docs survive (the erase is surgical)"
    );
}

#[test]
fn srch_d4_erase_tombstone_rides_the_live_consumer_path() {
    let r = "myelin://acme/knowledge/page/p1";
    let map: std::collections::HashMap<String, myelin_search::SearchProjection> =
        [(r.to_string(), proj("a body", BTreeMap::new()))]
            .into_iter()
            .collect();
    let fetcher = Arc::new(Fetcher {
        map: std::sync::Mutex::new(map),
    });
    let ix = Arc::new(IncrementalIndexer::new(
        vec![page_spec()],
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    ix.index(&created_event(r)).expect("index");
    assert_eq!(ix.live_count(&tenant(), &region()), 1);

    let mut erased = created_event(r);
    erased.type_ = EventType("knowledge.page.erased".into());
    erased.payload = serde_json::json!({ "ref": r });
    assert_eq!(
        ix.index(&erased),
        Ok(()),
        "the `*.erased` tombstone flows through the SAME live consumer index() path (no backdoor)"
    );
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        0,
        "the doc was purged via the live consumer path"
    );
}
