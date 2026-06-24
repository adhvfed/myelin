//! # BUS-D8 (reaches-backups leg) + STOR-D4 — the Bus's crypto-shred reaches BACKUPS (EB-29 / P-420)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` rows **BUS-D8**
//! (*erase a subject → inline-PII events unrecoverable; `*.erased` tombstones; consumers degrade*) +
//! **STOR-D4** (*erase a subject; attempt recovery from backups → per-subject ciphertext
//! unrecoverable (key destroyed, excluded from backup). 0 recoverable PII in any backup*) +
//! **STOR-D2** (cell-scale restore re-confirmed). Thresholds: **0 recoverable inline-PII in the log
//! AND backups; tombstones present; 0 resurrected post-restore** — all EXACT, never weakened.
//!
//! ## What this drill proves (the EB-29 M5 follow-on of the EB-15 live-store floor)
//! EB-15 (P-092) proved the Bus's crypto-shred in the LIVE log (the DEK destroyed → the live payload
//! unrecoverable + tombstones present). EB-29 extends it to BACKUPS, against the **REAL**
//! [`KmsEngine`](myelin_storage::KmsEngine) (not the events-side in-memory floor): the Bus's holder
//! crypto-shreds through [`KmsBusShredder`](myelin_storage::KmsBusShredder) — the same key hierarchy
//! storage owns — so a destroyed per-subject DEK is **excluded from `backup_snapshot`** (§7.5) and a
//! REAL `restore_to_offset` of that snapshot cannot resurrect it. The chain:
//!
//! 1. **Live-store leg (BUS-D8 / EB-15 re-confirmed against the real KMS):** mint a per-subject DEK,
//!    append the Bus's inline-PII events, `erase_and_record` → the DEK is destroyed in the live engine,
//!    the live payload is unrecoverable, `*.erased` tombstones are emitted.
//! 2. **Reaches-backups leg (BUS-D8-backups / STOR-D4):** the destroyed DEK is ABSENT from
//!    `backup_snapshot()` — 0 recoverable inline-PII in the backup.
//! 3. **Cell-scale restore re-confirmed (STOR-D2):** a REAL [`restore_to_offset`] of the backup
//!    snapshot brings back NOT the destroyed DEK (the restored key set excludes it); a post-restore
//!    re-erasure pass re-confirms **0 resurrected** (the key stays dead across a restore).
//! 4. **Per-subject isolation:** another subject's DEK is untouched in the live engine AND the
//!    backup (one person's erasure never erases another's).
//!
//! ## The assertion is REAL, not vacuous (EI-01 §3)
//! [`bus_d8_without_erase_the_backup_still_carries_the_subject`] proves the backup DOES carry the
//! subject's DEK BEFORE the erase — so the green drill's "0 recoverable in backup" is earned by the
//! crypto-shred reaching the backup, not by the DEK having been absent anyway.
//!
//! The verdict is bridged into the FROZEN §10.2 harness assertion library
//! ([`SignalSource`]/[`Predicate`]) so the green is LOUD, never swallowed: `RestoreCrossSeamMismatch`
//! (== 0, the restore lands consistent) carries the 0-recoverable-in-backup zero.

use std::sync::Arc;

