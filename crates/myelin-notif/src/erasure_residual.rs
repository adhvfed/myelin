//! # The erasure residual instanced — the X-7 posture for Notif (NOTIF-P27 / global P-469, M5)
//!
//! **Owning architecture doc:** `notifications.md` §3.9 (the `PersonalDataHolder` — the residual
//! stated **BY REFERENCE** to the platform posture X-7 / contract 10.9: the structural floor is
//! per-subject DEK crypto-shred of any inline-PII delivery columns + the `restrict` suppression +
//! a provider-side erasure request for the off-cell payload, the named sub-processor obligation;
//! Notif does **not** restate the posture). **Reconciliation:** `00-reconciliation-decisions.md`
//! §X-7 / OQ-G (ONE platform-wide free-text/immutable-content erasure lawful-basis posture,
//! instantiated per subsystem BY REFERENCE). **Drill source:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **NOTIF-D6** (erase a user →
//! every inbox item humanises to `[erased user]`; 0 recoverable PII incl. backups; off-cell-sent
//! payload crypto-shredded/erasure-requested; erase-receipt).
//!
//! **Contracts:** **7.7** PersonalDataHolder erase/restrict (OWNED — the residual instanced, the
//! erase/restrict half completed). CONSUMED: **10.9** the one erasure posture (BY REFERENCE — Notif
//! restates nothing), **11.4** per-subject DEK crypto-shred (the inline-PII delivery columns),
//! **10.8** the erasure ledger (the erase receipt is sealed here so the DSAR fan-out NOTIF-P30 can
//! prove Notif's holder coverage), **10.1** restrict (suppresses indexing/agent-use/analytics/notif).
//!
//! ## What NOTIF-P27 ships — the X-7 posture instanced (the structural floor, completed)
//!
//! The Notif erasure surface is SMALL and STRUCTURAL. Four legs, composed by [`erase_residual`]:
//!
//! 1. **References-not-payloads tombstone-for-free** (built since NOTIF-P4, [`crate::holder`]). An
//!    inbox row stores the subject ONLY as the OPAQUE recipient pseudonym + structured refs, never a
//!    stored name. Erasing the subject tombstones their appearance in EVERY inbox with **0 PII-column
//!    mutation** — Identity's 4.8 pseudonym shred makes the opaque id unresolvable and the title
//!    resolves to `[erased user]` (NOTIF-P9) at READ time. The holder reports the surface; it does NOT
//!    rewrite the refs-stored rows. (Re-confirmed in place, NOT re-implemented here.)
//!
//! 2. **Per-subject DEK crypto-shred of inline-PII delivery columns (11.4).** The ONE place Notif
//!    emits free text outside the cell is an off-cell **redacted** summary; where that summary holds
//!    inline PII it is sealed under a per-subject DEK ([`InlineDeliveryShredder`]). The erase destroys
//!    that DEK → the inline-PII delivery column (in the live store AND in every backup, which holds
//!    only the wrapped key) becomes unrecoverable ciphertext. Idempotent + **loud on a real failure**
//!    (a key it cannot reach is an error, NEVER a silent "assume erased").
//!
//! 3. **`restrict` suppression (10.1).** The erase first records the subject in the shared
//!    [`crate::holder::RestrictSet`] so the router/delivery suppress its NEW routing/delivery (and the
//!    subject is never indexed/agent-read/analysed pending erasure). Art. 18/21 + the X-7 suppression.
//!
//! 4. **The provider-side erasure request for the already-sent off-cell payload.** The named
//!    sub-processor obligation: for each off-cell payload Notif delivered for the subject, the
//!    [`crate::eu_provider::EuSovereignAdapter::request_provider_erasure`] hook (NOTIF-P26) issues a
//!    provider-side erasure request against the durable `provider_ref` so the sub-processor purges its
//!    copy. LOUD on a rejection (an un-purged copy is the residual — never silently swallowed).
//!
//! The erase emits its receipt into the Notif slice of the **erasure ledger** ([`NotifErasureLedger`],
//! 10.8) — PII-free + non-shred-erasable (it outlives the keys it records AND a restore, so the DSAR
//! fan-out can replay "this subject was erased" after a restore-from-backup).
//!
//! ## Notif restates NO platform posture (X-7)
//!
//! The residual third-party free-text case (a name a DIFFERENT user typed into THEIR content) is
//! governed where the content lives — the authoring subsystem — NOT in the inbox. Notif references the
//! one platform posture (10.9 / §X-7); it adds NO new `[OPEN — LEGAL]` residual of its own.
//!
//! ## FLOORS named (VISION §3 / EI-01 §1 — name your floors)
//!
//! - **The one `[OPEN — LEGAL]` residual lawful-basis statement (10.9)** awaits counsel/DPO
//!   ratification ([`crate::eu_provider::OPEN_LEGAL_PROVIDER_DPA`], dated 2026-06-25). The STRUCTURAL
//!   floor (all four legs above) ships NOW regardless; the residual is the ONE ratified statement, not
//!   a Notif-restated posture. Flagged, never silently claimed done.
//! - **The real KMS DEK destroy** (`myelin_storage::kms::KmsEngine::destroy_dek`, 11.3/11.4) is bound
//!   DOWNSTREAM of `myelin-notif` in the §2.9 DAG; [`InlineDeliveryShredder`] is the DAG-respecting
//!   LOCAL seam (the same posture `myelin-events`'s `InlinePiiShredder` takes), and
//!   [`InMemoryDeliveryShredder`] is the deterministic test/floor backing. The real binding swaps in
//!   behind the trait with NO change to the erase path.
//! - **The real off-cell provider** is the `[OPEN — LEGAL]` EU vendor (NOTIF-P26); the
//!   [`crate::eu_provider::RecordingEuTransport`] is the deterministic drill double the
//!   provider-side-erasure leg runs over here.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//!
//! The erase path is erasure-correctness critical (the X-7 posture for Notif): a leaked inline-PII DEK
//! is a recoverable-PII leak; a dropped provider-side erasure request is an un-purged sub-processor
//! copy; a missing ledger receipt is an un-provable erase. **Floor: ≥ 80% of viable mutants caught**
//! (`cargo mutants -p myelin-notif -f crates/myelin-notif/src/erasure_residual.rs`). Every leg — the
//! crypto-shred destroy + is-live, the loud-failure path, the restrict write, the provider-erasure
//! fan-out, the ledger seal, the 0-recoverable assertion — has a test a mutation flips.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{PiiKeyRef, Timestamp};
use myelin_tenancy::TenantId;

