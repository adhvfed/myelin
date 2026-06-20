//! # The DSR orchestrator API + the state machine + the controller/processor posture gate
//! (P-GA-11 → P-111)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§4.1** (the state
//! machine `received → validated → fanned-out → {awaiting-holders} → verified → completed`,
//! total + ordered, no skipping `awaiting-holders`; `deadline = now + 1 month`, Art. 12(3)) and
//! **§1** (the two legal postures — **processor** for *tenant content* (the customer org is the
//! controller; Myelin must NOT unilaterally erase tenant content except on tenant instruction or
//! offboarding), **controller** for *platform-operational* data (Myelin is the first-line DSR
//! responder)). The DSR API shape is **§4** / §8.1 + contract-index row **10.4**
//! (`dsr_submit(kind, subject, scope, posture) → dsr_id`; `dsr_status(dsr_id) → {state,
//! deadline, checklist}`; `dsr_certificate(dsr_id) → MerkleProvenBundle`).
//!
//! **Contract-index:** row **10.4** (OWNED here — the DSR API + the state machine + the posture
//! gate). Consumed: row **10.3** (`data_map()` — the orchestrator reads the generated inventory
//! to resolve a *read-only* per-holder checklist into `dsr_status`; the map, not a hand-written
//! list, drives the scope — gdpr §4.1 step 2). The pseudonym lever (4.8) is consumed by the
//! fan-out, which is **P-GA-12**, not here.
//!
//! ## What THIS prompt (P-GA-11) ships — and what it explicitly DEFERS
//! This prompt ships the DSR *spine*: the **synchronous state machine** (total + ordered), the
//! **controller/processor posture gate** (the validate step that refuses a Myelin-initiated
//! erase of tenant content), the **coarse deadline** (`now + 1 month`, set on submit), and the
//! three API entry points (`dsr_submit` / `dsr_status` / `dsr_certificate`). The certificate
//! here carries the signed per-holder receipts collected so far; the **Merkle seal** of that
//! bundle into the per-tenant audit tree is wired in **P-GA-20**.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The per-holder checklist drive + the resumable fan-out + the verifiable receipts + the
//!   legal-hold gate** → **P-GA-12 → P-112, NOW FILLED in [`crate::fanout`]** (this prompt
//!   RESOLVES the read-only checklist FROM the data map into `dsr_status` via [`Self::fan_out`] and
//!   moves the machine to `awaiting-holders`; the actual holder `erase` calls + receipt collection
//!   plus resumability plus the legal-hold gate are the P-GA-12 [`crate::fanout::FanOutDriver`],
//!   which drives [`crate::orchestration::UpstreamHolderOrchestrator`] and reads the request via
//!   [`Self::request_view`]).
//! - **Tenant-operability** (Art. 28 tenant-facing DSR + `EraseScope::Tenant` offboarding +
//!   restrict/rectify/portability surfaces) → **P-GA-13 → P-113**.
//! - **The durable deadline timer** (the `myelin-flow` minute-bucket wheel `sleep_until` + the
//!   nearing-deadline warning `Signal`) → **M2 P-GA-21 → P-148** (GA-D4). On THIS floor the
//!   deadline is a COARSE tracked timestamp (`submitted_at + 30 days`), computed via an
//!   injectable [`myelin_substrate::Clock`]; the durable wheel REPLACES the coarse tracking — the
//!   `deadline` field shape does not change.
//! - **The Merkle SEAL of the certificate receipts into the per-tenant audit tree** → **P-GA-20
//!   → P-119** (this prompt SIGNS the certificate bundle content-address; P-GA-20 anchors the
//!   root into the audit Merkle tree, making `dsr_certificate → MerkleProvenBundle` inclusion-
//!   provable).
//! - **The durable Postgres `dsr_request` (G1) / `dsr_receipt` (G2) tables** are the same DB
//!   floor every M0 in-memory store carries (P-007 / P-S12). On this floor the DSR register is an
//!   in-memory [`DsrRegister`] with byte-for-byte the §4.1 state-machine semantics.
//!
//! ## The state machine (the load-bearing correctness property — §4.1)
//! The transitions are **total + ordered**: the only legal path is
//! `Received → Validated → FannedOut → AwaitingHolders → Verified → Completed`, plus the two
//! terminal off-ramps `Validated → Refused` (the posture gate denied) and any state →
//! `Failed` (an upstream error). **`AwaitingHolders` cannot be skipped** — `Verified` is only
//! reachable FROM `AwaitingHolders` (you cannot declare a DSR verified before its holders have
//! been driven), which is exactly the "we marked it done without driving the holders" trap §4.1
//! forecloses. [`DsrState::can_transition_to`] is the single, total guard every transition runs
//! through; an illegal transition is a typed [`DsrError`], never a silent skip.
//!
//! ## Mutation floor (P-GA-11 TESTS — the state-machine transitions + the posture gate are
//! mandatory-core). `cargo mutants -p myelin-gdpr-service --file src/dsr.rs` (2026-06-20): 49
//! mutants, **38 caught, 10 unviable, 1 missed**. Every BEHAVIORAL mutant on the mandatory-core
//! paths is CAUGHT — [`DsrState::can_transition_to`] (the total guard; the `awaiting-holders`
//! unskippable edge), [`DsrOrchestrator::posture_gate_refuses`] (every conjunct: erasure ∧
//! processor ∧ Myelin-initiated ∧ ¬offboarding), the `now + 1 month` deadline (`+` not `*`),
//! [`DsrState::is_terminal`], the id-ordinal advance, and every transition fn
//! (`validate`/`fan_out`/`verify`/`complete`/`fail`). The 1 residual is documented non-core:
//! `<DsrError as Display>::fmt -> Ok(Default::default())` — the human-readable error MESSAGE
//! text. The error *variants* are mutation-killed (every `unwrap_err()` asserts the typed
//! [`DsrError`] by `PartialEq`); only the rendered string body is unkilled, which is cosmetic, not
//! behavior (exactly the audit module's `verify_chain`-wrapper-residual class). Stated, not hidden
//! (EI-01 §3).

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{DataRole, EraseScope, SubjectRef, TenantId};
use myelin_substrate::Clock;

