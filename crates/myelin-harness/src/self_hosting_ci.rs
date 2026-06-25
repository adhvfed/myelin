//! The **Myelin self-hosting CI graph** — the dogfood loop (P-507 / P-S37 → M6).
//!
//! ## What this module is (the dogfood loop is live)
//! The cheapest, most honest load generator is the platform's own development (substrate roadmap
//! §2 SUB-M6 thesis; EI-01 §5 — *the ratchet runs on the builders' own work*). This module is the
//! **Myelin CI pipeline definition** that wires the substrate's M0 ratchet to run **as Myelin CI
//! jobs on every Myelin commit**:
//!
//! 1. **the twelve architecture lints** (P-S10/P-S11, contract 1.6 / arch §2.11) — `lint-gate`'s
//!    exit code over the workspace + the `myelin-lints` fixture matrix;
//! 2. **the contract-coverage scanner** (P-S21) — `contract-coverage`'s exit code + its self-test;
//! 3. **the mandatory-core cargo-mutants mutation gate** (ledger-overview §6) — `cargo mutants`
//!    over the correctness-critical surface declared in `.cargo/mutants.toml`
//!    (outbox/relay/consumer/dedup + `ResilientClient` + `FailStatic` + the shed lane);
//! 4. **the substrate's own surge / restore / migration drills** SUB-D3 / SUB-D6 / SUB-D10 — the
//!    harness drives them as part of the self-hosting graph (the M6 work item: "the harness drives
//!    the substrate's own surge/restore/migration drills as part of the self-hosting CI graph").
//!
//! ## Why a graph of jobs, not a hard-coded shell script (the ratchet, EI-01 §5)
//! "An uncommitted gate is no gate." The graph is FROZEN DATA ([`self_hosting_jobs`]) — an ordered
//! `Vec<SelfHostJob>`, each naming its stable id, its human title, and the exact proof command
//! (argv, no shell) that emits its dated green artifact. The runner binary
//! (`src/bin/self-hosting-ci.rs`) executes the graph on Myelin's OWN commit (HEAD), records each
//! job PASS / RED into a dated artifact, and **exits non-zero on any red** — the gate IS the
//! process exit code, so there is no `... || true` swallow path possible. A deliberately-violating
//! commit (a lint red, a surviving mutant, a red drill) makes a job exit non-zero, which reds the
//! whole graph: the ratchet rejects on Myelin's own work (the SUB-M6 exit gate).
//!
//! ## Where it sits (the leaf test-support crate, NOT a production DAG node)
//! Like the band-boundary scorecard runners ([`crate::scorecard`]), this lives in `myelin-harness`
//! (architecture §2.9: a leaf test-support crate above `myelin-substrate`, the way a service's
//! `main.rs` is). It SHELLS OUT to `cargo` / `cargo mutants` exactly the way `sub-m0-scorecard`
//! does — the one legitimate host-exec site for CI orchestration tooling (named, loud; it is never
//! reachable on a user/agent request path, so the `no-host-exec` rule that guards PLATFORM code
//! does not apply — see the `lint-gate` crate note). It WIRES the existing lints/scanner/drills; it
//! does NOT re-implement them (P-S37 DELIVERABLE — "wire the substrate's ratchet into the
//! self-hosting Myelin CI graph").
//!
//! ## What this prompt does NOT ship (split, named)
//! The every-incident-adds-a-drill loop on Myelin's tracker + the truth-up pass are P-S38 (P-510).

use crate::scorecard::today_iso;
use std::process::Command;

/// **One job of the Myelin self-hosting CI graph (the dogfood pipeline).** A job is one ratchet
/// step run on Myelin's own commit: it names a stable `id`, a one-line `title`, the exact `argv`
/// the runner invokes, and which `tool` runs it (plain `cargo` vs `cargo mutants`). Every field is
/// frozen data — adding a job is a `self_hosting_jobs` edit, the row set is auditable, and the
/// proof command is run directly (no shell), so a non-zero exit is a RED row that cannot be
/// swallowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelfHostJob {
    /// The stable job id (e.g. `"lints"`, `"contract-coverage"`, `"mutation-gate"`, `"SUB-D3"`).
    pub id: &'static str,
    /// A one-line human title for the rendered artifact.
    pub title: &'static str,
    /// The job's family (which ratchet primitive it dogfoods).
    pub kind: JobKind,
    /// The tool the proof command runs under.
    pub tool: JobTool,
    /// The proof-command argv (the args AFTER the tool binary), run directly — no shell, so a
    /// non-zero exit is a RED row, never softened.
    pub argv: &'static [&'static str],
}

