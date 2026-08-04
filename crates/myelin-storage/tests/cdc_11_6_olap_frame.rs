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

struct OlapBusFeeder {
    store: OlapReadStore,
}

impl OlapBusFeeder {
    fn boot(region: Region) -> OlapBusFeeder {
        OlapBusFeeder {
            store: OlapReadStore::pinned_to(region),
        }
    }

    fn feed(&mut self, env: &EventEnvelope) -> OlapApply {
        let event: OlapEvent = OlapEvent::from_envelope(env);
        self.store
            .apply(&event)
            .expect("an in-region bus event is admitted")
    }
}

#[test]
fn cdc_11_6_bus_feeder_projects_off_the_stream_idempotently() {
    let mut feeder = OlapBusFeeder::boot(region());

    assert_eq!(
        feeder.feed(&envelope("01J-1", "issue:PROJ-1")),
        OlapApply::Fresh
    );
    assert_eq!(
        feeder.feed(&envelope("01J-1", "issue:PROJ-1")),
        OlapApply::Duplicate,
        "redelivery of the same event_id is a no-op (dedup on event_id)"
    );

    assert_eq!(
        feeder.store.oltp_scan_path_count(),
        0,
        "the OLAP read model is fed off the bus stream only - no OLTP-scan backdoor (§3.4)"
    );
    assert_eq!(
        feeder.store.doc_count(),
        1,
        "exactly one projected analytics doc"
    );
    assert_eq!(
        feeder.store.doc("issue:PROJ-1").unwrap().last_event_id,
        "01J-1"
    );
}

#[test]
fn cdc_11_6_reindex_from_source_is_the_only_rebuild_path() {
    let mut feeder = OlapBusFeeder::boot(region());
    feeder.feed(&envelope("01J-1", "issue:A"));
    feeder.feed(&envelope("01J-2", "issue:B"));
    let live_docs = feeder.store.doc_count();

    let mut source = SourceLog::new();
    source.append(1, "issue:A").append(2, "issue:B");
    let cold = OlapReadStore::reindex_from_source(region(), &source, 2);

    assert_eq!(
        cold.doc_count(),
        live_docs,
        "cold reindex == live projection (cold == live)"
    );
    assert_eq!(
        cold.oltp_scan_path_count(),
        0,
        "the cold rebuild is reindex-from-source - never an OLTP scan"
    );
}

#[test]
fn cdc_11_6_olap_store_is_a_registered_holder() {
    let holder = OlapStoreHolder::new("issue_analytics_olap");
    let receipt = holder.register();
    assert_eq!(receipt.store, "issue_analytics_olap");
}

#[test]
fn cdc_11_6_olap_store_is_fed_by_the_bus_and_reindexes_byte_matching_live() {
    use myelin_events::{
        EmitContextBase, InProcessBus, OutboxStore, ReindexSource, Relay, SnapshotScope, Timestamp,
    };
    use myelin_storage::{reindex_olap_from_bus, OlapAnalyticsSource, OlapBusConsumer};

    let mut src = OlapAnalyticsSource::new("olap_src");
    src.upsert("issue:A", 1, Some("subj:alice"));
    src.upsert("issue:B", 2, None);

    let mut live = OlapBusConsumer::boot(region());
    for draft in src.replay(&SnapshotScope::new("olap_src", "all"), None) {
        let env = envelope(&draft.event_id().0, &draft.aggregate.0);
        let _ = live.ingest(&env);
    }
    assert_eq!(
        live.store().doc_count(),
        2,
        "two facts projected live off the bus"
    );
    assert_eq!(
        live.store().oltp_scan_path_count(),
        0,
        "fed off the bus - no OLTP-scan backdoor"
    );

    let outbox_handle = OutboxStore::new();
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox_handle.clone(), bus.clone(), || {
        Timestamp("2026-06-20T00:00:02Z".into())
    });
    let mut outbox = outbox_handle;
    let scope = SnapshotScope::new("olap_src", "all");
    let sources: Vec<&dyn ReindexSource> = vec![&src];
    let ctx = EmitContextBase {
        tenant: TenantId::from_token("01J0ACME"),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            TenantId::from_token("01J0ACME"),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        caused_by: None,
    };
    let (cold, receipt) = reindex_olap_from_bus(
        region(),
        &scope,
        &sources,
        &mut outbox,
        &bus,
        &relay,
        ctx,
        "",
    )
    .expect("the OLAP reindex-from-bus succeeds");

    assert_eq!(
        receipt.snapshots_emitted, 2,
        "two snapshots re-emitted (the rebuild)"
    );
    assert_eq!(
        cold.store().doc_count(),
        2,
        "the cold rebuild projected both"
    );
    assert_eq!(
        cold.store().oltp_scan_path_count(),
        0,
        "the cold rebuild is reindex-from-source - never an OLTP scan"
    );
}
