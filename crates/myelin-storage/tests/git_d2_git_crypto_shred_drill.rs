use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    BusErase, ColumnCryptor, CryptoShredErase, DekId, EpochMillis, EraseError, EraseHolders,
    ErasureLedgerSink, GitCryptoShredReach, GitResidual, GitShreddable, KekId, KeyClass, KmsEngine,
    PiiKeyRef, PseudonymShred, RefsTombstone, SearchPurge, SubjectId,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

fn region() -> Region {
    Region("eu-west".into())
}

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

fn seal_git_structure(
    kms: &KmsEngine,
    tenant: &TenantId,
    bytes: &[u8],
) -> (PiiKeyRef, [u8; 12], Vec<u8>) {
    let key_ref = PiiKeyRef::new(tenant.clone(), 0, KeyClass::Blob);
    let dek = kms
        .resolve_dek(&key_ref, &region())
        .expect("resolve blob dek");
    let (nonce, ct) = dek.seal(bytes);
    (key_ref, nonce, ct)
}

#[test]
fn git_d2_storage_half_erase_commit_author_zero_recoverable_git_structures_residual_is_the_posture()
{
    let tenant = TenantId("acme".into());
    let author = SubjectId::new("u-commit-author");

    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    let cryptor = ColumnCryptor::new(&kms, region());
    let body_col = cryptor
        .encrypt(
            &tenant,
            Some(&author),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"the author's commit message body (free text PII)",
        )
        .expect("seal the author's free-text body");
    kms.ensure_dek(&tenant, &region(), KeyClass::Blob)
        .expect("blob dek");

    let reflog = seal_git_structure(
        &kms,
        &tenant,
        b"refs/heads/main 0000 abcd <pseudonym>@acme.noreply pushed",
    );
    let bitmap = seal_git_structure(
        &kms,
        &tenant,
        b"\x42\x49\x54\x4d pack reachability bitmap index",
    );
    let pack_backup =
        seal_git_structure(&kms, &tenant, b"PACK\0\0\0\x02 a pack-tier backup object");

    assert!(
        cryptor.decrypt(&body_col).is_ok(),
        "the author body decrypts before erase"
    );
    let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);
    for (kr, nonce, ct) in [&reflog, &bitmap, &pack_backup] {
        let dek = kms
            .resolve_dek(kr, &region())
            .expect("git structure DEK resolves before erase");
        assert!(
            dek.open(nonce, ct).is_some(),
            "git structure decrypts before erase"
        );
    }
    assert!(
        kms.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
        "the per-tenant blob DEK is in the backup before the git crypto-shred"
    );

    let git_reach = GitCryptoShredReach::new(&kms, region());
    let git_receipt = git_reach.shred_git_structures(&tenant);
    assert!(
        git_receipt.blob_dek_destroyed_now,
        "the GIT-D2 reach destroyed the per-tenant blob DEK"
    );

    let eraser = CryptoShredErase::new(&kms, region());
    let wiring = DrillWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring,
        search: &wiring,
        refs: &wiring,
        bus: &wiring,
        ledger: &wiring,
        git_reach: Some(&git_reach),
    };
    let erase_receipt = eraser
        .erase(&author, &tenant, &holders, 1_718_000_000_000)
        .expect("the commit author's erase completes (incl. the git reach)");
    assert!(
        erase_receipt.is_green(),
        "the per-subject crypto-shred is green"
    );

    for (kr, _nonce, _ct) in [&reflog, &bitmap, &pack_backup] {
        if kms.resolve_dek(kr, &region()).is_ok() {
            panic!(
                "GIT-D2 BREACH: a git structure's DEK STILL RESOLVES after the crypto-shred - a \
                 reflog/bitmap/pack-backup could be recovered live"
            );
        }
    }

    if kms.backup_snapshot().iter().any(|(d, _)| *d == blob_dek) {
        panic!(
            "GIT-D2 BREACH: the per-tenant blob DEK is STILL in the backup snapshot - a backup \
             could resurrect a git reflog/bitmap/pack object (0-recoverable-in-backup violated)"
        );
    }
    assert_eq!(
        git_receipt.recoverable_in_backup, 0,
        "GIT-D2 (storage half): 0 git structures recoverable from any backup"
    );

    assert_eq!(git_receipt.structures_reached, GitShreddable::ALL.to_vec());

    assert_eq!(
        git_receipt.residual,
        GitResidual::PseudonymousByDefault,
        "GIT-D2: the residual is the documented pseudonymous-by-default posture, never a byte-mutation"
    );
    assert!(git_receipt.is_green(), "GIT-D2 (storage half) GREEN");

    println!(
        "GIT-D2 (storage half) GREEN @ 2026-06-21 (P-253 / P-ST-24): \
         tenant={tenant} commit_author={author} \
         git_structures_reached={n} (reflog,bitmap,pack-tier-backup) \
         recoverable_in_backup={rec} (0 = crypto-shred reached backups by construction, §7.5) \
         residual={residual:?} (pseudonymous-by-default, 10.9 by reference; history-rewrite 10.6 = named follow-on) \
         blob_dek_destroyed={destroyed}",
        tenant = tenant.as_str(),
        author = author.as_str(),
        n = git_receipt.structures_reached.len(),
        rec = git_receipt.recoverable_in_backup,
        residual = git_receipt.residual,
        destroyed = git_receipt.blob_dek_destroyed_now,
    );
}

#[test]
fn git_d2_residual_is_handled_by_reference_never_a_storage_local_restatement() {
    assert!(GitResidual::RESIDUAL_POSTURE_REF.contains("10.9"));
    assert!(GitResidual::RESIDUAL_POSTURE_REF.contains("pseudonymous-by-default"));
    assert!(
        GitResidual::RESIDUAL_POSTURE_REF.contains("10.6"),
        "the on-demand audited history-rewrite follow-on is NAMED"
    );
}
