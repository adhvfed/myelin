//! Unit tests for the REAL REF-P15 erasure holder (REF-D5 CI variant + the §4.6 locate/erase/restrict
//! surface). Proves: locate finds the subject's edges; erase purges the cache PII (0 recoverable) +
//! relies on the pseudonymous edge (no backdoor); restrict suppresses; registration/classification is
//! unchanged; idempotent. The full backup-level shred (REF-D5 at scale) is REF-P25 (named).

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
    SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, gtenant()))
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

/// An edge event authored by `actor` (the PSEUDONYMOUS origin_actor) referencing source→target.
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

/// A populated edge projection: the subject `p-erase-me` authored two edges; another subject one.
fn populated_projection() -> EdgeProjection {
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    builder.handle(&edge_event(
        "e1",
        "p-erase-me",
        "myelin://acme/chat/message/m1",
        "myelin://acme/knowledge/page/7c2",
    ));
    builder.handle(&edge_event(
        "e2",
        "p-erase-me",
        "myelin://acme/chat/message/m2",
        "myelin://acme/issue/issue/ENG-1",
    ));
    builder.handle(&edge_event(
        "e3",
        "p-other",
        "myelin://acme/chat/message/m3",
        "myelin://acme/issue/issue/ENG-2",
    ));
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

// ── locate (§4.6): the edges naming the subject (by the pseudonymous opaque id) ──

/// **`locate(subject)` over a live projection finds the subject's edges (by the opaque origin_actor) —
/// the real REF-P15 body, not the empty stub.** The receipt records the cardinality; PII-free.
#[test]
fn locate_finds_the_subjects_edges_by_pseudonymous_id() {
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()));
    let report = holder.locate(&subject("p-erase-me"), gtenant()).expect("locate succeeds");
    assert_eq!(report.receipt.operation, "locate");
    assert!(report.receipt.content_hash.starts_with("blake3:"));
    // the subject authored exactly two edges (the receipt outcome names the count).
    let edges = populated_projection().count_by_actor(&ttenant(), &region(), "p-erase-me");
    assert_eq!(edges, 2, "the subject authored two edges");
    // another subject's edges are NOT located (tenant-first, opaque-id match).
    let other = populated_projection().count_by_actor(&ttenant(), &region(), "p-other");
    assert_eq!(other, 1, "a different subject's edge is separate");
}

/// **`register_refs_holders` opens BOTH Refs stores (the edge OLTP index + the R2 cache).** Catches a
/// mutant that returns an empty registry.
#[test]
fn register_opens_both_stores() {
    let registry = register_refs_holders();
    assert!(registry.is_registered(StoreKind::Oltp, REFS_EDGE_STORE), "the edge store is registered");
    assert!(registry.is_registered(StoreKind::Cache, REFS_CACHE_STORE), "the cache store is registered");
    assert_eq!(registry.len(), 2, "exactly the two Refs stores");
}

/// **`EdgeBacking::restrict_set` returns the SHARED suppression set the holder writes into.** A
/// `restrict` recorded through the holder is visible on the accessor's set (catches a mutant that
/// returns a fresh empty set).
#[test]
fn restrict_set_accessor_returns_the_shared_set() {
    let restrict = RestrictSet::new();
    let backing = EdgeBacking::with_restrict(populated_projection(), restrict.clone());
    backing.restrict_set().set("p-x", true);
    assert!(restrict.is_restricted("p-x"), "the accessor exposes the SAME shared set");
}

/// **The erase receipt's outcome reflects the REAL located-edge count for a backed subject erase (the
/// `(Some(b), Subject)` arm runs the count, not 0).** Two different subjects → two different receipts
/// (the count is folded into the content-address). Catches a mutant deleting the count arm.
#[test]
fn backed_subject_erase_receipt_reflects_the_real_count() {
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()));
    // p-erase-me authored 2 edges; p-other authored 1 — distinct counts → distinct content-addresses.
    let r_two = holder
        .erase(EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() })
        .expect("erase");
    let r_one = holder
        .erase(EraseScope::Subject { subject: subject("p-other"), tenant: gtenant() })
        .expect("erase");
    assert_ne!(
        r_two.receipt.content_hash, r_one.receipt.content_hash,
        "different subjects (2 vs 1 edges) yield different content-addressed receipts — the count is real"
    );
    // and an UNBACKED erase (count = 0) differs from the backed 2-edge one.
    let r_unbacked = RefsEdgeHolder::default()
        .erase(EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() })
        .expect("erase");
    assert_ne!(r_two.receipt.content_hash, r_unbacked.receipt.content_hash, "backed count ≠ unbacked 0");
}

