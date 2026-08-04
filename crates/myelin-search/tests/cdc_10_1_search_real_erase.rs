use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Region, TenantId, Timestamp, Visibility,
};
use myelin_gdpr::{EraseReceipt, EraseScope, LocateReport, PersonalDataHolder, SubjectRef};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{FieldType, FieldValue};
use myelin_storage::KmsEngine;

use myelin_search::{
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
    SearchDekPin, SearchEraseHolder, SearchProjection,
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

struct Fetcher {
    map: std::sync::Mutex<std::collections::HashMap<String, SearchProjection>>,
}
impl ProjectFetcher for Fetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.map.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

fn page_spec() -> IndexSpec {
    let mut fields = BTreeMap::new();
    fields.insert("actor".to_string(), FieldType::Principal);
    IndexSpec::new("knowledge", "page", fields).semantic()
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

fn proj(text: &str, actor_id: &str) -> SearchProjection {
    let mut f = BTreeMap::new();
    f.insert("actor".to_string(), FieldValue::Principal(actor_id.into()));
    SearchProjection {
        text: text.into(),
        fields: f,
        lang: None,
    }
}

fn provider() -> (SearchEraseHolder, Arc<IncrementalIndexer>) {
    let r = "myelin://acme/knowledge/page/cdc";
    let map: std::collections::HashMap<String, SearchProjection> =
        [(r.to_string(), proj("a page by the subject", "u-cdc"))]
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

    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    pin.reserve(&tenant(), &region()).expect("reserve");
    (SearchEraseHolder::new(ix.clone(), pin, region()), ix)
}

struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}
impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }
    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| {
                h.locate(subject, tenant.clone())
                    .expect("a Search holder locate succeeds")
            })
            .collect()
    }
    fn fan_out_erase(&self, scope: EraseScope) -> Vec<EraseReceipt> {
        self.holders
            .iter()
            .map(|h| {
                h.erase(scope.clone())
                    .expect("a Search holder erase succeeds")
            })
            .collect()
    }
}

#[test]
fn dsr_orchestrator_fans_locate_and_real_erase_out_to_the_search_holder() {
    let (holder, ix) = provider();
    let subj = subject("u-cdc");
    let consumer = DsrOrchestratorConsumer::new(vec![&holder]);

    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(
        reports.len(),
        1,
        "the Search holder responded to locate via the contract"
    );
    assert_eq!(reports[0].receipt.operation, "locate");
    assert!(
        reports[0].receipt.content_hash.starts_with("blake3:"),
        "content-addressed receipt"
    );
    assert!(
        reports[0].receipt.key_epoch_destroyed.is_none(),
        "locate shreds no key"
    );

    assert_eq!(
        ix.live_count(&tenant(), &region()),
        1,
        "the subject's doc is indexed before erase"
    );

    let receipts = consumer.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant(),
    });
    assert_eq!(
        receipts.len(),
        1,
        "the Search holder honoured the erase contract"
    );
    assert_eq!(receipts[0].receipt.operation, "erase");
    assert!(
        receipts[0].receipt.content_hash.starts_with("blake3:"),
        "content-addressed receipt"
    );
    assert!(
        receipts[0].receipt.key_epoch_destroyed.is_none(),
        "a per-subject purge shreds no key (the primary mechanism is purge + reindex, not crypto-shred)"
    );

    assert_eq!(
        ix.live_count(&tenant(), &region()),
        0,
        "the subject's doc was purged via the contract"
    );
}

#[test]
fn tenant_offboard_records_the_destroyed_key_epoch() {
    let (holder, _ix) = provider();
    let consumer = DsrOrchestratorConsumer::new(vec![&holder]);
    let receipts = consumer.fan_out_erase(EraseScope::Tenant(tenant()));
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].receipt.key_epoch_destroyed,
        Some(0),
        "the tenant-decommission shred records the destroyed key epoch"
    );
}
