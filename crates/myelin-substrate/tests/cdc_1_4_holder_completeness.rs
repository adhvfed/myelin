//! **CDC 1.4 (P-S27 confirmation) — the exhaustive `PersonalDataHolder` (H1–H18) completeness
//! assertion against the REAL holder set** (P-S27 → global P-088). Contract-index row 1.4 (the
//! exhaustive-holder mechanism against the real H1–H18 set — CONFIRMED) + 10.1 (the H1–H18 list —
//! GDPR owns it; the substrate confirms registration completeness).
//!
//! Architecture: `gdpr-and-audit.md §3.2` (the EXHAUSTIVE holder list H1–H18 — "the list is
//! exhaustive and enforced by the data map") + `00-platform-substrate.md §3.4` (auto-register
//! every store the harness opens).
//!
//! ## What this CDC confirmation proves (the substrate's half of contract 1.4 / 10.1)
//! P-S15 proved a harness-opened store REGISTERS; P-GA-04 proved a store opened OUTSIDE the harness
//! FAILS the `holder-registered` test. P-S27 confirms the THIRD §3.2 property: every store the
//! harness opens maps to one of the EIGHTEEN named holders (H1–H18) — **no orphan store outside the
//! list** — and a deliberately-orphaned store (one opened without a holder classification) fails.
//! - **PROVIDER (each store):** a service boots through `serve`; the auto-registration mechanism
//!   opens every store through the one door, producing the `HolderRegistration` receipts.
//! - **CONSUMER (the DSR fan-out / the §3.2 data map):** the holder-completeness assertion joins
//!   those receipts against the exhaustive H1–H18 catalog: every opened store classifies to an
//!   H-holder, or it is an orphan (a store the RoPA inventory never accounted for — a GDPR +
//!   data-loss hole, EI-01 §2).

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
    }
}

/// **CDC 1.4 (confirmation) — every store the LIVE harness opens is in the H1–H18 set.** A service
/// boots through `serve`; its OLTP store auto-registers; with the OLTP store declared as its
/// H-holder (here H3 Issues), the holder-completeness assertion over the registry's REAL
/// receipts is green — the store the harness opened is accounted for in the exhaustive §3.2 list.
#[test]
fn cdc_1_4_live_harness_opened_store_is_in_the_exhaustive_h_list() {
    let handle = boot(spec("issue_oltp")).expect("boot");
    // the auto-registration mechanism produced the receipts for the opened stores.
    let opened = handle.registered_holders();
    // the service declares its OLTP store's H-holder (gdpr §3.2 — the per-subsystem assignment).
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

/// **CDC 1.4 (confirmation, RED) — a deliberately-orphaned store fails.** The same live boot, but
/// the service declares NO H-holder for its OLTP store — so the store the harness opened maps to
/// none of the eighteen. The completeness assertion FAILS, naming the orphan: a store outside the
/// exhaustive §3.2 list is a build failure (it would escape the DSR fan-out + the data map).
#[test]
fn cdc_1_4_live_orphaned_store_fails_the_completeness_assertion() {
    let handle = boot(spec("rogue_oltp")).expect("boot");
    let opened = handle.registered_holders();
    let classifier = StoreClassifier::new(); // the service forgot to declare its store's holder.

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

/// **CDC 1.4 (confirmation) — the four §3.4 store kinds all classify into the H1–H18 set.** A
/// service that opens an OLTP schema + a blob prefix + a cache namespace + a search index: the OLTP
/// store declares its holder (H3), and the three non-OLTP kinds classify STRUCTURALLY to their
/// single platform-wide holders (blob→H6, cache→H9, search→H7). No store kind escapes the list.
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
        "all four §3.4 store kinds classify into the H1–H18 set — no orphan"
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

/// **CDC 1.4 — the catalog is EXHAUSTIVE (eighteen holders, the drift guard).** The substrate
/// catalog mirrors the GDPR-owned §3.2 list; it names exactly H1–H18. A holder added/removed
/// without a §3.2 co-edit is a loud failure, so the two can never silently diverge.
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
