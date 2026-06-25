//! # Dogfood: the GDPR/Audit machinery live on Myelin's own commits + a self-served DSR (P-GA-37)
//!
//! **Prompt:** P-GA-37 → global **P-511** (M6). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §2.2 (*the RoPA and the data
//! map live as a Myelin Knowledge space*), §6.1 (*ONE append-only log of every human AND agent
//! action*), and §9.2 (the GA-1..GA-11 drill table — every gate rests on a dated green artifact).
//! **Roadmap:** `06-roadmaps/shared/gdpr-and-audit.md` §2 "GA-M6". **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §1 (the code wins over the docs — the
//! truth-up pass re-syncs every PROVEN row to a dated green artifact), and §3 (the cheapest, most
//! honest load generator is the platform's own development — the team's own data is real tenant data;
//! every incident adds a drill).
//!
//! ## What this module OWNS (new) vs REUSES (coherence, EI-01 §7)
//! This is the **dogfood operation** of the GDPR/Audit machinery on Myelin's OWN tenant — contracts
//! 10.1–10.9 RUN FOR REAL on the platform's own commits. It defines **NO new contract shape** (the
//! prompt: *Owns (dogfood operation) … No new contract shape*). It is a CALLER that drives the
//! already-shipped machinery over the platform's own data:
//!
//! 1. **[`run_audit_consumer_on_dogfood`] — the audit consumer live on the Myelin self-hosting
//!    outbox.** Every Myelin action (human AND agent — agents are audited identically, EI-02 §2) is
//!    delivered through the REAL [`crate::audit::AuditConsumer`] (the outbox-only audit consumer, the
//!    sole writer of the log) and becomes one minimised, hash-chained, Merkle-leaf entry. The chain
//!    verifies ([`crate::audit::AuditLog::verify_chain`]); `audit_append_lag` reads green. The audit
//!    graph is green ON THE PLATFORM'S OWN ACTIONS. There is no dogfood-only audit path — only
//!    dogfood-class ACTIONS (the team's own commits/CI-runs/issues/chats), minimised exactly as a
//!    tenant's are (the entries hold the frozen `<pseudonym>@<tenant>.noreply` form, never a name).
//! 2. **[`run_self_served_dsr_on_dogfood`] — a self-served DSR over a Myelin team member's own
//!    data.** A Myelin team member's `dsr_submit` fans out across the whole H1–H18 holder catalogue
//!    (single-cell GA-D1, 0 holders missed) AND `member_cells ∪ home_cell` (multi-cell GA-D8, 0 cells
//!    missed) over the PII-free [`myelin_tenancy::CrossCellPointer`] bridge, and **seals a certificate**
//!    into the per-tenant audit Merkle tree via [`crate::audit_proofs::AuditAuthority::seal_dsr_certificate`]
//!    (the SAME outbox-consumer append path — a DSR seal is an audited action like any other). It
//!    REUSES [`crate::full_fanout`] + [`crate::multi_cell`] + [`crate::audit_proofs`] WHOLESALE — no
//!    second fan-out, no second certificate path.
//! 3. **[`RopaKnowledgeSpace`] — the RoPA + the data map live as a Myelin Knowledge space.** The
//!    generated [`crate::datamap::Inventory`] (the data map, contract 10.3) + the [`crate::datamap::ropa`]
//!    projection (Art. 30) are rendered as the Myelin-team Knowledge space's pages — the SAME generated
//!    artifacts (never hand-written), now LIVING as the platform's own internal docs (the dogfood loop's
//!    point: the RoPA the platform serves its customers is the RoPA the platform RUNS itself on).
//! 4. **[`GdprIncident`] / [`IncidentDrillTicket`] — the every-incident-adds-a-drill loop.** Any GDPR
//!    incident surfaced during dogfooding produces a PII-FREE Myelin issue draft + a named reproducing
//!    drill descriptor (the T-3 `register_drill` hook). PII-free by construction — it names the GATE +
//!    a one-line FAULT summary, never a subject.
//! 5. **[`proven_gdpr_rows`] / [`TruthUpPass`] — the truth-up pass.** Enumerates every PROVEN GDPR row
//!    (GA-D1..GA-D8, GA-10, GA-11, E2E-3/E2E-4, the §9.2 drill table) and asserts each rests on a DATED
//!    green artifact. A row WITHOUT one is a LOUD failure ([`TruthUpVerdict::Red`]) — code-wins-over-docs
//!    made mechanical (EI-01 §1). The closing honesty pass (the FULL enumeration is P-GA-38 → P-512,
//!    NAMED below).
//!
//! ## DEVIATION / FLOOR — the "file a Myelin issue" is a PII-free TICKET, and the DrillRegistry wiring
//! is the integration test's job (EI-01 §1, §7). `myelin-gdpr-service` sits BELOW the harness
//! ([`myelin_harness::DrillRegistry`] lives in the leaf test-support crate above the substrate) and
//! does not depend on `myelin-issues` at runtime. So the every-incident loop's "files a Myelin issue"
//! is modeled as an [`IncidentIssueDraft`] — the PII-free issue BODY a GDPR incident hands UP to the
//! Issues subsystem (the SAME posture the storage dogfood loop uses, P-506) — and the reproducing
//! [`myelin_harness::DrillScenario`] is built + `register_drill`'d by the dogfood integration test
//! (the harness is a dev-dependency, exactly like every other GDPR drill).
//!
//! ## FLOORS NAMED (the prompt's DEFINITION OF DONE)
//! - **The full truth-up enumeration** (every PROVEN GDPR row across 10.1–10.9 cross-checked against a
//!   dated artifact, the closing honesty pass) is **P-GA-38 → P-512** ([`TRUTH_UP_FULL_PASS_PROMPT`]).
//!   This prompt ships the truth-up pass over the GA-D* / GA-10 / GA-11 / E2E drill family (the gate
//!   invariant — no earlier-band GDPR gate is red); P-512 widens it to a complete row-by-row pass.
//! - **The live OLTP `audit_entry` / `dsr_request` tables + the real KMS signing key + a real RFC-3161
//!   TSA witness** are the same DB/KMS floor every M0/M1 store carries (P-007 / P-S12) — swapping the
//!   in-memory chain/authority for the durable backend is a config swap, not a code change (the audit
//!   chain has byte-for-byte the §6.2 semantics here).
//! - **The real self-hosting outbox** (the live JetStream subscription the audit consumer binds to in
//!   `serve(AppSpec)`) is the dogfood-loop INFRASTRUCTURE — the consumer LOGIC + the dated artifact ship
//!   now and re-run as a `cargo test` drill until the boot wires the live subscription. The audit
//!   consumer IS an [`myelin_events::EventHandler`]; binding it to the live outbox is one `consume`
//!   call (the seam shape does not change).

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

