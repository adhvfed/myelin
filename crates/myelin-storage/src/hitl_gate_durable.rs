//! # The durable HITL gate VERDICT store over the `agent_hitl_gate` table (R2.4 / agent-fabric §4.4)
//!
//! **Owning architecture doc:** `agent-fabric.md` §4.4 (the `hitl_gate` table: `gate_id`, `run_id`,
//! `effect_id`, `risk_summary`, `cost_estimate`, `approver_filter`, `state`, `card_ref`) + §5.3 (the
//! withhold → surface → resume loop). The 2026-07-06 review's HIGH finding: HITL approval was a
//! CALLER-SUPPLIED boolean on the MCP `tools/call` body, and the gate id was a deterministic display
//! string never stored or looked up. **This module is the server-side verdict authority that closes
//! it**: a gated tool/effect INSERTs a `waiting` row here; approve/reject/expire UPDATE the row's
//! state (both approve and reject enforce distinct-HUMAN eligibility SERVER-SIDE); and a re-drive is
//! admitted ONLY if the presented `gate_id` is `approved` in THIS store — lookup-able across
//! requests AND processes (the durable arm). The caller's `approval.granted` boolean is no longer
//! an enforcement input anywhere.
//!
//! ## The role-struct + backing shape (MR-009b convention — durable-by-default)
//! [`HitlVerdictStore`] is the role struct; its backend enum has the always-compiled production arm
//! [`DurableHitlGates`] (PG over `agent_hitl_gate`, RLS-scoped through the MR-022
//! [`SubstrateProvider::with_tenant_tx`] convention) and a `test-support`-gated in-memory TEST DOUBLE
//! (`Memory` — `#[cfg(any(test, feature = "test-support"))]`, stripped by the
//! `no-in-memory-durable-store` scanner). Same shape as `reserve_settle::CostLedger` / the W3b outbox.
//!
//! ## The distinct-HUMAN-approver rule (enforced server-side, twice)
//! 1. **At decide time** ([`HitlVerdictStore::approve`] and [`HitlVerdictStore::reject`]): the
//!    decider must be a **`Human`
//!    principal** (R2.4b — a machine/agent/service principal is refused even if it sits in the
//!    filter, closing the machine-collusion gap), must be a member of the gate's `approver_filter`,
//!    AND must differ from the gate's `requested_by` (the agent principal that tripped the gate). A
//!    non-human, self-, or out-of-filter approval is a typed refusal — the row stays `waiting`.
//! 2. **At consult time** ([`GateRecord::authorizes`]): the gate admits the re-drive ONLY if it is
//!    `approved`, its `effect_id` matches the effect being re-driven (an approval is bound to ONE
//!    effect, never a tool name), and the recorded `decided_by` differs from the requesting
//!    principal. (An `approved` row can only have been reached through the human-gated `approve`
//!    above, so a machine approver can never have produced one.)
//!
//! ## The boot-migration gap this closes (R2.4 grounding)
//! `myelin-agent-service::migrations` DECLAREs the §4.4 `agent_hitl_gate` shape (migration id
//! `0004_create_agent_hitl_gate`) but that group was NEVER in [`crate::provider::all_durable_migrations`]
//! (the W7.2 boot aggregate spans the storage-owned groups `0010`–`0053` only, and storage cannot
//! depend on the agent-service crate). So nothing boot-applied the table — the declared schema was
//! dead. [`hitl_gate_durable_migrations`] (ids `0054`-`0055`) is the EXECUTED boot declaration, now in
//! [`crate::provider::durable_migration_groups`]; the agent-service crate (which depends on this one)
//! carries a parity test asserting its §4.4 model DDL columns are a subset of THIS boot DDL, so the
//! two declarations cannot drift silently. The boot DDL adds two audit/enforcement columns the §4.4
//! field list did not carry (`requested_by`, `decided_by`) — required to enforce the distinct-approver
//! rule across processes.

use crate::migration::{Migration, Migrations};
use crate::rls::TenantScope;
use myelin_identity::PrincipalKind;
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;

/// Default lifetime for a surfaced HITL approval window.
pub const DEFAULT_HITL_GATE_TTL_SECS: i64 = 3600;

// =================================================================================================
// Migration 0054 — the tenant-owned (FORCE-RLS) agent_hitl_gate table (the §4.4 shape, executed).
// =================================================================================================

/// The `agent_hitl_gate` table (agent-fabric §4.4) + its FORCE-RLS `(tenant, region)` policy. The
/// §4.4 field list (`gate_id, run_id, effect_id, risk_summary, cost_estimate, approver_filter,
/// state, card_ref`) plus the two R2.4 enforcement columns (`requested_by`, `decided_by`). The id
/// columns are `text` (the code-side `GateId`/run ids are opaque strings; an OPAQUE random gate id
/// is the whole point — never a guessable display string). `risk_summary bytea` stays the
/// per-subject-DEK-encrypted humanised-slot carrier (11.4); `cost_estimate bigint` is integer
/// minor-units. Forward-only (`IF NOT EXISTS` / `DROP POLICY IF EXISTS` — idempotent).
pub const AGENT_HITL_GATE_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS agent_hitl_gate (
    tenant_id       text   NOT NULL,
    region          text   NOT NULL,
    gate_id         text   NOT NULL,
    run_id          text   NOT NULL,
    effect_id       text   NOT NULL,
    risk_summary    bytea,
    cost_estimate   bigint NOT NULL,
    approver_filter text[] NOT NULL,
    state           text   NOT NULL,
    card_ref        text,
    requested_by    text   NOT NULL,
    decided_by      text,
    PRIMARY KEY (tenant_id, region, gate_id)
);
ALTER TABLE agent_hitl_gate ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_hitl_gate FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_hitl_gate;
CREATE POLICY myelin_tenant_isolation ON agent_hitl_gate \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
              WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

/// Forward-only expansion adding bounded approval lifetime and one-shot consumption. This is a
/// separate migration because `0054_agent_hitl_gate` may already be recorded in production and its
/// checksum must never be rewritten.
pub const AGENT_HITL_GATE_LIFETIME_MIGRATION: &str = "\
ALTER TABLE agent_hitl_gate
  ADD COLUMN IF NOT EXISTS opened_at_unix bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS decided_at_unix bigint,
  ADD COLUMN IF NOT EXISTS expires_at_unix bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS approval_consumed_at_unix bigint;";

/// The forward-only migration set the durable HITL verdict store binds to (`0054` plus the
/// forward-only lifetime/consumption extension in `0055`, appended after the `0053` bus-erasure
/// group). In [`crate::provider::durable_migration_groups`] → boot-applied by
/// `all_durable_migrations()` at every service main (W7.2 sequence); idempotent on re-boot.
pub fn hitl_gate_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0054_agent_hitl_gate", AGENT_HITL_GATE_MIGRATION),
        Migration::plain(
            "0055_agent_hitl_gate_lifetime",
            AGENT_HITL_GATE_LIFETIME_MIGRATION,
        ),
    ])
}