use crate::eu_provider::{EuProviderError, EuSovereignAdapter, ProviderErasureOutcome};
use crate::holder::RestrictSet;

/// The global ledger id of THIS prompt — the X-7 erasure-residual instancing for Notif. Asserted by
/// the scorecard test so the residual is a VISIBLE, named deliverable (not a silent claim).
pub const ERASURE_RESIDUAL_PROMPT: &str = "NOTIF-P27";

// ════════════════════════════════════════════════════════════════════════════════════════════
// (1) The per-subject DEK crypto-shred seam for inline-PII delivery columns (contract 11.4)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The crypto-shred seam the Notif erase destroys an inline-PII delivery-column DEK through
/// (contract 11.4 — per-subject DEK crypto-shred, CONSUMED).** A LOCAL trait (the real backing,
/// `myelin_storage::kms::KmsEngine::destroy_dek`, lives DOWNSTREAM of `myelin-notif` in the §2.9 DAG —
/// the SAME DAG-respecting posture `myelin_events::holder::InlinePiiShredder` takes). The contract:
/// destroying the key named by a per-subject `pii_key_ref` renders **every** inline-PII delivery
/// column sealed under it unrecoverable — in the live store AND in any backup (a backup holds only the
/// wrapped key, useless once its DEK is gone, storage §7.5). The op is **idempotent** (destroying an
/// already-destroyed key is a no-op success — re-erasure after a restore re-applies it) and **loud on
/// a real failure** (a key it cannot reach is an error, NEVER a silent "assume erased").
pub trait InlineDeliveryShredder {
    /// Destroy the DEK named by `key_ref` (crypto-shred the inline-PII delivery column). After this,
    /// [`is_live`](InlineDeliveryShredder::is_live) for `key_ref` is `false` and the sealed delivery
    /// column is unrecoverable ciphertext. Idempotent: destroying an already-destroyed key succeeds.
    fn destroy_key(&self, key_ref: &PiiKeyRef) -> Result<(), DeliveryShredError>;

