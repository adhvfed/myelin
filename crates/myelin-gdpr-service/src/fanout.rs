//! # The data-map-driven per-holder checklist + the resumable fan-out + verifiable receipts +
//! the legal-hold gate (P-GA-12 → P-112)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§4.1** (the DSR
//! algorithm: step 2 *resolve scope FROM the data map* → a per-holder checklist with the
//! per-field erasure mechanism — *the map, not a hand-written list, drives fan-out*, "we forgot
//! the search index" is impossible; step 3 *the legal-hold gate* — an active `legal_hold` (G4)
//! suspends erasure + retention-expiry for the held scope (Art. 17(3)(e)), the request is
//! recorded *partially deferred*, access/portability still proceed; step 4 *fan out through the
//! holder contract* — each call **idempotent + resumable**, the **durable checklist IS the
//! state**, a crashed orchestrator re-drives only un-receipted holders, the canonical erase
//! order; step 5 *collect + verify receipts* + seal a **DSR completion receipt**) and **§4.2**
//! (verifiable receipts — `receipt = sign(hash(request_id ∥ holder ∥ scope ∥ outcome ∥
//! key_epoch_destroyed? ∥ timestamp))`, appended to the per-tenant audit Merkle tree). Prove-it:
//! `external-insights/01-process-and-quality-doctrine.md` §3 (a verifiable receipt recording the
//! destroyed key epoch makes "we erased it" independently checkable) + §4 (chain mutations
//! end-to-end — a DSR is a SEQUENCE; the chained fan-out is tested, not a single holder).
//!
//! **Contract-index:** row **10.4** — the data-map-driven checklist + the resumable fan-out + the
//! receipts + the legal-hold gate (OWNED here). Consumed: 10.1 (the holder contract — driven via
//! [`crate::orchestration::UpstreamHolderOrchestrator`]), 10.3 (`data_map()` — the
//! [`crate::datamap::Inventory`] the checklist is resolved from), 4.8 (the pseudonym lever — the
//! canonical-order step-1 the upstream orchestrator already sequences Identity-first).
//!
//! ## What THIS prompt (P-GA-12) ships — and what it reuses
//! P-GA-11 ([`crate::dsr::DsrOrchestrator`]) shipped the DSR **spine**: the total + ordered state
//! machine, the posture gate, the coarse deadline, and the READ-ONLY checklist resolve into
//! `dsr_status`. P-GA-06 ([`crate::orchestration::UpstreamHolderOrchestrator`]) shipped the
//! **canonical-order resumable fan-out over the holder contract** + the durable
//! [`crate::orchestration::EraseChecklist`]. This prompt **DRIVES** them together: the
//! [`FanOutDriver`] (1) resolves the per-holder checklist FROM the data map (already done by the
//! orchestrator's `fan_out`, surfaced in `dsr_status`), (2) applies the **legal-hold gate** (the
//! NEW G4 [`LegalHoldRegistry`] — fail-safe-to-suspend), (3) drives the resumable fan-out through
//! the upstream orchestrator (reusing the EXISTING checklist resumability), (4) collects + verifies
//! the receipts and **constructs the verifiable DSR completion receipt** ([`DsrCompletionReceipt`])
//! per the §4.2 formula, and (5) advances the DSR state machine to `Verified` / `Completed`. It
//! REUSES the existing orchestrators wholesale — it does not re-define the state machine, the
//! checklist, or the fan-out (EI-01 §7 coherence: extend in place, never duplicate).
//!
//! ## The legal-hold gate (§4.1 step 3 — the NEW surface this prompt wires)
//! An active `legal_hold` (G4) **suspends erasure** for the held scope: the gate is **wired here,
//! fail-safe-to-suspend** (a hold-registry read error suspends rather than risks an unlawful
//! erase under hold). When a scope is held, an ERASE is recorded *partially deferred*
//! ([`HoldVerdict::Deferred`]) — the fan-out does NOT run, the DSR does not reach `Verified`
//! through the held path; **access / portability still proceed** (a read right is never suspended
//! by a hold — §4.1 step 3). The retention-suspend ENGINE (the tightest-policy-wins +
//! legal-hold-aware retention engine) is **M2 P-GA-22 → P-149**; this prompt wires the GATE the
//! engine will back. The durable Postgres `legal_hold` (G4) table is the same DB floor every M0
//! in-memory store carries (P-007 / P-S12) — here it is an in-memory [`LegalHoldRegistry`] with
//! byte-for-byte the gate semantics.
//!
//! ## The DSR completion receipt (§4.2 — the verifiable content-addressed seal)
//! [`DsrCompletionReceipt`] is the **content-addressed** DSR-level receipt sealing the per-holder
//! fan-out: it carries the `request_id`, the scope token, the outcome, and the **ordered per-holder
//! receipts** (each itself content-addressed + recording its destroyed key epoch, P-105/P-106),
//! and content-addresses the whole bundle `blake3:<hex>` over the §4.2 canonical body
//! (`request_id ∥ holder ∥ scope ∥ outcome ∥ key_epoch_destroyed? ∥ timestamp`). It is the input
//! the [`crate::dsr::MerkleProvenBundle`] certificate seals (`dsr_certificate`). PII-free: it
//! carries only opaque ids + content-addresses — never a name/email — so it is safe to seal into
//! the tamper-evident audit log.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The Merkle SEAL of the completion receipt into the per-tenant audit tree** → **P-GA-20 →
//!   P-119** (this prompt CONSTRUCTS the content-addressed receipt; P-GA-20 anchors its root into
//!   the audit Merkle tree, making the certificate inclusion-provable —
//!   [`crate::dsr::MerkleProvenBundle::merkle_inclusion`] stays `None` until then).
//! - **The end-to-end erasure PROOF** (post-fan-out `locate` = 0 recoverable INCL. backups +
//!   worker-kill resumability across a restart/deploy) → **P-GA-14 → P-114** (this prompt proves
//!   the SYNCHRONOUS fan-out + the in-process worker-kill resumability; the backup-recovery +
//!   restart proof is P-GA-14, which reuses this driver).
//! - **The multi-cell `member_cells` iteration** (fan out to each cell's holders + merge per-cell
//!   receipts over the PII-free `CrossCellPointer` bridge) → **M5 P-GA-33 → and the GA-D8 gate**
//!   (this prompt is the single-cell driver each cell runs; the control plane sequences the wave).
//! - **The retention-expiry SUSPEND under a hold** (the tightest-policy-wins retention engine the
//!   gate suspends) → **M2 P-GA-22 → P-149** (this prompt wires the GATE; the engine is P-GA-22).
//! - **The durable Postgres `legal_hold` (G4) / `dsr_receipt` (G2) tables** → the same DB floor
//!   every M0 in-memory store carries (P-007 / P-S12). On this floor the hold registry + the
//!   receipt set are in-memory with byte-for-byte the gate + content-address semantics.
//!
//! ## Mutation floor (P-GA-12 TESTS — the resumable-checklist drive + the receipt-construction +
//! the legal-hold-gate paths are mandatory-core). `cargo mutants -p myelin-gdpr-service --file
//! src/fanout.rs` (2026-06-20): **24 mutants, 19 caught, 5 unviable, 0 missed** — every behavioral
//! mutant on the mandatory-core paths is CAUGHT. The behavioral core — the
//! [`LegalHoldRegistry::verdict`] gate (held ∧ erasure ⇒ defer; access/portability ⇒ proceed; the
//! fail-safe-to-suspend default), the [`FanOutDriver::drive`] sequence (resolve → gate → fan-out →
//! verify → complete; the deferred-no-fan-out branch), and the [`DsrCompletionReceipt`]
//! content-address (the §4.2 canonical body) — is the floor every mutation must be caught on
//! (EI-01 §3, stated not hidden).

