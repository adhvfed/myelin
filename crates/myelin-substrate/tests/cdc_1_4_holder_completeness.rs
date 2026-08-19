use myelin_events::relay::InProcessBus;
use myelin_events::OutboxStore;
use myelin_substrate::serve::{boot, AppSpec, OutboxSpec};
use myelin_substrate::{
    assert_holder_completeness, classify_store, holder_completeness, Config, CriticalDependencies,
    Holder, HotTables, InternalRpc, Migrations, OrphanStore, PublicRoutes, StoreClassifier,
    StoreHolder, StoreKind, StoreManifest,
};

fn spec(name: &'static str) -> AppSpec {
    AppSpec {
        name,
        config: Config::default(),
        migrations: Migrations::default(),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: vec![],
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::new(OutboxStore::new(), InProcessBus::new()),
        critical: CriticalDependencies::default(),
        intake_scope: None,
    }
}

#[test]
fn cdc_1_4_live_harness_opened_store_is_in_the_exhaustive_h_list() {
    let handle = boot(spec("issue_oltp")).expect("boot");
    let opened = handle.registered_holders();
    let classifier = StoreClassifier::of([StoreHolder::new(
        StoreKind::Oltp,
        "issue_oltp",
        Holder::H3Issues,
    )]);

    assert_eq!(
        assert_holder_completeness(opened, &classifier),
        Ok(()),
        "every store the harness opened classifies into the exhaustive H1–H18 set (no orphan)"
    );
    assert_eq!(
        classify_store(StoreKind::Oltp, "issue_oltp", &classifier),
        Some(Holder::H3Issues),
        "the opened OLTP store classifies to its §3.2 holder (H3 Issues)"
    );
}

#[test]
fn cdc_1_4_live_orphaned_store_fails_the_completeness_assertion() {
    let handle = boot(spec("rogue_oltp")).expect("boot");
    let opened = handle.registered_holders();
    let classifier = StoreClassifier::new();

    let orphans = holder_completeness(opened, &classifier);
    assert_eq!(
        orphans,
        vec![OrphanStore {
            kind: StoreKind::Oltp,
            name: "rogue_oltp".into()
        }],
        "the OLTP store opened with no declared holder is the orphan"
    );
    let err = assert_holder_completeness(opened, &classifier)
        .expect_err("a store outside the exhaustive H1–H18 list must FAIL");
    let msg = err[0].message();
    assert!(msg.contains("rogue_oltp"), "names the orphan store: {msg}");
    assert!(msg.contains("H1–H18"), "names the exhaustive list: {msg}");
}

#[test]
fn cdc_1_4_all_four_store_kinds_classify_into_the_exhaustive_list() {
    use myelin_substrate::HolderRegistry;
    let mut reg = HolderRegistry::new();
    reg.open(StoreKind::Oltp, "svc_oltp");
    reg.open(StoreKind::Blob, "svc_blobs");
    reg.open(StoreKind::Cache, "svc_cache");
    reg.open(StoreKind::SearchIndex, "svc_index");
    let classifier = StoreClassifier::of([StoreHolder::new(
        StoreKind::Oltp,
        "svc_oltp",
        Holder::H4Knowledge,
    )]);

    assert_eq!(
        assert_holder_completeness(reg.registrations(), &classifier),
        Ok(()),
        "all four §3.4 store kinds classify into the H1–H18 set - no orphan"
    );
    assert_eq!(
        classify_store(StoreKind::Blob, "svc_blobs", &classifier),
        Some(Holder::H6BlobStore)
    );
    assert_eq!(
        classify_store(StoreKind::Cache, "svc_cache", &classifier),
        Some(Holder::H9Caches)
    );
    assert_eq!(
        classify_store(StoreKind::SearchIndex, "svc_index", &classifier),
        Some(Holder::H7SearchIndex)
    );
}

#[test]
fn cdc_1_4_catalog_is_exhaustive_eighteen() {
    assert_eq!(
        Holder::ALL.len(),
        18,
        "the §3.2 holder list is exhaustive: H1–H18"
    );
    for (i, h) in Holder::ALL.iter().enumerate() {
        assert_eq!(
            h.tag(),
            format!("H{}", i + 1),
            "the catalog names H{} in order",
            i + 1
        );
    }
}
