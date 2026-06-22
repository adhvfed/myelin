//! # `reerase` — the erasure ledger hook for post-restore re-erasure (EB-16 / P-093)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §4.8 (retention + crypto-shred + tombstones — **post-restore re-erasure fan-out**: the key stays
//! destroyed even after a backup is restored).
//!
//! **Contract-index cluster:** row **10.8** (the **erasure ledger** — PII-free, non-shred-erasable;
//! opaque subject id + the keys shredded; drives post-restore re-erasure GD-14 — **CONSUMED** here:
//! the Bus PARTICIPATES in the re-erasure fan-out) + row **11.5** (backup / restore cross-seam — the
//! **event-log offset is the cross-seam cursor**; `post_restore_reerase` — the Bus's leg).
//!
//! ## What this module is (the key stays destroyed across a restore)
//! EB-15 ([`crate::holder`]) shipped the live-store erasure: `erase(subject)` crypto-shreds the
//! subject's RARE inline-PII DEK (destroy the key → the live-log ciphertext is unrecoverable) and
//! emits `*.erased` tombstones. But an append-only log lives in BACKUPS too, and a `restore` of an
//! OLDER backup — taken BEFORE the erase — can bring a still-live DEK (and its sealed inline-PII
//! ciphertext) back. external-insights/04 §1 names the invariant: **the key stays destroyed even
//! after a backup is restored.** This module is the mechanism that holds it.
//!
//! The resolution (storage §7 / GDPR 10.8, the SAME shape identity's `PseudonymErasureLedger` +
//! `re_erase_after_restore` uses, EI-01 §7 cold == live): a **PII-free erasure ledger**
//! ([`BusErasureLedger`]) durably records — outside the crypto-shred blast radius, so it survives the
//! key destruction it records AND a restore — *which opaque subject was erased and which key refs
//! were shredded*. After Storage restores an older backup, the Bus's holder REPLAYS the ledger
//! ([`BusHolder::re_erase_after_restore`]): for every ledger-listed subject it re-runs the IDENTICAL
//! [`BusHolder::erase`] crypto-shred (idempotent — destroying an already-dead key is a no-op success,
//! [`crate::holder::InlinePiiShredder`] contract), re-destroying any key the restore resurrected and
//! re-emitting the `*.erased` tombstones for any log row the restore brought back. The result is a
//! dated [`ReErasureReceipt`] proving **0 resurrected** inline-PII keys post-restore (the GATE
//! threshold).
//!
//! ## Why the ledger is PII-free and non-shred-erasable (10.8)
//! The ledger is the ONE thing that must outlive a crypto-shred: if erasing a subject also erased the
//! record that the subject was erased, a restore could resurrect the subject with nothing to re-apply.
//! So the ledger carries only the OPAQUE subject discriminator (`subject:<id>` — already the
//! pseudonymous handle, never real-identity PII) + the opaque `pii_key_ref` strings (a key NAME, not
//! key material) + a timestamp. It is itself NOT a `PersonalDataHolder` target (a DSR does not erase
//! the fact-of-erasure record — that would be self-defeating); this is the §4.4 "non-shred-erasable"
//! property the contract names.
//!
//! ## DEVIATION (EI-01 §1, documented — the DAG, code-wins-over-docs)
//! The same §2.9 DAG constraint [`crate::holder`] documents applies: the erasure ledger CONTRACT
//! (10.8) is OWNED by GDPR/Audit (`myelin-gdpr`), which is DOWNSTREAM of `myelin-events`; and the
//! restore that TRIGGERS the re-erasure (11.5) is OWNED by Storage (`myelin-storage`), also
//! downstream. `myelin-events` therefore cannot name `gdpr::ErasureLedger` or `storage`'s
//! `RestoreVerifyGate` without inverting the DAG. So this module ships the Bus's **participation
//! mechanism** — its own PII-free [`BusErasureLedger`] (the Bus's slice of the 10.8 record, recording
//! the BUS keys it shredded) + the [`BusHolder::re_erase_after_restore`] replay hook the cross-seam
//! restore drives. The thin adapter that registers THIS hook as the Bus's leg of the GDPR-owned
//! global erasure ledger + the Storage `post_restore_reerase` cross-seam call lives downstream in the
//! GDPR DSR orchestration (**P-GA-06 / P-106** — the upstream-store holder orchestration) and the
//! Storage restore-verify wiring (**P-ST-14 / P-100** — the post-restore re-erasure pass): the
//! **named floors**. This module owns the Bus mechanism those wire to.
//!
//! ## Floors named (stubbed/deferred → filling prompt)
//! - **The cross-seam trigger** (Storage's `restore` calling every holder's `re_erase_after_restore`
//!   over the GDPR-owned global ledger) is **P-ST-14 (P-100)** + **P-GA-06 (P-106)**. This module
//!   ships the Bus's hook + drills it in isolation against a modeled restore.
//! - **Contract row 10.8 stays `deferred` (landing P-115)** in the coverage manifest: the full
//!   provider⇄consumer pair (GDPR mints + owns the global ledger) lands at GA-15. THIS prompt ships
//!   the Bus's CONSUMER-SIDE participation CDC (`tests/cdc_10_8_bus_reerase.rs`) — the Bus's leg, not
//!   the GDPR provider side; the row flips to `covered` only when the GDPR provider lands (P-115).

