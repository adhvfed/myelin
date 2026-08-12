use myelin_gdpr::ErasureMethod;
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::{
    BusErase, ColumnCryptor, EpochMillis, EraseError, EraseHolders, ErasureLedgerSink,
    FullHolderFanOut, GitCryptoShredReach, HolderClass, KekId, KeyClass, KmsEngine, PseudonymShred,
    RefsTombstone, SearchPurge, SubjectId,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

fn region() -> Region {
    Region("fr-par".into())
}

#[derive(Default)]
struct Seams {
    erased: RefCell<BTreeSet<String>>,
}
impl PseudonymShred for Seams {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl SearchPurge for Seams {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl RefsTombstone for Seams {
    fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl BusErase for Seams {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl ErasureLedgerSink for Seams {
    fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.erased.borrow_mut().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
        self.erased.borrow().contains(&subject.0)
    }
}

fn holders(seams: &Seams) -> EraseHolders<'_> {
    EraseHolders {
        pseudonym: seams,
        search: seams,
        refs: seams,
        bus: seams,
        ledger: seams,
        git_reach: None,
    }
}

fn holders_with_git_reach<'a>(
    seams: &'a Seams,
    git_reach: &'a GitCryptoShredReach<'a>,
) -> EraseHolders<'a> {
    EraseHolders {
        pseudonym: seams,
        search: seams,
        refs: seams,
        bus: seams,
        ledger: seams,
        git_reach: Some(git_reach),
    }
}

fn engine_seeded_across_holders(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    let cryptor = ColumnCryptor::new(&kms, region());
    cryptor
        .encrypt(
            tenant,
            Some(subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"alice PII seeded into every holder (oltp/chat/ci/agent/knowledge free-text)",
        )
        .expect("seal the per-subject free-text column");
    kms.ensure_dek(tenant, &region(), KeyClass::Blob)
        .expect("create the per-tenant blob DEK");
    kms
}

#[test]
fn e2e4_full_holder_fanout_is_green() {
    let tenant = TenantId::from_token("01J0ACME");
    let subject = SubjectId::new("u-e2e4-green");
    let kms = engine_seeded_across_holders(&tenant, &subject);
    let fanout = FullHolderFanOut::new(&kms, region());
    let seams = Seams::default();
    let git_reach = GitCryptoShredReach::new(&kms, region());

    let set = fanout
        .fan_out(
            &subject,
            &tenant,
            &holders_with_git_reach(&seams, &git_reach),
            1_000,
        )
        .expect("the full-holder fan-out succeeds");

    let holders_missed = set.holders_missed();
    let recoverable = set.recoverable_pii();

    assert_eq!(set.coverages.len(), 18, "one coverage per H1–H18 holder");
    assert_eq!(holders_missed, 0, "0 holders missed (the E2E-4 zero)");
    assert_eq!(recoverable, 0, "0 recoverable PII across every holder");
    assert!(
        set.vectors_purged(),
        "embeddings purged, not hidden (incl. vectors)"
    );
    assert!(
        set.backups_clean(),
        "0 recoverable in any backup (incl. backups)"
    );
    assert!(
        set.residual.is_documented(),
        "residual == the one documented posture"
    );
    assert!(
        set.is_complete(),
        "the holder-coverage set is COMPLETE + green"
    );

    let cert = set.seal_certificate();
    assert!(
        cert.sealed,
        "the E2E-4 certificate is sealed on a green fan-out"
    );
    assert!(cert.is_green(), "the certificate is green (0/0, sealed)");
    assert_eq!(cert.digest, set.seal_certificate().digest);

    for cov in &set.coverages {
        assert!(
            cov.is_green(),
            "{} ({}) green",
            cov.holder.h_number(),
            cov.holder.holder_id()
        );
    }

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, holders_missed as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    let mut rsig = SignalSource::new();
    rsig.set_scalar(SignalName::CrossTenantCount, recoverable as i64);
    rsig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-446 E2E-4 GREEN 2026-06-24] storage DSAR full-holder fan-out: a single erasure reached \
         all {} H1–H18 holders incl. vectors (H8 search embeddings PURGED, not hidden) incl. backups \
         (H18 0 recoverable by construction), holders_missed={} (the E2E-4 zero), recoverable_pii={}, \
         residual==the one documented posture (10.9), certificate sealed (digest={}). {}",
        HolderClass::ALL.len(),
        holders_missed,
        recoverable,
        cert.digest.to_multihash_string(),
        set.summary(),
    );
}

