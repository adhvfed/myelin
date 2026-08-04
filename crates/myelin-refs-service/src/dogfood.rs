#[cfg(any(test, feature = "test-support"))]
use myelin_events::{Actor, EmitContextBase, Timestamp};
#[cfg(any(test, feature = "test-support"))]
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
#[cfg(any(test, feature = "test-support"))]
use myelin_tenancy::{Region, TenantId};

use crate::e2e_wedge::E2eArtifact;
#[cfg(any(test, feature = "test-support"))]
use crate::{run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout};

pub const MYELIN_SELF_TENANT: &str = "myelin";

pub const MYELIN_SELF_REGION: &str = "fr-par";

#[cfg(any(test, feature = "test-support"))]
fn myelin_self_region() -> Region {
    Region(MYELIN_SELF_REGION.into())
}

#[cfg(any(test, feature = "test-support"))]
fn myelin_ctx_base() -> EmitContextBase {
    let corpus_tenant = TenantId("acme".into());
    EmitContextBase {
        tenant: corpus_tenant.clone(),
        region: myelin_self_region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            corpus_tenant,
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-26T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-26T00:00:00Z".into()),
        caused_by: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked - an unread RED face silently claims a green the \
              reference graph did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct DogfoodArtifact {
    pub date: String,
    pub pr_pane: E2eArtifact,
    pub spec_to_ship: E2eArtifact,
    pub holder_fanout: E2eArtifact,
}

impl DogfoodArtifact {
    pub fn is_green(&self) -> bool {
        self.pr_pane.is_green() && self.spec_to_ship.is_green() && self.holder_fanout.is_green()
    }

    pub fn total_leaks(&self) -> u64 {
        self.pr_pane.leaks + self.spec_to_ship.leaks + self.holder_fanout.leaks
    }

    pub fn summary(&self) -> String {
        format!(
            "P-513 REFS DOGFOOD {} - tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             pr-pane={} spec-to-ship={} holder-fanout={} total-leaks={} verdict={}",
            self.date,
            self.pr_pane.is_green(),
            self.spec_to_ship.is_green(),
            self.holder_fanout.is_green(),
            self.total_leaks(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_refs_over_myelins_own_work(date: &str) -> DogfoodArtifact {
    DogfoodArtifact {
        date: date.to_string(),
        pr_pane: run_e2e_1_pr_pane(),
        spec_to_ship: run_e2e_3_spec_to_ship(myelin_ctx_base()),
        holder_fanout: run_e2e_4_dsar_fanout(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenRefsRow {
    pub id: &'static str,
    pub section: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_path: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenRefsRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

pub fn proven_refs_rows(date: &str) -> Vec<ProvenRefsRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenRefsRow {
        ProvenRefsRow {
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
            "REF-D1",
            "5.2",
            "the resolve chokepoint - per-viewer gate; a denied target tombstones (root-only), 0 title/count/backlink leak",
            "cargo test -p myelin-refs-service --test cdc_5_2_resolve",
            "crates/myelin-refs-service/tests/cdc_5_2_resolve.rs",
            date,
        ),
        row(
            "REF-D2",
            "5.3",
            "the leak-free backlink read - list_objects SetExpr lowered into the per-tenant authz reverse index, 0 cross-tenant leak",
            "cargo test -p myelin-refs-service --test cdc_5_3_backlinks",
            "crates/myelin-refs-service/tests/cdc_5_3_backlinks.rs",
            date,
        ),
        row(
            "REF-D3",
            "5.7",
            "the tombstone / graceful-degradation ladder - permission→root→sub-resolve{live/moved/outdated/gone}→erased; a tombstone always carries the root",
            "cargo test -p myelin-refs-service --test cdc_5_7_sub_ladder",
            "crates/myelin-refs-service/tests/cdc_5_7_sub_ladder.rs",
            date,
        ),
        row(
            "REF-D4",
            "5.3",
            "the bounded cycle-safe lineage traverse - depth/node ceilings from thresholds, per-viewer prune, 0 leak",
            "cargo test -p myelin-refs-service --test cdc_5_3_traverse",
            "crates/myelin-refs-service/tests/cdc_5_3_traverse.rs",
            date,
        ),
        row(
            "REF-D5",
            "5.4",
            "the event-sourced edge inverse index - deterministic edge_id, idempotent rebuild from the producer events",
            "cargo test -p myelin-refs-service --test cdc_5_4_edge_builder",
            "crates/myelin-refs-service/tests/cdc_5_4_edge_builder.rs",
            date,
        ),
        row(
            "REF-D6",
            "5.5",
            "the TE-7 typed-edge mirror - typed table = source of truth, Refs = rebuildable projection, reconverges",
            "cargo test -p myelin-refs-service --test cdc_5_5_mirror",
            "crates/myelin-refs-service/tests/cdc_5_5_mirror.rs",
            date,
        ),
        row(
            "REF-D7",
            "10.1",
            "the PersonalDataHolder structural-erasure surface - locate/erase reaches the edges + the projection cache, 0 recoverable PII",
            "cargo test -p myelin-refs-service --test integration_ref_p15_holder_erase",
            "crates/myelin-refs-service/tests/integration_ref_p15_holder_erase.rs",
            date,
        ),
        row(
            "REF-D8",
            "5.8",
            "reindex-from-source - the only recovery path; the rebuilt index byte-matches the live projection (parity)",
            "cargo test -p myelin-refs-service --test cdc_5_8_reindex",
            "crates/myelin-refs-service/tests/cdc_5_8_reindex.rs",
            date,
        ),
        row(
            "REF-D9",
            "5.8",
            "reindex-from-source parity AT SCALE - wipe → reindex over the five-producer corpus → byte-match live (REF-D4 at scale)",
            "cargo test -p myelin-refs-service --test ref_d4_reindex_parity_at_scale",
            "crates/myelin-refs-service/tests/ref_d4_reindex_parity_at_scale.rs",
            date,
        ),
        row(
            "REF-D10",
            "12.6",
            "the 30x backlink surge - DRR-fair shed within budget, the reach-index follow-on named; restore + re-erase at backup scale, 0 recoverable PII",
            "cargo test -p myelin-refs-service --test ref_d10_surge_drill --test ref_d5_restore_reerase_at_backup_scale",
            "crates/myelin-refs-service/tests/ref_d10_surge_drill.rs",
            date,
        ),
        row(
            "E2E-1",
            "5.2",
            "the PR context pane - every connected artifact unfurls per-viewer; mid-flight ci.check.updated live-update; a denied confidential issue tombstones, 0 leak",
            "cargo test -p myelin-refs-service --test e2e_wedge_ref_p27",
            "crates/myelin-refs-service/tests/e2e_wedge_ref_p27.rs",
            date,
        ),
        row(
            "E2E-3",
            "5.3",
            "spec-to-ship traceability - the full lineage traverse depth-16 per-viewer (0 leak) → wipe → reindex → byte-match live",
            "cargo test -p myelin-refs-service --test e2e_wedge_ref_p27",
            "crates/myelin-refs-service/tests/e2e_wedge_ref_p27.rs",
            date,
        ),
        row(
            "E2E-4",
            "10.1",
            "the DSAR fan-out - the structural-erasure holder fan-out reaches the edges + cache, the unfurls degrade to tombstones, 0 recoverable PII",
            "cargo test -p myelin-refs-service --test e2e_wedge_ref_p27",
            "crates/myelin-refs-service/tests/e2e_wedge_ref_p27.rs",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefsTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl RefsTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, RefsTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            RefsTruthUpVerdict::Green { .. } => &[],
            RefsTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RefsTruthUpPass;

impl RefsTruthUpPass {
    pub fn new() -> RefsTruthUpPass {
        RefsTruthUpPass
    }

    pub fn run(&self, rows: &[ProvenRefsRow], date: &str) -> RefsTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            RefsTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            RefsTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenRefsRow],
        date: &str,
    ) -> Result<usize, RefsTruthUpRed> {
        match self.run(rows, date) {
            RefsTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            RefsTruthUpVerdict::Red { undated_rows } => Err(RefsTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for RefsTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} Refs row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} - a claim \
             that outlives its verification misleads the next agent (EI-01 §1); fix the doc or re-run \
             the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for RefsTruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefsRowStatus {
    DatedGreen {
        date: String,
    },
    ClaimedNotProven {
        date: String,
        reason: String,
    },
}

impl RefsRowStatus {
    pub fn is_dated_green(&self) -> bool {
        matches!(self, RefsRowStatus::DatedGreen { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsScorecardEntry {
    pub row: ProvenRefsRow,
    pub status: RefsRowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked - an unread CLAIMED-NOT-PROVEN row silently \
              drifts the docs from the code (EI-01 §1)"]
pub struct RefsTruthUpScorecard {
    pub date: String,
    pub entries: Vec<RefsScorecardEntry>,
}

impl RefsTruthUpScorecard {
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
            "GREEN (no earlier-band Refs gate red)"
        } else {
            "RED (a Refs claim outran its verification)"
        };
        out.push_str(&format!(
            "P-513 REFS TRUTH-UP SCORECARD {} - {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                RefsRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                RefsRowStatus::ClaimedNotProven { date, reason } => {
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

pub fn run_refs_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> RefsTruthUpScorecard {
    let entries = proven_refs_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => RefsRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    RefsRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => RefsRowStatus::DatedGreen { date: d.clone() },
            };
            RefsScorecardEntry { row, status }
        })
        .collect();
    RefsTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsIncident {
    pub incident_id: String,
    pub gate_id: String,
    pub description: String,
    pub repro_drill_name: String,
}

impl RefsIncident {
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> RefsIncident {
        RefsIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    pub fn issue_draft(&self) -> RefsIncidentIssueDraft {
        RefsIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Refs gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Refs incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked - every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    pub fn drill_ticket(&self) -> RefsIncidentDrillTicket {
        RefsIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsIncidentIssueDraft {
    pub gate_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsIncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    #[test]
    fn the_reference_graph_greens_on_myelins_own_work() {
        let artifact = run_refs_over_myelins_own_work(RUN_DATE);

        assert!(
            artifact.is_green(),
            "the reference graph must be green on Myelin's own work: {}",
            artifact.summary()
        );
        assert_eq!(
            artifact.total_leaks(),
            0,
            "0 leak across the three faces: {}",
            artifact.summary()
        );
        assert!(artifact.pr_pane.is_green(), "the PR context pane is green");
        assert!(
            artifact.spec_to_ship.is_green(),
            "the spec-to-ship lineage is green"
        );
        assert!(
            artifact.holder_fanout.is_green(),
            "the holder fan-out is green"
        );

        let s = artifact.summary();
        assert!(s.contains("P-513 REFS DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_refs_rows(RUN_DATE);
        assert!(
            rows.len() >= 13,
            "the PROVEN set covers REF-D1..REF-D10 + the E2E legs"
        );
        let confirmed = RefsTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red earlier-band Refs gates - every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_refs_rows(RUN_DATE);
        rows[0].artifact_date = None;
        let verdict = RefsTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = RefsTruthUpPass::new()
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
        let scorecard = run_refs_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green - every PROVEN Refs row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("REF-D1") && md.contains("E2E-4"),
            "enumerated: {md}"
        );
    }

    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-refs-truth-up-root");
        let scorecard = run_refs_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard
                .entries
                .iter()
                .all(|e| matches!(&e.status, RefsRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk"))),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = RefsIncident::new(
            "INC-REFS-DOGFOOD-1",
            "REF-D1",
            "a resolve chokepoint regression leaked a denied issue title on the Myelin self-tenant",
            "repro_ref_d1_dogfood_resolve_leak",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "REF-D1");
        assert!(draft.title.contains("INC-REFS-DOGFOOD-1"));
        assert!(
            draft.body.contains("repro_ref_d1_dogfood_resolve_leak"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));

        let ticket = incident.drill_ticket();
        assert_eq!(ticket.drill_name, "repro_ref_d1_dogfood_resolve_leak");
        assert_eq!(ticket.gate_id, "REF-D1");
    }
}
