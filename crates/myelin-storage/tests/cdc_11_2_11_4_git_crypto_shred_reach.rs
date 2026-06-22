//! Contract 11.2 / 11.4 CDC pair — the GIT CRYPTO-SHRED REACH (P-ST-24 / global P-253; the storage
//! half of rows 11.2/11.4 — the crypto-shred reach into git's reflogs / bitmaps / pack-tier backups).
//!
//! Rows 11.2/11.4's git-reach half is the storage-side MECHANISM that extends the `erase(subject)`
//! crypto-shred (the per-subject DEK destroy) to ALSO reach git's structures sealed under the
//! per-tenant blob DEK. This CDC pair pins the seam between:
//!   - the **PROVIDER** = `myelin-storage` — [`GitCryptoShredReach`] (destroy the per-tenant blob
//!     DEK → reflog/bitmap/pack-tier-backup ciphertext unrecoverable live AND 0 recoverable in
//!     backup; the commit-object bytes are the pseudonymous-by-default residual, by reference 10.9),
//!     wired as the [`BlobShredReach`] seam the [`CryptoShredErase`] step-2 crypto-shred invokes;
//!   - the **CONSUMER** = the DSR ORCHESTRATOR (GDPR 10.1/10.11) — when a subject who authored
//!     commits/PRs is erased, it wires the git reach behind `EraseHolders::git_reach` and calls
//!     `erase(subject, tenant)`, expecting the git structures to be reached in the SAME crypto-shred
//!     step and the receipt to show 0 recoverable in backup.
//!
//! If the reach's blob-DEK-destroy, its 0-recoverable post-condition, or its residual-posture
//! (pseudonymous-by-default, NOT a byte-mutation) drifts, this stops passing — exactly the
//! consumer-driven contract the DSR orchestrator depends on for a commit author's Art. 17 erasure.

use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    BlobShredReach, BusErase, ColumnCryptor, CryptoShredErase, EpochMillis, EraseError,
    EraseHolders, ErasureLedgerSink, GitCryptoShredReach, GitResidual, KekId, KeyClass, KmsEngine,
    PseudonymShred, RefsTombstone, SearchPurge, SubjectId,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

fn region() -> Region {
    Region("eu-west".into())
}

// ── The CONSUMER's wiring: the DSR orchestrator's five cross-holder seams (deterministic doubles). ──
#[derive(Default)]
struct OrchestratorWiring {
    order: RefCell<Vec<&'static str>>,
    erased: RefCell<BTreeSet<String>>,
}
impl PseudonymShred for OrchestratorWiring {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.order.borrow_mut().push("1:pseudonym");
        Ok(())
    }
}
impl SearchPurge for OrchestratorWiring {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.order.borrow_mut().push("3:search");
        Ok(())
    }
}
impl RefsTombstone for OrchestratorWiring {
    fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.order.borrow_mut().push("4:refs");
        Ok(())
    }
}
impl BusErase for OrchestratorWiring {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.order.borrow_mut().push("5:bus");
        Ok(())
    }
}
impl ErasureLedgerSink for OrchestratorWiring {
    fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.order.borrow_mut().push("6:ledger");
        self.erased.borrow_mut().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
        self.erased.borrow().contains(&subject.0)
    }
}

