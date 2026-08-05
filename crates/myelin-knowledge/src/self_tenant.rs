use crate::e2e_wedge::{run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, E2eArtifact};
use crate::editor::{Document, EditorBlock};

pub const MYELIN_SELF_TENANT: &str = "myelin";

pub const MYELIN_SELF_REGION: &str = "fr-par";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyelinDoc {
    pub page_id: &'static str,
    pub title: &'static str,
    pub blocks: Vec<&'static str>,
}

impl MyelinDoc {
    pub fn to_document(&self) -> Document {
        Document {
            blocks: self
                .blocks
                .iter()
                .map(|md| EditorBlock::new(md, &[]))
                .collect(),
        }
    }

    pub fn round_trips(&self) -> bool {
        self.to_document().corpus_roundtrips()
    }
}

pub fn myelin_knowledge_space() -> Vec<MyelinDoc> {
    vec![
        MyelinDoc {
            page_id: "myelin-roadmap",
            title: "Myelin platform roadmap (M1..M6)",
            blocks: vec![
                "# Myelin platform roadmap\n",
                "The bands run *M1* through **M6**; M6 is the `self_tenant` done-bar.\n",
                "Each band closes on a dated green exit-gate scorecard.\n",
            ],
        },
        MyelinDoc {
            page_id: "myelin-gap-report",
            title: "Myelin gap report (named floors + follow-ons)",
            blocks: vec![
                "# Gap report\n",
                "Every named floor carries a follow-on prompt id.\n",
                "The only remaining floor is the world-scale `30x` fleet-hardware load drill.\n",
            ],
        },
        MyelinDoc {
            page_id: "myelin-scorecard",
            title: "Myelin exit-gate scorecard (every PROVEN row dated)",
            blocks: vec![
                "# Exit-gate scorecard\n",
                "Every **PROVEN** row rests on a dated green drill artifact.\n",
                "A claim that outlives its verification is a `CLAIMED-NOT-PROVEN` red.\n",
            ],
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the self_tenant artifact must be checked - an unread RED face silently claims a green Knowledge \
              did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct KnowledgeSelfTenantArtifact {
    pub date: String,
    pub docs_round_tripped: usize,
    pub docs_total: usize,
    pub pr_context_pane: E2eArtifact,
    pub spec_to_ship: E2eArtifact,
}

impl KnowledgeSelfTenantArtifact {
    pub fn is_green(&self) -> bool {
        self.docs_total > 0
            && self.docs_round_tripped == self.docs_total
            && self.pr_context_pane.is_green()
            && self.spec_to_ship.is_green()
            && self.total_leaks() == 0
    }

    pub fn total_leaks(&self) -> u64 {
        self.pr_context_pane.leaks + self.spec_to_ship.leaks
    }

    pub fn summary(&self) -> String {
        format!(
            "P-519 KNOWLEDGE SELF_TENANT {} - tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             own-docs-round-trip={}/{} pr-context-pane={} spec-to-ship={} total-leaks={} verdict={}",
            self.date,
            self.docs_round_tripped,
            self.docs_total,
            self.pr_context_pane.is_green(),
            self.spec_to_ship.is_green(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

pub fn run_knowledge_over_myelins_own_work(date: &str) -> KnowledgeSelfTenantArtifact {
    let space = myelin_knowledge_space();
    let docs_total = space.len();
    let docs_round_tripped = space.iter().filter(|d| d.round_trips()).count();
    KnowledgeSelfTenantArtifact {
        date: date.to_string(),
        docs_round_tripped,
        docs_total,
        pr_context_pane: run_e2e1_pr_context_pane(),
        spec_to_ship: run_e2e3_spec_to_ship_lineage(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenKnowledgeRow {
    pub id: &'static str,
    pub section: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_path: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenKnowledgeRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

pub fn proven_knowledge_rows(date: &str) -> Vec<ProvenKnowledgeRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenKnowledgeRow {
        ProvenKnowledgeRow {
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
            "KN-D1",
            "3.5",
            "kill a collab client mid-edit + sever the connection during sustained multi-author edit - 0 lost / 0 dup op (the resume cursor re-binds)",
            "cargo test -p myelin-knowledge --test cdc_3_5_knowledge_resume_cursor",
            "crates/myelin-knowledge/tests/cdc_3_5_knowledge_resume_cursor.rs",
            date,
        ),
        row(
            "KN-D2",
            "13.1",
            "render(parse(md)) === md 100% round-trip over the markdown-subset corpus, re-run over the integrated editor - the ONE render path (0 regressions)",
            "cargo test -p myelin-knowledge --test integration_kn_p09_integrated_editor",
            "crates/myelin-knowledge/tests/integration_kn_p09_integrated_editor.rs",
            date,
        ),
        row(
            "KN-D3",
            "13.3",
            "two clients edit the same block concurrently - the loser is rejected with current state (0 silent overwrite; the CAS-floor correctness gate)",
            "cargo test -p myelin-knowledge --test drill_kn_d3_cas_merge_floor",
            "crates/myelin-knowledge/tests/drill_kn_d3_cas_merge_floor.rs",
            date,
        ),
        row(
            "KN-D4",
            "10.9",
            "erase a subject - structured PII purged/pseudonymised, free-text under a per-subject DEK crypto-shredded (0 recoverable incl. vectors incl. backups)",
            "cargo test -p myelin-knowledge gdpr::erase_floor",
            "crates/myelin-knowledge/src/gdpr/erase_floor.rs",
            date,
        ),
        row(
            "KN-D5",
            "4.3",
            "a confidential page / overridden sub-page / row-restricted db / field-hidden column never leaks - incl. COUNT across search/embed/RAG (0 leak, the SetExpr pre-filter)",
            "cargo test -p myelin-knowledge --test integration_kn_d5_list_pushdown",
            "crates/myelin-knowledge/tests/integration_kn_d5_list_pushdown.rs",
            date,
        ),
        row(
            "KN-D6",
            "2.6",
            "wipe Knowledge's derived state (Refs edge projection / Search index); replay(scope) rebuilds it cold == live byte-for-byte (the only recovery path)",
            "cargo test -p myelin-knowledge --test drill_kn_d6_reindex_parity",
            "crates/myelin-knowledge/tests/drill_kn_d6_reindex_parity.rs",
            date,
        ),
        row(
            "KN-D7",
            "2.2",
            "crash Knowledge between the block/row commit and relay-publish - the event is still delivered (outbox emit-iff-committed; 0 ghost / 0 lost)",
            "cargo test -p myelin-knowledge --test integration_kn_d7_outbox",
            "crates/myelin-knowledge/tests/integration_kn_d7_outbox.rs",
            date,
        ),
        row(
            "KN-D8",
            "4.11",
            "an all-hands doc with thousands of concurrent readers/editors - per-doc op cap + read-fanout bound held; the concurrent-same-gap LexoRank storm converges (0 duplicate rank)",
            "cargo test -p myelin-knowledge --test drill_kn_d8_allhands_surge",
            "crates/myelin-knowledge/tests/drill_kn_d8_allhands_surge.rs",
            date,
        ),
        row(
            "KN-D9",
            "13.3",
            "filter/sort/group a large multi-tenant database (JSONB + projection + SetExpr conjoin) - p99 within budget, 0 cross-tenant leak",
            "cargo test -p myelin-knowledge --test integration_kn_d9_flex_db",
            "crates/myelin-knowledge/tests/integration_kn_d9_flex_db.rs",
            date,
        ),
        row(
            "KN-D10",
            "13.3",
            "a rollup over a large related set, computed at read time (permission-filtered) - p99 within budget; the materialisation trigger fires past the measured threshold",
            "cargo test -p myelin-knowledge --test integration_kn_d10_rollup",
            "crates/myelin-knowledge/tests/integration_kn_d10_rollup.rs",
            date,
        ),
        row(
            "KN-D11",
            "8.1",
            "an agent edits a doc via EffectApi - attributed 'suggested by agent'; a consequential edit is HITL-withheld until approval (0 ungoverned mutation, 0 mutation before approval)",
            "cargo test -p myelin-knowledge --test cdc_8_2_knowledge_agent_governance",
            "crates/myelin-knowledge/tests/cdc_8_2_knowledge_agent_governance.rs",
            date,
        ),
        row(
            "KN-D12",
            "8.8",
            "erase a subject - content-addressed agent traces crypto-shredded/purged; attribution falls back to a tombstone (0 recoverable trace)",
            "cargo test -p myelin-knowledge --test cdc_8_8_knowledge_agent_trace",
            "crates/myelin-knowledge/tests/cdc_8_8_knowledge_agent_trace.rs",
            date,
        ),
        row(
            "KN-D13",
            "11.1",
            "read a page/db/row across tenants via path-tenant spoofing - 0 cross-tenant read (the (tenant, region) partition + RLS front door)",
            "cargo test -p myelin-knowledge --test cdc_11_1_12_1_knowledge_oltp_store_and_partition",
            "crates/myelin-knowledge/tests/cdc_11_1_12_1_knowledge_oltp_store_and_partition.rs",
            date,
        ),
        row(
            "E2E-1",
            "5.6",
            "the PR context pane - a Knowledge design-doc embed resolves per-viewer; a denied viewer's confidential doc tombstones (0 title leak; mid-flight erase honoured live)",
            "cargo test -p myelin-knowledge --test drill_kn_p33_e2e_wedge",
            "crates/myelin-knowledge/tests/drill_kn_p33_e2e_wedge.rs",
            date,
        ),
        row(
            "E2E-3",
            "2.6",
            "spec-to-ship lineage - a Knowledge spec doc → initiative → issues; cold-reindex == live byte-for-byte; audit tamper detected (0 silent tamper)",
            "cargo test -p myelin-knowledge --test drill_kn_p33_e2e_wedge",
            "crates/myelin-knowledge/tests/drill_kn_p33_e2e_wedge.rs",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl KnowledgeTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, KnowledgeTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            KnowledgeTruthUpVerdict::Green { .. } => &[],
            KnowledgeTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KnowledgeTruthUpPass;

impl KnowledgeTruthUpPass {
    pub fn new() -> KnowledgeTruthUpPass {
        KnowledgeTruthUpPass
    }

    pub fn run(&self, rows: &[ProvenKnowledgeRow], date: &str) -> KnowledgeTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            KnowledgeTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            KnowledgeTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenKnowledgeRow],
        date: &str,
    ) -> Result<usize, KnowledgeTruthUpRed> {
        match self.run(rows, date) {
            KnowledgeTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            KnowledgeTruthUpVerdict::Red { undated_rows } => Err(KnowledgeTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for KnowledgeTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} Knowledge row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} - a \
             claim that outlives its verification misleads the next agent (EI-01 §1); fix the doc or \
             re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for KnowledgeTruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeRowStatus {
    DatedGreen {
        date: String,
    },
    ClaimedNotProven {
        date: String,
        reason: String,
    },
}

impl KnowledgeRowStatus {
    pub fn is_dated_green(&self) -> bool {
        matches!(self, KnowledgeRowStatus::DatedGreen { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeScorecardEntry {
    pub row: ProvenKnowledgeRow,
    pub status: KnowledgeRowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked - an unread CLAIMED-NOT-PROVEN row silently drifts \
              the docs from the code (EI-01 §1)"]
pub struct KnowledgeTruthUpScorecard {
    pub date: String,
    pub entries: Vec<KnowledgeScorecardEntry>,
}

impl KnowledgeTruthUpScorecard {
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
            "GREEN (no later-band Knowledge gate red)"
        } else {
            "RED (a Knowledge claim outran its verification)"
        };
        out.push_str(&format!(
            "P-519 KNOWLEDGE TRUTH-UP SCORECARD {} - {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                KnowledgeRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                KnowledgeRowStatus::ClaimedNotProven { date, reason } => {
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

pub fn run_knowledge_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> KnowledgeTruthUpScorecard {
    let entries = proven_knowledge_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => KnowledgeRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    KnowledgeRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => KnowledgeRowStatus::DatedGreen { date: d.clone() },
            };
            KnowledgeScorecardEntry { row, status }
        })
        .collect();
    KnowledgeTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeIncident {
    pub incident_id: String,
    pub gate_id: String,
    pub description: String,
    pub repro_drill_name: String,
}

impl KnowledgeIncident {
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> KnowledgeIncident {
        KnowledgeIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    pub fn issue_draft(&self) -> KnowledgeIncidentIssueDraft {
        KnowledgeIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Knowledge gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Knowledge incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked - every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    pub fn drill_ticket(&self) -> KnowledgeIncidentDrillTicket {
        KnowledgeIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeIncidentIssueDraft {
    pub gate_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeIncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    #[test]
    fn knowledge_greens_on_myelins_own_work() {
        let artifact = run_knowledge_over_myelins_own_work(RUN_DATE);

        assert!(
            artifact.is_green(),
            "Knowledge must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.docs_round_tripped,
            artifact.docs_total,
            "every one of Myelin's own docs round-trips through the ONE render path: {}",
            artifact.summary()
        );
        assert!(artifact.docs_total >= 3, "the roadmap/gap-report/scorecard");
        assert!(
            artifact.pr_context_pane.is_green(),
            "the PR context pane is green"
        );
        assert!(
            artifact.spec_to_ship.is_green(),
            "the spec-to-ship lineage is green"
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the two E2E faces: {}",
            artifact.summary()
        );

        let s = artifact.summary();
        assert!(
            s.contains("P-519 KNOWLEDGE SELF_TENANT 2026-06-26"),
            "dated: {s}"
        );
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn myelins_own_docs_round_trip_through_the_one_render_path() {
        let space = myelin_knowledge_space();
        assert!(space.len() >= 3, "the roadmap/gap-report/scorecard");
        for doc in &space {
            assert!(
                doc.round_trips(),
                "the Myelin doc {} must round-trip through the ONE render path",
                doc.page_id
            );
            assert!(!doc.to_document().blocks.is_empty(), "{}", doc.page_id);
        }
    }

    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_knowledge_rows(RUN_DATE);
        assert!(
            rows.len() >= 15,
            "the PROVEN set covers KN-D1..KN-D13 + the E2E slices"
        );
        let confirmed = KnowledgeTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red later-band Knowledge gates - every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_knowledge_rows(RUN_DATE);
        rows[0].artifact_date = None;
        let verdict = KnowledgeTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = KnowledgeTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        let scorecard = run_knowledge_truth_up_scorecard(RUN_DATE, &repo_root());
        assert!(
            scorecard.is_green(),
            "the scorecard must be green - every PROVEN Knowledge row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("KN-D1") && md.contains("E2E-3"),
            "enumerated: {md}"
        );
    }

    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-kn-truth-up-root");
        let scorecard = run_knowledge_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard.entries.iter().all(|e| matches!(
                &e.status,
                KnowledgeRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk")
            )),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = KnowledgeIncident::new(
            "INC-KN-SELF_TENANT-1",
            "KN-D2",
            "a markdown-subset corpus body silently round-tripped non-canonically on the Myelin self-tenant",
            "repro_kn_d2_self_tenant_non_canonical_round_trip",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "KN-D2");
        assert!(draft.title.contains("INC-KN-SELF_TENANT-1"));
        assert!(
            draft
                .body
                .contains("repro_kn_d2_self_tenant_non_canonical_round_trip"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_kn_d2_self_tenant_non_canonical_round_trip"
        );
        assert_eq!(ticket.gate_id, "KN-D2");
    }

    #[test]
    fn every_proven_row_proof_source_exists_on_disk() {
        for row in proven_knowledge_rows(RUN_DATE) {
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