use crate::datamap::Inventory;

// ───────────────────────── the DSR request kind (§4 / Arts. 15–20) ─────────────────────────

/// The kind of data-subject request (the Art. 15–20 rights the orchestrator answers). The
/// `kind` decides which holder operation the eventual fan-out drives (P-GA-12) — but the STATE
/// MACHINE + the posture gate are uniform across kinds (every kind validates posture, every kind
/// gets the coarse deadline). The posture gate's REFUSAL only bites [`DsrKind::Erasure`] of
/// tenant content (§1 — Myelin must not unilaterally erase tenant content); the read rights
/// (access/portability) always proceed even under the processor posture (gdpr §4.1 step 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DsrKind {
    /// Art. 15 — access (a `locate` + `export` read; never erases — always proceeds).
    Access,
    /// Art. 20 — portability (a structured `export`; never erases — always proceeds).
    Portability,
    /// Art. 16 — rectification (corrects the primary store + reindex-from-source; P-GA-13 body).
    Rectification,
    /// Art. 18/21 — restriction (a per-subject suppression flag; reversible; P-GA-13 body).
    Restriction,
    /// Art. 17 — erasure (the fan-out crypto-shred). The ONLY kind the posture gate can REFUSE
    /// (a Myelin-initiated erase of *tenant content* — §1).
    Erasure,
}

impl DsrKind {
    /// Whether this request, if granted, would ERASE personal data (only [`DsrKind::Erasure`]).
    /// The posture gate only refuses an *erase* of tenant content (§1); a read right is never
    /// refused on posture grounds (§4.1 step 3 — "access/portability still proceed").
    pub fn is_erasure(self) -> bool {
        matches!(self, DsrKind::Erasure)
    }
}

// ───────────────────────── the controller/processor posture (§1) ─────────────────────────

/// The legal posture the orchestrator validates a request under (gdpr §1.3). Encoded from the
/// schema-level `data_role` tag (§2.1), **not a runtime guess** — for tenant content Myelin is a
/// **processor** (the customer org is the controller); for platform-operational data Myelin is the
/// **controller** (the first-line DSR responder).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Posture {
    /// **Controller** — platform-operational data (tenant-admin contacts, billing, the security
    /// audit log, product telemetry). Myelin is the first-line DSR responder; a Myelin-initiated
    /// erase is permitted.
    Controller,
    /// **Processor** — tenant content (repos, issues, docs, chat, CI logs + embedded personal
    /// data of the customer's people). The customer org is the controller; a Myelin-initiated
    /// erase of this data is REFUSED unless tenant-instructed or an offboarding.
    Processor,
}

impl Posture {
    /// The posture implied by a field's `data_role` classification (the X-5 names anchor — the
    /// `data_role` tag IS the posture, §1). Tenant content ⇒ processor; platform-operational ⇒
    /// controller.
    pub fn from_data_role(role: DataRole) -> Posture {
        match role {
            DataRole::TenantContent => Posture::Processor,
            DataRole::PlatformOperational => Posture::Controller,
        }
    }
}

/// **Who initiated the request** — the input the posture gate needs to decide whether a
/// processor-posture erase is permitted (§1 / §4.4). A Myelin-initiated erase of tenant content
/// is REFUSED; the SAME erase is ADMITTED when it carries a tenant instruction or is an
/// offboarding (`EraseScope::Tenant`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initiator {
    /// Myelin-initiated (a platform operator / first-line controller DSR). Refused for a
    /// tenant-content erase (Myelin must not unilaterally erase tenant content — §1).
    Myelin,
    /// Tenant-instructed (Art. 28 assistance — the controller instructed the erase) OR a tenant
    /// offboarding. Admits a processor-posture erase (the controller authorised it).
    TenantInstructed,
}

// ───────────────────────── the state machine (§4.1) ─────────────────────────

/// **The DSR state machine (gdpr §4.1).** The states + the TOTAL, ORDERED transition guard. The
/// only legal happy-path sequence is
/// `Received → Validated → FannedOut → AwaitingHolders → Verified → Completed`; `Validated`
/// off-ramps to `Refused` (the posture gate denied), and any non-terminal state may move to
/// `Failed` (an upstream error). **`AwaitingHolders` is NOT skippable** — `Verified` is reachable
/// ONLY from `AwaitingHolders` (you cannot verify a DSR whose holders were never driven).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DsrState {
    /// The request arrived (the entry state — every `dsr_submit` lands here first).
    Received,
    /// Validated + posture decided (the §4.1 step 1). The fork point: a permitted request
    /// proceeds to `FannedOut`; a refused one off-ramps to `Refused`.
    Validated,
    /// The scope was resolved FROM the data map → a per-holder checklist (§4.1 step 2). The
    /// fan-out is queued; the machine moves to `AwaitingHolders`.
    FannedOut,
    /// Awaiting the per-holder receipts (§4.1 step 4 — the resumable fan-out, P-GA-12). The
    /// machine CANNOT leave this state for `Verified` until every checklist holder is receipted.
    AwaitingHolders,
    /// Every holder returned a receipt; the receipts were verified (§4.1 step 5).
    Verified,
    /// The DSR completion receipt was sealed; the certificate is exportable (§4.1 step 5 — the
    /// Merkle seal is P-GA-20). The terminal success state.
    Completed,
    /// The posture gate REFUSED the request (a Myelin-initiated erase of tenant content — §1).
    /// A terminal off-ramp from `Validated`. NOT a failure (the gate working as designed — a
    /// captured-expected denial).
    Refused,
    /// An upstream error halted the DSR (a holder fan-out error, P-GA-12). A terminal off-ramp
    /// from any non-terminal state; the resumable checklist (P-GA-12) re-drives from here.
    Failed,
}