    /// Whether the DEK named by `key_ref` is still live (resolvable). `false` once destroyed. The
    /// erase uses this to PROVE 0-recoverable after a shred and to detect already-erased keys.
    fn is_live(&self, key_ref: &PiiKeyRef) -> bool;
}

/// A loud crypto-shred failure (never silent — EI-01 §3). The erase surfaces this; it does not
/// "assume erased".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryShredError {
    /// The KMS could not be reached / the key could not be destroyed — the erase is INCOMPLETE and
    /// MUST be retried (the DSR is not done). Carries the offending `pii_key_ref` for the receipt.
    KmsUnavailable(PiiKeyRef),
}

impl std::fmt::Display for DeliveryShredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryShredError::KmsUnavailable(k) => write!(
                f,
                "crypto-shred: KMS unavailable for inline-PII delivery DEK {} — erase INCOMPLETE, retry",
                k.0
            ),
        }
    }
}

impl std::error::Error for DeliveryShredError {}

/// A deterministic in-memory [`InlineDeliveryShredder`] — the TEST/FLOOR backing. It models exactly
/// the `KmsEngine::destroy_dek` semantics (idempotent destroy; a destroyed key never resolves) without
/// pulling the downstream `myelin-storage` dependency (DAG-respecting). A key starts live the moment
/// it is [`seal`](InMemoryDeliveryShredder::seal)ed (when the off-cell redaction step minted the
/// per-subject `pii_key_ref` for the inline-PII delivery column); `destroy` makes it permanently dead.
#[derive(Clone, Default)]
pub struct InMemoryDeliveryShredder {
    /// Live key refs → still resolvable. A destroyed ref is REMOVED (a destroyed key has no entry —
    /// it is gone, not merely flagged, mirroring a wrapped-key delete).
    live: Arc<Mutex<std::collections::BTreeSet<String>>>,
    /// Simulate an unreachable KMS for the loud-failure drill (a key in this set fails to destroy).
    unreachable: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

impl InMemoryDeliveryShredder {
    /// A fresh shredder with no keys.
    pub fn new() -> InMemoryDeliveryShredder {
        InMemoryDeliveryShredder::default()
    }

    /// Seal (register) a per-subject DEK as live — what the off-cell redaction step did when it minted
    /// the `pii_key_ref` for an inline-PII delivery column. The erase's `is_live` reads this.
    pub fn seal(&self, key_ref: &PiiKeyRef) {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key_ref.0.clone());
    }

    /// Mark a key as unreachable (the KMS-outage drill): a subsequent `destroy_key` fails LOUDLY.
    pub fn make_unreachable(&self, key_ref: &PiiKeyRef) {
        self.unreachable
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key_ref.0.clone());
    }
}

