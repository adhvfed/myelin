//! # `pseudonym_erase` — `resolve_pseudonym` + `erase`: the per-subject crypto-shred lever (P-ID-20 → P-078)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §11 (the **`resolve_pseudonym` / `erase`** row of the frozen surface — `resolve_pseudonym(subject,
//! tenant)` + the `PersonalDataHolder` `erase(subject)`; **the pseudonym-map shred = DSR step 1**),
//! §12 (the **D8** assertion — restore to a consistent point → **no resurrected grants past an
//! erasure**; post-restore re-erasure runs → the **re-erasure receipt**), §2 (the **S2** row: per-
//! SUBJECT key = the erasure lever), recon §X-7 (the pseudonym-shred / erasure residual posture).
//!
//! **Contract-index:** rows **4.8** (`resolve_pseudonym`/`erase` — the RPC body + the crypto-shred,
//! OWNED here; the store + grammar half is P-ID-19 / P-077) and **10.8** (the PII-free erasure ledger
//! — CONSUMED/built here: an erasure is recorded so post-restore re-erasure can replay) +
//! **11.5 / STOR-D1/D2** (restore-verify; ID-D8 rides it).
//!
//! ## What this module ships (P-ID-20 — the erase BODY + the re-erasure path)
//! 1. **`erase(subject)` = DSR step 1** ([`StoreBackedCheck::erase_in`], wired below in `lib.rs`):
//!    *destroy the per-subject DEK (crypto-shred)* + *shred the pseudonym-map row* + *write the
//!    erasure to the PII-free [`PseudonymErasureLedger`]* (10.8) so post-restore re-erasure can
//!    replay. The opaque `principal_id` SURVIVES (it still attributes events; the public pseudonym
//!    handle survives — EI-04 §1 immutable-attribution split); only the `pseudonym → real_identity`
//!    resolution is destroyed.
//! 2. **`resolve_pseudonym(subject, tenant)`** ([`StoreBackedCheck::resolve_pseudonym_in`]) — returns
//!    the subject's PUBLIC per-tenant pseudonym rendering for a LIVE subject; **fails closed** (a
//!    typed `Erased` error) for an erased one (the row is shredded — never a fabricated handle).
//! 3. **Post-restore re-erasure** ([`StoreBackedCheck::re_erase_after_restore`]) — the ID-D8 path:
//!    after a restore replays an OLDER (pre-erasure) backup state, re-run the crypto-shred for every
//!    subject the [`PseudonymErasureLedger`] marks erased, and emit a **dated [`ReErasureReceipt`]**
//!    (the green artifact: 0 resurrected, the count re-erased, the run timestamp). The key stays
//!    destroyed across a restore (STOR-D3/STOR-D4); a restore resurrects nothing.
//!
//! ## The PII-free erasure ledger (10.8) — non-shred-erasable by construction
//! [`PseudonymErasureLedger`] records `(tenant, region, subject_principal_id, erased_at)` — the
//! **opaque** principal id ONLY, **never** the real identity (which is the thing being shredded). It
//! is therefore PII-free, so it does NOT itself need a crypto-shred lever (recording an erasure in it
//! is not a new PII liability), and — crucially — it must SURVIVE the very key destruction it records
//! and survive a restore, so that re-erasure can replay against it. This is the load-bearing 10.8
//! property the ID-D8 drill rides.
//!
//! ## The two mutation-tested mandatory-core properties (the prompt GATE)
//! - **The crypto-shred (DEK destroy)** — `erase` MUST destroy the per-subject DEK; a mutation that
//!   leaves the DEK recoverable (e.g. shreds only the row, not the key) MUST be caught: a post-erase
//!   resolve must fail LOUDLY (the key is gone), never return a fabricated subject, and the key must
//!   stay destroyed across a backup→restore round-trip.
//! - **The no-resurrection-post-restore path** — after a restore replays a pre-erasure backup,
//!   re-erasure MUST re-destroy the key; a mutation that skips re-erasure (so a restore resurrects the
//!   subject's resolvable real identity / its grants) MUST be caught.
//!
//! ## Floors named (this prompt's deferred follow-ons)
//! - **The audited history-rewrite erasure path (when a body must be EXPUNGED, not pseudonymised)**
//!   is the **M5 / on-demand follow-on** (recon §X-7 / 10.6, owned by the **Git + GDPR roadmaps**).
//!   This prompt ships the *identity half* — the pseudonym-map per-subject-DEK crypto-shred (DSR step
//!   1); the immutable-bytes history-rewrite is a separate, disruptive, hash-changing path named
//!   there. Recorded, not silent.
//! - **The cross-seam restore-verify GATE (STOR-D1/D2, the permanent gate, P-061/P-100)** is OWNED by
//!   Storage; ID-D8 RIDES it. This module ships the identity re-erasure replay the storage gate
//!   drives; the storage-side prod-scale restore-verify is its own gate (P-100).
//! - **The DSR-orchestrator-driven fan-out** (the GDPR side that CALLS this holder's `erase` across
//!   every H1–H18 holder) is the GDPR M1 spine (P-GA-06/-12/-14). This module is the *identity
//!   holder's* `erase` body the fan-out reaches; the orchestration is GDPR-owned.
//! - **The OUTBOX emit of an `iam.subject_erased` audit event** (so the tamper-evident audit log
//!   seals the erasure, P-GA-19/-20) is named: the receipt this module mints is the content-addressed
//!   body that event carries; the projection/emit lands with the audit consumer. Recorded.

