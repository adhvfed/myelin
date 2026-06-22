//! Contract 11.5 CDC pair — the backup / archiving / PITR half (P-ST-11 / global P-059).
//!
//! The prompt requires "CDC: provider+consumer pair for 11.5 (the ops backup caller)". This is the
//! consumer-driven contract test:
//!
//! - the **PROVIDER** is `myelin-storage` — the [`ContinuousArchiver`] + [`BaseBackup`] +
//!   [`BackupSet`] + the [`StoreTier`] classification this prompt ships;
//! - the **CONSUMER** is the **ops backup caller** (modelled here as a tiny `OpsBackupJob`) that
//!   runs the periodic backup: it records the WAL commits the primary streams, continuously
//!   archives the WAL tail, takes a base backup, snapshots the backed-up tiers at the cross-seam
//!   offset, and reads the measured RPO to decide pass/fail. This is exactly the call shape the
//!   real ops/cron backup job (and the restore-verify gate P-061) relies on — if `archive_segment`
//!   / `take_base_backup` / `snapshot_tier` / `measure_rpo` drift, this stops compiling/passing.
//!
//! It also pins the load-bearing contract properties the consumer depends on: a DERIVED tier
//! cannot be added to a backup set (there is no backup-restore path for it), and a crypto-shredded
//! key is excluded from the backup (it stays dead across a restore, §7.5).
//!
//! NOTE on row 11.5: the contract-index row 11.5 spans the BACKUP half (this prompt, P-ST-11), the
//! `restore(to_offset)` + cross-seam half (P-ST-12 / global P-060), and the CI-wired restore-verify
//! GATE (P-ST-13 / global P-061). This CDC pair covers the BACKUP half (the ops caller that
//! archives + base-backs + snapshots the tiers + reads the RPO); P-060/P-061 add the restore caller
//! to the same row.

use myelin_storage::{
    BackupError, BackupSet, ContinuousArchiver, KekId, KeyClass, KmsEngine, StoreTier, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

/// A consumer of 11.5: the ops backup job that runs a periodic backup cycle. It drives the
/// provider exactly as the real cron/ops backup caller does.
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

    /// The primary streamed a commit up to `offset` at time `at` — the job records it (so it can
    /// measure the data-at-risk window).
    fn observe_commit(&mut self, offset: u64, at: u64) {
        self.archiver.record_commit(offset, at);
    }

    /// Continuously archive the WAL tail up to `offset` (freshness `at`).
    fn archive_up_to(&mut self, offset: u64, at: u64) -> Result<(), BackupError> {
        self.archiver.archive_segment(WalSegment {
            end_offset: offset,
            committed_at: at,
        })
    }

    /// Take a periodic base backup at the current archived tail.
    fn base_backup(&mut self, at: u64) {
        self.archiver.take_base_backup(at);
    }

    /// Snapshot the backed-up tiers at the cross-seam offset (the archived tail). The job MUST NOT
    /// include a derived tier (the provider rejects it). The crypto-shred exclusion is automatic.
    fn snapshot(&self, tiers: &[StoreTier]) -> Result<BackupSet, BackupError> {
        let offset = self.archiver.latest_archived_offset().unwrap_or(0);
        let mut set = BackupSet::new(offset, self.kms);
        for t in tiers {
            set.snapshot_tier(*t)?;
        }
        Ok(set)
    }

    /// The measured RPO (seconds of data at risk) — the number the job checks against its SLA.
    fn measured_rpo(&self) -> u64 {
        self.archiver.measure_rpo()
    }
}

/// **The 11.5 backup CDC happy path:** the ops caller archives the WAL tail, takes a base backup,
/// snapshots the systems-of-record tiers at the cross-seam offset, and reads an in-window RPO. The
/// contract shape the periodic ops backup + the restore-verify gate (P-061) both rely on.
#[test]
fn ops_backup_job_archives_base_backs_and_snapshots_within_rpo() {
    let kms = KmsEngine::new();
    // A live tenant with a key (so the backup has key material to carry).
    let tenant = TenantId("acme".into());
    let region = Region("eu-west".into());
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    kms.ensure_dek(&tenant, &region, KeyClass::Tenant).unwrap();

    let mut job = OpsBackupJob::boot(&kms);

    // The primary commits up to offset 500 at t=1000; the archiver ships the WAL tail at freshness
    // t=995 (5 s of lag) and takes a base backup.
    job.observe_commit(500, 1000);
    job.archive_up_to(500, 995).unwrap();
    job.base_backup(996);

    // The RPO is within the 5-min (300 s) bound (here 0 — the tail reached the committed offset).
    assert!(
        job.measured_rpo() <= 300,
        "RPO must be within the 5-min bound"
    );

    // Snapshot the systems-of-record tiers (T1/T2/T3/T5) — all admitted.
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

/// **The contract property the consumer depends on (1): a DERIVED tier has no backup-restore
/// path.** The ops caller cannot snapshot an OLAP / cache / derived index — the provider rejects it
/// (it is rebuilt from source). If this contract relaxed, a restore would wrongly try to restore a
/// derived store from its own backup (drift from source).
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

/// **The contract property the consumer depends on (2): a crypto-shredded key is excluded from a
/// fresh backup (§7.5).** The ops caller's `snapshot` carries only live-tenant key material — a
/// tenant whose KEK was destroyed (offboard / crypto-shred) is NOT in the backup, so a later
/// restore cannot resurrect it.
#[test]
fn ops_callers_backup_excludes_a_crypto_shredded_tenant() {
    let kms = KmsEngine::new();
    let region = Region("eu-west".into());
    let gone = TenantId("offboarded".into());
    let gone_kek = KekId::new(gone.clone(), region.clone());
    kms.ensure_kek(&gone_kek);
    kms.ensure_dek(&gone, &region, KeyClass::Tenant).unwrap();

    let job = OpsBackupJob::boot(&kms);
    // Before shred: the tenant's key is backed up.
    assert!(job
        .snapshot(&[StoreTier::Kms])
        .unwrap()
        .contains_key_for_tenant(&gone));

    // Crypto-shred (offboard).
    assert!(kms.destroy_kek(&gone_kek));

    // After shred: a fresh backup EXCLUDES the tenant — it stays dead across a restore.
    let after = job.snapshot(&[StoreTier::Kms]).unwrap();
    assert!(
        !after.contains_key_for_tenant(&gone),
        "a crypto-shredded tenant's key MUST be excluded from a fresh backup (§7.5)"
    );
}