/// Which ratchet primitive a self-hosting job dogfoods (for the rendered artifact's grouping +
/// the SUB-M6 exit-gate readout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// The twelve architecture lints (contract 1.6 / arch §2.11).
    Lints,
    /// The contract-coverage scanner (P-S21).
    ContractCoverage,
    /// The mandatory-core cargo-mutants mutation gate (ledger-overview §6).
    MutationGate,
    /// One of the substrate's surge/restore/migration drills (SUB-D3/D6/D10) the harness drives.
    Drill,
}

impl JobKind {
    /// A short label for the rendered artifact.
    pub fn label(self) -> &'static str {
        match self {
            JobKind::Lints => "lints",
            JobKind::ContractCoverage => "contract-coverage",
            JobKind::MutationGate => "mutation-gate",
            JobKind::Drill => "drill",
        }
    }
}

/// The tool a job's proof command runs under. The mutation gate runs under `cargo mutants` (the
/// extra subcommand); every other ratchet job runs under plain `cargo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobTool {
    /// Plain `cargo <argv>` (the lints, the scanner, the drills).
    Cargo,
    /// `cargo mutants <argv>` — the mandatory-core mutation gate (reads `.cargo/mutants.toml`).
    CargoMutants,
}

/// The verdict of running one self-hosting job — a typed result, never a swallowed bool. A FAIL
/// carries the reason (the non-zero exit / spawn error) so the artifact names exactly what reds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobResult {
    /// The job's proof command exited 0 — a dated green artifact row.
    Pass {
        /// The job id.
        id: String,
        /// The dated proof line the runner records.
        proof: String,
    },
    /// The job's proof command read RED (non-zero exit) or could not be spawned.
    Red {
        /// The job id.
        id: String,
        /// Why this job reds (the non-zero exit / spawn error).
        reason: String,
    },
}

impl JobResult {
    /// `true` iff this job passed.
    pub fn is_pass(&self) -> bool {
        matches!(self, JobResult::Pass { .. })
    }

    /// The job id this result is for.
    pub fn id(&self) -> &str {
        match self {
            JobResult::Pass { id, .. } | JobResult::Red { id, .. } => id,
        }
    }

    /// The rendered scorecard row for this result on `date`.
    pub fn artifact_row(&self, date: &str) -> String {
        match self {
            JobResult::Pass { id, proof } => format!("| `{id}` | PASS | [{date}] {proof} |"),
            JobResult::Red { id, reason } => format!("| `{id}` | **RED** | [{date}] {reason} |"),
        }
    }
}