use myelin_storage::{ContentHash, DekId, KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::pseudonym_store::PseudonymStore;
use myelin_identity::PrincipalId;

/// The stable holder/ledger name of the PII-free erasure ledger (10.8). Named so the ledger appears
/// as the durable, replayable record the re-erasure pass reads — never a string buried in a closure.
pub const ERASURE_LEDGER: &str = "identity_pseudonym_erasure_ledger";

/// **A typed erase/resolve failure (the 4.8 body half).** Every variant is a LOUD value — an erased
/// subject's resolve is an explicit [`PseudonymEraseError::Erased`], NEVER a fabricated handle or a
/// silent empty (the 0-fail-open invariant, identity §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudonymEraseError {
    /// `resolve_pseudonym` was called for a subject whose mapping has been crypto-shredded (the row
    /// shredded + the per-subject DEK destroyed). Fails CLOSED: the real-identity resolution is gone
    /// forever — we return THIS, never a fabricated `<pseudonym>@<tenant>.noreply`.
    Erased {
        /// the opaque principal id (PII-free — it survives erasure and still attributes events).
        subject: String,
    },
    /// `resolve_pseudonym` was called for a subject that has no S2 mapping in the verified scope (it
    /// was never registered, or it is a different tenant/region). Distinct from `Erased` so an
    /// operator can tell "never existed" from "erased".
    NoMapping {
        /// the opaque principal id queried.
        subject: String,
    },
}

impl core::fmt::Display for PseudonymEraseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PseudonymEraseError::Erased { subject } => write!(
                f,
                "subject `{subject}` is ERASED (the pseudonym-map row + per-subject DEK are \
                 crypto-shredded) — its real identity is unrecoverable; resolve fails CLOSED, never \
                 a fabricated handle (the opaque principal_id still attributes events)"
            ),
            PseudonymEraseError::NoMapping { subject } => write!(
                f,
                "subject `{subject}` has no pseudonym mapping in the verified (tenant, region) scope \
                 (never registered, or a different partition) — refused"
            ),
        }
    }
}

impl std::error::Error for PseudonymEraseError {}

/// **An [`ErasureReceipt`] — the dated, PII-free proof that an `erase` (DSR step 1) completed.**
///
/// References-not-payloads: it names the op, the destroyed per-subject DEK class (so audit can prove
/// WHICH key was shredded), the run timestamp, and a content-address — **never** the real identity.
/// The content-address (`blake3:<hex>`) is the audit-log hash-link the tamper-evident Merkle seal
/// (P-GA-20) binds; the OUTBOX emit of the `iam.subject_erased` audit event that carries it is the
/// named follow-on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureReceipt {
    /// The opaque principal id erased (PII-free — survives erasure, still attributes events).
    pub subject: PrincipalId,
    /// The verified tenant partition the erasure ran under.
    pub tenant: TenantId,
    /// The residency region partition (12.1).
    pub region: Region,
    /// The per-subject DEK class destroyed (the crypto-shred lever; `subject:<id>`). Naming it lets
    /// audit prove exactly which key the Art. 17 erase shredded.
    pub shredded_dek_class: String,
    /// `true` iff the per-subject DEK was present-and-destroyed by THIS call (idempotency: a re-erase
    /// of an already-shredded subject is a no-op-but-recorded — the receipt still seals).
    pub dek_destroyed: bool,
    /// `true` iff the pseudonym-map row was present-and-shredded by THIS call.
    pub row_shredded: bool,
    /// The erase timestamp (the dated artifact — EI-01 §3 "prove it": a receipt without a date is
    /// not a green artifact).
    pub erased_at: myelin_events::Timestamp,
    /// The content-address of the receipt body (`blake3:<hex>`) — the audit hash-link (Merkle seal
    /// is P-GA-20). Deterministic over `(op, subject, tenant, region, dek_class, erased_at)`.
    pub content_hash: String,
}