/// **Mint an OPAQUE, unguessable gate id (R2.4 / R2.4b NIT).** 128 bits from the OS CSPRNG
/// (`aes_gcm::aead::OsRng` — the SAME vetted entropy source `kms.rs` uses for key/nonce material),
/// hex-encoded as `gate:<32 hex>`. The gate id is the verdict-store PK the MCP layer returns on a
/// withhold; the caller PRESENTS it to re-drive.
///
/// **Defense-in-depth only:** enforcement is the STORED verdict — a re-drive clears the gate only if
/// that specific row is `approved` for the exact effect by a distinct HUMAN principal (see
/// [`HitlVerdictStore::approve`] / [`GateRecord::authorizes`]). Guessing an id yields nothing (an
/// unknown id fetches `None` → fail-closed deny). The unpredictability is a second wall on top of
/// the verdict check, never the check itself — so a CSPRNG (not a hash of nothing) is the correct
/// source even though the security does not rest on it.
pub fn opaque_gate_id() -> String {
    use aes_gcm::aead::rand_core::RngCore;
    use aes_gcm::aead::OsRng;
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut s = String::from("gate:");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// =================================================================================================
// The gate verdict state + row record.
// =================================================================================================

/// The `agent_hitl_gate.state` taxonomy (§4.4 — the SAME frozen lowercase tokens the agent-service
/// `HitlGateState` machine uses). A gate opens `waiting` and transitions ONCE to a terminal state;
/// a terminal row never re-transitions (a double-decide is a typed refusal, never a re-apply).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GateState {
    /// open, parked, undecided — the ONLY state a decide may transition from.
    Waiting,
    /// approved by a distinct eligible principal — the ONLY state that authorizes a re-drive.
    Approved,
    /// rejected — the effect is withheld forever (0 mutation, AG-8).
    Rejected,
    /// the approval window lapsed (auto-deny) — withheld forever.
    Expired,
}

impl GateState {
    /// The frozen §4.4 wire token for the `state` column.
    pub fn as_str(self) -> &'static str {
        match self {
            GateState::Waiting => "waiting",
            GateState::Approved => "approved",
            GateState::Rejected => "rejected",
            GateState::Expired => "expired",
        }
    }

    /// Parse the frozen wire token. An unknown token is durable corruption → FAIL-STATIC loud.
    pub fn parse(s: &str) -> GateState {
        match s {
            "waiting" => GateState::Waiting,
            "approved" => GateState::Approved,
            "rejected" => GateState::Rejected,
            "expired" => GateState::Expired,
            other => {
                panic!("FAIL-STATIC: unknown agent_hitl_gate.state `{other}` (durable corruption)")
            }
        }
    }

    /// Whether the state is terminal (decided) — a terminal row never re-transitions.
    pub fn is_terminal(self) -> bool {
        !matches!(self, GateState::Waiting)
    }
}

/// One `agent_hitl_gate` row — the §4.4 fields + the R2.4 enforcement columns. This is the durable
/// carrier the MCP governance layer and the agent-fabric HITL machinery both persist/consult; the
/// richer domain types (`HitlGate`, `RiskSummary`) live in `myelin-agent-service` and map onto it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateRecord {
    /// the OPAQUE server-issued gate id (the PK tail; NEVER a guessable display string).
    pub gate_id: String,
    /// the run whose gated effect this row withholds.
    pub run_id: String,
    /// the PER-EFFECT key this approval is bound to (`gate:{tool}:{object}` for a pipeline gate, the
    /// MCP effect key for an MCP gate, the per-effect `idem_key` for a batch card). An approval
    /// authorizes exactly THIS effect — never a bare tool name.
    pub effect_id: String,
    /// the per-subject-DEK-encrypted humanised risk-summary carrier (11.4); empty when not surfaced.
    pub risk_summary: Vec<u8>,
    /// the LIVE cost estimate, integer minor-units (never floats).
    pub cost_estimate: u64,
    /// who MAY approve (opaque pseudonyms, 4.8) — `list_subjects(object, approve_perm)`; the
    /// requesting principal is structurally excluded by the writer.
    pub approver_filter: Vec<String>,
    /// the §4.4 state machine.
    pub state: GateState,
    /// the surfaced card ref / the durable-wait idem_key base (§4.4).
    pub card_ref: Option<String>,
    /// the principal whose call tripped the gate (the agent) — the distinct-approver anchor.
    pub requested_by: String,
    /// the principal that decided the gate (approve/reject); `None` while waiting or on expiry.
    pub decided_by: Option<String>,
    /// Unix second at which the durable waiting row opened.
    pub opened_at_unix: i64,
    /// Unix second at which approve/reject/expiry made the row terminal.
    pub decided_at_unix: Option<i64>,
    /// Absolute Unix-second approval deadline. Waiting gates at or past this instant auto-expire.
    pub expires_at_unix: i64,
    /// Unix second at which an exact-bound approval was atomically consumed. `Some` is permanently
    /// non-authorizing, preventing a successful re-drive from applying twice.
    pub approval_consumed_at_unix: Option<i64>,
}

impl GateRecord {
    /// **The consult-time admission rule (the re-drive gate).** `true` IFF this gate is `Approved`,
    /// its `effect_id`, `run_id`, and original `requested_by` all exactly match the re-drive, the
    /// recorded approver is distinct, and the approval has not already been consumed. This is an
    /// observational pre-check only; callers must use [`HitlVerdictStore::consume_approval`] for
    /// the atomic one-shot authorization decision.
    pub fn authorizes(&self, effect_id: &str, run_id: &str, requester: &str) -> bool {
        self.state == GateState::Approved
            && self.effect_id == effect_id
            && self.run_id == run_id
            && self.requested_by == requester
            && self.approval_consumed_at_unix.is_none()
            && matches!(self.decided_by.as_deref(), Some(d) if d != requester)
    }
}

// =================================================================================================
// Typed refusals.
// =================================================================================================

/// A refusal opening a gate row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateOpenError {
    /// a row with this `gate_id` already exists (opaque ids never collide; a re-open is a bug).
    Duplicate,
    /// the record was not `Waiting` (a gate always opens undecided).
    NotWaiting,
}

impl core::fmt::Display for GateOpenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GateOpenError::Duplicate => {
                write!(f, "agent_hitl_gate row already exists for this gate_id")
            }
            GateOpenError::NotWaiting => write!(f, "a gate must open in the waiting state"),
        }
    }
}

impl std::error::Error for GateOpenError {}

/// A refusal deciding a gate (approve/reject/expire). Every arm is loud + typed; the row is
/// unchanged on refusal (the decide is fail-closed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateDecideError {
    /// no row with this `gate_id` in this tenant scope.
    NotFound,
    /// the gate is already terminal (a double-decide is refused, never a re-transition).
    AlreadyDecided(GateState),
    /// the approver is not in the gate's `approver_filter` — not eligible to decide.
    NotEligible,
    /// the approver IS the principal that requested the gated effect — a self-approval is refused
    /// SERVER-SIDE (the distinct-approver rule).
    SelfApproval,
    /// the approver is NOT a `Human` principal (a machine/agent/service) — the safety-critical HITL
    /// gate STRUCTURALLY requires a human approver (R2.4b — closes the machine-collusion gap where
    /// two in-tenant machine principals could clear a gate). Refused even if the machine sits in the
    /// `approver_filter` and differs from the requester.
    MachineApproverRefused,
    /// the durable approval window elapsed before this Human decision arrived.
    ApprovalWindowExpired,
}

