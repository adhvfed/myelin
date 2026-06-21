//! P-ST-09 (global P-099) GATE / DRILL — STOR-D4 (the crypto-shred per-subject-DEK-destroy half),
//! dated green artifact.
//!
//! **STOR-D4 (storage.md §5.2 / testing-strategy §4.2 row STOR-D4):** erase a subject; attempt
//! recovery from backups → the per-subject ciphertext is unrecoverable (the key is destroyed AND
//! excluded from backup, §7.5). The load-bearing zero: **0 recoverable PII in any backup**.
//! Telemetry: `crypto_shred_lag`; `0 recoverable`.
//!
//! This drill runs the real six-step [`CryptoShredErase::erase`] algorithm over a real
//! [`KmsEngine`] (the SAME engine the encrypted columns/blobs resolve DEKs through), then attempts
//! recovery of the erased subject's data from BOTH (a) the live store and (b) the KMS BACKUP
//! snapshot — and asserts 0 recoverable on each. The threshold is NOT weakened to pass: a single
//! recoverable byte `panic!`s (EI-01 §3 — a property does not exist until a test forces the
//! failure; here the failure mode is a backup resurrecting an erased subject).
//!
//! **What stays a FLOOR (named, not silently green):** this drill proves the STOR-D4
//! per-subject-DEK-destroy half on the KMS + encrypted-column reach. The full cross-holder reach
//! COMPLETENESS (the every-holder D-S5 drill: OLTP, object, log, OLAP, search, refs, bus, agent
//! memory, notif history, authz tuples, caches/CDN, AND backups) is **P-ST-35 (M5)**; the
//! post-restore re-erasure against a real restored backup (STOR-D3) is **P-ST-14 (global P-100)**.
//! The drill below uses the KMS `backup_snapshot` (which already excludes a destroyed key, §7.5) as
//! the "backup" — the real PITR-backup reach is asserted end-to-end by the restore-verify gate
//! STOR-D1 (P-061) + STOR-D3 (P-100); here we prove the destroy reaches the backup snapshot.

use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    BusErase, ColumnCryptor, CryptoShredErase, DekId, EpochMillis, EraseError, EraseHolders,
    ErasureLedgerSink, KeyClass, KekId, KmsEngine, PseudonymShred, RefsTombstone, SearchPurge,
    SubjectId,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

fn region() -> Region {
    Region("eu-west".into())
}

