//! The self-hosting CI graph IS the test (P-507 / P-S37 → M6) — the dogfood loop.
//!
//! The prompt's TESTS field: "The self-hosting CI pipeline IS the test: a Myelin commit triggers
//! the lints + scanner + mutation gate; a deliberately-violating commit is rejected (the ratchet
//! rejects on Myelin's own work)." These tests drive the graph with an INJECTED runner (no shelling
//! to cargo — fast + hermetic), proving:
//!   1. the frozen graph carries the substrate ratchet (the twelve lints + the scanner + the
//!      mandatory-core mutation gate) AND drives SUB-D3/D6/D10 (the harness drives the drills);
//!   2. an all-green commit reads GREEN (the dogfood gate passes on a clean commit);
//!   3. a DELIBERATELY-VIOLATING commit (any ratchet job red) is REJECTED — the gate reds and names
//!      the red job (the ratchet rejects on Myelin's own work, EI-01 §5).
//!
//! The REAL end-to-end run (`cargo run -p myelin-harness --bin self-hosting-ci`) shells the actual
//! lints/scanner/mutation-gate/drills on Myelin's own commit; the CI job IS that artifact. These
//! unit tests prove the graph's COMPOSITION + REJECTION LOGIC without the multi-minute cargo run.

use myelin_harness::self_hosting_ci::{
    run_graph, self_hosting_jobs, JobKind, JobResult, JobTool, SelfHostJob,
};

/// A stub runner that PASSES every job (a clean Myelin commit).
fn all_green(job: &SelfHostJob) -> JobResult {
    JobResult::Pass {
        id: job.id.to_string(),
        proof: format!("stub PASS `{}`", job.id),
    }
}

/// A stub runner that reds exactly the job whose id is `violating` (a deliberately-violating
/// commit: that one ratchet job exits non-zero).
fn reds_one(violating: &'static str) -> impl Fn(&SelfHostJob) -> JobResult {
    move |job: &SelfHostJob| {
        if job.id == violating {
            JobResult::Red {
                id: job.id.to_string(),
                reason: format!("stub RED `{}` — the deliberately-violating commit", job.id),
            }
        } else {
            all_green(job)
        }
    }
}

#[test]
fn the_graph_carries_the_substrate_ratchet_and_drives_the_drills() {
    let jobs = self_hosting_jobs();

    // The twelve architecture lints (the lint-gate job + the fixture matrix).
    assert!(
        jobs.iter()
            .any(|j| j.id == "lints" && j.kind == JobKind::Lints),
        "the self-hosting graph MUST run the twelve architecture lints on Myelin's own commit"
    );
    assert!(
        jobs.iter()
            .any(|j| j.id == "lints-fixtures" && j.kind == JobKind::Lints),
        "the self-hosting graph MUST run the lint fixture matrix (the red-fixture rejects)"
    );

    // The contract-coverage scanner (the meta-gate + its self-test).
    assert!(
        jobs.iter()
            .any(|j| j.id == "contract-coverage" && j.kind == JobKind::ContractCoverage),
        "the self-hosting graph MUST run the contract-coverage scanner on Myelin's own commit"
    );

    // The mandatory-core cargo-mutants mutation gate — and it MUST run under `cargo mutants`.
    let mutation = jobs
        .iter()
        .find(|j| j.id == "mutation-gate")
        .expect("the self-hosting graph MUST run the mandatory-core cargo-mutants mutation gate");
    assert_eq!(
        mutation.kind,
        JobKind::MutationGate,
        "the mutation gate job must be a MutationGate"
    );
    assert_eq!(
        mutation.tool,
        JobTool::CargoMutants,
        "the mutation gate MUST run under `cargo mutants` (reads .cargo/mutants.toml)"
    );

    // The harness drives the substrate's surge/restore/migration drills (SUB-D3/D6/D10).
    for drill in ["SUB-D3", "SUB-D6", "SUB-D10"] {
        assert!(
            jobs.iter()
                .any(|j| j.id == drill && j.kind == JobKind::Drill),
            "the harness MUST drive {drill} as part of the self-hosting CI graph"
        );
    }
}

