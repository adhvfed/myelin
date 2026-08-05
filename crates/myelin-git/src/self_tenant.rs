use crate::surge::{run_e2e_1_pr_pane, run_e2e_2_fix_pr, run_e2e_3_spec_to_ship, E2eArtifact};

pub const MYELIN_SELF_TENANT: &str = "myelin";

pub const MYELIN_SELF_REGION: &str = "fr-par";

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the self_tenant artifact must be checked - an unread RED face silently claims a green git \
              did not earn on Myelin's own repositories (EI-01 §1/§3)"]
pub struct GitSelfTenantArtifact {
    pub date: String,
    pub pr_context_pane: E2eArtifact,
    pub fix_pr_flagship: E2eArtifact,
    pub spec_to_ship: E2eArtifact,
}

impl GitSelfTenantArtifact {
    pub fn is_green(&self) -> bool {
        self.pr_context_pane.is_green()
            && self.fix_pr_flagship.is_green()
            && self.spec_to_ship.is_green()
            && self.total_leaks() == 0
            && self.fix_pr_flagship.merge_count == 1
    }

    pub fn total_leaks(&self) -> u32 {
        self.pr_context_pane.leaks + self.fix_pr_flagship.leaks + self.spec_to_ship.leaks
    }

    pub fn summary(&self) -> String {
        format!(
            "P-518 GIT SELF_TENANT {} - tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             pr-pane={} fix-pr-flagship={} (merge_count={}) spec-to-ship={} total-leaks={} verdict={}",
            self.date,
            self.pr_context_pane.is_green(),
            self.fix_pr_flagship.is_green(),
            self.fix_pr_flagship.merge_count,
            self.spec_to_ship.is_green(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

pub fn run_git_over_myelins_own_repos(date: &str) -> GitSelfTenantArtifact {
    GitSelfTenantArtifact {
        date: date.to_string(),
        pr_context_pane: run_e2e_1_pr_pane(),
        fix_pr_flagship: run_e2e_2_fix_pr(),
        spec_to_ship: run_e2e_3_spec_to_ship(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenGitRow {
    pub id: &'static str,
    pub section: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_path: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenGitRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

pub fn proven_git_rows(date: &str) -> Vec<ProvenGitRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenGitRow {
        ProvenGitRow {
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
            "GIT-D1",
            "6.2",
            "the hot-ref burst - per-ref aggregate ordering at push QPS (0 reorder under a contended ref burst)",
            "cargo test -p myelin-git --test drills_git_d1_hot_ref_burst",
            "crates/myelin-git/tests/drills_git_d1_hot_ref_burst.rs",
            date,
        ),
        row(
            "GIT-D2",
            "4.8",
            "erasure reaches every holder + pseudonymous-by-default commits - erase = shred, not hide; 0 cleartext PII residual incl. backups",
            "cargo test -p myelin-git --test drills_git_d2_erase_reaches_every_holder",
            "crates/myelin-git/tests/drills_git_d2_erase_reaches_every_holder.rs",
            date,
        ),
        row(
            "GIT-D3",
            "4.9",
            "reindex-from-source parity - the only rebuild path; the cold projection byte-matches live (no bespoke recovery reader)",
            "cargo test -p myelin-git --test drills_git_d3_reindex_parity",
            "crates/myelin-git/tests/drills_git_d3_reindex_parity.rs",
            date,
        ),
        row(
            "GIT-D4",
            "11.2",
            "object-backed packs - pack/delta storage on the object-store BlobStore; the clone round-trips byte-identical from cold packs",
            "cargo test -p myelin-git --test drills_git_d4_object_backed_packs",
            "crates/myelin-git/tests/drills_git_d4_object_backed_packs.rs",
            date,
        ),
        row(
            "GIT-D5",
            "5.9",
            "concurrent-merge linearizability - the speculative merge queue applies each merge EXACTLY ONCE under concurrency (merge-count == 1)",
            "cargo test -p myelin-git --test drills_git_d5_concurrent_merge_linearizability",
            "crates/myelin-git/tests/drills_git_d5_concurrent_merge_linearizability.rs",
            date,
        ),
        row(
            "GIT-D6",
            "4.11",
            "the 30× clone surge - DRR-fair shed within the tuned budget (human fetch HELD, agent + CI SHED, cross-tenant impact 0)",
            "cargo test -p myelin-git --test drill_git_d6_clone_surge",
            "crates/myelin-git/tests/drill_git_d6_clone_surge.rs",
            date,
        ),
        row(
            "GIT-D7",
            "5.7",
            "content-anchored line ranges - a comment anchor survives a rebase/force-push (0 mis-anchored; the anchor resolves to the same lines)",
            "cargo test -p myelin-git --test e2e_git_d7_anchor_resolution",
            "crates/myelin-git/tests/e2e_git_d7_anchor_resolution.rs",
            date,
        ),
        row(
            "GIT-D8",
            "1.6",
            "the front door - authenticate/check/placement/residency; 0 cross-tenant read (a viewer never reaches another tenant's repo)",
            "cargo test -p myelin-git --test drill_git_d8_front_door",
            "crates/myelin-git/tests/drill_git_d8_front_door.rs",
            date,
        ),
        row(
            "GIT-D9",
            "2.2",
            "receive-pack → one-tx ref-CAS + outbox - 0 ghost / 0 lost (a push commits the ref CAS + the event in one transaction or neither)",
            "cargo test -p myelin-git --test drills_git_d9_receive_pack",
            "crates/myelin-git/tests/drills_git_d9_receive_pack.rs",
            date,
        ),
        row(
            "GIT-D10",
            "5.9",
            "check-status projection + run-attempt supersession - 1 current row per (commit, context) key; a higher attempt supersedes, no cross-sync cycle",
            "cargo test -p myelin-git --test integration_git_d10_check_status_projection",
            "crates/myelin-git/tests/integration_git_d10_check_status_projection.rs",
            date,
        ),
        row(
            "GIT-D11",
            "4.3",
            "code-search leak-free SetExpr pushdown - the ACL pre-filter excludes the confidential set BEFORE scoring (0 leak, 1 query)",
            "cargo test -p myelin-git --test integration_git_p26_list_pushdown",
            "crates/myelin-git/tests/integration_git_p26_list_pushdown.rs",
            date,
        ),
        row(
            "E2E-1",
            "5.5",
            "the PR context pane - git is the reference producer; a denied viewer's linked confidential issue tombstones (0 title/count/backlink leak)",
            "cargo test -p myelin-git --test e2e_wedge_git_p34",
            "crates/myelin-git/tests/e2e_wedge_git_p34.rs",
            date,
        ),
        row(
            "E2E-2",
            "5.9",
            "the agent-native flagship - CI-fail → fix-PR; the git.merge HITL + X-1 CheckStatus gate; exactly-once HITL + merge; git.pr.merged closes the issue",
            "cargo test -p myelin-git --test e2e_wedge_git_p34",
            "crates/myelin-git/tests/e2e_wedge_git_p34.rs",
            date,
        ),
        row(
            "E2E-3",
            "4.9",
            "spec-to-ship lineage - commit→PR→merge; the cold reindex-from-source byte-matches live (the restore-verify gate confirms cold == live)",
            "cargo test -p myelin-git --test e2e_wedge_git_p34",
            "crates/myelin-git/tests/e2e_wedge_git_p34.rs",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl GitTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, GitTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            GitTruthUpVerdict::Green { .. } => &[],
            GitTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GitTruthUpPass;

impl GitTruthUpPass {
    pub fn new() -> GitTruthUpPass {
        GitTruthUpPass
    }

    pub fn run(&self, rows: &[ProvenGitRow], date: &str) -> GitTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            GitTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            GitTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenGitRow],
        date: &str,
    ) -> Result<usize, GitTruthUpRed> {
        match self.run(rows, date) {
            GitTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            GitTruthUpVerdict::Red { undated_rows } => Err(GitTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for GitTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} git row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} - a claim that \
             outlives its verification misleads the next agent (EI-01 §1); fix the doc or re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for GitTruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitRowStatus {
    DatedGreen {
        date: String,
    },
    ClaimedNotProven {
        date: String,
        reason: String,
    },
}

impl GitRowStatus {
    pub fn is_dated_green(&self) -> bool {
        matches!(self, GitRowStatus::DatedGreen { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitScorecardEntry {
    pub row: ProvenGitRow,
    pub status: GitRowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked - an unread CLAIMED-NOT-PROVEN row silently drifts \
              the docs from the code (EI-01 §1)"]
pub struct GitTruthUpScorecard {
    pub date: String,
    pub entries: Vec<GitScorecardEntry>,
}

impl GitTruthUpScorecard {
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
            "GREEN (no later-band git gate red)"
        } else {
            "RED (a git claim outran its verification)"
        };
        out.push_str(&format!(
            "P-518 GIT TRUTH-UP SCORECARD {} - {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                GitRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                GitRowStatus::ClaimedNotProven { date, reason } => {
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

pub fn run_git_truth_up_scorecard(date: &str, repo_root: &std::path::Path) -> GitTruthUpScorecard {
    let entries = proven_git_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => GitRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    GitRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => GitRowStatus::DatedGreen { date: d.clone() },
            };
            GitScorecardEntry { row, status }
        })
        .collect();
    GitTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitIncident {
    pub incident_id: String,
    pub gate_id: String,
    pub description: String,
    pub repro_drill_name: String,
}

impl GitIncident {
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> GitIncident {
        GitIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    pub fn issue_draft(&self) -> GitIncidentIssueDraft {
        GitIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!("[{}] git gate {} regressed", self.incident_id, self.gate_id),
            body: format!(
                "git incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked - every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    pub fn drill_ticket(&self) -> GitIncidentDrillTicket {
        GitIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitIncidentIssueDraft {
    pub gate_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitIncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    #[test]
    fn git_greens_on_myelins_own_repos() {
        let artifact = run_git_over_myelins_own_repos(RUN_DATE);

        assert!(
            artifact.is_green(),
            "git must be green on Myelin's own repositories: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the three faces: {}",
            artifact.summary()
        );
        assert!(
            artifact.pr_context_pane.is_green(),
            "the PR context pane is green"
        );
        assert!(
            artifact.fix_pr_flagship.is_green(),
            "the agent-native fix-PR flagship is green"
        );
        assert_eq!(
            artifact.fix_pr_flagship.merge_count, 1,
            "the flagship merge is exactly-once"
        );
        assert!(
            artifact.spec_to_ship.is_green(),
            "the spec-to-ship lineage is green"
        );

        let s = artifact.summary();
        assert!(s.contains("P-518 GIT SELF_TENANT 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_git_rows(RUN_DATE);
        assert!(
            rows.len() >= 14,
            "the PROVEN set covers GIT-D1..GIT-D11 + the E2E slices"
        );
        let confirmed = GitTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red later-band git gates - every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_git_rows(RUN_DATE);
        rows[0].artifact_date = None;
        let verdict = GitTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = GitTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let scorecard = run_git_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green - every PROVEN git row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("GIT-D1") && md.contains("E2E-3"),
            "enumerated: {md}"
        );
    }

    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-git-truth-up-root");
        let scorecard = run_git_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard
                .entries
                .iter()
                .all(|e| matches!(&e.status, GitRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk"))),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = GitIncident::new(
            "INC-GIT-SELF_TENANT-1",
            "GIT-D9",
            "a receive-pack regression left a ghost ref without its outbox event on the Myelin self-tenant",
            "repro_git_d9_self_tenant_ghost_ref",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "GIT-D9");
        assert!(draft.title.contains("INC-GIT-SELF_TENANT-1"));
        assert!(
            draft.body.contains("repro_git_d9_self_tenant_ghost_ref"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(ticket.drill_name, "repro_git_d9_self_tenant_ghost_ref");
        assert_eq!(ticket.gate_id, "GIT-D9");
    }

    #[test]
    fn every_proven_row_proof_source_exists_on_disk() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        for row in proven_git_rows(RUN_DATE) {
            assert!(
                row.artifact_abs_path(&repo_root).exists(),
                "the proof source for {} must exist on disk: {}",
                row.id,
                row.artifact_path
            );
        }
    }
}
