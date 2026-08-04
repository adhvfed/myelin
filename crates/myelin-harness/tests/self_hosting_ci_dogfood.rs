use myelin_harness::self_hosting_ci::{
    run_graph, self_hosting_jobs, JobKind, JobResult, JobTool, SelfHostJob,
};

fn all_green(job: &SelfHostJob) -> JobResult {
    JobResult::Pass {
        id: job.id.to_string(),
        proof: format!("stub PASS `{}`", job.id),
    }
}

fn reds_one(violating: &'static str) -> impl Fn(&SelfHostJob) -> JobResult {
    move |job: &SelfHostJob| {
        if job.id == violating {
            JobResult::Red {
                id: job.id.to_string(),
                reason: format!("stub RED `{}` - the deliberately-violating commit", job.id),
            }
        } else {
            all_green(job)
        }
    }
}

#[test]
fn the_graph_carries_the_substrate_ratchet_and_drives_the_drills() {
    let jobs = self_hosting_jobs();

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

    assert!(
        jobs.iter()
            .any(|j| j.id == "contract-coverage" && j.kind == JobKind::ContractCoverage),
        "the self-hosting graph MUST run the contract-coverage scanner on Myelin's own commit"
    );

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
    let jobs = self_hosting_jobs();

    assert!(
        jobs.iter()
            .any(|j| j.id == "tenancy-lints" && j.kind == JobKind::Lints),
        "the self-hosting graph MUST run the two Tenancy lints as Myelin CI jobs on the platform's \
         own commit (P-CP-23 - the ratchet bites on a PII column / out-of-region write)"
    );

    assert!(
        jobs.iter()
            .any(|j| j.id == "CP-D23-dogfood" && j.kind == JobKind::Drill),
        "the harness MUST drive the Tenancy dogfood drill (self-host + residency_verify on own data \
         + truth-up) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_tenancy_lint_commit_is_rejected() {
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
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("CP-D23-dogfood"));
    assert!(
        !run.is_green(),
        "a red Tenancy dogfood drill (residency mismatch / claimed-not-proven row) MUST reject the \
         commit - the truth-up pass is part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["CP-D23-dogfood"]);
}

#[test]
fn the_graph_runs_the_ci_dogfood_band() {
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
            "the self-hosting graph MUST run the CI dogfood band job `{id}` - {why}"
        );
    }
}

#[test]
fn a_red_ci_dogfood_drill_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("CI-P35-dogfood"));
    assert!(
        !run.is_green(),
        "a red CI dogfood drill (a switch-test wall / a claimed-not-proven CI row) MUST reject the \
         commit - the switch test + truth-up pass are part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["CI-P35-dogfood"]);
}

#[test]
fn the_graph_runs_the_gdpr_dogfood_band() {
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
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("GA-P511-dogfood"));
    assert!(
        !run.is_green(),
        "a red GDPR dogfood drill (a broken own-commit audit chain / a missed DSR holder / a \
         claimed-not-proven GDPR row) MUST reject the commit - the GDPR machinery on Myelin's own \
         commits is part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["GA-P511-dogfood"]);
}

