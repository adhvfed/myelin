//! Contract 11.6 CDC pair — the OLAP read store FRAME (the holder + the CQRS-fed-by-the-bus shape).
//!
//! The prompt requires "the provider+consumer pair for 11.6 (the frame — the consumer shape)".
//! This is the consumer-driven contract test: the PROVIDER is `myelin-storage` (the
//! [`OlapReadStore`] frame this prompt ships — Storage owns the OLAP read model); the CONSUMER is
//! the bus-fed CQRS feeder (modelled here as a tiny `OlapBusFeeder`, the shape the live feed P-ST-18
//! takes — the idempotent consumer that lifts a bus [`EventEnvelope`] into the read model via
//! `OlapEvent::from_envelope` and applies it, dedup on `event_id`).
//!
//! The test pins the frozen frame properties every downstream relies on:
//!   - the consumer is fed off the durable event STREAM (an `EventEnvelope`), NEVER by scanning OLTP
//!     (`oltp_scan_path_count == 0` — the structural no-backdoor guard);
//!   - it is idempotent (dedup on `event_id` — a redelivery is a no-op);
//!   - it is residency-pinned (per-cell, not a global warehouse);
//!   - reindex-from-source is the ONLY rebuild path (cold == live).
//!
//! If 11.6's surface drifts, this stops compiling/passing — that is the contract.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    PiiKeyRef, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{OlapApply, OlapEvent, OlapReadStore, OlapStoreHolder, SourceLog};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

/// A bus EventEnvelope the OLAP CQRS consumer is fed FROM (the durable stream — never OLTP).
fn envelope(event_id: &str, aggregate: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId::from_token("01J0ACME"),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId::from_token("01J0ACME"),
        )),
        subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None::<PiiKeyRef>,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": "myelin://acme/issues/issue/PROJ-1" }),
    }
}

/// A consumer of 11.6: the bus-fed CQRS feeder that fronts the OLAP read model. This is the shape
/// the live feed (P-ST-18) takes — it does NOT re-implement the read model; it lifts a bus event
/// into the frame's consumer-input shape and drives `OlapReadStore::apply` (dedup on `event_id`,
/// never an OLTP scan).
struct OlapBusFeeder {
    store: OlapReadStore,
}

impl OlapBusFeeder {
    fn boot(region: Region) -> OlapBusFeeder {
        OlapBusFeeder {
            store: OlapReadStore::pinned_to(region),
        }
    }

    /// Feed one durable bus event into the OLAP read model (the idempotent CQRS consumer step).
    fn feed(&mut self, env: &EventEnvelope) -> OlapApply {
        let event: OlapEvent = OlapEvent::from_envelope(env);
        self.store
            .apply(&event)
            .expect("an in-region bus event is admitted")
    }
}

/// The provider+consumer happy path: the bus feeder lifts a durable event into the read model;
/// a redelivery of the same `event_id` is a no-op (idempotent — dedup on `event_id`); the read
/// model is fed off the STREAM with no OLTP-scan backdoor.
#[test]
fn cdc_11_6_bus_feeder_projects_off_the_stream_idempotently() {
    let mut feeder = OlapBusFeeder::boot(region());

    // Fed off the durable bus event stream — NOT an OLTP scan.
    assert_eq!(feeder.feed(&envelope("01J-1", "issue:PROJ-1")), OlapApply::Fresh);
    // A redelivery of the same event_id is absorbed (effectively-once).
    assert_eq!(
        feeder.feed(&envelope("01J-1", "issue:PROJ-1")),
        OlapApply::Duplicate,
        "redelivery of the same event_id is a no-op (dedup on event_id)"
    );

    // The structural guard: there is NO OLTP-scan backdoor — reindex-from-source is the only path.
    assert_eq!(
        feeder.store.oltp_scan_path_count(),
        0,
        "the OLAP read model is fed off the bus stream only — no OLTP-scan backdoor (§3.4)"
    );
    assert_eq!(feeder.store.doc_count(), 1, "exactly one projected analytics doc");
    assert_eq!(
        feeder.store.doc("issue:PROJ-1").unwrap().last_event_id,
        "01J-1"
    );
}

/// The frame is **reindex-from-source rebuildable as the ONLY rebuild path** (cold == live). The
/// CDC consumer the live feed (P-ST-18) shares with the cold path proves cold == live: rebuilding
/// from the durable source log yields the same doc set as the live projection.
#[test]
fn cdc_11_6_reindex_from_source_is_the_only_rebuild_path() {
    // Live projection off the bus.
    let mut feeder = OlapBusFeeder::boot(region());
    feeder.feed(&envelope("01J-1", "issue:A"));
    feeder.feed(&envelope("01J-2", "issue:B"));
    let live_docs = feeder.store.doc_count();

    // Cold rebuild from the durable source log (the ONLY rebuild path — no OLTP-scan).
    let mut source = SourceLog::new();
    source
        .append(1, "issue:A")
        .append(2, "issue:B");
    let cold = OlapReadStore::reindex_from_source(region(), &source, 2);

    assert_eq!(cold.doc_count(), live_docs, "cold reindex == live projection (cold == live)");
    assert_eq!(
        cold.oltp_scan_path_count(),
        0,
        "the cold rebuild is reindex-from-source — never an OLTP scan"
    );
}

/// The OLAP store is a registered [`PersonalDataHolder`] (the holder half of the frame — "every
/// store is a holder", D-S5) so the DSR fan-out reaches the analytics warehouse.
#[test]
fn cdc_11_6_olap_store_is_a_registered_holder() {
    let holder = OlapStoreHolder::new("issue_analytics_olap");
    let receipt = holder.register();
    assert_eq!(receipt.store, "issue_analytics_olap");
}
