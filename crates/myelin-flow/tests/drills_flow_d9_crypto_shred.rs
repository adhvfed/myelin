//! # FLOW-D9 — crypto-shred reaching history: the PersonalDataHolder erase path COMPLETE
//! (P-FLOW-24, M5)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **FLOW-D9**
//! (FLOW / erasure, SCHED): *"Erase a subject with inline-PII history/signal rows → keys destroyed
//! (unrecoverable incl. backups), references tombstoned, structure preserved."* Green artifact:
//! **crypto-shred-lag; 0 recoverable.**
//!
//! **Thresholds (exact — NEVER weaken, EI-01 §3):**
//! - a subject with inline-PII `wf_history.result_key_ref` AND `wf_signal.payload_key_ref` rows is
//!   erased → their per-subject DEK is DESTROYED;
//! - **0 recoverable PII**: every inline-PII cell the subject sealed is unrecoverable in the LIVE
//!   journal AND after a **backup-restore-then-read attempt** (the shredded DEK is excluded from the
//!   backup snapshot — it stays dead across a restore, storage §7.5);
//! - **references tombstoned**: the refs-stored (non-inline-PII) rows tombstone for free (0 PII
//!   columns mutated — Identity's §4.8 pseudonym-shred);
//! - **structure preserved**: the journal rows survive byte-identical → deterministic replay still
//!   rebuilds the run (the PII is a tombstone, the structure lives);
//! - the **crypto-shred-lag** telemetry signal (contract 1.8) is emitted — the dated green artifact;
//! - a DIFFERENT subject's inline PII is UNTOUCHED (the GD-4 individual lever, not a tenant wipe).
//!
//! ## What this drill proves — the §4.8 triad COMPLETE (the P-FLOW-03 floor CLOSED)
//! P-FLOW-03 shipped the STRUCTURAL references-not-payloads erase (the refs-stored rows tombstone for
//! free) and NAMED its floor: the rare inline-PII rows were not yet crypto-shredded. P-FLOW-24 CLOSES
//! it — the holder's `erase`, wired with the crypto-shred lever
//! ([`myelin_flow::WfHistoryHolder::with_crypto_shred`]), destroys the erased subject's per-subject
//! DEK over the ONE [`KmsEngine`], so the inline-PII ciphertext is unrecoverable incl. backups, WITHOUT
//! rewriting the append-only journal. This is the FLOW-D9 chained drill on the harness: erase → 0
//! recoverable (incl. a backup-restore attempt that fails to decrypt), references tombstoned, structure
//! preserved.
//!
//! ## DB-free
//! The drill operates over the in-memory [`WfJournal`] + the ONE [`KmsEngine`] + the
//! [`myelin_storage::encryption::ColumnCryptor`]; the real per-subject-DEK destroy across the live
//! `myelin-flow` Postgres + the KMS backups rides the storage restore-verify drill (STOR-D3) at cell
//! scale. The dated SCHED green artifact for FLOW-D9 against the dev stack is the integration leg; this
//! is the chained unit-of-proof on the harness.

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

/// Build a `wf_history` row: `result` names `actor` by ref (references-not-payloads); `key_ref` is the
/// inline-PII `result_key_ref` (the per-subject DEK locator) when `Some`.
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

/// Seed an inline-PII `wf_history` row whose `result_key_ref` names `subject`'s per-subject DEK, with
/// `plaintext` sealed under it. Returns the sealed column (the SAME ciphertext that rests in the live
/// journal column AND any backup of it) so the drill can attempt to recover it before/after the erase.
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

