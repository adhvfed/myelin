//! P-ST-29 (global P-331) GATE / DRILL — **D-S12: restricted-subject OLAP suppression (C5)**. Dated
//! green artifact (2026-06-22).
//!
//! **D-S12 (storage.md §3.4 C5 / testing-strategy §4.2 row D-S12):** `restrict(subject)`; run
//! CFD/cycle-time/velocity → assert the subject's contribution is **absent**. **Gate:
//! `olap_restricted_subject_leak` = 0.** This is the storage realisation of the C5 SHARPENED
//! contract: a restricted subject's rows are excluded from analytics aggregates pending erasure/lift
//! — a COMPLIANCE gate, not a tuning knob, the thing that unblocks the partially-blocking Issues ask
//! (CR §8: `issue.*`/`sla.*`/`cycle.*` reports depend on T4 and must not leak a restricted subject).
//!
//! This drill runs the C5 gate END-TO-END over the REAL OLAP LIVE BUS FEED: the OLAP read model is
//! populated by ingesting durable `EventEnvelope`s through the real [`OlapBusConsumer`] (the P-ST-18
//! feed — never an OLTP scan), then a subject is restricted and the four analytics aggregates
//! (CFD/cycle-time/velocity/delivery-health) are computed via [`OlapAnalytics`]; the gate asserts the
//! restricted subject's contribution is absent from EVERY aggregate and `olap_restricted_subject_leak
//! == 0`. A green here is PROVEN (the aggregate output is measured, the restricted subject genuinely
//! gone), never claimed (EI-01 §3): a single surviving contribution flips the leak count `> 0` and
//! FAILS the drill, and the threshold is NOT weakened to pass.
//!
//! **STOR-D1 / STOR-D2 remain green (re-run):** this prompt adds a QUERY-TIME aggregate FILTER over
//! the unchanged read model and touches NO restore/backup code, so the two permanent restore-verify
//! gates stay green by construction (their drill files run in the same `cargo test --workspace`). An
//! OLAP row is a derived, NOT-backed-up store (T4) — it inherits crypto-shred via the source DEK.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Region, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{AnalyticsAggregate, OlapAnalytics, OlapBusConsumer, RestrictionGateSignal};
use myelin_tenancy::TenantId;

fn region() -> Region {
    Region("fr-par".into())
}

fn tenant() -> TenantId {
    TenantId("01J0ACME".into())
}

/// A live bus envelope about `subject`, projecting into `aggregate` (the work item the analytics doc
/// is keyed by). The OLAP consumer lifts ONLY the routing refs (PII-free) — the subject ref is the
/// C5 filter key.
fn envelope(event_id: &str, aggregate: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": aggregate }),
    }
}

/// Boot a consumer and feed it the issue facts of two subjects (alice: two items, bob: one).
fn fed_consumer() -> OlapBusConsumer {
    let mut consumer = OlapBusConsumer::boot(region());
    for (id, agg, subj) in [
        ("e1", "issue:PROJ-1", "subj:alice"),
        ("e2", "issue:PROJ-2", "subj:bob"),
        ("e3", "issue:PROJ-3", "subj:alice"),
    ] {
        consumer
            .ingest(&envelope(id, agg, subj))
            .expect("an in-region bus event is admitted");
    }
    consumer
}

/// **D-S12 — a restricted subject's contribution is ABSENT from CFD/cycle-time/velocity/
/// delivery-health; `olap_restricted_subject_leak` = 0.** The headline drill: restrict `subj:alice`
/// after the live feed projected her rows, then run every C5 aggregate and assert her contribution is
/// gone while bob's survives.
#[test]
fn d_s12_restricted_subject_absent_from_every_aggregate_zero_leak() {
    let mut consumer = fed_consumer();
    // Before restriction: all three contribute (the live-fed read model).
    assert_eq!(
        OlapAnalytics::over(consumer.store()).velocity(),
        3,
        "all three items contribute before restriction"
    );

    // `restrict(subject)` propagates into T4 — the storage realisation of the GDPR restrict flag.
    consumer.store_mut().set_restricted("subj:alice", true);

    let analytics = OlapAnalytics::over(consumer.store());

    // CFD: alice's two work items are excluded — only bob's remains.
    let cfd = analytics.cfd();
    assert_eq!(cfd.len(), 1, "CFD: only bob's item survives");
    assert!(cfd.contains_key("issue:PROJ-2"), "bob's item present");
    assert!(!cfd.contains_key("issue:PROJ-1"), "alice excluded from CFD");
    assert!(!cfd.contains_key("issue:PROJ-3"), "alice excluded from CFD");

    // cycle-time / velocity / delivery-health: alice's contribution is absent.
    assert_eq!(
        analytics.cycle_time_sample_size(),
        1,
        "alice out of cycle-time"
    );
    assert_eq!(analytics.velocity(), 1, "alice out of velocity");
    assert_eq!(
        analytics.delivery_health_wip(),
        1,
        "alice out of delivery-health"
    );

    // THE GATE: olap_restricted_subject_leak == 0 across all four aggregates.
    let audit = analytics.leak_audit();
    assert_eq!(
        audit.olap_restricted_subject_leak, 0,
        "GATE: olap_restricted_subject_leak == 0 (a restricted subject leaked into analytics is a §3.4 breach)"
    );
    assert_eq!(
        audit.per_aggregate.len(),
        AnalyticsAggregate::ALL.len(),
        "the gate ran over every C5 aggregate"
    );
    assert!(audit.leaked_subjects.is_empty(), "no leaked subjects");

    // The dated GREEN D-S12 artifact (non-vacuous: one subject restricted, four aggregates checked).
    let signal = RestrictionGateSignal::from_audit("issue_analytics_olap", &audit, 1);
    assert!(
        signal.is_green(),
        "D-S12 green: olap_restricted_subject_leak == 0, all four aggregates, ≥1 restricted: {signal:?}"
    );
}

/// **The restriction LIFTS → the subject REAPPEARS (no reindex).** §3.4: "withheld until restriction
/// lifts or erasure completes". The rows stayed in the read model; lifting the flag makes alice
/// contribute again on the very next query.
#[test]
fn d_s12_restriction_lifts_subject_reappears_no_reindex() {
    let mut consumer = fed_consumer();
    consumer.store_mut().set_restricted("subj:alice", true);
    assert_eq!(OlapAnalytics::over(consumer.store()).velocity(), 1);

    consumer.store_mut().set_restricted("subj:alice", false);
    assert_eq!(
        OlapAnalytics::over(consumer.store()).velocity(),
        3,
        "alice reappears the instant restriction lifts — filter-at-query-time, no reindex"
    );
}

/// **The gate is non-vacuous and the leak is REAL-measured.** A drill that restricts NOTHING reads
/// RED (proving the gate is exercised, not vacuously green); and the leak count is computed from the
/// REAL aggregate output (the contributing-subject set), not a claim.
#[test]
fn d_s12_gate_is_non_vacuous_and_leak_is_measured() {
    let consumer = fed_consumer();
    let analytics = OlapAnalytics::over(consumer.store());
    let audit = analytics.leak_audit();
    // Nothing restricted → 0 leak BUT a vacuous run reads RED (the gate must restrict ≥ 1 subject).
    let vacuous = RestrictionGateSignal::from_audit("issue_analytics_olap", &audit, 0);
    assert!(
        !vacuous.is_green(),
        "a run that restricted 0 subjects proves nothing — the gate reads RED"
    );
    // The contributing-subject set is the REAL aggregate output the leak audit intersects.
    assert_eq!(
        analytics.contributing_subjects().len(),
        2,
        "alice + bob contribute"
    );
}
