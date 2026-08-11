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

fn mint_subject_dek(kms: &KmsEngine, subject: &str) -> EventsPiiKeyRef {
    kms.ensure_kek(&KekId::new(tenant(), region()))
        .expect("seed the in-memory KEK");
    let kref = kms
        .ensure_dek(&tenant(), &region(), KeyClass::Subject(subject.into()))
        .expect("mint subject DEK");
    EventsPiiKeyRef(kref.to_uri())
}

fn dek_id_for(subject: &str) -> DekId {
    DekId::new(tenant(), KeyClass::Subject(subject.into()))
}

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

#[test]
fn bus_d8_crypto_shred_reaches_backups_zero_recoverable_after_restore() {
    let kms = Arc::new(KmsEngine::new());

    let k_u42 = mint_subject_dek(&kms, "u42");
    let k_keep = mint_subject_dek(&kms, "keep_me");

    let shredder = KmsBusShredder::new(kms.clone(), region());
    let holder = BusHolder::new(tenant(), region(), shredder.clone());

    let mut live_log = BusEventLog::new();
    live_log.append(inline_pii("01J-1", "u42", &k_u42));
    live_log.append(inline_pii("01J-2", "u42", &k_u42));
    live_log.append(inline_pii("01J-9", "keep_me", &k_keep));

    assert!(
        kms.backup_snapshot()
            .iter()
            .any(|(d, _)| *d == dek_id_for("u42")),
        "precondition: u42's DEK is in the backup before erase"
    );

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
    assert!(
        !shredder.is_live(&k_u42),
        "BUS-D8 live leg: u42's DEK is dead in the live engine"
    );

    let recoverable_in_backup = kms
        .backup_snapshot()
        .iter()
        .filter(|(d, _)| *d == dek_id_for("u42"))
        .count();
    assert_eq!(
        recoverable_in_backup, 0,
        "BUS-D8 reaches-backups RED: a backup snapshot still carries u42's per-subject DEK \
         (a restore could resurrect the subject) - the threshold is 0 and is NOT weakened"
    );

    let archiver = reachable_archiver(100);
    let blobs = BlobPresence::new();
    let source = SourceLog::new();
    let restore = restore_to_offset(&archiver, 100, &[], &blobs, &source, &kms)
        .expect("restore lands at a consistent point");
    let u42_dek = dek_id_for("u42");
    assert!(
        !restore.restored_keys.iter().any(|(d, _)| *d == u42_dek),
        "STOR-D2: the restored key set does NOT bring back u42's destroyed DEK"
    );
    assert_eq!(
        restore.dangling_ref_count, 0,
        "STOR-D2: the restore lands at ONE consistent cross-seam point (0 dangling)"
    );

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
        "the real KMS restore excluded the destroyed DEK - nothing came back to re-shred"
    );

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

    let mut src = SignalSource::new();
    src.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        (recoverable_in_backup + re.resurrected) as i64,
    );
    let verdict = src.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        verdict.is_green(),
        "BUS-D8 reaches-backups + STOR-D2 GREEN: 0 recoverable inline-PII in backups, \
         0 resurrected post-restore - {verdict:?}"
    );
}

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
        "WITHOUT an erase the subject's DEK IS in the backup - the headline drill's 0 is earned \
         by the crypto-shred reaching the backup, not by absence"
    );
}

#[test]
fn bus_d8_a_malformed_key_ref_aborts_the_erase_loudly() {
    let kms = Arc::new(KmsEngine::new());
    let shredder = KmsBusShredder::new(kms, region());
    let holder = BusHolder::new(tenant(), region(), shredder);

    let mut log = BusEventLog::new();
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
        "a malformed key ref aborts the erase as INCOMPLETE - never a silent assume-erased"
    );
}
