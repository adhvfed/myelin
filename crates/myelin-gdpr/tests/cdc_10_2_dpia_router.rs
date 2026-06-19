//! # CDC 10.2 (SpecialCategory-marker leg) — the `SpecialCategory` → DPIA router (P-GA-08 → P-108)
//!
//! **Contract:** index row 10.2 — the `SpecialCategory`→DPIA marker leg of the classify-derive. A
//! field tagged `category = SpecialCategory(<kind>)` emits a DPIA MARKER into the generated
//! inventory; the DPIA ROUTER consumes the marker set and records a newly-appeared marker as a
//! DPIA-required change. This is the consumer-driven contract test the coverage scanner (P-S21)
//! reads both halves of:
//!
//! - **provider** = a schema struct that DERIVES `#[derive(PersonalData)]` and tags a field
//!   `category = SpecialCategory(...)`. The derive (P-107) emits the registry entry; `DpiaMarker::
//!   from_field` (P-108) mints the marker off it. The provider is the SOURCE of the special-category
//!   marker the data map commits (gdpr §2.3).
//! - **consumer** = the DPIA router (`DpiaRouter`, the shape the data-map diff gate P-GA-10 drives):
//!   fed the prior committed marker set + the current build's, it routes each newly-appeared marker
//!   to a `DpiaVerdict::Required` — the Art. 35 obligation, surfaced for a DPO, never auto-decided.
//!   The diff-gate plumbing that commits + compares the inventory is P-GA-09/P-GA-10; this test
//!   exercises the marker→router leg in isolation (the consumer the diff gate will call).
//!
//! The dated green artifact: from the derive's emitted registry alone, a new special-category flow
//! routes deterministically into the DPIA gate, and an unchanged flow does not. If the marker shape
//! or the routing semantics drift, this stops compiling/passing — that is the contract.

use myelin_gdpr::{
    dpia_markers, DpiaMarker, DpiaRouter, DpiaVerdict, HasPersonalData, PersonalData,
};
use std::collections::BTreeSet;

// ── The PROVIDER side: a schema struct whose derive feeds the special-category marker ────────────

/// A provider schema row carrying an Art. 9 special-category field (the DPIA route) alongside an
/// ordinary-category field (no DPIA obligation). The derive emits the registry; the marker is minted
/// off the special-category entry only.
#[derive(PersonalData)]
#[allow(dead_code)]
struct ProfileRow {
    /// ordinary contact PII — NOT a DPIA route (proves the marker is special-category-only).
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    email: String,
    /// an Art. 9 special-category field — the DPIA route (the marker the router consumes).
    #[personal_data(
        category = SpecialCategory(health),
        role = PlatformOperational,
        basis = Consent(c-7),
        retention = Fixed(365d),
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    health_note: String,
}

#[test]
fn dpia_provider_emits_the_marker_the_router_consumes() {
    // PROVIDER: the derive's registry → the special-category marker set (the data-map contribution).
    let current: BTreeSet<DpiaMarker> = dpia_markers::<ProfileRow>();
    assert_eq!(current.len(), 1, "exactly the one special-category field emits a marker");
    let marker = current.iter().next().unwrap();
    assert_eq!(marker.field_path, "ProfileRow.health_note");
    assert_eq!(marker.special_category_kind, "health");

    // CONSUMER: the DPIA router routes the new flow (prior = empty: a fresh data map) into the gate.
    let router = DpiaRouter::new();
    let verdicts = router.route(&BTreeSet::new(), &current);
    assert_eq!(verdicts.len(), 1, "the new special-category flow fires the DPIA gate");
    match &verdicts[0] {
        DpiaVerdict::Required { marker, reason } => {
            assert_eq!(marker.field_path, "ProfileRow.health_note");
            assert!(reason.contains("DPIA required"));
            // Surfaced for a human/DPO call, NOT auto-decided (gdpr §2.3).
            assert!(reason.contains("DPO"));
        }
    }

    // The diff is the trigger: routing the SAME map against itself yields no new obligation (an
    // already-adjudicated flow does not re-fire — only an inventory DIFF fires the gate).
    assert!(router.route(&current, &current).is_empty());

    // The 100% property, structural: the marker count equals the special-category field count.
    let special_count = ProfileRow::personal_data_fields()
        .iter()
        .filter(|f| f.is_special_category().is_some())
        .count();
    assert_eq!(current.len(), special_count);
}
