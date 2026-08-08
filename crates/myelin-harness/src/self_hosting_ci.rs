use crate::scorecard::today_iso;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelfHostJob {
    pub id: &'static str,
    pub title: &'static str,
    pub kind: JobKind,
    pub tool: JobTool,
    pub argv: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Lints,
    ContractCoverage,
    MutationGate,
    Drill,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            JobKind::Lints => "lints",
            JobKind::ContractCoverage => "contract-coverage",
            JobKind::MutationGate => "mutation-gate",
            JobKind::Drill => "drill",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobTool {
    Cargo,
    CargoMutants,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobResult {
    Pass {
        id: String,
        proof: String,
    },
    Red {
        id: String,
        reason: String,
    },
}

impl JobResult {
    pub fn is_pass(&self) -> bool {
        matches!(self, JobResult::Pass { .. })
    }

    pub fn id(&self) -> &str {
        match self {
            JobResult::Pass { id, .. } | JobResult::Red { id, .. } => id,
        }
    }

    pub fn artifact_row(&self, date: &str) -> String {
        match self {
            JobResult::Pass { id, proof } => format!("| `{id}` | PASS | [{date}] {proof} |"),
            JobResult::Red { id, reason } => format!("| `{id}` | **RED** | [{date}] {reason} |"),
        }
    }
}

pub fn self_hosting_jobs() -> Vec<SelfHostJob> {
    vec![
        SelfHostJob {
            id: "lints",
            title: "the twelve architecture lints over Myelin's own source (lint-gate + fixtures)",
            kind: JobKind::Lints,
            tool: JobTool::Cargo,
            argv: &["run", "-p", "myelin-lints", "--bin", "lint-gate"],
        },
        SelfHostJob {
            id: "lints-fixtures",
            title: "the lint fixture matrix + the CI-gate self-test (red fixture ⇒ non-zero exit)",
            kind: JobKind::Lints,
            tool: JobTool::Cargo,
            argv: &["test", "-p", "myelin-lints"],
        },
        SelfHostJob {
            id: "contract-coverage",
            title: "the contract-coverage scanner over the contract-index + manifest",
            kind: JobKind::ContractCoverage,
            tool: JobTool::Cargo,
            argv: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
        },
        SelfHostJob {
            id: "contract-coverage-selftest",
            title: "the scanner self-test (red manifest fixture ⇒ non-zero exit; green ⇒ zero)",
            kind: JobKind::ContractCoverage,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-lints",
                "--test",
                "contract_coverage_gate",
            ],
        },
        SelfHostJob {
            id: "mutation-gate",
            title: "the mandatory-core cargo-mutants mutation gate (outbox/relay/consumer/dedup + \
                    ResilientClient + FailStatic + shed lane)",
            kind: JobKind::MutationGate,
            tool: JobTool::CargoMutants,
            argv: &[],
        },
        SelfHostJob {
            id: "SUB-D3",
            title: "the 30× surge family - the human lane holds, the machine lane sheds (CI smoke)",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d3_surge_family",
            ],
        },
        SelfHostJob {
            id: "SUB-D6",
            title: "restore-verify lands at one consistent point within RPO/RTO (CI smoke)",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d6_restore_verify",
            ],
        },
        SelfHostJob {
            id: "SUB-D10",
            title:
                "migration-under-load - a blocking ALTER blows the budget; online migration holds",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d10_migration_under_load",
            ],
        },
        SelfHostJob {
            id: "tenancy-lints",
            title: "the two Tenancy lints (residency-pin + control-plane-pii-free) bite on a \
                    fixture commit (PII column / out-of-region write) - the ratchet on Myelin's code",
            kind: JobKind::Lints,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-lints",
                "--test",
                "tenancy_lints",
                "--test",
                "tenancy_control_plane_lints",
            ],
        },
        SelfHostJob {
            id: "CP-D23-self_tenant",
            title: "Myelin self-hosts as one cell + residency_verify GREEN on the team's own data \
                    + the truth-up pass (no later-band CP gate red)",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-control-plane",
                "--test",
                "cp_d23_self_tenant_self_host_drill",
            ],
        },
        SelfHostJob {
            id: "ci-pipeline-determinism",
            title: "the ci.pipeline body's bit-identical replay (CI-D9) + crash-recovery (CI-D1) run \
                    as Myelin CI - the durable workflow hosting the Myelin build",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-ci-controlplane",
                "--test",
                "drills_ci_p15_ci_pipeline",
                "--test",
                "drills_ci_p16_effectively_once",
            ],
        },
        SelfHostJob {
            id: "ci-check-seam",
            title: "the Git↔CI check seam on Myelin's own commits (5.9 / CI-D8 - ci.result rollup → \
                    merge-queue wake, exactly-once)",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-ci-controlplane",
                "--test",
                "drills_ci_p19_seam_gate",
            ],
        },
        SelfHostJob {
            id: "ci-e2e-flagship",
            title: "CI's slice of the agent-native E2E flagship (E2E-2) - CI-fail → triage agent → \
                    issue → chat → fix-PR - driven as a Myelin CI job",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-ci-controlplane",
                "--test",
                "drill_ci_p34_e2e2_flagship",
            ],
        },
        SelfHostJob {
            id: "CI-P35-self_tenant",
            title: "the CI switch test (driven against the real `myelin ci` run/log/deploy surface vs \
                    the interactive budget) + the CI consistency pass",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-ci-controlplane",
                "--test",
                "ci_p35_self_tenant_switch_test_drill",
            ],
        },
        SelfHostJob {
            id: "GA-P511-self_tenant",
            title: "the GDPR/Audit machinery on Myelin's own commits - the audit consumer live on the \
                    self-hosting outbox + a self-served DSR fans out + seals a certificate + the \
                    RoPA/data-map Knowledge space + the GDPR truth-up pass (0 red earlier GDPR gate)",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-gdpr-service",
                "--test",
                "ga_p511_self_tenant_self_served_dsr_drill",
            ],
        },
        SelfHostJob {
            id: "REF-P28-self_tenant",
            title: "the reference graph on Myelin's own work - the PR context pane + the spec-to-ship \
                    lineage + the holder fan-out (all green, 0 leak) + the Refs truth-up pass (0 red \
                    earlier-band Refs gate) + the self-hosted every-incident-adds-a-drill loop",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-refs-service",
                "--test",
                "ref_p28_self_tenant_drill",
            ],
        },
        SelfHostJob {
            id: "REF-P29-switch-test",
            title: "the reference-graph switch test driven over the real surface - the four-keystroke \
                    cross-artifact jump works vs the four-tool anchor (0 walls), measured within the \
                    latency budgets (backlink/unfurl/no-spinner-flash), 0 leak",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-refs-service",
                "--test",
                "ref_p29_switch_test_drill",
            ],
        },
        SelfHostJob {
            id: "SRCH-P33-self_tenant",
            title: "Search on Myelin's own work - code + issue search + the Knowledge-space \
                    reindex-parity + the DSAR fan-out (all green, 0 leak) + the Search truth-up pass \
                    (0 red earlier-band Search gate) + the self-hosted every-incident-adds-a-drill loop",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-search",
                "--test",
                "srch_p33_self_tenant_drill",
            ],
        },
        SelfHostJob {
            id: "SRCH-P33-switch-test",
            title: "the Search switch test driven over the real surface - code-by-symbol / \
                    doc-by-content / issue-by-facet found vs the three-tool anchor (0 walls), measured \
                    within the latency budgets, 0 leak",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-search",
                "--test",
                "srch_p33_switch_test_drill",
            ],
        },
        SelfHostJob {
            id: "FLOW-P29-self_tenant",
            title: "Myelin's own pipelines/merge-queue/SLA-timers as myelin-flow workflows - the \
                    ci.pipeline workflow + the merge queue merging a real Myelin PR exactly once + a \
                    real Myelin SLA timer firing on a real Myelin issue (all green) + the FLOW truth-up \
                    pass (0 red earlier-band FLOW gate) + the self-hosted every-incident-adds-a-drill loop",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-flow",
                "--test",
                "flow_p29_self_tenant_drill",
            ],
        },
        SelfHostJob {
            id: "AG-P26-self_tenant",
            title: "the platform's own agents on Myelin's own work - a MOCK triage agent on a real \
                    Myelin CI failure (explicit-first dispatch + balanced reserve/settle ledger + a \
                    content-addressed trace per run) + the Fabric truth-up pass (0 red later-band \
                    Fabric gate) + the self-hosted every-incident-adds-a-drill loop",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-agent-service",
                "--test",
                "ag_p26_self_tenant_drill",
            ],
        },
        SelfHostJob {
            id: "GIT-P35-self_tenant",
            title: "git hosts Myelin's own repositories - the PR context pane + the agent-native fix-PR \
                    flagship (exactly-once merge) + the spec-to-ship lineage (all green, 0 leak) + the \
                    git truth-up pass (0 red later-band git gate) + the self-hosted \
                    every-incident-adds-a-drill loop",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-git",
                "--test",
                "git_p35_self_tenant_drill",
            ],
        },
        SelfHostJob {
            id: "GIT-P35-switch-test",
            title: "the Git OQ-12 switch test driven over the real surface - the PR overview render \
                    within budget + render(parse(md)) === md at 100% + every status overlay at ≥ 4.5:1 \
                    contrast with no blocked workflow requirements",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-git",
                "--test",
                "git_p35_switch_test_drill",
            ],
        },
        SelfHostJob {
            id: "ISS-P37-self_tenant",
            title: "Myelin tracks its own issues - own work as Myelin issues (round-trip) + the PR \
                    context pane + the agent-native flagship (exactly-once close) + the spec-to-ship \
                    lineage (all green, 0 leak) + the Issues truth-up pass (0 red later-band Issues \
                    gate) + the self-hosted every-incident-adds-a-drill loop",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-issues",
                "--test",
                "iss_p37_self_tenant_drill",
            ],
        },
        SelfHostJob {
            id: "ISS-P37-switch-test",
            title: "the Issues ISS-D14 switch test driven over the real surface - the \
                    create→triage→plan→board→done loop without a manual + the canonical view render \
                    within budget + render(parse(md)) === md at 100% + every overlay at ≥ 4.5:1 contrast \
                    + every primary-screen state reached",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-issues",
                "--test",
                "iss_p37_switch_test_drill",
            ],
        },
    ]
}

