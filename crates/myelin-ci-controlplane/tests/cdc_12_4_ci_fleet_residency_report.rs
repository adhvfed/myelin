use myelin_ci_controlplane::{EuFleetProvider, FleetResidencyReport, GenericEuIaasAdapter};
use myelin_ci_sandbox::Region;

fn fr_par() -> Region {
    Region::new("fr-par")
}
fn eu_north() -> Region {
    Region::new("eu-north")
}

#[test]
fn provider_the_fleet_reports_its_runner_pool_region() {
    let fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
    let report = fleet.residency_report();
    assert_eq!(
        report.tenant_id, "01J0ACME",
        "the pool's tenant (opaque id)"
    );
    assert_eq!(
        report.region,
        fr_par(),
        "the pool's residency-pinned region"
    );
}

#[test]
fn consumer_a_wrong_region_report_fails_the_match_of_record() {
    let region_of_record = fr_par();

    let in_region = FleetResidencyReport {
        tenant_id: "01J0ACME".into(),
        region: fr_par(),
    };
    assert!(
        in_region.matches_region_of_record(&region_of_record),
        "an in-region fleet report passes the no-global-pool attestation"
    );

    let cross_region = FleetResidencyReport {
        tenant_id: "01J0ACME".into(),
        region: eu_north(),
    };
    assert!(
        !cross_region.matches_region_of_record(&region_of_record),
        "a wrong-region fleet report FAILs the attestation - the no-global-pool breach is caught"
    );
}

#[test]
fn provider_and_consumer_agree_on_the_report_pair() {
    let fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
    let report = fleet.residency_report();

    assert!(
        report.matches_region_of_record(fleet.cell_region()),
        "the fleet's report region IS its pinned cell region - the provider and the consumer agree"
    );
    assert_eq!(
        report.tenant_id,
        fleet.tenant_id(),
        "the report tenant IS the pool's tenant - no drift between provider and consumer"
    );
}
