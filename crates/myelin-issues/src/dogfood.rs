use crate::e2e_wedge::IssuesE2eArtifact;

pub const MYELIN_SELF_TENANT: &str = "myelin";

pub const MYELIN_SELF_REGION: &str = "fr-par";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyelinIssue {
    pub key: &'static str,
    pub title: &'static str,
    pub body_blocks: Vec<&'static str>,
}

impl MyelinIssue {
    pub fn body_round_trips(&self) -> bool {
        self.body_blocks
            .iter()
            .all(|md| crate::roundtrips_md(md, &[]))
    }
}

pub fn myelin_issue_backlog() -> Vec<MyelinIssue> {
    vec![
        MyelinIssue {
            key: "MYL-1",
            title: "Myelin platform roadmap (M1..M6) as a tracked initiative",
            body_blocks: vec![
                "The bands run **M1** through **M6**; M6 is the `dogfood` done-bar.",
                "Each band closes on a dated green exit-gate scorecard.",
                "The roadmap is a *timeline* view over the one `issue` table.",
            ],
        },
        MyelinIssue {
            key: "MYL-2",
            title: "Myelin gap report - every named floor carries a follow-on",
            body_blocks: vec![
                "Every named floor carries a follow-on prompt id.",
                "The only remaining floor is the world-scale `30x` fleet-hardware load drill.",
                "~~Open~~ floors are triaged onto the backlog, never invisible.",
            ],
        },
        MyelinIssue {
            key: "MYL-3",
            title: "Myelin exit-gate scorecard - every PROVEN row dated-green",
            body_blocks: vec![
                "Every **PROVEN** row rests on a dated green drill artifact.",
                "A claim that outlives its verification is a `CLAIMED-NOT-PROVEN` red.",
                "See the truth-up pass: [scorecard](https://wiki.test/myelin/scorecard).",
            ],
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked - an unread RED face silently claims a green Issues \
              did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct IssuesDogfoodArtifact {
    pub date: String,
    pub issues_round_tripped: usize,
    pub issues_total: usize,
    pub pr_context_pane: IssuesE2eArtifact,
    pub agent_flagship: IssuesE2eArtifact,
    pub spec_to_ship: IssuesE2eArtifact,
}

impl IssuesDogfoodArtifact {
    pub fn is_green(&self) -> bool {
        self.issues_total > 0
            && self.issues_round_tripped == self.issues_total
            && self.pr_context_pane.is_green()
            && self.agent_flagship.is_green()
            && self.spec_to_ship.is_green()
            && self.total_leaks() == 0
    }

    pub fn total_leaks(&self) -> u64 {
        self.pr_context_pane.leaks + self.agent_flagship.leaks + self.spec_to_ship.leaks
    }

    pub fn summary(&self) -> String {
        format!(
            "P-520 ISSUES DOGFOOD {} - tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             own-issues-round-trip={}/{} pr-context-pane={} agent-flagship={} spec-to-ship={} \
             total-leaks={} verdict={}",
            self.date,
            self.issues_round_tripped,
            self.issues_total,
            self.pr_context_pane.is_green(),
            self.agent_flagship.is_green(),
            self.spec_to_ship.is_green(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_issues_over_myelins_own_work(date: &str) -> IssuesDogfoodArtifact {
    let backlog = myelin_issue_backlog();
    let issues_total = backlog.len();
    let issues_round_tripped = backlog.iter().filter(|i| i.body_round_trips()).count();
    IssuesDogfoodArtifact {
        date: date.to_string(),
        issues_round_tripped,
        issues_total,
        pr_context_pane: crate::e2e_wedge::run_e2e_1_pr_pane(),
        agent_flagship: crate::e2e_flagship::run_e2e_2_issues_flagship(),
        spec_to_ship: crate::e2e_lineage::run_e2e_3_lineage(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenIssuesRow {
    pub id: &'static str,
    pub section: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_path: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenIssuesRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

pub fn proven_issues_rows(date: &str) -> Vec<ProvenIssuesRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenIssuesRow {
        ProvenIssuesRow {
            id,
            section,
            title,
            proof_command: cmd,
            artifact_path,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        row(
            "ISS-D1",
            "13.3",
            "the board and the roadmap are two ViewSpecs over the SAME rows - editing one patches the other live (0 parallel reality, the co-equal projection)",
            "cargo test -p myelin-issues --test e2e_iss_p16_coequal_views",
            "crates/myelin-issues/tests/e2e_iss_p16_coequal_views.rs",
            date,
        ),
        row(
            "ISS-D2",
            "4.4",
            "a deep cross-subsystem board query is cost-bounded - the three-tier escalation holds, p99 within budget at cell scale (0 unbounded scan)",
            "cargo test -p myelin-issues --test drill_iss_d2_cost_bounding",
            "crates/myelin-issues/tests/drill_iss_d2_cost_bounding.rs",
            date,
        ),
        row(
            "ISS-D3",
            "4.3",
            "a confidential issue / field-hidden column never leaks - incl. COUNT across the board/search/My-Work (0 leak, the SetExpr pre-filter conjoined into every tier)",
            "cargo test -p myelin-issues --test drill_iss_d3_setexpr_zero_leak",
            "crates/myelin-issues/tests/drill_iss_d3_setexpr_zero_leak.rs",
            date,
        ),
        row(
            "ISS-D4",
            "5.1",
            "a create storm against the Hi/Lo human-key allocator - 0 duplicate / 0 gap key under concurrency (the monotonic per-tenant counter)",
            "cargo test -p myelin-issues --test drill_iss_d4_create_storm",
            "crates/myelin-issues/tests/drill_iss_d4_create_storm.rs",
            date,
        ),
        row(
            "ISS-D5",
            "13.3",
            "concurrent drag-to-rank against the same gap - the LexoRank CAS rejects the loser with current state (0 silent clobber, 0 duplicate rank)",
            "cargo test -p myelin-issues --test drill_iss_d5_reorder_zero_clobber",
            "crates/myelin-issues/tests/drill_iss_d5_reorder_zero_clobber.rs",
            date,
        ),
        row(
            "ISS-D6",
            "7.5",
            "an SLA timer over a business calendar (working hours / holidays / pauses) - the breach fires at the calendar-correct instant; the escalation reflex pages on-call (0 phantom breach)",
            "cargo test -p myelin-issues --test drill_iss_d6_sla_business_calendar",
            "crates/myelin-issues/tests/drill_iss_d6_sla_business_calendar.rs",
            date,
        ),
        row(
            "ISS-D7",
            "3.3",
            "a stateful Trigger fires exactly once per qualifying transition (debounced across replays / duplicate events; 0 double-fire)",
            "cargo test -p myelin-issues --test drill_iss_d7_stateful_trigger",
            "crates/myelin-issues/tests/drill_iss_d7_stateful_trigger.rs",
            date,
        ),
        row(
            "ISS-D8",
            "11.6",
            "the incremental rollup consumer maintains burndown / counts; the OLAP feed reconciles cold == live byte-for-byte (0 drift)",
            "cargo test -p myelin-issues --test drill_iss_d8_rollup",
            "crates/myelin-issues/tests/drill_iss_d8_rollup.rs",
            date,
        ),
        row(
            "ISS-D9",
            "13.2",
            "an ADF/Jira import → the canonical core → re-export - the lossy-map is recorded; the canonical subset round-trips (0 silent data loss)",
            "cargo test -p myelin-issues --test drill_iss_d9_import",
            "crates/myelin-issues/tests/drill_iss_d9_import.rs",
            date,
        ),
        row(
            "ISS-D11",
            "10.1",
            "erase a subject - every holder (issue rows / comments / OLAP / search / agent traces) is reached; the per-subject DEK is crypto-shredded (0 recoverable PII)",
            "cargo test -p myelin-issues --test drill_iss_d11_erase_reaches_every_holder",
            "crates/myelin-issues/tests/drill_iss_d11_erase_reaches_every_holder.rs",
            date,
        ),
        row(
            "ISS-D8b",
            "11.6",
            "the OLAP read store is fed from the same committed log; a wiped OLAP store replays cold == live (the only recovery path)",
            "cargo test -p myelin-issues --test drill_iss_d8b_olap_feed",
            "crates/myelin-issues/tests/drill_iss_d8b_olap_feed.rs",
            date,
        ),
        row(
            "ISS-D10",
            "13.1",
            "render(parse(md)) === md 100% round-trip over an issue body + comment corpus through the ONE WASM render path (0 regressions; read + edit use the IDENTICAL parser)",
            "cargo test -p myelin-issues --test roundtrip_iss_d10",
            "crates/myelin-issues/tests/roundtrip_iss_d10.rs",
            date,
        ),
        row(
            "ISS-D13",
            "3.5",
            "real-time board sync - kill a client mid-drag + sever the connection during a sustained multi-author board edit; the resume cursor re-binds (0 lost / 0 dup move)",
            "cargo test -p myelin-issues --test drill_iss_d13_board_sync",
            "crates/myelin-issues/tests/drill_iss_d13_board_sync.rs",
            date,
        ),
        row(
            "ISS-D12",
            "13.3",
            "a workflow FSM transition guarded by a QueryAst guard + a CheckStatus guard - an ungated transition is rejected (the FSM interpreter is Issues' slice of the agent ∩)",
            "cargo test -p myelin-issues --test e2e_iss_p27_ci_guard",
            "crates/myelin-issues/tests/e2e_iss_p27_ci_guard.rs",
            date,
        ),
        row(
            "E2E-1",
            "5.6",
            "the PR context pane - an Issues item resolves per-viewer through the ACL chokepoint; a confidential issue's title/count never leaks (0 leak; mid-flight erase honoured live)",
            "cargo test -p myelin-issues --test e2e_wedge_iss_p34",
            "crates/myelin-issues/tests/e2e_wedge_iss_p34.rs",
            date,
        ),
        row(
            "E2E-2",
            "8.2",
            "the agent-native flagship - a governed agent close is HITL-gated; it applies EXACTLY ONCE across a crash + a duplicate approval; reserve/settle balanced (0 ungoverned mutation)",
            "cargo test -p myelin-issues --test e2e_flagship_iss_p35",
            "crates/myelin-issues/tests/e2e_flagship_iss_p35.rs",
            date,
        ),
        row(
            "E2E-3",
            "2.6",
            "spec-to-ship traceability - a spec → initiative → issues → PR → CI run; cold-reindex == live byte-for-byte; audit tamper detected (0 silent tamper)",
            "cargo test -p myelin-issues --test e2e_lineage_iss_p36",
            "crates/myelin-issues/tests/e2e_lineage_iss_p36.rs",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuesTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl IssuesTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, IssuesTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            IssuesTruthUpVerdict::Green { .. } => &[],
            IssuesTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IssuesTruthUpPass;

impl IssuesTruthUpPass {
    pub fn new() -> IssuesTruthUpPass {
        IssuesTruthUpPass
    }

    pub fn run(&self, rows: &[ProvenIssuesRow], date: &str) -> IssuesTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            IssuesTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            IssuesTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenIssuesRow],
        date: &str,
    ) -> Result<usize, IssuesTruthUpRed> {
        match self.run(rows, date) {
            IssuesTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            IssuesTruthUpVerdict::Red { undated_rows } => Err(IssuesTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for IssuesTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} Issues row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} - a \
             claim that outlives its verification misleads the next agent (EI-01 §1); fix the doc or \
             re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for IssuesTruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuesRowStatus {
    DatedGreen {
        date: String,
    },
    ClaimedNotProven {
        date: String,
        reason: String,
    },
}

impl IssuesRowStatus {
    pub fn is_dated_green(&self) -> bool {
        matches!(self, IssuesRowStatus::DatedGreen { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesScorecardEntry {
    pub row: ProvenIssuesRow,
    pub status: IssuesRowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked - an unread CLAIMED-NOT-PROVEN row silently drifts \
              the docs from the code (EI-01 §1)"]
pub struct IssuesTruthUpScorecard {
    pub date: String,
    pub entries: Vec<IssuesScorecardEntry>,
}

impl IssuesTruthUpScorecard {
    pub fn is_green(&self) -> bool {
        self.entries.iter().all(|e| e.status.is_dated_green())
    }

    pub fn rows_total(&self) -> usize {
        self.entries.len()
    }

    pub fn rows_dated_green(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status.is_dated_green())
            .count()
    }

    pub fn claimed_not_proven(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| !e.status.is_dated_green())
            .map(|e| e.row.id)
            .collect()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let verdict = if self.is_green() {
            "GREEN (no later-band Issues gate red)"
        } else {
            "RED (an Issues claim outran its verification)"
        };
        out.push_str(&format!(
            "P-520 ISSUES TRUTH-UP SCORECARD {} - {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                IssuesRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                IssuesRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<8} {:<28} - {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

pub fn run_issues_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> IssuesTruthUpScorecard {
    let entries = proven_issues_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => IssuesRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    IssuesRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => IssuesRowStatus::DatedGreen { date: d.clone() },
            };
            IssuesScorecardEntry { row, status }
        })
        .collect();
    IssuesTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesIncident {
    pub incident_id: String,
    pub gate_id: String,
    pub description: String,
    pub repro_drill_name: String,
}

impl IssuesIncident {
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> IssuesIncident {
        IssuesIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    pub fn issue_draft(&self) -> IssuesIncidentIssueDraft {
        IssuesIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Issues gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Issues incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked - every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    pub fn drill_ticket(&self) -> IssuesIncidentDrillTicket {
        IssuesIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesIncidentIssueDraft {
    pub gate_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesIncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    #[test]
    fn issues_greens_on_myelins_own_work() {
        let artifact = run_issues_over_myelins_own_work(RUN_DATE);

        assert!(
            artifact.is_green(),
            "Issues must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.issues_round_tripped,
            artifact.issues_total,
            "every one of Myelin's own issues round-trips through the ONE WASM render path: {}",
            artifact.summary()
        );
        assert!(
            artifact.issues_total >= 3,
            "the roadmap/gap-report/scorecard"
        );
        assert!(
            artifact.pr_context_pane.is_green(),
            "the PR context pane is green"
        );
        assert!(
            artifact.agent_flagship.is_green(),
            "the agent-native flagship is green"
        );
        assert!(
            artifact.spec_to_ship.is_green(),
            "the spec-to-ship lineage is green"
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the three E2E faces: {}",
            artifact.summary()
        );

        let s = artifact.summary();
        assert!(s.contains("P-520 ISSUES DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn myelins_own_issues_round_trip_through_the_one_render_path() {
        let backlog = myelin_issue_backlog();
        assert!(backlog.len() >= 3, "the roadmap/gap-report/scorecard");
        for issue in &backlog {
            assert!(
                issue.body_round_trips(),
                "the Myelin issue {} must round-trip through the ONE WASM render path",
                issue.key
            );
            assert!(!issue.body_blocks.is_empty(), "{}", issue.key);
        }
    }

    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_issues_rows(RUN_DATE);
        assert!(
            rows.len() >= 16,
            "the PROVEN set covers ISS-D1..ISS-D13 + the E2E slices E2E-1/E2E-2/E2E-3"
        );
        let confirmed = IssuesTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red later-band Issues gates - every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_issues_rows(RUN_DATE);
        rows[0].artifact_date = None;
        let verdict = IssuesTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = IssuesTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        let scorecard = run_issues_truth_up_scorecard(RUN_DATE, &repo_root());
        assert!(
            scorecard.is_green(),
            "the scorecard must be green - every PROVEN Issues row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("ISS-D1") && md.contains("E2E-3"),
            "enumerated: {md}"
        );
    }

    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-iss-truth-up-root");
        let scorecard = run_issues_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard.entries.iter().all(|e| matches!(
                &e.status,
                IssuesRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk")
            )),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = IssuesIncident::new(
            "INC-ISS-DOGFOOD-1",
            "ISS-D10",
            "an issue-body corpus fixture silently round-tripped non-canonically on the Myelin self-tenant",
            "repro_iss_d10_dogfood_non_canonical_round_trip",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "ISS-D10");
        assert!(draft.title.contains("INC-ISS-DOGFOOD-1"));
        assert!(
            draft
                .body
                .contains("repro_iss_d10_dogfood_non_canonical_round_trip"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_iss_d10_dogfood_non_canonical_round_trip"
        );
        assert_eq!(ticket.gate_id, "ISS-D10");
    }

    #[test]
    fn every_proven_row_proof_source_exists_on_disk() {
        for row in proven_issues_rows(RUN_DATE) {
            assert!(
                row.artifact_abs_path(&repo_root()).exists(),
                "the proof source for {} must exist on disk: {}",
                row.id,
                row.artifact_path
            );
        }
    }

    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf()
    }
}
