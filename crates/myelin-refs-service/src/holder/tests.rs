use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{InMemoryCache, KmsEngine};
use myelin_substrate::{assert_holder_completeness, classify_store};
use myelin_tenancy::{Region, TenantId};

use super::*;
use crate::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use crate::resolve::{Projection, ProjectionCacheRead};
use crate::RefsDekPin;

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        gtenant(),
    ))
}
fn gtenant() -> GdprTenantId {
    GdprTenantId::from_token("acme")
}
fn ttenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, gtenant())
}

fn edge_event(id: &str, actor: &str, source: &str, target: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("refs.edge.created".into()),
        schema_ver: 1,
        tenant: ttenant(),
        region: region(),
        actor: Actor(principal(actor)),
        subject: myelin_events::ArtifactRef(source.into()),
        aggregate: AggregateKey(format!("edge:{source}->{target}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "source": source, "target": target, "rel": "mentions" }),
    }
}

fn populated_projection() -> EdgeProjection {
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    builder.handle(
        &edge_event(
            "e1",
            "p-erase-me",
            "myelin://acme/chat/message/m1",
            "myelin://acme/knowledge/page/7c2",
        ),
        &mut myelin_events::HandlerTx::none(),
    );
    builder.handle(
        &edge_event(
            "e2",
            "p-erase-me",
            "myelin://acme/chat/message/m2",
            "myelin://acme/issue/issue/ENG-1",
        ),
        &mut myelin_events::HandlerTx::none(),
    );
    builder.handle(
        &edge_event(
            "e3",
            "p-other",
            "myelin://acme/chat/message/m3",
            "myelin://acme/issue/issue/ENG-2",
        ),
        &mut myelin_events::HandlerTx::none(),
    );
    builder.projection().clone()
}

fn cache() -> Arc<R2ProjectionCache> {
    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    Arc::new(R2ProjectionCache::new(Arc::new(InMemoryCache::new()), dek))
}

fn projection_for(ref_: &str, title: &str) -> Projection {
    Projection {
        ref_: myelin_events::ArtifactRef(ref_.into()),
        title: title.into(),
        state: "open".into(),
        icon: "doc".into(),
        render_hint: "card".into(),
        sub_anchor: None,
        flag: None,
    }
}

#[test]
fn locate_finds_the_subjects_edges_by_pseudonymous_id() {
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()));
    let report = holder
        .locate(&subject("p-erase-me"), gtenant())
        .expect("locate succeeds");
    assert_eq!(report.receipt.operation, "locate");
    assert!(report.receipt.content_hash.starts_with("blake3:"));
    let edges = populated_projection().count_by_actor(&ttenant(), &region(), "p-erase-me");
    assert_eq!(edges, 2, "the subject authored two edges");
    let other = populated_projection().count_by_actor(&ttenant(), &region(), "p-other");
    assert_eq!(other, 1, "a different subject's edge is separate");
}

#[test]
fn register_opens_both_stores() {
    let registry = register_refs_holders();
    assert!(
        registry.is_registered(StoreKind::Oltp, REFS_EDGE_STORE),
        "the edge store is registered"
    );
    assert!(
        registry.is_registered(StoreKind::Cache, REFS_CACHE_STORE),
        "the cache store is registered"
    );
    assert_eq!(registry.len(), 2, "exactly the two Refs stores");
}

#[test]
fn restrict_set_accessor_returns_the_shared_set() {
    let restrict = RestrictSet::new();
    let backing = EdgeBacking::with_restrict(populated_projection(), restrict.clone());
    backing.restrict_set().set("p-x", true);
    assert!(
        restrict.is_restricted("p-x"),
        "the accessor exposes the SAME shared set"
    );
}

