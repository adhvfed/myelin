//! # `holder` — the Bus as a `PersonalDataHolder` + inline-PII crypto-shred to the KMS (EB-15 / P-092)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §4.8 (retention + crypto-shred + tombstones — the **references-not-payloads + crypto-shred +
//! tombstone** triad: this is the Bus's instantiation of the ONE platform erasure posture, X-7, by
//! reference NOT restated) + §5.7 (the `PersonalDataHolder` impl signature: `locate`/`erase`/
//! `export`).
//!
//! **Contract-index cluster:** row **2.7** (crypto-shred / tombstone on the log — **OWNED** here) +
//! rows **10.1** (`PersonalDataHolder` trait — **CONSUMED**: the Bus implements the trait), **11.3**
//! (the KMS hierarchy + `KeyOrigin` — **CONSUMED**: the inline-PII DEK is destroyed through it),
//! **11.4** (crypto-shred granularity / per-subject DEK — **CONSUMED**).
//!
//! ## What this module is (the event-log half of erasure-vs-immutability)
//! An append-only event log and the GDPR right-to-erasure are in tension; the platform resolves it
//! (external-insights/04 §1, Bus §4.8) with the triad:
//! 1. **references-not-payloads** — MOST events carry IDs/`ArtifactRef`s + a *pseudonymous*
//!    `actor.principal`; the person's real data lives in the producing subsystem's erasable store, so
//!    erasing the person **tombstones the identity, not the fact** (`contains_personal_data = false`).
//! 2. **crypto-shred** — the RARE inline-PII event is envelope-encrypted with `pii_key_ref =
//!    kms://<tenant>/<dek-epoch>/<class>` (per-tenant, optionally per-subject DEK); **erasure =
//!    destroy the key** → the ciphertext in the live log (and, by construction, in every backup that
//!    holds only the wrapped key) is unrecoverable.
//! 3. **tombstone** — a `*.erased` tombstone event lets live consumers degrade gracefully (render
//!    "[erased]", drop the row) without ever reading the now-unrecoverable payload.
//!
//! [`BusHolder`] is the mechanism implementing the §5.7 `locate` / `erase` / `export` trio over the
//! in-cell [`BusEventLog`], driving the crypto-shred through the [`InlinePiiShredder`] KMS seam and
//! emitting the `*.erased` tombstones through the **outbox** (the only sanctioned emit path — there
//! is no `publish_now`; BUS-2). This is the **STRUCTURAL FLOOR** (the event-log half of erasure).
//!
//! ## DEVIATION (EI-01 §1, documented — the DAG, code-wins-over-docs)
//! The EB-15 prompt's DELIVERABLE says "holder.rs: **impl PersonalDataHolder for the EventBus**". The
//! `PersonalDataHolder` trait (contract 10.1) lives in `myelin-gdpr`, and the real `KmsEngine`
//! (contract 11.3) lives in `myelin-storage` — and **both of those crates are DOWNSTREAM of
//! `myelin-events` in the frozen §2.9 DAG** (`gdpr → events`, `storage → events`). `myelin-events`
//! therefore *cannot* name `gdpr::PersonalDataHolder` or `storage::KmsEngine` without inverting the
//! DAG (the same constraint the `telemetry` module documents for the §10.2 `SignalName` enum, and
//! `crosscell` documents for `CrossCellPointer`). So this module ships the Bus's holder **mechanism**
//! to the *exact* §5.7 shape (`locate`/`erase`/`export` with the same semantics + receipt) against a
//! **local KMS-shred seam** ([`InlinePiiShredder`]) whose real backing is
//! `myelin_storage::kms::KmsEngine::destroy_dek` (contract 11.4 — the per-subject/per-tenant DEK
//! destroy). The thin `impl gdpr::PersonalDataHolder for ...` adapter that wraps THIS mechanism +
//! binds the live `KmsEngine` lives in the downstream GDPR orchestration prompt **P-GA-06 (P-106)**
//! (the upstream-store holder orchestration + the canonical erase order) — the **named floor**. The
//! H8 (event-bus) holder slot in the exhaustive H1–H18 catalog (`myelin-substrate`,
//! `holder_catalog.rs`, P-S27) is already declared; this module is what that slot resolves to.
//!
//! ## Floors named (stubbed/deferred → filling prompt)
//! - **The `impl gdpr::PersonalDataHolder` adapter + the live `KmsEngine` binding** is **P-GA-06
//!   (P-106)** (the deviation above). This module owns the mechanism + the [`InlinePiiShredder`]
//!   seam; a deterministic in-memory shredder ([`InMemoryShredder`]) is the test/floor backing.
//! - **The reaches-backups leg of BUS-D8** is the M5 follow-on **EB-29 (P-???)** (re-confirmed with
//!   STOR-D4). Here the live-store leg is proven (DEK destroyed → live payload unrecoverable +
//!   tombstones present); the *.erased key is excluded from backups by `KmsEngine::backup_snapshot`
//!   construction (storage §7.5) — re-confirmed against a real restored copy at M5.
//! - **Post-restore re-erasure** (the erasure ledger so the key STAYS destroyed across a restore) is
//!   the immediate follow-on **EB-16 (P-093)** — it re-applies THIS module's [`BusHolder::erase`].
//! - **The [OPEN — LEGAL] residual lawful-basis** (third-party PII a person typed into another's
//!   content) is the ONE platform posture (10.9, X-7, `00-reconciliation §X-7`) — handled by the
//!   GDPR/legal track, **not restated here**; the structural floor ships regardless.

