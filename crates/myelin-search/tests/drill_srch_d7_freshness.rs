//! # Drill — SRCH-D7 freshness CI floor (SRCH-P06 → P-169)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D7
//! (freshness; the CI freshness floor here, the full-scale-under-load variant is SRCH-P24/M5).
//! **Architecture:** `search-and-indexing.md` §4.1 (near-real-time incremental indexing) + §4.11
//! (the `index_lag` telemetry, contract 1.8 — observability is part of the pass).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-20)
//! A synthetic domain event → searchable within the seconds-grade budget, and the `index_lag`
//! telemetry (contract 1.8) emits + recovers to 0 (no signal == failed drill). The pipeline here is
//! synchronous (the indexer applies the upsert in-line on `handle`), so the freshness is bounded by
//! the indexer's own apply time, well under the seconds-grade budget — this is the CI FLOOR. The
//! full-scale freshness budget under load (the index_lag alarm BEFORE user-visible staleness) is the
//! M5 follow-on (SRCH-P24); this CI floor is the named precursor.
//!
//! ## Floors named
//! - The full-scale-under-load freshness budget (the index-lag alarm at world scale) is **SRCH-P24 /
//!   M5** — this is the CI floor, not the load drill.
//! - The mock embedding adapter + the synthetic producer are the SRCH-P06 named floors (real model
//!   post-M5; real per-subsystem IndexSpecs M3 Git/KN, M4 Issues/CI/Chat).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    HandleOutcome, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::engine::AclFilter;
use myelin_search::{
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
    SearchProjection,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

/// The seconds-grade freshness budget for the CI floor (the synchronous in-process pipeline is far
/// under this; the number is the FLOOR the M5 full-scale drill tightens). 2 seconds is the
/// near-real-time SLO the architecture names for the steady-state indexer (§4.1).
const FRESHNESS_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

/// A synthetic-producer owner projection (the named floor): a fixed projection per ref.
#[derive(Default)]
struct SyntheticOwner {
    projections: Mutex<BTreeMap<String, SearchProjection>>,
}
impl SyntheticOwner {
    fn put(&self, ref_: &str, text: &str) {
        self.projections.lock().unwrap().insert(
            ref_.to_string(),
            SearchProjection {
                text: text.into(),
                fields: BTreeMap::new(),
                lang: None,
            },
        );
    }
}
impl ProjectFetcher for SyntheticOwner {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        self.projections
            .lock()
            .unwrap()
            .get(&ref_.0)
            .cloned()
            .ok_or(ProjectFetchError::Gone)
    }
}

fn event(id: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("knowledge.page.created".into()),
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

/// **SRCH-D7 (CI freshness floor): a synthetic event → searchable within the seconds-grade budget,
/// and `index_lag` (contract 1.8) emits + recovers to 0.**
#[test]
fn srch_d7_synthetic_event_is_searchable_within_the_freshness_budget() {
    let r = "myelin://acme/knowledge/page/42";
    let owner = Arc::new(SyntheticOwner::default());
    owner.put(r, "distributed consensus and raft");
    // Knowledge pages are semantically indexed (the embed branch runs through the mock adapter).
    let indexer = IncrementalIndexer::new(
        vec![IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()],
        owner,
        Arc::new(MockEmbeddingAdapter::new(16)),
    );

    // index_lag emits (contract 1.8) — 0 before any event (the signal exists, named).
    assert_eq!(
        indexer.index_lag(),
        0,
        "the index_lag signal reads 0 on a fresh indexer"
    );
    assert_eq!(
        IncrementalIndexer::INDEX_LAG_SIGNAL,
        "search.index_lag",
        "the contract-1.8 signal name"
    );

    // The synthetic event arrives; measure the time to searchable.
    let t0 = Instant::now();
    assert_eq!(indexer.handle(&event("01J-1", r)), HandleOutcome::Done);
    let hits = indexer
        .search_ft(&tenant(), &region(), &AclFilter::ids([r]), "raft", 10)
        .expect("search");
    let elapsed = t0.elapsed();

    assert_eq!(hits.len(), 1, "the synthetic event is searchable");
    assert_eq!(hits[0].doc_id, r);
    assert!(
        elapsed < FRESHNESS_BUDGET,
        "freshness: searchable in {elapsed:?}, well under the {FRESHNESS_BUDGET:?} CI floor budget"
    );

    // index_lag recovered to 0 after the synchronous apply (the steady-state signal).
    assert_eq!(
        indexer.index_lag(),
        0,
        "index_lag recovers to 0 after projection (no stuck lag)"
    );
}

/// **The `index_lag` signal is NON-ZERO while an event is mid-flight (the drill that pauses mid-apply
/// reads it non-zero — the SLO is the time-to-project; no signal == failed drill).** We inject an
/// owner that BLOCKS until we observe the lag, proving the signal tracks in-flight work.
#[test]
fn srch_d7_index_lag_is_observable_mid_flight() {
    use std::sync::mpsc;

    let r = "myelin://acme/knowledge/page/7";

    /// An owner whose `project` BLOCKS (signalling it has started, then waiting) so the test can read
    /// `index_lag` while the indexer is mid-apply — proving the signal is live, not a constant 0.
    struct BlockingOwner {
        started: mpsc::Sender<()>,
        release: Mutex<Option<mpsc::Receiver<()>>>,
    }
    impl ProjectFetcher for BlockingOwner {
        fn project(
            &self,
            _t: &TenantId,
            _r: &Region,
            _ref: &ArtifactRef,
        ) -> Result<SearchProjection, ProjectFetchError> {
            // Signal that we have entered the pipeline (index_lag is now non-zero), then block until
            // released.
            let _ = self.started.send(());
            if let Some(rx) = self.release.lock().unwrap().take() {
                let _ = rx.recv();
            }
            Ok(SearchProjection {
                text: "blocked body".into(),
                fields: BTreeMap::new(),
                lang: None,
            })
        }
    }

    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let owner = Arc::new(BlockingOwner {
        started: started_tx,
        release: Mutex::new(Some(release_rx)),
    });
    let indexer = IncrementalIndexer::new(
        vec![IndexSpec::new("knowledge", "page", BTreeMap::new())],
        owner,
        Arc::new(MockEmbeddingAdapter::new(8)),
    );

    let ix = indexer.clone();
    let ev = event("01J-7", r);
    let h = std::thread::spawn(move || ix.handle(&ev));

    // Wait until the indexer has entered the pipeline (the owner fetch started) — index_lag is now > 0.
    started_rx.recv().expect("the indexer entered the pipeline");
    assert!(
        indexer.index_lag() >= 1,
        "index_lag is NON-ZERO while an event is mid-flight (the live signal)"
    );

    // Release the owner; the apply completes and lag recovers to 0.
    release_tx.send(()).expect("release");
    assert_eq!(h.join().expect("handle thread"), HandleOutcome::Done);
    assert_eq!(
        indexer.index_lag(),
        0,
        "index_lag recovers to 0 once the apply completes"
    );
}