#[test]
fn the_graph_runs_the_tenancy_dogfood_band() {
    // P-508 / P-CP-23 → CP-M6: the two Tenancy lints run as Myelin CI jobs on the platform's own
    // commit, and the harness drives the dogfood drill (self-host + residency_verify on own data +
    // the truth-up pass). The dogfood loop for Tenancy is part of the self-hosting CI graph.
    let jobs = self_hosting_jobs();

    // The two Tenancy-OWNED lints (residency-pin + control-plane-pii-free) bite on a fixture commit.
    assert!(
        jobs.iter()
            .any(|j| j.id == "tenancy-lints" && j.kind == JobKind::Lints),
        "the self-hosting graph MUST run the two Tenancy lints as Myelin CI jobs on the platform's \
         own commit (P-CP-23 — the ratchet bites on a PII column / out-of-region write)"
    );

    // The dogfood drill: Myelin self-hosts as one cell + residency_verify GREEN on own data + the
    // truth-up pass (no later-band CP gate red).
    assert!(
        jobs.iter()
            .any(|j| j.id == "CP-D23-dogfood" && j.kind == JobKind::Drill),
        "the harness MUST drive the Tenancy dogfood drill (self-host + residency_verify on own data \
         + truth-up) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_tenancy_lint_commit_is_rejected() {
    // A Tenancy lint regression (a PII column / out-of-region write slipping in) reds the graph —
    // the ratchet rejects on Myelin's own work, exactly as for the substrate lints.
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("tenancy-lints"));
    assert!(
        !run.is_green(),
        "a Tenancy lint regression on Myelin's own commit MUST be rejected (EI-01 §5)"
    );
    assert_eq!(run.red_jobs(), vec!["tenancy-lints"]);
}

#[test]
fn a_red_tenancy_dogfood_drill_rejects_the_commit() {
    // The dogfood drill is part of the gate: a red truth-up pass / residency mismatch reds the graph.
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("CP-D23-dogfood"));
    assert!(
        !run.is_green(),
        "a red Tenancy dogfood drill (residency mismatch / claimed-not-proven row) MUST reject the \
         commit — the truth-up pass is part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["CP-D23-dogfood"]);
}

#[test]
fn the_graph_runs_the_ci_dogfood_band() {
    // P-509 / CI-P35 → CI-M6 (the done-bar): the Myelin build/test/lint/mutation pipeline runs AS a
    // Myelin `ci.pipeline` — the body's determinism (CI-D9) + crash-recovery (CI-D1) + the Git↔CI
    // check seam (CI-D8) + CI's E2E flagship (E2E-2) run as Myelin CI jobs, and the CI-P35 dogfood
    // drill (the switch test + the truth-up pass) is the done-bar gate. The dogfood loop carries CI.
    let jobs = self_hosting_jobs();

    for (id, why) in [
        (
            "ci-pipeline-determinism",
            "the ci.pipeline body (the durable workflow hosting the Myelin build) runs as Myelin CI \
             with bit-identical replay (CI-D9) + crash-recovery (CI-D1)",
        ),
        (
            "ci-check-seam",
            "the Git↔CI check seam (5.9 / CI-D8) runs on Myelin's own commits",
        ),
        (
            "ci-e2e-flagship",
            "CI's slice of the agent-native E2E flagship (E2E-2) is driven as a Myelin CI job",
        ),
        (
            "CI-P35-dogfood",
            "the CI switch test (driven, measured) + the CI truth-up pass run as the done-bar gate",
        ),
    ] {
        assert!(
            jobs.iter()
                .any(|j| j.id == id && j.kind == JobKind::Drill),
            "the self-hosting graph MUST run the CI dogfood band job `{id}` — {why}"
        );
    }
}

#[test]
fn a_red_ci_dogfood_drill_rejects_the_commit() {
    // The CI-P35 dogfood drill is part of the gate: a switch-test wall / an undated PROVEN CI row reds
    // the graph — the ratchet rejects on Myelin's own work (the done-bar holds itself, EI-01 §5).
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("CI-P35-dogfood"));
    assert!(
        !run.is_green(),
        "a red CI dogfood drill (a switch-test wall / a claimed-not-proven CI row) MUST reject the \
         commit — the switch test + truth-up pass are part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["CI-P35-dogfood"]);
}

