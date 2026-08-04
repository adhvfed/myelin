use myelin_events::{BusTransport, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::iam_events::IDENTITY_TUPLE_WRITTEN;
use myelin_identity::{
    ObjectId, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::TupleStore;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = myelin_identity::Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn actor() -> myelin_identity::Principal {
    myelin_identity::Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn consumer_compiles_role_grant(object: &str, relation: &str, subject: &str) -> Vec<TupleDelta> {
    vec![TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })]
}

#[test]
fn cdc_4_6_write_tuples_provider_emits_consumer_stamps_zookie() {
    let outbox = OutboxStore::new();
    let provider = TupleStore::new(outbox.clone());
    let s = scope("acme");

    let deltas = consumer_compiles_role_grant("org:acme", "member", "p:alice");

    let zookie: Zookie = provider
        .write_tuples(
            &s,
            &actor(),
            &deltas,
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("the provider write_tuples returns the zookie");

    let stamped = provider.object_zookie(&s, "org:acme");
    assert_eq!(
        stamped, zookie,
        "the consumer stamps + reads back exactly the provider's zookie"
    );

    assert_eq!(
        outbox.outbox_depth(),
        1,
        "exactly one event for the one committed write"
    );
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    let published = bus.consume("");
    assert_eq!(
        published.len(),
        1,
        "the relay published exactly the one event"
    );
    assert_eq!(published[0].type_.0, IDENTITY_TUPLE_WRITTEN);
    assert_eq!(
        published[0].payload["zookie"],
        serde_json::json!(zookie.0),
        "the emitted identity.tuple.written carries the write's zookie (the S8 watermark)"
    );
}

#[test]
fn cdc_4_6_zookie_advances_monotonically_across_writes() {
    let provider = TupleStore::new(OutboxStore::new());
    let s = scope("acme");
    let z0 = provider
        .write_tuples(
            &s,
            &actor(),
            &consumer_compiles_role_grant("org:acme", "member", "p:alice"),
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .unwrap();
    let z1 = provider
        .write_tuples(
            &s,
            &actor(),
            &consumer_compiles_role_grant("org:acme", "admin", "p:bob"),
            None,
            None,
            Timestamp("2026-06-19T00:00:01Z".into()),
        )
        .unwrap();
    assert!(
        z1.0 > z0.0,
        "the provider's zookie advances monotonically: {z1:?} after {z0:?}"
    );
}