/// **The FROZEN Myelin self-hosting CI graph (the dogfood pipeline definition).** The ordered set
/// of ratchet jobs that run on every Myelin commit (SUB-M6). The order is intentional — the cheap,
/// fast gates first (lints, scanner), then the mandatory-core mutation gate, then the substrate's
/// own surge/restore/migration drills. The runner runs them ALL (it does not fail-fast — every red
/// is reported in one pass so the artifact is complete), then reds the gate iff ANY job is red.
pub fn self_hosting_jobs() -> Vec<SelfHostJob> {
    vec![
        // (1) the twelve architecture lints — the lint-gate exit code + the fixture matrix.
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
        // (2) the contract-coverage scanner — the meta-gate exit code + its self-test.
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
        // (3) the mandatory-core cargo-mutants mutation gate — reads .cargo/mutants.toml.
        SelfHostJob {
            id: "mutation-gate",
            title: "the mandatory-core cargo-mutants mutation gate (outbox/relay/consumer/dedup + \
                    ResilientClient + FailStatic + shed lane)",
            kind: JobKind::MutationGate,
            tool: JobTool::CargoMutants,
            // No extra args — the examine/exclude/test surface is the committed .cargo/mutants.toml.
            argv: &[],
        },
        // (4) the substrate's own surge / restore / migration drills (SUB-D3/D6/D10).
        SelfHostJob {
            id: "SUB-D3",
            title: "the 30× surge family — the human lane holds, the machine lane sheds (CI smoke)",
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
                "migration-under-load — a blocking ALTER blows the budget; online migration holds",
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
        // (5) the TENANCY dogfood band (P-508 / P-CP-23 → CP-M6): Myelin self-hosts as exactly one
        //     cell + the two Tenancy lints run as Myelin CI jobs on the platform's own commit. The
        //     `residency-pin` + `control-plane-pii-free` lints are part of the twelve-lint `lints`
        //     job above; these jobs prove the two Tenancy-OWNED lints still BITE on a fixture commit
        //     (a PII column / an out-of-region write), and drive the team-tenant residency-verify +
        //     the truth-up pass — the dogfood loop for Tenancy (no new gate, no new floor).
        SelfHostJob {
            id: "tenancy-lints",
            title: "the two Tenancy lints (residency-pin + control-plane-pii-free) bite on a \
                    fixture commit (PII column / out-of-region write) — the ratchet on Myelin's code",
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
            id: "CP-D23-dogfood",
            title: "Myelin self-hosts as one cell + residency_verify GREEN on the team's own data \
                    + the truth-up pass (no later-band CP gate red)",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-control-plane",
                "--test",
                "cp_d23_dogfood_self_host_drill",
            ],
        },
        // (6) the CI dogfood band (P-509 / CI-P35 → CI-M6): the done-bar. The Myelin build/test/lint/
        //     mutation pipeline runs AS a Myelin `ci.pipeline` — the `ci.pipeline` body's determinism
        //     (CI-D9) + crash-recovery (CI-D1) + the Git↔CI check seam (CI-D8) run as Myelin CI jobs on
        //     the platform's own commit; CI's whole-system E2E flagship (E2E-2) is driven; and the
        //     CI-P35 dogfood drill (the switch test driven against the real `myelin ci` run/log/deploy
        //     surface + the CI truth-up pass + the self-hosted every-incident-adds-a-drill loop) is the
        //     done-bar gate. These WIRE the existing CI drills (EI-01 §7 — never re-implemented here).
        SelfHostJob {
            id: "ci-pipeline-determinism",
            title: "the ci.pipeline body's bit-identical replay (CI-D9) + crash-recovery (CI-D1) run \
                    as Myelin CI — the durable workflow hosting the Myelin build",
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
            title: "the Git↔CI check seam on Myelin's own commits (5.9 / CI-D8 — ci.result rollup → \
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
            title: "CI's slice of the agent-native E2E flagship (E2E-2) — CI-fail → triage agent → \
                    issue → chat → fix-PR — driven as a Myelin CI job",
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
            id: "CI-P35-dogfood",
            title: "the CI switch test (driven against the real `myelin ci` run/log/deploy surface vs \
                    the GitHub Actions anchor, measured) + the CI truth-up pass (0 red earlier CI gate)",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-ci-controlplane",
                "--test",
                "ci_p35_dogfood_switch_test_drill",
            ],
        },
        // (7) the GDPR/Audit dogfood band (P-511 / P-GA-37 → GA-M6): the GDPR/Audit machinery runs on
        //     Myelin's OWN commits — the audit consumer is live on the platform's own actions (the
        //     audit graph is green on Myelin's own commits), a self-served DSR over a Myelin team
        //     member's own data fans out (GA-D1 + GA-D8) + seals a certificate, the RoPA/data-map lives
        //     as a Myelin Knowledge space, the every-incident-adds-a-drill loop is self-hosted, and the
        //     truth-up pass confirms 0 red earlier-band GDPR gates. WIRES the existing GDPR machinery
        //     (EI-01 §7 — never re-implemented here).
        SelfHostJob {
            id: "GA-P511-dogfood",
            title: "the GDPR/Audit machinery on Myelin's own commits — the audit consumer live on the \
                    self-hosting outbox + a self-served DSR fans out + seals a certificate + the \
                    RoPA/data-map Knowledge space + the GDPR truth-up pass (0 red earlier GDPR gate)",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-gdpr-service",
                "--test",
                "ga_p511_dogfood_self_served_dsr_drill",
            ],
        },
        // (8) the REFERENCE GRAPH dogfood band (P-513 / REF-P28 → REF-M6): the reference graph runs over
        //     Myelin's OWN work — the PR context pane on the Myelin monorepo's PRs (commits ↔ issues ↔ CI
        //     ↔ KN docs ↔ chat), the spec-to-ship lineage on Myelin's roadmap/scorecard as Myelin issues
        //     + a Knowledge space, and the structural-erasure holder fan-out over a team member's own data
        //     — all green, 0 leak; the Refs truth-up pass confirms 0 red earlier-band Refs gates; the
        //     every-incident-adds-a-drill loop is self-hosted. WIRES the existing Refs surface + drills
        //     (EI-01 §7 — never re-implemented here). The switch-test browser drive is the named floor →
        //     REF-P29.
        SelfHostJob {
            id: "REF-P28-dogfood",
            title: "the reference graph on Myelin's own work — the PR context pane + the spec-to-ship \
                    lineage + the holder fan-out (all green, 0 leak) + the Refs truth-up pass (0 red \
                    earlier-band Refs gate) + the self-hosted every-incident-adds-a-drill loop",
            kind: JobKind::Drill,
            tool: JobTool::Cargo,
            argv: &[
                "test",
                "-p",
                "myelin-refs-service",
                "--test",
                "ref_p28_dogfood_drill",
            ],
        },
        // (9) the REFERENCE GRAPH switch-test band (P-514 / REF-P29 → REF-M6): the switch test driven
        //     over the real Refs surface — the four-keystroke cross-artifact jump (failing-test → line of
        //     code → issue → conversation, across the five subsystems) works without hitting a wall the
        //     four-tool anchor (GitHub/Jira/Linear/Notion/Slack) didn't have, MEASURED against the
        //     latency budgets (backlink read / unfurl within the keyboard / no-spinner-flash), 0 leak.
        //     WIRES the existing resolve chokepoint (EI-01 §7 — never re-implemented here). The
        //     pixel-level browser drive over the rendered Refs web tier is the honest named floor.
        SelfHostJob {
            id: "REF-P29-switch-test",
            title: "the reference-graph switch test driven over the real surface — the four-keystroke \
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
    ]
}

