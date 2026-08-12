use std::collections::BTreeSet;

use myelin_gdpr_service::full_fanout::{FullFanOutCoverage, GaD1Certificate, Holder};

fn complete_h1_h18_map_roster() -> Vec<&'static str> {
    vec![
        "oltp:git_oltp",
        "oltp:ci_oltp",
        "oltp:issue_oltp",
        "oltp:knowledge_oltp",
        "oltp:chat_oltp",
        "blob:blob_store",
        "search_index:search_index",
        "event_bus",
        "cache_cdn",
        "backups",
        "agent_memory",
        "refs_edge:refs_edge",
        "notif_history",
        "authz_tuples",
        "identity",
        "audit_carve_out",
        "oltp:agent_fabric_trace",
        "gdpr_own_stores",
    ]
}

fn drive_from_roster(roster: &[&str]) -> FullFanOutCoverage {
    let mut cov = FullFanOutCoverage::new();
    for id in roster {
        cov.record_reached_id(id);
    }
    cov
}

#[test]
fn ga_d1_full_h1_h18_fan_out_reaches_every_holder_0_missed() {
    let roster = complete_h1_h18_map_roster();
    assert_eq!(roster.len(), 18, "the complete map declares all 18 holders");

    let cov = drive_from_roster(&roster);

    assert_eq!(
        cov.holders_missed(),
        0,
        "GA-D1: 0 holders missed (incl. all five subsystems)"
    );
    assert_eq!(
        cov.erasure_fanout_coverage(),
        1.0,
        "GA-D1: erasure_fanout_coverage == 100% over the WHOLE H1–H18 set"
    );
    assert!(cov.is_complete(), "the fan-out reached every H1–H18 holder");
    assert!(cov.missed().is_empty(), "no holder named as missed");
    assert!(
        cov.unrecognised().is_empty(),
        "every roster id resolved to a known holder"
    );

    let manifest = cov.reach_manifest();
    assert_eq!(manifest.len(), 18, "one reach line per H1–H18 holder");
    for line in &manifest {
        assert!(
            line.reached,
            "{} was reached by the fan-out",
            line.holder.h_label()
        );
    }
    for h in [
        Holder::GitDb,
        Holder::CiDb,
        Holder::IssuesDb,
        Holder::KnowledgeDb,
        Holder::ChatDb,
    ] {
        assert!(
            cov.reach_manifest()
                .iter()
                .any(|r| r.holder == h && r.reached),
            "{} reached",
            h.h_label()
        );
    }

    let cert = GaD1Certificate::seal("acme/subject-cell-scale", &cov)
        .expect("a complete H1–H18 fan-out seals the GA-D1 certificate");
    assert!(cert.is_complete());
    assert_eq!(cert.holders_missed, 0);
    assert_eq!(cert.erasure_fanout_coverage, 1.0);
    assert_eq!(cert.reach.len(), 18);
    assert!(
        cert.content_hash.starts_with("blake3:"),
        "Merkle-provable leaf (the inclusion rides P-GA-20)"
    );
}

#[test]
fn ga_d1_a_missed_holder_is_detected_the_certificate_refuses_to_seal() {
    let roster: Vec<&str> = complete_h1_h18_map_roster()
        .into_iter()
        .filter(|id| Holder::from_id(id) != Some(Holder::SearchIndex))
        .collect();
    assert_eq!(roster.len(), 17, "one holder withheld");

    let cov = drive_from_roster(&roster);
    assert_eq!(
        cov.holders_missed(),
        1,
        "the missed holder is COUNTED (not masked as 100%)"
    );
    assert_eq!(cov.missed(), vec![Holder::SearchIndex], "named: H7 Search");
    assert!(
        cov.erasure_fanout_coverage() < 1.0,
        "coverage dropped below 1.0"
    );
    assert!(!cov.is_complete());

    let gap = GaD1Certificate::seal("acme/subject-cell-scale", &cov)
        .expect_err("a missed holder does NOT seal the GA-D1 certificate");
    assert_eq!(gap.holders_missed, 1);
    assert_eq!(gap.missed, vec![Holder::SearchIndex]);
}

#[test]
fn ga_d1_dropping_any_single_holder_is_detected() {
    for &dropped in Holder::ALL {
        let roster: Vec<&str> = complete_h1_h18_map_roster()
            .into_iter()
            .filter(|id| Holder::from_id(id) != Some(dropped))
            .collect();
        let cov = drive_from_roster(&roster);
        assert_eq!(
            cov.holders_missed(),
            1,
            "dropping {} is detected as exactly one missed holder",
            dropped.h_label()
        );
        assert_eq!(
            cov.missed(),
            vec![dropped],
            "the dropped holder {} is named",
            dropped.h_label()
        );
        assert!(
            GaD1Certificate::seal("acme/s", &cov).is_err(),
            "dropping {} blocks the certificate seal",
            dropped.h_label()
        );
    }
}

#[test]
fn ga_d1_reach_manifest_is_exactly_the_h1_h18_catalogue() {
    let cov = drive_from_roster(&complete_h1_h18_map_roster());
    let manifest_holders: BTreeSet<Holder> =
        cov.reach_manifest().iter().map(|r| r.holder).collect();
    let catalogue: BTreeSet<Holder> = Holder::ALL.iter().copied().collect();
    assert_eq!(
        manifest_holders, catalogue,
        "the manifest is exactly H1–H18"
    );
}
