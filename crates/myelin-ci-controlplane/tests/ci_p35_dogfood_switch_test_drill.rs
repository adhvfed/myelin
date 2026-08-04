use myelin_ci_controlplane::{
    proven_ci_rows, switch_capability_matrix, CiIncident, CiSwitchTest, CiTruthUpPass,
    IncidentDrillLoop, SwitchVerdict,
};
use myelin_substrate::thresholds::Thresholds;

fn today() -> String {
    "2026-06-25".to_string()
}

fn render_budget_us() -> u64 {
    let t = Thresholds::load_canonical().expect("load thresholds.toml");
    assert!(
        t.ci_switch_test.is_well_formed(),
        "the CI switch-test budget is well-formed (a positive render budget - no vacuous bar)"
    );
    t.ci_switch_test.render_budget_us
}

#[test]
fn ci_p35_dogfood_switch_test_and_truth_up_pass_green() {
    let date = today();

    let caps = switch_capability_matrix();
    assert!(!caps.is_empty(), "the capability matrix is non-empty");
    let mut caps = caps;
    caps.push(myelin_ci_controlplane::SwitchCapability {
        id: "ci-local",
        anchor_feature: "act (third-party; no first-party local runner)",
        myelin_surface: "myelin ci local (deferred-by-design named floor - arch 04 §2)",
        reached_by_driving: false,
        deferred_named_floor: true,
    });

    let measured_render_us: u64 = 1_200;
    let budget = render_budget_us();

    let switch = CiSwitchTest::new(caps.clone(), measured_render_us, budget);
    let verdict = switch.verdict();
    match &verdict {
        SwitchVerdict::Pass {
            reached,
            measured_render_us: m,
            budget_render_us: b,
            deferred_floors,
        } => {
            assert_eq!(
                *reached,
                caps.iter().filter(|c| c.reached_by_driving).count(),
                "every driven capability reached"
            );
            assert!(
                *m <= *b,
                "the measured render is within the thresholds-file budget"
            );
            assert_eq!(
                deferred_floors,
                &["ci-local"],
                "the `myelin ci local` floor is RECORDED in the gap report, never a wall"
            );
        }
        SwitchVerdict::Red {
            walls,
            latency_over_budget,
        } => panic!(
            "the CI switch test RED - walls: {walls:?}, latency_over_budget: {latency_over_budget} \
             (a migrating user would hit a wall the old tool didn't have)"
        ),
    }
    assert!(
        verdict.is_pass(),
        "the CI switch test PASSES (driven, measured)"
    );
    println!(
        "[{date}] CI switch test PASS - {} capabilities reached by driving the real `myelin ci` \
         run/log/deploy surface; render {measured_render_us}µs ≤ budget {budget}µs (thresholds-file); \
         seal {}",
        caps.iter().filter(|c| c.reached_by_driving).count(),
        switch.seal()
    );

    let rows = proven_ci_rows(&date);
    assert!(!rows.is_empty(), "the PROVEN CI set is non-empty");
    let confirmed = CiTruthUpPass::new()
        .run_or_fail_ci(&rows, &date)
        .expect("every PROVEN CI row rests on a dated green artifact (no later-band CI gate red)");
    assert_eq!(
        confirmed,
        rows.len(),
        "every PROVEN CI row confirmed dated-green"
    );
    println!(
        "[{date}] CI truth-up PASS - {confirmed} PROVEN CI rows each rest on a dated green artifact \
         (0 red earlier CI gates)"
    );

    let mut incidents = IncidentDrillLoop::new();
    incidents.record(CiIncident {
        id: "INC-self-hosting-graph-bootstrap",
        issue_ref: Some("myelin://myelin/issues/issue/CI-DOGFOOD-1"),
        repro_drill_id: Some("self_hosting_ci_dogfood"),
    });
    assert!(
        incidents.is_satisfied(),
        "the every-incident-adds-a-drill loop is SATISFIED self-hosted ({:?})",
        incidents.unguarded_incidents()
    );
    println!(
        "[{date}] every-incident-adds-a-drill loop SELF-HOSTED - {} incident(s), 0 unguarded",
        incidents.incidents().len()
    );
}

#[test]
fn ci_p35_dogfood_gate_is_not_vacuous() {
    let mut caps = switch_capability_matrix();
    caps[0].reached_by_driving = false;
    caps[0].deferred_named_floor = false;
    let verdict = CiSwitchTest::new(caps, 1_200, render_budget_us()).verdict();
    assert!(
        !verdict.is_pass(),
        "a wall reds the switch test (non-vacuous)"
    );
    assert!(!verdict.walls().is_empty(), "the wall is named");

    let mut rows = proven_ci_rows(&today());
    rows[0].artifact_date = None;
    let err = CiTruthUpPass::new()
        .run_or_fail_ci(&rows, &today())
        .expect_err("an undated PROVEN row reds the truth-up pass (non-vacuous)");
    assert!(
        !err.undated_rows.is_empty(),
        "the claimed-not-proven row is named"
    );

    let mut incidents = IncidentDrillLoop::new();
    incidents.record(CiIncident {
        id: "INC-no-drill",
        issue_ref: Some("myelin://myelin/issues/issue/X"),
        repro_drill_id: None,
    });
    assert!(
        !incidents.is_satisfied(),
        "a drill-less incident reds the loop (non-vacuous)"
    );
    assert_eq!(incidents.unguarded_incidents(), vec!["INC-no-drill"]);
}