#[test]
fn the_graph_runs_the_refs_dogfood_band() {
    let jobs = self_hosting_jobs();

    assert!(
        jobs.iter()
            .any(|j| j.id == "REF-P28-dogfood" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the Refs dogfood band (the reference graph on Myelin's own \
         work + the Refs truth-up pass) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_refs_dogfood_drill_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("REF-P28-dogfood"));
    assert!(
        !run.is_green(),
        "a red Refs dogfood drill (a leak on Myelin's own work / a claimed-not-proven Refs row) MUST \
         reject the commit - the reference graph on Myelin's own commits is part of the self-hosting \
         CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["REF-P28-dogfood"]);
}

#[test]
fn the_self_hosting_graph_runs_the_refs_switch_test_band() {
    let jobs = self_hosting_jobs();
    assert!(
        jobs.iter()
            .any(|j| j.id == "REF-P29-switch-test" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the Refs switch-test band (the four-keystroke cross-artifact \
         jump driven + measured) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_refs_switch_test_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("REF-P29-switch-test"));
    assert!(
        !run.is_green(),
        "a red Refs switch test (a wall / a blown budget / a leak in the four-keystroke jump) MUST \
         reject the commit - the switch test is part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["REF-P29-switch-test"]);
}

#[test]
fn the_graph_runs_the_search_dogfood_band() {
    let jobs = self_hosting_jobs();

    assert!(
        jobs.iter()
            .any(|j| j.id == "SRCH-P33-dogfood" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the Search dogfood band (Search on Myelin's own work + the \
         Search truth-up pass) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_search_dogfood_drill_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("SRCH-P33-dogfood"));
    assert!(
        !run.is_green(),
        "a red Search dogfood drill (a leak on Myelin's own work / a reindex-parity break / a \
         claimed-not-proven Search row) MUST reject the commit - Search on Myelin's own commits is \
         part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["SRCH-P33-dogfood"]);
}

#[test]
fn the_self_hosting_graph_runs_the_search_switch_test_band() {
    let jobs = self_hosting_jobs();
    assert!(
        jobs.iter()
            .any(|j| j.id == "SRCH-P33-switch-test" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the Search switch-test band (the three interactive finds \
         driven + measured) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_search_switch_test_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("SRCH-P33-switch-test"));
    assert!(
        !run.is_green(),
        "a red Search switch test (a wall / a blown budget / a leak in a find) MUST reject the \
         commit - the switch test is part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["SRCH-P33-switch-test"]);
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
        "every job in the frozen graph is run (no fail-fast - the artifact is complete in one pass)"
    );
    let md = run.render_markdown();
    assert!(
        md.contains("GATE: GREEN"),
        "a green run renders GATE: GREEN"
    );
    assert!(md.contains(&run.date), "the artifact carries the run date");
}

#[test]
fn a_deliberately_violating_lint_commit_is_rejected() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("lints"));

    assert!(
        !run.is_green(),
        "a deliberately-violating commit (a lint red) MUST be rejected - the ratchet rejects on \
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
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("SUB-D6"));
    assert!(
        !run.is_green(),
        "a red substrate drill (SUB-D6) MUST reject the commit - the harness-driven drills are \
         part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["SUB-D6"]);
}

#[test]
fn the_self_hosting_graph_runs_the_durable_workflow_dogfood_band() {
    let jobs = self_hosting_jobs();
    assert!(
        jobs.iter()
            .any(|j| j.id == "FLOW-P29-dogfood" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the durable-workflow dogfood band (Myelin's pipelines/\
         merge-queue/SLA-timers as myelin-flow workflows + the FLOW truth-up pass) as part of the \
         self-hosting CI graph"
    );
}

#[test]
fn a_red_flow_dogfood_drill_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("FLOW-P29-dogfood"));
    assert!(
        !run.is_green(),
        "a red FLOW dogfood drill (a re-dispatch / a double-merge on Myelin's own PR / an SLA \
         timer that didn't fire / a claimed-not-proven FLOW row) MUST reject the commit - Myelin's \
         own workflows on Myelin's own commits are part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["FLOW-P29-dogfood"]);
}

#[test]
fn the_self_hosting_graph_runs_the_agent_fabric_dogfood_band() {
    let jobs = self_hosting_jobs();
    assert!(
        jobs.iter()
            .any(|j| j.id == "AG-P26-dogfood" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the agent-fabric dogfood band (the platform's own agents on \
         Myelin's own work + the Fabric truth-up pass) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_agent_fabric_dogfood_drill_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("AG-P26-dogfood"));
    assert!(
        !run.is_green(),
        "a red Fabric dogfood drill (an unbalanced ledger / an interrupted in-flight run / a \
         claimed-not-proven Fabric row) MUST reject the commit - the platform's own agents on \
         Myelin's own commits are part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["AG-P26-dogfood"]);
}

#[test]
fn the_self_hosting_graph_runs_the_git_hosting_dogfood_band() {
    let jobs = self_hosting_jobs();
    assert!(
        jobs.iter()
            .any(|j| j.id == "GIT-P35-dogfood" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the git-hosting dogfood band (git hosts Myelin's own \
         repositories + the git truth-up pass) as part of the self-hosting CI graph"
    );
}

#[test]
fn a_red_git_hosting_dogfood_drill_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("GIT-P35-dogfood"));
    assert!(
        !run.is_green(),
        "a red git dogfood drill (a leak / a double-merge / a broken lineage / a claimed-not-proven git \
         row) MUST reject the commit - git hosting Myelin's own repositories is part of the self-hosting \
         CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["GIT-P35-dogfood"]);
}

#[test]
fn the_self_hosting_graph_runs_the_git_switch_test_band() {
    let jobs = self_hosting_jobs();
    assert!(
        jobs.iter()
            .any(|j| j.id == "GIT-P35-switch-test" && j.kind == JobKind::Drill),
        "the self-hosting graph MUST run the Git OQ-12 switch-test band (the PR overview render + the \
         markdown round-trip + the status overlays, driven over the real surface) as a Myelin CI job"
    );
}

#[test]
fn a_red_git_switch_test_drill_rejects_the_commit() {
    let jobs = self_hosting_jobs();
    let run = run_graph(&jobs, &reds_one("GIT-P35-switch-test"));
    assert!(
        !run.is_green(),
        "a red git switch-test drill (a wall / a blown render budget / a broken round-trip / a sub-floor \
         overlay) MUST reject the commit - the Git OQ-12 switch test is part of the self-hosting CI gate"
    );
    assert_eq!(run.red_jobs(), vec!["GIT-P35-switch-test"]);
}

#[test]
fn an_empty_run_is_not_green() {
    let run = run_graph(&[], &all_green);
    assert!(
        !run.is_green(),
        "an empty self-hosting run is RED, never vacuously GREEN - the gate cannot be gamed by \
         dropping every job"
    );
}