use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, BusErasureLedger, BusEventLog, BusHolder,
    DataRole, EmitContext, EventDraft, EventEnvelope, EventId, EventType, IdMinter,
    InlinePiiShredder, MonotonicMinter, OutboxStore, PiiKeyRef as EventsPiiKeyRef, Timestamp,
    Visibility,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{
    restore_to_offset, BlobPresence, ContinuousArchiver, DekId, KekId, KeyClass, KmsBusShredder,
    KmsEngine, PiiKeyRef as KmsPiiKeyRef, SourceLog, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn now() -> Timestamp {
    Timestamp("2026-06-24T00:00:00Z".into())
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

/// Mint a per-subject DEK in the REAL engine and return the events-side `pii_key_ref` URI that
/// names it (the URI the Bus's envelope carries; the holder shreds through it).
fn mint_subject_dek(kms: &KmsEngine, subject: &str) -> EventsPiiKeyRef {
    kms.ensure_kek(&KekId::new(tenant(), region()));
    let kref = kms
        .ensure_dek(&tenant(), &region(), KeyClass::Subject(subject.into()))
        .expect("mint subject DEK");
    EventsPiiKeyRef(kref.to_uri())
}

/// The storage-side `DekId` for a subject (to probe the backup snapshot).
fn dek_id_for(subject: &str) -> DekId {
    DekId::new(tenant(), KeyClass::Subject(subject.into()))
}

/// Build one of the Bus's inline-PII events for `subject`, sealed under its per-subject DEK
/// (references-not-payloads is the platform default; this is the rare inline-PII event, §4.8).
fn inline_pii(event_id: &str, subject: &str, key_ref: &EventsPiiKeyRef) -> EventEnvelope {
    let draft = EventDraft {
        type_: EventType("chat.message.created".into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
        aggregate: AggregateKey(format!("chat.message:{event_id}")),
        payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(key_ref.clone()),
    };
    let ctx = EmitContext {
        event_id: EventId(event_id.into()),
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId(subject.into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
        caused_by: None,
    };
    derive_envelope(draft, ctx, None)
}

/// A PITR-reachable archiver with a base anchor at 0 and a tail at `tail` (the restore cursor).
fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment {
        end_offset: 0,
        committed_at: 0,
    })
    .unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment {
        end_offset: tail,
        committed_at: 10,
    })
    .unwrap();
    arch
}

/// **BUS-D8 + STOR-D4 + STOR-D2 (the EB-29 headline): erase a Bus subject through the REAL KMS →
/// 0 recoverable inline-PII in the live log AND backups; a real restore does not resurrect it;
/// per-subject isolation holds.**
#[test]
fn bus_d8_crypto_shred_reaches_backups_zero_recoverable_after_restore() {
    let kms = Arc::new(KmsEngine::new());

    // Two subjects in the SAME cell — u42 will be erased, keep_me must be untouched (isolation).
    let k_u42 = mint_subject_dek(&kms, "u42");
    let k_keep = mint_subject_dek(&kms, "keep_me");

    // The Bus's holder over the REAL KMS-backed crypto-shred seam (the P-GA-06 binding).
    let shredder = KmsBusShredder::new(kms.clone(), region());
    let holder = BusHolder::new(tenant(), region(), shredder.clone());

    // The live retained log: u42's inline-PII events + one of keep_me's.
    let mut live_log = BusEventLog::new();
    live_log.append(inline_pii("01J-1", "u42", &k_u42));
    live_log.append(inline_pii("01J-2", "u42", &k_u42)); // a SECOND event under the same DEK.
    live_log.append(inline_pii("01J-9", "keep_me", &k_keep));

    // ── Pre-condition: every subject's DEK is in the backup BEFORE any erase. ──
    assert!(
        kms.backup_snapshot()
            .iter()
            .any(|(d, _)| *d == dek_id_for("u42")),
        "precondition: u42's DEK is in the backup before erase"
    );

    // ── (1) LIVE-STORE LEG (BUS-D8 / EB-15 re-confirmed on the real KMS): erase_and_record u42. ──
    let ledger = BusErasureLedger::new(tenant(), region());
    let mut live_outbox = OutboxStore::new();
    let receipt = holder
        .erase_and_record(
            "u42",
            &mut live_log,
            &mut live_outbox,
            minter(),
            &ledger,
            now(),
        )
        .expect("live erase + ledger record (real KMS reachable)");

    assert_eq!(
        receipt.recoverable_remaining, 0,
        "BUS-D8 live leg: 0 recoverable inline-PII in the live log after the erase"
    );
    assert_eq!(
        receipt.keys_shredded, 1,
        "one distinct per-subject DEK destroyed"
    );
    assert!(
        receipt.tombstones_emitted >= 2,
        "a *.erased tombstone per inline-PII event (2 events under u42's DEK)"
    );
    assert!(
        live_log.is_tombstoned("01J-1") && live_log.is_tombstoned("01J-2"),
        "consumers degrade gracefully: u42's rows are tombstoned"
    );
    // The DEK no longer resolves in the LIVE engine.
    assert!(
        !shredder.is_live(&k_u42),
        "BUS-D8 live leg: u42's DEK is dead in the live engine"
    );

    // ── (2) REACHES-BACKUPS LEG (BUS-D8-backups / STOR-D4): 0 recoverable in the BACKUP. ──
    let recoverable_in_backup = kms
        .backup_snapshot()
        .iter()
        .filter(|(d, _)| *d == dek_id_for("u42"))
        .count();
    assert_eq!(
        recoverable_in_backup, 0,
        "BUS-D8 reaches-backups RED: a backup snapshot still carries u42's per-subject DEK \
         (a restore could resurrect the subject) — the threshold is 0 and is NOT weakened"
    );

    // ── (3) CELL-SCALE RESTORE RE-CONFIRMED (STOR-D2): a REAL restore does not resurrect u42. ──
    let archiver = reachable_archiver(100);
    let blobs = BlobPresence::new();
    let source = SourceLog::new();
    let restore = restore_to_offset(&archiver, 100, &[], &blobs, &source, &kms)
        .expect("restore lands at a consistent point");
    // The restored key set is the backup snapshot — which EXCLUDES the destroyed DEK.
    let u42_dek = dek_id_for("u42");
    assert!(
        !restore.restored_keys.iter().any(|(d, _)| *d == u42_dek),
        "STOR-D2: the restored key set does NOT bring back u42's destroyed DEK"
    );
    assert_eq!(
        restore.dangling_ref_count, 0,
        "STOR-D2: the restore lands at ONE consistent cross-seam point (0 dangling)"
    );

    // Model the restore having brought the LOG ROWS back (a pre-erase backup of the log would carry
    // them, without their tombstones); the key, however, is gone from the engine. The post-restore
    // re-erasure pass re-confirms 0 resurrected.
    let mut restored_log = BusEventLog::new();
    restored_log.append(inline_pii("01J-1", "u42", &k_u42));
    restored_log.append(inline_pii("01J-2", "u42", &k_u42));
    let mut reerase_outbox = OutboxStore::new();
    let re = holder
        .re_erase_after_restore(
            &ledger,
            &mut restored_log,
            &mut reerase_outbox,
            minter(),
            now(),
        )
        .expect("post-restore re-erase (real KMS reachable)");
    assert_eq!(
        re.resurrected, 0,
        "STOR-D2/STOR-D4: 0 resurrected inline-PII keys post-restore (the key stays dead)"
    );
    assert!(re.is_green(), "the Bus's restore-verify leg is GREEN");
    assert_eq!(
        re.keys_resurrected_by_restore, 0,
        "the real KMS restore excluded the destroyed DEK — nothing came back to re-shred"
    );

    // ── (4) PER-SUBJECT ISOLATION: keep_me is untouched in the live engine AND the backup. ──
    assert!(
        shredder.is_live(&k_keep),
        "isolation: keep_me's DEK is still live (one person's erasure does not touch another's)"
    );
    assert!(
        kms.backup_snapshot()
            .iter()
            .any(|(d, _)| *d == dek_id_for("keep_me")),
        "isolation: keep_me's DEK is still in the backup"
    );

    // ── BRIDGE into the §10.2 harness assertion library — a LOUD green, never swallowed. ──
    // RestoreCrossSeamMismatch carries the 0-recoverable-in-backup + 0-resurrected zero.
    let mut src = SignalSource::new();
    src.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        (recoverable_in_backup + re.resurrected) as i64,
    );
    let verdict = src.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        verdict.is_green(),
        "BUS-D8 reaches-backups + STOR-D2 GREEN: 0 recoverable inline-PII in backups, \
         0 resurrected post-restore — {verdict:?}"
    );
}