/// The aggregated verdict of one self-hosting CI graph run on a Myelin commit. GREEN iff EVERY job
/// passed (the SUB-M6 exit gate: the self-hosting CI graph is green on the platform's own commits).
#[derive(Clone, Debug)]
pub struct SelfHostingRun {
    /// The ISO-8601 date the run was asserted.
    pub date: String,
    /// The job results, in graph order.
    pub results: Vec<JobResult>,
}

impl SelfHostingRun {
    /// `true` iff every job in the graph passed (the dogfood gate is GREEN).
    pub fn is_green(&self) -> bool {
        !self.results.is_empty() && self.results.iter().all(JobResult::is_pass)
    }

    /// The ids of the red jobs (empty iff GREEN). Loud, never swallowed.
    pub fn red_jobs(&self) -> Vec<&str> {
        self.results
            .iter()
            .filter(|r| !r.is_pass())
            .map(JobResult::id)
            .collect()
    }

    /// Render the dated committed artifact (the self-hosting CI scorecard body).
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# Myelin self-hosting CI graph — the dogfood loop (P-507 / P-S37, SUB-M6)\n\n",
        );
        out.push_str(&format!("Run date: {}\n\n", self.date));
        out.push_str(
            "The substrate ratchet (the twelve architecture lints + the contract-coverage scanner \
             + the mandatory-core cargo-mutants mutation gate) runs as Myelin CI jobs on Myelin's \
             OWN commit, and the harness drives the substrate's surge/restore/migration drills \
             (SUB-D3/D6/D10) — the dogfood loop is live. The gate is GREEN iff every job below \
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
            out.push_str("**GATE: GREEN** — the self-hosting CI graph is green on Myelin's own commit (SUB-M6).\n");
        } else {
            out.push_str(&format!(
                "**GATE: RED** — the dogfood ratchet rejected this commit; red jobs: {}.\n",
                self.red_jobs().join(", ")
            ));
        }
        out
    }
}

/// The signature of a job runner: given a job, return its [`JobResult`]. Injectable so the
/// rejection test can drive the graph with a stub runner (a deliberately-violating commit) without
/// shelling out to `cargo`, while the binary uses [`run_job_via_cargo`] (the real proof command).
pub type JobRunner<'a> = dyn Fn(&SelfHostJob) -> JobResult + 'a;

/// Run the whole self-hosting graph with `run` (the injected runner), returning a dated
/// [`SelfHostingRun`]. Runs every job (no fail-fast) so the artifact is complete in one pass.
pub fn run_graph(jobs: &[SelfHostJob], run: &JobRunner<'_>) -> SelfHostingRun {
    let date = today_iso();
    let results = jobs.iter().map(run).collect();
    SelfHostingRun { date, results }
}

/// Run one job by shelling out to its real proof command (the binary's runner). `cargo <argv>` for
/// the lints/scanner/drills, `cargo mutants <argv>` for the mutation gate (which reads
/// `.cargo/mutants.toml`). LOUD: the child's output is inherited so a failing job's own red
/// artifact prints; a non-zero exit (or a spawn failure) is a [`JobResult::Red`], never softened.
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
            reason: format!("`{shown}` exited non-zero ({status}) — the ratchet read RED"),
        },
        Err(e) => JobResult::Red {
            id: job.id.to_string(),
            reason: format!("could not spawn `{shown}`: {e}"),
        },
    }
}
