use myelin_gdpr_service::full_fanout::{
    FullFanOutCoverage, GaD1Certificate, Holder, HolderErasure,
};

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

#[test]
fn cdc_completeness_denominator_is_the_whole_catalogue() {
    let mut cov = FullFanOutCoverage::new();
    cov.record_reached(Holder::Identity);
    assert!((cov.erasure_fanout_coverage() - 1.0 / 18.0).abs() < 1e-12);
    assert_eq!(cov.holders_missed(), 17);
    assert!(!cov.is_complete());
}

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

#[test]
fn cdc_erasure_modality_is_total_and_correct() {
    for &h in Holder::ALL {
        let _ = h.erasure();
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
