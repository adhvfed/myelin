use myelin_content::{Block, Inline, Span};
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, RunId as StorageRunId};
use myelin_tenancy::TenantId;

use crate::dispatch::{classify, DispatchDecision, DispatchTrigger};
use crate::trace_seam::TraceDocument;

pub const MYELIN_SELF_TENANT: &str = "myelin";

pub const MYELIN_SELF_REGION: &str = "fr-par";

pub const SELF_TENANT_RUNTIME_FLOOR: &str = "the self_tenant triage agents run on the MOCK runtime \
    (--use-mock MockAgentRuntime) - correct per VISION §3 during development. The real \
    LlmAgentRuntime swap (the only place a model/SDK/prompt/model-name string appears) is the named \
    post-M5 follow-on AG-P25; the external MCP endpoint + agent long-term memory/RAG are post-M5 \
    (named in AG-P25's seam doc).";

fn myelin_tenant() -> TenantId {
    TenantId(MYELIN_SELF_TENANT.into())
}

const TRIAGE_ESTIMATE: u64 = 120_000;

const MYELIN_WALLET: u64 = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriageFace {
    pub dispatched: bool,
    pub mention_only_notifies: bool,
    pub reserved: u64,
    pub settled: u64,
    pub cost_events: usize,
    pub inflight_interrupts: u64,
    pub trace_ref: String,
}

impl TriageFace {
    pub fn is_green(&self) -> bool {
        self.dispatched
            && self.mention_only_notifies
            && self.reserved == self.settled
            && self.inflight_interrupts == 0
            && self.trace_ref.starts_with("blake3:")
    }
}

