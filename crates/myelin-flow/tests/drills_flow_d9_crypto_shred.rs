use myelin_flow::schema::WfHistoryRow;
use myelin_flow::{
    is_inline_pii_unrecoverable, open_inline_pii, seal_inline_pii, FlowTelemetry, WfHistoryHolder,
    WfJournal,
};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::encryption::SubjectId;
use myelin_storage::kms::{DekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn region() -> Region {
    Region::new("fr-par")
}
fn tenant() -> TenantId {
    TenantId::from_token("acme")
}
fn gdpr_tenant() -> GdprTenantId {
    GdprTenantId::from_token("acme")
}
fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        GdprTenantId::from_token("acme"),
    ))
}

fn history_row(run_id: &str, seq: i64, actor: &str, key_ref: Option<String>) -> WfHistoryRow {
    WfHistoryRow {
        tenant: tenant(),
        region: region(),
        run_id: run_id.into(),
        seq,
        kind: "activity_completed".into(),
        command_id: format!("agent.run:{seq}"),
        result: Some(vec![ArtifactRef(format!(
            "myelin://acme/identity/principal/{actor}"
        ))]),
        result_key_ref: key_ref,
    }
}

fn seed_inline_pii_history(
    kms: &KmsEngine,
    journal: &WfJournal,
    run_id: &str,
    seq: i64,
    subject_id: &str,
    plaintext: &[u8],
) -> myelin_storage::encryption::EncryptedColumn {
    let column = seal_inline_pii(
        kms,
        &region(),
        &tenant(),
        &SubjectId::new(subject_id),
        plaintext,
    )
    .expect("seal inline PII under the subject's per-subject DEK");
    journal.append_history_for_test(history_row(
        run_id,
        seq,
        subject_id,
        Some(column.key_ref.to_uri()),
    ));
    column
}

#[test]
fn flow_d9_crypto_shred_reaches_history_zero_recoverable_incl_backups() {
    let kms = Arc::new(KmsEngine::new());
    let journal = WfJournal::new();
    let telemetry = FlowTelemetry::new();

    let ada_col1 = seed_inline_pii_history(
        &kms,
        &journal,
        "run-1",
        0,
        "psn:ada",
        b"ada's medical note inlined into a run result",
    );
    let ada_col2 = seed_inline_pii_history(
        &kms,
        &journal,
        "run-2",
        0,
        "psn:ada",
        b"ada's home address inlined into a signal payload",
    );
    let bob_col = seed_inline_pii_history(
        &kms,
        &journal,
        "run-3",
        0,
        "psn:bob",
        b"bob's private content (a different subject)",
    );
    journal.append_history_for_test(history_row("run-4", 0, "psn:ada", None));

    assert!(
        !is_inline_pii_unrecoverable(&kms, &region(), &ada_col1),
        "ada's inline PII is recoverable before erase"
    );
    assert!(!is_inline_pii_unrecoverable(&kms, &region(), &ada_col2));
    assert!(!is_inline_pii_unrecoverable(&kms, &region(), &bob_col));
    assert_eq!(
        open_inline_pii(&kms, &region(), &ada_col1).expect("opens"),
        b"ada's medical note inlined into a run result"
    );

    let before = journal.history_in_tenant(&tenant());

    let holder = WfHistoryHolder::with_crypto_shred(
        journal.clone(),
        kms.clone(),
        region(),
        telemetry.clone(),
    );
    let receipt = holder
        .erase(EraseScope::Subject {
            subject: subject("psn:ada"),
            tenant: gdpr_tenant(),
        })
        .expect("erase succeeds");

    assert!(
        receipt.receipt.key_epoch_destroyed.is_some(),
        "the erase receipt records the destroyed per-subject DEK epoch (the crypto-shred reach)"
    );
    assert!(receipt.receipt.content_hash.starts_with("blake3:"));

    assert!(
        is_inline_pii_unrecoverable(&kms, &region(), &ada_col1),
        "0 recoverable: ada's inline PII (col1) is unrecoverable after the per-subject DEK shred"
    );
    assert!(
        is_inline_pii_unrecoverable(&kms, &region(), &ada_col2),
        "0 recoverable: ada's inline PII (col2) is unrecoverable"
    );

    let snapshot = kms.backup_snapshot();
    assert!(
        !snapshot
            .iter()
            .any(|(id, _)| *id == DekId::new(tenant(), KeyClass::Subject("psn:ada".into()))),
        "the crypto-shredded DEK is EXCLUDED from the backup snapshot - a restore cannot read ada's PII"
    );
    assert!(
        snapshot
            .iter()
            .any(|(id, _)| *id == DekId::new(tenant(), KeyClass::Subject("psn:bob".into()))),
        "bob's live DEK IS in the backup (a restore resurrects the live keys - just not the shredded one)"
    );

    assert!(
        !is_inline_pii_unrecoverable(&kms, &region(), &bob_col),
        "bob's inline PII still opens - ada's erasure shredded ONLY ada's DEK (GD-4 individual lever)"
    );

    let after = journal.history_in_tenant(&tenant());
    assert_eq!(
        after, before,
        "structure preserved: the journal rows survive the shred byte-identical (replay still works, \
         the PII is a tombstone)"
    );
    assert_eq!(
        after.len(),
        4,
        "no row deleted - the appearance stays, the PII is gone"
    );

    assert_eq!(
        telemetry.crypto_shreds_count(),
        1,
        "one subject's inline-PII rows made unrecoverable - the crypto-shred-lag signal is emitted"
    );
}

#[test]
fn flow_d9_refs_only_subject_tombstones_for_free_no_key_destroyed() {
    let kms = Arc::new(KmsEngine::new());
    let journal = WfJournal::new();
    let telemetry = FlowTelemetry::new();

    journal.append_history_for_test(history_row("run-1", 0, "psn:refs-only", None));

    let holder = WfHistoryHolder::with_crypto_shred(
        journal.clone(),
        kms.clone(),
        region(),
        telemetry.clone(),
    );
    let receipt = holder
        .erase(EraseScope::Subject {
            subject: subject("psn:refs-only"),
            tenant: gdpr_tenant(),
        })
        .expect("erase succeeds");

    assert!(
        receipt.receipt.key_epoch_destroyed.is_none(),
        "a refs-only subject shreds no key (the rows tombstone for free)"
    );
    assert_eq!(
        telemetry.crypto_shreds_count(),
        0,
        "no crypto-shred recorded when there was no inline-PII key to destroy"
    );
}