impl InlineDeliveryShredder for InMemoryDeliveryShredder {
    fn destroy_key(&self, key_ref: &PiiKeyRef) -> Result<(), DeliveryShredError> {
        if self
            .unreachable
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&key_ref.0)
        {
            // Loud failure — the erase is INCOMPLETE; the caller must retry (never "assume erased").
            return Err(DeliveryShredError::KmsUnavailable(key_ref.clone()));
        }
        // Idempotent: removing an absent key is a no-op success (re-erasure after a restore).
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key_ref.0);
        Ok(())
    }

    fn is_live(&self, key_ref: &PiiKeyRef) -> bool {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&key_ref.0)
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// (2) The Notif slice of the PII-free, non-shred-erasable erasure ledger (contract 10.8)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// One erasure-ledger entry — a PII-free record that a subject's notification residual was erased,
/// naming the inline-PII delivery DEK refs that were shredded + the off-cell `provider_ref`s whose
/// erasure was requested (contract 10.8 / GDPR §4.4). It carries ONLY opaque ids + counts, never any
/// payload, never real-identity PII. It MUST survive the crypto-shred it records AND a restore (it is
/// the fact-of-erasure record — non-shred-erasable; otherwise a restore could resurrect the subject
/// with nothing to re-apply).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasedNotifSubject {
    /// The opaque, pseudonymous subject id that was erased (the §3.9 recipient/actor pseudonym).
    pub subject_id: String,
    /// The inline-PII delivery DEK refs that were crypto-shredded for this subject (PII-free refs).
    pub shredded_keys: Vec<PiiKeyRef>,
    /// The off-cell `provider_ref`s whose provider-side erasure was REQUESTED (the sub-processor
    /// copies being purged). PII-free opaque vendor handles.
    pub provider_erasures_requested: Vec<String>,
    /// When the erasure was recorded (the audit timestamp). PII-free.
    pub erased_at: Timestamp,
}

/// **The Notif slice of the PII-free erasure ledger (contract 10.8, CONSUMED).** Durably records that
/// a subject's notification residual was erased + which DEK refs were shredded + which off-cell copies
/// were requested-erased, so the DSAR fan-out (NOTIF-P30) can prove Notif's holder coverage AND a
/// re-erasure pass can re-apply after a restore. PII-free + non-shred-erasable (it must outlive the
/// keys it records). In the real binding `record` writes into the GDPR-owned global ledger (10.8)
/// through the downstream adapter (the floor); here it is an in-cell `(tenant)`-scoped record with the
/// SAME shape. Idempotent: recording an already-recorded subject MERGES the new key/provider refs.
#[derive(Clone, Default)]
pub struct NotifErasureLedger {
    // subject_id → the durable erased-subject record (idempotent merge on re-record).
    entries: Arc<Mutex<BTreeMap<String, ErasedNotifSubject>>>,
}

impl NotifErasureLedger {
    /// A fresh, empty ledger.
    pub fn new() -> NotifErasureLedger {
        NotifErasureLedger::default()
    }

    /// **Record that `subject_id` was erased (contract 10.8).** Idempotent: recording an already-erased
    /// subject MERGES the new `shredded_keys` + `provider_erasures` into the existing entry (a
    /// re-erasure after a restore re-applies cleanly, never duplicating). Keeps the EARLIEST
    /// `erased_at` (the first-erase timestamp is the audit truth).
    pub fn record(
        &self,
        subject_id: &str,
        shredded_keys: &[PiiKeyRef],
        provider_erasures: &[String],
        erased_at: Timestamp,
    ) {
        let mut g = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = g
            .entry(subject_id.to_string())
            .or_insert_with(|| ErasedNotifSubject {
                subject_id: subject_id.to_string(),
                shredded_keys: Vec::new(),
                provider_erasures_requested: Vec::new(),
                erased_at: erased_at.clone(),
            });
        for k in shredded_keys {
            if !entry.shredded_keys.contains(k) {
                entry.shredded_keys.push(k.clone());
            }
        }
        for p in provider_erasures {
            if !entry.provider_erasures_requested.contains(p) {
                entry.provider_erasures_requested.push(p.clone());
            }
        }
    }

    /// Whether `subject_id` has been recorded as erased (the fact-of-erasure check — distinguishes
    /// "erased" from "never seen"). True once `record`ed; a restore CANNOT clear it (non-shred-erasable).
    pub fn is_erased(&self, subject_id: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(subject_id)
    }

    /// The recorded erasure entry for `subject_id`, if any.
    pub fn entry(&self, subject_id: &str) -> Option<ErasedNotifSubject> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_id)
            .cloned()
    }

    /// How many subjects the ledger has recorded as erased.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// (3) The erase-residual orchestration — the four legs composed (the X-7 posture instanced)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// One off-cell payload Notif delivered for the subject that the residual erase must reach: the
