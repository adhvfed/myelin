//! # CDC — the incremental-indexer consumer seam (SRCH-P06 → P-169)
//!
//! **Architecture:** `search-and-indexing.md` §4.1 (the indexer is an ordinary `myelin-events`
//! consumer; the per-event pipeline dedup → fetch `project(ref, viewer)` (NOT the DB) → analyze →
//! embed-if-semantic → build IndexDocument → stamp indexed_zookie+version → upsert → mark dedup →
//! ack). **Contracts:** 2.1 EventEnvelope, 2.4/2.5 the consumer template + dedup ledger, 5.6
//! `project(ref, viewer)`.
//!
//! This CDC pins TWO seams from the CONSUMER side (Search consumes both):
//! - **2.4 (the consumer template):** the indexer is driven by the ONE sanctioned consumer runtime
//!   ([`myelin_events::Consumer`] — the seven encoded rules), idempotent on `event_id` via the
//!   [`myelin_events::DedupLedger`] (2.5). A redelivered event is a handler no-op (0 dup); the
//!   handler never reaches the engine on a redelivery.
//! - **5.6 (`project(ref, viewer)`):** the indexer fetches the owner's searchable projection through
//!   the [`myelin_search::ProjectFetcher`] seam — and ONLY this. There is NO owner-DB read path (the
//!   no-cross-db floor). If a fetch is transiently unavailable the handler RETRIES (0 lost, never a
//!   fabricated projection); if the artifact is GONE the doc is removed.
//!
//! The dated green artifact (2026-06-20): a synthetic producer emits a domain event; the indexer
//! consumer fetches the owner's projection (5.6, never the DB), builds + stamps the IndexDocument, and
//! the doc is searchable — and a redelivery is deduped (2.4/2.5). The mock embedding adapter +
//! synthetic producer are the named floors (real model post-M5; real IndexSpecs M3/M4).

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

/// The CONSUMER-side stand-in for the owner's 5.6 `project(ref, viewer)`: a per-tenant map of
/// `ref → projection`, with a call counter so a redelivery's dedup-skip is observable (it must NOT
/// re-fetch). A ref not in the map is GONE. **This is NOT a DB** — it is the owner's per-viewer
/// projection over the resilient client (the only sanctioned way Search reads another subsystem's
/// artifact, 5.6).
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

/// **The consumer seam (2.4/2.5 + 5.6): the indexer runs through the sanctioned runtime, fetches the
/// owner's projection (NOT the DB), indexes the doc, and a redelivery is deduped (0 dup).**
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

    // The ONE sanctioned consumer runtime (2.4) over the indexer handler; idempotent via the dedup
    // ledger (2.5). The subscription whitelists a concrete subject prefix — NEVER `*`.
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

    // First delivery: the handler fetches the owner projection (5.6) ONCE and indexes the doc.
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Acked,
        "first delivery indexes + acks"
    );
    assert_eq!(
        owner.calls(r),
        1,
        "the owner's project(ref) was fetched once (5.6 — NOT the DB)"
    );

    // The doc is searchable (the freshness property — SRCH-D7).
    let hits = indexer
        .search_ft(&tenant(), &region(), &AclFilter::ids([r]), "deadlock", 10)
        .expect("search");
    assert_eq!(hits.len(), 1, "the indexed doc is searchable");
    assert_eq!(hits[0].doc_id, r);

    // Redelivery: the dedup ledger absorbs it (2.5) — the handler is SKIPped, the owner is NOT
    // re-fetched, and the index is unchanged (0 dup).
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

/// **The 5.6 fetch is the ONLY ingest path — a transient owner hiccup RETRIES (0 lost), never a
/// fabricated projection or a silent drop.** (The consumer side of 5.6 under the resilient client.)
#[test]
fn transient_owner_unavailable_retries_never_fabricates() {
    /// An owner that is unavailable until the Nth call (the transient hiccup the resilient client
    /// surfaces).
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

    // First handle: the owner is down → Retry (NOT acked, NOT a poison, NOTHING indexed).
    assert!(
        matches!(indexer.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Retry(_)),
        "a transient hiccup retries"
    );
    assert_eq!(
        indexer.live_count(&tenant(), &region()),
        0,
        "no fabricated projection on the hiccup"
    );
    // Redelivery: the owner is back → Done, the doc indexes (0 lost).
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