impl DsrState {
    /// **The single, TOTAL transition guard (§4.1).** Returns whether `self → next` is a legal
    /// transition. Every state-machine move runs through this — an illegal transition is a typed
    /// error, never a silent skip. The legal edges:
    /// - `Received → Validated`
    /// - `Validated → {FannedOut, Refused}`
    /// - `FannedOut → AwaitingHolders`
    /// - `AwaitingHolders → Verified`  (the ONLY way into `Verified` — no skip)
    /// - `Verified → Completed`
    /// - any non-terminal → `Failed`
    ///
    /// Terminal states (`Completed`, `Refused`, `Failed`) have NO outgoing edges.
    pub fn can_transition_to(self, next: DsrState) -> bool {
        use DsrState::*;
        // Any non-terminal state may fail (an upstream error). Terminal states do not.
        if next == Failed {
            return !self.is_terminal();
        }
        // The legal happy-path + the Validated→Refused off-ramp edges (the `→ Failed` edges are
        // handled above). An exhaustive `matches!` so a new state can't silently gain an edge.
        matches!(
            (self, next),
            (Received, Validated)
                | (Validated, FannedOut)
                | (Validated, Refused)
                | (FannedOut, AwaitingHolders)
                | (AwaitingHolders, Verified)
                | (Verified, Completed)
        )
    }

    /// Whether this is a terminal state (no outgoing edges): `Completed` (success), `Refused`
    /// (the posture gate denied), or `Failed` (an upstream error).
    pub fn is_terminal(self) -> bool {
        matches!(self, DsrState::Completed | DsrState::Refused | DsrState::Failed)
    }

    /// The stable string form for the `dsr_state` telemetry signal (the §4.1 GATE — `dsr_state`
    /// transitions are observable). PII-free (a state name, never a subject).
    pub fn as_str(self) -> &'static str {
        match self {
            DsrState::Received => "received",
            DsrState::Validated => "validated",
            DsrState::FannedOut => "fanned-out",
            DsrState::AwaitingHolders => "awaiting-holders",
            DsrState::Verified => "verified",
            DsrState::Completed => "completed",
            DsrState::Refused => "refused",
            DsrState::Failed => "failed",
        }
    }
}

/// The `dsr_state` telemetry signal NAME + UNIT (gdpr §4.1 GATE — `dsr_state` transitions are
/// observable). PII-free: the value is a [`DsrState::as_str`] state name, never a subject. The
/// posture-refusal is a captured-expected denial on this signal (`refused`), distinct from
/// `failed` (an upstream error).
pub const DSR_STATE: (&str, &str) = ("gdpr.dsr_state", "state");

/// The statutory deadline window in seconds: **1 month = 30 days** (Art. 12(3); §4.1 —
/// `deadline = now + 1 month`). Extendable to 3 months for complex requests (a recorded reason —
/// P-GA-13). The durable timer that fires on this deadline is M2 (P-GA-21); here it is the coarse
/// tracked timestamp the [`Dsr::deadline_secs`] field carries.
pub const DSR_DEADLINE_SECS: u64 = 30 * 24 * 60 * 60;

// ───────────────────────── a DSR id (§8.1 — dsr_submit → dsr_id) ─────────────────────────

/// An opaque DSR id (the `dsr_submit → dsr_id` return; §8.1). PII-free: a monotonic per-register
/// ordinal rendered `dsr:<n>`, never derived from the subject. The caller passes it back to
/// `dsr_status` / `dsr_certificate`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DsrId(pub String);

impl DsrId {
    fn of(n: u64) -> DsrId {
        DsrId(format!("dsr:{n}"))
    }
}

// ───────────────────────── the per-holder checklist (read-only, resolved from the map) ─────

/// One resolved checklist line: a holder the fan-out (P-GA-12) WILL drive, with the per-field
/// erasure mechanisms resolved **FROM the data map** (§4.1 step 2 — the map, not a hand-written
/// list, drives the scope). On THIS prompt the checklist is RESOLVED into `dsr_status` (so an
/// operator can see exactly what the DSR will touch) but NOT yet driven — the actual holder
/// `erase` calls + the resumability are P-GA-12.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChecklistItem {
    /// The PII-free holder id (`<kind>:<name>`) the fan-out will address (contract 1.4).
    pub holder_id: String,
    /// The per-field erasure mechanisms resolved off the map (`<field_path>::<erasure>`), sorted.
    /// Empty for a holder that contributes no PII field but is still IN scope (driven for
    /// completeness — gdpr §2.2 "every registered holder is in the map").
    pub field_mechanisms: Vec<String>,
}

/// **Resolve the per-holder checklist FROM the generated data map (§4.1 step 2).** The map, not a
/// hand-written list, drives the scope — every holder in the map's roster becomes a checklist line
/// (even a zero-PII holder, driven for completeness), and each holder's per-field erasure
/// mechanisms are read off the map's entries. This is the READ-ONLY resolve the orchestrator
/// surfaces in `dsr_status`; the fan-out that DRIVES the checklist is P-GA-12.
pub fn resolve_checklist_from_map(inventory: &Inventory) -> Vec<ChecklistItem> {
    let mut by_holder: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // every holder in the roster is a checklist line (incl. zero-PII holders) — the map drives it.
    for holder_id in &inventory.holders {
        by_holder.entry(holder_id.clone()).or_default();
    }
    // each tagged field's per-field mechanism, resolved off the map (never an out-of-band list).
    for e in &inventory.entries {
        by_holder
            .entry(e.holder_id.clone())
            .or_default()
            .push(format!("{}::{}", e.field_path, e.erasure));
    }
    by_holder
        .into_iter()
        .map(|(holder_id, mut field_mechanisms)| {
            field_mechanisms.sort();
            ChecklistItem { holder_id, field_mechanisms }
        })
        .collect()
}

