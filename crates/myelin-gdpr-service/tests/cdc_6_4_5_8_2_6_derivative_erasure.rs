use myelin_gdpr::{EraseScope, Patch, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    DerivativeErasureDriver, NotifHistoryHolder, NotifHistoryModel, RefsGraphHolder,
    RefsGraphModel, RefsResolve, SearchIndexHolder, SearchIndexModel, ERASED_USER,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

fn scope(id: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(id),
        tenant: TenantId::from_token("acme"),
    }
}

#[test]
fn cdc_6_4_search_purge_reindex_incl_embeddings() {
    let search = SearchIndexModel::new();
    search.index_from_source("u-cdc-6-4", "alice");
    assert_eq!(
        search.reidentify_hits("u-cdc-6-4"),
        1,
        "re-identifiable before erase"
    );

    let provider = SearchIndexHolder::new(&search);
    let consumer: &dyn PersonalDataHolder = &provider;
    let receipt = consumer
        .erase(scope("u-cdc-6-4"))
        .expect("Search erase honours 6.4");

    assert_eq!(search.hits("u-cdc-6-4"), 0, "6.4: doc purged");
    assert_eq!(
        search.reidentify_hits("u-cdc-6-4"),
        0,
        "6.4: embedding purged (not hidden)"
    );
    assert_eq!(receipt.receipt.operation, "erase");
    assert!(receipt.receipt.content_hash.starts_with("blake3:"));
}

#[test]
fn cdc_5_8_refs_tombstone_no_resolve_500() {
    let refs = RefsGraphModel::new();
    refs.add_edge_from_source("u-cdc-5-8", "pr:1");
    assert!(matches!(refs.resolve("u-cdc-5-8"), RefsResolve::Live(_)));

    let provider = RefsGraphHolder::new(&refs);
    let consumer: &dyn PersonalDataHolder = &provider;
    let receipt = consumer
        .erase(scope("u-cdc-5-8"))
        .expect("Refs erase honours 5.8");

    assert_eq!(
        refs.resolve("u-cdc-5-8"),
        RefsResolve::Tombstone,
        "5.8: tombstone, not a 500"
    );
    assert_eq!(refs.recoverable_edges("u-cdc-5-8"), 0, "5.8: 0 recoverable");
    assert_eq!(receipt.receipt.operation, "erase");
}

#[test]
fn cdc_notif_humanise_to_erased_user() {
    let notif = NotifHistoryModel::new();
    notif.add_item_from_source("inbox", "u-cdc-notif");

    let provider = NotifHistoryHolder::new(&notif);
    let consumer: &dyn PersonalDataHolder = &provider;
    consumer
        .erase(scope("u-cdc-notif"))
        .expect("Notif erase honours NOTIF-D6");

    assert_eq!(notif.render_mention("inbox").as_deref(), Some(ERASED_USER));
}

#[test]
fn cdc_2_6_reindex_from_source_rectification() {
    let search = SearchIndexModel::new();
    let refs = RefsGraphModel::new();
    search.index_from_source("u-cdc-2-6", "stale");
    refs.add_edge_from_source("u-cdc-2-6", "stale-target");

    let outcome = DerivativeErasureDriver::rectify_via_reindex_from_source(
        "u-cdc-2-6",
        "fresh",
        "fresh-target",
        &search,
        &refs,
    );
    assert_eq!(
        outcome.search_projection.as_deref(),
        Some("fresh"),
        "2.6: Search rebuilt from source"
    );
    assert_eq!(
        outcome.refs_target.as_deref(),
        Some("fresh-target"),
        "2.6: Refs rebuilt from source"
    );

    let search_provider = SearchIndexHolder::new(&search);
    let refs_provider = RefsGraphHolder::new(&refs);
    let sr = search_provider
        .rectify(&subject("u-cdc-2-6"), Patch("p".into()))
        .unwrap();
    let rr = refs_provider
        .rectify(&subject("u-cdc-2-6"), Patch("p".into()))
        .unwrap();
    assert_eq!(sr.receipt.operation, "rectify");
    assert_eq!(rr.receipt.operation, "rectify");
}
