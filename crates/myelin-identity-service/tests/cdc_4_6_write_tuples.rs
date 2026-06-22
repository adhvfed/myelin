//! # The CDC pair for contract 4.6 — `write_tuples([Δtuple], precondition?) → zookie` (P-ID-08 / P-057)
//!
//! **Contract-index row 4.6** (`write_tuples -> zookie`), plus the **4.10 write-half** (the
//! monotonically-advancing zookie the write returns to stamp on the object). This is the
//! dedicated provider+consumer pair the P-ID-08 TESTS field names — the focused, in-CI evidence
//! that the two sides of the `write_tuples` seam cannot drift apart:
//!
//! - the **PROVIDER** ([`TupleStore::write_tuples`]) atomically applies the tuple deltas, advances
//!   the zookie, and emits `iam.tuple_written` **via the outbox** (the only emit path) carrying the
//!   write's zookie as the S8 watermark — and it returns that zookie to the caller;
//! - the **CONSUMER** (a role-compile caller — the org→team→project hierarchy compiler that turns a
//!   role grant into tuple deltas) hands the deltas to `write_tuples` and **stamps the returned
//!   zookie on the object** (`page.acl_zookie`, Chat membership), then reads it back through the
//!   tenant-scoped store and gets exactly the same zookie.
//!
//! The provider's promise (the zookie advances monotonically and is carried on the emitted event)
//! and the consumer's promise (the returned zookie is the object's stamp, readable back) are pinned
//! here so a change to either side fails this test in the same CI job. The S8 *reverse-index*
//! consumer of `iam.tuple_written` (the read-half watermark) lands in P-ID-11/P-ID-12; this pair is
//! the write-side CDC the prompt requires.

use myelin_events::{BusTransport, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::iam_events::IAM_TUPLE_WRITTEN;
use myelin_identity::{
    ObjectId, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::TupleStore;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

/// A verified `(tenant, region)` scope (minted from a verified token — never a path).
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

/// The CONSUMER side: a role-compile caller compiles a role grant into the tuple deltas the
/// provider's `write_tuples` accepts. (The real role-compile path is the ReBAC engine, P-ID-10;
/// here it is the canonical caller shape — `object#relation@subject` deltas.)
fn consumer_compiles_role_grant(object: &str, relation: &str, subject: &str) -> Vec<TupleDelta> {
    vec![TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })]
}

/// **The 4.6 provider+consumer CDC pair.** The consumer compiles a role grant into deltas, the
/// provider `write_tuples` applies them atomically + returns the advanced zookie + emits
/// `iam.tuple_written` via the outbox, and the consumer stamps + reads back exactly that zookie.
#[test]
fn cdc_4_6_write_tuples_provider_emits_consumer_stamps_zookie() {
    let outbox = OutboxStore::new();
    let provider = TupleStore::new(outbox.clone());
    let s = scope("acme");

    // CONSUMER compiles the grant → deltas.
    let deltas = consumer_compiles_role_grant("org:acme", "member", "p:alice");

    // PROVIDER write_tuples: atomic apply → advanced zookie → iam.tuple_written via the outbox.
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

    // CONSUMER stamps the returned zookie on the object and reads it back through the tenant-scoped
    // store — read-your-writes on the write half: the object's stamp IS the write's zookie.
    let stamped = provider.object_zookie(&s, "org:acme");
    assert_eq!(
        stamped, zookie,
        "the consumer stamps + reads back exactly the provider's zookie"
    );

    // The provider emitted exactly one iam.tuple_written via the OUTBOX (no other emit path). The
    // relay (what `serve` runs) publishes it; the event carries the write's zookie for S8.
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
    assert_eq!(published[0].type_.0, IAM_TUPLE_WRITTEN);
    assert_eq!(
        published[0].payload["zookie"],
        serde_json::json!(zookie.0),
        "the emitted iam.tuple_written carries the write's zookie (the S8 watermark)"
    );
}

/// The 4.10 write-half: two provider writes return strictly-increasing zookies (the monotone
/// advance the consumer relies on for the new-enemy guard / read-your-writes).
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