/// Stand up an engine holding BOTH the subject's per-subject DEK (the free-text shred) AND the
/// per-tenant blob DEK (the git structures' shred) — a commit author with free-text content.
fn engine_with_subject_and_blob_dek(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    // The subject's free-text column (per-subject DEK).
    let cryptor = ColumnCryptor::new(&kms, region());
    cryptor
        .encrypt(
            tenant,
            Some(subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"alice's commit message body (free text)",
        )
        .expect("seal a per-subject column");
    // The per-tenant blob DEK git structures seal under.
    kms.ensure_dek(tenant, &region(), KeyClass::Blob)
        .expect("blob dek");
    kms
}

#[test]
fn cdc_git_reach_runs_in_the_erase_crypto_shred_step_for_a_commit_author() {
    let tenant = TenantId("acme".into());
    let subject = SubjectId::new("u-commit-author");
    let kms = engine_with_subject_and_blob_dek(&tenant, &subject);
    let eraser = CryptoShredErase::new(&kms, region());
    let git_reach = GitCryptoShredReach::new(&kms, region());

    // The DSR orchestrator wires the five seams + the git reach (the subject authored git content).
    let wiring = OrchestratorWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring,
        search: &wiring,
        refs: &wiring,
        bus: &wiring,
        ledger: &wiring,
        git_reach: Some(&git_reach),
    };
    let receipt = eraser
        .erase(&subject, &tenant, &holders, 1_000)
        .expect("the commit author's erase succeeds");

    // The cross-holder steps ran in order; step 2 (the crypto-shred) destroyed the per-subject DEK.
    assert_eq!(
        wiring.order.borrow().as_slice(),
        ["1:pseudonym", "3:search", "4:refs", "5:bus", "6:ledger"],
    );
    assert!(
        receipt.dek_destroyed_now,
        "step 2 destroyed the per-subject DEK"
    );
    assert_eq!(
        receipt.recoverable_in_backup, 0,
        "the per-subject ciphertext is 0 recoverable"
    );

    // THE GIT REACH RAN IN THE SAME CRYPTO-SHRED STEP: the per-tenant blob DEK is now gone, so git's
    // reflog/bitmap/pack-tier-backup ciphertext is unrecoverable live AND in backup.
    let blob_dek = myelin_storage::DekId::new(tenant.clone(), KeyClass::Blob);
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
        "the git crypto-shred reach destroyed the per-tenant blob DEK (0 recoverable in backup)"
    );
}

#[test]
fn cdc_git_reach_post_condition_is_verified_not_assumed() {
    // The reach is VERIFIED, not assumed (§5.2): the BlobShredReach seam returns Ok ONLY when the
    // post-condition (0 recoverable in backup) holds — and the receipt names the residual posture.
    let tenant = TenantId("acme".into());
    let subject = SubjectId::new("u-author");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    kms.ensure_dek(&tenant, &region(), KeyClass::Blob)
        .expect("blob dek");
    let git_reach = GitCryptoShredReach::new(&kms, region());

    // The seam succeeds (green) and really reaches the git structures.
    assert!(git_reach.shred_blob_tier(&subject, &tenant).is_ok());

    // The standalone receipt confirms the residual is the documented pseudonymous-by-default posture
    // (10.9, by reference) — NOT a byte-mutation of the commit objects, NOT a silent gap.
    let receipt = git_reach.shred_git_structures(&tenant);
    assert_eq!(receipt.residual, GitResidual::PseudonymousByDefault);
    assert!(receipt.is_green());
}

#[test]
fn cdc_subject_with_no_git_content_skips_the_reach_no_op() {
    // A subject who authored NO git content has git_reach=None — the per-subject free-text shred
    // (step 2 proper) still runs; the optional git reach is simply skipped (a no-op success).
    let tenant = TenantId("acme".into());
    let subject = SubjectId::new("u-chat-only");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    let cryptor = ColumnCryptor::new(&kms, region());
    cryptor
        .encrypt(
            &tenant,
            Some(&subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"chat body",
        )
        .expect("seal");
    let eraser = CryptoShredErase::new(&kms, region());
    let wiring = OrchestratorWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring,
        search: &wiring,
        refs: &wiring,
        bus: &wiring,
        ledger: &wiring,
        git_reach: None,
    };
    let receipt = eraser
        .erase(&subject, &tenant, &holders, 1)
        .expect("erase without git reach");
    assert!(
        receipt.dek_destroyed_now,
        "the per-subject free-text DEK was still destroyed"
    );
    assert!(receipt.is_green());
}