impl ErasureReceipt {
    /// Build a receipt for an erase, computing its content-address over the PII-free fields. The
    /// address is deterministic (so the same erasure receipt content-addresses identically) and
    /// PII-free (it digests the OPAQUE principal id + the partition + the dek class + the date).
    pub fn for_erase(
        subject: PrincipalId,
        tenant: TenantId,
        region: Region,
        shredded_dek_class: String,
        dek_destroyed: bool,
        row_shredded: bool,
        erased_at: myelin_events::Timestamp,
    ) -> ErasureReceipt {
        let body = format!(
            "erase|{}|{}|{}|{}|{}",
            subject.0, tenant.0, region.0, shredded_dek_class, erased_at.0
        );
        let content_hash = ContentHash::blake3(body.as_bytes()).to_multihash_string();
        ErasureReceipt {
            subject,
            tenant,
            region,
            shredded_dek_class,
            dek_destroyed,
            row_shredded,
            erased_at,
            content_hash,
        }
    }
}

/// **A [`ReErasureReceipt`] — the dated ID-D8 green artifact.** After a restore replays an older
/// (pre-erasure) backup, the re-erasure pass replays the [`PseudonymErasureLedger`] and re-runs the
/// crypto-shred for every recorded subject. THIS is the artifact the drill asserts: a dated receipt
/// naming the re-erased count and the **0 resurrected** invariant (no subject's real identity / grants
/// survive the restore). EI-01 §3: a system that survives a drill but emits no signal has FAILED it —
/// this receipt is the signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReErasureReceipt {
    /// The verified tenant partition the re-erasure ran under.
    pub tenant: TenantId,
    /// The residency region partition.
    pub region: Region,
    /// The number of subjects the ledger marked erased that this pass re-ran the crypto-shred for.
    /// This is the count restored-then-re-erased.
    pub re_erased: usize,
    /// **The number of subjects STILL recoverable AFTER the re-erasure pass — the ID-D8 quantified
    /// threshold. MUST be 0.** A non-zero value is a RED drill (a subject the re-erasure could not
    /// re-shred), recorded honestly, never softened. The gate reads THIS field.
    pub resurrected: usize,
    /// The number of subjects the restore brought BACK (recoverable BEFORE the re-erasure pass ran) —
    /// the honest "what the older backup resurrected" signal. Informational: the re-erasure pass
    /// re-shreds these to 0 (so `resurrected` ends at 0). EI-01 §3: observability is part of the pass.
    pub pre_pass_resurrected: usize,
    /// The per-receipt erase receipts (one per re-erased subject) — the dated, content-addressed
    /// proof each subject was re-shredded.
    pub per_subject: Vec<ErasureReceipt>,
    /// The re-erasure run timestamp (the dated artifact).
    pub ran_at: myelin_events::Timestamp,
}

impl ReErasureReceipt {
    /// `true` iff the re-erasure pass is GREEN: after the pass, **0 subjects remain recoverable**
    /// (every ledger-recorded subject is shredded again). This is the ID-D8 pass condition (the gate
    /// reads THIS): a restore that replayed a pre-erasure backup resurrects NOTHING.
    pub fn is_green(&self) -> bool {
        self.resurrected == 0
    }

    /// Carry the pre-pass resurrection count (the honest "what the restore brought back" signal) into
    /// the receipt, returning it (a fluent builder used at the call site).
    pub(crate) fn with_pre_pass_resurrected(mut self, pre_pass: usize) -> ReErasureReceipt {
        self.pre_pass_resurrected = pre_pass;
        self
    }