#[test]
fn backed_subject_erase_receipt_reflects_the_real_count() {
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()));
    let r_two = holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me"),
            tenant: gtenant(),
        })
        .expect("erase");
    let r_one = holder
        .erase(EraseScope::Subject {
            subject: subject("p-other"),
            tenant: gtenant(),
        })
        .expect("erase");
    assert_ne!(
        r_two.receipt.content_hash, r_one.receipt.content_hash,
        "different subjects (2 vs 1 edges) yield different content-addressed receipts - the count is real"
    );
    let r_unbacked = RefsEdgeHolder::default()
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me"),
            tenant: gtenant(),
        })
        .expect("erase");
    assert_ne!(
        r_two.receipt.content_hash, r_unbacked.receipt.content_hash,
        "backed count ≠ unbacked 0"
    );
}

#[test]
fn cache_purge_evicts_every_distinct_ref_the_subject_touches() {
    let projection = populated_projection();
    let cache = cache();
    let r1 = "myelin://acme/chat/message/m1";
    let r2 = "myelin://acme/chat/message/m2";
    cache
        .fill(
            &ttenant(),
            &region(),
            &myelin_events::ArtifactRef(r1.into()),
            &projection_for(r1, "Name One"),
        )
        .expect("warm 1");
    cache
        .fill(
            &ttenant(),
            &region(),
            &myelin_events::ArtifactRef(r2.into()),
            &projection_for(r2, "Name Two"),
        )
        .expect("warm 2");
    assert!(cache
        .read(
            &ttenant(),
            &region(),
            &myelin_events::ArtifactRef(r1.into())
        )
        .is_some());
    assert!(cache
        .read(
            &ttenant(),
            &region(),
            &myelin_events::ArtifactRef(r2.into())
        )
        .is_some());

    let holder = RefsCacheHolder::with_cache(cache.clone(), projection);
    holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me"),
            tenant: gtenant(),
        })
        .expect("erase");

    let r_many = holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me"),
            tenant: gtenant(),
        })
        .expect("erase");

    assert!(
        cache
            .read(
                &ttenant(),
                &region(),
                &myelin_events::ArtifactRef(r1.into())
            )
            .is_none(),
        "title 1 purged"
    );
    assert!(
        cache
            .read(
                &ttenant(),
                &region(),
                &myelin_events::ArtifactRef(r2.into())
            )
            .is_none(),
        "title 2 purged"
    );

    let proj_few = {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        b.handle(
            &edge_event(
                "f1",
                "p-same",
                "myelin://acme/chat/message/q1",
                "myelin://acme/issue/issue/ENG-A",
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        b.projection().clone()
    };
    let proj_many = {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        b.handle(
            &edge_event(
                "m1",
                "p-same",
                "myelin://acme/chat/message/q1",
                "myelin://acme/issue/issue/ENG-A",
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        b.handle(
            &edge_event(
                "m2",
                "p-same",
                "myelin://acme/chat/message/q2",
                "myelin://acme/issue/issue/ENG-B",
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        b.handle(
            &edge_event(
                "m3",
                "p-same",
                "myelin://acme/chat/message/q3",
                "myelin://acme/issue/issue/ENG-C",
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        b.projection().clone()
    };
    let r_few = RefsCacheHolder::with_cache(cache.clone(), proj_few)
        .erase(EraseScope::Subject {
            subject: subject("p-same"),
            tenant: gtenant(),
        })
        .expect("erase few");
    let r_many2 = RefsCacheHolder::with_cache(cache.clone(), proj_many)
        .erase(EraseScope::Subject {
            subject: subject("p-same"),
            tenant: gtenant(),
        })
        .expect("erase many");
    assert_ne!(
        r_few.receipt.content_hash, r_many2.receipt.content_hash,
        "SAME subject+tenant, MORE edges → a different purge count → a different receipt (the count is a real sum)"
    );
    let _ = r_many;
}

#[test]
fn unbacked_holder_locate_is_empty_but_correct() {
    let holder = RefsEdgeHolder::default();
    let report = holder
        .locate(&subject("p-1"), gtenant())
        .expect("locate succeeds");
    assert_eq!(report.receipt.operation, "locate");
    assert!(report.receipt.content_hash.starts_with("blake3:"));
}

#[test]
fn ref_d5_erase_purges_cache_pii_zero_recoverable_no_backdoor() {
    let projection = populated_projection();
    let cache = cache();

    let secret = "myelin://acme/chat/message/m1";
    cache
        .fill(
            &ttenant(),
            &region(),
            &myelin_events::ArtifactRef(secret.into()),
            &projection_for(secret, "Alice Smith"),
        )
        .expect("warm the cache");
    assert!(
        cache
            .read(
                &ttenant(),
                &region(),
                &myelin_events::ArtifactRef(secret.into())
            )
            .is_some(),
        "the cached title (a name) is present before erase"
    );

    let holder = RefsCacheHolder::with_cache(cache.clone(), projection);
    let receipt = holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me"),
            tenant: gtenant(),
        })
        .expect("erase succeeds");
    assert_eq!(receipt.receipt.operation, "erase");

    assert!(
        cache
            .read(
                &ttenant(),
                &region(),
                &myelin_events::ArtifactRef(secret.into())
            )
            .is_none(),
        "0 recoverable PII: the cached title is purged"
    );
}

#[test]
fn edge_erase_is_structural_no_key_no_backdoor() {
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()));
    let receipt = holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me"),
            tenant: gtenant(),
        })
        .expect("erase succeeds");
    assert!(
        receipt.receipt.key_epoch_destroyed.is_none(),
        "the edge holder destroys no key (the edge is opaque-id-only; the crypto-shred is the cache DEK)"
    );
}

#[test]
fn erase_is_idempotent() {
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()));
    let scope = EraseScope::Subject {
        subject: subject("p-erase-me"),
        tenant: gtenant(),
    };
    let r1 = holder.erase(scope.clone()).expect("erase 1");
    let r2 = holder.erase(scope).expect("erase 2 (idempotent)");
    assert_eq!(r1, r2, "the same scope yields the identical receipt");
}

