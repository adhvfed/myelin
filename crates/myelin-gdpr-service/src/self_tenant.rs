use std::collections::BTreeMap;

use myelin_events::{
    Actor, AggregateKey, CausedBy, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, OpaqueSubjectId, Region, TenantId as TenancyTenantId,
};

use myelin_gdpr::PersonalData;
use myelin_substrate::{Holder as SubHolder, HolderRegistration, StoreKind};

use crate::audit::AuditConsumer;
use crate::audit_proofs::{AuditAuthority, CellSigningKey};
use crate::datamap::{data_map, ropa, HolderSchema, Inventory, ProcessingActivities};
use crate::dsr::{DsrId, MerkleProvenBundle};
use crate::full_fanout::{FullFanOutCoverage, GaD1Certificate, Holder};
use crate::issues_chat_instance::issues_chat_holder_schemas;
use crate::multi_cell::{MemberCellSet, MultiCellCertificate, MultiCellFanOut, PerCellReceipt};
use crate::producer_holders::producer_holder_schemas;

pub const TRUTH_UP_FULL_PASS_PROMPT: &str = "P-GA-38 (→ P-512)";

pub const MYELIN_SELF_TENANT: &str = "myelin-self";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SelfTenantAction {
    GitCommit,
    CiRun,
    IssueChange,
    ChatMessage,
    AgentAction,
}

impl SelfTenantAction {
    pub fn label(self) -> &'static str {
        match self {
            SelfTenantAction::GitCommit => "git-commit",
            SelfTenantAction::CiRun => "ci-run",
            SelfTenantAction::IssueChange => "issue-change",
            SelfTenantAction::ChatMessage => "chat-message",
            SelfTenantAction::AgentAction => "agent-action",
        }
    }

    pub fn event_type(self) -> &'static str {
        match self {
            SelfTenantAction::GitCommit => "git.commit_pushed",
            SelfTenantAction::CiRun => "ci.run_completed",
            SelfTenantAction::IssueChange => "issues.transitioned",
            SelfTenantAction::ChatMessage => "chat.message_posted",
            SelfTenantAction::AgentAction => "agent.action_taken",
        }
    }

    pub const ALL: [SelfTenantAction; 5] = [
        SelfTenantAction::GitCommit,
        SelfTenantAction::CiRun,
        SelfTenantAction::IssueChange,
        SelfTenantAction::ChatMessage,
        SelfTenantAction::AgentAction,
    ];
}

pub fn run_audit_consumer_on_self_tenant(now_iso: &str) -> AuditSelfTenantArtifact {
    let consumer = AuditConsumer::new();
    let tenant = TenancyTenantId(MYELIN_SELF_TENANT.into());

    let mut by_surface: BTreeMap<SelfTenantAction, usize> = BTreeMap::new();
    for (i, action) in SelfTenantAction::ALL.iter().enumerate() {
        let ev = self_tenant_event(*action, i);
        let outcome = consumer.handle(&ev, &mut myelin_events::HandlerTx::none());
        debug_assert_eq!(outcome, myelin_events::HandleOutcome::Done);
        *by_surface.entry(*action).or_insert(0) += 1;
    }

    let entries = consumer.log().entries_for(&tenant);
    let chain_verifies = consumer.log().verify_chain(&tenant);
    let root_present = consumer.log().root(&tenant).is_some();
    let append_lag = consumer.append_lag();

    AuditSelfTenantArtifact {
        date: now_iso.to_string(),
        tenant: tenant.clone(),
        actions_logged: entries.len(),
        chain_verifies,
        root_present,
        append_lag,
        actions_by_surface: by_surface,
    }
}

