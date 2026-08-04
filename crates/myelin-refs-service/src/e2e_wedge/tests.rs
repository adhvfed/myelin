use super::*;
use myelin_events::{Actor, Timestamp};

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            e2e_tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        caused_by: None,
    }
}

#[test]
fn e2e_1_pr_pane_is_green_zero_leak() {
    let art = run_e2e_1_pr_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert!(art.is_green(), "E2E-1 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 title/count/backlink leak: {}",
        art.evidence
    );
}

#[test]
fn e2e_1_outsider_never_sees_confidential_title() {
    let art = run_e2e_1_pr_pane();
    assert!(
        art.evidence.contains("tombstone(denied)=true"),
        "the outsider's confidential issue must tombstone (denied): {}",
        art.evidence
    );
    assert_eq!(art.leaks, 0);
}

#[test]
fn e2e_3_spec_to_ship_is_green_zero_leak() {
    let art = run_e2e_3_spec_to_ship(ctx_base());
    assert_eq!(art.scenario, "E2E-3");
    assert!(art.is_green(), "E2E-3 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 leak through the traverse: {}",
        art.evidence
    );
}

#[test]
fn e2e_3_cold_reindex_byte_matches_live() {
    let art = run_e2e_3_spec_to_ship(ctx_base());
    assert!(
        art.evidence.contains("cold-reindex==live=true"),
        "the wiped index must reindex to byte-match live: {}",
        art.evidence
    );
}

#[test]
fn e2e_3_prune_drops_the_whole_unreadable_branch() {
    let art = run_e2e_3_spec_to_ship(ctx_base());
    assert!(
        art.evidence.contains("deploy_pruned=true") && art.evidence.contains("chat_pruned=true"),
        "the unreadable node AND its branch must be pruned: {}",
        art.evidence
    );
    assert_eq!(art.leaks, 0);
}

#[test]
fn e2e_4_dsar_fanout_is_green_zero_recoverable_pii() {
    let art = run_e2e_4_dsar_fanout();
    assert_eq!(art.scenario, "E2E-4");
    assert!(art.is_green(), "E2E-4 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 recoverable PII (incl. backups): {}",
        art.evidence
    );
}

#[test]
fn e2e_4_restore_is_non_vacuous() {
    let art = run_e2e_4_dsar_fanout();
    assert!(
        art.evidence.contains("restore resurrected"),
        "the restore must resurrect the PII before the re-erase re-shreds it: {}",
        art.evidence
    );
}

#[test]
fn whole_wedge_all_three_scenarios_green() {
    let artifacts = run_refs_e2e_wedge(ctx_base());
    assert_eq!(artifacts.len(), 3, "the wedge runs E2E-1/E2E-3/E2E-4");
    for (art, expected) in artifacts.iter().zip(E2E_SCENARIOS.iter()) {
        assert_eq!(&art.scenario, expected);
        assert!(
            art.is_green(),
            "{} must be green to complete R-M5: {}",
            art.scenario,
            art.evidence
        );
        assert_eq!(art.leaks, 0, "{} 0 leak", art.scenario);
    }
}

#[test]
fn e2e_scenarios_named() {
    assert_eq!(E2E_SCENARIOS, ["E2E-1", "E2E-3", "E2E-4"]);
}