#[test]
fn e2e4_gate_is_not_vacuous_a_withheld_holder_reads_red() {
    let tenant = TenantId::from_token("01J0ACME");
    let subject = SubjectId::new("u-e2e4-red");
    let kms = engine_seeded_across_holders(&tenant, &subject);
    let fanout = FullHolderFanOut::new(&kms, region());
    let seams = Seams::default();

    let set = fanout
        .fan_out_withholding(
            &subject,
            &tenant,
            &holders(&seams),
            1,
            &[HolderClass::CiLogs],
        )
        .unwrap();

    let holders_missed = set.holders_missed();
    assert_eq!(
        holders_missed, 1,
        "the withheld holder is MISSED (not dropped)"
    );
    assert!(
        set.recoverable_pii() >= 1,
        "the withheld holder leaves a recoverable key"
    );
    assert!(!set.is_complete(), "an incomplete fan-out is RED");
    assert!(
        !set.residual.is_documented(),
        "an undocumented residual is RED"
    );

    let cert = set.seal_certificate();
    assert!(!cert.sealed, "a red fan-out seals a non-sealed certificate");
    assert!(!cert.is_green());

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, holders_missed as i64);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a missed holder in a DSAR fan-out MUST read RED - the E2E-4 zero is a real tripwire"
    );
}

#[test]
fn e2e4_full_holder_fanout_is_idempotent() {
    let tenant = TenantId::from_token("01J0ACME");
    let subject = SubjectId::new("u-e2e4-idem");
    let kms = engine_seeded_across_holders(&tenant, &subject);
    let fanout = FullHolderFanOut::new(&kms, region());
    let seams = Seams::default();
    let git_reach = GitCryptoShredReach::new(&kms, region());

    let first = fanout
        .fan_out(
            &subject,
            &tenant,
            &holders_with_git_reach(&seams, &git_reach),
            1,
        )
        .unwrap();
    assert!(first.is_complete());
    assert!(
        first.erase_receipt.dek_destroyed_now,
        "the first pass destroys the DEK"
    );

    let second = fanout
        .fan_out(
            &subject,
            &tenant,
            &holders_with_git_reach(&seams, &git_reach),
            2,
        )
        .unwrap();
    assert_eq!(
        second.holders_missed(),
        0,
        "the re-run still misses 0 holders"
    );
    assert_eq!(
        second.recoverable_pii(),
        0,
        "still 0 recoverable after the re-run"
    );
    assert!(second.is_complete(), "the re-run is still complete + green");
    assert!(
        second.erase_receipt.re_run,
        "the second fan-out is an idempotent re-run"
    );
    assert!(
        !second.erase_receipt.dek_destroyed_now,
        "no DEK destroyed the second pass (already gone)"
    );
}

#[test]
fn e2e4_post_restore_reerase_across_full_holder_set_is_green() {
    use myelin_storage::{
        restore_to_offset, BlobPresence, ContinuousArchiver, SourceLog, WalSegment,
    };
    use myelin_storage::{DekId, KeyClass};
    use myelin_storage::{ErasureRecord, InMemoryPostPitLedger};

    let tenant = TenantId::from_token("01J0ACME");
    let subject = SubjectId::new("u-erased-after-backup");
    let kms = engine_seeded_across_holders(&tenant, &subject);
    let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
    assert!(
        kms.backup_snapshot()
            .unwrap()
            .iter()
            .any(|(d, _)| *d == subject_dek),
        "the restore resurrected the subject DEK (it was live at the backup PIT)"
    );

    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment {
        end_offset: 0,
        committed_at: 0,
    })
    .unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment {
        end_offset: 300,
        committed_at: 10,
    })
    .unwrap();
    let report = restore_to_offset(
        &arch,
        100,
        &[],
        &BlobPresence::new(),
        &SourceLog::new(),
        &kms,
    )
    .unwrap();

    let mut ledger = InMemoryPostPitLedger::new();
    ledger.record(ErasureRecord::new(subject.clone(), tenant.clone(), 140));

    let fanout = FullHolderFanOut::new(&kms, region());
    let seams = Seams::default();
    let rep = fanout
        .reerase_after_restore(&report, &ledger, &holders(&seams), 1_000)
        .expect("the post-restore re-erasure pass succeeds across the full holder set");

    assert!(
        rep.is_green(),
        "0 resurrected subjects after the pass (§7.5)"
    );
    assert_eq!(rep.resurrected_count, 0);
    assert!(rep.re_erased_subject(&subject, &tenant));
    assert!(
        !kms.backup_snapshot()
            .unwrap()
            .iter()
            .any(|(d, _)| *d == subject_dek),
        "the resurrected DEK is re-destroyed across the holder set"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, rep.resurrected_count as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-446 STOR-D3 (full-holder) GREEN 2026-06-24] storage post-restore re-erasure: a restore of \
         an older backup (PIT=100) resurrected a subject erased at offset 140; the re-erasure pass \
         re-applied the crypto-shred across the full holder set → resurrected_count={} (still erased).",
        rep.resurrected_count,
    );
}
