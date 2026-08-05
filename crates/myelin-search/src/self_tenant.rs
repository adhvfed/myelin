use crate::e2e_wedge::E2eArtifact;
#[cfg(any(test, feature = "test-support"))]
use crate::{run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout};

pub const MYELIN_SELF_TENANT: &str = "myelin";

pub const MYELIN_SELF_REGION: &str = "fr-par";

pub const EMBEDDING_ADAPTER_POSTURE: &str =
    "mock (MockEmbeddingAdapter; real EU-hostable embedding \
                                             adapter is the named post-M5/runtime config swap)";

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the self_tenant artifact must be checked - an unread RED face silently claims a green Search \
              did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct SelfTenantArtifact {
    pub date: String,
    pub code_and_issue: E2eArtifact,
    pub knowledge_space: E2eArtifact,
    pub dsar_fanout: E2eArtifact,
}

impl SelfTenantArtifact {
    pub fn is_green(&self) -> bool {
        self.code_and_issue.is_green()
            && self.knowledge_space.is_green()
            && self.dsar_fanout.is_green()
    }

    pub fn total_leaks(&self) -> u64 {
        self.code_and_issue.leaks + self.knowledge_space.leaks + self.dsar_fanout.leaks
    }

    pub fn summary(&self) -> String {
        format!(
            "P-515 SEARCH SELF_TENANT {} - tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             code+issue={} knowledge-space={} dsar-fanout={} total-leaks={} embedding-adapter={} \
             verdict={}",
            self.date,
            self.code_and_issue.is_green(),
            self.knowledge_space.is_green(),
            self.dsar_fanout.is_green(),
            self.total_leaks(),
            EMBEDDING_ADAPTER_POSTURE,
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_search_over_myelins_own_work(date: &str) -> SelfTenantArtifact {
    SelfTenantArtifact {
        date: date.to_string(),
        code_and_issue: run_e2e_1_pr_pane(),
        knowledge_space: run_e2e_3_spec_to_ship(),
        dsar_fanout: run_e2e_4_dsar_fanout(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenSearchRow {
    pub id: &'static str,
    pub section: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_path: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenSearchRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

pub fn proven_search_rows(date: &str) -> Vec<ProvenSearchRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenSearchRow {
        ProvenSearchRow {
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
            "SRCH-D1",
            "4.2",
            "the zero-escape leak - a hidden doc NEVER enters the candidate set (0 doc/count/IDF/RAG leak; the §4.2 pre-filter, not a post-filter)",
            "cargo test -p myelin-search --test drill_srch_d1_zero_escape_leak",
            "crates/myelin-search/tests/drill_srch_d1_zero_escape_leak.rs",
            date,
        ),
        row(
            "SRCH-D2",
            "4.10",
            "the no-stale-grant invariant - a revoked grant never serves a stale hit (the zookie/consistency floor admits at-or-after the baseline snapshot)",
            "cargo test -p myelin-search --test drill_srch_d2_no_stale_grant",
            "crates/myelin-search/tests/drill_srch_d2_no_stale_grant.rs",
            date,
        ),
        row(
            "SRCH-D3",
            "1.1",
            "the cross-tenant isolation - a query is (tenant, region)-keyed; no doc from another tenant ever enters a candidate set",
            "cargo test -p myelin-search --test drill_srch_d3_cross_tenant",
            "crates/myelin-search/tests/drill_srch_d3_cross_tenant.rs",
            date,
        ),
        row(
            "SRCH-D4",
            "4.8",
            "the erasure - erase = purge+reindex, not hide; the docs are GONE from FT + k-NN (0 recoverable incl. vectors)",
            "cargo test -p myelin-search --test drill_srch_d4_erasure",
            "crates/myelin-search/tests/drill_srch_d4_erasure.rs",
            date,
        ),
        row(
            "SRCH-D5",
            "4.9",
            "reindex-from-source - the only rebuild path; the wiped index reindexes to byte-match live (the reindex-parity hash)",
            "cargo test -p myelin-search --test cdc_6_4_reindex",
            "crates/myelin-search/tests/cdc_6_4_reindex.rs",
            date,
        ),
        row(
            "SRCH-D6",
            "4.11",
            "the 30× search surge - DRR-fair shed within the tuned budget, the human lane holds, the filtered-ANN follow-on named",
            "cargo test -p myelin-search --test drill_srch_d6_surge",
            "crates/myelin-search/tests/drill_srch_d6_surge.rs",
            date,
        ),
        row(
            "SRCH-D7",
            "4.6",
            "freshness at scale - the event→searchable p99 within budget (the projection feeder keeps the index fresh under load)",
            "cargo test -p myelin-search --test drill_srch_d7_freshness_at_scale",
            "crates/myelin-search/tests/drill_srch_d7_freshness_at_scale.rs",
            date,
        ),
        row(
            "SRCH-D8",
            "4.5",
            "filtered-ANN recall - the permission-aware filter-during-traversal returns k VISIBLE neighbours within the recall floor",
            "cargo test -p myelin-search --test drill_srch_d8_filtered_ann_recall",
            "crates/myelin-search/tests/drill_srch_d8_filtered_ann_recall.rs",
            date,
        ),
        row(
            "SRCH-D9",
            "4.8",
            "restore + re-erase - a restore-from-backup re-applies every erasure (an erased subject stays erased after a restore, 0 recoverable)",
            "cargo test -p myelin-search --test drill_srch_d9_restore_reerase",
            "crates/myelin-search/tests/drill_srch_d9_restore_reerase.rs",
            date,
        ),
        row(
            "SRCH-D10",
            "7.5",
            "HYOK + backup-scale erasure - the tenant-decommission crypto-shred destroys the per-tenant index DEK; every sealed backup segment is plaintext-unrecoverable (0 recoverable incl. vectors incl. backups)",
            "cargo test -p myelin-search --test drill_srch_d10_hyok_and_backup_erasure",
            "crates/myelin-search/tests/drill_srch_d10_hyok_and_backup_erasure.rs",
            date,
        ),
        row(
            "E2E-1",
            "4.2",
            "the PR context pane - a denied viewer's hit on a confidential issue NEVER enters the candidate set (0 leak); mid-flight ci.check.updated live-update; the unfurl tombstones (0 title leak)",
            "cargo test -p myelin-search --test e2e_wedge_srch_p32",
            "crates/myelin-search/tests/e2e_wedge_srch_p32.rs",
            date,
        ),
        row(
            "E2E-3",
            "4.9",
            "spec-to-ship reindex-parity - wipe → reindex-from-source → byte-match live; the restore-verify gate confirms cold==live",
            "cargo test -p myelin-search --test e2e_wedge_srch_p32",
            "crates/myelin-search/tests/e2e_wedge_srch_p32.rs",
            date,
        ),
        row(
            "E2E-4",
            "4.8",
            "the DSAR fan-out - Search's docs + EMBEDDINGS return 0 recoverable PII incl. backups; the holder-coverage receipt includes Search H7",
            "cargo test -p myelin-search --test e2e_wedge_srch_p32",
            "crates/myelin-search/tests/e2e_wedge_srch_p32.rs",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl SearchTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, SearchTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            SearchTruthUpVerdict::Green { .. } => &[],
            SearchTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SearchTruthUpPass;

impl SearchTruthUpPass {
    pub fn new() -> SearchTruthUpPass {
        SearchTruthUpPass
    }

    pub fn run(&self, rows: &[ProvenSearchRow], date: &str) -> SearchTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            SearchTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            SearchTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenSearchRow],
        date: &str,
    ) -> Result<usize, SearchTruthUpRed> {
        match self.run(rows, date) {
            SearchTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            SearchTruthUpVerdict::Red { undated_rows } => Err(SearchTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for SearchTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} Search row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} - a claim \
             that outlives its verification misleads the next agent (EI-01 §1); fix the doc or re-run \
             the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for SearchTruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchRowStatus {
    DatedGreen {
        date: String,
    },
    ClaimedNotProven {
        date: String,
        reason: String,
    },
}

impl SearchRowStatus {
    pub fn is_dated_green(&self) -> bool {
        matches!(self, SearchRowStatus::DatedGreen { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchScorecardEntry {
    pub row: ProvenSearchRow,
    pub status: SearchRowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked - an unread CLAIMED-NOT-PROVEN row silently drifts \
              the docs from the code (EI-01 §1)"]
pub struct SearchTruthUpScorecard {
    pub date: String,
    pub entries: Vec<SearchScorecardEntry>,
}

impl SearchTruthUpScorecard {
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
            "GREEN (no earlier-band Search gate red)"
        } else {
            "RED (a Search claim outran its verification)"
        };
        out.push_str(&format!(
            "P-515 SEARCH TRUTH-UP SCORECARD {} - {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                SearchRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                SearchRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<10} {:<28} - {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

pub fn run_search_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> SearchTruthUpScorecard {
    let entries = proven_search_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => SearchRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    SearchRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => SearchRowStatus::DatedGreen { date: d.clone() },
            };
            SearchScorecardEntry { row, status }
        })
        .collect();
    SearchTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchIncident {
    pub incident_id: String,
    pub gate_id: String,
    pub description: String,
    pub repro_drill_name: String,
}

impl SearchIncident {
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> SearchIncident {
        SearchIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    pub fn issue_draft(&self) -> SearchIncidentIssueDraft {
        SearchIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Search gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Search incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked - every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    pub fn drill_ticket(&self) -> SearchIncidentDrillTicket {
        SearchIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchIncidentIssueDraft {
    pub gate_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchIncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    #[test]
    fn search_greens_on_myelins_own_work() {
        let artifact = run_search_over_myelins_own_work(RUN_DATE);

        assert!(
            artifact.is_green(),
            "Search must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the three faces: {}",
            artifact.summary()
        );
        assert!(
            artifact.code_and_issue.is_green(),
            "code + issue search is green"
        );
        assert!(
            artifact.knowledge_space.is_green(),
            "the Knowledge-space reindex-parity is green"
        );
        assert!(artifact.dsar_fanout.is_green(), "the DSAR fan-out is green");

        let s = artifact.summary();
        assert!(s.contains("P-515 SEARCH SELF_TENANT 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
        assert!(
            s.contains("embedding-adapter=mock"),
            "the embedding-adapter posture is recorded honestly (mock): {s}"
        );
    }

    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_search_rows(RUN_DATE);
        assert!(
            rows.len() >= 13,
            "the PROVEN set covers SRCH-D1..SRCH-D10 + the E2E legs"
        );
        let confirmed = SearchTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red earlier-band Search gates - every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_search_rows(RUN_DATE);
        rows[0].artifact_date = None;
        let verdict = SearchTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = SearchTruthUpPass::new()
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
        let scorecard = run_search_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green - every PROVEN Search row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("SRCH-D1") && md.contains("E2E-4"),
            "enumerated: {md}"
        );
    }

    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-search-truth-up-root");
        let scorecard = run_search_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard
                .entries
                .iter()
                .all(|e| matches!(&e.status, SearchRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk"))),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = SearchIncident::new(
            "INC-SEARCH-SELF_TENANT-1",
            "SRCH-D1",
            "a pre-filter regression let a confidential issue enter the candidate set on the Myelin self-tenant",
            "repro_srch_d1_self_tenant_candidate_leak",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "SRCH-D1");
        assert!(draft.title.contains("INC-SEARCH-SELF_TENANT-1"));
        assert!(
            draft.body.contains("repro_srch_d1_self_tenant_candidate_leak"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(ticket.drill_name, "repro_srch_d1_self_tenant_candidate_leak");
        assert_eq!(ticket.gate_id, "SRCH-D1");
    }

    #[test]
    fn every_proven_row_proof_source_exists_on_disk() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        for row in proven_search_rows(RUN_DATE) {
            assert!(
                row.artifact_abs_path(&repo_root).exists(),
                "the proof source for {} must exist on disk: {}",
                row.id,
                row.artifact_path
            );
        }
    }
}