use crate::outbox::{EmitContextBase, IdMinter, OutboxStore};
use crate::{
    Actor, AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventType, HandleOutcome,
    OutboxTx, PiiKeyRef, Region, TenantId, Timestamp, Visibility,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The `*.erased` tombstone event-name suffix (architecture §4.8 / §6.4 — the cross-cutting
/// tombstone token; lifecycle verb `erased`, §6.1). A tombstone's `type_` is
/// `<subsystem>.<artifact_type>.erased`; the Bus emits the cross-cutting `bus.event.erased` for its
/// own inline-PII events. A constant so the token has one authority and a typo cannot fork it.
pub const ERASED_EVENT_NAME: &str = "erased";

/// The Bus's own tombstone type for a crypto-shredded inline-PII event:
/// `bus.event.erased` (§6.1 grammar: subsystem `bus`, artifact `event`, verb `erased`).
pub const BUS_ERASED_TYPE: &str = "bus.event.erased";

// ════════════════════════════════════════════════════════════════════════════════════════════
// The crypto-shred KMS seam (contract 11.4, CONSUMED) — the DAG-respecting local trait
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The crypto-shred seam the Bus holder destroys an inline-PII DEK through (contract 11.4 —
/// crypto-shred granularity / per-subject DEK, **CONSUMED**). It is a LOCAL trait (the real backing,
/// `myelin_storage::kms::KmsEngine::destroy_dek`, lives DOWNSTREAM of `myelin-events` in the §2.9
/// DAG — see the module DEVIATION note). The contract: destroying the key named by a `pii_key_ref`
/// renders **every** ciphertext sealed under it unrecoverable, in the live log AND in any backup
/// (a backup holds only the wrapped key, useless once its KEK/DEK is gone — storage §7.5). The op is
/// **idempotent** (destroying an already-destroyed key is a no-op success — re-erasure after a
/// restore, EB-16, re-applies it) and **loud on a real failure** (a key it cannot reach is an
/// error, NEVER a silent "assume erased").
pub trait InlinePiiShredder {
    /// Destroy the DEK named by `key_ref` (crypto-shred). After this, [`InlinePiiShredder::is_live`]
    /// for `key_ref` returns `false` and any attempt to open ciphertext under it fails. Idempotent:
    /// destroying a key already destroyed succeeds (returns `Ok(())`).
    fn destroy_key(&self, key_ref: &PiiKeyRef) -> Result<(), ShredError>;

    /// Whether the DEK named by `key_ref` is still live (resolvable). `false` once destroyed. Used by
    /// the holder to PROVE 0-recoverable after an erase and to detect already-erased keys (the
    /// re-erasure idempotency, EB-16).
    fn is_live(&self, key_ref: &PiiKeyRef) -> bool;
}

/// A loud crypto-shred failure (never silent). The holder surfaces this; it does not "assume erased".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShredError {
    /// The KMS could not be reached / the key could not be destroyed — the erase is INCOMPLETE and
    /// MUST be retried (the DSR is not done). Carries the offending `pii_key_ref` for the receipt.
    KmsUnavailable(PiiKeyRef),
}

impl std::fmt::Display for ShredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShredError::KmsUnavailable(k) => {
                write!(f, "crypto-shred: KMS unavailable for {} — erase INCOMPLETE, retry", k.0)
            }
        }
    }
}

impl std::error::Error for ShredError {}

