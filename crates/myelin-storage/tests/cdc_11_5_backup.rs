use myelin_storage::{
    BackupError, BackupSet, ContinuousArchiver, KekId, KeyClass, KmsEngine, StoreTier, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

struct OpsBackupJob<'a> {
    archiver: ContinuousArchiver,
    kms: &'a KmsEngine,
}

impl<'a> OpsBackupJob<'a> {
    fn boot(kms: &'a KmsEngine) -> Self {
        OpsBackupJob {
            archiver: ContinuousArchiver::new(),
            kms,
        }
    }

    fn observe_commit(&mut self, offset: u64, at: u64) {
        self.archiver.record_commit(offset, at);
    }

    fn archive_up_to(&mut self, offset: u64, at: u64) -> Result<(), BackupError> {
        self.archiver.archive_segment(WalSegment {
            end_offset: offset,
            committed_at: at,
        })
    }

    fn base_backup(&mut self, at: u64) {
        self.archiver.take_base_backup(at);
    }

    fn snapshot(&self, tiers: &[StoreTier]) -> Result<BackupSet, BackupError> {
        let offset = self.archiver.latest_archived_offset().unwrap_or(0);
        let mut set = BackupSet::new(offset, self.kms);
        for t in tiers {
            set.snapshot_tier(*t)?;
        }
        Ok(set)
    }

    fn measured_rpo(&self) -> u64 {
        self.archiver.measure_rpo()
    }
}

#[test]
fn ops_backup_job_archives_base_backs_and_snapshots_within_rpo() {
    let kms = KmsEngine::new();
    let tenant = TenantId("acme".into());
    let region = Region("eu-west".into());
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    kms.ensure_dek(&tenant, &region, KeyClass::Tenant).unwrap();

    let mut job = OpsBackupJob::boot(&kms);

    job.observe_commit(500, 1000);
    job.archive_up_to(500, 995).unwrap();
    job.base_backup(996);

    assert!(
        job.measured_rpo() <= 300,
        "RPO must be within the 5-min bound"
    );

    let set = job
        .snapshot(&[
            StoreTier::Oltp,
            StoreTier::Object,
            StoreTier::Log,
            StoreTier::Kms,
        ])
        .expect("systems of record are backed up");
    assert_eq!(
        set.at_offset, 500,
        "the backup is consistent at the archived cross-seam offset"
    );
    assert!(
        set.contains_key_for_tenant(&tenant),
        "the live tenant's key is in the backup"
    );
    assert_eq!(set.backed_tiers().len(), 4);
}

#[test]
fn ops_caller_cannot_back_up_a_derived_tier() {
    let kms = KmsEngine::new();
    let job = OpsBackupJob::boot(&kms);
    let err = job
        .snapshot(&[StoreTier::Oltp, StoreTier::Olap])
        .expect_err("a derived tier must be rejected from the backup set");
    assert_eq!(
        err,
        BackupError::DerivedTierNotBacked {
            tier: StoreTier::Olap
        }
    );
}

#[test]
fn ops_callers_backup_excludes_a_crypto_shredded_tenant() {
    let kms = KmsEngine::new();
    let region = Region("eu-west".into());
    let gone = TenantId("offboarded".into());
    let gone_kek = KekId::new(gone.clone(), region.clone());
    kms.ensure_kek(&gone_kek);
    kms.ensure_dek(&gone, &region, KeyClass::Tenant).unwrap();

    let job = OpsBackupJob::boot(&kms);
    assert!(job
        .snapshot(&[StoreTier::Kms])
        .unwrap()
        .contains_key_for_tenant(&gone));

    assert!(kms.destroy_kek(&gone_kek));

    let after = job.snapshot(&[StoreTier::Kms]).unwrap();
    assert!(
        !after.contains_key_for_tenant(&gone),
        "a crypto-shredded tenant's key MUST be excluded from a fresh backup (§7.5)"
    );
}