impl core::fmt::Display for GateDecideError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GateDecideError::NotFound => write!(f, "no such hitl gate in this tenant scope"),
            GateDecideError::AlreadyDecided(s) => {
                write!(f, "hitl gate is already terminal ({})", s.as_str())
            }
            GateDecideError::NotEligible => {
                write!(f, "approver is not in the gate's approver_filter")
            }
            GateDecideError::SelfApproval => write!(
                f,
                "the requesting principal cannot approve its own gate (distinct-approver rule)"
            ),
            GateDecideError::MachineApproverRefused => write!(
                f,
                "a non-human (machine/agent/service) principal cannot approve a HITL gate — the \
                 gate structurally requires a distinct HUMAN approver"
            ),
            GateDecideError::ApprovalWindowExpired => {
                write!(f, "the hitl gate approval window has expired")
            }
        }
    }
}

impl std::error::Error for GateDecideError {}

/// A refusal consuming an approved gate for one effect application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateConsumeError {
    NotFound,
    NotApproved,
    BindingMismatch,
    AlreadyConsumed,
    Expired,
}

impl core::fmt::Display for GateConsumeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GateConsumeError::NotFound => write!(f, "no such hitl gate in this tenant scope"),
            GateConsumeError::NotApproved => write!(f, "hitl gate is not approved"),
            GateConsumeError::BindingMismatch => {
                write!(
                    f,
                    "hitl approval does not belong to this effect, run, and requester"
                )
            }
            GateConsumeError::AlreadyConsumed => write!(f, "hitl approval was already consumed"),
            GateConsumeError::Expired => write!(f, "hitl approval has expired"),
        }
    }
}

impl std::error::Error for GateConsumeError {}

// =================================================================================================
// The role struct + backend enum (the MR-009b durable-by-default shape).
// =================================================================================================

/// **The HITL verdict store role struct.** The production arm is the pool-backed
/// [`DurableHitlGates`] over the FORCE-RLS `agent_hitl_gate` table (migrations `0054`-`0055`); the
/// `test-support`-gated `Memory` arm is the DB-free TEST DOUBLE the unit tests + the MCP/agent
/// test compositions use. The method surface is identical across arms — the core verdict-lookup
/// logic is unit-testable DB-free.
pub struct HitlVerdictStore {
    backend: HitlVerdictBackend,
}

/// The backend of a [`HitlVerdictStore`] — in-memory test double (gated) or the durable PG arm.
enum HitlVerdictBackend {
    /// The in-memory gate map. **TEST DOUBLE (`#[cfg(any(test, feature = "test-support"))]` only).**
    #[cfg(any(test, feature = "test-support"))]
    Memory(MemoryHitlGates),
    /// The durable production backing over the `agent_hitl_gate` table.
    Durable(Box<DurableHitlGates>),
}

#[cfg(any(test, feature = "test-support"))]
impl Default for HitlVerdictStore {
    fn default() -> Self {
        Self::new()
    }
}

impl HitlVerdictStore {
    /// A fresh, empty IN-MEMORY store — the **test double** (`test-support`-gated). The PRODUCTION
    /// store is [`Self::with_pg`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> HitlVerdictStore {
        HitlVerdictStore {
            backend: HitlVerdictBackend::Memory(MemoryHitlGates::default()),
        }
    }

    /// Wrap the durable PG backing as the production store (over `agent_hitl_gate`, boot-applied by
    /// migrations `0054`-`0055`). **Must be called inside a tokio runtime** (the durable backing captures
    /// `Handle::current()` for its sync→async bridge).
    pub fn with_pg(provider: crate::provider::SubstrateProvider) -> HitlVerdictStore {
        HitlVerdictStore {
            backend: HitlVerdictBackend::Durable(Box::new(DurableHitlGates::new(provider))),
        }
    }

    /// **INSERT a pending gate (the withhold).** The record must be `Waiting`; a duplicate
    /// `gate_id` is refused. 0 mutation of anything else — a gate row is a durable wait, not an
    /// apply.
    pub fn open(&mut self, scope: &TenantScope, record: GateRecord) -> Result<(), GateOpenError> {
        if record.state != GateState::Waiting {
            return Err(GateOpenError::NotWaiting);
        }
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.open(scope, record),
            HitlVerdictBackend::Durable(d) => d.open(scope, record),
        }
    }

    /// **APPROVE a waiting gate — the server-side verdict.** Enforced HERE, not at the caller:
    /// the gate must exist and be `waiting`; the `approver` must be a **`Human` principal**
    /// (`approver_kind` — R2.4b: a machine/agent/service is refused even if listed in the filter);
    /// `approver` must be in the gate's `approver_filter`; and `approver` must DIFFER from the
    /// gate's `requested_by` (self-approval refused). On success the row is `approved` with
    /// `decided_by = approver`. The `approver_kind` is the AUTHENTICATED approver's kind (the same
    /// principal `decided_by` records) — it is checked, never persisted.
    pub fn approve(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        approver: &str,
        approver_kind: PrincipalKind,
    ) -> Result<(), GateDecideError> {
        self.approve_at(scope, gate_id, approver, approver_kind, system_unix_now())
    }

    pub fn approve_at(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        approver: &str,
        approver_kind: PrincipalKind,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        let is_human = matches!(approver_kind, PrincipalKind::Human);
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.decide(
                scope,
                gate_id,
                GateState::Approved,
                Some(approver),
                is_human,
                decided_at_unix,
            ),
            HitlVerdictBackend::Durable(d) => d.decide(
                scope,
                gate_id,
                GateState::Approved,
                Some(approver),
                is_human,
                decided_at_unix,
            ),
        }
    }

    /// **REJECT a waiting gate** (`decided_by = decider`; the reject reason rides the trace/audit
    /// domain side, not this row). A terminal rejection is a denial-of-service capability over the
    /// requested effect, so it requires the same distinct Human eligibility proof as approval.
    pub fn reject(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        decider: &str,
        decider_kind: PrincipalKind,
    ) -> Result<(), GateDecideError> {
        self.reject_at(scope, gate_id, decider, decider_kind, system_unix_now())
    }

    pub fn reject_at(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        decider: &str,
        decider_kind: PrincipalKind,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        let is_human = matches!(decider_kind, PrincipalKind::Human);
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.decide(
                scope,
                gate_id,
                GateState::Rejected,
                Some(decider),
                is_human,
                decided_at_unix,
            ),
            HitlVerdictBackend::Durable(d) => d.decide(
                scope,
                gate_id,
                GateState::Rejected,
                Some(decider),
                is_human,
                decided_at_unix,
            ),
        }
    }

    /// **EXPIRE a waiting gate** (the approval window lapsed — auto-deny; `decided_by` stays NULL).
    pub fn expire(&mut self, scope: &TenantScope, gate_id: &str) -> Result<(), GateDecideError> {
        self.expire_at(scope, gate_id, system_unix_now())
    }

    pub fn expire_at(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.decide(
                scope,
                gate_id,
                GateState::Expired,
                None,
                false,
                decided_at_unix,
            ),
            HitlVerdictBackend::Durable(d) => d.decide(
                scope,
                gate_id,
                GateState::Expired,
                None,
                false,
                decided_at_unix,
            ),
        }
    }

    /// Expire every waiting or approved-but-unconsumed gate whose durable deadline has elapsed,
    /// returning the exact rows that transitioned so the caller can emit minimized audit outcomes.
    /// A consumed approval remains immutable evidence of the one authorized attempt.
    pub fn expire_due(&mut self, scope: &TenantScope, now_unix: i64) -> Vec<GateRecord> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.expire_due(scope, now_unix),
            HitlVerdictBackend::Durable(d) => d.expire_due(scope, now_unix),
        }
    }

    /// Settle one exact gate as expired when its durable deadline has elapsed. This is the
    /// operator-decision counterpart to [`Self::expire_due`], avoiding a stale waiting row when an
    /// approve/reject command arrives on the deadline.
    pub fn expire_if_due(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        now_unix: i64,
    ) -> Option<GateRecord> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.expire_if_due(scope, gate_id, now_unix),
            HitlVerdictBackend::Durable(d) => d.expire_if_due(scope, gate_id, now_unix),
        }
    }

    /// Atomically consume one approved gate. Success is the authorization grant itself: the same
    /// gate can never return success again, even after a process restart.
    pub fn consume_approval(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        effect_id: &str,
        run_id: &str,
        requester: &str,
        now_unix: i64,
    ) -> Result<(), GateConsumeError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => {
                m.consume_approval(scope, gate_id, effect_id, run_id, requester, now_unix)
            }
            HitlVerdictBackend::Durable(d) => {
                d.consume_approval(scope, gate_id, effect_id, run_id, requester, now_unix)
            }
        }
    }

    /// **The verdict LOOKUP (the re-drive consult).** Fetch the gate row by its opaque `gate_id`
    /// within the verified tenant scope. `None` for an unknown/made-up id (fail-closed at the
    /// caller).
    pub fn fetch(&self, scope: &TenantScope, gate_id: &str) -> Option<GateRecord> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.fetch(scope, gate_id),
            HitlVerdictBackend::Durable(d) => d.fetch(scope, gate_id),
        }
    }

    /// Find the WAITING gate for `(run_id, effect_id)`, if one is already open (so a retried call
    /// re-surfaces the SAME pending gate instead of spawning duplicates).
    pub fn find_waiting(
        &self,
        scope: &TenantScope,
        run_id: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.find_waiting(scope, run_id, effect_id),
            HitlVerdictBackend::Durable(d) => d.find_waiting(scope, run_id, effect_id),
        }
    }
}