use crate::holder::{BusEventLog, BusHolder, EraseReceipt, InlinePiiShredder, ShredError};
use crate::outbox::{IdMinter, OutboxStore};
use crate::{PiiKeyRef, Region, TenantId, Timestamp};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// ════════════════════════════════════════════════════════════════════════════════════════════
// The PII-free, non-shred-erasable erasure ledger (contract 10.8, CONSUMED)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// One ledger entry — a PII-free record that a subject was erased and which Bus key refs were
/// shredded (contract 10.8 / GDPR §4.4). It carries ONLY opaque ids (the `subject:<id>`
/// discriminator + the `pii_key_ref` strings, which are key NAMES not key material) + a timestamp —
/// never any payload, never real-identity PII. It must survive the crypto-shred it records AND a
/// restore (it is non-shred-erasable), so the re-erasure pass can replay it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasedSubject {
    /// The opaque subject discriminator that was erased (the `subject:<id>` half of the per-subject
    /// `pii_key_ref` — already pseudonymous, never real-identity PII).
    pub subject: String,
    /// The DISTINCT Bus inline-PII key refs that were crypto-shredded for this subject. A re-erasure
    /// re-destroys each (idempotent). Sorted/deduped — a key NAME, never key material.
    pub key_refs: Vec<PiiKeyRef>,
    /// When the erasure was recorded (the audit timestamp). PII-free.
    pub erased_at: Timestamp,
}

/// The Bus's slice of the PII-free erasure ledger (contract 10.8, **CONSUMED**). It durably records
/// which subjects the Bus erased + which key refs it shredded, so [`BusHolder::re_erase_after_restore`]
/// can replay them after a restore. PII-free + non-shred-erasable (it must outlive the keys it records
/// and survive a restore — that is the whole point: a restored backup must not be able to resurrect a
/// subject the ledger remembers erasing).
///
/// In the real binding the Bus's `record` writes into the GDPR-owned global ledger (10.8) through the
/// downstream adapter (P-GA-06, the floor); here it is an in-cell `(tenant, region)`-scoped record
/// (the Bus never crosses a cell — residency-pin, EB-13). The map is keyed by subject so a re-erase of
/// an already-recorded subject MERGES key refs (idempotent record).
#[derive(Clone)]
pub struct BusErasureLedger {
    tenant: TenantId,
    region: Region,
    /// subject discriminator → (the distinct key refs shredded, the first erased-at). A `BTreeMap` so
    /// the replay order is deterministic (sorted by subject) — the drill artifact is reproducible.
    entries: Arc<Mutex<BTreeMap<String, ErasedSubject>>>,
}

