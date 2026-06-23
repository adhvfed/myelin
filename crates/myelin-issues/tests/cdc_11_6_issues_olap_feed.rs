//! # Contract 11.6 CDC pair — the Issues OLAP feed (ISS-P20 / P-387, M4).
//!
//! **Contract 11.6 (the OLAP read store + restriction flag): CONSUMED by Issues.** Storage OWNS the
//! OLAP read store FRAME (`myelin_storage::olap::OlapReadStore`) + the C5 restriction gate
//! (`myelin_storage::olap_restrict::OlapAnalytics`); Issues is the CONSUMER that feeds the SHARED store
//! off its `issue.*`/`sla.*`/`cycle.*` analytics stream and adds the SLA-compliance leg. This CDC pair
//! pins the consumer/provider seam:
//!
//! - **PROVIDER side (Storage's frozen frame):** the OLAP read store ingests an `OlapEvent` lifted from
//!   the bus `EventEnvelope` (the `from_envelope` seam), residency-pinned, idempotent on `event_id`,
//!   reindex-from-source is the ONLY rebuild path, and the C5 restriction filter excludes a restricted
//!   subject. Issues drives THIS frozen API — never a parallel store (EI-01 §7).
//! - **CONSUMER side (Issues' feed):** `IssueOlapConsumer` is the bus `EventHandler` (contract 2.4)
//!   whose whitelist is the analytics-driving Issues tokens (NEVER `*`); it projects each envelope into
//!   the frozen `OlapEvent` and `apply`s it; the analytics (CFD/cycle-time/velocity/SLA-compliance)
//!   honour the restriction flag; the feed rebuilds drift-free by reindex-from-source (contract 2.6).
//!
//! The drift-killer: Issues feeds the SHARED `OlapReadStore` through the SAME `OlapEvent::from_envelope`
//! seam Storage's live `OlapBusFeeder` uses — one projection path, no second OLAP model (EI-01 §7).

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events;
use myelin_issues::{IssueOlapConsumer, RestrictionFlag};
use myelin_storage::olap::{OlapApply, OlapEvent, OlapReadStore};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

fn ev(id: &str, type_token: &str, subject: &str, aggregate: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_token.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId(subject.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

/// **PROVIDER side: the frozen OLAP read store ingests an `OlapEvent` lifted from the bus envelope (the
/// `from_envelope` seam the Issues consumer drives).** The same `event_id` projects ONCE (idempotent);
/// the residency pin holds. This is the Storage-owned frame the Issues feed consumes — Issues drives
/// THIS API, never a parallel store.
#[test]
fn provider_olap_read_store_ingests_from_envelope() {
    let mut store = OlapReadStore::pinned_to(region());
    let envelope = ev(
        "01J-1",
        events::SLA_MET,
        "myelin://acme/issue/issue/ENG-1",
        "issue:ENG-1",
    );
    let olap_event = OlapEvent::from_envelope(&envelope);
    // The lift preserves the idempotency key, the partition key, and the aggregate row.
    assert_eq!(olap_event.event_id, "01J-1");
    assert_eq!(olap_event.region, region());
    assert_eq!(olap_event.aggregate_row, "issue:ENG-1");
    // The frozen frame projects it once (idempotent on event_id).
    assert_eq!(store.apply(&olap_event).unwrap(), OlapApply::Fresh);
    assert_eq!(
        store.apply(&olap_event).unwrap(),
        OlapApply::Duplicate,
        "redelivery is a no-op"
    );
    assert_eq!(store.doc_count(), 1);
}

/// **CONSUMER side: the Issues feed drives the SAME frozen frame off the analytics stream.** The
/// `IssueOlapConsumer` whitelist is the analytics-driving Issues tokens (NEVER `*`); it projects an
/// `issue.sla.met` into the shared store. The consumer + the provider agree on the seam (the same
/// `OlapEvent` shape, the same idempotency).
#[test]
fn consumer_issue_feed_drives_the_shared_frame() {
    let c = IssueOlapConsumer::new(region(), RestrictionFlag::new());
    // The whitelist is the analytics-driving tokens, NEVER `*` (BUS-3).
    let subjects: Vec<String> = c.subjects().iter().map(|s| s.0.clone()).collect();
    assert!(subjects.contains(&events::SLA_MET.to_string()));
    assert!(subjects.contains(&events::ISSUE_TRANSITIONED.to_string()));
    assert!(subjects.contains(&events::CYCLE_COMPLETED.to_string()));
    assert!(subjects.iter().all(|s| s != "*"), "never `*` (BUS-3)");
    // The consumer projects an analytics event into the shared store (the same frame as the provider).
    c.handle(&ev(
        "01J-1",
        events::SLA_MET,
        "myelin://acme/issue/issue/ENG-1",
        "issue:ENG-1",
    ));
    assert_eq!(
        c.doc_count(),
        1,
        "the consumer projected one analytics doc into the shared frame"
    );
    // GATE: 0 OLTP reads from the analytics path (CQRS, off the bus).
    assert_eq!(c.oltp_read_count(), 0);
}

/// **The restriction-flag propagation (11.6 — the C5 gate the consumer honours).** A restricted subject
/// contributes 0 rows to the consumer's analytics — proving Issues consumes the restriction flag, not
/// just declares it.
#[test]
fn consumer_honours_the_restriction_flag() {
    let flag = RestrictionFlag::new();
    let c = IssueOlapConsumer::new(region(), flag.clone());
    c.handle(&ev("a1", events::SLA_MET, "psn:alice", "issue:A"));
    c.handle(&ev("b1", events::SLA_MET, "psn:bob", "issue:B"));
    c.analytics(|a| assert_eq!(a.velocity(), 2, "both contribute unrestricted"));
    flag.set("psn:alice", true);
    c.analytics(|a| {
        assert_eq!(a.velocity(), 1, "alice excluded → only bob");
        assert_eq!(
            a.leak_audit().restricted_subject_leak,
            0,
            "the restriction is honoured (0 leak)"
        );
    });
}

/// **The consumer + provider share ONE projection path (EI-01 §7 — no second OLAP model).** A store fed
/// by the Issues consumer and a store fed by the raw frame `apply` (the same events) project the SAME
/// read model — Issues does not fork a second store/projection.
#[test]
fn consumer_and_provider_project_the_same_read_model() {
    // The Issues consumer feed.
    let c = IssueOlapConsumer::new(region(), RestrictionFlag::new());
    c.handle(&ev("e1", events::SLA_MET, "psn:a", "issue:1"));
    c.handle(&ev("e2", events::ISSUE_TRANSITIONED, "psn:b", "issue:2"));

    // The raw frame, fed the SAME events via the SAME from_envelope seam.
    let mut raw = OlapReadStore::pinned_to(region());
    raw.apply(&OlapEvent::from_envelope(&ev(
        "e1",
        events::SLA_MET,
        "psn:a",
        "issue:1",
    )))
    .unwrap();
    raw.apply(&OlapEvent::from_envelope(&ev(
        "e2",
        events::ISSUE_TRANSITIONED,
        "psn:b",
        "issue:2",
    )))
    .unwrap();

    assert_eq!(
        c.parity_bytes(),
        raw.parity_bytes(),
        "the Issues consumer feeds the SAME frozen frame projection — one OLAP model, no fork"
    );
}