// ───────────────────────── the DSR record (the G1 dsr_request row) ─────────────────────────

/// One DSR's full state (the in-memory model of the G1 `dsr_request` row — the durable Postgres
/// table is a named floor). Carries the request inputs (kind / subject / scope / posture /
/// initiator), the current [`DsrState`], the COARSE deadline (`submitted_at + 1 month`), the
/// resolved checklist (read-only here; driven in P-GA-12), and the collected receipts (the
/// certificate input; the Merkle seal is P-GA-20).
#[derive(Clone, Debug)]
pub struct Dsr {
    /// The opaque DSR id (`dsr:<n>`).
    pub id: DsrId,
    /// The Art. 15–20 right requested.
    pub kind: DsrKind,
    /// The tenant the request runs under (the partition key; PII-free token).
    pub tenant: TenantId,
    /// The subject (a verified [`SubjectRef`] — an opaque `principal_id`, never a name/email).
    pub subject: SubjectRef,
    /// The erase scope (subject-within-tenant, or a whole-tenant offboarding).
    pub scope: EraseScope,
    /// The legal posture the request validated under (§1).
    pub posture: Posture,
    /// Who initiated it (the posture-gate input).
    pub initiator: Initiator,
    /// The current state-machine state.
    pub state: DsrState,
    /// The wall-clock second the request was submitted (the deadline base).
    pub submitted_at_secs: u64,
    /// **The statutory deadline (§4.1 — `now + 1 month`), COARSE-tracked here.** The durable
    /// timer that fires on it is M2 (P-GA-21); the field shape does not change.
    pub deadline_secs: u64,
    /// The per-holder checklist resolved FROM the data map (§4.1 step 2). Empty until the
    /// `FannedOut` transition resolves it; read-only here (driven in P-GA-12).
    pub checklist: Vec<ChecklistItem>,
    /// The signed per-holder receipts collected so far (the certificate input). Collected by the
    /// fan-out (P-GA-12); the Merkle seal is P-GA-20.
    pub receipts: Vec<String>,
}

impl Dsr {
    /// The status projection the `dsr_status` contract returns (`{state, deadline, checklist}`;
    /// §8.1). A read-only view; never mutates.
    pub fn status(&self) -> DsrStatus {
        DsrStatus {
            state: self.state,
            deadline_secs: self.deadline_secs,
            checklist: self.checklist.clone(),
        }
    }
}

/// The `dsr_status(dsr_id) → {state, deadline, checklist}` return (§8.1 / contract 10.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrStatus {
    /// The current state-machine state.
    pub state: DsrState,
    /// The statutory deadline (`submitted_at + 1 month`), coarse-tracked.
    pub deadline_secs: u64,
    /// The per-holder checklist resolved from the data map (read-only here).
    pub checklist: Vec<ChecklistItem>,
}

// ───────────────────────── the certificate (dsr_certificate → MerkleProvenBundle) ─────────

/// The `dsr_certificate(dsr_id) → MerkleProvenBundle` return (§8.1 / contract 10.4). On THIS
/// prompt it carries the signed per-holder receipts + a content-addressed bundle digest; the
/// **Merkle inclusion proof** that makes it `MerkleProven` (the seal into the per-tenant audit
/// tree) is wired in **P-GA-20 → P-119** — the field [`merkle_inclusion`] is `None` until then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerkleProvenBundle {
    /// The DSR the certificate is for.
    pub dsr_id: DsrId,
    /// The collected per-holder receipts (the certificate's verifiable body — each receipt is
    /// content-addressed + signed, P-GA-12).
    pub receipts: Vec<String>,
    /// The content-addressed digest over the whole bundle (`blake3:<hex>` of the canonical form).
    /// The Merkle leaf P-GA-20 seals.
    pub bundle_digest: String,
    /// The Merkle inclusion proof anchoring the bundle into the per-tenant audit tree. `None` on
    /// this floor (the seal is P-GA-20 → P-119); the FIELD is frozen so the consumer compiles.
    pub merkle_inclusion: Option<String>,
}

impl MerkleProvenBundle {
    /// Content-address the bundle: a deterministic `blake3:<hex>` digest over the DSR id + the
    /// ordered receipts (the Merkle leaf P-GA-20 will seal). Deterministic: the SAME (id,
    /// receipts) always content-addresses the same.
    fn content_addressed(dsr_id: &DsrId, receipts: &[String]) -> MerkleProvenBundle {
        let mut preimage = dsr_id.0.clone();
        for r in receipts {
            preimage.push('\u{1f}'); // unit separator — the ∥ in receipt = hash(.. ∥ ..)
            preimage.push_str(r);
        }
        let digest = blake3::hash(preimage.as_bytes());
        MerkleProvenBundle {
            dsr_id: dsr_id.clone(),
            receipts: receipts.to_vec(),
            bundle_digest: format!("blake3:{}", hex::encode(digest.as_bytes())),
            merkle_inclusion: None, // sealed in P-GA-20.
        }
    }
}

// ───────────────────────── typed errors (loud, never swallowed) ─────────────────────────

/// A DSR orchestrator error (EI-01 §3 — make violations loud). The posture REFUSAL is NOT an
/// error (it is a legal terminal state, [`DsrState::Refused`]); these are genuine faults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DsrError {
    /// No DSR with this id in the register (a stale / forged id).
    UnknownDsr(DsrId),
    /// An illegal state-machine transition was attempted (§4.1 — the transition guard rejected
    /// it; e.g. skipping `awaiting-holders`). Carries the (from, to) the guard refused.
    IllegalTransition { from: DsrState, to: DsrState },
    /// `dsr_certificate` was requested for a DSR that has not reached a state where a certificate
    /// exists (`Verified` / `Completed`). Carries the current state.
    CertificateNotReady(DsrState),
    /// A per-holder fan-out (P-GA-12, [`crate::fanout`]) errored — a holder's `erase` returned a
    /// fault (a holder unavailable / a key-destruction error). Carries the holder error message.
    /// The fan-out leaves a RESUMABLE checklist (the receipted holders are recorded), so re-driving
    /// the DSR re-drives only the failed-onward holders (§4.1 step 4).
    HolderFanOut(String),
}