use std::collections::BTreeSet;
use std::sync::Mutex;

use myelin_gdpr::EraseScope;
use myelin_substrate::Clock;

use crate::dsr::{DsrError, DsrId, DsrKind, DsrOrchestrator, DsrState, Result};
use crate::orchestration::{EraseChecklist, HolderReceipt, UpstreamHolderOrchestrator};

// ───────────────────────── the legal-hold gate (G4, §4.1 step 3) ─────────────────────────

/// The scope a legal hold can cover (G4 — gdpr §4.1 step 3 / §2.3). A hold on a whole **tenant**
/// (e.g. an active litigation / regulatory hold over the org) suspends erasure for every subject
/// in the tenant; a hold on a **subject** suspends erasure for that one data subject. PII-free: a
/// hold key is an opaque tenant token + an opaque subject token, never a name/email.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HoldScope {
    /// A hold over a whole tenant (every subject in it is held).
    Tenant(String),
    /// A hold over one data subject within a tenant (`tenant`, `subject_token`).
    Subject {
        /// the opaque tenant token the held subject lives under.
        tenant: String,
        /// the opaque subject token (the `principal_id`, never PII).
        subject: String,
    },
}

/// The legal-hold gate's verdict for a DSR (§4.1 step 3). An ERASE under an active hold is
/// **deferred** (recorded *partially deferred*; the fan-out does NOT run); a read right
/// (access / portability) is **never** suspended by a hold; an erase NOT under a hold proceeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldVerdict {
    /// No active hold bars this request — the fan-out / read proceeds.
    Proceed,
    /// An active legal hold suspends the ERASE for the held scope (Art. 17(3)(e)). The request is
    /// recorded *partially deferred*; the fan-out does NOT run. **Only an erase is deferred** — a
    /// read right (access/portability) never reaches this verdict (it always proceeds — §4.1 step 3).
    Deferred,
}

/// **The legal-hold registry (G4) — the gate the fan-out passes through (§4.1 step 3).** An
/// active hold over a scope suspends erasure for that scope. On the M1 floor this is an in-memory
/// set of held scopes (the durable Postgres `legal_hold` table is a named floor); the GATE
/// SEMANTICS — held ∧ erasure ⇒ defer; read ⇒ always proceed; **fail-safe-to-suspend** — are
/// byte-for-byte what the durable engine (P-GA-22) backs.
///
/// **Fail-safe-to-suspend:** the gate is biased to SUSPEND, never to risk an unlawful erase under
/// hold. If the registry cannot be consulted ([`LegalHoldRegistry::poisoned`] — a lock poisoned by
/// a panicked writer), the gate returns [`HoldVerdict::Deferred`] for an erase rather than
/// proceeding (a hold we cannot read is treated as PRESENT — gdpr §4.1 step 3, Art. 17(3)(e)).
#[derive(Default)]
pub struct LegalHoldRegistry {
    holds: Mutex<BTreeSet<HoldScope>>,
    /// A test lever forcing the fail-safe path: when set, [`LegalHoldRegistry::verdict`] treats the
    /// registry as un-readable (a poisoned-lock / unavailable-store stand-in) and SUSPENDS an erase.
    unreadable: std::sync::atomic::AtomicBool,
}

impl LegalHoldRegistry {
    /// A fresh hold registry (no active holds).
    pub fn new() -> LegalHoldRegistry {
        LegalHoldRegistry::default()
    }