// =================================================================================================
// The in-memory TEST DOUBLE arm (test-support-gated; stripped by the scanner).
// =================================================================================================

/// The in-memory gate map — the DB-free test double behind [`HitlVerdictStore::new`]. Keys rows by
/// `(tenant, region, gate_id)` exactly as the durable PK does, and applies the SAME decide rules
/// (waiting-only, eligibility, distinct-approver), so the verdict-lookup logic is provable DB-free.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct MemoryHitlGates {
    rows: HashMap<(String, String, String), GateRecord>,
}

#[cfg(any(test, feature = "test-support"))]
impl MemoryHitlGates {
    fn key(scope: &TenantScope, gate_id: &str) -> (String, String, String) {
        (
            scope.tenant().0.clone(),
            scope.region().0.clone(),
            gate_id.to_string(),
        )
    }

    fn open(&mut self, scope: &TenantScope, record: GateRecord) -> Result<(), GateOpenError> {
        let key = Self::key(scope, &record.gate_id);
        if self.rows.contains_key(&key) {
            return Err(GateOpenError::Duplicate);
        }
        self.rows.insert(key, record);
        Ok(())
    }

    fn decide(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        to: GateState,
        decider: Option<&str>,
        approver_is_human: bool,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        let key = Self::key(scope, gate_id);
        let row = self.rows.get_mut(&key).ok_or(GateDecideError::NotFound)?;
        decide_rules(row, to, decider, approver_is_human, decided_at_unix)?;
        row.state = to;
        row.decided_by = decider.map(str::to_string);
        row.decided_at_unix = Some(decided_at_unix);
        Ok(())
    }

    fn expire_due(&mut self, scope: &TenantScope, now_unix: i64) -> Vec<GateRecord> {
        let (tenant, region) = (scope.tenant().0.as_str(), scope.region().0.as_str());
        let mut expired = Vec::new();
        for ((row_tenant, row_region, _), row) in &mut self.rows {
            if row_tenant == tenant
                && row_region == region
                && matches!(row.state, GateState::Waiting | GateState::Approved)
                && row.approval_consumed_at_unix.is_none()
                && row.expires_at_unix <= now_unix
            {
                row.state = GateState::Expired;
                row.decided_at_unix = Some(now_unix);
                expired.push(row.clone());
            }
        }
        expired
    }

    fn expire_if_due(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        now_unix: i64,
    ) -> Option<GateRecord> {
        let row = self.rows.get_mut(&Self::key(scope, gate_id))?;
        if matches!(row.state, GateState::Waiting | GateState::Approved)
            && row.approval_consumed_at_unix.is_none()
            && row.expires_at_unix <= now_unix
        {
            row.state = GateState::Expired;
            row.decided_at_unix = Some(now_unix);
            Some(row.clone())
        } else {
            None
        }
    }

    fn consume_approval(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        effect_id: &str,
        run_id: &str,
        requester: &str,
        now_unix: i64,
    ) -> Result<(), GateConsumeError> {
        let row = self
            .rows
            .get_mut(&Self::key(scope, gate_id))
            .ok_or(GateConsumeError::NotFound)?;
        consume_rules(row, effect_id, run_id, requester, now_unix)?;
        row.approval_consumed_at_unix = Some(now_unix);
        Ok(())
    }

    fn fetch(&self, scope: &TenantScope, gate_id: &str) -> Option<GateRecord> {
        self.rows.get(&Self::key(scope, gate_id)).cloned()
    }

    fn find_waiting(
        &self,
        scope: &TenantScope,
        run_id: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        let (t, rg) = (scope.tenant().0.as_str(), scope.region().0.as_str());
        self.rows
            .iter()
            .filter(|((kt, kr, _), _)| kt == t && kr == rg)
            .map(|(_, v)| v)
            .find(|v| {
                v.state == GateState::Waiting && v.run_id == run_id && v.effect_id == effect_id
            })
            .cloned()
    }
}

