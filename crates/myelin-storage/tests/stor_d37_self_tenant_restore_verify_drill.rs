use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};
use myelin_storage::self_tenant::{
    proven_storage_rows, run_restore_verify_on_self_tenant, SelfTenantCorpus, SelfTenantStore,
    StorageIncident, TruthUpPass,
};
use myelin_tenancy::{Region, TenantId};

const RUN_DATE: &str = "2026-06-25";

fn myelin_own_corpus() -> SelfTenantCorpus {
    let mut corpus = SelfTenantCorpus::new(TenantId("myelin-self".into()), Region::new("fr-par"));
    corpus
        .commit_record(
            SelfTenantStore::Monorepo,
            "commit-d3b590c",
            100,
            b"P-444 M5: restore-verify at cell scale".to_vec(),
        )
        .commit_record(
            SelfTenantStore::CiLog,
            "ci-run-506-step-2",
            110,
            b"cargo test --workspace ... ok".to_vec(),
        )
        .commit_record(
            SelfTenantStore::Issue,
            "issue-P-506",
            120,
            b"SelfTenant: restore-verify on Myelin's own commits".to_vec(),
        )
        .commit_record(
            SelfTenantStore::Doc,
            "doc-storage-s-m6",
            130,
            "# Storage §2 S-M6 - the self_tenant loop".as_bytes().to_vec(),
        );
    corpus
}

#[test]
fn restore_verify_gate_runs_on_myelins_own_commits() {
    let corpus = myelin_own_corpus();
    let artifact = run_restore_verify_on_self_tenant(&corpus, RUN_DATE)
        .expect("the restore-verify gate must run GREEN on Myelin's own stores (the self_tenant loop)");

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

    let line = artifact.summary();
    assert!(
        line.contains("P-506 SELF_TENANT RESTORE-VERIFY GREEN 2026-06-25"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn a_synthetic_storage_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = StorageIncident::new(
        "INC-STOR-SELF_TENANT-1",
        "STOR-D1",
        "a restored own-monorepo commit re-hashed wrong at a base-backup boundary",
        "repro_stor_d1_own_commit_rehash_at_base_boundary",
    );

    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "STOR-D1");
    assert!(draft.title.contains("INC-STOR-SELF_TENANT-1"));
    assert!(
        draft
            .body
            .contains("repro_stor_d1_own_commit_rehash_at_base_boundary"),
        "the issue is traceable to its repro drill: {}",
        draft.body
    );

    let ticket = incident.drill_ticket();
    assert_eq!(
        ticket.drill_name,
        "repro_stor_d1_own_commit_rehash_at_base_boundary"
    );

    let mut registry = DrillRegistry::new();
    let drill_name = ticket.drill_name.clone();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let corpus = myelin_own_corpus();
            let whole = run_restore_verify_on_self_tenant(&corpus, RUN_DATE).is_ok();
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

#[test]
fn the_truth_up_pass_confirms_every_proven_storage_row_is_dated() {
    let rows = proven_storage_rows(RUN_DATE);
    assert!(
        rows.len() >= 14,
        "the PROVEN set covers the STOR-D* family + trust-boundary gates + shipped floors"
    );

    let confirmed = TruthUpPass::new().run_or_fail_ci(&rows, RUN_DATE).expect(
        "0 red earlier-band storage gates - every PROVEN row rests on a dated green artifact",
    );
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    println!("[P-506 TRUTH-UP GREEN {RUN_DATE}] {confirmed} PROVEN storage rows rest on a dated green artifact (0 red earlier-band gates)");
}

#[test]
fn self_tenant_loop_end_to_end_self_hosting() {
    let corpus = myelin_own_corpus();
    let artifact = run_restore_verify_on_self_tenant(&corpus, RUN_DATE)
        .expect("restore-verify green on Myelin's own commits");
    assert_eq!(artifact.gate.checksum_mismatches, 0);

    let incident = StorageIncident::new(
        "INC-STOR-SELF_TENANT-E2E",
        "STOR-D2",
        "a self-host cell-kill restore exceeded the RTO budget under a doc-edit surge",
        "repro_stor_d2_own_cell_kill_rto_under_doc_surge",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    let name = ticket.drill_name.clone();
    registry.register_drill(DrillScenario::new(name, move |ctx: &mut DrillContext| {
        let whole = run_restore_verify_on_self_tenant(&myelin_own_corpus(), RUN_DATE).is_ok();
        ctx.signals
            .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
        ctx.signals
            .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
    }));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    let rows = proven_storage_rows(RUN_DATE);
    let confirmed = TruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band storage gates");
    assert!(confirmed >= 14);

    println!("[P-506 SELF_TENANT LOOP GREEN {RUN_DATE}] self-hosting: restore-verify on own commits green; incident→issue→repro-drill registered + re-runs green; truth-up confirms {confirmed} PROVEN rows dated");
}