/// inline-PII delivery DEK ref (crypto-shred target) + the `idem_key` (the provider-side-erasure-hook
/// target). References-not-payloads: this names refs/keys, never a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffCellResidual {
    /// The `idem_key` of the off-cell delivery (the [`EuSovereignAdapter::request_provider_erasure`]
    /// target — the durable handle the provider de-dupes + erases on).
    pub idem_key: String,
    /// The per-subject DEK ref sealing this delivery's inline-PII column (the crypto-shred target).
    /// `None` if the redacted summary carried NO inline PII (a fully-tombstoned summary — nothing to
    /// shred at this delivery).
    pub inline_pii_key: Option<PiiKeyRef>,
}

/// **The erase-residual receipt — the NOTIF-D6 artifact (the X-7 posture instanced for Notif).** The
/// PROOF the four legs ran: which DEKs were shredded, which provider-side erasures were requested, and
/// — the gate threshold — the count of inline-PII delivery columns that remain RECOVERABLE (their key
/// still live), which MUST be **0** after a successful erase. PII-free: subject discriminator + counts
/// + opaque refs, never a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidualEraseReceipt {
    /// The opaque, pseudonymous subject id erased.
    pub subject_id: String,
    /// The tenant the erase ran within (the holder never crosses a cell).
    pub tenant: TenantId,
    /// Whether the subject's NEW routing/delivery was suppressed (restrict, 10.1). Always `true` on a
    /// successful erase (the suppression is recorded FIRST).
    pub restrict_applied: bool,
    /// The inline-PII delivery DEK refs crypto-shredded (11.4).
    pub shredded_keys: Vec<PiiKeyRef>,
    /// The off-cell `provider_ref`s whose provider-side erasure was requested (the sub-processor
    /// copies being purged).
    pub provider_erasures_requested: Vec<String>,
    /// **The gate threshold — how many inline-PII delivery columns remain RECOVERABLE (key still
    /// live). MUST be 0 after a successful erase (NOTIF-D6 "0 recoverable PII"). Never softened.**
    pub recoverable_remaining: usize,
}

impl ResidualEraseReceipt {
    /// **The NOTIF-D6 green property: 0 recoverable PII.** True iff no inline-PII delivery column is
    /// still recoverable AND the restrict suppression was applied. The drill asserts THIS.
    pub fn is_green(&self) -> bool {
        self.recoverable_remaining == 0 && self.restrict_applied
    }
}

/// A loud erase-residual failure (never silent — EI-01 §3). The erase is INCOMPLETE; the DSR is not
/// done and MUST be retried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidualEraseError {
    /// A per-subject DEK could not be crypto-shredded (the KMS was unreachable). The inline-PII column
    /// is still recoverable — the erase is INCOMPLETE.
    Shred(DeliveryShredError),
    /// A sub-processor REJECTED a provider-side erasure request — the off-cell copy is un-purged (the
    /// residual surfaced, never silently swallowed). Carries the offending `provider_ref`.
    ProviderErasure(EuProviderError),
}

impl std::fmt::Display for ResidualEraseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidualEraseError::Shred(e) => write!(f, "erase residual incomplete: {e}"),
            ResidualEraseError::ProviderErasure(e) => {
                write!(
                    f,
                    "erase residual incomplete: provider-side erasure failed: {e}"
                )
            }
        }
    }
}

impl std::error::Error for ResidualEraseError {}

impl From<DeliveryShredError> for ResidualEraseError {
    fn from(e: DeliveryShredError) -> Self {
        ResidualEraseError::Shred(e)
    }
}

impl From<EuProviderError> for ResidualEraseError {
    fn from(e: EuProviderError) -> Self {
        ResidualEraseError::ProviderErasure(e)
    }
}