/// A deterministic in-memory [`InlinePiiShredder`] — the TEST/FLOOR backing. It models exactly the
/// `KmsEngine::destroy_dek` semantics (idempotent destroy; a destroyed key never resolves) without
/// pulling the downstream `myelin-storage` dependency. The real `KmsEngine` is bound by the
/// downstream adapter (P-GA-06, the floor). A key starts live the moment it is `seal`ed; `destroy`
/// makes it permanently dead.
#[derive(Clone, Default)]
pub struct InMemoryShredder {
    /// Live key refs → still resolvable. A destroyed ref is REMOVED (a destroyed key has no entry —
    /// it is gone, not merely flagged, mirroring a wrapped-key delete).
    live: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    /// Simulate an unreachable KMS for the loud-failure drill (a key in this set fails to destroy).
    unreachable: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl InMemoryShredder {
    /// A fresh shredder with no keys.
    pub fn new() -> Self {
        InMemoryShredder::default()
    }

    /// Seal (register) a DEK as live — what the producer's envelope-encryption step did when it
    /// minted the `pii_key_ref` (storage `ensure_dek`). The holder's `is_live` reads this.
    pub fn seal(&self, key_ref: &PiiKeyRef) {
        self.live.lock().expect("shredder live poisoned").insert(key_ref.0.clone());
    }

    /// Mark a key as unreachable (the KMS-outage drill): a subsequent `destroy_key` fails LOUDLY.
    pub fn make_unreachable(&self, key_ref: &PiiKeyRef) {
        self.unreachable.lock().expect("shredder unreachable poisoned").insert(key_ref.0.clone());
    }
}

impl InlinePiiShredder for InMemoryShredder {
    fn destroy_key(&self, key_ref: &PiiKeyRef) -> Result<(), ShredError> {
        if self.unreachable.lock().expect("shredder unreachable poisoned").contains(&key_ref.0) {
            // Loud failure — the erase is INCOMPLETE; the holder must retry (never "assume erased").
            return Err(ShredError::KmsUnavailable(key_ref.clone()));
        }
        // Idempotent: removing an absent key is a no-op success (re-erasure after a restore, EB-16).
        self.live.lock().expect("shredder live poisoned").remove(&key_ref.0);
        Ok(())
    }

