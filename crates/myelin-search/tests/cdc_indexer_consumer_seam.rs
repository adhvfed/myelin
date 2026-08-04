use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{
    Actor, AggregateKey, Consumer, ConsumerName, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventHandler, EventId, EventType, HandleOutcome, Message, PrefetchBound,
    Subscription, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{FieldType, FieldValue};
use myelin_search::engine::AclFilter;
use myelin_search::{
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
    SearchProjection, INDEXER_CONSUMER,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

#[derive(Default)]
struct OwnerProjectStandIn {
    projections: Mutex<BTreeMap<String, SearchProjection>>,
    calls: Mutex<BTreeMap<String, u32>>,
}
impl OwnerProjectStandIn {
    fn put(&self, ref_: &str, text: &str, fields: BTreeMap<String, FieldValue>) {
        self.projections.lock().unwrap().insert(
            ref_.to_string(),
            SearchProjection {
                text: text.into(),
                fields,
                lang: None,
            },
        );
    }
    fn calls(&self, ref_: &str) -> u32 {
        self.calls.lock().unwrap().get(ref_).copied().unwrap_or(0)
    }
}
impl ProjectFetcher for OwnerProjectStandIn {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        *self
            .calls
            .lock()
            .unwrap()
            .entry(ref_.0.clone())
            .or_insert(0) += 1;
        self.projections
            .lock()
            .unwrap()
            .get(&ref_.0)
            .cloned()
            .ok_or(ProjectFetchError::Gone)
    }
}

fn issue_spec() -> IndexSpec {
    let mut f = BTreeMap::new();
    f.insert("status".to_string(), FieldType::Select);
    IndexSpec::new("issue", "issue", f)
}

fn event(id: &str, type_: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(format!("agg:{subject}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "zookie": "zk-1", "version": 1 }),
    }
}

#[test]
fn indexer_consumes_via_2_4_and_fetches_via_5_6() {
    let r = "myelin://acme/issue/issue/ENG-1";
    let owner = Arc::new(OwnerProjectStandIn::default());
    let mut fields = BTreeMap::new();
    fields.insert("status".to_string(), FieldValue::Select("open".into()));
    owner.put(r, "deadlock in the scheduler", fields);

    let indexer = IncrementalIndexer::new(
        vec![issue_spec()],
        owner.clone(),
        Arc::new(MockEmbeddingAdapter::new(8)),
    );

    let sub = Subscription::bind(
        ConsumerName(INDEXER_CONSUMER.into()),
        &["myelin://acme/issue/"],
        PrefetchBound::DEFAULT,
    )
    .expect("a concrete subject is admitted");
    let consumer = Consumer::new(indexer.clone(), sub, DedupLedger::new());

    let ev = event("01J-1", "issue.issue.created", r);
    let msg = Message {
        subject: r.to_string(),
        envelope: ev,
    };

    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Acked,
        "first delivery indexes + acks"
    );
    assert_eq!(
        owner.calls(r),
        1,
        "the owner's project(ref) was fetched once (5.6 - NOT the DB)"
    );

    let hits = indexer
        .search_ft(&tenant(), &region(), &AclFilter::ids([r]), "deadlock", 10)
        .expect("search");
    assert_eq!(hits.len(), 1, "the indexed doc is searchable");
    assert_eq!(hits[0].doc_id, r);

    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Deduplicated,
        "redelivery is deduped (0 dup)"
    );
    assert_eq!(
        owner.calls(r),
        1,
        "the deduped redelivery never re-fetched the owner"
    );
    assert_eq!(
        indexer.live_count(&tenant(), &region()),
        1,
        "still exactly one doc"
    );
}

#[test]
fn transient_owner_unavailable_retries_never_fabricates() {
    struct FlakyOwner {
        fail_remaining: Mutex<u32>,
        text: String,
    }
    impl ProjectFetcher for FlakyOwner {
        fn project(
            &self,
            _t: &TenantId,
            _r: &Region,
            _ref: &ArtifactRef,
        ) -> Result<SearchProjection, ProjectFetchError> {
            let mut n = self.fail_remaining.lock().unwrap();
            if *n > 0 {
                *n -= 1;
                return Err(ProjectFetchError::Unavailable("owner down".into()));
            }
            Ok(SearchProjection {
                text: self.text.clone(),
                fields: BTreeMap::new(),
                lang: None,
            })
        }
    }

    let r = "myelin://acme/issue/issue/ENG-9";
    let owner = Arc::new(FlakyOwner {
        fail_remaining: Mutex::new(1),
        text: "body".into(),
    });
    let indexer = IncrementalIndexer::new(
        vec![issue_spec()],
        owner,
        Arc::new(MockEmbeddingAdapter::new(8)),
    );
    let ev = event("01J-9", "issue.issue.created", r);

    assert!(
        matches!(indexer.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Retry(_)),
        "a transient hiccup retries"
    );
    assert_eq!(
        indexer.live_count(&tenant(), &region()),
        0,
        "no fabricated projection on the hiccup"
    );
    assert_eq!(
        indexer.handle(&ev, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done,
        "the redelivery succeeds"
    );
    assert_eq!(
        indexer.live_count(&tenant(), &region()),
        1,
        "0 lost: indexed on the redelivery"
    );
}
