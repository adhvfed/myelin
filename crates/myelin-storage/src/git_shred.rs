use myelin_tenancy::{Region, TenantId};

use crate::erase::{BlobShredReach, EraseError};
use crate::kms::{DekId, KeyClass, KmsEngine};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitShreddable {
    Reflog,
    Bitmap,
    PackTierBackup,
}

impl GitShreddable {
    pub fn label(self) -> &'static str {
        match self {
            GitShreddable::Reflog => "reflog",
            GitShreddable::Bitmap => "bitmap",
            GitShreddable::PackTierBackup => "pack-tier-backup",
        }
    }

    pub const ALL: [GitShreddable; 3] = [
        GitShreddable::Reflog,
        GitShreddable::Bitmap,
        GitShreddable::PackTierBackup,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitResidual {
    PseudonymousByDefault,
}

impl GitResidual {
    pub const RESIDUAL_POSTURE_REF: &'static str =
        "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
         git commit bytes = pseudonymous-by-default (Id 4.8); on-demand history-rewrite = 10.6";
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitShredReceipt {
    pub tenant: TenantId,
    pub blob_dek_destroyed_now: bool,
    pub recoverable_in_backup: usize,
    pub structures_reached: Vec<GitShreddable>,
    pub residual: GitResidual,
}

impl GitShredReceipt {
    pub fn is_green(&self) -> bool {
        self.recoverable_in_backup == 0
            && self.residual == GitResidual::PseudonymousByDefault
            && self.structures_reached.len() == GitShreddable::ALL.len()
    }
}

pub struct GitCryptoShredReach<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> GitCryptoShredReach<'a> {
    pub fn new(engine: &'a KmsEngine, region: Region) -> GitCryptoShredReach<'a> {
        GitCryptoShredReach { engine, region }
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    fn blob_dek_id(tenant: &TenantId) -> DekId {
        DekId::new(tenant.clone(), KeyClass::Blob)
    }

    pub fn shred_git_structures(&self, tenant: &TenantId) -> GitShredReceipt {
        let blob_dek = Self::blob_dek_id(tenant);

        let blob_dek_destroyed_now = self.engine.destroy_dek(&blob_dek);

        let recoverable_in_backup = self
            .engine
            .backup_snapshot()
            .iter()
            .filter(|(d, _)| *d == blob_dek)
            .count();

        GitShredReceipt {
            tenant: tenant.clone(),
            blob_dek_destroyed_now,
            recoverable_in_backup,
            structures_reached: GitShreddable::ALL.to_vec(),
            residual: GitResidual::PseudonymousByDefault,
        }
    }
}

impl BlobShredReach for GitCryptoShredReach<'_> {
    fn shred_blob_tier(
        &self,
        _subject: &crate::encryption::SubjectId,
        tenant: &TenantId,
    ) -> Result<(), EraseError> {
        let receipt = self.shred_git_structures(tenant);
        if receipt.is_green() {
            Ok(())
        } else {
            Err(EraseError::BlobShredReach(format!(
                "git crypto-shred reach for tenant `{}` is NOT green: {} git structure(s) still \
                 recoverable in backup (the per-tenant blob DEK was not excluded) - the erase is \
                 ABORTED as INCOMPLETE (a reflog/bitmap/pack backup could resurrect the structure)",
                tenant.as_str(),
                receipt.recoverable_in_backup,
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::SubjectId;
    use crate::kms::{KekId, PiiKeyRef};
    use std::sync::Arc;

    fn t() -> TenantId {
        TenantId("acme".into())
    }
    fn r() -> Region {
        Region("eu-west".into())
    }

    fn engine_with_blob_dek(tenant: &TenantId) -> Arc<KmsEngine> {
        let kms = Arc::new(KmsEngine::new());
        kms.ensure_kek(&KekId::new(tenant.clone(), r()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(tenant, &r(), KeyClass::Blob)
            .expect("blob dek");
        kms
    }

    fn seal_git_structure(
        engine: &KmsEngine,
        tenant: &TenantId,
        bytes: &[u8],
    ) -> (PiiKeyRef, [u8; 12], Vec<u8>) {
        let key_ref = PiiKeyRef::new(tenant.clone(), 0, KeyClass::Blob);
        let dek = engine
            .resolve_dek(&key_ref, &r())
            .expect("resolve blob dek");
        let (nonce, ct) = dek.seal(bytes);
        (key_ref, nonce, ct)
    }

    #[test]
    fn reach_destroys_the_blob_dek_and_renders_git_structures_unrecoverable() {
        let tenant = t();
        let engine = engine_with_blob_dek(&tenant);

        let reflog = b"refs/heads/main 0000 abcd <pseudonym>@acme.noreply pushed";
        let (key_ref, nonce, ct) = seal_git_structure(&engine, &tenant, reflog);
        let dek_before = engine
            .resolve_dek(&key_ref, &r())
            .expect("blob dek resolves before shred");
        assert_eq!(
            dek_before.open(&nonce, &ct).expect("decrypts before shred"),
            reflog
        );

        let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);
        assert!(
            engine.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
            "the blob DEK is in the backup before the git shred"
        );

        let reach = GitCryptoShredReach::new(&engine, r());
        let receipt = reach.shred_git_structures(&tenant);

        assert!(
            receipt.blob_dek_destroyed_now,
            "the per-tenant blob DEK was destroyed"
        );
        assert_eq!(
            receipt.recoverable_in_backup, 0,
            "GIT-D2: 0 git structures recoverable in backup"
        );
        assert_eq!(receipt.residual, GitResidual::PseudonymousByDefault);
        assert!(receipt.is_green(), "GIT-D2 (storage half) green");

        assert!(
            !engine.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
            "the blob DEK is absent from the backup after the git shred (0 recoverable, §7.5)"
        );
        assert!(
            engine.resolve_dek(&key_ref, &r()).is_err(),
            "the git structure is unrecoverable after the crypto-shred (live): the blob DEK is gone"
        );
    }

    #[test]
    fn reach_covers_every_shreddable_structure_and_names_the_residual() {
        let tenant = t();
        let engine = engine_with_blob_dek(&tenant);
        let reach = GitCryptoShredReach::new(&engine, r());
        let receipt = reach.shred_git_structures(&tenant);

        assert_eq!(receipt.structures_reached, GitShreddable::ALL.to_vec());
        assert!(receipt.structures_reached.contains(&GitShreddable::Reflog));
        assert!(receipt.structures_reached.contains(&GitShreddable::Bitmap));
        assert!(receipt
            .structures_reached
            .contains(&GitShreddable::PackTierBackup));
        assert_eq!(receipt.residual, GitResidual::PseudonymousByDefault);
    }

    #[test]
    fn reach_is_idempotent_a_second_shred_is_a_noop_success() {
        let tenant = t();
        let engine = engine_with_blob_dek(&tenant);
        let reach = GitCryptoShredReach::new(&engine, r());

        let r1 = reach.shred_git_structures(&tenant);
        assert!(
            r1.blob_dek_destroyed_now,
            "first shred destroys the blob DEK"
        );
        assert!(r1.is_green());

        let r2 = reach.shred_git_structures(&tenant);
        assert!(
            !r2.blob_dek_destroyed_now,
            "the blob DEK was already destroyed (idempotent re-run)"
        );
        assert_eq!(r2.recoverable_in_backup, 0, "still 0 recoverable in backup");
        assert!(r2.is_green());
    }

    #[test]
    fn reach_wired_as_the_blob_shred_seam_succeeds_when_green() {
        let tenant = t();
        let engine = engine_with_blob_dek(&tenant);
        let reach = GitCryptoShredReach::new(&engine, r());
        let subject = SubjectId::new("u-commit-author");
        assert!(
            reach.shred_blob_tier(&subject, &tenant).is_ok(),
            "the git shred reach as the erase seam succeeds when green"
        );
        let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);
        assert!(!engine.backup_snapshot().iter().any(|(d, _)| *d == blob_dek));
    }

    #[test]
    fn receipt_is_green_only_when_zero_recoverable_and_residual_is_the_posture() {
        let green = GitShredReceipt {
            tenant: t(),
            blob_dek_destroyed_now: true,
            recoverable_in_backup: 0,
            structures_reached: GitShreddable::ALL.to_vec(),
            residual: GitResidual::PseudonymousByDefault,
        };
        assert!(green.is_green());
        let red = GitShredReceipt {
            recoverable_in_backup: 1,
            ..green.clone()
        };
        assert!(
            !red.is_green(),
            "a recoverable git structure in backup is RED"
        );
        let dropped = GitShredReceipt {
            structures_reached: vec![GitShreddable::Reflog],
            ..green.clone()
        };
        assert!(!dropped.is_green(), "a missed git structure is RED");
    }

    #[test]
    fn shreddable_labels_and_residual_ref_are_stable_and_pii_free() {
        assert_eq!(GitShreddable::Reflog.label(), "reflog");
        assert_eq!(GitShreddable::Bitmap.label(), "bitmap");
        assert_eq!(GitShreddable::PackTierBackup.label(), "pack-tier-backup");
        assert_eq!(GitShreddable::ALL.len(), 3);
        assert!(GitResidual::RESIDUAL_POSTURE_REF.contains("10.9"));
        assert!(GitResidual::RESIDUAL_POSTURE_REF.contains("pseudonymous-by-default"));
        assert!(
            GitResidual::RESIDUAL_POSTURE_REF.contains("10.6"),
            "names the history-rewrite follow-on"
        );
    }

    #[test]
    fn region_accessor_returns_the_kek_region() {
        let kms = KmsEngine::new();
        let reach = GitCryptoShredReach::new(&kms, r());
        assert_eq!(reach.region(), &r());
    }
}