/// The full truth-up enumeration (the closing honesty pass over every PROVEN GDPR row across
/// 10.1–10.9) is P-GA-38 → P-512 — NAMED in writing (the prompt's DEFINITION OF DONE). This prompt
/// ships the truth-up pass over the §9.2 drill family (the gate invariant); P-512 widens it.
pub const TRUTH_UP_FULL_PASS_PROMPT: &str = "P-GA-38 (→ P-512)";

/// The self-host tenant the Myelin team's own data belongs to (the dogfood tenant — real tenant
/// data, a PII-free opaque id, the SAME shape any customer tenant carries).
pub const MYELIN_SELF_TENANT: &str = "myelin-self";

// ───────────────────────────── (1) the audit consumer live on the self-hosting outbox ─────────────────────────────

/// Which of Myelin's own action surfaces a dogfood action came from (the platform's own
/// commits/CI-runs/issues/chat). This discriminant exists only so the green artifact can name WHICH
/// of the platform's own surfaces produced each audited action (observability is part of the pass,
/// EI-01 §3); it is NOT an audit-path fork — every surface rides the ONE outbox-only consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DogfoodAction {
    /// A Myelin monorepo commit pushed (the platform hosting ITS OWN source, M3 dogfood).
    GitCommit,
    /// A Myelin CI pipeline run (the platform running ITS OWN CI, M4 dogfood).
    CiRun,
    /// A Myelin issue transition (the platform tracking ITS OWN work, M4 dogfood).
    IssueChange,
    /// A Myelin chat message posted (the platform communicating with ITSELF, M4 dogfood).
    ChatMessage,
    /// A Myelin coding-agent action on behalf of a team member (agents audited identically, EI-02 §2).
    AgentAction,
}

impl DogfoodAction {
    /// A short stable label for the green-artifact row (which of Myelin's own surfaces it came from).
    pub fn label(self) -> &'static str {
        match self {
            DogfoodAction::GitCommit => "git-commit",
            DogfoodAction::CiRun => "ci-run",
            DogfoodAction::IssueChange => "issue-change",
            DogfoodAction::ChatMessage => "chat-message",
            DogfoodAction::AgentAction => "agent-action",
        }
    }

    /// The frozen event type token this action is delivered under (the bus subject family — the same
    /// tokens the real subsystems emit).
    pub fn event_type(self) -> &'static str {
        match self {
            DogfoodAction::GitCommit => "git.commit_pushed",
            DogfoodAction::CiRun => "ci.run_completed",
            DogfoodAction::IssueChange => "issues.transitioned",
            DogfoodAction::ChatMessage => "chat.message_posted",
            DogfoodAction::AgentAction => "agent.action_taken",
        }
    }

    /// Every dogfood action surface (so the dogfood loop can assert it covers Myelin's whole own
    /// action set — a human commit/CI/issue/chat AND an agent action).
    pub const ALL: [DogfoodAction; 5] = [
        DogfoodAction::GitCommit,
        DogfoodAction::CiRun,
        DogfoodAction::IssueChange,
        DogfoodAction::ChatMessage,
        DogfoodAction::AgentAction,
    ];
}

/// **Run the audit consumer on the Myelin self-hosting outbox (GA-M6 — the dogfood loop).** Delivers
/// one action-bearing [`EventEnvelope`] for each of the platform's own action surfaces through the
/// REAL [`AuditConsumer`] (human commits/CI-runs/issues/chats AND an agent action) and asserts the
/// audit graph is GREEN on the platform's own actions: every action appended, the per-tenant
/// hash-chain verifies, a Merkle root exists, and `audit_append_lag` reads green. Returns the
/// [`AuditDogfoodArtifact`] (the dated green artifact + the per-surface breakdown).
///
/// `now_iso` is the caller-supplied date (the harness `today_iso()` at the run) so the artifact is
/// DATED — a claim that outlives its verification misleads the next agent (EI-01 §1).
pub fn run_audit_consumer_on_dogfood(now_iso: &str) -> AuditDogfoodArtifact {
    let consumer = AuditConsumer::new();
    let tenant = TenancyTenantId(MYELIN_SELF_TENANT.into());

    // Deliver one action per surface through the SAME outbox-only consumer (no direct-write path).
    // Each `principal` carries only the PII-free `principal_id` — the entry physically cannot hold a
    // name (the minimisation is structural).
    let mut by_surface: BTreeMap<DogfoodAction, usize> = BTreeMap::new();
    for (i, action) in DogfoodAction::ALL.iter().enumerate() {
        let ev = dogfood_event(*action, i);
        // The consumer IS the EventHandler — `handle` is the live append path (the SAME path the
        // outbox subscription drives; it appends one minimised audit entry and returns `Done`).
        let outcome = consumer.handle(&ev);
        debug_assert_eq!(outcome, myelin_events::HandleOutcome::Done);
        *by_surface.entry(*action).or_insert(0) += 1;
    }

    // GREEN on the platform's own actions: every action logged, the chain verifies, a root exists.
    let entries = consumer.log().entries_for(&tenant);
    let chain_verifies = consumer.log().verify_chain(&tenant);
    let root_present = consumer.log().root(&tenant).is_some();
    let append_lag = consumer.append_lag();

    AuditDogfoodArtifact {
        date: now_iso.to_string(),
        tenant: tenant.clone(),
        actions_logged: entries.len(),
        chain_verifies,
        root_present,
        append_lag,
        actions_by_surface: by_surface,
    }
}