/// **FLOW-D9 — erase a subject with inline-PII history rows → 0 recoverable incl. backups, references
/// tombstoned, structure preserved, crypto-shred-lag emitted.**
#[test]
fn flow_d9_crypto_shred_reaches_history_zero_recoverable_incl_backups() {
    let kms = Arc::new(KmsEngine::new());
    let journal = WfJournal::new();
    let telemetry = FlowTelemetry::new();

    // The erased subject (ada) seals inline PII into two history rows; a DIFFERENT subject (bob) seals
    // their own inline PII (must be UNTOUCHED — the GD-4 individual lever). A refs-stored control row
    // (no inline PII) tombstones for free.
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
    // A refs-stored control row that names ada as a referenced actor but carries NO inline PII (it
    // tombstones for free — 0 PII columns mutated).
    journal.append_history_for_test(history_row("run-4", 0, "psn:ada", None));

    // BEFORE the erase: every subject's inline PII is recoverable while their key lives.
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

    // Snapshot the EXACT journal bytes before the erase (the structure-preserved assertion).
    let before = journal.history_in_tenant(&tenant());

    // ERASE ada through the holder wired with the crypto-shred lever (the COMPLETE P-FLOW-24 path).
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

    // THE RECEIPT records the destroyed per-subject DEK epoch (P-FLOW-03's None is now FILLED — the
    // crypto-shred reach).
    assert!(
        receipt.receipt.key_epoch_destroyed.is_some(),
        "the erase receipt records the destroyed per-subject DEK epoch (the crypto-shred reach)"
    );
    assert!(receipt.receipt.content_hash.starts_with("blake3:"));

    // 0 RECOVERABLE: ada's inline PII is unrecoverable in the LIVE journal.
    assert!(
        is_inline_pii_unrecoverable(&kms, &region(), &ada_col1),
        "0 recoverable: ada's inline PII (col1) is unrecoverable after the per-subject DEK shred"
    );
    assert!(
        is_inline_pii_unrecoverable(&kms, &region(), &ada_col2),
        "0 recoverable: ada's inline PII (col2) is unrecoverable"
    );

    // 0 RECOVERABLE incl. BACKUPS: a backup-restore-then-read attempt fails too — the shredded DEK is
    // EXCLUDED from the backup snapshot (it stays dead across a restore, §7.5), so a restore cannot
    // resurrect ada's key. We model the restore by reading the post-erase backup snapshot and
    // asserting ada's DEK is absent (so any restore of it is a no-op) AND the ciphertext still fails to
    // decrypt against the live (post-shred) engine.
    let snapshot = kms.backup_snapshot();
    assert!(
        !snapshot
            .iter()
            .any(|(id, _)| *id == DekId::new(tenant(), KeyClass::Subject("psn:ada".into()))),
        "the crypto-shredded DEK is EXCLUDED from the backup snapshot — a restore cannot read ada's PII"
    );
    // The restored backup still carries bob's live DEK (proof the snapshot is non-empty / a real
    // restore would resurrect the LIVE keys, just not the shredded one).
    assert!(
        snapshot
            .iter()
            .any(|(id, _)| *id == DekId::new(tenant(), KeyClass::Subject("psn:bob".into()))),
        "bob's live DEK IS in the backup (a restore resurrects the live keys — just not the shredded one)"
    );

    // A DIFFERENT subject's inline PII is UNTOUCHED (the GD-4 individual lever, not a tenant wipe).
    assert!(
        !is_inline_pii_unrecoverable(&kms, &region(), &bob_col),
        "bob's inline PII still opens — ada's erasure shredded ONLY ada's DEK (GD-4 individual lever)"
    );

    // STRUCTURE PRESERVED: the journal rows survive byte-identical → deterministic replay still works.
    let after = journal.history_in_tenant(&tenant());
    assert_eq!(
        after, before,
        "structure preserved: the journal rows survive the shred byte-identical (replay still works, \
         the PII is a tombstone)"
    );
    assert_eq!(
        after.len(),
        4,
        "no row deleted — the appearance stays, the PII is gone"
    );

    // CRYPTO-SHRED-LAG: the FLOW-D9 telemetry green artifact is emitted (one shred recorded).
    assert_eq!(
        telemetry.crypto_shreds_count(),
        1,
        "one subject's inline-PII rows made unrecoverable — the crypto-shred-lag signal is emitted"
    );
}

/// **FLOW-D9 — a subject with ONLY refs-stored rows shreds no key (the references-not-payloads common
/// case): the erase still succeeds, 0 keys destroyed, the rows tombstone for free.** The vast majority
/// of erasures never reach the crypto-shred — they tombstone for free (P-FLOW-03). This pins that the
/// COMPLETE path degrades correctly to the structural erase when there is no inline PII.
#[test]
fn flow_d9_refs_only_subject_tombstones_for_free_no_key_destroyed() {
    let kms = Arc::new(KmsEngine::new());
    let journal = WfJournal::new();
    let telemetry = FlowTelemetry::new();

    // A refs-stored row naming the subject — NO inline PII.
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

    // No inline-PII DEK → nothing to shred → key_epoch_destroyed = None (the refs-stored rows tombstone
    // for free). The erase still succeeds.
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