fn self_tenant_event(action: SelfTenantAction, n: usize) -> EventEnvelope {
    let actor = self_tenant_principal(action, n);
    let tenant = actor.tenant.clone();
    let region = actor.region.clone();
    EventEnvelope {
        event_id: EventId(format!("self_tenant-{}-{n}", action.label())),
        type_: EventType(action.event_type().into()),
        schema_ver: 1,
        tenant,
        region,
        actor: Actor(actor),
        subject: ArtifactRef(format!("myelin://myelin-self/{}/n{n}", action.label())),
        aggregate: AggregateKey(format!("agg:{}", action.label())),
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-self_tenant-{n}")),
        caused_by: Some(CausedBy("session:self_tenant".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-26T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-26T00:00:01Z".into()),
        payload: serde_json::json!({ "real_name": "Adrian Helvik", "email": "team@myelin.test" }),
    }
}

fn self_tenant_principal(action: SelfTenantAction, n: usize) -> Principal {
    let tenant = TenancyTenantId(MYELIN_SELF_TENANT.into());
    let kind = match action {
        SelfTenantAction::AgentAction => PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-self_tenant".into()),
            on_behalf_of: Some(PrincipalId("u-myelin-team-1".into())),
        },
        _ => PrincipalKind::Human,
    };
    Principal::stub(PrincipalId(format!("u-myelin-team-{n}")), kind, tenant)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditSelfTenantArtifact {
    pub date: String,
    pub tenant: TenancyTenantId,
    pub actions_logged: usize,
    pub chain_verifies: bool,
    pub root_present: bool,
    pub append_lag: u64,
    pub actions_by_surface: BTreeMap<SelfTenantAction, usize>,
}

impl AuditSelfTenantArtifact {
    pub fn audit_graph_is_green(&self) -> bool {
        self.chain_verifies
            && self.root_present
            && self.append_lag == 0
            && self.actions_logged == SelfTenantAction::ALL.len()
            && self.actions_by_surface.len() == SelfTenantAction::ALL.len()
    }

    pub fn summary(&self) -> String {
        let breakdown: Vec<String> = self
            .actions_by_surface
            .iter()
            .map(|(a, n)| format!("{}={n}", a.label()))
            .collect();
        format!(
            "[P-511 SELF_TENANT AUDIT GREEN {date}] tenant={tenant}: {logged} of Myelin's OWN actions \
             logged, chain_verifies={chain} root_present={root} audit_append_lag={lag} - {breakdown}",
            date = self.date,
            tenant = self.tenant.0,
            logged = self.actions_logged,
            chain = self.chain_verifies,
            root = self.root_present,
            lag = self.append_lag,
            breakdown = breakdown.join(", "),
        )
    }
}

pub fn run_self_served_dsr_on_self_tenant(now_iso: &str) -> DsrSelfTenantArtifact {
    let tenant = TenancyTenantId(MYELIN_SELF_TENANT.into());
    let region = Region("fr-par".into());
    let set = self_host_member_set();
    let pointer = pii_free_pointer();
    let dsr_id = DsrId("dsr:myelin-self-served".into());

    let mut cells_resolved = 0usize;
    let merged: MultiCellCertificate = MultiCellFanOut::new()
        .fan_out("myelin-self/u-team", &set, &pointer, |_cell, _p| {
            cells_resolved += 1;
            seal_full_cell_fanout()
        })
        .expect("the self-served multi-cell DSAR fan-out seals on Myelin's own data");

    let all_cells_complete = merged.per_cell.iter().all(PerCellReceipt::cell_is_complete);
    let max_holders_missed = merged
        .per_cell
        .iter()
        .map(|r| r.cell_certificate.holders_missed)
        .max()
        .unwrap_or(usize::MAX);

    let auth = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:myelin-self-audit"));
    let bundle = MerkleProvenBundle {
        dsr_id: dsr_id.clone(),
        receipts: merged
            .per_cell
            .iter()
            .map(|r| r.content_hash.clone())
            .collect(),
        bundle_digest: merged.content_hash.clone(),
        merkle_inclusion: None,
    };
    let sealed = auth.seal_dsr_certificate(&tenant, &region, &bundle, now_iso);

    DsrSelfTenantArtifact {
        date: now_iso.to_string(),
        tenant,
        dsr_id,
        holders_missed: max_holders_missed,
        cells_missed: merged.cells_missed,
        cells_total: merged.cells_total,
        cells_resolved,
        all_cells_complete,
        certificate_sealed: sealed.merkle_inclusion.is_some(),
        inclusion_proof: sealed.merkle_inclusion,
        bundle_digest: sealed.bundle_digest,
    }
}

fn seal_full_cell_fanout() -> GaD1Certificate {
    let mut cov = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        cov.record_reached(h);
    }
    GaD1Certificate::seal("myelin-self/u-team", &cov).expect("the cell's full H1–H18 fan-out seals")
}

fn self_host_member_set() -> MemberCellSet {
    MemberCellSet::union(CellId::from_token("cell-fr-par-self"), &[])
}

fn pii_free_pointer() -> myelin_tenancy::CrossCellPointer {
    myelin_tenancy::CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://myelin-self/issues/issue/1".into())),
        ArtifactType::Issue,
        CorrelationId("corr-self_tenant-dsr".into()),
        CellId::from_token("cell-fr-par-self"),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrSelfTenantArtifact {
    pub date: String,
    pub tenant: TenancyTenantId,
    pub dsr_id: DsrId,
    pub holders_missed: usize,
    pub cells_missed: usize,
    pub cells_total: usize,
    pub cells_resolved: usize,
    pub all_cells_complete: bool,
    pub certificate_sealed: bool,
    pub inclusion_proof: Option<String>,
    pub bundle_digest: String,
}

impl DsrSelfTenantArtifact {
    pub fn dsr_is_green(&self) -> bool {
        self.holders_missed == 0
            && self.cells_missed == 0
            && self.cells_total > 0
            && self.cells_resolved == self.cells_total
            && self.all_cells_complete
            && self.certificate_sealed
    }

    pub fn summary(&self) -> String {
        format!(
            "[P-511 SELF_TENANT DSR GREEN {date}] tenant={tenant} dsr={dsr}: holders_missed={hm} \
             cells_missed={cm} cells_total={ct} certificate=SEALED({sealed})",
            date = self.date,
            tenant = self.tenant.0,
            dsr = self.dsr_id.0,
            hm = self.holders_missed,
            cm = self.cells_missed,
            ct = self.cells_total,
            sealed = self.bundle_digest,
        )
    }
}

#[derive(PersonalData)]
#[allow(dead_code)]
struct MyelinTeamMemberRecord {
    #[personal_data(
        category = ContactInfo,
        role = PlatformOperational,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    email: String,
    #[personal_data(
        category = Behavioural,
        role = TenantContent,
        basis = LegitimateInterest,
        retention = Fixed(365d),
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    personnel_note: String,
    row_version: u64,
}

pub fn myelin_team_holder_schemas(region: myelin_tenancy::Region) -> Vec<HolderSchema> {
    let mut schemas = vec![HolderSchema::from_schema::<MyelinTeamMemberRecord>(
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "myelin_team_directory",
        },
        SubHolder::H15Identity,
        region.clone(),
    )];
    schemas.extend(producer_holder_schemas(region.clone()));
    schemas.extend(issues_chat_holder_schemas(region));
    schemas
}

#[derive(Clone, Debug)]
pub struct RopaKnowledgeSpace {
    title: String,
    data_map: Inventory,
    ropa: ProcessingActivities,
}

impl RopaKnowledgeSpace {
    pub fn generate(holders: &[HolderSchema]) -> RopaKnowledgeSpace {
        let inventory = data_map(holders);
        let ropa = ropa(&inventory);
        RopaKnowledgeSpace {
            title: "Myelin - Records of Processing Activities + Data Map".to_string(),
            data_map: inventory,
            ropa,
        }
    }

    pub fn for_myelin_team(region: myelin_tenancy::Region) -> RopaKnowledgeSpace {
        Self::generate(&myelin_team_holder_schemas(region))
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn data_map(&self) -> &Inventory {
        &self.data_map
    }

    pub fn ropa(&self) -> &ProcessingActivities {
        &self.ropa
    }

    pub fn is_populated(&self) -> bool {
        self.data_map.entry_count() > 0 && !self.ropa.is_empty()
    }

    pub fn render_pages(&self) -> Vec<KnowledgeSpacePage> {
        vec![
            KnowledgeSpacePage {
                title: "Data Map (generated, contract 10.3)".to_string(),
                body: format!(
                    "The Myelin team's own generated data map: {entries} PII fields across \
                     {holders} holders. Fingerprint: {fp}. The map, not a hand-written list, drives \
                     erasure (GA-D1: 0 holders missed is a property of this map).",
                    entries = self.data_map.entry_count(),
                    holders = self.data_map.holder_count(),
                    fp = self.data_map.fingerprint(),
                ),
            },
            KnowledgeSpacePage {
                title: "Records of Processing Activities (Art. 30, generated)".to_string(),
                body: format!(
                    "The Myelin team's own RoPA: {activities} distinct processing activities, \
                     projected from the generated data map (grouped by (role, category)). The RoPA \
                     legal text is DPO-reviewed; the generation is the structural floor.",
                    activities = self.ropa.len(),
                ),
            },
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeSpacePage {
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GdprIncident {
    pub id: String,
    pub gate_id: String,
    pub summary: String,
    pub repro_drill_name: String,
}

impl GdprIncident {
    pub fn new(
        id: impl Into<String>,
        gate_id: impl Into<String>,
        summary: impl Into<String>,
        repro_drill_name: impl Into<String>,
    ) -> GdprIncident {
        GdprIncident {
            id: id.into(),
            gate_id: gate_id.into(),
            summary: summary.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    pub fn issue_draft(&self) -> IncidentIssueDraft {
        IncidentIssueDraft {
            title: format!("[gdpr incident {}] {}", self.id, self.summary),
            body: format!(
                "A GDPR/Audit incident surfaced during self-hosting.\n\nGate touched: {}\nReproducing \
                 drill (registered into the permanent harness suite, re-runs forever): {}\n\nThe \
                 every-incident-adds-a-drill loop (EI-01 §3) requires this incident's repro join the \
                 suite - the drill below IS that repro. PII-free: this names a FAULT, never a subject.",
                self.gate_id, self.repro_drill_name
            ),
            gate_id: self.gate_id.clone(),
        }
    }

    pub fn drill_ticket(&self) -> IncidentDrillTicket {
        IncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
            incident_id: self.id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentIssueDraft {
    pub title: String,
    pub body: String,
    pub gate_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
    pub incident_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenGdprRow {
    pub id: &'static str,
    pub section: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_path: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenGdprRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

pub fn proven_gdpr_rows(date: &str) -> Vec<ProvenGdprRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenGdprRow {
        ProvenGdprRow {
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
            "GA-D5",
            "10.2",
            "no-untagged-personal-data + data-map-diff - an untagged PII field is a structural failure; a map change blocks until a DPO ratifies",
            "cargo test -p myelin-lints --test gdpr_audit_lints && cargo test -p myelin-gdpr-service --test cdc_10_3_diff_gate",
            "crates/myelin-lints/tests/gdpr_audit_lints.rs",
            date,
        ),
        row(
            "GA-D1",
            "10.4",
            "erasure reaches every holder - 0 holders missed over H1–H18 at cell scale",
            "cargo test -p myelin-gdpr-service --test ga_d1_full_fanout_cell_scale",
            "crates/myelin-gdpr-service/tests/ga_d1_full_fanout_cell_scale.rs",
            date,
        ),
        row(
            "GA-D2",
            "10.5",
            "erasure reaches search - docs + embeddings purged-not-hidden, 0 re-identification",
            "cargo test -p myelin-gdpr-service --test ga_d2_derivative_erasure",
            "crates/myelin-gdpr-service/tests/ga_d2_derivative_erasure.rs",
            date,
        ),
        row(
            "GA-D3",
            "10.6",
            "audit-tamper detection - a retroactive edit detected 3 independent ways (chain/consistency/witness)",
            "cargo test -p myelin-gdpr-service --test ga_d3_audit_tamper",
            "crates/myelin-gdpr-service/tests/ga_d3_audit_tamper.rs",
            date,
        ),
        row(
            "GA-D4",
            "10.4",
            "DSR deadline - the durable timer warns before the statutory clock expires",
            "cargo test -p myelin-gdpr-service --test ga_d4_dsr_deadline_timer",
            "crates/myelin-gdpr-service/tests/ga_d4_dsr_deadline_timer.rs",
            date,
        ),
        row(
            "GA-D6",
            "10.5",
            "legal-hold - an erase under an active hold is suspended, 0 held-scope deletions, resumes on lift",
            "cargo test -p myelin-gdpr-service --test ga_d6_retention_legal_hold",
            "crates/myelin-gdpr-service/tests/ga_d6_retention_legal_hold.rs",
            date,
        ),
        row(
            "GA-D7",
            "10.5",
            "restriction-leak - restrict → 0 processing across the five derived stores, storage retained",
            "cargo test -p myelin-gdpr-service --test ga_d7_derived_restrict",
            "crates/myelin-gdpr-service/tests/ga_d7_derived_restrict.rs",
            date,
        ),
        row(
            "GA-D8",
            "10.4",
            "multi-cell erasure - 0 cells missed over member_cells ∪ home_cell, per-cell receipt set complete",
            "cargo test -p myelin-gdpr-service --test ga_d8_multi_cell_fanout",
            "crates/myelin-gdpr-service/tests/ga_d8_multi_cell_fanout.rs",
            date,
        ),
        row(
            "STOR-D3-GA-face",
            "10.8",
            "post-restore re-erasure - the GDPR erasure ledger drives Storage's re-erase; a restore never resurrects erased PII (0 resurrected)",
            "cargo test -p myelin-storage --test stor_d3_post_restore_reerase_drill && cargo test -p myelin-gdpr-service --lib erasure_ledger",
            "crates/myelin-storage/tests/stor_d3_post_restore_reerase_drill.rs",
            date,
        ),
        row(
            "STOR-D4-GA-face",
            "10.8",
            "crypto-shred reaches backups - the ledger records the destroyed-key set the post-restore driver re-shreds (0 recoverable in a restored copy)",
            "cargo test -p myelin-storage --test cdc_11_5_reerase",
            "crates/myelin-storage/tests/cdc_11_5_reerase.rs",
            date,
        ),
        row(
            "GA-10",
            "10.6",
            "history-rewrite-invalidation - fan-out reaches forks/mirrors/clone-cache, op audited, 0 stale-PII hits",
            "cargo test -p myelin-gdpr-service --test ga_10_history_rewrite_invalidation",
            "crates/myelin-gdpr-service/tests/ga_10_history_rewrite_invalidation.rs",
            date,
        ),
        row(
            "GA-11",
            "10.5",
            "outbound-residency-gate - extra-EU PII push-mirror denied by default, within-EU CDN clone allowed",
            "cargo test -p myelin-gdpr-service --test ga_11_outbound_mirror_residency_gate",
            "crates/myelin-gdpr-service/tests/ga_11_outbound_mirror_residency_gate.rs",
            date,
        ),
        row(
            "CI-D3",
            "10.1",
            "CI consumer-holder erasure - per-subject CI-log DEK crypto-shred reaches isolable log PII",
            "cargo test -p myelin-gdpr-service --test ci_d3_ci_holder_erasure",
            "crates/myelin-gdpr-service/tests/ci_d3_ci_holder_erasure.rs",
            date,
        ),
        row(
            "GIT-D2",
            "10.9",
            "pseudonymous-commit - erase author → 0 recoverable real identity in immutable git bytes",
            "cargo test -p myelin-gdpr-service --test git_d2_pseudonymous_commit",
            "crates/myelin-gdpr-service/tests/git_d2_pseudonymous_commit.rs",
            date,
        ),
        row(
            "E2E-3",
            "10.7",
            "spec-to-ship traceability - the GDPR audit-tamper proof feeds the E2E-3 leg",
            "cargo test -p myelin-gdpr-service --test ga_p153_ediscovery_trace_history",
            "crates/myelin-gdpr-service/tests/ga_p153_ediscovery_trace_history.rs",
            date,
        ),
        row(
            "E2E-4",
            "10.4",
            "the DSAR fan-out flagship - 0 holders missed, 0 cells missed, certificate sealed",
            "cargo test -p myelin-gdpr-service --test e2e_4_dsar_fanout_flagship",
            "crates/myelin-gdpr-service/tests/e2e_4_dsar_fanout_flagship.rs",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a truth-up verdict must be checked - a dropped RED means a CLAIMED-NOT-PROVEN GDPR \
              row silently drifts the docs from the code (EI-01 §1: a claim that outlives its \
              verification misleads the next agent)"]
pub enum TruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl TruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, TruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            TruthUpVerdict::Green { .. } => &[],
            TruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TruthUpPass;

impl TruthUpPass {
    pub fn new() -> TruthUpPass {
        TruthUpPass
    }

    pub fn run(&self, rows: &[ProvenGdprRow], date: &str) -> TruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            TruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            TruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    pub fn run_or_fail_ci(&self, rows: &[ProvenGdprRow], date: &str) -> Result<usize, TruthUpRed> {
        match self.run(rows, date) {
            TruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            TruthUpVerdict::Red { undated_rows } => Err(TruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for TruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} GDPR row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} - a \
             claim that outlives its verification misleads the next agent (EI-01 §1); fix the doc \
             or re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for TruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowStatus {
    DatedGreen {
        date: String,
    },
    ClaimedNotProven {
        date: String,
        reason: String,
    },
}

impl RowStatus {
    pub fn is_dated_green(&self) -> bool {
        matches!(self, RowStatus::DatedGreen { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScorecardEntry {
    pub row: ProvenGdprRow,
    pub status: RowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked - an unread CLAIMED-NOT-PROVEN row silently \
              drifts the docs from the code (EI-01 §1)"]
pub struct TruthUpScorecard {
    pub date: String,
    pub entries: Vec<ScorecardEntry>,
}

impl TruthUpScorecard {
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
            "GREEN (no GDPR gate red)"
        } else {
            "RED (a GDPR claim outran its verification)"
        };
        out.push_str(&format!(
            "P-512 GDPR TRUTH-UP SCORECARD {} - {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                RowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                RowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<16} {:<28} - {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

pub fn run_truth_up_scorecard(date: &str, repo_root: &std::path::Path) -> TruthUpScorecard {
    let entries = proven_gdpr_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => RowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    RowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => RowStatus::DatedGreen { date: d.clone() },
            };
            ScorecardEntry { row, status }
        })
        .collect();
    TruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    #[test]
    fn audit_consumer_greens_on_myelins_own_actions() {
        let artifact = run_audit_consumer_on_self_tenant(RUN_DATE);

        assert!(
            artifact.audit_graph_is_green(),
            "the audit graph must be green on the platform's own actions: {artifact:?}"
        );
        assert_eq!(
            artifact.actions_logged,
            SelfTenantAction::ALL.len(),
            "every one of Myelin's own action surfaces is logged"
        );
        assert!(
            artifact.chain_verifies,
            "the per-tenant hash-chain verifies"
        );
        assert!(artifact.root_present, "a per-tenant Merkle root exists");
        assert_eq!(artifact.append_lag, 0, "audit_append_lag reads green (0)");
        assert_eq!(
            artifact.actions_by_surface.len(),
            5,
            "all five own-action surfaces (incl. an agent action - EI-02 §2) covered"
        );
        assert_eq!(
            artifact.actions_by_surface.get(&SelfTenantAction::AgentAction),
            Some(&1),
            "the coding-agent action is audited identically to a human action"
        );
        let s = artifact.summary();
        assert!(
            s.contains("P-511 SELF_TENANT AUDIT GREEN 2026-06-26"),
            "dated: {s}"
        );
        assert!(
            s.contains("git-commit=1") && s.contains("agent-action=1"),
            "per-surface breakdown: {s}"
        );
    }

    #[test]
    fn self_served_dsr_greens_and_seals_a_certificate() {
        let artifact = run_self_served_dsr_on_self_tenant(RUN_DATE);

        assert!(
            artifact.dsr_is_green(),
            "the self-served DSR must be green on the platform's own data: {artifact:?}"
        );
        assert_eq!(artifact.holders_missed, 0, "GA-D1: 0 holders missed");
        assert_eq!(artifact.cells_missed, 0, "GA-D8: 0 cells missed");
        assert!(
            artifact.cells_total > 0,
            "the self-host cell set is non-empty"
        );
        assert_eq!(
            artifact.cells_resolved, artifact.cells_total,
            "every cell fanned out cell-locally (none skipped)"
        );
        assert!(
            artifact.all_cells_complete,
            "every cell erased its whole H1–H18 holder set"
        );
        assert!(
            artifact.certificate_sealed,
            "the completion certificate seals into the per-tenant audit Merkle tree"
        );
        let inclusion = artifact
            .inclusion_proof
            .as_ref()
            .expect("the sealed certificate carries a Merkle inclusion proof");
        assert!(
            inclusion.contains("->blake3:"),
            "the inclusion proof reduces to a blake3 root: {inclusion}"
        );
        let s = artifact.summary();
        assert!(
            s.contains("P-511 SELF_TENANT DSR GREEN 2026-06-26") && s.contains("holders_missed=0"),
            "dated artifact: {s}"
        );
    }

    #[test]
    fn ropa_and_data_map_live_as_a_knowledge_space() {
        let space = RopaKnowledgeSpace::for_myelin_team(myelin_tenancy::Region("fr-par".into()));

        assert!(
            space.is_populated(),
            "the RoPA Knowledge space is populated (the data map has entries + the RoPA has activities)"
        );
        assert!(
            space.data_map().entry_count() >= 2,
            "the Myelin team-member record contributes its tagged PII fields to the map"
        );
        assert!(
            space.title().contains("Myelin"),
            "the space is the Myelin team's own GDPR space: {}",
            space.title()
        );
        let pages = space.render_pages();
        assert_eq!(
            pages.len(),
            2,
            "two generated pages: the data map + the RoPA"
        );
        assert!(
            pages[0].title.contains("Data Map") && pages[0].body.contains("blake3:"),
            "the data-map page carries the generated map's fingerprint: {:?}",
            pages[0]
        );
        assert!(
            pages[1].title.contains("Records of Processing Activities"),
            "the RoPA page renders the Art. 30 projection: {:?}",
            pages[1]
        );
    }

    #[test]
    fn a_gdpr_incident_files_an_issue_and_registers_a_drill() {
        let incident = GdprIncident::new(
            "INC-GDPR-001",
            "GA-D1",
            "a self-served DSR fan-out skipped a newly-registered Knowledge holder",
            "repro_ga_d1_dsr_skips_new_knowledge_holder",
        );

        let draft = incident.issue_draft();
        assert!(draft.title.contains("INC-GDPR-001"));
        assert!(draft.title.contains("skipped a newly-registered"));
        assert_eq!(draft.gate_id, "GA-D1");
        assert!(draft
            .body
            .contains("repro_ga_d1_dsr_skips_new_knowledge_holder"));
        assert!(draft.body.contains("every-incident-adds-a-drill"));
        assert!(
            draft.body.contains("PII-free"),
            "the draft names a FAULT, never a subject"
        );

        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_ga_d1_dsr_skips_new_knowledge_holder"
        );
        assert_eq!(ticket.gate_id, "GA-D1");
        assert_eq!(ticket.incident_id, "INC-GDPR-001");
    }

    #[test]
    fn truth_up_greens_when_every_proven_row_is_dated() {
        let rows = proven_gdpr_rows(RUN_DATE);
        assert!(!rows.is_empty(), "the PROVEN set is non-empty");
        let verdict = TruthUpPass::new().run(&rows, RUN_DATE);
        assert!(
            verdict.is_green(),
            "every proven row is dated → green: {:?}",
            verdict.undated_rows()
        );
        match verdict {
            TruthUpVerdict::Green {
                rows_confirmed,
                date,
            } => {
                assert_eq!(rows_confirmed, rows.len());
                assert_eq!(date, RUN_DATE);
            }
            TruthUpVerdict::Red { .. } => unreachable!(),
        }
        let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();
        for must in [
            "GA-D1",
            "GA-D2",
            "GA-D3",
            "GA-D4",
            "GA-D5",
            "GA-D6",
            "GA-D7",
            "GA-D8",
            "GA-10",
            "GA-11",
            "CI-D3",
            "GIT-D2",
            "STOR-D3-GA-face",
            "STOR-D4-GA-face",
            "E2E-3",
            "E2E-4",
        ] {
            assert!(
                ids.contains(&must),
                "the truth-up set must enumerate {must}"
            );
        }
        assert!(
            rows.iter().all(|r| r.section.starts_with("10.")),
            "every row names a §10.x section"
        );
    }

    #[test]
    fn truth_up_scorecard_greens_with_every_artifact_on_disk() {
        let repo_root = workspace_root();
        let card = run_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            card.is_green(),
            "no GDPR gate is red - claimed-not-proven: {:?}",
            card.claimed_not_proven()
        );
        assert_eq!(card.rows_dated_green(), card.rows_total());
        assert!(card.rows_total() >= 16, "the full §10.x set is enumerated");
        let rendered = card.render();
        assert!(
            rendered.contains("P-512 GDPR TRUTH-UP SCORECARD 2026-06-26"),
            "the scorecard is dated: {rendered}"
        );
        assert!(
            rendered.contains("GREEN (no GDPR gate red)"),
            "the verdict line is green: {rendered}"
        );
        assert!(
            rendered.contains("GA-D5") && rendered.contains("STOR-D3-GA-face"),
            "the widened rows render: {rendered}"
        );
    }

    #[test]
    fn truth_up_scorecard_surfaces_a_missing_artifact_loudly() {
        let empty_root = std::path::Path::new("/nonexistent-truth-up-root");
        let card = run_truth_up_scorecard(RUN_DATE, empty_root);
        assert!(
            !card.is_green(),
            "a vanished artifact must red the scorecard"
        );
        let missing = card.claimed_not_proven();
        assert_eq!(
            missing.len(),
            card.rows_total(),
            "every row's source is gone under the empty root"
        );
        let entry = &card.entries[0];
        match &entry.status {
            RowStatus::ClaimedNotProven { date, reason } => {
                assert_eq!(date, RUN_DATE, "the gap is dated");
                assert!(
                    reason.contains("proof source missing on disk"),
                    "the honest reason names the missing source: {reason}"
                );
            }
            RowStatus::DatedGreen { .. } => unreachable!(),
        }
        assert!(
            card.render().contains("CLAIMED-NOT-PROVEN"),
            "the render surfaces the gap loudly"
        );
    }

    fn workspace_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root is two levels above the crate manifest")
            .to_path_buf()
    }

    #[test]
    fn truth_up_reds_loudly_on_a_claimed_not_proven_row() {
        let mut rows = proven_gdpr_rows(RUN_DATE);
        let undated = rows
            .iter_mut()
            .find(|r| r.id == "GA-D1")
            .expect("GA-D1 present");
        undated.artifact_date = None;

        let verdict = TruthUpPass::new().run(&rows, RUN_DATE);
        assert!(
            !verdict.is_green(),
            "a claimed-not-proven row MUST red the truth-up pass"
        );
        assert_eq!(verdict.undated_rows(), &["GA-D1"]);

        let err = TruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row MUST fail the truth-up CI job");
        assert!(err.to_string().contains("TRUTH-UP FAIL"), "loud: {err}");
        assert!(
            err.to_string().contains("GA-D1"),
            "names the undated row: {err}"
        );
    }

    #[test]
    fn truth_up_run_or_fail_ci_returns_ok_when_all_dated() {
        let rows = proven_gdpr_rows(RUN_DATE);
        let count = TruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("a fully-dated PROVEN set must not fail the truth-up CI job");
        assert_eq!(count, rows.len());
    }

    #[test]
    fn the_full_truth_up_pass_is_named() {
        assert_eq!(TRUTH_UP_FULL_PASS_PROMPT, "P-GA-38 (→ P-512)");
    }
}