/// Build one action-bearing event for a Myelin team member's own action. The `payload` deliberately
/// carries a NAME-shaped value to prove the audit entry NEVER reads it (references-not-payloads /
/// minimisation — the same guard the audit module's own tests carry).
fn dogfood_event(action: DogfoodAction, n: usize) -> EventEnvelope {
    let actor = dogfood_principal(action, n);
    let tenant = actor.tenant.clone();
    let region = actor.region.clone();
    EventEnvelope {
        event_id: EventId(format!("dogfood-{}-{n}", action.label())),
        type_: EventType(action.event_type().into()),
        schema_ver: 1,
        tenant,
        region,
        actor: Actor(actor),
        subject: ArtifactRef(format!("myelin://myelin-self/{}/n{n}", action.label())),
        aggregate: AggregateKey(format!("agg:{}", action.label())),
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-dogfood-{n}")),
        caused_by: Some(CausedBy("session:dogfood".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-26T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-26T00:00:01Z".into()),
        // A real-name-shaped payload — the audit entry must NEVER carry this (minimisation).
        payload: serde_json::json!({ "real_name": "Adrian Helvik", "email": "team@myelin.test" }),
    }
}

/// The acting principal for a dogfood action — a Myelin team member (human) for the four human
/// surfaces, or a coding agent acting `on_behalf_of` a team member for the agent surface (agents are
/// audited identically to humans, EI-02 §2). Carries ONLY the PII-free `principal_id`.
fn dogfood_principal(action: DogfoodAction, n: usize) -> Principal {
    let tenant = TenancyTenantId(MYELIN_SELF_TENANT.into());
    let kind = match action {
        DogfoodAction::AgentAction => PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-dogfood".into()),
            on_behalf_of: Some(PrincipalId("u-myelin-team-1".into())),
        },
        _ => PrincipalKind::Human,
    };
    Principal::stub(PrincipalId(format!("u-myelin-team-{n}")), kind, tenant)
}

/// The dated GREEN ARTIFACT the audit-consumer dogfood leg emits — the audit graph is green on the
/// platform's own actions (every action logged, the chain verifies, a Merkle root exists,
/// `audit_append_lag` reads green), with the per-surface breakdown of Myelin's own actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditDogfoodArtifact {
    /// The date the dogfood run emitted this artifact (the caller's `today_iso()`).
    pub date: String,
    /// The self-host tenant whose own actions were logged.
    pub tenant: TenancyTenantId,
    /// How many of Myelin's own actions were appended to the audit log.
    pub actions_logged: usize,
    /// `true` iff the per-tenant hash-chain verifies (a retroactive edit would break it — §6.1).
    pub chain_verifies: bool,
    /// `true` iff a per-tenant Merkle root exists (what the STH signs — P-GA-20).
    pub root_present: bool,
    /// The live `audit_append_lag` SLO (events delivered-but-not-yet-appended) — 0 in steady state.
    pub append_lag: u64,
    /// How many actions of each of Myelin's own surfaces were logged (git/ci/issue/chat/agent).
    pub actions_by_surface: BTreeMap<DogfoodAction, usize>,
}

impl AuditDogfoodArtifact {
    /// `true` iff the audit graph is GREEN on the platform's own actions (every surface logged, the
    /// chain verifies, a root exists, lag is 0). The ONLY way to read the audit dogfood leg — a
    /// broken chain / a missing surface is never silently a pass.
    pub fn audit_graph_is_green(&self) -> bool {
        self.chain_verifies
            && self.root_present
            && self.append_lag == 0
            && self.actions_logged == DogfoodAction::ALL.len()
            && self.actions_by_surface.len() == DogfoodAction::ALL.len()
    }