#[derive(Clone, Debug)]
pub struct SelfHostingRun {
    pub date: String,
    pub results: Vec<JobResult>,
}

impl SelfHostingRun {
    pub fn is_green(&self) -> bool {
        !self.results.is_empty() && self.results.iter().all(JobResult::is_pass)
    }

    pub fn red_jobs(&self) -> Vec<&str> {
        self.results
            .iter()
            .filter(|r| !r.is_pass())
            .map(JobResult::id)
            .collect()
    }

    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# Myelin self-hosting CI graph - the self_tenant loop (P-507 / P-S37, SUB-M6)\n\n",
        );
        out.push_str(&format!("Run date: {}\n\n", self.date));
        out.push_str(
            "The substrate ratchet (the twelve architecture lints + the contract-coverage scanner \
             + the mandatory-core cargo-mutants mutation gate) runs as Myelin CI jobs on Myelin's \
             OWN commit, and the harness drives the substrate's surge/restore/migration drills \
             (SUB-D3/D6/D10) - the self_tenant loop is live. The gate is GREEN iff every job below \
             passed; a single red job reds the gate (the ratchet rejects on Myelin's own work).\n\n",
        );
        out.push_str("| Job | Verdict | Proof / reason |\n");
        out.push_str("|---|---|---|\n");
        for r in &self.results {
            out.push_str(&r.artifact_row(&self.date));
            out.push('\n');
        }
        out.push('\n');
        if self.is_green() {
            out.push_str("**GATE: GREEN** - the self-hosting CI graph is green on Myelin's own commit (SUB-M6).\n");
        } else {
            out.push_str(&format!(
                "**GATE: RED** - the self_tenant ratchet rejected this commit; red jobs: {}.\n",
                self.red_jobs().join(", ")
            ));
        }
        out
    }
}

pub type JobRunner<'a> = dyn Fn(&SelfHostJob) -> JobResult + 'a;

pub fn run_graph(jobs: &[SelfHostJob], run: &JobRunner<'_>) -> SelfHostingRun {
    let date = today_iso();
    let results = jobs.iter().map(run).collect();
    SelfHostingRun { date, results }
}

pub fn run_job_via_cargo(job: &SelfHostJob) -> JobResult {
    let mut cmd = Command::new(env!("CARGO"));
    let mut shown = String::from("cargo");
    if matches!(job.tool, JobTool::CargoMutants) {
        cmd.arg("mutants");
        shown.push_str(" mutants");
    }
    cmd.args(job.argv);
    for a in job.argv {
        shown.push(' ');
        shown.push_str(a);
    }
    match cmd.status() {
        Ok(status) if status.success() => JobResult::Pass {
            id: job.id.to_string(),
            proof: format!("PASS `{shown}`"),
        },
        Ok(status) => JobResult::Red {
            id: job.id.to_string(),
            reason: format!("`{shown}` exited non-zero ({status}) - the ratchet read RED"),
        },
        Err(e) => JobResult::Red {
            id: job.id.to_string(),
            reason: format!("could not spawn `{shown}`: {e}"),
        },
    }
}