/// The SHARED decide-time rules (the memory arm applies them in Rust; the durable arm applies the
/// SAME predicate as the pre-read validation of its guarded SQL UPDATE, so the refusal is TYPED,
/// not a bare rows-affected-0). Fail-closed: waiting-only; Human decisions additionally require (in
/// order) a distinct principal, a HUMAN principal (R2.4b), and membership in the approver filter.
///
/// The `approver_is_human` flag is the AUTHENTICATED decider's `PrincipalKind == Human`. It applies
/// to approval and rejection; system expiry is the only decision without a Human.
fn decide_rules(
    row: &GateRecord,
    to: GateState,
    decider: Option<&str>,
    approver_is_human: bool,
    decided_at_unix: i64,
) -> Result<(), GateDecideError> {
    if row.state.is_terminal() {
        return Err(GateDecideError::AlreadyDecided(row.state));
    }
    if matches!(to, GateState::Approved | GateState::Rejected) {
        if row.expires_at_unix <= decided_at_unix {
            return Err(GateDecideError::ApprovalWindowExpired);
        }
        let Some(approver) = decider else {
            return Err(GateDecideError::NotEligible);
        };
        // (1) distinct from the requester (self-approval refused first — the most specific fact).
        if approver == row.requested_by {
            return Err(GateDecideError::SelfApproval);
        }
        // (2) HUMAN — the safety-critical HITL gate structurally requires a human approver, so a
        //     machine/agent/service in the filter can NEVER clear a gate (the machine-collusion
        //     gap, R2.4b).
        if !approver_is_human {
            return Err(GateDecideError::MachineApproverRefused);
        }
        // (3) eligible — in the gate's approver_filter.
        if !row.approver_filter.iter().any(|a| a == approver) {
            return Err(GateDecideError::NotEligible);
        }
    }
    Ok(())
}

fn consume_rules(
    row: &GateRecord,
    effect_id: &str,
    run_id: &str,
    requester: &str,
    now_unix: i64,
) -> Result<(), GateConsumeError> {
    if row.state != GateState::Approved {
        return Err(GateConsumeError::NotApproved);
    }
    if row.effect_id != effect_id || row.run_id != run_id || row.requested_by != requester {
        return Err(GateConsumeError::BindingMismatch);
    }
    if !matches!(row.decided_by.as_deref(), Some(decider) if decider != requester) {
        return Err(GateConsumeError::BindingMismatch);
    }
    if row.approval_consumed_at_unix.is_some() {
        return Err(GateConsumeError::AlreadyConsumed);
    }
    if row.expires_at_unix <= now_unix {
        return Err(GateConsumeError::Expired);
    }
    Ok(())
}

fn system_unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .expect("system clock must be after the Unix epoch")
}

// =================================================================================================
// DurableHitlGates — the always-compiled production arm over agent_hitl_gate (FORCE RLS).
// =================================================================================================

/// The REAL durable HITL gate store (production default) over the `agent_hitl_gate` table (0054-0055),
/// RLS-scoped through the MR-022 `with_tenant_tx` convention. Cloneable; holds the tokio runtime
/// handle so the SYNC store API bridges onto the async pool (the DurableCostLedger convention).
#[derive(Clone)]
pub struct DurableHitlGates {
    provider: crate::provider::SubstrateProvider,
    rt: tokio::runtime::Handle,
}

impl DurableHitlGates {
    /// Build the durable backing over the MR-022 provider. **Must be called inside a tokio runtime**
    /// (captures `Handle::current()` for the sync→async bridge).
    pub fn new(provider: crate::provider::SubstrateProvider) -> DurableHitlGates {
        DurableHitlGates {
            provider,
            rt: tokio::runtime::Handle::current(),
        }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// The sync→async bridge. A hard DB fault is FAIL-STATIC LOUD — the store's typed errors are
    /// domain refusals, never a coerced infra fault (a verdict that did not commit must never read
    /// as decided).
    fn block<T>(
        &self,
        fut: impl std::future::Future<Output = Result<T, crate::provider::ProviderError>>,
    ) -> T {
        tokio::task::block_in_place(|| self.rt.block_on(fut)).unwrap_or_else(|e| {
            panic!("FAIL-STATIC: durable hitl gate store fault (the gate row did not commit): {e}")
        })
    }

    fn open(&self, scope: &TenantScope, record: GateRecord) -> Result<(), GateOpenError> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        self.block(self.provider.with_tenant_tx(&scope.tenant().0, move |conn| {
            Box::pin(async move {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&record.gate_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                if exists {
                    return Ok(Err(GateOpenError::Duplicate));
                }
                sqlx::query(
                    "INSERT INTO agent_hitl_gate \
                       (tenant_id, region, gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                        approver_filter, state, card_ref, requested_by, decided_by, opened_at_unix, \
                        decided_at_unix, expires_at_unix, approval_consumed_at_unix) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NULL, $12, NULL, $13, NULL)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&record.gate_id)
                .bind(&record.run_id)
                .bind(&record.effect_id)
                .bind(&record.risk_summary)
                .bind(record.cost_estimate as i64)
                .bind(&record.approver_filter)
                .bind(GateState::Waiting.as_str())
                .bind(&record.card_ref)
                .bind(&record.requested_by)
                .bind(record.opened_at_unix)
                .bind(record.expires_at_unix)
                .execute(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                Ok(Ok(()))
            })
        }))
    }

    fn decide(
        &self,
        scope: &TenantScope,
        gate_id: &str,
        to: GateState,
        decider: Option<&str>,
        approver_is_human: bool,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let gate_id = gate_id.to_string();
        let decider = decider.map(str::to_string);
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        // One tenant-scoped tx: read the row FOR UPDATE, validate the SAME decide rules the
                        // memory arm applies (typed refusal), then write the terminal state + decided_by.
                        let row = sqlx::query(
                    "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                            state, card_ref, requested_by, decided_by, opened_at_unix, \
                            decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                     FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 FOR UPDATE",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&gate_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        let Some(row) = row else {
                            return Ok(Err(GateDecideError::NotFound));
                        };
                        let record = row_to_record(&gate_id, &row);
                        if let Err(e) = decide_rules(
                            &record,
                            to,
                            decider.as_deref(),
                            approver_is_human,
                            decided_at_unix,
                        ) {
                            return Ok(Err(e));
                        }
                        sqlx::query(
                            "UPDATE agent_hitl_gate SET state = $4, decided_by = $5, \
                                                       decided_at_unix = $6 \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 AND state = 'waiting'",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&gate_id)
                        .bind(to.as_str())
                        .bind(&decider)
                        .bind(decided_at_unix)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        Ok(Ok(()))
                    })
                }),
        )
    }

    fn fetch(&self, scope: &TenantScope, gate_id: &str) -> Option<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let gate_id = gate_id.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                    "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                            state, card_ref, requested_by, decided_by, opened_at_unix, \
                            decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                     FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&gate_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        Ok(row.map(|r| row_to_record(&gate_id, &r)))
                    })
                }),
        )
    }

    fn find_waiting(
        &self,
        scope: &TenantScope,
        run_id: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let run_id = run_id.to_string();
        let effect_id = effect_id.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                            "SELECT gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                            approver_filter, state, card_ref, requested_by, decided_by, \
                            opened_at_unix, decided_at_unix, expires_at_unix, \
                            approval_consumed_at_unix \
                     FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND effect_id = $4 \
                       AND state = 'waiting' \
                     ORDER BY gate_id LIMIT 1",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&run_id)
                        .bind(&effect_id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        Ok(row.map(|r| {
                            use sqlx::Row as _;
                            let gid: String = r.get("gate_id");
                            row_to_record(&gid, &r)
                        }))
                    })
                }),
        )
    }

    fn expire_due(&self, scope: &TenantScope, now_unix: i64) -> Vec<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let rows = sqlx::query(
                            "UPDATE agent_hitl_gate \
                     SET state = 'expired', decided_at_unix = $3 \
                     WHERE tenant_id = $1 AND region = $2 \
                       AND state IN ('waiting', 'approved') \
                       AND approval_consumed_at_unix IS NULL \
                       AND expires_at_unix <= $3 \
                     RETURNING gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                               approver_filter, state, card_ref, requested_by, decided_by, \
                               opened_at_unix, decided_at_unix, expires_at_unix, \
                               approval_consumed_at_unix",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(now_unix)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        Ok(rows
                            .iter()
                            .map(|row| {
                                use sqlx::Row as _;
                                let gate_id: String = row.get("gate_id");
                                row_to_record(&gate_id, row)
                            })
                            .collect())
                    })
                }),
        )
    }

    fn expire_if_due(
        &self,
        scope: &TenantScope,
        gate_id: &str,
        now_unix: i64,
    ) -> Option<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let gate_id = gate_id.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                            "UPDATE agent_hitl_gate \
                     SET state = 'expired', decided_at_unix = $4 \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 \
                       AND state IN ('waiting', 'approved') \
                       AND approval_consumed_at_unix IS NULL \
                       AND expires_at_unix <= $4 \
                     RETURNING gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                               approver_filter, state, card_ref, requested_by, decided_by, \
                               opened_at_unix, decided_at_unix, expires_at_unix, \
                               approval_consumed_at_unix",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&gate_id)
                        .bind(now_unix)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        Ok(row.map(|row| {
                            use sqlx::Row as _;
                            let gate_id: String = row.get("gate_id");
                            row_to_record(&gate_id, &row)
                        }))
                    })
                }),
        )
    }

    fn consume_approval(
        &self,
        scope: &TenantScope,
        gate_id: &str,
        effect_id: &str,
        run_id: &str,
        requester: &str,
        now_unix: i64,
    ) -> Result<(), GateConsumeError> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let gate_id = gate_id.to_string();
        let effect_id = effect_id.to_string();
        let run_id = run_id.to_string();
        let requester = requester.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                    "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                            state, card_ref, requested_by, decided_by, opened_at_unix, \
                            decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                     FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 FOR UPDATE",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&gate_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        let Some(row) = row else {
                            return Ok(Err(GateConsumeError::NotFound));
                        };
                        let record = row_to_record(&gate_id, &row);
                        if let Err(error) =
                            consume_rules(&record, &effect_id, &run_id, &requester, now_unix)
                        {
                            return Ok(Err(error));
                        }
                        let updated = sqlx::query(
                            "UPDATE agent_hitl_gate SET approval_consumed_at_unix = $4 \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 \
                       AND approval_consumed_at_unix IS NULL",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&gate_id)
                        .bind(now_unix)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        if updated.rows_affected() != 1 {
                            return Ok(Err(GateConsumeError::AlreadyConsumed));
                        }
                        Ok(Ok(()))
                    })
                }),
        )
    }
}

