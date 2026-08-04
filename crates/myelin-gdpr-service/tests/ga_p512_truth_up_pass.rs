use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_gdpr_service::dogfood::{
    proven_gdpr_rows, run_truth_up_scorecard, RowStatus, TruthUpPass, TruthUpScorecard,
    TRUTH_UP_FULL_PASS_PROMPT,
};

const RUN_DATE: &str = "2026-06-26";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root is two levels above the crate manifest")
        .to_path_buf()
}

#[test]
fn truth_up_pass_confirms_every_proven_gdpr_gate_rests_on_a_dated_green_artifact() {
    let repo_root = workspace_root();
    let card: TruthUpScorecard = run_truth_up_scorecard(RUN_DATE, &repo_root);

    assert!(
        card.rows_total() >= 16,
        "the full §10.1–10.9 PROVEN set is enumerated, got {}",
        card.rows_total()
    );

    assert!(
        card.is_green(),
        "TRUTH-UP RED - these GDPR rows are CLAIMED-NOT-PROVEN (no dated green artifact / vanished \
         proof source): {:?}",
        card.claimed_not_proven()
    );
    assert_eq!(
        card.rows_dated_green(),
        card.rows_total(),
        "every enumerated PROVEN row is dated-green"
    );

    for entry in &card.entries {
        let abs = entry.row.artifact_abs_path(&repo_root);
        assert!(
            abs.exists(),
            "row {} ({}) names a proof source that must exist on disk: {}",
            entry.row.id,
            entry.row.section,
            abs.display()
        );
        assert!(
            matches!(entry.status, RowStatus::DatedGreen { .. }),
            "row {} resolved to a dated green artifact",
            entry.row.id
        );
    }

    let rendered = card.render();
    assert!(
        rendered.contains("P-512 GDPR TRUTH-UP SCORECARD 2026-06-26"),
        "the scorecard is dated: {rendered}"
    );
    assert!(
        rendered.contains("GREEN (no GDPR gate red)"),
        "the verdict line is green: {rendered}"
    );
    println!("{rendered}");
}

#[test]
fn truth_up_pass_enumerates_the_full_section_10_coverage() {
    let rows = proven_gdpr_rows(RUN_DATE);
    let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();

    for must in [
        "GA-D1",
        "GA-D2",
        "GA-D3",
        "GA-D4",
        "GA-D5",
        "GA-D6",
        "GA-D7",
        "GA-D8",
        "GA-10",
        "GA-11",
        "CI-D3",
        "GIT-D2",
        "STOR-D3-GA-face",
        "STOR-D4-GA-face",
        "E2E-3",
        "E2E-4",
    ] {
        assert!(
            ids.contains(&must),
            "the truth-up pass must enumerate the PROVEN row {must}"
        );
    }

    let sections: BTreeMap<&str, usize> = rows.iter().fold(BTreeMap::new(), |mut m, r| {
        *m.entry(r.section).or_insert(0) += 1;
        m
    });
    for sec in [
        "10.1", "10.2", "10.4", "10.5", "10.6", "10.7", "10.8", "10.9",
    ] {
        assert!(
            sections.contains_key(sec),
            "§{sec} owns at least one PROVEN row in the truth-up pass"
        );
    }

    assert_eq!(TRUTH_UP_FULL_PASS_PROMPT, "P-GA-38 (→ P-512)");
}

#[test]
fn a_claimed_not_proven_row_fails_the_truth_up_pass_loudly() {
    let mut rows = proven_gdpr_rows(RUN_DATE);
    let undated = rows
        .iter_mut()
        .find(|r| r.id == "GA-D1")
        .expect("GA-D1 present");
    undated.artifact_date = None;

    let err = TruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect_err("a claimed-not-proven row MUST fail the truth-up CI job");
    assert!(err.to_string().contains("TRUTH-UP FAIL"), "loud: {err}");
    assert!(
        err.to_string().contains("GA-D1"),
        "names the undated row: {err}"
    );

    let card = run_truth_up_scorecard(RUN_DATE, Path::new("/nonexistent-truth-up-root"));
    assert!(
        !card.is_green(),
        "a vanished proof source reds the scorecard"
    );
    let entry = card
        .entries
        .iter()
        .find(|e| e.row.id == "GA-D2")
        .expect("GA-D2 present");
    match &entry.status {
        RowStatus::ClaimedNotProven { date, reason } => {
            assert_eq!(date, RUN_DATE, "the gap is dated");
            assert!(
                reason.contains("proof source missing on disk"),
                "the honest reason names the missing source: {reason}"
            );
        }
        RowStatus::DatedGreen { .. } => {
            unreachable!("the proof source is gone under the empty root")
        }
    }
    assert!(
        card.render().contains("CLAIMED-NOT-PROVEN"),
        "the render surfaces the gap loudly"
    );
}

#[test]
fn the_truth_up_pass_is_a_permanent_re_runnable_drill() {
    let repo_root = workspace_root();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "repro_p512_truth_up_every_gdpr_gate_dated_green",
        move |ctx: &mut DrillContext| {
            let card = run_truth_up_scorecard(RUN_DATE, &repo_root);
            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                card.claimed_not_proven().len() as i64,
            );
            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        },
    ));

    let first = registry.run_all();
    let second = registry.run_all();
    assert!(
        first[0].is_pass(),
        "the truth-up drill must pass - no GDPR gate is red: {:?}",
        first[0]
    );
    assert!(second[0].is_pass(), "it re-runs green forever");
    assert!(
        registry.all_green(),
        "the suite is green with the truth-up drill registered"
    );
}