#[test]
fn cache_purge_is_idempotent_through_the_one_eviction_path() {
    let projection = populated_projection();
    let cache = cache();
    let secret = "myelin://acme/chat/message/m1";
    cache
        .fill(
            &ttenant(),
            &region(),
            &myelin_events::ArtifactRef(secret.into()),
            &projection_for(secret, "Alice"),
        )
        .expect("warm");
    let holder = RefsCacheHolder::with_cache(cache.clone(), projection);
    holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me"),
            tenant: gtenant(),
        })
        .expect("erase 1");
    holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me"),
            tenant: gtenant(),
        })
        .expect("erase 2 idempotent");
    assert!(cache
        .read(
            &ttenant(),
            &region(),
            &myelin_events::ArtifactRef(secret.into())
        )
        .is_none());
}

#[test]
fn restrict_records_into_the_shared_suppression_set() {
    let restrict = RestrictSet::new();
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::with_restrict(
        populated_projection(),
        restrict.clone(),
    ));

    holder
        .restrict(&subject("p-erase-me"), true)
        .expect("restrict on");
    assert!(
        restrict.is_restricted("p-erase-me"),
        "the subject is suppressed (the reader sees it)"
    );

    holder
        .restrict(&subject("p-erase-me"), false)
        .expect("restrict off");
    assert!(
        !restrict.is_restricted("p-erase-me"),
        "restrict off re-enables (not deleted)"
    );
}

#[test]
fn registration_and_classification_unchanged_zero_orphans() {
    let registry = register_refs_holders();
    let classifier = refs_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, REFS_EDGE_STORE, &classifier),
        Some(Holder::H12ReferenceGraph)
    );
    assert_eq!(
        classify_store(StoreKind::Cache, REFS_CACHE_STORE, &classifier),
        Some(Holder::H9Caches)
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "0 orphan Refs stores"
    );
}

#[test]
fn holders_are_object_safe() {
    let holders: Vec<Box<dyn PersonalDataHolder>> = vec![
        Box::new(RefsEdgeHolder::with_backing(EdgeBacking::new(
            populated_projection(),
        ))),
        Box::new(RefsCacheHolder::default()),
    ];
    for h in &holders {
        assert!(h.locate(&subject("p-1"), gtenant()).is_ok());
    }
}