    /// **`legal_hold_set(scope, on)` (contract 10.5 face — the gate half).** Set or clear a legal
    /// hold over a scope (G4). Setting a hold suspends erasure for the scope until cleared; the
    /// retention engine (P-GA-22) reads the same registry to suspend retention-expiry.
    pub fn set(&self, scope: HoldScope, on: bool) {
        let mut holds = self.holds.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            holds.insert(scope);
        } else {
            holds.remove(&scope);
        }
    }

    /// The count of active holds (the `legal_hold_active_count` telemetry signal — contract 1.8).
    pub fn active_count(&self) -> usize {
        self.holds.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Force the **fail-safe-to-suspend** path (a test lever standing in for an unavailable /
    /// poisoned hold registry). When `true`, [`Self::verdict`] treats the registry as un-readable
    /// and DEFERS an erase (never proceeds under an un-readable hold state).
    pub fn set_unreadable(&self, unreadable: bool) {
        self.unreadable
            .store(unreadable, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether the registry is (forced) un-readable — the fail-safe-to-suspend trigger.
    fn poisoned(&self) -> bool {
        self.unreadable.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether an active hold covers this erase scope (a subject is held if it is held directly OR
    /// its whole tenant is held; a tenant offboarding is held if the tenant is held). Read-only.
    fn scope_is_held(&self, scope: &EraseScope) -> bool {
        let holds = self.holds.lock().unwrap_or_else(|e| e.into_inner());
        match scope {
            EraseScope::Subject { subject, tenant } => {
                let tenant_token = tenant.0.clone();
                let subject_token = subject.principal.principal_id.0.clone();
                holds.contains(&HoldScope::Tenant(tenant_token.clone()))
                    || holds.contains(&HoldScope::Subject {
                        tenant: tenant_token,
                        subject: subject_token,
                    })
            }
            EraseScope::Tenant(tenant) => holds.contains(&HoldScope::Tenant(tenant.0.clone())),
        }
    }

    /// **The legal-hold gate verdict (§4.1 step 3).** For a request of `kind` over `scope`:
    /// - a **read right** (access / portability — `!kind.is_erasure()`) ALWAYS proceeds (a hold
    ///   never suspends access — §4.1 step 3);
    /// - an **erase** under an active hold (or under an un-readable registry, fail-safe) is
    ///   **deferred**;
    /// - an erase NOT under a hold proceeds.
    pub fn verdict(&self, kind: DsrKind, scope: &EraseScope) -> HoldVerdict {
        // A read right is never suspended by a hold (§4.1 step 3 — access/portability still proceed).
        if !kind.is_erasure() {
            return HoldVerdict::Proceed;
        }
        // Fail-safe-to-suspend: an un-readable registry defers the erase (a hold we cannot rule out
        // is treated as PRESENT — never risk an unlawful erase under hold).
        if self.poisoned() {
            return HoldVerdict::Deferred;
        }
        if self.scope_is_held(scope) {
            HoldVerdict::Deferred
        } else {
            HoldVerdict::Proceed
        }
    }
}

/// The `legal_hold_active_count` telemetry signal NAME + UNIT (contract 1.8 — gdpr §4.1 step 3 /
/// §5). PII-free: a count of active holds, never a held subject.
pub const LEGAL_HOLD_ACTIVE_COUNT: (&str, &str) = ("gdpr.legal_hold_active_count", "count");

// ───────────────────────── the verifiable DSR completion receipt (§4.2) ─────────────────────────

/// **The verifiable DSR completion receipt (§4.2).** Content-addressed over the §4.2 canonical
/// body — `request_id ∥ holder ∥ scope ∥ outcome ∥ key_epoch_destroyed? ∥ timestamp` — sealing
/// the per-holder fan-out into ONE DSR-level proof. It carries the ordered per-holder receipts
/// (each itself content-addressed + recording its destroyed key epoch, P-105/P-106), so an Art. 28
/// audit / supervisory authority can independently check "we erased it" against the KMS
/// key-destruction log. The **Merkle inclusion** that anchors it into the per-tenant audit tree is
/// **P-GA-20 → P-119**; this struct is the input that certificate seals.
///
/// PII-free: the `scope_token` is an opaque `tenant/subject` id pair (never a name/email); the
/// per-holder receipts carry only opaque ids + content-addresses. Safe to seal into the
/// tamper-evident audit log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrCompletionReceipt {
    /// The DSR this receipt completes (`request_id` — the §4.2 first field).
    pub request_id: DsrId,
    /// The opaque scope token (`tenant` for an offboarding; `tenant/subject` for a subject erase) —
    /// the §4.2 `scope` field. PII-free.
    pub scope_token: String,
    /// The DSR-level outcome (`"erased"` for a driven erase; `"deferred:legal_hold"` for a held
    /// erase; `"access"` / `"portability"` for a read right) — the §4.2 `outcome` field.
    pub outcome: String,
    /// The ordered per-holder receipts the fan-out collected (canonical erase-order; each records
    /// its destroyed key epoch). Empty for a read right or a deferred erase.
    pub holder_receipts: Vec<HolderReceipt>,
    /// The content-address over the whole bundle — `blake3:<hex>` of the §4.2 canonical body
    /// (`request_id ∥ holder ∥ scope ∥ outcome ∥ key_epoch_destroyed? ∥ timestamp`). The Merkle
    /// leaf P-GA-20 seals. Deterministic: the same inputs always content-address the same.
    pub content_hash: String,
    /// The completion timestamp (seconds) — the §4.2 `timestamp` field, folded into the content
    /// address so a receipt cannot claim a completion time it did not record.
    pub completed_at_secs: u64,
}

impl DsrCompletionReceipt {
    /// **Construct the content-addressed completion receipt (§4.2).** Hashes the canonical body
    /// `request_id ∥ holder ∥ scope ∥ outcome ∥ key_epoch_destroyed? ∥ timestamp` with BLAKE3 and
    /// renders `blake3:<hex>` (the ONE multihash convention the per-holder receipts + the audit
    /// Merkle leaf use). The `holder ∥ key_epoch_destroyed?` part folds in EACH per-holder
    /// receipt's content-address + destroyed epoch (the §4.2 "∥ holder ∥ … ∥ key_epoch_destroyed?"
    /// over the fan-out set) — so the DSR receipt cannot claim a holder erase it did not collect.
    fn build(
        request_id: &DsrId,
        scope_token: &str,
        outcome: &str,
        holder_receipts: &[HolderReceipt],
        completed_at_secs: u64,
    ) -> DsrCompletionReceipt {
        // The §4.2 canonical body — field-tagged + unit-separator-joined so two different field
        // sets can never collide into the same digest (a fixed separator, not raw concatenation).
        let mut body = format!(
            "request_id={}\u{1f}scope={scope_token}\u{1f}outcome={outcome}",
            request_id.0
        );
        // ∥ holder ∥ … ∥ key_epoch_destroyed? — fold in each per-holder receipt (content-address +
        // destroyed epoch), in the canonical erase-order the fan-out collected them.
        for hr in holder_receipts {
            body.push('\u{1f}');
            body.push_str(&format!(
                "holder={}:{}:{}",
                hr.holder_id,
                hr.receipt.receipt.content_hash,
                match hr.receipt.receipt.key_epoch_destroyed {
                    Some(e) => e.to_string(),
                    None => "none".to_string(),
                }
            ));
        }
        // ∥ timestamp
        body.push_str(&format!("\u{1f}timestamp={completed_at_secs}"));
        let digest = blake3::hash(body.as_bytes());
        DsrCompletionReceipt {
            request_id: request_id.clone(),
            scope_token: scope_token.to_string(),
            outcome: outcome.to_string(),
            holder_receipts: holder_receipts.to_vec(),
            content_hash: format!("blake3:{}", hex::encode(digest.as_bytes())),
            completed_at_secs,
        }
    }
}

/// The opaque, PII-free scope token for the §4.2 receipt body (`tenant` for an offboarding;
/// `tenant/subject` for a subject erase). Never a name/email — the [`EraseScope`] holds only the
/// opaque `principal_id` + the tenant token.
fn scope_token(scope: &EraseScope) -> String {
    match scope {
        EraseScope::Subject { subject, tenant } => {
            format!("{}/{}", tenant.0, subject.principal.principal_id.0)
        }
        EraseScope::Tenant(tenant) => tenant.0.clone(),
    }
}

// ───────────────────────── the fan-out outcome ─────────────────────────

/// The outcome of driving a DSR's fan-out (the [`FanOutDriver::drive`] return). Either the erase
/// was **driven** (the holders fanned + receipts collected + the DSR completed), the erase was
/// **deferred** under a legal hold (recorded *partially deferred*; the fan-out did NOT run), or a
/// **read right** completed (access/portability — no holder erase).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanOutOutcome {
    /// The erase fan-out ran to completion: every existing holder was driven in canonical order,
    /// the receipts collected + verified, the DSR sealed. Carries the verifiable completion receipt.
    Erased(DsrCompletionReceipt),
    /// The erase was **deferred under a legal hold** (§4.1 step 3 — *partially deferred*). The
    /// fan-out did NOT run; the DSR is parked (it does not advance to `Verified` through the held
    /// path). Carries the (zero-holder) deferred completion receipt (the audit trail of the defer).
    DeferredUnderHold(DsrCompletionReceipt),
    /// A **read right** (access / portability) completed — no holder erase ran (a read right is
    /// never suspended by a hold). Carries the (zero-holder) completion receipt.
    ReadRightServed(DsrCompletionReceipt),
}

impl FanOutOutcome {
    /// The verifiable completion receipt of this outcome (every outcome produces one — the audit
    /// trail, whether the erase ran, was deferred, or a read right was served).
    pub fn receipt(&self) -> &DsrCompletionReceipt {
        match self {
            FanOutOutcome::Erased(r)
            | FanOutOutcome::DeferredUnderHold(r)
            | FanOutOutcome::ReadRightServed(r) => r,
        }
    }
}

// ───────────────────────── the fan-out driver (the P-GA-12 surface) ─────────────────────────

/// **The DSR fan-out driver (contract 10.4 — the data-map-driven resumable fan-out).** Ties the
/// DSR spine ([`DsrOrchestrator`], P-GA-11) + the canonical-order resumable holder fan-out
/// ([`UpstreamHolderOrchestrator`], P-GA-06) + the legal-hold gate ([`LegalHoldRegistry`]) into the
/// §4.1 algorithm. It REUSES both orchestrators wholesale — it does NOT re-define the state
/// machine, the durable checklist, or the per-holder fan-out (EI-01 §7 coherence).
///
/// The driver is intentionally STATELESS (it holds only references) — the durable state is the DSR
/// register (G1, in the orchestrator) + the per-holder [`EraseChecklist`] (the resumability state).
/// A crashed driver re-`drive`s the SAME DSR id over the SAME checklist and re-drives only
/// un-receipted holders (resumability is a property of the checklist, not the driver).
pub struct FanOutDriver<'a, C: Clock> {
    /// The DSR spine (the state machine + the request register).
    dsr: &'a DsrOrchestrator<C>,
    /// The legal-hold gate (G4).
    holds: &'a LegalHoldRegistry,
    /// **The erasure ledger (10.8, P-GA-15) — written on a completed ERASE.** When present, a driven
    /// erase that completes writes one PII-free [`crate::erasure_ledger::ErasureLedgerEntry`]
    /// recording the opaque subject + the holders erased + the destroyed key epochs + the cross-seam
    /// completion offset, so a later restore re-erases the subject from it (Storage's
    /// `post_restore_reerase`, §4.4 / GD-14). The write is IDEMPOTENT (keyed on the DSR id — a worker
    /// restart re-driving the same id does NOT duplicate). `None` for the read-only / non-ledgered
    /// fan-out paths (e.g. the unit drills that prove the fan-out in isolation).
    ledger: Option<&'a crate::erasure_ledger::ErasureLedger>,
}

impl<'a, C: Clock> FanOutDriver<'a, C> {
    /// Build a driver over the DSR spine + the legal-hold gate (no erasure ledger — the fan-out runs
    /// but no completion is recorded into the 10.8 ledger). The upstream holder orchestrator + the
    /// durable checklist are passed per-`drive` (they are per-DSR-fan-out state).
    pub fn new(dsr: &'a DsrOrchestrator<C>, holds: &'a LegalHoldRegistry) -> FanOutDriver<'a, C> {
        FanOutDriver {
            dsr,
            holds,
            ledger: None,
        }
    }

    /// **Build a driver that WRITES the erasure ledger (10.8, P-GA-15) on a completed erase.** A
    /// driven erase that reaches `Completed` records one PII-free completion entry into `ledger`
    /// (the opaque subject + holders + destroyed key epochs + the cross-seam completion offset),
    /// driving Storage's `post_restore_reerase`. This is the constructor the cell-orchestration /
    /// boot path (`myelin-control-plane`) wires; the ledger then drives the restore-verify gate's
    /// re-erasure pass. The write is idempotent (a resume does not duplicate).
    pub fn with_ledger(
        dsr: &'a DsrOrchestrator<C>,
        holds: &'a LegalHoldRegistry,
        ledger: &'a crate::erasure_ledger::ErasureLedger,
    ) -> FanOutDriver<'a, C> {
        FanOutDriver {
            dsr,
            holds,
            ledger: Some(ledger),
        }
    }

    /// **Drive a validated DSR's fan-out (§4.1 steps 2–5), data-map-driven + resumable.** The
    /// caller has already `dsr_submit`+`validate`d the DSR (the posture gate ran, the request is
    /// `Validated`). This driver then:
    ///
    /// 1. **Resolves the per-holder checklist FROM the data map** — by advancing the orchestrator's
    ///    `fan_out` (which records the read-only checklist resolved from `inventory` into
    ///    `dsr_status` and parks the machine at `AwaitingHolders`). *The map, not a hand-written
    ///    list, drives the scope* (§4.1 step 2).
    /// 2. **Applies the legal-hold gate** (§4.1 step 3): an ERASE under an active hold is DEFERRED
    ///    (the fan-out does NOT run; the DSR stays parked at `AwaitingHolders`, recorded *partially
    ///    deferred*); a read right always proceeds.
    /// 3. **Fans out** through `upstream` in the canonical erase order, idempotently + resumably
    ///    (the durable `checklist` IS the state — a crashed driver re-drives only un-receipted
    ///    holders) — for an ERASE that is not held. A read right does NOT fan an erase (it has no
    ///    holder-erase step on this floor).
    /// 4. **Collects + verifies** the per-holder receipts (records them into the DSR via `verify`,
    ///    moving `AwaitingHolders → Verified`) and **constructs the verifiable DSR completion
    ///    receipt** (§4.2).
    /// 5. **Completes** the DSR (`Verified → Completed`) for a driven erase / a served read right.
    ///
    /// Returns the [`FanOutOutcome`]. Errors propagate the DSR state-machine errors (an illegal
    /// transition — never a silent skip) and the holder fan-out errors (a holder error fails the
    /// fan-out, leaving a resumable checklist — `drive` can be re-called to resume).
    pub fn drive(
        &self,
        id: &DsrId,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let req = self.dsr.request_view(id)?;
        let now = req.submitted_at_secs; // the receipt timestamp base (deterministic on the clock).

        // §4.1 step 2 — resolve the per-holder checklist FROM the data map (idempotent: if the DSR
        // is already at AwaitingHolders from a prior crashed drive, this is a no-op re-resolve).
        if req.state == DsrState::Validated {
            self.dsr.fan_out(id, inventory)?;
        }

        // §4.1 step 3 — the legal-hold gate.
        match self.holds.verdict(req.kind, &req.scope) {
            HoldVerdict::Deferred => {
                // The erase is suspended under an active hold (recorded *partially deferred*). The
                // fan-out does NOT run; the DSR stays parked at AwaitingHolders. We DO emit a
                // verifiable receipt of the defer (the audit trail) — zero holder receipts.
                let receipt = DsrCompletionReceipt::build(
                    id,
                    &scope_token(&req.scope),
                    "deferred:legal_hold",
                    &[],
                    now,
                );
                return Ok(FanOutOutcome::DeferredUnderHold(receipt));
            }
            HoldVerdict::Proceed => {}
        }

        // §4.1 step 3 — a READ RIGHT (access/portability) proceeds without a holder ERASE fan-out.
        if !req.kind.is_erasure() {
            // The read right is served: verify (no holder erase) + complete + a content-addressed
            // receipt. The per-holder access/export fan-out detail is the P-GA-13 read-rights body;
            // here the read right completes the state machine with a zero-erase receipt.
            self.dsr.verify(id, Vec::new())?;
            self.dsr.complete(id)?;
            let receipt = DsrCompletionReceipt::build(
                id,
                &scope_token(&req.scope),
                read_right_outcome(req.kind),
                &[],
                now,
            );
            return Ok(FanOutOutcome::ReadRightServed(receipt));
        }

        // §4.1 step 4 — fan out the ERASE through the holder contract in the canonical order,
        // idempotently + resumably (the durable checklist IS the state). A crashed driver re-drives
        // only un-receipted holders.
        let holder_receipts = self.dsr_fan_out_erase(&req.scope, upstream, checklist)?;

        // §4.1 step 5 — collect + verify the receipts (move AwaitingHolders → Verified), construct
        // the verifiable DSR completion receipt (§4.2), and complete the DSR (Verified → Completed).
        let receipt_strings: Vec<String> = holder_receipts
            .iter()
            .map(|hr| format!("{}:{}", hr.holder_id, hr.receipt.receipt.content_hash))
            .collect();
        // Verify is idempotent across a resume: only run it if the DSR has not already passed it.
        if self.dsr.state_of(id)? == DsrState::AwaitingHolders {
            self.dsr.verify(id, receipt_strings)?;
        }
        if self.dsr.state_of(id)? == DsrState::Verified {
            self.dsr.complete(id)?;
        }

        let receipt = DsrCompletionReceipt::build(
            id,
            &scope_token(&req.scope),
            "erased",
            &holder_receipts,
            now,
        );

        // §4.4 step 5 (P-GA-15) — WRITE THE ERASURE LEDGER (10.8) on a completed erase. The PII-free
        // completion entry (opaque subject + holders erased + destroyed key epochs + the cross-seam
        // completion offset) DRIVES Storage's `post_restore_reerase`: a later restore re-erases this
        // subject from the ledger so the restore never resurrects them (§3.2 / GD-14). The write is
        // IDEMPOTENT (keyed on the DSR id) — a worker restart re-driving the SAME id does NOT
        // duplicate (the ledger keeps the FIRST completion's offset). The LOAD-BEARING guard is
        // `state == Completed`: we NEVER record a completion entry for a DSR that did not reach the
        // verified+sealed terminal (a partial/failed fan-out leaves a resumable checklist, not a
        // completion). The ledger-present check lives inside [`Self::write_erasure_ledger_entry`].
        if self.dsr.state_of(id)? == DsrState::Completed {
            self.write_erasure_ledger_entry(id, &req.scope, &holder_receipts, now);
        }

        Ok(FanOutOutcome::Erased(receipt))
    }

    /// Write the PII-free [`crate::erasure_ledger::ErasureLedgerEntry`] for a completed erase (10.8)
    /// — a NO-OP when no ledger is wired ([`FanOutDriver::new`] vs [`FanOutDriver::with_ledger`]).
    /// The opaque subject token is the pseudonymous `principal_id` (never PII); a tenant offboarding
    /// records the `"*"` sentinel. The per-holder destroyed key epochs are read off the collected
    /// receipts (the §4.2 trail). The write is idempotent (keyed on the DSR id).
    ///
    /// **FLOOR (documented, EI-01 §1):** the **cross-seam completion offset** is, on this M1 floor,
    /// the completion timestamp `completed_at_secs` (a monotonic surrogate for the §7.3 WAL cursor —
    /// the same value Storage's `ErasureRecord.completed_at_offset` carries). The live binding (the
    /// real WAL offset the DSR completion lands at) is supplied by the cell-orchestration restore
    /// driver (the P-S12/P-S15 storage floor) when the durable `erasure_ledger` table lands; the
    /// ledger read shape (`completed_at_offset > pit`) does not change. The timestamp is monotone +
    /// strictly ordered with the restore PITs in the M1 drills, so the `> pit` selection is exact.
    fn write_erasure_ledger_entry(
        &self,
        id: &DsrId,
        scope: &EraseScope,
        holder_receipts: &[HolderReceipt],
        completed_at_secs: u64,
    ) {
        let Some(ledger) = self.ledger else { return };
        let (subject_token, tenant_token) = match scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.0.clone())
            }
            EraseScope::Tenant(tenant) => ("*".to_string(), tenant.0.clone()),
        };
        let holders_erased: Vec<String> = holder_receipts
            .iter()
            .map(|hr| hr.holder_id.to_string())
            .collect();
        let key_epochs_destroyed: Vec<crate::erasure_ledger::DestroyedKeyEpoch> = holder_receipts
            .iter()
            .map(|hr| crate::erasure_ledger::DestroyedKeyEpoch {
                holder_id: hr.holder_id.to_string(),
                key_epoch_destroyed: hr.receipt.receipt.key_epoch_destroyed,
            })
            .collect();
        ledger.record_completion(
            id.clone(),
            subject_token,
            tenant_token,
            holders_erased,
            key_epochs_destroyed,
            completed_at_secs, // the cross-seam completion offset (the §7.3 cursor surrogate — see floor).
            completed_at_secs,
        );
    }

    /// The §4.1-step-4 fan-out itself (split out so [`Self::drive`] reads as the §4.1 algorithm).
    /// Delegates to the EXISTING resumable canonical-order fan-out — no re-implementation.
    fn dsr_fan_out_erase(
        &self,
        scope: &EraseScope,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<Vec<HolderReceipt>> {
        upstream
            .fan_out_erase(scope, checklist)
            .map_err(|e| DsrError::HolderFanOut(e.0))
    }
}