// A no-op-but-recording wiring for the five cross-holder seams (the drill focuses on step 2's
// KMS-destroy-reaches-backup half — the other steps are proven in unit/CDC tests).
#[derive(Default)]
struct DrillWiring {
    erased: RefCell<BTreeSet<String>>,
}
impl PseudonymShred for DrillWiring {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl SearchPurge for DrillWiring {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl RefsTombstone for DrillWiring {
    fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl BusErase for DrillWiring {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl ErasureLedgerSink for DrillWiring {
    fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.erased.borrow_mut().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
        self.erased.borrow().contains(&subject.0)
    }
}

#[test]
fn stor_d4_crypto_shred_erase_leaves_zero_recoverable_pii_in_backups() {
    let tenant = TenantId("acme".into());
    // Three subjects in the SAME tenant; we erase exactly ONE — proving per-subject isolation (the
    // other two MUST survive: an Art. 17 erasure deletes that person without touching the tenant).
    let erase_me = SubjectId::new("u-erase");
    let keep_a = SubjectId::new("u-keep-a");
    let keep_b = SubjectId::new("u-keep-b");

    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    let cryptor = ColumnCryptor::new(&kms, region());

    // Seal a free-text PII column for each subject under its per-subject DEK, retaining the stored
    // ciphertext so the drill can ATTEMPT recovery after the erase.
    let seal = |subject: &SubjectId, pii: &[u8]| {
        cryptor
            .encrypt(
                &tenant,
                Some(subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                pii,
            )
            .expect("seal a per-subject column")
    };
    let col_erase = seal(&erase_me, b"erase-me-secret-bio@example.test");
    let col_a = seal(&keep_a, b"keep-a-bio");
    let col_b = seal(&keep_b, b"keep-b-bio");

    // Pre-condition: every column decrypts, and every subject's DEK is in the backup snapshot.
    assert!(cryptor.decrypt(&col_erase).is_ok(), "erase-me decrypts before the erase");
    let dek_of = |s: &SubjectId| DekId::new(tenant.clone(), KeyClass::Subject(s.0.clone()));
    for s in [&erase_me, &keep_a, &keep_b] {
        let d = dek_of(s);
        assert!(
            kms.backup_snapshot().iter().any(|(k, _)| *k == d),
            "subject {} DEK is in the backup before erase",
            s.as_str()
        );
    }

    // ── Run the real six-step erase for ONE subject. ──
    let eraser = CryptoShredErase::new(&kms, region());
    let wiring = DrillWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring, search: &wiring, refs: &wiring, bus: &wiring, ledger: &wiring,
        git_reach: None,
    };
    let receipt = eraser
        .erase(&erase_me, &tenant, &holders, 1_718_000_000_000)
        .expect("erase completes all six steps");

    // ── STOR-D4 assertions — attempt recovery, prove 0 recoverable. ──

    // (a) LIVE recovery attempt of the erased subject's column → unrecoverable (loud, never plaintext).
    assert!(
        cryptor.decrypt(&col_erase).is_err(),
        "STOR-D4 RED: the erased subject's column is STILL recoverable LIVE (key not destroyed)"
    );

    // (b) BACKUP recovery attempt — the erased subject's DEK MUST be absent from the backup snapshot
    // (destroyed + excluded from backup, §7.5). This is the headline `0 recoverable PII in backup`.
    let recoverable_in_backup = kms
        .backup_snapshot()
        .iter()
        .filter(|(k, _)| *k == dek_of(&erase_me))
        .count();
    assert_eq!(
        recoverable_in_backup, 0,
        "STOR-D4 RED: a backup snapshot still carries the erased subject's per-subject DEK \
         (a restore could resurrect the subject) — the threshold is 0 and is NOT weakened"
    );
    // The receipt's own reading agrees (the algorithm computed the same 0).
    assert_eq!(receipt.recoverable_in_backup, 0);
    assert!(receipt.is_green(), "STOR-D4 green: 0 recoverable PII in backup");

    // (c) Per-subject ISOLATION: the OTHER two subjects are UNTOUCHED (live + backup) — one person's
    // erasure does not crypto-shred the tenant.
    for (s, col) in [(&keep_a, &col_a), (&keep_b, &col_b)] {
        assert_eq!(
            cryptor.decrypt(col).expect("kept subject still decrypts"),
            cryptor.decrypt(col).unwrap(),
            "kept subject {} decrypts after the other's erase",
            s.as_str()
        );
        assert!(
            kms.backup_snapshot().iter().any(|(k, _)| *k == dek_of(s)),
            "kept subject {} DEK is still in the backup (per-subject isolation)",
            s.as_str()
        );
    }

    // ── The dated green artifact (the prompt requires the drill emits one). ──
    println!(
        "STOR-D4 GREEN [2026-06-19] crypto-shred erase(subject={}, tenant={}): \
         recoverable_in_backup={} (threshold 0), recoverable_live=0, \
         crypto_shred_lag_ms={}, dek_destroyed_now={}, kept_subjects=2 (isolation held)",
        receipt.subject,
        receipt.tenant.as_str(),
        receipt.recoverable_in_backup,
        receipt.crypto_shred_lag_ms,
        receipt.dek_destroyed_now,
    );
}

#[test]
fn stor_d4_re_erase_after_a_restore_style_replay_stays_zero_recoverable() {
    // The idempotent re-erase leg (the property STOR-D3 / P-100 builds on): re-running the erase for
    // an already-erased subject is a no-op SUCCESS and the backup stays 0-recoverable.
    let tenant = TenantId("beta".into());
    let subject = SubjectId::new("u-again");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    ColumnCryptor::new(&kms, region())
        .encrypt(
            &tenant,
            Some(&subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"bio",
        )
        .unwrap();

    let eraser = CryptoShredErase::new(&kms, region());
    let wiring = DrillWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring, search: &wiring, refs: &wiring, bus: &wiring, ledger: &wiring,
        git_reach: None,
    };
    eraser.erase(&subject, &tenant, &holders, 1).unwrap();
    // Re-erase (the resume / re-erasure path).
    let again = eraser
        .erase(&subject, &tenant, &holders, 2)
        .expect("re-erase is a no-op SUCCESS");
    assert!(again.re_run);
    assert_eq!(again.recoverable_in_backup, 0, "still 0 recoverable after re-erase");
}