impl BusErasureLedger {
    /// A fresh ledger for one `(tenant, region)` cell.
    pub fn new(tenant: TenantId, region: Region) -> Self {
        BusErasureLedger {
            tenant,
            region,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// The cell this ledger is scoped to (the Bus never crosses it).
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    /// The region this ledger is scoped to.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// Record that `subject` was erased, shredding `key_refs` (contract 10.8). Idempotent: recording a
    /// subject already present MERGES the key refs (a later erase may have located more keys) and
    /// keeps the FIRST `erased_at`. Called by the DSR orchestrator after a successful
    /// [`BusHolder::erase`] (or by [`BusHolder::erase_and_record`], which does both atomically).
    pub fn record(&self, subject: &str, key_refs: &[PiiKeyRef], erased_at: Timestamp) {
        let mut g = self.entries.lock().expect("erasure ledger poisoned");
        let entry = g
            .entry(subject.to_string())
            .or_insert_with(|| ErasedSubject {
                subject: subject.to_string(),
                key_refs: Vec::new(),
                erased_at: erased_at.clone(),
            });
        for k in key_refs {
            if !entry.key_refs.contains(k) {
                entry.key_refs.push(k.clone());
            }
        }
        entry.key_refs.sort_by(|a, b| a.0.cmp(&b.0));
    }

    /// Whether the ledger remembers erasing `subject` (the fail-closed read EB-15's `resolve` uses —
    /// "erased" vs "never seen"). True once `record`ed; a restore CANNOT clear it (non-shred-erasable).
    pub fn is_erased(&self, subject: &str) -> bool {
        self.entries
            .lock()
            .expect("erasure ledger poisoned")
            .contains_key(subject)
    }

    /// Every recorded erasure, in deterministic (subject-sorted) order — what the re-erasure pass
    /// replays. PII-free.
    pub fn entries(&self) -> Vec<ErasedSubject> {
        self.entries
            .lock()
            .expect("erasure ledger poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// How many subjects the ledger has recorded as erased.
    pub fn len(&self) -> usize {
        self.entries.lock().expect("erasure ledger poisoned").len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The re-erasure receipt (the STOR-D1/D2 Bus-leg artifact)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated artifact a post-restore re-erasure pass returns (the Bus's leg of STOR-D1/D2). It is the
/// PROOF the key stays destroyed across a restore: how many subjects were re-erased, how many keys the
/// restore RESURRECTED (live again after the restore — the honest "what the backup brought back"
/// signal), and the post-pass `resurrected` count which MUST be **0** (the gate threshold). PII-free:
/// opaque subject ids + counts, never payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReErasureReceipt {
    /// The cell the re-erasure ran within (the Bus never crosses it).
    pub tenant: TenantId,
    /// The region the re-erasure ran within.
    pub region: Region,
    /// How many ledger-listed subjects were replayed through the re-erasure crypto-shred.
    pub re_erased_subjects: usize,
    /// How many distinct inline-PII keys the RESTORE resurrected (were live again BEFORE the
    /// re-erasure pass re-destroyed them) — the honest signal of what the older backup brought back.
    pub keys_resurrected_by_restore: usize,
    /// How many tombstones the pass re-emitted for log rows the restore brought back (the restored
    /// rows lost their tombstone — re-tombstone them so consumers degrade gracefully again).
    pub tombstones_re_emitted: usize,
    /// **THE GATE READING:** how many of the ledger's keys are STILL recoverable AFTER the re-erasure
    /// pass — MUST be **0** (the re-erasure re-destroyed everything the restore resurrected). A
    /// non-zero value is a RED drill: a restored backup resurrected an erased subject's inline-PII key.
    pub resurrected: usize,
    /// When the pass ran (the dated artifact).
    pub ran_at: Timestamp,
}

impl ReErasureReceipt {
    /// Whether the Bus's restore-verify leg is GREEN: 0 resurrected inline-PII keys post-restore.
    pub fn is_green(&self) -> bool {
        self.resurrected == 0
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The BusHolder hooks: record-on-erase + replay-after-restore
// ════════════════════════════════════════════════════════════════════════════════════════════

impl<S: InlinePiiShredder> BusHolder<S> {
    /// `erase(subject)` AND record it in the PII-free erasure ledger (10.8) — the durable record that
    /// drives post-restore re-erasure. This is the path the DSR orchestrator calls so an erasure is
    /// remembered across a restore. It runs the IDENTICAL [`BusHolder::erase`] crypto-shred, then —
    /// only if the erase SUCCEEDED (loud on failure; never record an INCOMPLETE erase) — records the
    /// distinct key refs that were located + shredded into the ledger.
    ///
    /// The keys recorded are the subject's located inline-PII key refs (the same set `erase`
    /// shredded). Recording is idempotent (a re-erase merges, keeps the first timestamp), so calling
    /// this twice is well-defined.
    pub fn erase_and_record(
        &self,
        subject: &str,
        log: &mut BusEventLog,
        tx: &mut OutboxStore,
        minter: Arc<dyn IdMinter>,
        ledger: &BusErasureLedger,
        now: Timestamp,
    ) -> Result<EraseReceipt, ShredError> {
        // Locate the subject's key refs BEFORE the erase tombstones the rows (locate reads the log;
        // erase mutates it). These are exactly the keys the erase will shred — record them so the
        // re-erasure pass can re-destroy them after a restore resurrects them.
        let report = self.locate(subject, log);
        let mut distinct: Vec<PiiKeyRef> = Vec::new();
        for ev in &report.inline_pii_events {
            if !distinct.contains(&ev.pii_key_ref) {
                distinct.push(ev.pii_key_ref.clone());
            }
        }

        // Run the IDENTICAL live-store erase (loud on a KMS failure — aborts as INCOMPLETE).
        let receipt = self.erase(subject, log, tx, minter)?;

        // Only on a SUCCESSFUL erase: record into the PII-free, non-shred-erasable ledger (10.8).
        // (Even if `distinct` is empty — references-not-payloads — we record the subject so the
        // ledger remembers it was erased; the re-erasure pass is then a confirmed no-op for it.)
        ledger.record(subject, &distinct, now);
        Ok(receipt)
    }

    /// **Post-restore re-erasure (EB-16 / GD-14) — the key stays destroyed across a restore.** After
    /// Storage restores an OLDER backup (one taken before an erase), REPLAY the PII-free erasure
    /// ledger (10.8): for every subject the ledger marks erased, re-run the IDENTICAL crypto-shred
    /// (destroy any inline-PII DEK the restore resurrected) + re-emit `*.erased` tombstones for any
    /// log row the restore brought back. Returns a dated [`ReErasureReceipt`] — the Bus's leg of
    /// STOR-D1/D2 (the threshold: **0 resurrected** inline-PII keys post-restore).
    ///
    /// "Cold == live" (EI-01 §7): re-erasure runs the SAME [`BusHolder::erase`] the first erase did,
    /// not a bespoke recovery path. A key is **resurrected** (the RED condition) iff, after the
    /// restore, its DEK is live again ([`InlinePiiShredder::is_live`]) — which this pass re-shreds. We
    /// probe `is_live` BEFORE re-erasing to report the honest "what the restore brought back" count,
    /// then re-confirm 0 live AFTER the pass for the gate reading.
    ///
    /// `log` + `tx` are the RESTORED post-restore state (the older backup's log, with its rows back
    /// and — crucially — without the tombstones a post-backup erase had added). The pass re-tombstones
    /// and re-emits through the outbox so live consumers degrade gracefully on the restored rows.
    pub fn re_erase_after_restore(
        &self,
        ledger: &BusErasureLedger,
        log: &mut BusEventLog,
        tx: &mut OutboxStore,
        minter: Arc<dyn IdMinter>,
        now: Timestamp,
    ) -> Result<ReErasureReceipt, ShredError> {
        let entries = ledger.entries();

        // (a) PROBE: how many of the ledger's keys did the restore RESURRECT (live again)? This is the
        //     honest "what the backup brought back" signal — count BEFORE re-shredding.
        let mut keys_resurrected_by_restore = 0usize;
        for entry in &entries {
            for key in &entry.key_refs {
                if self.shredder.is_live(key) {
                    keys_resurrected_by_restore += 1;
                }
            }
        }

        // (b) REPLAY: for every ledger-listed subject, re-run the IDENTICAL crypto-shred + tombstone
        //     emit (cold == live). Idempotent: a key already dead is a no-op success; a row already
        //     tombstoned re-tombstones harmlessly. Loud on a real KMS failure — re-erasure is part of
        //     the DSR, never "assume erased".
        let mut tombstones_re_emitted = 0usize;
        for entry in &entries {
            let receipt = self.erase(&entry.subject, log, tx, minter.clone())?;
            tombstones_re_emitted += receipt.tombstones_emitted;
        }

        // (c) RE-CONFIRM: after the pass, NONE of the ledger's keys may be live (0 resurrected). This
        //     is the gate reading — the re-erasure re-destroyed everything the restore resurrected.
        let mut resurrected = 0usize;
        for entry in &entries {
            for key in &entry.key_refs {
                if self.shredder.is_live(key) {
                    resurrected += 1;
                }
            }
        }

        Ok(ReErasureReceipt {
            tenant: ledger.tenant().clone(),
            region: ledger.region().clone(),
            re_erased_subjects: entries.len(),
            keys_resurrected_by_restore,
            tombstones_re_emitted,
            resurrected,
            ran_at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EmitContext;
    use crate::holder::InMemoryShredder;
    use crate::outbox::MonotonicMinter;
    use crate::{
        derive_envelope, Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EventDraft, EventId,
        EventType, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }
    fn actor_for(id: &str) -> Actor {
        Actor(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    /// Build a retained inline-PII envelope sealed under `subject`'s per-subject DEK.
    fn inline_pii(event_id: &str, subject: &str) -> crate::EventEnvelope {
        let draft = EventDraft {
            type_: EventType("chat.message.created".into()),
            subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
            aggregate: AggregateKey(format!("chat.message:{event_id}")),
            payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            contains_personal_data: true,
            pii_key_ref: Some(PiiKeyRef(format!("kms://acme/0/subject:{subject}"))),
        };
        let ctx = EmitContext {
            event_id: EventId(event_id.into()),
            tenant: tenant(),
            region: region(),
            actor: actor_for(subject),
            schema_ver: 1,
            occurred_at: now(),
            recorded_at: now(),
            caused_by: Some(CausedBy("human:h".into())),
        };
        derive_envelope(draft, ctx, None)
    }

    /// Seal a log + shredder for `subjects`, one inline-PII event each. Returns the fresh log +
    /// shredder with every DEK live (the pre-erase state).
    fn seeded(subjects: &[&str]) -> (BusEventLog, InMemoryShredder) {
        let mut log = BusEventLog::new();
        let shredder = InMemoryShredder::new();
        for (i, s) in subjects.iter().enumerate() {
            let ev = inline_pii(&format!("01J-{i}"), s);
            if let Some(k) = &ev.pii_key_ref {
                shredder.seal(k);
            }
            log.append(ev);
        }
        (log, shredder)
    }

    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    /// Unit: `erase_and_record` shreds the key AND records the subject in the PII-free ledger.
    #[test]
    fn erase_and_record_writes_the_pii_free_ledger() {
        let (mut log, shredder) = seeded(&["u42"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();

        holder
            .erase_and_record("u42", &mut log, &mut outbox, minter(), &ledger, now())
            .expect("erase+record");

        assert!(
            ledger.is_erased("u42"),
            "the ledger remembers u42 was erased"
        );
        assert_eq!(ledger.len(), 1);
        let entry = &ledger.entries()[0];
        assert_eq!(entry.subject, "u42");
        assert_eq!(
            entry.key_refs,
            vec![PiiKeyRef("kms://acme/0/subject:u42".into())]
        );
        // The ledger is PII-free: only the opaque discriminator + the key NAME, never a payload.
        assert!(
            !shredder.is_live(&PiiKeyRef("kms://acme/0/subject:u42".into())),
            "key shredded"
        );
    }

    /// Unit (the EB-16 core): a post-restore re-erasure pass re-destroys the keys for a
    /// previously-erased subject — the restored backup does NOT resurrect the key.
    #[test]
    fn re_erase_after_restore_re_destroys_resurrected_keys() {
        // (1) Erase u42 in the live cell + record it in the ledger.
        let (mut live_log, shredder) = seeded(&["u42"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        holder
            .erase_and_record("u42", &mut live_log, &mut outbox, minter(), &ledger, now())
            .expect("erase+record");
        let key = PiiKeyRef("kms://acme/0/subject:u42".into());
        assert!(!shredder.is_live(&key), "key dead in the live cell");

        // (2) RESTORE an OLDER backup: the restore brings back the pre-erase state — the DEK is LIVE
        //     again (re-sealed) and the log row is back WITHOUT its tombstone. (This is exactly what a
        //     restore of a backup taken before the erase does.)
        let (mut restored_log, _) = seeded(&["u42"]); // the row is back, no tombstone
        shredder.seal(&key); // the restore resurrected the DEK
        assert!(shredder.is_live(&key), "the restore RESURRECTED u42's DEK");

        // (3) RE-ERASE AFTER RESTORE: replay the ledger — re-destroy the resurrected key.
        let mut reerase_outbox = OutboxStore::new();
        let receipt = holder
            .re_erase_after_restore(
                &ledger,
                &mut restored_log,
                &mut reerase_outbox,
                minter(),
                now(),
            )
            .expect("re-erase");

        // The key is DEAD again — the restored backup did not resurrect it past the re-erasure.
        assert!(
            !shredder.is_live(&key),
            "the key stays destroyed across the restore"
        );
        assert_eq!(receipt.re_erased_subjects, 1);
        assert_eq!(
            receipt.keys_resurrected_by_restore, 1,
            "the restore brought the key back"
        );
        assert!(
            receipt.tombstones_re_emitted >= 1,
            "re-tombstoned the restored row"
        );
        // THE GATE: 0 resurrected inline-PII keys post-restore.
        assert_eq!(receipt.resurrected, 0, "0 resurrected keys post-restore");
        assert!(receipt.is_green());
    }

    /// Unit: re-erasure is idempotent — replaying the ledger when NOTHING was resurrected (the key is
    /// already dead) is a clean no-op success (still 0 resurrected, no panic, no false failure).
    #[test]
    fn re_erase_is_idempotent_when_nothing_resurrected() {
        let (mut log, shredder) = seeded(&["u42"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        holder
            .erase_and_record("u42", &mut log, &mut outbox, minter(), &ledger, now())
            .expect("erase+record");

        // No restore happened — the key is already dead. Re-erase: clean no-op.
        let (mut log2, _) = seeded(&["u42"]);
        let mut outbox2 = OutboxStore::new();
        let receipt = holder
            .re_erase_after_restore(&ledger, &mut log2, &mut outbox2, minter(), now())
            .expect("re-erase no-op");
        // The DEK in `shredder` is still dead (never resurrected) → 0 resurrected.
        assert_eq!(
            receipt.keys_resurrected_by_restore, 0,
            "nothing was resurrected"
        );
        assert_eq!(receipt.resurrected, 0);
        assert!(receipt.is_green());
    }

    /// Unit: the ledger is non-shred-erasable + survives across many erasures (multi-subject replay,
    /// deterministic order).
    #[test]
    fn ledger_records_many_subjects_and_replays_all() {
        let (mut log, shredder) = seeded(&["u1", "u2", "u3"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        let m = minter(); // ONE minter across the DSR so event_ids stay monotonic (no id collision).
        for s in ["u1", "u2", "u3"] {
            holder
                .erase_and_record(s, &mut log, &mut outbox, m.clone(), &ledger, now())
                .expect("erase+record");
        }
        assert_eq!(ledger.len(), 3);

        // Restore resurrects all three.
        let (mut restored, _) = seeded(&["u1", "u2", "u3"]);
        for s in ["u1", "u2", "u3"] {
            shredder.seal(&PiiKeyRef(format!("kms://acme/0/subject:{s}")));
        }
        let mut ro = OutboxStore::new();
        let receipt = holder
            .re_erase_after_restore(&ledger, &mut restored, &mut ro, minter(), now())
            .expect("re-erase all");
        assert_eq!(receipt.re_erased_subjects, 3);
        assert_eq!(receipt.keys_resurrected_by_restore, 3);
        assert_eq!(
            receipt.resurrected, 0,
            "all three stay destroyed across the restore"
        );
    }

    /// Unit: a KMS failure during the re-erasure pass is LOUD (never "assume re-erased") — the pass
    /// surfaces the error so the DSR retries; it does not silently report green.
    #[test]
    fn re_erase_is_loud_on_kms_failure() {
        let (mut log, shredder) = seeded(&["u42"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        holder
            .erase_and_record("u42", &mut log, &mut outbox, minter(), &ledger, now())
            .expect("erase+record");

        // Restore resurrects the key, but the KMS is now unreachable → the re-erase MUST be loud.
        let (mut restored, _) = seeded(&["u42"]);
        let key = PiiKeyRef("kms://acme/0/subject:u42".into());
        shredder.seal(&key);
        shredder.make_unreachable(&key);
        let mut ro = OutboxStore::new();
        let err = holder
            .re_erase_after_restore(&ledger, &mut restored, &mut ro, minter(), now())
            .expect_err("loud on KMS failure");
        assert!(matches!(err, ShredError::KmsUnavailable(_)));
    }
}