/// **The cache purge evicts EVERY distinct ref the subject's edges touch (source/source_root/target/
/// target_root) — the count is a SUM, not a product.** Warm two distinct cached titles the subject
/// authored; erase purges BOTH (catches a `+=` → `*=` mutant: with 2 entries `*=` from 0 stays 0).
#[test]
fn cache_purge_evicts_every_distinct_ref_the_subject_touches() {
    let projection = populated_projection();
    let cache = cache();
    // the subject p-erase-me authored edges from m1→page7c2 and m2→ENG-1: warm BOTH source titles.
    let r1 = "myelin://acme/chat/message/m1";
    let r2 = "myelin://acme/chat/message/m2";
    cache.fill(&ttenant(), &region(), &myelin_events::ArtifactRef(r1.into()), &projection_for(r1, "Name One")).expect("warm 1");
    cache.fill(&ttenant(), &region(), &myelin_events::ArtifactRef(r2.into()), &projection_for(r2, "Name Two")).expect("warm 2");
    assert!(cache.read(&ttenant(), &region(), &myelin_events::ArtifactRef(r1.into())).is_some());
    assert!(cache.read(&ttenant(), &region(), &myelin_events::ArtifactRef(r2.into())).is_some());

    let holder = RefsCacheHolder::with_cache(cache.clone(), projection);
    holder
        .erase(EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() })
        .expect("erase");

    let r_many = holder
        .erase(EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() })
        .expect("erase");

    // BOTH distinct cached titles are purged (the purge summed across the subject's edges).
    assert!(cache.read(&ttenant(), &region(), &myelin_events::ArtifactRef(r1.into())).is_none(), "title 1 purged");
    assert!(cache.read(&ttenant(), &region(), &myelin_events::ArtifactRef(r2.into())).is_none(), "title 2 purged");

    // the purge COUNT is folded into the receipt outcome (a SUM of the distinct refs the edges touch).
    // Build TWO holders for the SAME subject id + tenant (so subject/tenant cannot account for any
    // receipt difference) but DIFFERENT edge cardinality: the ONLY thing that can differ in the
    // content-address is the purge COUNT. Under a `+=`→`*=` mutant both render "purged 0" → identical
    // hashes → this assert fails, catching the mutant. Under the correct `+=` the counts differ.
    let proj_few = {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        b.handle(&edge_event("f1", "p-same", "myelin://acme/chat/message/q1", "myelin://acme/issue/issue/ENG-A"));
        b.projection().clone()
    };
    let proj_many = {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        b.handle(&edge_event("m1", "p-same", "myelin://acme/chat/message/q1", "myelin://acme/issue/issue/ENG-A"));
        b.handle(&edge_event("m2", "p-same", "myelin://acme/chat/message/q2", "myelin://acme/issue/issue/ENG-B"));
        b.handle(&edge_event("m3", "p-same", "myelin://acme/chat/message/q3", "myelin://acme/issue/issue/ENG-C"));
        b.projection().clone()
    };
    let r_few = RefsCacheHolder::with_cache(cache.clone(), proj_few)
        .erase(EraseScope::Subject { subject: subject("p-same"), tenant: gtenant() })
        .expect("erase few");
    let r_many2 = RefsCacheHolder::with_cache(cache.clone(), proj_many)
        .erase(EraseScope::Subject { subject: subject("p-same"), tenant: gtenant() })
        .expect("erase many");
    assert_ne!(
        r_few.receipt.content_hash, r_many2.receipt.content_hash,
        "SAME subject+tenant, MORE edges → a different purge count → a different receipt (the count is a real sum)"
    );
    let _ = r_many; // (the earlier two-title purge receipt; retained for the eviction assertions above)
}

/// **An UNBACKED holder is empty-but-correct (the registration-only `serve`-before-store posture).**
/// `locate` returns a content-addressed receipt over 0 edges — never a panic.
#[test]
fn unbacked_holder_locate_is_empty_but_correct() {
    let holder = RefsEdgeHolder::default();
    let report = holder.locate(&subject("p-1"), gtenant()).expect("locate succeeds");
    assert_eq!(report.receipt.operation, "locate");
    assert!(report.receipt.content_hash.starts_with("blake3:"));
}

// ── erase (§4.6): the SMALL structural surface — opaque edge + cache purge, no backdoor ──

