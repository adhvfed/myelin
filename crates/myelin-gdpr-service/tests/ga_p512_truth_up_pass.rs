//! P-GA-38 → global P-512 (M6) — **the truth-up pass: every PROVEN GDPR gate rests on a dated green
//! artifact.** The closing honesty pass over §10.1–10.9 (gdpr-and-audit §9.2 drill table + the lint /
//! erasure-ledger faces). Code-wins-over-docs made mechanical (EI-01 §1): a PROVEN row that does not
//! rest on a dated green artifact — or whose proof SOURCE has vanished from disk — is surfaced as
//! CLAIMED-NOT-PROVEN with a date, never swallowed into a green it did not earn.
//!
//! The prompt's required TESTS (integration): the truth-up pass enumerates every GDPR PROVEN row and
//! asserts each rests on a dated green artifact (a row without one is surfaced, not swallowed). This
//! is the gate invariant — no earlier-band GDPR gate is red — proven end-to-end against the REAL proof
//! sources on disk (not a doc claim).
//!
//! It is NOT behind the `integration` feature: the truth-up pass owns no DB/object-store/cache/bus
//! contract (the prompt: "Owns: the truth-up scorecard over 10.1–10.9 — no new contract shape"). It
//! reads the FROZEN PROVEN-row enumeration and checks each row's proof source exists in the workspace,
//! then renders the enumerated scorecard (the GATE green artifact). It joins the permanent `cargo test`
//! suite (re-runs on every Myelin commit — the closing honesty pass re-runs forever).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_gdpr_service::dogfood::{
    proven_gdpr_rows, run_truth_up_scorecard, RowStatus, TruthUpPass, TruthUpScorecard,
    TRUTH_UP_FULL_PASS_PROMPT,
};

/// A dated run stamp (the truth-up CI run's date). The harness `today_iso()` supplies the real one in
/// a live run; the test pins a date so the rendered scorecard artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// Resolve the workspace root from this crate's manifest dir (`crates/myelin-gdpr-service`). The
/// `artifact_path`s are relative to this root, so the existence check reads the REAL proof sources.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root is two levels above the crate manifest")
        .to_path_buf()
}

/// **THE HEADLINE: the truth-up pass is GREEN — every PROVEN GDPR row across §10.1–10.9 rests on a
/// DATED green artifact whose proof source exists on disk (no GDPR gate is red).** This is the gate
/// invariant holding end-to-end (EI-01 §1, master-sequencing §2 M6): not a doc claim, but each row
/// resolved against the real proof file in the workspace.
#[test]
fn truth_up_pass_confirms_every_proven_gdpr_gate_rests_on_a_dated_green_artifact() {
    let repo_root = workspace_root();
    let card: TruthUpScorecard = run_truth_up_scorecard(RUN_DATE, &repo_root);

    // (a) The enumeration is COMPLETE — the full §10.1–10.9 PROVEN set (not just the §9.2 core).
    assert!(
        card.rows_total() >= 16,
        "the full §10.1–10.9 PROVEN set is enumerated, got {}",
        card.rows_total()
    );

    // (b) Every row rests on a dated green artifact — no GDPR gate is red.
    assert!(
        card.is_green(),
        "TRUTH-UP RED — these GDPR rows are CLAIMED-NOT-PROVEN (no dated green artifact / vanished \
         proof source): {:?}",
        card.claimed_not_proven()
    );
    assert_eq!(
        card.rows_dated_green(),
        card.rows_total(),
        "every enumerated PROVEN row is dated-green"
    );

    // (c) Every row's proof SOURCE actually exists on disk (a row cannot claim a vanished artifact).
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

    // (d) The rendered scorecard IS the dated green artifact (the GATE/DRILLS telemetry).
    let rendered = card.render();
    assert!(
        rendered.contains("P-512 GDPR TRUTH-UP SCORECARD 2026-06-26"),
        "the scorecard is dated: {rendered}"
    );
    assert!(
        rendered.contains("GREEN (no GDPR gate red)"),
        "the verdict line is green: {rendered}"
    );
    // Print the enumerated scorecard so the green artifact is observable in CI output (EI-01 §3).
    println!("{rendered}");
}

/// **The §10.1–10.9 coverage is COMPLETE: every PROVEN-row family the prompt enumerates is present —
/// the §9.2 drill table (GA-D1..GA-D8 / GA-10 / GA-11) PLUS GA-D5 (the lint + data-map-diff face), the
/// STOR-D3/D4-GA erasure-ledger faces, CI-D3, GIT-D2, and the E2E-3/E2E-4 legs.** A missing family
/// would mean a GDPR gate was silently dropped from the truth-up pass.
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

    // Every §10.x section that owns a PROVEN gate is represented (the scorecard groups by section).
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

    // The full-pass prompt lineage is recorded in writing (P-GA-38 → P-512).
    assert_eq!(TRUTH_UP_FULL_PASS_PROMPT, "P-GA-38 (→ P-512)");
}

/// **MANDATORY-CORE: a row WITHOUT a dated green artifact is surfaced, NOT swallowed.** The truth-up
/// pass FAILs LOUDLY on a CLAIMED-NOT-PROVEN row (the `run_or_fail_ci` entrypoint returns an `Err` that
/// names the row) — a claim that outran its verification can never masquerade as proven (EI-01 §1).
#[test]
fn a_claimed_not_proven_row_fails_the_truth_up_pass_loudly() {
    let mut rows = proven_gdpr_rows(RUN_DATE);
    // The headline gate loses its dated artifact (a doc claim that outran its proof).
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

    // And the scorecard surfaces it with a DATE + an honest reason (a missing proof source), never
    // a silent green: run the scorecard against an empty root so every dated row is caught.
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

/// **The truth-up pass joins the permanent drill suite (the closing honesty pass re-runs forever).**
/// Registered into the harness [`DrillRegistry`] as a re-runnable scenario: a regression that re-reds
/// any PROVEN GDPR row (drops its artifact, or deletes its proof source) flips the signal off `0` and
/// the suite goes red — so the gate invariant is enforced mechanically on every Myelin commit (EI-01
/// §5: the ratchet).
#[test]
fn the_truth_up_pass_is_a_permanent_re_runnable_drill() {
    let repo_root = workspace_root();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "repro_p512_truth_up_every_gdpr_gate_dated_green",
        move |ctx: &mut DrillContext| {
            let card = run_truth_up_scorecard(RUN_DATE, &repo_root);
            // 0 claimed-not-proven rows ⇒ green; any red GDPR gate flips this off 0.
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
        "the truth-up drill must pass — no GDPR gate is red: {:?}",
        first[0]
    );
    assert!(second[0].is_pass(), "it re-runs green forever");
    assert!(
        registry.all_green(),
        "the suite is green with the truth-up drill registered"
    );
}