    /// A one-line dated summary for the green artifact / scorecard (EI-01 §3 — observability is part
    /// of the pass). PII-free (it names counts + the partition + the date, never a subject's identity).
    pub fn summary(&self) -> String {
        format!(
            "ID-D8 re-erasure [{}]: tenant={} region={} re_erased={} \
             pre_pass_resurrected={} resurrected={} → {}",
            self.ran_at.0,
            self.tenant.0,
            self.region.0,
            self.re_erased,
            self.pre_pass_resurrected,
            self.resurrected,
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

/// The ledger's partitioned inner map: `(tenant, region)` → (opaque subject id → entry). Named so
/// the `MutexGuard` over it is not a "very complex type" at every accessor.
type LedgerByPartition = BTreeMap<(String, String), BTreeMap<String, ErasureLedgerEntry>>;

/// One PII-free ledger entry (10.8). Holds ONLY the opaque principal id + the partition + the date —
/// never the real identity (which is the thing being shredded). The per-subject DEK CLASS is carried
/// so re-erasure can re-destroy exactly the right key without re-reading the (shredded) map row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureLedgerEntry {
    /// the opaque principal id erased (PII-free).
    pub subject: PrincipalId,
    /// the per-subject DEK class to (re-)destroy (`subject:<id>`).
    pub dek_class: myelin_storage::KeyClass,
    /// the erase timestamp.
    pub erased_at: myelin_events::Timestamp,
}

/// **The PII-free per-subject erasure ledger (contract 10.8).** Records every per-subject erasure so
/// post-restore re-erasure can replay it. PII-free by construction (opaque principal id + partition +
/// date only), and **non-shred-erasable** (it must SURVIVE the key destruction it records + survive a
/// restore — recording an erasure is not a new PII liability). `(tenant, region)`-partitioned, like
/// every Id store.
///
/// A cloneable handle over shared state (so the erase path + the re-erasure pass + the live service
/// read the SAME ledger).
#[derive(Clone, Default)]
pub struct PseudonymErasureLedger {
    /// `(tenant, region)` partition → (opaque subject id → entry). The OUTER map is the partition (no
    /// cross-tenant read path). Keyed by the opaque subject so a re-erase is idempotent (the same
    /// subject records once; a re-erase updates the date, never duplicates).
    inner: Arc<Mutex<LedgerByPartition>>,
}

impl PseudonymErasureLedger {
    /// A fresh, empty ledger.
    pub fn new() -> PseudonymErasureLedger {
        PseudonymErasureLedger::default()
    }

    /// Record an erasure (10.8) — PII-free, idempotent. Built from a verified [`TenantScope`] so the
    /// record carries its `(tenant, region)` predicate (the tenant-predicate floor). Recording the
    /// same subject again updates the timestamp, never duplicates (idempotent-by-construction).
    pub fn record(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
        dek_class: myelin_storage::KeyClass,
        erased_at: myelin_events::Timestamp,
    ) {
        let part = Self::part_key(scope);
        let entry = ErasureLedgerEntry {
            subject: subject.clone(),
            dek_class,
            erased_at,
        };
        self.lock()
            .entry(part)
            .or_default()
            .insert(subject.0.clone(), entry);
    }

    /// Every erasure entry in a `(tenant, region)` partition (the re-erasure pass replays THIS). No
    /// cross-partition accessor exists — a read is scoped to one verified `(tenant, region)`.
    pub fn entries_in(&self, scope: &TenantScope) -> Vec<ErasureLedgerEntry> {
        self.lock()
            .get(&Self::part_key(scope))
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// `true` iff the subject is recorded erased in the verified scope (the ledger remembers an
    /// erasure even after the map row + DEK are gone — the load-bearing 10.8 property).
    pub fn is_erased(&self, scope: &TenantScope, subject: &PrincipalId) -> bool {
        self.lock()
            .get(&Self::part_key(scope))
            .map(|m| m.contains_key(&subject.0))
            .unwrap_or(false)
    }

    fn part_key(scope: &TenantScope) -> (String, String) {
        (scope.tenant().0.clone(), scope.region().0.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LedgerByPartition> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// **The erase engine — the 4.8 crypto-shred body, shared by `erase_in` + `re_erase_after_restore`.**
///
/// One primitive (EI-01 §7): the FIRST erase and the post-restore re-erase run the IDENTICAL shred
/// (destroy the per-subject DEK + shred the map row), so "cold == live" — a restore that replayed a
/// pre-erasure backup is brought back to the erased state by re-running the SAME code path, never a
/// bespoke recovery path. The ledger is written by the FIRST erase; re-erasure REPLAYS it.
pub(crate) struct EraseEngine;

impl EraseEngine {
    /// Run the per-subject crypto-shred over the S2 store + KMS: destroy the per-subject DEK +
    /// shred the pseudonym-map row, returning `(dek_destroyed, row_shredded, dek_class)`.
    ///
    /// Idempotent: a re-erase of an already-shredded subject destroys nothing (the key is gone) +
    /// shreds nothing (the row is gone) and reports `(false, false, …)` — but it is still a valid,
    /// receiptable run (the subject IS erased; the no-op is correct).
    pub(crate) fn shred(
        store: &PseudonymStore,
        kms: &KmsEngine,
        scope: &TenantScope,
        subject: &PrincipalId,
        dek_class: &myelin_storage::KeyClass,
    ) -> (bool, bool, String) {
        // (1) Destroy the per-subject DEK (the crypto-shred lever, 11.4 GD-4). After this, the sealed
        //     real-identity link is forever unwrappable in DBs + backups (the backup_snapshot excludes
        //     a DEK with no live KEK; a destroyed DEK is removed outright). A resolve now fails LOUDLY.
        let dek_id = DekId::new(scope.tenant().clone(), dek_class.clone());
        let dek_destroyed = kms.destroy_dek(&dek_id);
        // (2) Shred the pseudonym-map row (the row + the sealed link + the reverse index entry). The
        //     PUBLIC pseudonym handle does NOT survive in S2 after a full erase — the resolvable
        //     mapping is gone; historic attribution lives in the already-emitted immutable git bytes
        //     (which carry the public handle), never by re-reading S2 (EI-04 §1).
        let row_shredded = store.shred_row(scope, subject);
        (dek_destroyed, row_shredded, dek_class.as_token())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalKind};
    use myelin_storage::KeyClass;

    fn scope(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    fn ts(s: &str) -> myelin_events::Timestamp {
        myelin_events::Timestamp(s.into())
    }

    /// **The PII-free erasure ledger (10.8) records + remembers an erasure — and survives the key
    /// destruction it records.** A record persists (the load-bearing 10.8 property) so re-erasure can
    /// replay it; it is keyed by the OPAQUE principal id (PII-free).
    #[test]
    fn erasure_ledger_records_and_remembers() {
        let ledger = PseudonymErasureLedger::new();
        let s = scope("acme", "eu-west");
        let subject = PrincipalId("p:alice".into());
        assert!(!ledger.is_erased(&s, &subject), "not erased before record");
        ledger.record(
            &s,
            &subject,
            KeyClass::Subject("p:alice".into()),
            ts("2026-06-19T00:00:00Z"),
        );
        assert!(ledger.is_erased(&s, &subject), "the ledger remembers the erasure");
        let entries = ledger.entries_in(&s);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].subject, subject);
        assert_eq!(entries[0].dek_class, KeyClass::Subject("p:alice".into()));
    }

    /// **The ledger is `(tenant, region)`-partitioned — no cross-tenant read path.** An erasure
    /// recorded under `acme` is invisible to a read under `globex` (and under a different region).
    #[test]
    fn erasure_ledger_is_partitioned() {
        let ledger = PseudonymErasureLedger::new();
        let acme = scope("acme", "eu-west");
        let globex = scope("globex", "eu-west");
        let acme_us = scope("acme", "us-east");
        let subject = PrincipalId("p:alice".into());
        ledger.record(&acme, &subject, KeyClass::Subject("p:alice".into()), ts("t"));
        assert!(ledger.is_erased(&acme, &subject));
        assert!(!ledger.is_erased(&globex, &subject), "no cross-tenant ledger read");
        assert!(!ledger.is_erased(&acme_us, &subject), "no cross-region ledger read");
        assert!(ledger.entries_in(&globex).is_empty());
    }

    /// **Recording is idempotent (re-erase updates the date, never duplicates).** The ledger keys on
    /// the opaque subject, so a re-erase (post-restore re-erasure) does not bloat the ledger.
    #[test]
    fn erasure_ledger_record_is_idempotent() {
        let ledger = PseudonymErasureLedger::new();
        let s = scope("acme", "eu-west");
        let subject = PrincipalId("p:alice".into());
        ledger.record(&s, &subject, KeyClass::Subject("p:alice".into()), ts("t1"));
        ledger.record(&s, &subject, KeyClass::Subject("p:alice".into()), ts("t2"));
        let entries = ledger.entries_in(&s);
        assert_eq!(entries.len(), 1, "a re-record does not duplicate");
        assert_eq!(entries[0].erased_at, ts("t2"), "the timestamp updates");
    }

    /// **The erase receipt is dated, content-addressed, and PII-free.** It names the OPAQUE subject +
    /// the partition + the destroyed dek class + the date; the content-hash is a deterministic
    /// `blake3:<hex>` over those PII-free fields (so the same erasure addresses identically). The
    /// receipt body NEVER carries a real identity.
    #[test]
    fn erase_receipt_is_dated_content_addressed_and_pii_free() {
        let r = ErasureReceipt::for_erase(
            PrincipalId("p:alice".into()),
            TenantId("acme".into()),
            Region("eu-west".into()),
            "subject:p:alice".into(),
            true,
            true,
            ts("2026-06-19T00:00:00Z"),
        );
        assert_eq!(r.erased_at, ts("2026-06-19T00:00:00Z"), "dated");
        assert!(r.content_hash.starts_with("blake3:"), "content-addressed: {}", r.content_hash);
        // Deterministic: the same erasure content-addresses identically.
        let r2 = ErasureReceipt::for_erase(
            PrincipalId("p:alice".into()),
            TenantId("acme".into()),
            Region("eu-west".into()),
            "subject:p:alice".into(),
            true,
            true,
            ts("2026-06-19T00:00:00Z"),
        );
        assert_eq!(r.content_hash, r2.content_hash, "deterministic content-address");
        // A different subject ⇒ a different address (the digest covers the opaque id).
        let r3 = ErasureReceipt::for_erase(
            PrincipalId("p:bob".into()),
            TenantId("acme".into()),
            Region("eu-west".into()),
            "subject:p:bob".into(),
            true,
            true,
            ts("2026-06-19T00:00:00Z"),
        );
        assert_ne!(r.content_hash, r3.content_hash);
    }

    /// **The re-erasure receipt is GREEN iff 0 resurrected (the ID-D8 threshold); its summary is a
    /// dated PII-free artifact.** `is_green` reads the post-pass `resurrected` count; the summary
    /// names counts + partition + date, never a subject's identity.
    #[test]
    fn re_erasure_receipt_green_iff_zero_resurrected() {
        let green = ReErasureReceipt {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            re_erased: 3,
            resurrected: 0,
            pre_pass_resurrected: 3,
            per_subject: Vec::new(),
            ran_at: ts("2026-06-19T00:00:00Z"),
        };
        assert!(green.is_green(), "0 resurrected ⇒ green");
        assert!(green.summary().contains("GREEN"));
        assert!(green.summary().contains("re_erased=3"));
        assert!(green.summary().contains("2026-06-19T00:00:00Z"), "dated");

        let red = ReErasureReceipt {
            resurrected: 1,
            ..green
        };
        assert!(!red.is_green(), "a resurrected subject ⇒ RED (never softened)");
        assert!(red.summary().contains("RED"));
    }

    /// **The erase errors render LOUD, distinct, non-empty messages.** An `Erased` resolve and a
    /// `NoMapping` resolve are distinguishable in the audit log (a mutation blanking either is caught).
    #[test]
    fn erase_errors_render_loud_distinct_messages() {
        let erased = PseudonymEraseError::Erased {
            subject: "p:alice".into(),
        }
        .to_string();
        let no_map = PseudonymEraseError::NoMapping {
            subject: "p:bob".into(),
        }
        .to_string();
        assert!(erased.contains("ERASED"), "{erased}");
        assert!(erased.contains("fails CLOSED"), "{erased}");
        assert!(no_map.contains("no pseudonym mapping"), "{no_map}");
        assert_ne!(erased, no_map);
    }
}