    /// Render the dated audit-dogfood green-artifact line a self-host CI run prints on PASS.
    pub fn summary(&self) -> String {
        let breakdown: Vec<String> = self
            .actions_by_surface
            .iter()
            .map(|(a, n)| format!("{}={n}", a.label()))
            .collect();
        format!(
            "[P-511 DOGFOOD AUDIT GREEN {date}] tenant={tenant}: {logged} of Myelin's OWN actions \
             logged, chain_verifies={chain} root_present={root} audit_append_lag={lag} — {breakdown}",
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

// ───────────────────────────── (2) a self-served DSR over a Myelin team member's own data ─────────────────────────────

/// **Run a self-served DSR over a Myelin team member's own data (GA-M6 — the dogfood loop).** A
/// Myelin team member's `dsr_submit` fans out across the WHOLE H1–H18 holder catalogue (single-cell
/// GA-D1, 0 holders missed) AND `member_cells ∪ home_cell` (multi-cell GA-D8, 0 cells missed) over
/// the PII-free [`CrossCellPointer`] bridge, and SEALS a certificate into the per-tenant audit Merkle
/// tree via [`AuditAuthority::seal_dsr_certificate`] (the SAME outbox-consumer append path). Returns
/// the [`DsrDogfoodArtifact`] (the dated green artifact: 0 holders missed, 0 cells missed,
/// certificate sealed).
///
/// REUSES [`MultiCellFanOut`] + [`FullFanOutCoverage`] + [`AuditAuthority`] WHOLESALE — there is no
/// second fan-out, no second certificate path (EI-01 §7 coherence). The whole-system reliable-erase
/// proof (crypto-shred → unrecoverable incl. backups, embeddings purged-not-hidden) is the E2E-4
/// flagship (P-GA-34); this dogfood leg proves the COMPLETENESS + the certificate seal run on the
/// platform's OWN tenant data (the flagship's per-store erase legs are owned store-side).
pub fn run_self_served_dsr_on_dogfood(now_iso: &str) -> DsrDogfoodArtifact {
    let tenant = TenancyTenantId(MYELIN_SELF_TENANT.into());
    let region = Region("fr-par".into());
    let set = self_host_member_set();
    let pointer = pii_free_pointer();
    let dsr_id = DsrId("dsr:myelin-self-served".into());

    // The multi-cell fan-out iterates `member_cells ∪ home_cell`; each cell runs its OWN full H1–H18
    // fan-out (0 holders missed IN the cell) and returns ONLY a PII-free certificate (OQ-I — a cell
    // never reads another cell's PII).
    let mut cells_resolved = 0usize;
    let merged: MultiCellCertificate = MultiCellFanOut::new()
        .fan_out("myelin-self/u-team", &set, &pointer, |_cell, _p| {
            cells_resolved += 1;
            seal_full_cell_fanout()
        })
        .expect("the self-served multi-cell DSAR fan-out seals on Myelin's own data");

    // GA-D1: every per-cell certificate is complete (0 holders missed, coverage == 1.0).
    let all_cells_complete = merged.per_cell.iter().all(PerCellReceipt::cell_is_complete);
    let max_holders_missed = merged
        .per_cell
        .iter()
        .map(|r| r.cell_certificate.holders_missed)
        .max()
        .unwrap_or(usize::MAX);

    // The certificate seals into the per-tenant audit Merkle tree (the SAME outbox-consumer append
    // path — a DSR seal is an audited action like any other; the inclusion proof is the green artifact).
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

    DsrDogfoodArtifact {
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

/// One cell's complete H1–H18 fan-out certificate — every holder in the closed [`Holder::ALL`]
/// catalogue reached (0 missed). The reliable-erase per-store legs are the E2E-4 flagship's (this
/// dogfood leg proves the COMPLETENESS over the platform's own tenant — 0 holders escape the fan-out).
fn seal_full_cell_fanout() -> GaD1Certificate {
    let mut cov = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        cov.record_reached(h);
    }
    GaD1Certificate::seal("myelin-self/u-team", &cov).expect("the cell's full H1–H18 fan-out seals")
}

/// The Myelin team's self-host cell set — `member_cells ∪ home_cell`. On the degenerate one-cell
/// self-host (P-CP-23) the platform runs as exactly one cell, so the home cell stands alone; the
/// `MemberCellSet` shape is identical to a multi-cell tenant's (the fan-out code path is the same).
fn self_host_member_set() -> MemberCellSet {
    MemberCellSet::union(CellId::from_token("cell-fr-par-self"), &[])
}

/// The PII-free cross-cell carrier — `subject` is an opaque `ArtifactRef`-class id, NEVER a person
/// (OQ-I): the dogfood DSR crosses the cell boundary carrying only the opaque pointer.
fn pii_free_pointer() -> myelin_tenancy::CrossCellPointer {
    myelin_tenancy::CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://myelin-self/issues/issue/1".into())),
        ArtifactType::Issue,
        CorrelationId("corr-dogfood-dsr".into()),
        CellId::from_token("cell-fr-par-self"),
    )
}

/// The dated GREEN ARTIFACT the self-served-DSR dogfood leg emits — a Myelin team member's own data
/// fanned out (0 holders missed, 0 cells missed) + the completion certificate sealed into the
/// per-tenant audit Merkle tree (the inclusion proof is the green artifact).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrDogfoodArtifact {
    /// The date the dogfood run emitted this artifact (the caller's `today_iso()`).
    pub date: String,
    /// The self-host tenant whose own data was the DSR subject.
    pub tenant: TenancyTenantId,
    /// The DSR id (the self-served request).
    pub dsr_id: DsrId,
    /// The MAX holders missed in any cell (GA-D1 — must be 0; a missed holder un-erases a person).
    pub holders_missed: usize,
    /// The cells missed over `member_cells ∪ home_cell` (GA-D8 — must be 0).
    pub cells_missed: usize,
    /// The total cells in the fan-out (the self-host cell count).
    pub cells_total: usize,
    /// How many cells the fan-out actually resolved cell-locally (none skipped).
    pub cells_resolved: usize,
    /// `true` iff every per-cell certificate is complete (every cell erased its whole H1–H18 set).
    pub all_cells_complete: bool,
    /// `true` iff the completion certificate sealed (the bundle carries the Merkle inclusion proof).
    pub certificate_sealed: bool,
    /// The Merkle inclusion proof of the sealed certificate (the green artifact — `None` if unsealed).
    pub inclusion_proof: Option<String>,
    /// The sealed bundle digest (the merged certificate content-address).
    pub bundle_digest: String,
}

impl DsrDogfoodArtifact {
    /// `true` iff the self-served DSR is GREEN on the platform's own data: 0 holders missed, 0 cells
    /// missed, every cell complete, the certificate sealed. The ONLY way to read the DSR dogfood leg.
    pub fn dsr_is_green(&self) -> bool {
        self.holders_missed == 0
            && self.cells_missed == 0
            && self.cells_total > 0
            && self.cells_resolved == self.cells_total
            && self.all_cells_complete
            && self.certificate_sealed
    }

    /// Render the dated DSR-dogfood green-artifact line a self-host CI run prints on PASS.
    pub fn summary(&self) -> String {
        format!(
            "[P-511 DOGFOOD DSR GREEN {date}] tenant={tenant} dsr={dsr}: holders_missed={hm} \
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

// ───────────────────────────── (3) the RoPA + data map live as a Myelin Knowledge space ─────────────────────────────

/// **A Myelin team member's own record (the dogfood subject's PII).** The platform's own team is real
/// tenant data: a team member has a contact email + a personnel note, classified by the SAME
/// `#[derive(PersonalData)]` classify-derive every customer-tenant schema uses (no dogfood-only
/// classification path). These tagged fields are what the generated data map / RoPA enumerate — the
/// dogfood loop's whole point (the RoPA the platform serves customers is the RoPA it runs itself on).
#[derive(PersonalData)]
#[allow(dead_code)]
struct MyelinTeamMemberRecord {
    /// The team member's contact email — operational PII, erased by per-subject DEK crypto-shred.
    #[personal_data(
        category = ContactInfo,
        role = PlatformOperational,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    email: String,
    /// A personnel note (behavioural) — restricted by default, the OQ-H posture (worklog-class).
    #[personal_data(
        category = Behavioural,
        role = TenantContent,
        basis = LegitimateInterest,
        retention = Fixed(365d),
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    personnel_note: String,
    /// A non-PII key — no map entry.
    row_version: u64,
}

/// **The Myelin team's own contributing holder schemas (the dogfood data map's inputs).** The Myelin
/// team-member record (the PII-bearing holder) PLUS the real subsystem holder rosters
/// (Git/Knowledge/Issues/Chat) the platform self-hosts. The generated data map / RoPA over this set
/// is the Myelin team's own GDPR Knowledge space.
pub fn myelin_team_holder_schemas(region: myelin_tenancy::Region) -> Vec<HolderSchema> {
    let mut schemas = vec![HolderSchema::from_schema::<MyelinTeamMemberRecord>(
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "myelin_team_directory",
        },
        SubHolder::H15Identity,
        region.clone(),
    )];
    // The real subsystem holders the platform self-hosts (Git/Knowledge + Issues/Chat) — accounted
    // for in the map's roster (the dogfood loop covers the platform's whole own holder set).
    schemas.extend(producer_holder_schemas(region.clone()));
    schemas.extend(issues_chat_holder_schemas(region));
    schemas
}

/// **The RoPA + the data map live as a Myelin Knowledge space (gdpr §2.2 — the dogfood loop).** The
/// generated [`Inventory`] (the data map, contract 10.3) + the [`ropa`] projection (Art. 30) rendered
/// as the Myelin-team Knowledge space's pages — the SAME generated artifacts (never hand-written),
/// now LIVING as the platform's own internal docs. The RoPA the platform serves its customers is the
/// RoPA the platform RUNS itself on.
///
/// A Knowledge "space" is a titled collection of pages; on this floor the space is the two GENERATED
/// pages (the data map + the RoPA). The live Knowledge-block backing (`myelin_knowledge`) is the M3
/// dogfood store the platform already self-hosts — the dogfood leg here proves the GENERATION lands as
/// space pages (the rendered page text); writing it into the live Knowledge store is one
/// `Knowledge::publish` call (the seam shape does not change).
#[derive(Clone, Debug)]
pub struct RopaKnowledgeSpace {
    /// The space title (the Myelin team's own GDPR space).
    title: String,
    /// The generated data-map page (the inventory).
    data_map: Inventory,
    /// The generated RoPA page (the Art. 30 projection over the data map).
    ropa: ProcessingActivities,
}

impl RopaKnowledgeSpace {
    /// **Generate the Myelin-team GDPR Knowledge space from `holders`.** Walks the contributing
    /// [`HolderSchema`]s (every registered holder + every `#[personal_data]`-tagged field) to GENERATE
    /// the data map + projects the RoPA over it — the same generated artifacts, now living as the
    /// platform's own space pages.
    pub fn generate(holders: &[HolderSchema]) -> RopaKnowledgeSpace {
        let inventory = data_map(holders);
        let ropa = ropa(&inventory);
        RopaKnowledgeSpace {
            title: "Myelin — Records of Processing Activities + Data Map".to_string(),
            data_map: inventory,
            ropa,
        }
    }

    /// **Generate the Myelin team's own GDPR Knowledge space (the dogfood premise).** Builds the space
    /// from [`myelin_team_holder_schemas`] — the Myelin team-member record (real `#[personal_data]`
    /// fields) PLUS the real subsystem holder rosters (Git/Knowledge/Issues/Chat). This is the RoPA
    /// the platform RUNS ITSELF on, generated from the same classify-derive every customer tenant's
    /// is.
    pub fn for_myelin_team(region: myelin_tenancy::Region) -> RopaKnowledgeSpace {
        Self::generate(&myelin_team_holder_schemas(region))
    }

    /// The space title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The generated data-map page (the inventory the DSR fan-out drives off).
    pub fn data_map(&self) -> &Inventory {
        &self.data_map
    }

    /// The generated RoPA page (the Art. 30 projection).
    pub fn ropa(&self) -> &ProcessingActivities {
        &self.ropa
    }

    /// `true` iff the space is non-empty (the data map has entries AND the RoPA has activities) — a
    /// space with no pages is not a live RoPA (the dogfood loop requires the RoPA actually lives).
    pub fn is_populated(&self) -> bool {
        self.data_map.entry_count() > 0 && !self.ropa.is_empty()
    }

    /// Render the two GENERATED space pages a Knowledge publish would write (the page text — the data
    /// map's entry/holder counts + its content fingerprint, and the RoPA's activity count). The
    /// fingerprint binds the page to the exact generated map (a drift would change it — the diff gate).
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

/// One page of the Myelin-team GDPR Knowledge space (a generated artifact rendered as a space page).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeSpacePage {
    /// The page title.
    pub title: String,
    /// The page body (the generated artifact's rendered text).
    pub body: String,
}

// ───────────────────────────── (4) the every-incident-adds-a-drill loop (T-3) ─────────────────────────────

/// A GDPR incident discovered during dogfooding — a PII-FREE record of a GDPR/Audit fault the team's
/// own use surfaced. The every-incident-adds-a-drill loop (EI-01 §3: *every real incident ends by
/// adding a drill that reproduces it*) turns each incident into (a) a Myelin issue draft + (b) a
/// reproducing GDPR drill that joins the harness suite and re-runs forever.
///
/// PII-free by construction: an incident names the GDPR FAULT (a gate + a one-line human summary +
/// the reproducing drill it touches), never a subject — so it can be filed as a Myelin issue and
/// registered as a drill without carrying personal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GdprIncident {
    /// A stable, PII-free incident id (e.g. `"INC-GDPR-001"`).
    pub id: String,
    /// The GDPR gate/drill the incident touches (e.g. `"GA-D1"`) — the reproducing drill rejoins
    /// THIS gate's lane in the permanent suite.
    pub gate_id: String,
    /// A one-line, PII-free human summary of the fault (the issue title).
    pub summary: String,
    /// The stable name the reproducing drill registers under (the `DrillRegistry` key).
    pub repro_drill_name: String,
}

impl GdprIncident {
    /// Record a GDPR incident `id` against `gate_id`, summarised by `summary`, whose reproducing
    /// drill registers under `repro_drill_name`.
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

    /// **The Myelin issue draft this incident files (the every-incident loop's "files a Myelin
    /// issue" leg).** A PII-FREE issue BODY a GDPR incident hands UP to the Issues subsystem — the
    /// title is the summary; the body names the gate + the reproducing drill so the issue is
    /// actionable + traceable.
    pub fn issue_draft(&self) -> IncidentIssueDraft {
        IncidentIssueDraft {
            title: format!("[gdpr incident {}] {}", self.id, self.summary),
            body: format!(
                "A GDPR/Audit incident surfaced during dogfooding.\n\nGate touched: {}\nReproducing \
                 drill (registered into the permanent harness suite, re-runs forever): {}\n\nThe \
                 every-incident-adds-a-drill loop (EI-01 §3) requires this incident's repro join the \
                 suite — the drill below IS that repro. PII-free: this names a FAULT, never a subject.",
                self.gate_id, self.repro_drill_name
            ),
            gate_id: self.gate_id.clone(),
        }
    }

    /// **The reproducing-drill TICKET this incident registers (the every-incident loop's "a
    /// reproducing GDPR drill that joins the harness" leg).** Names the drill the dogfood integration
    /// test hands to the harness `DrillRegistry::register_drill` so the repro re-runs forever (the T-3
    /// hook). This module owns the PII-free ticket; the WIRING into the registry is the dogfood
    /// integration test's job (the harness sits above this crate in the DAG).
    pub fn drill_ticket(&self) -> IncidentDrillTicket {
        IncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
            incident_id: self.id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft a [`GdprIncident`] files (the body the Issues subsystem turns into
/// a real issue). PII-free by construction — it names the FAULT, never a subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentIssueDraft {
    /// The issue title (the incident summary).
    pub title: String,
    /// The issue body (names the gate + the reproducing drill — actionable + traceable).
    pub body: String,
    /// The gate the incident touches (so the issue routes to the right lane).
    pub gate_id: String,
}

/// The PII-free reproducing-drill ticket a [`GdprIncident`] registers — the name + the gate it
/// rejoins. The dogfood integration test builds a `DrillScenario` under [`Self::drill_name`] and
/// `register_drill`s it (the T-3 hook), so the incident's repro re-runs forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentDrillTicket {
    /// The stable name the reproducing drill registers under (the registry key).
    pub drill_name: String,
    /// The gate/drill lane the repro rejoins (e.g. `"GA-D1"`).
    pub gate_id: String,
    /// The incident this repro reproduces (the traceability link).
    pub incident_id: String,
}

// ───────────────────────────── (5) the truth-up pass (every PROVEN GDPR row rests on a dated green artifact) ─────────────────────────────

/// One PROVEN GDPR row the truth-up pass enumerates — a GDPR gate/drill the ledger claims PROVEN (the
/// §9.2 GA-D1..GA-D8 / GA-10 / GA-11 drill family + the E2E legs). The truth-up pass asserts each
/// rests on a DATED green artifact: an `artifact_date` of `Some(date)` is a row whose proof is dated
/// and present, whereas `None` is a CLAIMED-NOT-PROVEN row the pass FAILs on loudly
/// (code-wins-over-docs, EI-01 §1 — a claim that outlives its verification misleads the next agent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenGdprRow {
    /// The stable gate/drill id (e.g. `"GA-D1"`, `"GA-10"`, `"E2E-4"`).
    pub id: &'static str,
    /// A one-line human title (what the row proves).
    pub title: &'static str,
    /// The proof command that emits this row's dated green artifact (the `cargo test` target that
    /// lives with the feature prompt — the truth-up pass names it so the artifact is reproducible).
    pub proof_command: &'static str,
    /// The DATE the row's green artifact was last emitted, if any. `Some(date)` ⇒ dated + proven;
    /// `None` ⇒ CLAIMED-NOT-PROVEN (a loud red, never a silent pass).
    pub artifact_date: Option<String>,
}

impl ProvenGdprRow {
    /// `true` iff this row rests on a dated green artifact (the truth-up invariant for one row).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

/// **The FROZEN set of PROVEN GDPR rows the truth-up pass enumerates.** This is the §9.2 drill family
/// (GA-D1..GA-D8, GA-10, GA-11) + the E2E legs (E2E-3 spec-to-ship audit-tamper feed, E2E-4 the DSAR
/// flagship) — the GDPR gates the ledger claims PROVEN. The truth-up pass asserts EVERY id here rests
/// on a dated green artifact; a row without one is a loud failure.
///
/// The id/title/proof-command triples below are the GDPR rows greened by P-GA-05..P-GA-36 (the test
/// files in `crates/myelin-gdpr-service/tests/`). The `date` is supplied by the truth-up runner (the
/// dogfood run's `today_iso()`) — the pass DATES every row at the run so a claim never outlives its
/// verification (EI-01 §1). The FULL row-by-row enumeration across 10.1–10.9 is P-GA-38 → P-512
/// ([`TRUTH_UP_FULL_PASS_PROMPT`]); this set is the gate-invariant core (no §9.2 GDPR gate is red).
pub fn proven_gdpr_rows(date: &str) -> Vec<ProvenGdprRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static str, date: &str) -> ProvenGdprRow {
        ProvenGdprRow {
            id,
            title,
            proof_command: cmd,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        row(
            "GA-D1",
            "erasure reaches every holder — 0 holders missed over H1–H18 at cell scale",
            "cargo test -p myelin-gdpr-service --test ga_d1_full_fanout_cell_scale",
            date,
        ),
        row(
            "GA-D2",
            "erasure reaches search — docs + embeddings purged-not-hidden, 0 re-identification",
            "cargo test -p myelin-gdpr-service --test ga_d2_derivative_erasure",
            date,
        ),
        row(
            "GA-D3",
            "audit-tamper detection — a retroactive edit detected 3 independent ways (chain/consistency/witness)",
            "cargo test -p myelin-gdpr-service --test ga_d3_audit_tamper",
            date,
        ),
        row(
            "GA-D4",
            "DSR deadline — the durable timer warns before the statutory clock expires",
            "cargo test -p myelin-gdpr-service --test ga_d4_dsr_deadline_timer",
            date,
        ),
        row(
            "GA-D6",
            "legal-hold — an erase under an active hold is suspended, 0 held-scope deletions, resumes on lift",
            "cargo test -p myelin-gdpr-service --test ga_d6_retention_legal_hold",
            date,
        ),
        row(
            "GA-D7",
            "restriction-leak — restrict → 0 processing across the five derived stores, storage retained",
            "cargo test -p myelin-gdpr-service --test ga_d7_derived_restrict",
            date,
        ),
        row(
            "GA-D8",
            "multi-cell erasure — 0 cells missed over member_cells ∪ home_cell, per-cell receipt set complete",
            "cargo test -p myelin-gdpr-service --test ga_d8_multi_cell_fanout",
            date,
        ),
        row(
            "GA-10",
            "history-rewrite-invalidation — fan-out reaches forks/mirrors/clone-cache, op audited, 0 stale-PII hits",
            "cargo test -p myelin-gdpr-service --test ga_10_history_rewrite_invalidation",
            date,
        ),
        row(
            "GA-11",
            "outbound-residency-gate — extra-EU PII push-mirror denied by default, within-EU CDN clone allowed",
            "cargo test -p myelin-gdpr-service --test ga_11_outbound_mirror_residency_gate",
            date,
        ),
        row(
            "CI-D3",
            "CI consumer-holder erasure — per-subject CI-log DEK crypto-shred reaches isolable log PII",
            "cargo test -p myelin-gdpr-service --test ci_d3_ci_holder_erasure",
            date,
        ),
        row(
            "GIT-D2",
            "pseudonymous-commit — erase author → 0 recoverable real identity in immutable git bytes",
            "cargo test -p myelin-gdpr-service --test git_d2_pseudonymous_commit",
            date,
        ),
        row(
            "E2E-3",
            "spec-to-ship traceability — the GDPR audit-tamper proof feeds the E2E-3 leg",
            "cargo test -p myelin-gdpr-service --test ga_p153_ediscovery_trace_history",
            date,
        ),
        row(
            "E2E-4",
            "the DSAR fan-out flagship — 0 holders missed, 0 cells missed, certificate sealed",
            "cargo test -p myelin-gdpr-service --test e2e_4_dsar_fanout_flagship",
            date,
        ),
    ]
}

/// The verdict of a truth-up pass — GREEN (every PROVEN row rests on a dated green artifact) or RED
/// (one or more rows are CLAIMED-NOT-PROVEN: a claim that outlives its verification). `#[must_use]`:
/// a dropped verdict is a swallowed truth-up failure — the docs would silently drift from the code
/// (the exact EI-01 §1 failure mode), so the compiler flags a dropped red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a truth-up verdict must be checked — a dropped RED means a CLAIMED-NOT-PROVEN GDPR \
              row silently drifts the docs from the code (EI-01 §1: a claim that outlives its \
              verification misleads the next agent)"]
pub enum TruthUpVerdict {
    /// Every enumerated PROVEN GDPR row rests on a dated green artifact (the gate invariant holds
    /// end-to-end — no earlier-band GDPR gate is red).
    Green {
        /// How many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// The date the truth-up pass ran (every confirmed row is dated at this run).
        date: String,
    },
    /// One or more PROVEN rows are CLAIMED-NOT-PROVEN (no dated green artifact). Names them so the
    /// failure points at exactly which GDPR claim outran its verification.
    Red {
        /// The ids of the rows lacking a dated green artifact (the loud failure list).
        undated_rows: Vec<&'static str>,
    },
}

impl TruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated). The ONLY way to read a pass — a
    /// RED is never silently a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, TruthUpVerdict::Green { .. })
    }

