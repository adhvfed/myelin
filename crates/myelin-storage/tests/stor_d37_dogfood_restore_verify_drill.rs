//! P-ST-37 → global P-506 (M6) — the dogfood drill: the restore-verify gate runs on Myelin's OWN
//! commits + the every-incident-adds-a-drill loop + the truth-up pass.
//!
//! This is the prompt's required end-to-end integration of the dogfood loop, chaining the three
//! deliverables (EI-01 §4: chain operations, do not exercise handlers in isolation):
//!
//! 1. **The restore-verify CI job runs on the platform's own stores** — a [`DogfoodCorpus`] of
//!    Myelin's OWN monorepo commits / CI logs / issues / docs is restored + verified by the SAME
//!    permanent restore-verify gate, emitting a dated green artifact on real team data.
//! 2. **The every-incident-adds-a-drill loop** — a synthetic storage incident files a PII-free Myelin
//!    issue draft AND registers a reproducing drill into the harness [`DrillRegistry`] (the T-3
//!    `register_drill` hook), which then RE-RUNS and stays green forever.
//! 3. **The truth-up pass** — enumerates every PROVEN Storage row and asserts each rests on a dated
//!    green artifact; a row without one is a LOUD failure (not a silent pass).
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! modeled own-data corpus (the real `pg_restore` + object-store backing is the named P-S12/P-S15
//! floor, and the LIVE-stack restore-verify cross-seam drill is the existing `stage3_drills`
//! `STOR-D-RESTORE` infra-gate row run `--features integration`). This drill proves the dogfood
//! WIRING — the gate + the incident loop + the truth-up pass — and joins the permanent `cargo test`
//! suite (re-runs on every Myelin commit, the dogfood loop's whole point).

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};
use myelin_storage::dogfood::{
    proven_storage_rows, run_restore_verify_on_dogfood, DogfoodCorpus, DogfoodStore,
    StorageIncident, TruthUpPass,
};
use myelin_tenancy::{Region, TenantId};

/// A dated run stamp (the dogfood CI run's date). The harness `today_iso()` supplies the real one in
/// a live run; the test pins a date so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-25";

/// Myelin's OWN data, modeled as the self-host cell's tenant data — one record of each store class
/// (monorepo / ci-log / issue / doc), at increasing WAL offsets.
fn myelin_own_corpus() -> DogfoodCorpus {
    let mut corpus = DogfoodCorpus::new(TenantId("myelin-self".into()), Region::new("fr-par"));
    corpus
        .commit_record(
            DogfoodStore::Monorepo,
            "commit-d3b590c",
            100,
            b"P-444 M5: restore-verify at cell scale".to_vec(),
        )
        .commit_record(
            DogfoodStore::CiLog,
            "ci-run-506-step-2",
            110,
            b"cargo test --workspace ... ok".to_vec(),
        )
        .commit_record(
            DogfoodStore::Issue,
            "issue-P-506",
            120,
            b"Dogfood: restore-verify on Myelin's own commits".to_vec(),
        )
        .commit_record(
            DogfoodStore::Doc,
            "doc-storage-s-m6",
            130,
            "# Storage §2 S-M6 — the dogfood loop".as_bytes().to_vec(),
        );
    corpus
}