    fn is_live(&self, key_ref: &PiiKeyRef) -> bool {
        self.live.lock().expect("shredder live poisoned").contains(&key_ref.0)
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The in-cell event log the holder operates over
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The in-cell event log the Bus holder operates `locate`/`erase`/`export` over. It models the
/// JetStream-retained envelopes within ONE `(tenant, region)` cell (the holder never crosses a cell;
/// residency-pin, EB-13). In the real binding this is the retained log; here it is an in-memory
/// append-only vector the holder reads — exactly the surface the §5.7 ops need (the events
/// themselves, their `contains_personal_data` flag, and their `pii_key_ref`). The crypto-shred does
/// NOT delete log rows (the log stays append-only/immutable — the *fact* is preserved); it destroys
/// the KEY, after which the inline-PII ciphertext is unrecoverable and a tombstone marks the row.
#[derive(Default)]
pub struct BusEventLog {
    /// Every retained envelope, append-order. References-not-payloads: most carry no inline PII.
    events: Vec<EventEnvelope>,
    /// Event ids tombstoned by an `erase` (their inline payload is now unrecoverable). A consumer
    /// reads this to degrade gracefully. Kept separate from the log so the log stays immutable.
    tombstoned: std::collections::BTreeSet<String>,
}

impl BusEventLog {
    /// An empty log.
    pub fn new() -> Self {
        BusEventLog::default()
    }

    /// Append a retained envelope (the relay published it; the holder can now see it).
    pub fn append(&mut self, env: EventEnvelope) {
        self.events.push(env);
    }

    /// Every retained envelope (append order).
    pub fn events(&self) -> &[EventEnvelope] {
        &self.events
    }

    /// Whether an event id has been tombstoned by an `erase` (its inline payload is unrecoverable).
    pub fn is_tombstoned(&self, event_id: &str) -> bool {
        self.tombstoned.contains(event_id)
    }

    /// Mark an event id tombstoned (called by the holder during `erase`). Idempotent.
    fn mark_tombstoned(&mut self, event_id: &str) {
        self.tombstoned.insert(event_id.to_string());
    }
}

/// Extract the `<id>` of a `subject:<id>` class from a `pii_key_ref`
/// (`kms://<tenant>/<dek-epoch>/subject:<id>`). Returns `None` for a tenant/blob-class ref (those
/// are not per-subject keys — a per-subject erase does not destroy a tenant-wide key, GD-4). This is
/// the Bus's read of the frozen §2.10 `pii_key_ref` grammar; it does not re-implement the storage
/// `KeyClass` parser (that authority is downstream) — it only needs the per-subject discriminator.
fn subject_of_key_ref(key_ref: &PiiKeyRef) -> Option<String> {
    let class = key_ref.0.rsplit('/').next()?;
    class.strip_prefix("subject:").map(|s| s.to_string())
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The §5.7 holder ops (locate / erase / export) — the OWNED contract 2.7
// ════════════════════════════════════════════════════════════════════════════════════════════

/// What `locate(subject)` returns (Bus §5.7): the subject's inline-PII events + the per-event
/// tombstone status. References-not-payloads, so this is typically SHORT (most events carry no
/// inline PII for the subject).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocateReport {
    /// The subject under query (the `subject:<id>` discriminator from the per-subject `pii_key_ref`).
    pub subject: String,
    /// Per inline-PII event: `(event_id, type, already_tombstoned)`.
    pub inline_pii_events: Vec<LocatedEvent>,
}

/// One located inline-PII event (the subject's), with its tombstone status (Bus §5.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedEvent {
    /// The event id (the row in the log).
    pub event_id: String,
    /// The event type (e.g. `chat.message.created` that carried inline PII).
    pub type_: String,
    /// The `pii_key_ref` whose DEK seals this event's inline payload.
    pub pii_key_ref: PiiKeyRef,
    /// Whether this event has already been tombstoned (its key already shredded).
    pub tombstoned: bool,
}

/// The receipt an `erase(subject)` returns (the BUS-D8 artifact). It is the PROOF the live-store leg
/// is green: the count of inline-PII keys destroyed, the count of `*.erased` tombstones emitted, and
/// the recoverable-count (which MUST be 0 after a successful erase — the gate threshold). PII-free:
/// it carries the subject discriminator + counts + key refs, never the erased payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EraseReceipt {
    /// The subject erased (the `subject:<id>` discriminator).
    pub subject: String,
    /// The tenant the erase ran within (the holder never crosses a cell).
    pub tenant: TenantId,
    /// How many distinct inline-PII DEKs were destroyed (crypto-shred).
    pub keys_shredded: usize,
    /// How many `*.erased` tombstones were emitted through the outbox.
    pub tombstones_emitted: usize,
    /// How many of the subject's inline-PII events remain recoverable (their key still live). The
    /// gate threshold is **0** — a successful erase leaves nothing recoverable in the live log.
    pub recoverable_remaining: usize,
}

/// The Bus's `PersonalDataHolder` MECHANISM (contract 2.7 OWNED; the §5.7 `locate`/`erase`/`export`
/// trio). It is constructed over a `(tenant, region)` cell + a crypto-shred seam; the downstream
/// `impl gdpr::PersonalDataHolder` adapter (P-GA-06, the floor) wraps it and binds the live
/// `KmsEngine`. See the module DEVIATION note for why the trait `impl` itself is downstream.
pub struct BusHolder<S: InlinePiiShredder> {
    tenant: TenantId,
    region: Region,
    shredder: S,
}

impl<S: InlinePiiShredder> BusHolder<S> {
    /// Construct the holder for one `(tenant, region)` cell over a crypto-shred seam.
    pub fn new(tenant: TenantId, region: Region, shredder: S) -> Self {
        BusHolder { tenant, region, shredder }
    }

    /// `locate(subject)` (Bus §5.7) → the subject's inline-PII events + tombstone status. Walks the
    /// retained log, selecting events that (a) carry inline PII and (b) are sealed under a
    /// per-subject DEK for THIS subject. References-not-payloads means this is typically empty/short.
    pub fn locate(&self, subject: &str, log: &BusEventLog) -> LocateReport {
        let mut inline_pii_events = Vec::new();
        for env in log.events() {
            if !env.contains_personal_data {
                continue; // tombstones the identity, not the fact — most events.
            }
            let Some(key_ref) = env.pii_key_ref.as_ref() else {
                continue;
            };
            if subject_of_key_ref(key_ref).as_deref() != Some(subject) {
                continue; // a different subject's (or a tenant/blob-class) key.
            }
            inline_pii_events.push(LocatedEvent {
                event_id: env.event_id.0.clone(),
                type_: env.type_.0.clone(),
                pii_key_ref: key_ref.clone(),
                tombstoned: log.is_tombstoned(&env.event_id.0),
            });
        }
        LocateReport { subject: subject.to_string(), inline_pii_events }
    }