impl std::fmt::Display for DsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DsrError::UnknownDsr(id) => write!(f, "no DSR with id `{}`", id.0),
            DsrError::IllegalTransition { from, to } => write!(
                f,
                "illegal DSR transition {} → {} (§4.1 — the state machine is total + ordered; \
                 awaiting-holders cannot be skipped)",
                from.as_str(),
                to.as_str()
            ),
            DsrError::CertificateNotReady(state) => write!(
                f,
                "dsr_certificate not ready: DSR is `{}` (a certificate exists only once verified)",
                state.as_str()
            ),
            DsrError::HolderFanOut(msg) => write!(
                f,
                "holder fan-out errored: {msg} (§4.1 step 4 — the checklist is resumable; re-drive \
                 to resume from the failed holder)"
            ),
        }
    }
}

impl std::error::Error for DsrError {}

/// The orchestrator's result type.
pub type Result<T> = std::result::Result<T, DsrError>;

// ───────────────────────── the orchestrator (the DSR API, contract 10.4) ─────────────────────

/// **The DSR orchestrator (contract 10.4).** Holds the in-memory DSR register (the G1
/// `dsr_request` rows on this floor — the durable Postgres table is a named floor) and exposes
/// the three API entry points (`dsr_submit` / `dsr_status` / `dsr_certificate`). It decides the
/// posture, runs the total + ordered state machine, sets the coarse deadline, and resolves the
/// read-only checklist FROM the data map. It NEVER reaches into a store (it consumes only the
/// generated [`Inventory`] + the holder contract via P-GA-12's fan-out) — the no-cross-store-read
/// law (§3.1) holds structurally.
///
/// The clock is injectable ([`myelin_substrate::Clock`]) so the deadline (`now + 1 month`) is
/// testable deterministically (the M2 durable wheel, P-GA-21, replaces the coarse tracking
/// without changing the `deadline` shape).
pub struct DsrOrchestrator<C: Clock> {
    clock: C,
    register: Mutex<DsrRegister>,
}

/// The in-memory DSR register (the G1 `dsr_request` table model). The durable Postgres table is
/// a named floor (P-007 / P-S12); the resumability + state-machine semantics are byte-for-byte.
#[derive(Default)]
struct DsrRegister {
    next: u64,
    dsrs: BTreeMap<DsrId, Dsr>,
}

impl<C: Clock> DsrOrchestrator<C> {
    /// Build an orchestrator over an injectable clock (the deadline base). Production wires
    /// [`myelin_substrate::SystemClock`]; the drills wire [`myelin_substrate::TestClock`].
    pub fn new(clock: C) -> DsrOrchestrator<C> {
        DsrOrchestrator { clock, register: Mutex::new(DsrRegister::default()) }
    }

    /// **`dsr_submit(kind, subject, scope, posture) → dsr_id` (§8.1 / contract 10.4).** Records a
    /// new DSR in the `Received` state, sets the COARSE statutory deadline (`now + 1 month`,
    /// §4.1), and returns the opaque id. The state machine is NOT advanced here — the caller (or
    /// the eventual durable workflow, P-GA-12/P-GA-21) drives it via [`Self::validate`] etc. The
    /// `initiator` is the posture-gate input ([`Self::validate`] reads it).
    pub fn dsr_submit(
        &self,
        kind: DsrKind,
        tenant: TenantId,
        subject: SubjectRef,
        scope: EraseScope,
        posture: Posture,
        initiator: Initiator,
    ) -> DsrId {
        let now = self.clock.now_secs();
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let id = DsrId::of(reg.next);
        reg.next += 1;
        let dsr = Dsr {
            id: id.clone(),
            kind,
            tenant,
            subject,
            scope,
            posture,
            initiator,
            state: DsrState::Received,
            submitted_at_secs: now,
            // §4.1 — the deadline is set to now + 1 month ON SUBMIT (coarse here; durable timer
            // is M2 P-GA-21). The field shape does not change when the wheel lands.
            deadline_secs: now + DSR_DEADLINE_SECS,
            checklist: Vec::new(),
            receipts: Vec::new(),
        };
        reg.dsrs.insert(id.clone(), dsr);
        id
    }