/// The §4.2 outcome string for a read right (`access` for Art. 15, `portability` for Art. 20). A
/// non-read kind is a programming error here (the caller checked `!is_erasure()`); we map the
/// rectify/restrict kinds to their own names (P-GA-13 bodies them) for forward-compatibility.
fn read_right_outcome(kind: DsrKind) -> &'static str {
    match kind {
        DsrKind::Access => "access",
        DsrKind::Portability => "portability",
        DsrKind::Rectification => "rectification",
        DsrKind::Restriction => "restriction",
        DsrKind::Erasure => "erased", // unreachable on the read-right path; defensive.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holders::{InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
    use crate::orchestration::{holder_ids, SeamHolder};
    use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::TestClock;

    use crate::datamap::{Inventory, InventoryEntry};
    use crate::dsr::{Initiator, Posture};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject {
            subject: subject(s),
            tenant: t("acme"),
        }
    }

    /// A KMS seeded with one key per upstream holder (each holder shreds its OWN class).
    fn kms_with_all_holder_keys(tenant: &TenantId, base_epoch: u64) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        for (i, id) in [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .iter()
        .enumerate()
        {
            kms.provision(
                ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Subject((*id).to_string()),
                },
                base_epoch + i as u64,
            );
        }
        kms
    }

    fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
        [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .into_iter()
        .map(|id| {
            (
                id,
                SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
            )
        })
        .collect()
    }

    /// A real-shaped data-map inventory: one tagged identity field + one zero-PII derived holder.
    fn inventory() -> Inventory {
        let mut holders = BTreeSet::new();
        holders.insert("identity".to_string());
        holders.insert("search_index:search_index".to_string());
        Inventory {
            entries: vec![InventoryEntry {
                field_path: "PrincipalRow.email".into(),
                holder_id: "identity".into(),
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
        }
    }

    /// Submit + validate a controller-posture erase (admitted by the posture gate), returning the id.
    fn submit_validated_erase<C: Clock>(dsr: &DsrOrchestrator<C>, who: &str) -> DsrId {
        let id = dsr.dsr_submit(
            DsrKind::Erasure,
            t("acme"),
            subject(who),
            subject_scope(who),
            Posture::Controller,
            Initiator::Myelin,
        );
        assert!(dsr.validate(&id).unwrap(), "controller erase admitted");
        id
    }

    // ───────────── the data-map-driven fan-out (the GATE) ─────────────

    /// **The fan-out is DATA-MAP-DRIVEN + completes the DSR with a verifiable receipt.** The driver
    /// resolves the checklist FROM the map, fans the erase over every existing holder in canonical
    /// order, collects the receipts, and seals a content-addressed completion receipt. The DSR ends
    /// `Completed`; `erasure_fanout_coverage` is 100%.
    #[test]
    fn drive_fans_out_data_map_driven_and_seals_a_verifiable_receipt() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 100);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
        let holds = LegalHoldRegistry::new();
        let driver = FanOutDriver::new(&dsr, &holds);

        let id = submit_validated_erase(&dsr, "u-floor");
        let checklist = EraseChecklist::new();
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();

        // The DSR is COMPLETED via the state machine (awaiting-holders → verified → completed).
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
        // The checklist was resolved FROM the map (surfaced in dsr_status) — the map drives it.
        let status_checklist = dsr.dsr_status(&id).unwrap().checklist;
        let ids: Vec<&str> = status_checklist
            .iter()
            .map(|c| c.holder_id.as_str())
            .collect();
        assert!(ids.contains(&"identity") && ids.contains(&"search_index:search_index"));

        // The fan-out hit EVERY existing upstream holder in canonical order; 100% coverage.
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
        let receipt = match &outcome {
            FanOutOutcome::Erased(r) => r,
            other => panic!("expected Erased, got {other:?}"),
        };
        assert_eq!(
            receipt.holder_receipts.len(),
            6,
            "all six upstream holders receipted"
        );
        assert_eq!(
            receipt.holder_receipts[0].holder_id,
            holder_ids::IDENTITY,
            "Identity FIRST"
        );
        assert_eq!(receipt.outcome, "erased");
        assert!(
            receipt.content_hash.starts_with("blake3:"),
            "content-addressed (§4.2)"
        );
        // Every per-holder receipt records its destroyed key epoch (the §4.2 independent-check trail).
        for hr in &receipt.holder_receipts {
            assert!(hr.receipt.receipt.key_epoch_destroyed.is_some());
        }
        // The DSR certificate seals the same receipts (the auditor's verifiable bundle).
        let cert = dsr.dsr_certificate(&id).unwrap();
        assert_eq!(cert.receipts.len(), 6);
        assert!(
            cert.merkle_inclusion.is_none(),
            "the Merkle seal is P-GA-20"
        );
    }

    /// **The checklist is built FROM the data map, not a hard-coded list.** A DIFFERENT map (a new
    /// holder added) yields a DIFFERENT checklist — the map drives the scope. (The fan-out drives
    /// the UPSTREAM holder set; the checklist proves "we forgot the search index" is impossible.)
    #[test]
    fn checklist_is_resolved_from_the_map_a_new_map_holder_appears() {
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let driver = FanOutDriver::new(&dsr, &holds);
        let kms = kms_with_all_holder_keys(&t("acme"), 10);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );

        // Map A: one derived holder.
        let id_a = submit_validated_erase(&dsr, "u-a");
        driver
            .drive(&id_a, &inventory(), &upstream, &EraseChecklist::new())
            .unwrap();
        let a: BTreeSet<String> = dsr
            .dsr_status(&id_a)
            .unwrap()
            .checklist
            .iter()
            .map(|c| c.holder_id.clone())
            .collect();

        // Map B: an EXTRA holder added — the checklist grows (the map drives it).
        let mut inv_b = inventory();
        inv_b.holders.insert("refs_edge:refs_edge".to_string());
        let id_b = submit_validated_erase(&dsr, "u-b");
        driver
            .drive(&id_b, &inv_b, &upstream, &EraseChecklist::new())
            .unwrap();
        let b: BTreeSet<String> = dsr
            .dsr_status(&id_b)
            .unwrap()
            .checklist
            .iter()
            .map(|c| c.holder_id.clone())
            .collect();

        assert!(!a.contains("refs_edge:refs_edge"));
        assert!(
            b.contains("refs_edge:refs_edge"),
            "the new map holder appears in the checklist"
        );
    }

    // ───────────── the legal-hold gate (§4.1 step 3) ─────────────

    /// **The legal-hold gate DEFERS an erase under an active hold; the fan-out does NOT run.** A
    /// subject under a hold: the erase is recorded *partially deferred*, no holder is driven, the
    /// DSR stays parked at `AwaitingHolders`, and a deferred receipt is emitted.
    #[test]
    fn legal_hold_defers_an_erase_and_does_not_fan_out() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 200);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        // Set a hold over the subject.
        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            true,
        );
        assert_eq!(holds.active_count(), 1);
        let driver = FanOutDriver::new(&dsr, &holds);

        let id = submit_validated_erase(&dsr, "u-held");
        let checklist = EraseChecklist::new();
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();

        // DEFERRED — the fan-out did NOT run, the DSR is parked at AwaitingHolders.
        assert!(
            matches!(outcome, FanOutOutcome::DeferredUnderHold(_)),
            "erase deferred under hold"
        );
        assert_eq!(outcome.receipt().outcome, "deferred:legal_hold");
        assert!(
            outcome.receipt().holder_receipts.is_empty(),
            "no holder was driven"
        );
        assert_eq!(
            dsr.state_of(&id).unwrap(),
            DsrState::AwaitingHolders,
            "parked, not completed"
        );
        assert_eq!(
            upstream.fanout_coverage(&checklist),
            0.0,
            "0 holders driven under hold"
        );

        // Clear the hold and RE-DRIVE: the erase now proceeds to completion (resumable — the same
        // checklist re-drives the un-receipted holders).
        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            false,
        );
        let outcome2 = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        assert!(matches!(outcome2, FanOutOutcome::Erased(_)));
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
    }

    /// **A whole-tenant hold defers a subject erase within the tenant** (the held scope covers
    /// every subject in it).
    #[test]
    fn a_tenant_hold_defers_a_subject_erase_in_that_tenant() {
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &subject_scope("anyone")),
            HoldVerdict::Deferred
        );
        // a different tenant is NOT held.
        let other = EraseScope::Subject {
            subject: subject("x"),
            tenant: t("other"),
        };
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &other),
            HoldVerdict::Proceed
        );
    }

    /// **A read right (access/portability) is NEVER suspended by a hold (§4.1 step 3) — it still
    /// proceeds and completes.** Even with the subject under a hold, the access request completes.
    #[test]
    fn legal_hold_never_suspends_a_read_right() {
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true); // the whole tenant is held.
        let driver = FanOutDriver::new(&dsr, &holds);
        let kms = kms_with_all_holder_keys(&t("acme"), 300);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );

        for (kind, want) in [
            (DsrKind::Access, "access"),
            (DsrKind::Portability, "portability"),
        ] {
            let id = dsr.dsr_submit(
                kind,
                t("acme"),
                subject("reader"),
                subject_scope("reader"),
                Posture::Controller,
                Initiator::Myelin,
            );
            dsr.validate(&id).unwrap();
            let outcome = driver
                .drive(&id, &inventory(), &upstream, &EraseChecklist::new())
                .unwrap();
            assert!(
                matches!(outcome, FanOutOutcome::ReadRightServed(_)),
                "{kind:?} proceeds under hold"
            );
            assert_eq!(outcome.receipt().outcome, want);
            assert_eq!(
                dsr.state_of(&id).unwrap(),
                DsrState::Completed,
                "{kind:?} completes"
            );
        }
    }

    /// **Fail-safe-to-suspend: an un-readable hold registry DEFERS an erase** (never proceeds under
    /// a hold state it cannot rule out). A read right is still served (it is never gated by a hold).
    #[test]
    fn an_unreadable_hold_registry_fails_safe_to_suspend_for_an_erase() {
        let holds = LegalHoldRegistry::new();
        holds.set_unreadable(true);
        // an ERASE is deferred (fail-safe-to-suspend).
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &subject_scope("x")),
            HoldVerdict::Deferred
        );
        // a READ RIGHT is never gated by a hold (it short-circuits before the registry read).
        assert_eq!(
            holds.verdict(DsrKind::Access, &subject_scope("x")),
            HoldVerdict::Proceed
        );
    }

    // ───────────── resumability (the §4.1 step-4 property, driven through the driver) ─────────────

    /// **A worker kill mid-fan-out re-drives ONLY un-receipted holders (0 double-erase).** We
    /// simulate a crash by running a PARTIAL fan-out (first three holders) over the same checklist,
    /// then re-`drive` the full DSR: the first three are SKIPPED (not re-called), only the rest are
    /// driven, the DSR completes, and the result is a complete in-order receipt set.
    #[test]
    fn drive_is_resumable_a_worker_kill_redrives_only_un_receipted_holders() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 400);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let driver = FanOutDriver::new(&dsr, &holds);
        let id = submit_validated_erase(&dsr, "u-resume");
        let checklist = EraseChecklist::new();

        // Simulate a CRASH after the first three phases: drive only Identity/Blob/Authz over a
        // sub-orchestrator, recording into the SAME checklist.
        let first_three: Vec<(&'static str, &dyn PersonalDataHolder)> = holders
            .iter()
            .filter(|(id, _)| {
                *id == holder_ids::IDENTITY
                    || *id == holder_ids::BLOB
                    || *id == holder_ids::AUTHZ_TUPLES
            })
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect();
        let partial = UpstreamHolderOrchestrator::register_m1_upstream(first_three);
        partial
            .fan_out_erase(&subject_scope("u-resume"), &checklist)
            .unwrap();
        assert_eq!(
            checklist.done_count(),
            3,
            "the crash left three holders receipted"
        );
        let calls_after_partial: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        // RE-DRIVE the full DSR (resume after the crash) — only un-receipted holders are re-driven.
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        assert!(matches!(outcome, FanOutOutcome::Erased(_)));

        // The first three holders were NOT re-called (resumability — 0 double-erase).
        for (i, (id, _)) in holders.iter().enumerate() {
            if *id == holder_ids::IDENTITY
                || *id == holder_ids::BLOB
                || *id == holder_ids::AUTHZ_TUPLES
            {
                assert_eq!(
                    holders[i].1.erase_call_count(),
                    calls_after_partial[i],
                    "holder {id} was already receipted ⇒ NOT re-called (0 double-erase)"
                );
            } else {
                assert_eq!(
                    holders[i].1.erase_call_count(),
                    1,
                    "holder {id} driven on resume"
                );
            }
        }
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
    }

    /// **Re-driving a COMPLETED DSR is an idempotent no-op** (every holder already receipted ⇒ all
    /// skipped; the DSR is already Completed; the same content-addressed receipt re-affirms).
    #[test]
    fn re_driving_a_completed_dsr_is_idempotent() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 500);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(42));
        let holds = LegalHoldRegistry::new();
        let driver = FanOutDriver::new(&dsr, &holds);
        let id = submit_validated_erase(&dsr, "u-idem");
        let checklist = EraseChecklist::new();

        let first = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        let calls_after_first: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        let second = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        let calls_after_second: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        assert_eq!(
            first.receipt().content_hash,
            second.receipt().content_hash,
            "an idempotent re-drive seals the SAME content-addressed receipt"
        );
        assert_eq!(
            calls_after_first, calls_after_second,
            "no holder re-called on the idempotent re-drive"
        );
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
    }

    // ───────────── the verifiable receipt encodes the §4.2 fields ─────────────

    /// **The completion receipt encodes (request_id, holder, scope, outcome, key_epoch, timestamp)
    /// exactly (§4.2)** and is content-addressed (deterministic) — and CHANGING any §4.2 field
    /// changes the content address (a receipt cannot claim a field it did not record).
    #[test]
    fn completion_receipt_content_addresses_the_4_2_fields() {
        let id = DsrId("dsr:7".into());
        let hr = |holder: &'static str, epoch: Option<u64>| HolderReceipt {
            holder_id: holder,
            phase: crate::orchestration::CanonicalErasePhase::CryptoShredDek,
            receipt: myelin_gdpr::EraseReceipt {
                receipt: myelin_gdpr::Receipt::content_addressed(
                    "erase",
                    holder,
                    "u",
                    "acme",
                    "crypto_shred",
                    epoch,
                    0,
                ),
            },
        };
        let base = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9))],
            1000,
        );
        // deterministic: the SAME inputs content-address the same.
        let same = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9))],
            1000,
        );
        assert_eq!(base.content_hash, same.content_hash);
        assert!(base.content_hash.starts_with("blake3:"));

        // request_id matters.
        let diff_id = DsrCompletionReceipt::build(
            &DsrId("dsr:8".into()),
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_id.content_hash,
            "request_id is in the content address"
        );
        // scope matters.
        let diff_scope = DsrCompletionReceipt::build(
            &id,
            "acme/v",
            "erased",
            &[hr("blob_store", Some(9))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_scope.content_hash,
            "scope is in the content address"
        );
        // outcome matters.
        let diff_outcome = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "deferred:legal_hold",
            &[hr("blob_store", Some(9))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_outcome.content_hash,
            "outcome is in the content address"
        );
        // a holder's key_epoch matters.
        let diff_epoch = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(10))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_epoch.content_hash,
            "key_epoch is in the content address"
        );
        // timestamp matters.
        let diff_ts = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9))],
            2000,
        );
        assert_ne!(
            base.content_hash, diff_ts.content_hash,
            "timestamp is in the content address"
        );
        // the held holder set matters (an added holder receipt changes the address).
        let diff_holder = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9)), hr("event_bus", Some(11))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_holder.content_hash,
            "the holder set is in the content address"
        );
        // the fields are recorded verbatim.
        assert_eq!(base.request_id, id);
        assert_eq!(base.scope_token, "acme/u");
        assert_eq!(base.outcome, "erased");
        assert_eq!(base.completed_at_secs, 1000);
    }

    /// A tenant offboarding's scope token is the bare tenant (no subject); a subject erase's is
    /// `tenant/subject` (the §4.2 PII-free scope field).
    #[test]
    fn scope_token_is_pii_free_tenant_or_tenant_subject() {
        assert_eq!(scope_token(&subject_scope("u1")), "acme/u1");
        assert_eq!(scope_token(&EraseScope::Tenant(t("acme"))), "acme");
    }

    // ───────────── the erasure-ledger write on completion (P-GA-15, 10.8) ─────────────

    /// **A completed ERASE writes a PII-free erasure-ledger entry (10.8) — and a RESUME does not
    /// duplicate it.** The `with_ledger` driver records the opaque subject + holders + destroyed key
    /// epochs + the cross-seam completion offset; a second drive (a worker restart) re-affirms the
    /// SAME completion with NO duplicate entry (the idempotent ledger write). This is the §4.4 step-5
    /// write that drives Storage's `post_restore_reerase`.
    #[test]
    fn a_completed_erase_writes_the_erasure_ledger_idempotently() {
        use crate::erasure_ledger::ErasureLedger;

        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 600);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
        let holds = LegalHoldRegistry::new();
        let ledger = ErasureLedger::new();
        let driver = FanOutDriver::with_ledger(&dsr, &holds, &ledger);

        let id = submit_validated_erase(&dsr, "u-ledger");
        let checklist = EraseChecklist::new();
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        assert!(matches!(outcome, FanOutOutcome::Erased(_)));
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);

        // The ledger recorded ONE PII-free entry for the completion.
        assert_eq!(ledger.len(), 1, "the completion wrote one ledger entry");
        let entry = ledger.entry(&id).unwrap();
        assert_eq!(
            entry.subject_token, "u-ledger",
            "the opaque subject token (principal_id), never PII"
        );
        assert_eq!(entry.tenant_token, "acme");
        // every driven holder is recorded with its destroyed key epoch (the §4.2 trail).
        assert_eq!(
            entry.holders_erased.len(),
            6,
            "all six driven holders recorded"
        );
        assert!(entry.erased_holder(holder_ids::IDENTITY));
        for ke in &entry.key_epochs_destroyed {
            assert!(
                ke.key_epoch_destroyed.is_some(),
                "each holder's destroyed key epoch is recorded"
            );
        }
        // the completion offset drives re-erasure: a restore to BEFORE it re-erases this subject.
        assert_eq!(entry.completed_at_offset, 1_700_000_000);
        let post_pit = ledger.post_pit_records_after(1_699_999_999);
        assert_eq!(
            post_pit.len(),
            1,
            "a restore before the completion re-erases this subject"
        );
        assert_eq!(post_pit[0].subject, "u-ledger");
        // a restore AFTER the completion does not re-erase (already dead in that backup).
        assert!(ledger.post_pit_records_after(1_700_000_000).is_empty());

        // A RESUME (a worker restart re-driving the SAME id over the durable checklist) does NOT
        // duplicate the ledger entry (idempotent write).
        let driver2 = FanOutDriver::with_ledger(&dsr, &holds, &ledger);
        driver2
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        assert_eq!(
            ledger.len(),
            1,
            "a resume does NOT duplicate the ledger entry"
        );
    }

    /// **A DEFERRED erase (under a legal hold) writes NO ledger entry** (the erasure did not complete,
    /// so there is nothing to re-erase). The ledger records only COMPLETED erasures.
    #[test]
    fn a_deferred_erase_writes_no_ledger_entry() {
        use crate::erasure_ledger::ErasureLedger;

        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 700);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            true,
        );
        let ledger = ErasureLedger::new();
        let driver = FanOutDriver::with_ledger(&dsr, &holds, &ledger);

        let id = submit_validated_erase(&dsr, "u-held");
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &EraseChecklist::new())
            .unwrap();
        assert!(matches!(outcome, FanOutOutcome::DeferredUnderHold(_)));
        assert!(
            ledger.is_empty(),
            "a deferred erase writes NO ledger entry (it did not complete)"
        );
    }

    /// The telemetry signal name + unit are pinned (the `legal_hold_active_count` SLO, contract 1.8).
    #[test]
    fn legal_hold_telemetry_name_and_unit_are_pinned() {
        assert_eq!(LEGAL_HOLD_ACTIVE_COUNT.0, "gdpr.legal_hold_active_count");
        assert_eq!(LEGAL_HOLD_ACTIVE_COUNT.1, "count");
    }

    /// Setting then clearing a hold leaves the active count at 0 (the gate is reversible).
    #[test]
    fn a_hold_is_reversible_set_then_clear() {
        let holds = LegalHoldRegistry::new();
        let s = HoldScope::Subject {
            tenant: "acme".into(),
            subject: "u".into(),
        };
        holds.set(s.clone(), true);
        assert_eq!(holds.active_count(), 1);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &subject_scope("u")),
            HoldVerdict::Deferred
        );
        holds.set(s, false);
        assert_eq!(holds.active_count(), 0);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &subject_scope("u")),
            HoldVerdict::Proceed
        );
    }
}