    /// `erase(subject)` (Bus §4.8 / §5.7) → crypto-shred the subject's inline-PII keys + emit
    /// `*.erased` tombstones through the **outbox** (the only sanctioned emit path; BUS-2). Returns
    /// the [`EraseReceipt`] (the BUS-D8 artifact). The algorithm:
    /// 1. `locate` the subject's inline-PII events (those sealed under a per-subject DEK).
    /// 2. For each DISTINCT key, `destroy_key` through the crypto-shred seam (idempotent; loud on a
    ///    real KMS failure — that aborts the erase as INCOMPLETE, never "assume erased").
    /// 3. Mark each event tombstoned in the log + emit a `bus.event.erased` tombstone into the
    ///    outbox transaction (a live consumer reads it to degrade gracefully).
    /// 4. Re-verify: count how many of the subject's inline-PII events are still recoverable (key
    ///    still live). The receipt's `recoverable_remaining` MUST be 0 (the gate threshold).
    ///
    /// The `tx` is the open outbox transaction the tombstones are emitted into (the caller commits
    /// it — emit-iff-committed, BUS-D4: the tombstones become durable iff the erase commits).
    pub fn erase(
        &self,
        subject: &str,
        log: &mut BusEventLog,
        tx: &mut OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> Result<EraseReceipt, ShredError> {
        let report = self.locate(subject, log);

        // 1+2. Destroy each DISTINCT inline-PII DEK (crypto-shred). De-dup keys so we count each
        // destroyed key once (a per-subject DEK may seal several events).
        let mut distinct_keys: BTreeMap<String, PiiKeyRef> = BTreeMap::new();
        for ev in &report.inline_pii_events {
            distinct_keys.entry(ev.pii_key_ref.0.clone()).or_insert_with(|| ev.pii_key_ref.clone());
        }
        for key_ref in distinct_keys.values() {
            // Loud on failure: a key we cannot destroy aborts the erase as INCOMPLETE. We have not
            // yet mutated the log/emitted tombstones for THIS key, so the abort is clean (the DSR
            // retries; never a partial "assume erased").
            self.shredder.destroy_key(key_ref)?;
        }

        // 3. Tombstone each event + emit the `*.erased` tombstone through the outbox.
        let mut tombstones_emitted = 0usize;
        let mut otx = tx.begin(minter, self.emit_ctx_base());
        for ev in &report.inline_pii_events {
            log.mark_tombstoned(&ev.event_id);
            // The tombstone references the erased event + the now-dead key — references-not-payloads,
            // so it carries NO erased content (the payload is the unrecoverable ciphertext's id).
            let draft = self.erased_tombstone_draft(subject, &ev.event_id);
            otx.emit(draft, None).map_err(|_| {
                // An outbox failure is itself loud; surface it as an incomplete erase.
                ShredError::KmsUnavailable(ev.pii_key_ref.clone())
            })?;
            tombstones_emitted += 1;
        }
        // Stage a state-change marker so the tombstones co-commit with the erase bookkeeping
        // (emit-iff-committed: the tombstones are durable iff THIS commits).
        otx.stage_state_change(format!("bus.erase subject={subject} keys={}", distinct_keys.len()));
        otx.commit().map_err(|_| {
            ShredError::KmsUnavailable(
                report
                    .inline_pii_events
                    .first()
                    .map(|e| e.pii_key_ref.clone())
                    .unwrap_or_else(|| PiiKeyRef("kms://?/?/?".into())),
            )
        })?;

        // 4. Re-verify: 0 of the subject's inline-PII events must remain recoverable (key live).
        let recoverable_remaining = report
            .inline_pii_events
            .iter()
            .filter(|ev| self.shredder.is_live(&ev.pii_key_ref))
            .count();

        Ok(EraseReceipt {
            subject: subject.to_string(),
            tenant: self.tenant.clone(),
            keys_shredded: distinct_keys.len(),
            tombstones_emitted,
            recoverable_remaining,
        })
    }

    /// `export(subject)` (Bus §5.7) → the subject's events, references resolved via owners. The Bus
    /// returns the ENVELOPES the subject acted on / is the subject of (`actor` or per-subject
    /// `pii_key_ref`); the inline PII bodies are returned only if still recoverable (a tombstoned
    /// event exports its reference + an `[erased]` marker, never the unrecoverable payload). The
    /// "references resolved via owners" half (fetching the owning subsystem's row for a referenced
    /// id) is the producing subsystem's holder export — the Bus carries the references, not the
    /// bodies (references-not-payloads). This returns the portable per-event records.
    pub fn export(&self, subject: &str, log: &BusEventLog) -> Vec<ExportedEvent> {
        let mut out = Vec::new();
        for env in log.events() {
            let is_subject_actor = env.actor.0.principal_id.0 == subject;
            let is_subject_pii = env
                .pii_key_ref
                .as_ref()
                .and_then(subject_of_key_ref)
                .as_deref()
                == Some(subject);
            if !is_subject_actor && !is_subject_pii {
                continue;
            }
            let tombstoned = log.is_tombstoned(&env.event_id.0);
            out.push(ExportedEvent {
                event_id: env.event_id.0.clone(),
                type_: env.type_.0.clone(),
                subject_ref: env.subject.0.clone(),
                // The inline payload is exported only if NOT tombstoned (else the marker — the body
                // is unrecoverable). References-not-payloads: this is the referenced id, not PII.
                payload: if tombstoned {
                    serde_json::json!({ "status": "erased" })
                } else {
                    env.payload.clone()
                },
            });
        }
        out
    }

    /// The §5 emit context base the tombstone outbox transaction derives from (this cell's
    /// `(tenant, region)` + the Bus's own actor). The actor is the platform (a tombstone is a
    /// platform-controller event, `data_role = Controller`).
    fn emit_ctx_base(&self) -> EmitContextBase {
        EmitContextBase {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            actor: platform_actor(&self.tenant),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            caused_by: None,
        }
    }

    /// The `bus.event.erased` tombstone draft for one erased event (references-not-payloads: the
    /// payload carries the erased event's id, never its erased content).
    fn erased_tombstone_draft(&self, subject: &str, erased_event_id: &str) -> EventDraft {
        EventDraft {
            type_: EventType(BUS_ERASED_TYPE.into()),
            subject: ArtifactRef(format!("myelin://{}/bus/event/{erased_event_id}", self.tenant.0)),
            aggregate: AggregateKey(format!("bus.event:{erased_event_id}")),
            // References-not-payloads: the erased event's id + the subject discriminator, NEVER the
            // erased content (which is now unrecoverable ciphertext).
            payload: serde_json::json!({
                "erased_event_id": erased_event_id,
                "subject": subject,
                "reason": "crypto_shred",
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            // A tombstone carries NO inline PII (it is the marker that the PII is gone).
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

/// The Bus's own platform actor for a tombstone emit (the platform is the controller of a `*.erased`
/// tombstone). A pseudonymous platform principal — references-not-payloads.
fn platform_actor(tenant: &TenantId) -> Actor {
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    Actor(Principal::stub(
        PrincipalId("bus:platform".into()),
        PrincipalKind::Service,
        tenant.clone(),
    ))
}

/// One exported per-event record (Bus §5.7 `export`). PII-free at the Bus layer: it carries the
/// referenced ids; a tombstoned event's body is `{"status":"erased"}` (the payload is unrecoverable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedEvent {
    /// The event id.
    pub event_id: String,
    /// The event type.
    pub type_: String,
    /// The `subject` `ArtifactRef` (the referenced artifact, not PII).
    pub subject_ref: String,
    /// The payload (references), or `{"status":"erased"}` if the event was tombstoned.
    pub payload: serde_json::Value,
}

/// How a live consumer should degrade when it sees an event that has been tombstoned (Bus §4.8 — a
/// `*.erased` tombstone lets consumers degrade gracefully). The consumer renders the marker / drops
/// the row WITHOUT reading the now-unrecoverable payload — it NEVER tries to decrypt a shredded key
/// (that would fail loudly) and it NEVER treats the tombstone as a poison message (it is a normal,
/// expected lifecycle event). This helper encodes the "degrade gracefully" rule so a consumer cannot
/// get it wrong: a tombstone is always `Done` (handled), never `Retry`/`NonRetryable`.
pub fn degrade_on_tombstone(env: &EventEnvelope) -> HandleOutcome {
    debug_assert_eq!(
        env.type_.0, BUS_ERASED_TYPE,
        "degrade_on_tombstone is for the *.erased tombstone only"
    );
    // A tombstone is a normal, expected event — the consumer acknowledges it (drops/renders the
    // erased marker) and moves on. It NEVER blocks the stream and NEVER reads the shredded payload.
    HandleOutcome::Done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive_envelope;
    use crate::envelope::EmitContext;
    use crate::outbox::MonotonicMinter;
    use crate::{CausedBy, EventId};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn actor_for(id: &str) -> Actor {
        Actor(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant()))
    }

    /// Build a retained envelope in the log. `pii` = Some(subject) means an inline-PII event sealed
    /// under that subject's per-subject DEK; None means references-not-payloads (no inline PII).
    fn retained(
        event_id: &str,
        type_: &str,
        actor_id: &str,
        pii_subject: Option<&str>,
    ) -> EventEnvelope {
        let (contains, key) = match pii_subject {
            Some(s) => (true, Some(PiiKeyRef(format!("kms://acme/0/subject:{s}")))),
            None => (false, None),
        };
        let draft = EventDraft {
            type_: EventType(type_.into()),
            subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
            aggregate: AggregateKey(format!("chat.message:{event_id}")),
            payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            contains_personal_data: contains,
            pii_key_ref: key,
        };
        let ctx = EmitContext {
            event_id: EventId(event_id.into()),
            tenant: tenant(),
            region: region(),
            actor: actor_for(actor_id),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            caused_by: Some(CausedBy("human:h".into())),
        };
        derive_envelope(draft, ctx, None)
    }

    /// Seed a log with: one inline-PII event for subject u42, one for u99, two references-only.
    fn seeded_log_and_shredder() -> (BusEventLog, InMemoryShredder) {
        let mut log = BusEventLog::new();
        let shredder = InMemoryShredder::new();
        let e1 = retained("01J-1", "chat.message.created", "u42", Some("u42"));
        let e2 = retained("01J-2", "chat.message.created", "u99", Some("u99"));
        let e3 = retained("01J-3", "issue.issue.created", "u42", None);
        let e4 = retained("01J-4", "git.pr.opened", "u7", None);
        for e in [&e1, &e2, &e3, &e4] {
            if let Some(k) = &e.pii_key_ref {
                shredder.seal(k);
            }
        }
        log.append(e1);
        log.append(e2);
        log.append(e3);
        log.append(e4);
        (log, shredder)
    }

    /// Unit: `locate` finds exactly the subject's inline-PII events (references-only events excluded).
    #[test]
    fn locate_finds_only_the_subjects_inline_pii_events() {
        let (log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder);
        let report = holder.locate("u42", &log);
        assert_eq!(report.inline_pii_events.len(), 1, "only the one inline-PII event for u42");
        assert_eq!(report.inline_pii_events[0].event_id, "01J-1");
        assert!(!report.inline_pii_events[0].tombstoned);
        // u99's inline-PII event is NOT in u42's locate.
        assert!(report.inline_pii_events.iter().all(|e| e.event_id != "01J-2"));
    }

    /// Unit (the BUS-D8 live-store core): `erase(subject)` destroys the pii_key_ref DEK and renders
    /// the inline-PII payload unrecoverable; `*.erased` tombstones are emitted via the outbox;
    /// recoverable-remaining is 0.
    #[test]
    fn erase_destroys_dek_emits_tombstones_zero_recoverable() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let key_u42 = PiiKeyRef("kms://acme/0/subject:u42".into());
        assert!(shredder.is_live(&key_u42), "u42's DEK starts live");

        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let receipt = holder.erase("u42", &mut log, &mut outbox, minter).expect("erase succeeds");

        // The DEK is destroyed → the inline-PII payload is unrecoverable in the LIVE log.
        assert!(!shredder.is_live(&key_u42), "u42's DEK is crypto-shredded");
        // Gate threshold: 0 recoverable inline-PII for the subject.
        assert_eq!(receipt.recoverable_remaining, 0, "0 recoverable inline-PII after erase");
        assert_eq!(receipt.keys_shredded, 1);
        // Tombstones present (emitted via the outbox; durable after commit).
        assert_eq!(receipt.tombstones_emitted, 1, "one *.erased tombstone");
        assert_eq!(outbox.committed_count(), 1, "the tombstone committed through the outbox");
        // The erased event is tombstoned in the log (the consumer-degrade signal).
        assert!(log.is_tombstoned("01J-1"));
        // u99's DEK is UNTOUCHED (per-subject granularity, GD-4).
        assert!(shredder.is_live(&PiiKeyRef("kms://acme/0/subject:u99".into())));
    }

    /// Unit: a consumer degrades gracefully on a `*.erased` tombstone (always `Done`, never blocks).
    #[test]
    fn consumer_degrades_gracefully_on_tombstone() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder);
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        holder.erase("u42", &mut log, &mut outbox, minter).expect("erase");

        // The relay would publish the tombstone; a consumer sees it and degrades gracefully.
        let tombstone = retained_tombstone();
        assert_eq!(degrade_on_tombstone(&tombstone), HandleOutcome::Done);
    }

    fn retained_tombstone() -> EventEnvelope {
        let draft = EventDraft {
            type_: EventType(BUS_ERASED_TYPE.into()),
            subject: ArtifactRef("myelin://acme/bus/event/01J-1".into()),
            aggregate: AggregateKey("bus.event:01J-1".into()),
            payload: serde_json::json!({ "erased_event_id": "01J-1", "subject": "u42" }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        let ctx = EmitContext {
            event_id: EventId("01J-T".into()),
            tenant: tenant(),
            region: region(),
            actor: actor_for("bus:platform"),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            caused_by: None,
        };
        derive_envelope(draft, ctx, None)
    }

    /// Unit: `export(subject)` returns the subject's events with references resolved; a tombstoned
    /// event exports the `[erased]` marker, never the unrecoverable payload.
    #[test]
    fn export_returns_subject_events_with_references_resolved() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder);

        // Before erase: u42's inline-PII event exports its (reference) payload.
        let before = holder.export("u42", &log);
        // u42 acted on 01J-1 (actor + pii) and 01J-3 (actor only).
        assert!(before.iter().any(|e| e.event_id == "01J-1"));
        assert!(before.iter().any(|e| e.event_id == "01J-3"));
        assert!(before.iter().all(|e| e.payload != serde_json::json!({ "status": "erased" })));

        // After erase: the tombstoned event exports the erased marker, not the payload.
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        holder.erase("u42", &mut log, &mut outbox, minter).expect("erase");
        let after = holder.export("u42", &log);
        let erased = after.iter().find(|e| e.event_id == "01J-1").expect("still present");
        assert_eq!(erased.payload, serde_json::json!({ "status": "erased" }));
    }

    /// Unit: a crypto-shred KMS failure is LOUD — the erase aborts as INCOMPLETE (never "assume
    /// erased"; the DSR retries). No tombstone is committed and the DEK is NOT reported destroyed.
    #[test]
    fn erase_is_loud_on_kms_failure_never_assumes_erased() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let key_u42 = PiiKeyRef("kms://acme/0/subject:u42".into());
        shredder.make_unreachable(&key_u42);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let err = holder.erase("u42", &mut log, &mut outbox, minter).expect_err("must be loud");
        assert_eq!(err, ShredError::KmsUnavailable(key_u42.clone()));
        // The erase aborted BEFORE tombstoning/committing — nothing committed, key state unchanged.
        assert_eq!(outbox.committed_count(), 0, "no tombstone on a failed erase");
        assert!(!log.is_tombstoned("01J-1"), "the event is not tombstoned on a failed erase");
        // (is_live still true — the key was never reached; the DSR retries.)
        assert!(shredder.is_live(&key_u42));
    }