    /// **`dsr_status(dsr_id) → {state, deadline, checklist}` (§8.1 / contract 10.4).** The
    /// read-only status projection. Errors on an unknown id (a stale / forged id is loud, never a
    /// silent empty).
    pub fn dsr_status(&self, id: &DsrId) -> Result<DsrStatus> {
        let reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        reg.dsrs
            .get(id)
            .map(Dsr::status)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))
    }

    /// **`dsr_certificate(dsr_id) → MerkleProvenBundle` (§8.1 / contract 10.4).** Builds the
    /// content-addressed, signed certificate bundle over the collected receipts. Errors if the
    /// DSR has not reached a state where a certificate exists (`Verified` / `Completed`) — a
    /// certificate of an un-driven DSR would be a false proof. The **Merkle inclusion** that
    /// makes it `MerkleProven` is wired in P-GA-20 (the `merkle_inclusion` field is `None` here).
    pub fn dsr_certificate(&self, id: &DsrId) -> Result<MerkleProvenBundle> {
        let reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg.dsrs.get(id).ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        if !matches!(dsr.state, DsrState::Verified | DsrState::Completed) {
            return Err(DsrError::CertificateNotReady(dsr.state));
        }
        Ok(MerkleProvenBundle::content_addressed(id, &dsr.receipts))
    }

    // ─────────────── the state-machine transitions (§4.1, total + ordered) ───────────────

    /// **§4.1 step 1 — validate + decide the posture gate.** Moves `Received → Validated`, then:
    /// - if the request is a **Myelin-initiated erase of tenant content** (the processor
    ///   posture), it is **REFUSED** (§1 — Myelin must not unilaterally erase tenant content),
    ///   moving `Validated → Refused`. The function returns `Ok(false)` (the gate working — NOT
    ///   an error; a captured-expected denial).
    /// - otherwise the request is admitted (the controller posture, a read right, or a
    ///   tenant-instructed / offboarding erase), staying at `Validated`. Returns `Ok(true)`.
    ///
    /// **The posture gate (§1):** an erase is refused IFF `kind == Erasure` AND
    /// `posture == Processor` AND `initiator == Myelin` AND the scope is NOT a whole-tenant
    /// offboarding (`EraseScope::Tenant` is an authorised offboarding). A tenant-instructed erase
    /// is ADMITTED (the controller authorised it via Art. 28).
    pub fn validate(&self, id: &DsrId) -> Result<bool> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg.dsrs.get_mut(id).ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::Validated)?;
        if Self::posture_gate_refuses(dsr) {
            transition(dsr, DsrState::Refused)?;
            return Ok(false);
        }
        Ok(true)
    }

    /// The posture-gate predicate (§1 — pure, no state mutation). A Myelin-initiated erase of
    /// *tenant content* (the processor posture) is refused, UNLESS it is a whole-tenant
    /// offboarding (`EraseScope::Tenant`, which IS an authorised erase — §4.4). A read right
    /// (access/portability) or a controller-posture erase or a tenant-instructed erase is never
    /// refused.
    fn posture_gate_refuses(dsr: &Dsr) -> bool {
        dsr.kind.is_erasure()
            && dsr.posture == Posture::Processor
            && dsr.initiator == Initiator::Myelin
            && !matches!(dsr.scope, EraseScope::Tenant(_))
    }

    /// **§4.1 step 2 — resolve the scope FROM the data map → the per-holder checklist.** Moves
    /// `Validated → FannedOut → AwaitingHolders` and records the read-only checklist resolved
    /// from the generated [`Inventory`] (the map, not a hand-written list, drives the scope). The
    /// actual fan-out (driving the holders + collecting receipts) is P-GA-12 — this transition
    /// only RESOLVES + QUEUES it (the machine parks at `AwaitingHolders`).
    pub fn fan_out(&self, id: &DsrId, inventory: &Inventory) -> Result<Vec<ChecklistItem>> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg.dsrs.get_mut(id).ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::FannedOut)?;
        dsr.checklist = resolve_checklist_from_map(inventory);
        // §4.1 — fanned-out THEN awaiting-holders; never skip awaiting-holders.
        transition(dsr, DsrState::AwaitingHolders)?;
        Ok(dsr.checklist.clone())
    }

    /// **§4.1 step 5 — record the verified per-holder receipts.** Moves
    /// `AwaitingHolders → Verified` (the ONLY way into `Verified` — no skip). The receipts come
    /// from the P-GA-12 fan-out; here they are recorded for the certificate. A DSR cannot reach
    /// `Verified` without having passed through `AwaitingHolders` (the transition guard enforces
    /// it).
    pub fn verify(&self, id: &DsrId, receipts: Vec<String>) -> Result<()> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg.dsrs.get_mut(id).ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::Verified)?;
        dsr.receipts = receipts;
        Ok(())
    }

    /// **§4.1 — seal the DSR completion (the terminal success).** Moves `Verified → Completed`.
    /// The Merkle seal of the receipts into the per-tenant audit tree is P-GA-20; here this marks
    /// the DSR done so `dsr_certificate` is final.
    pub fn complete(&self, id: &DsrId) -> Result<()> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg.dsrs.get_mut(id).ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::Completed)
    }

    /// Mark a DSR `Failed` (an upstream error — a holder fan-out error, P-GA-12). Legal from any
    /// non-terminal state; the resumable checklist re-drives from here (P-GA-12).
    pub fn fail(&self, id: &DsrId) -> Result<()> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg.dsrs.get_mut(id).ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::Failed)
    }

    /// The current state of a DSR (for telemetry / tests). Errors on an unknown id.
    pub fn state_of(&self, id: &DsrId) -> Result<DsrState> {
        let reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        reg.dsrs.get(id).map(|d| d.state).ok_or_else(|| DsrError::UnknownDsr(id.clone()))
    }

    /// **The PII-free request view the P-GA-12 fan-out driver reads** (the request inputs the
    /// driver needs to drive the erase: the kind, the scope, the posture, and the deadline base).
    /// A read-only snapshot — never mutates. Errors on an unknown id. The driver
    /// ([`crate::fanout`]) consumes this to decide WHAT to fan out (the scope) and HOW (the kind);
    /// the orchestrator itself never reaches into a store (the no-cross-store-read law, §3.1).
    pub fn request_view(&self, id: &DsrId) -> Result<DsrRequestView> {
        let reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg.dsrs.get(id).ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        Ok(DsrRequestView {
            id: dsr.id.clone(),
            kind: dsr.kind,
            tenant: dsr.tenant.clone(),
            scope: dsr.scope.clone(),
            posture: dsr.posture,
            initiator: dsr.initiator,
            state: dsr.state,
            submitted_at_secs: dsr.submitted_at_secs,
        })
    }
}

/// A read-only, PII-free view of a DSR's request inputs (the P-GA-12 fan-out driver reads it to
/// decide WHAT to fan out + HOW). Carries only opaque ids + enum tags — never a name/email (the
/// [`SubjectRef`] inside `scope` holds the opaque `principal_id`, never PII). The driver
/// ([`crate::fanout`]) consumes this; the orchestrator hands it out so the driver never has to
/// reach into the private register.
#[derive(Clone, Debug)]
pub struct DsrRequestView {
    /// The opaque DSR id.
    pub id: DsrId,
    /// The Art. 15–20 right requested.
    pub kind: DsrKind,
    /// The tenant the request runs under (the partition key).
    pub tenant: TenantId,
    /// The erase scope (subject-within-tenant, or a whole-tenant offboarding) the fan-out drives.
    pub scope: EraseScope,
    /// The legal posture the request validated under (§1).
    pub posture: Posture,
    /// Who initiated it.
    pub initiator: Initiator,
    /// The current state-machine state.
    pub state: DsrState,
    /// The wall-clock second the request was submitted (the receipt timestamp base).
    pub submitted_at_secs: u64,
}

