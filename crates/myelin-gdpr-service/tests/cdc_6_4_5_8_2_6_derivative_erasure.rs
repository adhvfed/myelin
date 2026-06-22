//! # CDC 6.4 / 5.8 / 2.6 — the per-derivative erasure fan-out (P-GA-24 → P-151)
//!
//! **Contracts:** index rows **6.4** (Search `purge+reindex`), **5.8** (Refs `tombstone`), **2.6**
//! (`reindex-from-source`). The per-derivative erasure FAN-OUT (the orchestration leg of 10.1) wires
//! the derived-store holders as the orchestrator's per-holder erase calls + fans the reindex-from-
//! source rectification over them. This is the consumer-driven contract test the coverage scanner
//! (P-S21) reads both halves of:
//!
//! - **provider** = a DERIVED-store holder (Search [`SearchIndexHolder`] / Refs [`RefsGraphHolder`] /
//!   Notif [`NotifHistoryHolder`], the faithful M2 store doubles whose `erase` is the real purge /
//!   tombstone / humanise — and whose `rectify` is a reindex-from-source rebuild) IMPLEMENTING the
//!   contract — the store owns its `erase`/`rectify`; GDPR calls it.
//! - **consumer** = the [`DerivativeErasureDriver`] (the DSR orchestrator's per-derivative fan-out
//!   stage) CALLING the derived holders through the [`PersonalDataHolder`] contract — it NEVER
//!   reaches into a store (the no-cross-store-read law, gdpr §3.1).
//!
//! The dated green artifacts:
//! - **6.4** — the consumer fans `erase(Subject)` to Search (provider): the doc + embedding are
//!   purged (not hidden), 0 re-identification.
//! - **5.8** — the consumer fans `erase(Subject)` to Refs (provider): the edge tombstones, 0
//!   recoverable, no resolve-500.
//! - **2.6** — the consumer fans `rectify` via reindex-from-source to Search + Refs (providers): the
//!   derived projection rebuilds from source (drift = 0), never patched-in-place.
//!
//! If any of 6.4 / 5.8 / 2.6's shape drifts, this stops compiling/passing — that is the contract.

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

/// **6.4 (provider Search ⇄ consumer driver): `erase` = purge + reindex incl. embeddings.** The
/// consumer fans the erase to the Search provider through the contract; the doc + embedding are
/// purged (not hidden) — 0 re-identification. The receipt attests the contract was honoured.
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
    // The CONSUMER calls the provider via `dyn PersonalDataHolder` — never into the store.
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

/// **5.8 (provider Refs ⇄ consumer driver): `erase` = tombstone, 0 recoverable, no resolve-500.** The
/// consumer fans the erase to the Refs provider; the edge tombstones — a resolve returns the
/// tombstone, never a 500.
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

/// **NOTIF-D6 face (provider Notif ⇄ consumer driver): `erase` humanises mentions to `[erased
/// user]`.** The consumer fans the erase to the Notif provider; the mention humanises.
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

/// **2.6 (provider Search+Refs ⇄ consumer driver): `rectify` via reindex-from-source.** The consumer
/// rectifies the derived stores by REBUILDING from the corrected source — the derived projection
/// equals the rebuilt value (drift = 0), never patched-in-place. The providers' `rectify` receipts
/// attest the reindex-from-source posture.
#[test]
fn cdc_2_6_reindex_from_source_rectification() {
    let search = SearchIndexModel::new();
    let refs = RefsGraphModel::new();
    search.index_from_source("u-cdc-2-6", "stale");
    refs.add_edge_from_source("u-cdc-2-6", "stale-target");

    // The CONSUMER drives the reindex-from-source rebuild over the providers (no patch-in-place path).
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

    // The providers' `rectify` receipts attest the reindex-from-source posture (never patch-in-place).
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
