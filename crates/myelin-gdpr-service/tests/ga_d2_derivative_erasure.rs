use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
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
fn ga_d2_ref_d5_notif_d6_derivative_erasure_fan_out_is_green() {
    let search = SearchIndexModel::new();
    let refs = RefsGraphModel::new();
    let notif = NotifHistoryModel::new();

    search.index_from_source("victim", "victim@example.com");
    refs.add_edge_from_source("victim", "issue:99");
    notif.add_item_from_source("inbox-1", "victim");
    notif.add_item_from_source("inbox-2", "bystander");

    assert_eq!(search.hits("victim"), 1, "indexed before erase");
    assert_eq!(
        search.reidentify_hits("victim"),
        1,
        "embedding re-identifies before erase"
    );
    assert_eq!(refs.resolve("victim"), RefsResolve::Live("issue:99".into()));
    assert_eq!(notif.render_mention("inbox-1").as_deref(), Some("victim"));

    let sh = SearchIndexHolder::new(&search);
    let rh = RefsGraphHolder::new(&refs);
    let nh = NotifHistoryHolder::new(&notif);

    let receipt = DerivativeErasureDriver::fan_out_erase(
        &scope("victim"),
        &search,
        &sh as &dyn PersonalDataHolder,
        &refs,
        &rh as &dyn PersonalDataHolder,
        &notif,
        &nh as &dyn PersonalDataHolder,
    )
    .expect("the per-derivative fan-out succeeds");

    assert_eq!(search.hits("victim"), 0, "GA-D2: 0 search hits after purge");
    assert_eq!(
        search.reidentify_hits("victim"),
        0,
        "GA-D2: 0 embedding re-identification (purged, NOT hidden) - the measured number"
    );
    assert!(
        receipt.embeddings_purged,
        "GA-D2: the embedding-purge receipt records the purge"
    );

    assert_eq!(
        refs.resolve("victim"),
        RefsResolve::Tombstone,
        "REF-D5: a resolve returns the tombstone, NOT a 500"
    );
    assert_eq!(
        refs.recoverable_edges("victim"),
        0,
        "REF-D5: 0 recoverable edges"
    );
    assert!(
        receipt.refs_tombstoned,
        "REF-D5: the receipt records the tombstone"
    );

    assert_eq!(
        notif.render_mention("inbox-1").as_deref(),
        Some(ERASED_USER),
        "NOTIF-D6: the erased subject's mention humanises to [erased user]"
    );
    assert_eq!(
        notif.render_mention("inbox-1").as_deref(),
        Some("[erased user]")
    );
    assert_eq!(
        notif.render_mention("inbox-2").as_deref(),
        Some("bystander"),
        "a bystander's mention is untouched (only the erased subject humanises)"
    );

    assert_eq!(
        receipt.holder_receipts.len(),
        3,
        "Search + Refs + Notif receipts collected"
    );
    for hr in &receipt.holder_receipts {
        assert!(
            hr.receipt.content_hash.starts_with("blake3:"),
            "each derivative receipt is content-addressed"
        );
    }
}

#[test]
fn rectification_via_reindex_from_source_rebuilds_drift_is_zero() {
    let search = SearchIndexModel::new();
    let refs = RefsGraphModel::new();
    search.index_from_source("subj", "wrong name");
    refs.add_edge_from_source("subj", "wrong-target");

    let outcome = DerivativeErasureDriver::rectify_via_reindex_from_source(
        "subj",
        "corrected name",
        "corrected-target",
        &search,
        &refs,
    );

    assert_eq!(
        outcome.search_projection.as_deref(),
        Some("corrected name"),
        "Search reindexed from the corrected source"
    );
    assert_eq!(
        outcome.refs_target.as_deref(),
        Some("corrected-target"),
        "Refs rebuilt the edge from the corrected source"
    );
    assert_eq!(search.projection("subj").as_deref(), Some("corrected name"));
    assert_eq!(
        refs.resolve("subj"),
        RefsResolve::Live("corrected-target".into())
    );
}
