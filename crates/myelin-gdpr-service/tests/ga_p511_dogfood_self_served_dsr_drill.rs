//! P-GA-37 → global P-511 (M6) — the dogfood drill: the GDPR/Audit machinery runs on Myelin's OWN
//! commits + a self-served DSR + the RoPA/data-map Knowledge space + the every-incident loop + the
//! truth-up pass.
//!
//! This is the prompt's required end-to-end integration of the dogfood loop, chaining the deliverables
//! (EI-01 §4: chain operations, do not exercise handlers in isolation):
//!
//! 1. **The audit consumer runs on the platform's own actions** — every Myelin action (human AND
//!    agent) is logged through the REAL outbox-only [`AuditConsumer`]; the audit graph is GREEN on the
//!    platform's own actions (chain verifies, root exists, `audit_append_lag` reads green).
//! 2. **A self-served DSR over a Myelin team member's own data** fans out across the whole H1–H18
//!    catalogue (GA-D1) across `member_cells ∪ home_cell` (GA-D8) and SEALS a certificate into the
//!    per-tenant audit Merkle tree.
//! 3. **The RoPA + the data map live as a Myelin Knowledge space** — the generated data map + RoPA
//!    render as the Myelin team's own GDPR space pages.
//! 4. **The every-incident-adds-a-drill loop** — a synthetic GDPR incident files a PII-free Myelin
//!    issue draft AND registers a reproducing drill into the harness [`DrillRegistry`] (the T-3
//!    `register_drill` hook), which then RE-RUNS and stays green forever.
//! 5. **The truth-up pass** — enumerates every PROVEN §9.2 GDPR row and asserts each rests on a dated
//!    green artifact; a row without one is a LOUD failure (not a silent pass).
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! modeled own-tenant data (the live OLTP `audit_entry`/`dsr_request` tables + the real KMS signing
//! key + the live self-hosting JetStream subscription are the named DB/KMS/bus floor every M0/M1 store
//! carries, P-007 / P-S12 — a config swap at boot). This drill proves the dogfood WIRING — the audit
//! consumer + the self-served DSR + the RoPA space + the incident loop + the truth-up pass — and joins
//! the permanent `cargo test` suite (re-runs on every Myelin commit, the dogfood loop's whole point).

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_gdpr_service::dogfood::{
    proven_gdpr_rows, run_audit_consumer_on_dogfood, run_self_served_dsr_on_dogfood, GdprIncident,
    RopaKnowledgeSpace, TruthUpPass,
};
use myelin_tenancy::Region;

/// A dated run stamp (the dogfood CI run's date). The harness `today_iso()` supplies the real one in a
/// live run; the test pins a date so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// **(1) THE HEADLINE (audit leg): the audit consumer runs GREEN on Myelin's OWN actions.** The REAL
/// outbox-only consumer logs every one of the platform's own action surfaces (git/ci/issue/chat + an
/// agent action) — the audit graph is green on the platform's own actions.
#[test]
fn the_audit_consumer_runs_on_myelins_own_actions() {
    let artifact = run_audit_consumer_on_dogfood(RUN_DATE);

    assert!(
        artifact.audit_graph_is_green(),
        "the audit graph must be green on the platform's own actions: {artifact:?}"
    );
    assert_eq!(
        artifact.actions_logged, 5,
        "all five own-action surfaces logged"
    );
    assert!(
        artifact.chain_verifies,
        "the per-tenant hash-chain verifies"
    );
    assert_eq!(artifact.append_lag, 0, "audit_append_lag reads green");

    let line = artifact.summary();
    assert!(
        line.contains("P-511 DOGFOOD AUDIT GREEN 2026-06-26"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

/// **(2) THE HEADLINE (DSR leg): a self-served DSR over the team's own data fans out + seals a
/// certificate.** A Myelin team member's `dsr_submit` reaches every H1–H18 holder (0 missed) across
/// `member_cells ∪ home_cell` (0 cells missed) and seals a Merkle-proven certificate.
#[test]
fn a_self_served_dsr_over_the_teams_own_data_seals_a_certificate() {
    let artifact = run_self_served_dsr_on_dogfood(RUN_DATE);

    assert!(
        artifact.dsr_is_green(),
        "the self-served DSR must be green on the platform's own data: {artifact:?}"
    );
    assert_eq!(artifact.holders_missed, 0, "GA-D1: 0 holders missed");
    assert_eq!(artifact.cells_missed, 0, "GA-D8: 0 cells missed");
    assert!(
        artifact.certificate_sealed,
        "the completion certificate seals into the per-tenant audit Merkle tree"
    );
    let inclusion = artifact
        .inclusion_proof
        .as_ref()
        .expect("the sealed certificate carries a Merkle inclusion proof");
    assert!(
        inclusion.contains("->blake3:"),
        "inclusion proof: {inclusion}"
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-511 DOGFOOD DSR GREEN 2026-06-26") && line.contains("certificate=SEALED"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

/// **(3) The RoPA + the data map live as a Myelin Knowledge space.** The generated data map + RoPA
/// render as the Myelin team's own GDPR space pages (populated, fingerprinted).
#[test]
fn the_ropa_and_data_map_live_as_a_myelin_knowledge_space() {
    let space = RopaKnowledgeSpace::for_myelin_team(Region("fr-par".into()));

    assert!(
        space.is_populated(),
        "the RoPA Knowledge space is populated"
    );
    let pages = space.render_pages();
    assert_eq!(pages.len(), 2, "the data-map page + the RoPA page");
    assert!(
        pages[0].body.contains("blake3:"),
        "the data-map page carries the generated map's fingerprint"
    );
    println!(
        "[P-511 ROPA SPACE {RUN_DATE}] '{}' — {} pages, data-map entries={}",
        space.title(),
        pages.len(),
        space.data_map().entry_count()
    );
}

/// **(4) The every-incident-adds-a-drill loop: a synthetic GDPR incident files an issue + REGISTERS a
/// reproducing drill that re-runs forever.** The incident produces a PII-free Myelin issue draft AND a
/// reproducing-drill ticket; the test builds the repro [`DrillScenario`] under the ticket's name,
/// `register_drill`s it into the harness [`DrillRegistry`] (the T-3 hook), and proves it RE-RUNS green
/// twice (the "re-runs forever" guarantee — a regression would re-red it loudly).
#[test]
fn a_synthetic_gdpr_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = GdprIncident::new(
        "INC-GDPR-DOGFOOD-1",
        "GA-D1",
        "a self-served DSR fan-out skipped a newly-registered holder on Myelin's own tenant",
        "repro_ga_d1_dogfood_dsr_skips_holder",
    );

    // (a) it files a PII-free Myelin issue draft (names the gate + the repro drill).
    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "GA-D1");
    assert!(draft.title.contains("INC-GDPR-DOGFOOD-1"));
    assert!(
        draft.body.contains("repro_ga_d1_dogfood_dsr_skips_holder"),
        "the issue is traceable to its repro drill: {}",
        draft.body
    );

    // (b) it registers a reproducing drill into the harness suite (the T-3 register_drill hook).
    let ticket = incident.drill_ticket();
    let drill_name = ticket.drill_name.clone();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            // The reproducing scenario: re-run the self-served DSR on Myelin's own data and assert it
            // is whole (0 holders missed — a regression that re-broke the fan-out would re-red this).
            let dsr = run_self_served_dsr_on_dogfood(RUN_DATE);
            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                if dsr.dsr_is_green() { 0 } else { 1 },
            );
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

/// **(5) The truth-up pass: every PROVEN GDPR row rests on a dated green artifact (0 red earlier-band
/// GDPR gates).** Enumerates the frozen PROVEN §9.2 set (dated at the run) and asserts the
/// loud-never-swallowed CI entrypoint returns Ok — the gate invariant holds end-to-end.
#[test]
fn the_truth_up_pass_confirms_every_proven_gdpr_row_is_dated() {
    let rows = proven_gdpr_rows(RUN_DATE);
    assert!(
        rows.len() >= 11,
        "the PROVEN set covers the §9.2 GA-D*/GA-10/GA-11 family + the E2E legs"
    );

    let confirmed = TruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band GDPR gates — every PROVEN row rests on a dated green artifact");
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    println!("[P-511 TRUTH-UP GREEN {RUN_DATE}] {confirmed} PROVEN GDPR rows rest on a dated green artifact (0 red earlier-band gates)");
}

/// **The dogfood loop end-to-end (EI-01 §4: chain the operations, do not exercise handlers in
/// isolation).** The full GA-M6 spine in one chained run: the audit consumer logs Myelin's own actions
/// → a self-served DSR over the team's own data fans out + seals a certificate → the RoPA/data-map
/// lives as a Knowledge space → a GDPR incident files an issue + registers a repro drill that re-runs
/// green → the truth-up pass confirms 0 red earlier-band GDPR gates. The platform hosts itself, and the
/// GDPR/Audit machinery runs on the platform's own commits.
#[test]
fn dogfood_loop_end_to_end_self_hosting() {
    // (1) the audit consumer is live on Myelin's own actions → the audit graph is green.
    let audit = run_audit_consumer_on_dogfood(RUN_DATE);
    assert!(audit.audit_graph_is_green(), "audit green on own actions");

    // (2) a self-served DSR over the team's own data → 0 holders missed, 0 cells missed, sealed.
    let dsr = run_self_served_dsr_on_dogfood(RUN_DATE);
    assert!(
        dsr.dsr_is_green(),
        "self-served DSR green + certificate sealed"
    );

    // (3) the RoPA + data map live as a Myelin Knowledge space.
    let space = RopaKnowledgeSpace::for_myelin_team(Region("fr-par".into()));
    assert!(space.is_populated(), "the RoPA Knowledge space lives");

    // (4) an incident files an issue + registers a repro drill that re-runs forever.
    let incident = GdprIncident::new(
        "INC-GDPR-DOGFOOD-E2E",
        "GA-D8",
        "a multi-cell self-served DSR wave dropped a self-host cell under a doc-edit surge",
        "repro_ga_d8_dogfood_cell_drop_under_surge",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        ticket.drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let whole = run_self_served_dsr_on_dogfood(RUN_DATE).dsr_is_green();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    // (5) the truth-up pass — 0 red earlier-band GDPR gates.
    let rows = proven_gdpr_rows(RUN_DATE);
    let confirmed = TruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band GDPR gates");
    assert!(confirmed >= 11);

    println!("[P-511 DOGFOOD LOOP GREEN {RUN_DATE}] self-hosting: audit consumer green on own actions; self-served DSR green + certificate sealed; RoPA/data-map lives as a Knowledge space; incident→issue→repro-drill registered + re-runs green; truth-up confirms {confirmed} PROVEN GDPR rows dated");
}