/// **Erase a subject's notification residual — the X-7 posture instanced for Notif (NOTIF-P27).**
/// Composes the four legs in the order that makes the erase LOUD + provable:
///
/// 1. **Restrict FIRST** — record the subject in `restrict` so NO new routing/delivery (or
///    indexing/agent-use/analytics) happens while the erase runs (10.1).
/// 2. **Crypto-shred** every inline-PII delivery DEK (11.4) — destroy the per-subject key so the
///    sealed off-cell summary column is unrecoverable ciphertext in the live store AND every backup.
///    LOUD on a KMS failure (the erase is INCOMPLETE — surfaced, never assumed-done).
/// 3. **Provider-side erasure request** for each already-sent off-cell payload (the named
///    sub-processor obligation) via the [`EuSovereignAdapter`] hook (NOTIF-P26). LOUD on a rejection.
/// 4. **Seal the erase receipt into the ledger** (10.8) so the DSAR fan-out (NOTIF-P30) can prove
///    coverage AND a re-erasure pass can re-apply after a restore.
///
/// The references-not-payloads tombstone-for-free leg ([`crate::holder`]) needs no work here — it is
/// structural (every inbox appearance already tombstones to `[erased user]` on the Identity pseudonym
/// shred). This function instances the INLINE-PII residual the platform posture (10.9) names. The
/// returned [`ResidualEraseReceipt`] proves **0 recoverable PII** (the NOTIF-D6 threshold).
///
/// Idempotent: a re-erase re-applies cleanly (the shred is idempotent, the provider de-dupes, the
/// ledger merges) and still reports 0 recoverable.
#[allow(clippy::too_many_arguments)]
pub fn erase_residual<S: InlineDeliveryShredder>(
    subject_id: &str,
    tenant: &TenantId,
    residuals: &[OffCellResidual],
    shredder: &S,
    restrict: &RestrictSet,
    provider: &EuSovereignAdapter,
    ledger: &NotifErasureLedger,
    erased_at: Timestamp,
) -> Result<ResidualEraseReceipt, ResidualEraseError> {
    // (1) Restrict FIRST — stop new routing/delivery (and indexing/agent-use/analytics) for the
    // subject while the erase runs (10.1). Idempotent.
    restrict.set(subject_id, true);

    // (2) Crypto-shred every inline-PII delivery DEK (11.4). LOUD on a KMS failure.
    let mut shredded_keys: Vec<PiiKeyRef> = Vec::new();
    for r in residuals {
        if let Some(key) = &r.inline_pii_key {
            shredder.destroy_key(key)?;
            if !shredded_keys.contains(key) {
                shredded_keys.push(key.clone());
            }
        }
    }

    // (3) Provider-side erasure request for each already-sent off-cell payload (NOTIF-P26 hook). LOUD
    // on a rejection (an un-purged sub-processor copy is the residual). A NothingToErase outcome (an
    // in-cell item, or a never-delivered one) is a surfaced no-op — not an error.
    let mut provider_erasures_requested: Vec<String> = Vec::new();
    for r in residuals {
        match provider.request_provider_erasure(&r.idem_key)? {
            ProviderErasureOutcome::Requested { provider_ref } => {
                if !provider_erasures_requested.contains(&provider_ref) {
                    provider_erasures_requested.push(provider_ref);
                }
            }
            ProviderErasureOutcome::NothingToErase => {}
        }
    }

    // The gate proof: 0 inline-PII delivery columns remain recoverable (every shredded key is dead).
    let recoverable_remaining = shredded_keys.iter().filter(|k| shredder.is_live(k)).count();

    // (4) Seal the erase receipt into the ledger (10.8) — PII-free, non-shred-erasable.
    ledger.record(
        subject_id,
        &shredded_keys,
        &provider_erasures_requested,
        erased_at,
    );

    Ok(ResidualEraseReceipt {
        subject_id: subject_id.to_string(),
        tenant: tenant.clone(),
        restrict_applied: restrict.is_restricted(subject_id),
        shredded_keys,
        provider_erasures_requested,
        recoverable_remaining,
    })
}

#[cfg(test)]
mod tests;
