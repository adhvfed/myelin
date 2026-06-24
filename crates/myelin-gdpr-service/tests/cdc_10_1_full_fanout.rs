//! # CDC — contract 10.1 (the full H1–H18 DSR fan-out completeness) — PROVIDER side
//!
//! P-GA-32 → P-448. The full-fan-out completeness of contract 10.1 is OWNED in
//! `myelin-gdpr-service::full_fanout`. This CDC pins the PROVIDER side of the catalogue contract the
//! storage-side CDC (`myelin-storage/tests/cdc_e2e4_holder_coverage.rs`) consumes:
//!   - [`Holder::ALL`] is the EXHAUSTIVE H1–H18 set (18 holders, labelled H1..H18, distinct ids);
//!   - every holder id round-trips through [`Holder::from_id`] (the data-map id → H-class resolver the
//!     fan-out completeness layer depends on);
//!   - the completeness measure's denominator is the WHOLE catalogue (a partial reach is NOT vacuously
//!     100% — the load-bearing GA-D1 zero, EI-01 §2).
//!
//! The CONSUMER (`myelin-storage`) asserts its D-S5 catalogue covers the storage-owned subset of these
//! ids — so the two H1–H18 catalogues (gdpr §3.2 numbering + storage D-S5 numbering) describe the same
//! real holders. `myelin-gdpr-service` does NOT depend on `myelin-storage`; the cross-crate agreement
//! is asserted from the storage side (the dev-only edge, no cycle — coherence EI-01 §7).

use myelin_gdpr_service::full_fanout::{
    FullFanOutCoverage, GaD1Certificate, Holder, HolderErasure,
};

/// **PROVIDER: the catalogue is the exhaustive H1–H18 set** — 18 holders, labelled H1..H18 in order,
/// distinct store ids, distinct labels. The contract the storage CDC's "both catalogues are 18" pins.
#[test]
fn cdc_catalogue_is_exhaustive_h1_h18() {
    assert_eq!(Holder::ALL.len(), 18);
    let labels: Vec<&str> = Holder::ALL.iter().map(|h| h.h_label()).collect();
    let expected: Vec<String> = (1..=18).map(|n| format!("H{n}")).collect();
    assert_eq!(
        labels,
        expected.iter().map(|s| s.as_str()).collect::<Vec<_>>()
    );
    let ids: std::collections::BTreeSet<&str> = Holder::ALL.iter().map(|h| h.holder_id()).collect();
    assert_eq!(ids.len(), 18, "18 distinct holder ids");
}

/// **PROVIDER: every holder id resolves to exactly its H-class** — the round-trip the consumer's
/// "resolves to exactly one storage HolderClass" CDC relies on.
#[test]
fn cdc_every_holder_id_round_trips() {
    for &h in Holder::ALL {
        assert_eq!(
            Holder::from_id(h.holder_id()),
            Some(h),
            "{} round-trips",
            h.h_label()
        );
    }
}

/// **PROVIDER: the completeness denominator is the WHOLE catalogue** — a single-holder reach is 1/18,
/// NOT vacuously 100% (the orchestration-subset trap the M5 gate closes). This is the load-bearing
/// distinction the GA-D1 gate (0 holders missed) rests on.
#[test]
fn cdc_completeness_denominator_is_the_whole_catalogue() {
    let mut cov = FullFanOutCoverage::new();
    cov.record_reached(Holder::Identity);
    assert!((cov.erasure_fanout_coverage() - 1.0 / 18.0).abs() < 1e-12);
    assert_eq!(cov.holders_missed(), 17);
    assert!(!cov.is_complete());
}

/// **PROVIDER: the certificate is the green artifact — it seals only on a complete fan-out.** A missed
/// holder yields a GAP (the gate reading the storage half's `holders_missed == 0` mirrors).
#[test]
fn cdc_certificate_seals_only_when_complete() {
    let mut full = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        full.record_reached(h);
    }
    assert!(
        GaD1Certificate::seal("acme/u", &full).is_ok(),
        "complete → seals"
    );

    let mut partial = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        if h != Holder::Backups {
            partial.record_reached(h);
        }
    }
    let gap = GaD1Certificate::seal("acme/u", &partial).expect_err("incomplete → gap");
    assert_eq!(gap.missed, vec![Holder::Backups]);
}

/// **PROVIDER: every holder has a defined erasure modality (§3.2 column 4)** — the routing is total
/// (no holder is left without a mechanism). The carve-out (H16) is the documented residual; the search
/// index (H7) is purge-and-reindex (NOT key-shred); the backup tier (H10) is by construction.
#[test]
fn cdc_erasure_modality_is_total_and_correct() {
    for &h in Holder::ALL {
        let _ = h.erasure(); // the exhaustive match guarantees a modality for every holder.
    }
    assert_eq!(
        Holder::AuditCarveOut.erasure(),
        HolderErasure::AuditCarveOutResidual
    );
    assert_eq!(
        Holder::SearchIndex.erasure(),
        HolderErasure::PurgeAndReindex
    );
    assert_eq!(
        Holder::Backups.erasure(),
        HolderErasure::CryptoShredByConstruction
    );
    assert_eq!(
        Holder::Identity.erasure(),
        HolderErasure::DeletePseudonymMapAndShredProfile
    );
}