/// Map an `agent_hitl_gate` row to the [`GateRecord`] carrier (lossless; `cost_estimate` round-trips
/// the two's-complement `bigint` reinterpret the cost ledger uses).
fn row_to_record(gate_id: &str, row: &sqlx::postgres::PgRow) -> GateRecord {
    use sqlx::Row as _;
    GateRecord {
        gate_id: gate_id.to_string(),
        run_id: row.get("run_id"),
        effect_id: row.get("effect_id"),
        risk_summary: row
            .try_get::<Option<Vec<u8>>, _>("risk_summary")
            .unwrap_or(None)
            .unwrap_or_default(),
        cost_estimate: row.get::<i64, _>("cost_estimate") as u64,
        approver_filter: row.get("approver_filter"),
        state: GateState::parse(row.get::<String, _>("state").as_str()),
        card_ref: row.try_get::<Option<String>, _>("card_ref").unwrap_or(None),
        requested_by: row.get("requested_by"),
        decided_by: row
            .try_get::<Option<String>, _>("decided_by")
            .unwrap_or(None),
        opened_at_unix: row.get("opened_at_unix"),
        decided_at_unix: row
            .try_get::<Option<i64>, _>("decided_at_unix")
            .unwrap_or(None),
        expires_at_unix: row.get("expires_at_unix"),
        approval_consumed_at_unix: row
            .try_get::<Option<i64>, _>("approval_consumed_at_unix")
            .unwrap_or(None),
    }
}

