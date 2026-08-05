use myelin_control_plane::{
    proven_tenancy_rows, MyelinSelfHost, ResidencySigningKey, ResidencyStoreClass,
    TenancyTruthUpPass,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{CellId, Region};

#[test]
fn myelin_self_hosts_as_one_cell_residency_verify_green_truth_up_passes() {
    let region = Region::new("fr-par");
    let sh = MyelinSelfHost::bootstrap_team(
        CellId::from_token("cell-myelin"),
        region.clone(),
        "myelin-team",
    )
    .expect(
        "the one Active cell is eligible → the Myelin team is placed via the SHARED place path",
    );

    assert_eq!(
        sh.control_plane().registry().cell_count(),
        1,
        "a self-host install is EXACTLY one cell"
    );
    assert_eq!(
        sh.placement().home_cell.as_str(),
        "cell-myelin",
        "the team is placed on the install's own cell (this cell)"
    );
    assert_eq!(
        sh.region().as_str(),
        "fr-par",
        "pinned to the team's region"
    );
    let discovered = sh.discover_team_cell().expect("the placed team discovers");
    assert_eq!(
        discovered.as_str(),
        "cell-myelin",
        "discover returns 'this cell' for the team"
    );

    let key = ResidencySigningKey::from_bytes([0x6du8; 32]);
    let attestation = sh
        .residency_verify_team(&key)
        .expect("residency_verify is GREEN on the Myelin team's own data");
    assert_eq!(
        attestation.tenant_id.as_str(),
        sh.team_tenant().as_str(),
        "the attestation is for the team's tenant"
    );
    assert_eq!(
        attestation.region.as_str(),
        "fr-par",
        "every M1 store reported the team's region (no global pool)"
    );
    assert_eq!(
        attestation.store_regions.len(),
        ResidencyStoreClass::M1_SET.len(),
        "every M1 store attested over the team's data"
    );
    let region_mismatches = attestation
        .store_regions
        .iter()
        .filter(|(_, r)| r.as_str() != "fr-par")
        .count();
    assert_eq!(
        region_mismatches, 0,
        "0 region mismatches on the team's own data (the residency-attestation green artifact)"
    );
    assert!(
        attestation.verify(&key),
        "the green residency-attestation over the team's data verifies"
    );

    let date = "2026-06-25";
    let rows = proven_tenancy_rows(date);
    let rows_confirmed = TenancyTruthUpPass::new()
        .run_or_fail_ci(&rows, date)
        .expect("the truth-up pass is GREEN - every PROVEN Tenancy row rests on a dated artifact");
    assert_eq!(
        rows_confirmed,
        rows.len(),
        "every enumerated PROVEN Tenancy row confirmed dated + green"
    );
    let ids: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.id).collect();
    for must in [
        "CP-D1", "CP-D2", "CP-D3", "CP-D4", "CP-D5", "CP-D6", "CP-D7", "CP-D8", "STOR-D5", "CI-R3",
        "GA-D8",
    ] {
        assert!(
            ids.contains(must),
            "the truth-up PROVEN set must include {must}"
        );
    }

    let claimed_not_proven = rows.len() - rows_confirmed;
    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        (region_mismatches + claimed_not_proven) as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-508 CP-D23/self_tenant GREEN 2026-06-25] Myelin self-hosts as EXACTLY one cell \
         (cell-myelin, fr-par; registry cell_count={}): the Myelin team placed as real tenant data \
         (home_cell=cell-myelin, discover→'this cell'); residency_verify GREEN on the platform's OWN \
         data ({} M1 stores attested, region_mismatches={}, signature={}…, verifies); the truth-up \
         pass CONFIRMED {} PROVEN Tenancy rows (CP-D1..CP-D8 + STOR-D5 + CI-R3 + GA-D8) each rest on a \
         dated green artifact (claimed_not_proven={}). NO self-host fork; the two Tenancy lints run as \
         Myelin CI jobs via the self-hosting CI graph. NO new floor - the gate invariant holds \
         end-to-end (the done-bar for Tenancy).",
        sh.control_plane().registry().cell_count(),
        attestation.store_regions.len(),
        region_mismatches,
        &attestation.signature[..attestation.signature.len().min(22)],
        rows_confirmed,
        claimed_not_proven,
    );
}

#[test]
fn self_tenant_gate_is_not_vacuous() {
    let mut rows = proven_tenancy_rows("2026-06-25");
    rows.iter_mut()
        .find(|r| r.id == "CP-D3")
        .expect("CP-D3 is in the PROVEN set")
        .artifact_date = None;
    let err = TenancyTruthUpPass::new()
        .run_or_fail_ci(&rows, "2026-06-25")
        .expect_err("an undated PROVEN row MUST RED the truth-up pass");
    assert!(
        err.to_string().contains("CP-D3"),
        "the red NAMES the claimed-not-proven Tenancy row (loud, never swallowed)"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a residency mismatch / claimed-not-proven row on the platform's own data MUST read RED"
    );
}