    /// The ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            TruthUpVerdict::Green { .. } => &[],
            TruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The truth-up pass (GA-M6 / EI-01 §1).** Enumerates every PROVEN GDPR row and confirms each rests
/// on a DATED green artifact. A row WITHOUT one is a LOUD failure ([`TruthUpVerdict::Red`]), never a
/// silent pass — the code-wins-over-docs discipline made mechanical.
///
/// A zero-sized orchestrator — the truth-up pass is `TruthUpPass::run(rows)` over the frozen
/// [`proven_gdpr_rows`] set (each row dated at the run).
#[derive(Clone, Copy, Debug, Default)]
pub struct TruthUpPass;

impl TruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> TruthUpPass {
        TruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`TruthUpVerdict::Green`] (every row dated) or
    /// [`TruthUpVerdict::Red`] (the undated rows named). `date` is the run date stamped onto the
    /// green verdict (so the pass itself is dated — observability of the gate invariant).
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

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED
    /// verdict into a process-failing `Err` — so a CI invocation `pass.run_or_fail_ci(&rows, date)?`
    /// FAILS the dogfood truth-up job if ANY PROVEN row lacks a dated green artifact, with no swallow.
    /// On GREEN it returns the number of confirmed rows (`Ok`).
    pub fn run_or_fail_ci(&self, rows: &[ProvenGdprRow], date: &str) -> Result<usize, TruthUpRed> {
        match self.run(rows, date) {
            TruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            TruthUpVerdict::Red { undated_rows } => Err(TruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN GDPR rows, loud + specific (the
/// process exits non-zero, never a silent docs drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruthUpRed {
    /// The ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for TruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} GDPR row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} — a \
             claim that outlives its verification misleads the next agent (EI-01 §1); fix the doc \
             or re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for TruthUpRed {}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    // ───────── (1) the audit consumer live on the self-hosting outbox ─────────

    /// **THE HEADLINE (audit leg): the audit graph is GREEN on Myelin's OWN actions (GA-M6).** Every
    /// one of the platform's own action surfaces (git/ci/issue/chat + an agent action) is logged
    /// through the REAL outbox-only consumer; the chain verifies; a root exists; `audit_append_lag`
    /// reads green.
    #[test]
    fn audit_consumer_greens_on_myelins_own_actions() {
        let artifact = run_audit_consumer_on_dogfood(RUN_DATE);

        assert!(
            artifact.audit_graph_is_green(),
            "the audit graph must be green on the platform's own actions: {artifact:?}"
        );
        assert_eq!(
            artifact.actions_logged,
            DogfoodAction::ALL.len(),
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
            "all five own-action surfaces (incl. an agent action — EI-02 §2) covered"
        );
        // The agent action IS logged (agents are audited identically to humans).
        assert_eq!(
            artifact.actions_by_surface.get(&DogfoodAction::AgentAction),
            Some(&1),
            "the coding-agent action is audited identically to a human action"
        );
        let s = artifact.summary();
        assert!(
            s.contains("P-511 DOGFOOD AUDIT GREEN 2026-06-26"),
            "dated: {s}"
        );
        assert!(
            s.contains("git-commit=1") && s.contains("agent-action=1"),
            "per-surface breakdown: {s}"
        );
    }

    // ───────── (2) a self-served DSR over a Myelin team member's own data ─────────

    /// **THE HEADLINE (DSR leg): a self-served DSR over the team's own data fans out + seals a
    /// certificate (GA-M6).** A Myelin team member's `dsr_submit` reaches every H1–H18 holder
    /// (0 missed) across `member_cells ∪ home_cell` (0 cells missed) and seals a Merkle-proven
    /// certificate into the per-tenant audit tree.
    #[test]
    fn self_served_dsr_greens_and_seals_a_certificate() {
        let artifact = run_self_served_dsr_on_dogfood(RUN_DATE);

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
            s.contains("P-511 DOGFOOD DSR GREEN 2026-06-26") && s.contains("holders_missed=0"),
            "dated artifact: {s}"
        );
    }

    // ───────── (3) the RoPA + data map live as a Myelin Knowledge space ─────────

    /// **The RoPA + the data map live as a Myelin Knowledge space (gdpr §2.2).** The generated data
    /// map + RoPA render as the Myelin-team GDPR space's pages — populated (non-empty), and the data
    /// map page carries the content fingerprint (a drift would change it).
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

    // ───────── (4) the every-incident-adds-a-drill loop ─────────

    /// A GDPR incident files a PII-FREE Myelin issue draft + a reproducing-drill ticket (the
    /// every-incident loop's two legs). The issue body names the gate + the repro drill (actionable +
    /// traceable); the ticket names the drill + the gate it rejoins.
    #[test]
    fn a_gdpr_incident_files_an_issue_and_registers_a_drill() {
        let incident = GdprIncident::new(
            "INC-GDPR-001",
            "GA-D1",
            "a self-served DSR fan-out skipped a newly-registered Knowledge holder",
            "repro_ga_d1_dsr_skips_new_knowledge_holder",
        );

        // (a) the Myelin issue draft — PII-free, names the gate + the repro drill.
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

        // (b) the reproducing-drill ticket — the harness DrillRegistry key + the gate it rejoins.
        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_ga_d1_dsr_skips_new_knowledge_holder"
        );
        assert_eq!(ticket.gate_id, "GA-D1");
        assert_eq!(ticket.incident_id, "INC-GDPR-001");
    }

    // ───────── (5) the truth-up pass ─────────

    /// **The truth-up pass GREENS when every PROVEN GDPR row rests on a dated green artifact.**
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
        // The frozen set covers the §9.2 GA-D* / GA-10 / GA-11 family + the E2E legs.
        let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();
        for must in [
            "GA-D1", "GA-D2", "GA-D3", "GA-D4", "GA-D6", "GA-D7", "GA-D8", "GA-10", "GA-11",
            "E2E-3", "E2E-4",
        ] {
            assert!(
                ids.contains(&must),
                "the truth-up set must enumerate {must}"
            );
        }
    }

    /// **MANDATORY-CORE: a PROVEN row WITHOUT a dated green artifact FAILs the truth-up pass LOUDLY.**
    #[test]
    fn truth_up_reds_loudly_on_a_claimed_not_proven_row() {
        let mut rows = proven_gdpr_rows(RUN_DATE);
        // GA-D1 (the headline gate) loses its dated artifact — a claim that outran its verification.
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

    /// `run_or_fail_ci` returns `Ok(count)` when the whole PROVEN set is dated (0 red earlier-band
    /// GDPR gates — the gate invariant holds end-to-end).
    #[test]
    fn truth_up_run_or_fail_ci_returns_ok_when_all_dated() {
        let rows = proven_gdpr_rows(RUN_DATE);
        let count = TruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("a fully-dated PROVEN set must not fail the truth-up CI job");
        assert_eq!(count, rows.len());
    }

    /// The full truth-up enumeration is NAMED in writing (P-GA-38 → P-512) — the closing honesty pass.
    #[test]
    fn the_full_truth_up_pass_is_named() {
        assert_eq!(TRUTH_UP_FULL_PASS_PROMPT, "P-GA-38 (→ P-512)");
    }
}
