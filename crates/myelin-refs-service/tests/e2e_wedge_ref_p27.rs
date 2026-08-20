use myelin_events::{Actor, EmitContextBase, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout, run_refs_e2e_wedge,
    E2E_SCENARIOS,
};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        caused_by: None,
    }
}

#[test]
fn e2e_1_pr_pane_green_refs_is_the_spine() {
    let art = run_e2e_1_pr_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert!(
        art.is_green(),
        "E2E-1 (the PR pane - Refs is the spine) must be green: {}",
        art.evidence
    );
    assert_eq!(
        art.leaks, 0,
        "E2E-1: 0 title/count/backlink leak - {}",
        art.evidence
    );
    assert!(art.evidence.contains("tombstone(denied)=true"));
    assert!(art.evidence.contains("live-updated=true"));
}

#[test]
fn e2e_3_spec_to_ship_green_lineage_and_reindex_parity() {
    let art = run_e2e_3_spec_to_ship(ctx_base());
    assert_eq!(art.scenario, "E2E-3");
    assert!(
        art.is_green(),
        "E2E-3 (the lineage traverse + reindex parity) must be green: {}",
        art.evidence
    );
    assert_eq!(
        art.leaks, 0,
        "E2E-3: 0 leak through the traverse - {}",
        art.evidence
    );
    assert!(art.evidence.contains("cold-reindex==live=true"));
    assert!(art.evidence.contains("cycle_surfaced=true"));
}

#[test]
fn e2e_4_dsar_fanout_green_zero_recoverable_pii_incl_backups() {
    let art = run_e2e_4_dsar_fanout();
    assert_eq!(art.scenario, "E2E-4");
    assert!(
        art.is_green(),
        "E2E-4 (the DSAR fan-out - Refs edges + cache return 0 recoverable PII) must be green: {}",
        art.evidence
    );
    assert_eq!(
        art.leaks, 0,
        "E2E-4: 0 recoverable PII incl. backups - {}",
        art.evidence
    );
    assert!(art.evidence.contains("restore resurrected"));
}

#[test]
fn whole_refs_e2e_wedge_completes_r_m5() {
    let artifacts = run_refs_e2e_wedge(ctx_base());
    assert_eq!(artifacts.len(), 3, "the wedge runs E2E-1/E2E-3/E2E-4");
    for (art, expected) in artifacts.iter().zip(E2E_SCENARIOS.iter()) {
        assert_eq!(&art.scenario, expected, "scenario order is -1/-3/-4");
        assert!(
            art.is_green(),
            "{} must be green to complete R-M5: {}",
            art.scenario,
            art.evidence
        );
        assert_eq!(art.leaks, 0, "{}: 0 leak at E2E scale", art.scenario);
    }
}