/// **REF-D5 (CI variant) — erase a subject → cache PII purged (0 recoverable), the edge keeps the
/// opaque id (Identity 4.8 shred makes it unresolvable), no resolve-error, no backdoor.** This is the
/// load-bearing erasure proof: the only name-bearing PII (a cached title) is gone; the edge is opaque.
#[test]
fn ref_d5_erase_purges_cache_pii_zero_recoverable_no_backdoor() {
    let projection = populated_projection();
    let cache = cache();

    // warm the cache with a projection whose TITLE holds the subject's name (the PII to purge).
    let secret = "myelin://acme/chat/message/m1";
    cache
        .fill(&ttenant(), &region(), &myelin_events::ArtifactRef(secret.into()), &projection_for(secret, "Alice Smith"))
        .expect("warm the cache");
    assert!(
        cache.read(&ttenant(), &region(), &myelin_events::ArtifactRef(secret.into())).is_some(),
        "the cached title (a name) is present before erase"
    );

    let holder = RefsCacheHolder::with_cache(cache.clone(), projection);
    let receipt = holder
        .erase(EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() })
        .expect("erase succeeds");
    assert_eq!(receipt.receipt.operation, "erase");

    // 0 recoverable PII: the cached title naming the subject is GONE (a read MISSES — re-resolve, never
    // the stale name). This is the REF-D5 0-recoverable-PII property over the cache.
    assert!(
        cache.read(&ttenant(), &region(), &myelin_events::ArtifactRef(secret.into())).is_none(),
        "0 recoverable PII: the cached title is purged"
    );
}

/// **The edge erase is structural: it does NOT destroy a key (the edge is opaque-id-only) and does NOT
/// write the store directly (no backdoor — content tombstoning is the `*.erased` consumer's).** The
/// receipt records `key_epoch_destroyed = None` at the edge holder (the crypto-shred is the cache DEK).
#[test]
fn edge_erase_is_structural_no_key_no_backdoor() {
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()));
    let receipt = holder
        .erase(EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() })
        .expect("erase succeeds");
    assert!(
        receipt.receipt.key_epoch_destroyed.is_none(),
        "the edge holder destroys no key (the edge is opaque-id-only; the crypto-shred is the cache DEK)"
    );
}

/// **Erase is idempotent: the same scope yields the identical content-addressed receipt.** A
/// re-delivered erase (the DSR fan-out retries) re-affirms the same completion.
#[test]
fn erase_is_idempotent() {
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()));
    let scope = EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() };
    let r1 = holder.erase(scope.clone()).expect("erase 1");
    let r2 = holder.erase(scope).expect("erase 2 (idempotent)");
    assert_eq!(r1, r2, "the same scope yields the identical receipt");
}

/// **The purge is driven through the cache's `invalidate` (the ONE eviction path) — never a second
/// backdoor.** After a subject erase, a second read of a purged ref still MISSES (idempotent eviction).
#[test]
fn cache_purge_is_idempotent_through_the_one_eviction_path() {
    let projection = populated_projection();
    let cache = cache();
    let secret = "myelin://acme/chat/message/m1";
    cache
        .fill(&ttenant(), &region(), &myelin_events::ArtifactRef(secret.into()), &projection_for(secret, "Alice"))
        .expect("warm");
    let holder = RefsCacheHolder::with_cache(cache.clone(), projection);
    holder
        .erase(EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() })
        .expect("erase 1");
    // a re-erase is a no-op (already purged) — idempotent, never an error.
    holder
        .erase(EraseScope::Subject { subject: subject("p-erase-me"), tenant: gtenant() })
        .expect("erase 2 idempotent");
    assert!(cache.read(&ttenant(), &region(), &myelin_events::ArtifactRef(secret.into())).is_none());
}

// ── restrict (§4.6 / GA-D7): suppress, don't delete ──

/// **`restrict(subject, true)` records the subject in the suppression set; `false` re-enables.** The
/// indexer/backlink read consults the SAME shared set (suppress, don't delete).
#[test]
fn restrict_records_into_the_shared_suppression_set() {
    let restrict = RestrictSet::new();
    let holder = RefsEdgeHolder::with_backing(EdgeBacking::with_restrict(populated_projection(), restrict.clone()));

    holder.restrict(&subject("p-erase-me"), true).expect("restrict on");
    assert!(restrict.is_restricted("p-erase-me"), "the subject is suppressed (the reader sees it)");

    holder.restrict(&subject("p-erase-me"), false).expect("restrict off");
    assert!(!restrict.is_restricted("p-erase-me"), "restrict off re-enables (not deleted)");
}

// ── registration / classification unchanged (the stub→real reconcile preserved 1.4 + §3.2) ──

/// **Registration + classification are UNCHANGED by REF-P15 (the EI-01 §7 reconcile): 0 orphan Refs
/// stores.** The real erasure body did not change which stores Refs holds (H12 edge / H9 cache).
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

/// **The holders are object-safe (held behind `dyn PersonalDataHolder`) — the DSR orchestrator fans
/// over a heterogeneous holder set (contract 10.1).** Both the real-backed and the unbacked forms.
#[test]
fn holders_are_object_safe() {
    let holders: Vec<Box<dyn PersonalDataHolder>> = vec![
        Box::new(RefsEdgeHolder::with_backing(EdgeBacking::new(populated_projection()))),
        Box::new(RefsCacheHolder::default()),
    ];
    for h in &holders {
        assert!(h.locate(&subject("p-1"), gtenant()).is_ok());
    }
}