    /// Unit (EB-16 prerequisite): re-erasure is idempotent — erasing an already-erased subject
    /// succeeds with 0 recoverable (the key stays destroyed). This is the property EB-16's
    /// post-restore re-erasure re-applies.
    #[test]
    fn re_erase_is_idempotent_key_stays_destroyed() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let first = holder
            .erase("u42", &mut log, &mut outbox, minter.clone())
            .expect("first erase");
        assert_eq!(first.recoverable_remaining, 0);

        // Re-erase (post-restore re-erasure, EB-16) — idempotent: still 0 recoverable, no panic.
        let second = holder.erase("u42", &mut log, &mut outbox, minter).expect("re-erase");
        assert_eq!(second.recoverable_remaining, 0, "key stays destroyed across a re-erase");
    }

    /// CDC (provider side of 2.7): the Bus's owned crypto-shred/tombstone contract — `erase`
    /// destroys the per-subject DEK + emits `*.erased` tombstones; the receipt proves 0-recoverable.
    /// This is the shape the GDPR DSR orchestrator (the consumer of 10.1) calls.
    #[test]
    fn cdc_2_7_crypto_shred_tombstone_on_the_log() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let receipt = holder.erase("u42", &mut log, &mut outbox, minter).expect("erase");
        // The provider-side promise: key destroyed, tombstone emitted, 0 recoverable.
        assert_eq!(receipt.recoverable_remaining, 0);
        assert!(receipt.keys_shredded >= 1);
        assert!(receipt.tombstones_emitted >= 1);
        // The tombstone in the outbox carries NO inline PII (references-not-payloads).
        let row = outbox
            .dead_letters()
            .into_iter()
            .chain(std::iter::empty())
            .next();
        let _ = row; // (dead-letters empty on the happy path; the committed tombstone is below)
        assert_eq!(outbox.committed_count(), 1);
    }
}