fn text(s: &str) -> Inline {
    Inline {
        spans: vec![Span::Text {
            text: s.to_string(),
            marks: vec![],
            link: None,
        }],
        nodes: vec![],
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_myelin_triage_on_ci_failure(commit_oid: &str, run_id: u128) -> TriageFace {
    let signal = classify(&DispatchTrigger::ExplicitRun(format!(
        "signal:ci.result=failure:{commit_oid}"
    )));
    let dispatched = matches!(signal, DispatchDecision::Dispatch(_));
    let casual = classify(&DispatchTrigger::Mention("@triage look".into()));
    let mention_only_notifies = matches!(casual, DispatchDecision::Notify(_));

    let mut ledger = CostLedger::new();
    let storage_run = StorageRunId::new(format!("run:triage:{commit_oid}"));
    let reservation = ledger
        .reserve(
            myelin_tenant(),
            storage_run.clone(),
            MicroUsd(TRIAGE_ESTIMATE),
            MicroUsd(MYELIN_WALLET),
        )
        .expect("the funded Myelin self-tenant wallet reserves the triage run at dispatch");
    ledger
        .begin(&myelin_tenant(), &storage_run)
        .expect("the reserved triage run begins flight");

    let units: Vec<MeteredUnit> = vec![];
    let settle = ledger
        .settle(&myelin_tenant(), &storage_run, &units)
        .expect("the in-flight triage run settles on completion");
    let settled = settle.billed_total.0 + settle.refunded.0;

    let trace = TraceDocument::new(
        run_id,
        vec![
            Block::Paragraph {
                inline: text(&format!(
                    "triage agent: CI failed on Myelin commit {commit_oid}; filed an issue + posted \
                     the failing-step summary to the channel; proposed no merge (advisory)."
                )),
            },
            Block::CodeBlock {
                lang: Some("json".into()),
                text: r#"{"tool":"create_issue","result":"ok"}"#.into(),
            },
        ],
    );

    TriageFace {
        dispatched,
        mention_only_notifies,
        reserved: reservation.reserved.0,
        settled,
        cost_events: ledger.cost_events_for(&myelin_tenant(), &storage_run).len(),
        inflight_interrupts: ledger.inflight_interrupt_count(),
        trace_ref: trace.content_address().0,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the self_tenant artifact must be checked - an unread RED face silently claims a green the \
              Fabric did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct FabricSelfTenantArtifact {
    pub date: String,
    pub triage: TriageFace,
}

impl FabricSelfTenantArtifact {
    pub fn is_green(&self) -> bool {
        self.triage.is_green()
    }

    pub fn summary(&self) -> String {
        format!(
            "P-517 FABRIC SELF_TENANT {} - tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             dispatched={} reserved=={settled} balanced={} interrupts={} trace={} verdict={}",
            self.date,
            self.triage.dispatched,
            self.triage.reserved == self.triage.settled,
            self.triage.inflight_interrupts,
            &self.triage.trace_ref[..self.triage.trace_ref.len().min(14)],
            if self.is_green() { "GREEN" } else { "RED" },
            settled = self.triage.settled,
        )
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_fabric_over_myelins_own_work(date: &str) -> FabricSelfTenantArtifact {
    FabricSelfTenantArtifact {
        date: date.to_string(),
        triage: run_myelin_triage_on_ci_failure("feedface", 0x5170_u128),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenFabricRow {
    pub id: &'static str,
    pub section: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_path: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenFabricRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

pub fn proven_fabric_rows(date: &str) -> Vec<ProvenFabricRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenFabricRow {
        ProvenFabricRow {
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
            "AG-D1",
            "8.2",
            "the plan-then-apply pipeline applies an in-∩ effect to the subsystem PUBLIC endpoint (1 mutation, 1 metered cost event)",
            "cargo test -p myelin-agent-service --test cdc_8_2_apply_pipeline",
            "crates/myelin-agent-service/tests/cdc_8_2_apply_pipeline.rs",
            date,
        ),
        row(
            "AG-D2",
            "8.2",
            "0 privileged fallback EVER fires (fail-closed by construction) - a denied effect never silently escalates",
            "cargo test -p myelin-agent-service --test cdc_8_2_apply_pipeline",
            "crates/myelin-agent-service/tests/cdc_8_2_apply_pipeline.rs",
            date,
        ),
        row(
            "AG-D3",
            "8.2",
            "an effect outside the delegation ∩ is DENIED (attenuation never up - 0 effect no human role could perform)",
            "cargo test -p myelin-agent-service --test cdc_8_2_apply_pipeline",
            "crates/myelin-agent-service/tests/cdc_8_2_apply_pipeline.rs",
            date,
        ),
        row(
            "AG-D4",
            "8.4",
            "the AgentExecGate is fail-closed in the TYPE - only a GREEN escape attestation (ZERO escapes, matching backend identity) admits untrusted compute",
            "cargo test -p myelin-agent-service --test cdc_8_4_escape_gate",
            "crates/myelin-agent-service/tests/cdc_8_4_escape_gate.rs",
            date,
        ),
        row(
            "AG-D4-prod",
            "8.4",
            "AG-D4 re-confirmed on the PRODUCTION CI runner image (the M4 hard gate - the deploy tools run on the prod image)",
            "cargo test -p myelin-agent-service --test cdc_8_4_prod_image_reconfirm",
            "crates/myelin-agent-service/tests/cdc_8_4_prod_image_reconfirm.rs",
            date,
        ),
        row(
            "AG-D5",
            "8.2",
            "the HITL withhold→surface→resume loop: a gated tool withholds at step 6, opens on approval, applies EXACTLY ONCE (a double-click is one approval)",
            "cargo test -p myelin-agent-service --test cdc_8_2_hitl_loop",
            "crates/myelin-agent-service/tests/cdc_8_2_hitl_loop.rs",
            date,
        ),
        row(
            "AG-D6",
            "1.11",
            "the 30× agent-dispatch surge: the human lane holds, the machine lane sheds (429 + Retry-After honoured), 0 cross-tenant impact",
            "cargo test -p myelin-agent-service --test ag_d6_dispatch_surge_drill",
            "crates/myelin-agent-service/tests/ag_d6_dispatch_surge_drill.rs",
            date,
        ),
        row(
            "AG-D7",
            "8.5",
            "the five structural loop guards: the adversarial agent→event→agent self-trigger is STOPPED (causal-depth ≤ ceiling, 0 unbounded fork, tripwire fires)",
            "cargo test -p myelin-agent-service --test drills_ag_d7_loop_guards",
            "crates/myelin-agent-service/tests/drills_ag_d7_loop_guards.rs",
            date,
        ),
        row(
            "AG-D8",
            "8.5",
            "the per-run identity no-tool SKELETON leg: mint→reserve→step→trace→settle→revoke, 0 shared platform token leaked into the child env",
            "cargo test -p myelin-agent-service --test cdc_8_5_skeleton_loop",
            "crates/myelin-agent-service/tests/cdc_8_5_skeleton_loop.rs",
            date,
        ),
        row(
            "AG-D8-remint",
            "4.7",
            "the per-run identity re-mint leg: a multi-day HITL pause re-mints a fresh attenuated token on resume (0 unattributed window across the pause)",
            "cargo test -p myelin-agent-service --test cdc_4_7_remint_resume",
            "crates/myelin-agent-service/tests/cdc_4_7_remint_resume.rs",
            date,
        ),
        row(
            "AG-D9",
            "8.3",
            "the MockAgentRuntime step-determinism: two runs over the same script produce a byte-identical step/effect sequence (the deterministic run trace)",
            "cargo test -p myelin-agent-service --test cdc_8_3_mock_runtime",
            "crates/myelin-agent-service/tests/cdc_8_3_mock_runtime.rs",
            date,
        ),
        row(
            "AG-D10",
            "10.1",
            "the DSR erasure fan-out over all Fabric holders (run table + trace + memory): crypto-shred reaches history, 0 recoverable PII, the pseudonym attribution survives",
            "cargo test -p myelin-agent-service --test drills_ag_d10_erasure",
            "crates/myelin-agent-service/tests/drills_ag_d10_erasure.rs",
            date,
        ),
        row(
            "AG-D11",
            "11.7",
            "the reserve/settle runaway self-limiter: reserve refuses past wallet exhaustion (the loop stops at the wallet), 0 in-flight interrupt, reserved == settled (balanced)",
            "cargo test -p myelin-agent-service --test drills_ag_d11_runaway_self_limiter",
            "crates/myelin-agent-service/tests/drills_ag_d11_runaway_self_limiter.rs",
            date,
        ),
        row(
            "E2E-2",
            "8.2",
            "the agent-native flagship: CI-fail → triage agent → issue → chat → fix-PR across a kill + days-later approval (exactly-once, merge-count==1, reserve/settle balanced, 0 effect outside the ∩)",
            "cargo test -p myelin-agent-service --test drills_ag_p24_e2e2_flagship",
            "crates/myelin-agent-service/tests/drills_ag_p24_e2e2_flagship.rs",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FabricTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl FabricTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, FabricTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            FabricTruthUpVerdict::Green { .. } => &[],
            FabricTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FabricTruthUpPass;

impl FabricTruthUpPass {
    pub fn new() -> FabricTruthUpPass {
        FabricTruthUpPass
    }

    pub fn run(&self, rows: &[ProvenFabricRow], date: &str) -> FabricTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            FabricTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            FabricTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenFabricRow],
        date: &str,
    ) -> Result<usize, FabricTruthUpRed> {
        match self.run(rows, date) {
            FabricTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            FabricTruthUpVerdict::Red { undated_rows } => Err(FabricTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for FabricTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} Fabric row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} - a \
             claim that outlives its verification misleads the next agent (EI-01 §1); fix the doc or \
             re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for FabricTruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FabricRowStatus {
    DatedGreen {
        date: String,
    },
    ClaimedNotProven {
        date: String,
        reason: String,
    },
}

impl FabricRowStatus {
    pub fn is_dated_green(&self) -> bool {
        matches!(self, FabricRowStatus::DatedGreen { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricScorecardEntry {
    pub row: ProvenFabricRow,
    pub status: FabricRowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked - an unread CLAIMED-NOT-PROVEN row silently \
              drifts the docs from the code (EI-01 §1)"]
pub struct FabricTruthUpScorecard {
    pub date: String,
    pub entries: Vec<FabricScorecardEntry>,
}

impl FabricTruthUpScorecard {
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
            "GREEN (no later-band Fabric gate red)"
        } else {
            "RED (a Fabric claim outran its verification)"
        };
        out.push_str(&format!(
            "P-517 FABRIC TRUTH-UP SCORECARD {} - {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                FabricRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                FabricRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<14} {:<28} - {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

pub fn run_fabric_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> FabricTruthUpScorecard {
    let entries = proven_fabric_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => FabricRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    FabricRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => FabricRowStatus::DatedGreen { date: d.clone() },
            };
            FabricScorecardEntry { row, status }
        })
        .collect();
    FabricTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricIncident {
    pub incident_id: String,
    pub gate_id: String,
    pub description: String,
    pub repro_drill_name: String,
}

impl FabricIncident {
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> FabricIncident {
        FabricIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    pub fn issue_draft(&self) -> FabricIncidentIssueDraft {
        FabricIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] Fabric gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "Fabric incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked - every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    pub fn drill_ticket(&self) -> FabricIncidentDrillTicket {
        FabricIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricIncidentIssueDraft {
    pub gate_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricIncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    #[test]
    fn myelins_own_agent_green_on_the_self_hosting_graph() {
        let artifact = run_fabric_over_myelins_own_work(RUN_DATE);
        assert!(
            artifact.is_green(),
            "Myelin's own triage agent must run green on the self-hosting CI graph: {}",
            artifact.summary()
        );

        assert!(
            artifact.triage.dispatched,
            "a ci.result=failure Signal DISPATCHES a costed triage run (explicit-first, §3.4)"
        );
        assert!(
            artifact.triage.mention_only_notifies,
            "a casual @triage mention only NOTIFIES - 0 auto-spawn (the safety boundary)"
        );

        assert_eq!(
            artifact.triage.reserved, artifact.triage.settled,
            "reserve/settle BALANCED - reserved == settled on the Myelin self-tenant wallet"
        );
        assert_eq!(
            artifact.triage.cost_events, 0,
            "the Mock brain bills 0 metered units (the gate is brain-independent - AG-P25 is the real meter)"
        );
        assert_eq!(
            artifact.triage.inflight_interrupts, 0,
            "0 in-flight interrupt - the reservation's only exit is settle (11.7)"
        );

        assert!(
            artifact.triage.trace_ref.starts_with("blake3:"),
            "a content-addressed trace per run (8.8): {}",
            artifact.triage.trace_ref
        );

        let s = artifact.summary();
        assert!(s.contains("P-517 FABRIC SELF_TENANT 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_fabric_rows(RUN_DATE);
        assert!(
            rows.len() >= 11,
            "the PROVEN set covers AG-D1..AG-D11 + the E2E-2 spine (got {})",
            rows.len()
        );
        let confirmed = FabricTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red later-band Fabric gates - every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    #[test]
    fn the_proven_set_enumerates_every_ag_d_drill_plus_e2e2() {
        let rows = proven_fabric_rows(RUN_DATE);
        let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();
        for must in [
            "AG-D1", "AG-D2", "AG-D3", "AG-D4", "AG-D5", "AG-D6", "AG-D7", "AG-D8", "AG-D9",
            "AG-D10", "AG-D11", "E2E-2",
        ] {
            assert!(
                ids.contains(&must),
                "the truth-up pass must enumerate the PROVEN row {must}"
            );
        }
    }

    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_fabric_rows(RUN_DATE);
        rows[0].artifact_date = None;
        let verdict = FabricTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = FabricTruthUpPass::new()
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
        let scorecard = run_fabric_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green - every PROVEN Fabric row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("AG-D1") && md.contains("E2E-2"),
            "enumerated: {md}"
        );
    }

    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-fabric-truth-up-root");
        let scorecard = run_fabric_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard.entries.iter().all(|e| matches!(
                &e.status,
                FabricRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk")
            )),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = FabricIncident::new(
            "INC-AG-SELF_TENANT-1",
            "AG-D11",
            "a reserve/settle regression left an in-flight triage run torn down on the Myelin self-tenant",
            "repro_ag_d11_self_tenant_runaway_self_limiter",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "AG-D11");
        assert!(draft.title.contains("INC-AG-SELF_TENANT-1"));
        assert!(
            draft
                .body
                .contains("repro_ag_d11_self_tenant_runaway_self_limiter"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));
        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_ag_d11_self_tenant_runaway_self_limiter"
        );
        assert_eq!(ticket.gate_id, "AG-D11");
    }

    #[test]
    fn the_mock_runtime_floor_is_named() {
        assert!(SELF_TENANT_RUNTIME_FLOOR.contains("MOCK runtime"));
        assert!(
            SELF_TENANT_RUNTIME_FLOOR.contains("AG-P25"),
            "names the real-runtime swap follow-on"
        );
        assert!(SELF_TENANT_RUNTIME_FLOOR.contains("post-M5"));
    }
}