/// **(1) THE HEADLINE: the restore-verify CI job runs GREEN on the platform's own stores.** The SAME
/// permanent gate, on Myelin's OWN monorepo commits / CI logs / issues / docs — emits a dated green
/// artifact naming the measured zeros + the per-store coverage of the team's own data.
#[test]
fn restore_verify_gate_runs_on_myelins_own_commits() {
    let corpus = myelin_own_corpus();
    let artifact = run_restore_verify_on_dogfood(&corpus, RUN_DATE)
        .expect("the restore-verify gate must run GREEN on Myelin's own stores (the dogfood loop)");

    assert_eq!(
        artifact.gate.restored_to_offset, 130,
        "restored to the latest own-data offset"
    );
    assert_eq!(
        artifact.gate.oltp_row_count, 4,
        "all four own-data records restored"
    );
    assert_eq!(
        artifact.gate.checksum_mismatches, 0,
        "0 own-data checksum mismatches"
    );
    assert_eq!(
        artifact.gate.cross_seam_mismatches, 0,
        "Myelin's own data lands at one consistent point"
    );
    assert_eq!(
        artifact.gate.resurrected_subjects, 0,
        "no erased subject resurrected"
    );
    assert_eq!(
        artifact.records_by_store.len(),
        4,
        "all four of Myelin's own store classes covered"
    );

    // The dated green artifact (observability is part of the pass — a CI run surfaces this line).
    let line = artifact.summary();
    assert!(
        line.contains("P-506 DOGFOOD RESTORE-VERIFY GREEN 2026-06-25"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

/// **(2) The every-incident-adds-a-drill loop: a synthetic storage incident files an issue +
/// REGISTERS a reproducing drill that re-runs forever.** The incident produces a PII-free Myelin
/// issue draft AND a reproducing-drill ticket; the test builds the repro [`DrillScenario`] under the
/// ticket's name, `register_drill`s it into the harness [`DrillRegistry`] (the T-3 hook), and proves
/// it RE-RUNS green twice (the "re-runs forever" guarantee — a regression would re-red it loudly).
#[test]
fn a_synthetic_storage_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    // A synthetic storage incident discovered during dogfooding (PII-free: a fault class + summary).
    let incident = StorageIncident::new(
        "INC-STOR-DOGFOOD-1",
        "STOR-D1",
        "a restored own-monorepo commit re-hashed wrong at a base-backup boundary",
        "repro_stor_d1_own_commit_rehash_at_base_boundary",
    );

    // (a) it files a PII-free Myelin issue draft (the issue body names the gate + the repro drill).
    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "STOR-D1");
    assert!(draft.title.contains("INC-STOR-DOGFOOD-1"));
    assert!(
        draft
            .body
            .contains("repro_stor_d1_own_commit_rehash_at_base_boundary"),
        "the issue is traceable to its repro drill: {}",
        draft.body
    );

    // (b) it registers a reproducing drill into the harness suite (the T-3 register_drill hook).
    let ticket = incident.drill_ticket();
    assert_eq!(
        ticket.drill_name,
        "repro_stor_d1_own_commit_rehash_at_base_boundary"
    );

    let mut registry = DrillRegistry::new();
    // The reproducing scenario: re-run the restore-verify gate on Myelin's own data and assert it is
    // whole (the incident's repro — a regression that re-broke the restore would re-red this drill).
    let drill_name = ticket.drill_name.clone();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let corpus = myelin_own_corpus();
            let whole = run_restore_verify_on_dogfood(&corpus, RUN_DATE).is_ok();
            // The survival signal: 0 checksum mismatches across the re-run restore of Myelin's own data.
            ctx.signals
                .set_scalar(SignalName::DeadLetterCount, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        },
    ));
    assert_eq!(
        registry.len(),
        1,
        "the incident's repro joined the permanent suite"
    );

    // It RE-RUNS forever — drive it twice, green both times (the every-incident loop's guarantee).
    let first = registry.run_all();
    let second = registry.run_all();
    assert!(
        first[0].is_pass(),
        "the registered repro drill must pass: {:?}",
        first[0]
    );
    assert!(second[0].is_pass(), "it re-runs green forever");
    assert!(
        registry.all_green(),
        "the suite is green with the incident's repro registered"
    );
    assert_eq!(first[0].name(), drill_name);
}

/// **(3) The truth-up pass: every PROVEN Storage row rests on a dated green artifact (0 red
/// earlier-band storage gates).** Enumerates the frozen PROVEN set (dated at the run) and asserts the
/// loud-never-swallowed CI entrypoint returns Ok — the gate invariant holds end-to-end.
#[test]
fn the_truth_up_pass_confirms_every_proven_storage_row_is_dated() {
    let rows = proven_storage_rows(RUN_DATE);
    assert!(
        rows.len() >= 14,
        "the PROVEN set covers the STOR-D* family + trust-boundary gates + shipped floors"
    );

    let confirmed = TruthUpPass::new().run_or_fail_ci(&rows, RUN_DATE).expect(
        "0 red earlier-band storage gates — every PROVEN row rests on a dated green artifact",
    );
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    // The dated truth-up verdict (observability of the gate invariant).
    println!("[P-506 TRUTH-UP GREEN {RUN_DATE}] {confirmed} PROVEN storage rows rest on a dated green artifact (0 red earlier-band gates)");
}

/// **The dogfood loop end-to-end (EI-01 §4: chain the operations, do not exercise handlers in
/// isolation).** The full S-M6 spine in one chained run: restore-verify GREEN on Myelin's own data →
/// a storage incident files an issue + registers a repro drill that re-runs green → the truth-up pass
/// confirms 0 red earlier-band storage gates. The platform hosts itself, and the restore-verify gate
/// + the every-incident loop + the truth-up pass all run on the platform's own commits.
#[test]
fn dogfood_loop_end_to_end_self_hosting() {
    // (1) restore-verify on Myelin's own commits → dated green.
    let corpus = myelin_own_corpus();
    let artifact = run_restore_verify_on_dogfood(&corpus, RUN_DATE)
        .expect("restore-verify green on Myelin's own commits");
    assert_eq!(artifact.gate.checksum_mismatches, 0);

    // (2) an incident files an issue + registers a repro drill that re-runs forever.
    let incident = StorageIncident::new(
        "INC-STOR-DOGFOOD-E2E",
        "STOR-D2",
        "a self-host cell-kill restore exceeded the RTO budget under a doc-edit surge",
        "repro_stor_d2_own_cell_kill_rto_under_doc_surge",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    let name = ticket.drill_name.clone();
    registry.register_drill(DrillScenario::new(name, move |ctx: &mut DrillContext| {
        let whole = run_restore_verify_on_dogfood(&myelin_own_corpus(), RUN_DATE).is_ok();
        ctx.signals
            .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
        ctx.signals
            .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
    }));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    // (3) the truth-up pass — 0 red earlier-band storage gates.
    let rows = proven_storage_rows(RUN_DATE);
    let confirmed = TruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band storage gates");
    assert!(confirmed >= 14);

    println!("[P-506 DOGFOOD LOOP GREEN {RUN_DATE}] self-hosting: restore-verify on own commits green; incident→issue→repro-drill registered + re-runs green; truth-up confirms {confirmed} PROVEN rows dated");
}