/// Run ONE state-machine transition through the total guard (§4.1). The single chokepoint every
/// transition passes through — an illegal transition is a typed [`DsrError::IllegalTransition`],
/// never a silent skip. Mutates the DSR's state IFF the transition is legal.
fn transition(dsr: &mut Dsr, to: DsrState) -> Result<()> {
    if !dsr.state.can_transition_to(to) {
        return Err(DsrError::IllegalTransition { from: dsr.state, to });
    }
    dsr.state = to;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::TestClock;

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject { subject: subject(s), tenant: tenant() }
    }

    fn orch_at(t0: u64) -> DsrOrchestrator<TestClock> {
        DsrOrchestrator::new(TestClock::at(t0))
    }

    // ───────────── the state machine is total + ordered (§4.1) ─────────────

    #[test]
    fn happy_path_is_received_validated_fannedout_awaiting_verified_completed() {
        let o = orch_at(1000);
        let id = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Received);
        assert!(o.validate(&id).unwrap(), "controller access is admitted");
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Validated);
        o.fan_out(&id, &Inventory::default()).unwrap();
        assert_eq!(o.state_of(&id).unwrap(), DsrState::AwaitingHolders);
        o.verify(&id, vec!["receipt-1".into()]).unwrap();
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Verified);
        o.complete(&id).unwrap();
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Completed);
    }

    #[test]
    fn awaiting_holders_cannot_be_skipped_verified_only_reachable_from_awaiting() {
        // The load-bearing §4.1 property: you cannot mark a DSR verified before its holders are
        // driven. Verified is reachable ONLY from AwaitingHolders.
        assert!(DsrState::AwaitingHolders.can_transition_to(DsrState::Verified));
        assert!(!DsrState::FannedOut.can_transition_to(DsrState::Verified));
        assert!(!DsrState::Validated.can_transition_to(DsrState::Verified));
        assert!(!DsrState::Received.can_transition_to(DsrState::Verified));
        // and you cannot jump received → completed.
        assert!(!DsrState::Received.can_transition_to(DsrState::Completed));
    }

    #[test]
    fn transition_guard_is_total_terminal_states_have_no_outgoing_edges() {
        use DsrState::*;
        let all = [
            Received, Validated, FannedOut, AwaitingHolders, Verified, Completed, Refused, Failed,
        ];
        // Terminal states have ZERO outgoing edges (incl. Failed).
        for &t in &[Completed, Refused, Failed] {
            assert!(t.is_terminal());
            for &n in &all {
                assert!(!t.can_transition_to(n), "{} → {} must be illegal", t.as_str(), n.as_str());
            }
        }
        // Every non-terminal is NOT terminal + CAN fail; every terminal IS terminal + canNOT.
        for &s in &[Received, Validated, FannedOut, AwaitingHolders, Verified] {
            assert!(!s.is_terminal(), "{} is non-terminal", s.as_str());
            assert!(s.can_transition_to(Failed), "{} can fail", s.as_str());
        }
        // the as_str telemetry form is the stable §4.1 state name (pinned — the dsr_state signal).
        assert_eq!(Received.as_str(), "received");
        assert_eq!(AwaitingHolders.as_str(), "awaiting-holders");
        assert_eq!(Refused.as_str(), "refused");
        assert_eq!(Completed.as_str(), "completed");
        assert_eq!(Failed.as_str(), "failed");
    }

    #[test]
    fn fail_is_a_legal_terminal_off_ramp_from_any_non_terminal_state() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        o.validate(&id).unwrap();
        o.fan_out(&id, &Inventory::default()).unwrap();
        // an upstream holder error fails the DSR (P-GA-12 re-drives from here).
        o.fail(&id).unwrap();
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Failed);
        // Failed is terminal — nothing further is legal.
        assert!(o.complete(&id).is_err());
    }

    #[test]
    fn submitted_dsr_ids_are_distinct_monotonic_ordinals() {
        let o = orch_at(0);
        let a = o.dsr_submit(
            DsrKind::Access, tenant(), subject("p1"), subject_scope("p1"),
            Posture::Controller, Initiator::Myelin,
        );
        let b = o.dsr_submit(
            DsrKind::Access, tenant(), subject("p2"), subject_scope("p2"),
            Posture::Controller, Initiator::Myelin,
        );
        assert_ne!(a, b, "each submit mints a distinct id (the ordinal advances)");
        assert_eq!(a, DsrId("dsr:0".into()));
        assert_eq!(b, DsrId("dsr:1".into()));
    }

    #[test]
    fn an_illegal_transition_is_a_loud_typed_error_never_a_silent_skip() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        // Received → Verified (skipping validate/fan-out) is rejected.
        let err = o.verify(&id, vec![]).unwrap_err();
        assert_eq!(
            err,
            DsrError::IllegalTransition { from: DsrState::Received, to: DsrState::Verified }
        );
        // the DSR did NOT silently advance.
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Received);
    }

    // ───────────── the controller/processor posture gate (§1) ─────────────

    #[test]
    fn posture_gate_refuses_a_myelin_initiated_erase_of_tenant_content() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Processor, // tenant content
            Initiator::Myelin,  // Myelin-initiated
        );
        assert!(!o.validate(&id).unwrap(), "the posture gate REFUSES it");
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Refused);
        // Refused is terminal — no fan-out can run.
        assert!(o.fan_out(&id, &Inventory::default()).is_err());
    }

    #[test]
    fn posture_gate_admits_a_tenant_instructed_erase_of_tenant_content() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Processor,          // tenant content
            Initiator::TenantInstructed, // the controller (the tenant) authorised it (Art. 28)
        );
        assert!(o.validate(&id).unwrap(), "a tenant-instructed erase is ADMITTED");
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Validated);
    }

    #[test]
    fn posture_gate_admits_a_tenant_offboarding_erase() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            EraseScope::Tenant(tenant()), // a whole-tenant offboarding (§4.4 — authorised)
            Posture::Processor,
            Initiator::Myelin, // even Myelin-initiated: offboarding IS authorised
        );
        assert!(o.validate(&id).unwrap(), "a tenant offboarding is an authorised erase");
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Validated);
    }

    #[test]
    fn posture_gate_admits_a_controller_posture_erase() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller, // platform-operational data — Myelin is the controller
            Initiator::Myelin,
        );
        assert!(o.validate(&id).unwrap(), "a controller-posture erase is admitted");
    }

    #[test]
    fn posture_gate_never_refuses_a_read_right_even_under_the_processor_posture() {
        // §4.1 step 3 — access/portability still proceed even when erasure would be refused.
        for kind in [DsrKind::Access, DsrKind::Portability] {
            let o = orch_at(0);
            let id = o.dsr_submit(
                kind,
                tenant(),
                subject("p1"),
                subject_scope("p1"),
                Posture::Processor,
                Initiator::Myelin,
            );
            assert!(o.validate(&id).unwrap(), "{kind:?} proceeds under the processor posture");
        }
    }

    #[test]
    fn posture_from_data_role_is_the_x5_anchor() {
        assert_eq!(Posture::from_data_role(DataRole::TenantContent), Posture::Processor);
        assert_eq!(Posture::from_data_role(DataRole::PlatformOperational), Posture::Controller);
    }

    // ───────────── the deadline is set coarse on submit (§4.1) ─────────────

    #[test]
    fn dsr_submit_sets_the_deadline_to_now_plus_one_month() {
        let o = orch_at(1_700_000_000);
        let id = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        let status = o.dsr_status(&id).unwrap();
        assert_eq!(status.deadline_secs, 1_700_000_000 + DSR_DEADLINE_SECS);
        assert_eq!(status.deadline_secs - 1_700_000_000, 30 * 24 * 60 * 60);
    }

    // ───────────── the checklist is resolved FROM the data map (§4.1 step 2) ─────────────

    #[test]
    fn fan_out_resolves_the_checklist_from_the_map_not_a_hardcoded_list() {
        use crate::datamap::{Inventory, InventoryEntry};
        use std::collections::BTreeSet;

        let mut holders = BTreeSet::new();
        holders.insert("oltp:identity_oltp".to_string());
        holders.insert("search_index:search_index".to_string()); // a zero-PII holder, still driven
        let inv = Inventory {
            entries: vec![InventoryEntry {
                field_path: "PrincipalRow.email".into(),
                holder_id: "oltp:identity_oltp".into(),
                holder: "H15".into(),
                region: "fr-par".into(),
                category: "ContactInfo".into(),
                role: "PlatformOperational".into(),
                basis: "Contract".into(),
                retention: "UntilContractEnd".into(),
                erasure: "CryptoShred(subject_dek)".into(),
                subject_locator: "principal_id".into(),
            }],
            holders,
            dpia_markers: BTreeSet::new(),
        };

        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        o.validate(&id).unwrap();
        let checklist = o.fan_out(&id, &inv).unwrap();

        // every holder in the map's roster is a checklist line (incl. the zero-PII one).
        assert_eq!(checklist.len(), 2);
        let ids: Vec<&str> = checklist.iter().map(|c| c.holder_id.as_str()).collect();
        assert!(ids.contains(&"oltp:identity_oltp"));
        assert!(ids.contains(&"search_index:search_index"));
        // the identity holder's per-field mechanism is resolved OFF the map.
        let identity = checklist.iter().find(|c| c.holder_id == "oltp:identity_oltp").unwrap();
        assert_eq!(identity.field_mechanisms, vec!["PrincipalRow.email::CryptoShred(subject_dek)"]);
        // the status surfaces the same checklist.
        assert_eq!(o.dsr_status(&id).unwrap().checklist, checklist);
    }

    // ───────────── dsr_certificate (§8.1) ─────────────

    #[test]
    fn dsr_certificate_is_content_addressed_and_not_ready_before_verified() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        // not ready before verified.
        assert_eq!(
            o.dsr_certificate(&id).unwrap_err(),
            DsrError::CertificateNotReady(DsrState::Received)
        );
        o.validate(&id).unwrap();
        o.fan_out(&id, &Inventory::default()).unwrap();
        o.verify(&id, vec!["receipt-a".into(), "receipt-b".into()]).unwrap();
        let cert = o.dsr_certificate(&id).unwrap();
        assert_eq!(cert.dsr_id, id);
        assert_eq!(cert.receipts, vec!["receipt-a".to_string(), "receipt-b".to_string()]);
        assert!(cert.bundle_digest.starts_with("blake3:"));
        // the Merkle seal is a named floor (P-GA-20) — None here.
        assert!(cert.merkle_inclusion.is_none(), "the Merkle seal is P-GA-20");
        // deterministic: the same (id, receipts) content-addresses the same.
        let cert2 = o.dsr_certificate(&id).unwrap();
        assert_eq!(cert.bundle_digest, cert2.bundle_digest);
    }

    #[test]
    fn unknown_dsr_id_is_a_loud_error_never_a_silent_empty() {
        let o = orch_at(0);
        let ghost = DsrId("dsr:999".into());
        assert_eq!(o.dsr_status(&ghost).unwrap_err(), DsrError::UnknownDsr(ghost.clone()));
        assert_eq!(o.dsr_certificate(&ghost).unwrap_err(), DsrError::UnknownDsr(ghost));
    }
}
