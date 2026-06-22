//! # CDC 10.1 (real erase) — the Search side of `PersonalDataHolder{locate, erase, restrict}` over a
//! LIVE index (SRCH-P15 → P-178)
//!
//! **Contract:** index row 10.1 (`PersonalDataHolder` — the five DSR operations; the **real erase** —
//! purge / crypto-shred / pseudonymise, never hide; `restrict` suppression). The signature was frozen
//! at P-GA-01; the SRCH-P02 STUB CDC pair (`cdc_10_1_search_holder.rs`) proved the empty-surface
//! holder. THIS file ships the **real-erase** Search side: the holder ([`SearchEraseHolder`])
//! IMPLEMENTING 10.1 over a live per-tenant index, and a DSR-orchestrator stand-in (the CONSUMER)
//! that fans `locate` + `erase` out to it via the contract and never reaches into the store.
//!
//! - **PROVIDER** = [`SearchEraseHolder`] (H7) implementing the five-operation 10.1 contract for real:
//!   `locate` reports the docs referencing the subject; `erase` PURGES them (+ their vectors) via the
//!   live consumer path and returns a receipt; `restrict` suppresses.
//! - **CONSUMER** = a DSR-orchestrator shape that holds the holder behind `dyn PersonalDataHolder`,
//!   fans `locate` then `erase` out, and asserts the contract is honoured — the shape the real
//!   orchestrator (P-GA-11/P-GA-12) takes when it fans a DSR out to the Search holder.
//!
//! The dated green artifact: the consumer fans `locate(subject)` → a content-addressed receipt over
//! the located set → `erase(subject)` → a content-addressed receipt recording the purge (0 recoverable
//! incl. vectors). If 10.1's body shape drifts, this stops compiling/passing — that is the contract.

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

/// Build the PROVIDER: a [`SearchEraseHolder`] over a live index holding a doc authored by `u-cdc`.
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

/// **The CONSUMER side (10.1): a DSR-orchestrator shape that fans out to the Search holder via the
/// contract.** Holds the holder behind `dyn PersonalDataHolder`; calls the contract — never reaches
/// into the store. The property pinned: the orchestrator touches the Search store ONLY through the
/// holder contract.
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

/// **provider + consumer wired together (the 10.1 Search real-erase CDC pair).** The orchestrator
/// (consumer) fans `locate` then `erase` out to the H7 index holder (provider) over a live index; the
/// holder reports the located set, then PURGES it (the doc is gone after — not hidden), returning
/// content-addressed receipts. This is the dated green artifact for the Search side of 10.1 (real erase).
#[test]
fn dsr_orchestrator_fans_locate_and_real_erase_out_to_the_search_holder() {
    let (holder, ix) = provider();
    let subj = subject("u-cdc");
    let consumer = DsrOrchestratorConsumer::new(vec![&holder]);

    // locate: a content-addressed receipt over the located set (the doc referencing the subject).
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

    // The doc exists before the erase (the provider really holds it).
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        1,
        "the subject's doc is indexed before erase"
    );

    // erase: the real purge — the doc is GONE after, not hidden; a content-addressed receipt is returned.
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

    // The contract is honoured for real: the subject's doc is purged (0 recoverable).
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        0,
        "the subject's doc was purged via the contract"
    );
}

/// **A tenant offboard (`EraseScope::Tenant`) over the real holder is a crypto-shred recording the
/// destroyed key epoch (the GD-4 lever's audit trail) — the contract's tenant-scope branch.**
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