// =================================================================================================
// DB-free unit tests — the verdict-lookup core over the memory arm (the same rules the SQL applies).
// =================================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn scope() -> TenantScope {
        let p = Principal::stub(
            PrincipalId("psn:human-x".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn waiting(gate_id: &str) -> GateRecord {
        GateRecord {
            gate_id: gate_id.into(),
            run_id: "mcp-run-1".into(),
            effect_id: "gate:git.merge:myelin://acme/git/pr/40".into(),
            risk_summary: vec![],
            cost_estimate: 50,
            approver_filter: vec!["psn:lead".into(), "psn:maintainer".into()],
            state: GateState::Waiting,
            card_ref: Some("card:R1:0".into()),
            requested_by: "agent:claude".into(),
            decided_by: None,
            opened_at_unix: 100,
            decided_at_unix: None,
            expires_at_unix: i64::MAX,
            approval_consumed_at_unix: None,
        }
    }

    /// A pending gate INSERTs waiting, is fetchable by its opaque id, and a duplicate open refuses.
    #[test]
    fn open_inserts_waiting_and_is_fetchable_by_gate_id() {
        let mut s = HitlVerdictStore::new();
        s.open(&scope(), waiting("gate:abc123")).expect("opens");
        let rec = s
            .fetch(&scope(), "gate:abc123")
            .expect("lookup-able by gate_id");
        assert_eq!(rec.state, GateState::Waiting);
        assert_eq!(rec.requested_by, "agent:claude");
        assert_eq!(
            s.open(&scope(), waiting("gate:abc123")),
            Err(GateOpenError::Duplicate),
            "a duplicate gate_id refuses"
        );
        assert_eq!(
            s.open(
                &scope(),
                GateRecord {
                    state: GateState::Approved,
                    ..waiting("gate:x")
                }
            ),
            Err(GateOpenError::NotWaiting),
            "a gate always opens undecided"
        );
    }

    /// An unknown/made-up gate id fetches None and cannot be decided (fail-closed).
    #[test]
    fn a_made_up_gate_id_is_nothing() {
        let mut s = HitlVerdictStore::new();
        assert!(s.fetch(&scope(), "gate:forged").is_none());
        assert_eq!(
            s.approve(&scope(), "gate:forged", "psn:lead", PrincipalKind::Human),
            Err(GateDecideError::NotFound)
        );
    }

    /// **The distinct-approver rule, server-side:** the requesting agent cannot approve its own
    /// gate; an out-of-filter principal cannot approve; an eligible DISTINCT human can — and the
    /// verdict records who decided.
    #[test]
    fn approve_enforces_eligibility_and_distinct_principal() {
        let mut s = HitlVerdictStore::new();
        // Even if the agent somehow sits in the filter, self-approval is refused FIRST.
        let mut rec = waiting("gate:abc");
        rec.approver_filter.push("agent:claude".into());
        s.open(&scope(), rec).unwrap();

        assert_eq!(
            s.approve(&scope(), "gate:abc", "agent:claude", PrincipalKind::Human),
            Err(GateDecideError::SelfApproval),
            "the requester can NEVER approve its own gate"
        );
        assert_eq!(
            s.approve(&scope(), "gate:abc", "psn:stranger", PrincipalKind::Human),
            Err(GateDecideError::NotEligible),
            "an out-of-filter principal cannot approve"
        );
        s.approve(&scope(), "gate:abc", "psn:lead", PrincipalKind::Human)
            .expect("an eligible distinct human approves");
        let rec = s.fetch(&scope(), "gate:abc").unwrap();
        assert_eq!(rec.state, GateState::Approved);
        assert_eq!(rec.decided_by.as_deref(), Some("psn:lead"));
    }

    /// **R2.4b — distinct-HUMAN, not just distinct-principal:** a machine/service/agent principal
    /// that IS in the `approver_filter` and DIFFERS from the requester is STILL refused (the
    /// machine-collusion gap); an eligible distinct HUMAN succeeds.
    #[test]
    fn approve_requires_a_human_principal_not_merely_a_distinct_one() {
        let machine_kinds = [
            PrincipalKind::Service,
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("rt-2".into()),
                on_behalf_of: None,
            },
        ];
        for kind in machine_kinds {
            let mut s = HitlVerdictStore::new();
            // A SECOND machine principal, listed in the filter and distinct from the requester.
            let mut rec = waiting("gate:m");
            rec.approver_filter.push("machine:ci-bot".into());
            s.open(&scope(), rec).unwrap();
            assert_eq!(
                s.approve(&scope(), "gate:m", "machine:ci-bot", kind),
                Err(GateDecideError::MachineApproverRefused),
                "a distinct, in-filter MACHINE approver is still refused (distinct-HUMAN rule)"
            );
            // the refused approval left the gate undecided.
            assert_eq!(
                s.fetch(&scope(), "gate:m").unwrap().state,
                GateState::Waiting
            );
        }

        // A distinct eligible HUMAN clears the same gate.
        let mut s = HitlVerdictStore::new();
        let mut rec = waiting("gate:m2");
        rec.approver_filter.push("machine:ci-bot".into());
        s.open(&scope(), rec).unwrap();
        s.approve(&scope(), "gate:m2", "psn:lead", PrincipalKind::Human)
            .expect("a human approver clears the gate");
        assert_eq!(
            s.fetch(&scope(), "gate:m2").unwrap().state,
            GateState::Approved
        );
    }

    #[test]
    fn reject_requires_the_same_distinct_eligible_human_as_approve() {
        let mut rec = waiting("gate:reject");
        rec.approver_filter.push("agent:claude".into());
        rec.approver_filter.push("machine:ci-bot".into());
        let mut store = HitlVerdictStore::new();
        store.open(&scope(), rec).unwrap();

        assert_eq!(
            store.reject(
                &scope(),
                "gate:reject",
                "agent:claude",
                PrincipalKind::Human,
            ),
            Err(GateDecideError::SelfApproval)
        );
        assert_eq!(
            store.reject(
                &scope(),
                "gate:reject",
                "psn:stranger",
                PrincipalKind::Human,
            ),
            Err(GateDecideError::NotEligible)
        );
        for kind in [
            PrincipalKind::Service,
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("rt:reject".into()),
                on_behalf_of: None,
            },
        ] {
            assert_eq!(
                store.reject(&scope(), "gate:reject", "machine:ci-bot", kind),
                Err(GateDecideError::MachineApproverRefused)
            );
        }
        assert_eq!(
            store.fetch(&scope(), "gate:reject").unwrap().state,
            GateState::Waiting,
            "every refused reject leaves the gate waiting"
        );
        store
            .reject(&scope(), "gate:reject", "psn:lead", PrincipalKind::Human)
            .expect("eligible distinct Human may reject");
        assert_eq!(
            store.fetch(&scope(), "gate:reject").unwrap().state,
            GateState::Rejected
        );
    }

    /// A terminal gate never re-transitions (double-decide refused, both directions).
    #[test]
    fn a_terminal_gate_refuses_re_decision() {
        let mut s = HitlVerdictStore::new();
        s.open(&scope(), waiting("gate:a")).unwrap();
        s.approve(&scope(), "gate:a", "psn:lead", PrincipalKind::Human)
            .unwrap();
        assert_eq!(
            s.approve(&scope(), "gate:a", "psn:maintainer", PrincipalKind::Human),
            Err(GateDecideError::AlreadyDecided(GateState::Approved))
        );
        assert_eq!(
            s.reject(&scope(), "gate:a", "psn:lead", PrincipalKind::Human),
            Err(GateDecideError::AlreadyDecided(GateState::Approved))
        );

        let mut s2 = HitlVerdictStore::new();
        s2.open(&scope(), waiting("gate:b")).unwrap();
        s2.reject(&scope(), "gate:b", "psn:lead", PrincipalKind::Human)
            .unwrap();
        assert_eq!(
            s2.approve(&scope(), "gate:b", "psn:lead", PrincipalKind::Human),
            Err(GateDecideError::AlreadyDecided(GateState::Rejected)),
            "a rejected gate can never be flipped to approved"
        );
    }

    /// **The consult-time admission rule** ([`GateRecord::authorizes`]): approved + same effect +
    /// distinct approver → true; waiting/rejected/expired, a DIFFERENT effect (an approval never
    /// transfers across effects, even same-tool), or a self-decided row → false.
    #[test]
    fn authorizes_is_per_effect_and_distinct_approver() {
        let mut s = HitlVerdictStore::new();
        s.open(&scope(), waiting("gate:a")).unwrap();

        // waiting → not authorized.
        let rec = s.fetch(&scope(), "gate:a").unwrap();
        assert!(!rec.authorizes(
            "gate:git.merge:myelin://acme/git/pr/40",
            "mcp-run-1",
            "agent:claude"
        ));

        s.approve(&scope(), "gate:a", "psn:lead", PrincipalKind::Human)
            .unwrap();
        let rec = s.fetch(&scope(), "gate:a").unwrap();
        assert!(
            rec.authorizes(
                "gate:git.merge:myelin://acme/git/pr/40",
                "mcp-run-1",
                "agent:claude"
            ),
            "approved + same effect/run/requester + distinct approver → authorized"
        );
        assert!(
            !rec.authorizes(
                "gate:git.merge:myelin://acme/git/pr/41",
                "mcp-run-1",
                "agent:claude"
            ),
            "an approval is bound to ITS effect — a sibling sharing the tool name is NOT authorized"
        );
        assert!(
            !rec.authorizes(
                "gate:git.merge:myelin://acme/git/pr/40",
                "mcp-run-1",
                "psn:lead"
            ),
            "the approver themselves re-driving is not a distinct-principal apply"
        );

        // A rejected gate never authorizes.
        let mut s2 = HitlVerdictStore::new();
        s2.open(&scope(), waiting("gate:b")).unwrap();
        s2.reject(&scope(), "gate:b", "psn:lead", PrincipalKind::Human)
            .unwrap();
        let rec = s2.fetch(&scope(), "gate:b").unwrap();
        assert!(!rec.authorizes(
            "gate:git.merge:myelin://acme/git/pr/40",
            "mcp-run-1",
            "agent:claude"
        ));
    }

    #[test]
    fn approval_is_exact_run_requester_bound_and_consumed_once() {
        let effect = "gate:git.merge:myelin://acme/git/pr/40";
        for (run_id, requester) in [
            ("different-run", "agent:claude"),
            ("mcp-run-1", "agent:different"),
        ] {
            let mut store = HitlVerdictStore::new();
            store.open(&scope(), waiting("gate:bound")).unwrap();
            store
                .approve_at(
                    &scope(),
                    "gate:bound",
                    "psn:lead",
                    PrincipalKind::Human,
                    110,
                )
                .unwrap();
            assert_eq!(
                store.consume_approval(&scope(), "gate:bound", effect, run_id, requester, 120,),
                Err(GateConsumeError::BindingMismatch)
            );
        }

        let mut store = HitlVerdictStore::new();
        store.open(&scope(), waiting("gate:once")).unwrap();
        store
            .approve_at(&scope(), "gate:once", "psn:lead", PrincipalKind::Human, 110)
            .unwrap();
        store
            .consume_approval(
                &scope(),
                "gate:once",
                effect,
                "mcp-run-1",
                "agent:claude",
                120,
            )
            .expect("the exact originating run consumes once");
        assert_eq!(
            store.consume_approval(
                &scope(),
                "gate:once",
                effect,
                "mcp-run-1",
                "agent:claude",
                121,
            ),
            Err(GateConsumeError::AlreadyConsumed)
        );
        assert!(!store.fetch(&scope(), "gate:once").unwrap().authorizes(
            effect,
            "mcp-run-1",
            "agent:claude"
        ));
    }

    #[test]
    fn elapsed_waiting_gate_expires_with_durable_timestamps() {
        let mut record = waiting("gate:elapsed");
        record.expires_at_unix = 200;
        let mut store = HitlVerdictStore::new();
        store.open(&scope(), record).unwrap();
        assert_eq!(store.expire_due(&scope(), 199), Vec::new());
        let expired = store.expire_due(&scope(), 200);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].state, GateState::Expired);
        assert_eq!(expired[0].opened_at_unix, 100);
        assert_eq!(expired[0].decided_at_unix, Some(200));
        assert_eq!(store.expire_due(&scope(), 201), Vec::new());

        let mut targeted = waiting("gate:targeted");
        targeted.expires_at_unix = 400;
        store.open(&scope(), targeted).unwrap();
        assert_eq!(store.expire_if_due(&scope(), "gate:targeted", 399), None);
        let expired = store
            .expire_if_due(&scope(), "gate:targeted", 400)
            .expect("the operator path settles the exact due row");
        assert_eq!(expired.state, GateState::Expired);
        assert_eq!(expired.decided_at_unix, Some(400));
        assert_eq!(store.expire_if_due(&scope(), "gate:targeted", 401), None);
    }

    #[test]
    fn elapsed_unconsumed_approval_expires_but_consumed_evidence_does_not() {
        let effect = "gate:git.merge:myelin://acme/git/pr/40";
        let mut unconsumed = waiting("gate:unconsumed");
        unconsumed.expires_at_unix = 200;
        let mut store = HitlVerdictStore::new();
        store.open(&scope(), unconsumed).unwrap();
        store
            .approve_at(
                &scope(),
                "gate:unconsumed",
                "psn:lead",
                PrincipalKind::Human,
                110,
            )
            .unwrap();
        assert_eq!(store.expire_due(&scope(), 200)[0].state, GateState::Expired);

        let mut consumed = waiting("gate:consumed");
        consumed.expires_at_unix = 300;
        store.open(&scope(), consumed).unwrap();
        store
            .approve_at(
                &scope(),
                "gate:consumed",
                "psn:lead",
                PrincipalKind::Human,
                210,
            )
            .unwrap();
        store
            .consume_approval(
                &scope(),
                "gate:consumed",
                effect,
                "mcp-run-1",
                "agent:claude",
                220,
            )
            .unwrap();
        assert_eq!(store.expire_due(&scope(), 300), Vec::new());
        let row = store.fetch(&scope(), "gate:consumed").unwrap();
        assert_eq!(row.state, GateState::Approved);
        assert_eq!(row.approval_consumed_at_unix, Some(220));
    }

    /// Expire is waiting-only and records no decider; find_waiting resurfaces the pending gate for
    /// the same (run, effect) and ignores decided rows.
    #[test]
    fn expire_and_find_waiting() {
        let mut s = HitlVerdictStore::new();
        s.open(&scope(), waiting("gate:a")).unwrap();
        let found = s
            .find_waiting(
                &scope(),
                "mcp-run-1",
                "gate:git.merge:myelin://acme/git/pr/40",
            )
            .expect("the pending gate is resurfaced (no duplicate spawn)");
        assert_eq!(found.gate_id, "gate:a");

        s.expire(&scope(), "gate:a").unwrap();
        let rec = s.fetch(&scope(), "gate:a").unwrap();
        assert_eq!(rec.state, GateState::Expired);
        assert_eq!(rec.decided_by, None, "an expiry has no decider");
        assert!(
            s.find_waiting(
                &scope(),
                "mcp-run-1",
                "gate:git.merge:myelin://acme/git/pr/40"
            )
            .is_none(),
            "a decided gate is no longer waiting"
        );
        assert_eq!(
            s.approve(&scope(), "gate:a", "psn:lead", PrincipalKind::Human),
            Err(GateDecideError::AlreadyDecided(GateState::Expired)),
            "an expired gate is terminal (auto-deny holds)"
        );
    }

    /// The migration group preserves deployed `0054` and appends `0055` for gate lifetime and
    /// one-shot approval consumption.
    #[test]
    fn migration_0054_carries_the_gate_shape_and_rls() {
        let m = hitl_gate_durable_migrations();
        assert_eq!(m.0.len(), 2);
        assert_eq!(m.0[0].id, "0054_agent_hitl_gate");
        assert_eq!(m.0[1].id, "0055_agent_hitl_gate_lifetime");
        for col in [
            "gate_id",
            "run_id",
            "effect_id",
            "risk_summary",
            "cost_estimate",
            "approver_filter",
            "state",
            "card_ref",
            "requested_by",
            "decided_by",
        ] {
            assert!(
                AGENT_HITL_GATE_MIGRATION.contains(col),
                "boot DDL carries `{col}`"
            );
        }
        assert!(AGENT_HITL_GATE_MIGRATION.contains("PRIMARY KEY (tenant_id, region, gate_id)"));
        assert!(AGENT_HITL_GATE_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(
            AGENT_HITL_GATE_MIGRATION.contains("risk_summary    bytea"),
            "the PII slot stays an encrypted byte carrier"
        );
        for column in [
            "opened_at_unix",
            "decided_at_unix",
            "expires_at_unix",
            "approval_consumed_at_unix",
        ] {
            assert!(AGENT_HITL_GATE_LIFETIME_MIGRATION.contains(column));
            assert!(!AGENT_HITL_GATE_MIGRATION.contains(column));
        }
    }
}