#[test]
fn the_graph_runs_the_gdpr_dogfood_band() {
    // P-511 / P-GA-37 → GA-M6: the GDPR/Audit machinery runs on Myelin's OWN commits — the audit
    // consumer is live on the platform's own actions, a self-served DSR over a Myelin team member's
    // own data fans out + seals a certificate, the RoPA/data-map lives as a Myelin Knowledge space,
    // and the GDPR truth-up pass confirms 0 red earlier-band GDPR gates. The dogfood loop carries GDPR.
    let jobs = self_hosting_jobs();

    assert!(
        jobs.iter()
            .any(|j| j.id == "GA-P511-dogfood" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the GDPR dogfood band (the audit consumer on Myelin's own \
         commits + a self-served DSR + the truth-up pass) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_gdpr_dogfood_drill_rejects_the_commit() {
    // The GDPR dogfood drill is part of the gate: a broken audit chain on Myelin's own actions / a
    // self-served DSR that misses a holder / an undated PROVEN GDPR row reds the graph — the ratchet
    // rejects on Myelin's own work (the GDPR-by-construction guarantee holds itself, EI-01 §5).
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("GA-P511-dogfood"));
    assert!(
        !run.is_green(),
        "a red GDPR dogfood drill (a broken own-commit audit chain / a missed DSR holder / a \
         claimed-not-proven GDPR row) MUST reject the commit — the GDPR machinery on Myelin's own \
         commits is part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["GA-P511-dogfood"]);
}

#[test]
fn a_clean_commit_reads_green() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &all_green);

    assert!(
        run.is_green(),
        "an all-green commit must read GREEN (the dogfood gate passes on a clean commit)"
    );
    assert!(run.red_jobs().is_empty(), "a green run names no red jobs");
    assert_eq!(
        run.results.len(),
        jobs.len(),
        "every job in the frozen graph is run (no fail-fast — the artifact is complete in one pass)"
    );
    // The rendered artifact reads GREEN and is dated.
    let md = run.render_markdown();
    assert!(
        md.contains("GATE: GREEN"),
        "a green run renders GATE: GREEN"
    );
    assert!(md.contains(&run.date), "the artifact carries the run date");
}

#[test]
fn a_deliberately_violating_lint_commit_is_rejected() {
    // The canonical rejection: a lint violation on Myelin's own commit reds the graph.
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("lints"));

    assert!(
        !run.is_green(),
        "a deliberately-violating commit (a lint red) MUST be rejected — the ratchet rejects on \
         Myelin's own work (EI-01 §5)"
    );
    assert_eq!(
        run.red_jobs(),
        vec!["lints"],
        "the gate names exactly the red ratchet job (loud, never swallowed)"
    );
    let md = run.render_markdown();
    assert!(md.contains("GATE: RED"), "a red run renders GATE: RED");
    assert!(
        md.contains("lints"),
        "the artifact names the red job so the rejection is auditable"
    );
}

#[test]
fn a_surviving_mutant_rejects_the_commit() {
    // The mutation gate reds (a surviving mutant = a test gap on a mandatory-core module).
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("mutation-gate"));
    assert!(
        !run.is_green(),
        "a surviving mutant on a mandatory-core module MUST reject the commit"
    );
    assert_eq!(run.red_jobs(), vec!["mutation-gate"]);
}

#[test]
fn a_red_substrate_drill_rejects_the_commit() {
    // The harness-driven drills are part of the gate: a red SUB-D6 restore-verify reds the graph.
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("SUB-D6"));
    assert!(
        !run.is_green(),
        "a red substrate drill (SUB-D6) MUST reject the commit — the harness-driven drills are \
         part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["SUB-D6"]);
}

#[test]
fn an_empty_run_is_not_green() {
    // Guard: a run with no jobs is RED, not vacuously GREEN (dropping the whole graph cannot game
    // the gate green — the same un-gameable discipline as the band scorecards).
    let run = run_graph(&[], &all_green);
    assert!(
        !run.is_green(),
        "an empty self-hosting run is RED, never vacuously GREEN — the gate cannot be gamed by \
         dropping every job"
    );
}