/// **The assertion is REAL (EI-01 §3): WITHOUT the erase, the backup DOES carry the subject's DEK.**
/// So the green drill's "0 recoverable in backup" is earned by the crypto-shred reaching the backup,
/// not by the DEK having been absent anyway. If this drill ever passed (0 in the backup pre-erase),
/// the headline drill would be vacuous.
#[test]
fn bus_d8_without_erase_the_backup_still_carries_the_subject() {
    let kms = KmsEngine::new();
    let _ = mint_subject_dek(&kms, "u42");
    let recoverable_in_backup = kms
        .backup_snapshot()
        .iter()
        .filter(|(d, _)| *d == dek_id_for("u42"))
        .count();
    assert_eq!(
        recoverable_in_backup, 1,
        "WITHOUT an erase the subject's DEK IS in the backup — the headline drill's 0 is earned \
         by the crypto-shred reaching the backup, not by absence"
    );
}

/// A malformed `pii_key_ref` aborts the Bus's erase LOUDLY (never "assume erased" off a ref the real
/// KMS adapter cannot parse) — the 0-fail-open posture reaching the backups leg too.
#[test]
fn bus_d8_a_malformed_key_ref_aborts_the_erase_loudly() {
    let kms = Arc::new(KmsEngine::new());
    let shredder = KmsBusShredder::new(kms, region());
    let holder = BusHolder::new(tenant(), region(), shredder);

    let mut log = BusEventLog::new();
    // A ref the Bus's `locate` DOES attribute to subject u42 (its trailing class token is
    // `subject:u42`), but which the REAL KMS adapter cannot parse (it is missing the `<dek-epoch>`
    // segment, so `KmsPiiKeyRef::parse` returns None). The holder locates the event, then the
    // adapter's `destroy_key` aborts LOUDLY — never a silent "assume erased".
    let bad_ref = EventsPiiKeyRef("kms://acme/subject:u42".into());
    assert!(
        KmsPiiKeyRef::parse(&bad_ref.0).is_none(),
        "the ref is genuinely unparseable by the real KMS (the precondition of this drill)"
    );
    log.append(inline_pii("01J-X", "u42", &bad_ref));

    let mut outbox = OutboxStore::new();
    let err = holder.erase("u42", &mut log, &mut outbox, minter());
    assert!(
        err.is_err(),
        "a malformed key ref aborts the erase as INCOMPLETE — never a silent assume-erased"
    );
}
