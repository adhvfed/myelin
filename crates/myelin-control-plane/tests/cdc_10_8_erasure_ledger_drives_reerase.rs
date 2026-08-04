use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef};
use myelin_gdpr_service::holders::{
    CryptoShredKms, InMemoryShredKms, ShredKeyClass, ShredKeyHandle,
};
use myelin_gdpr_service::orchestration::{
    holder_ids, EraseChecklist, SeamHolder, UpstreamHolderOrchestrator,
};
use myelin_gdpr_service::{
    DsrKind, DsrOrchestrator, DsrState, ErasureLedger, ErasureLedgerEntry, FanOutDriver,
    FanOutOutcome, Initiator, LegalHoldRegistry, PostPitRecord, Posture,
};

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::TestClock;

use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    restore_to_offset, BlobPresence, BusErase, ColumnCryptor, ContinuousArchiver, DekId,
    EpochMillis, EraseError, EraseHolders, ErasureLedgerSink, ErasureRecord, KekId, KeyClass,
    KmsEngine, PostRestoreErasureLedger, PseudonymShred, ReErasePass, RefsTombstone, SearchPurge,
    SourceLog, SubjectId, WalSegment,
};
use myelin_tenancy::{Region, TenantId as StorageTenantId};

use std::cell::RefCell;
use std::collections::BTreeSet;

struct GdprErasureLedgerSeam<'a> {
    ledger: &'a ErasureLedger,
}

impl PostRestoreErasureLedger for GdprErasureLedgerSeam<'_> {
    fn erasures_completed_after(&self, pit: myelin_storage::WalOffset) -> Vec<ErasureRecord> {
        self.ledger
            .post_pit_records_after(pit)
            .into_iter()
            .map(|r: PostPitRecord| {
                ErasureRecord::new(
                    SubjectId::new(&r.subject),
                    StorageTenantId(r.tenant),
                    r.completed_at_offset,
                )
            })
            .collect()
    }
}

fn region() -> Region {
    Region("eu-west".into())
}

fn gdpr_tenant() -> myelin_gdpr::TenantId {
    myelin_gdpr::TenantId::from_token("acme")
}

fn storage_tenant() -> StorageTenantId {
    StorageTenantId("acme".into())
}

fn subject_ref(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        gdpr_tenant(),
    ))
}

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject_ref(s),
        tenant: gdpr_tenant(),
    }
}

fn seed_gdpr_kms(base_epoch: u64) -> InMemoryShredKms {
    let kms = InMemoryShredKms::new();
    for (i, id) in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ]
    .iter()
    .enumerate()
    {
        kms.provision(
            ShredKeyHandle {
                tenant: gdpr_tenant(),
                class: ShredKeyClass::Subject((*id).to_string()),
            },
            base_epoch + i as u64,
        );
    }
    kms
}

fn gdpr_seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
    [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ]
    .into_iter()
    .map(|id| {
        (
            id,
            SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
        )
    })
    .collect()
}

fn inventory() -> myelin_gdpr_service::Inventory {
    use myelin_gdpr_service::{Inventory, InventoryEntry};
    let mut holders = BTreeSet::new();
    holders.insert(holder_ids::IDENTITY.to_string());
    Inventory {
        entries: vec![InventoryEntry {
            field_path: "PrincipalRow.email".into(),
            holder_id: holder_ids::IDENTITY.into(),
            holder: "H15".into(),
            region: "fr-par".into(),
            category: "ContactInfo".into(),
            role: "PlatformOperational".into(),
            basis: "Contract".into(),
            retention: "UntilContractEnd".into(),
            erasure: "CryptoShred(subject_dek)".into(),
            subject_locator: "principal_id".into(),
        }],
        holders,
        dpia_markers: BTreeSet::new(),
    }
}

fn complete_an_erase_and_record_to_ledger(
    ledger: &ErasureLedger,
    subject_id: &str,
    clock_secs: u64,
    base_epoch: u64,
) -> ErasureLedgerEntry {
    let kms = seed_gdpr_kms(base_epoch);
    let holders = gdpr_seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders
            .iter()
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect(),
    );
    let dsr = DsrOrchestrator::new(TestClock::at(clock_secs));
    let holds = LegalHoldRegistry::new();
    let driver = FanOutDriver::with_ledger(&dsr, &holds, ledger);

    let id = dsr.dsr_submit(
        DsrKind::Erasure,
        gdpr_tenant(),
        subject_ref(subject_id),
        subject_scope(subject_id),
        Posture::Controller,
        Initiator::Myelin,
    );
    assert!(dsr.validate(&id).unwrap());
    let outcome = driver
        .drive(&id, &inventory(), &upstream, &EraseChecklist::new())
        .unwrap();
    assert!(matches!(outcome, FanOutOutcome::Erased(_)));
    assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
    ledger
        .entry(&id)
        .expect("the completion wrote a ledger entry")
}

#[derive(Default)]
struct Seams {
    re_erased: RefCell<BTreeSet<String>>,
}
impl PseudonymShred for Seams {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &StorageTenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl SearchPurge for Seams {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &StorageTenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl RefsTombstone for Seams {
    fn tombstone(&self, _s: &SubjectId, _t: &StorageTenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl BusErase for Seams {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &StorageTenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl ErasureLedgerSink for Seams {
    fn record_erasure(&self, subject: &SubjectId, _t: &StorageTenantId, _at: EpochMillis) {
        self.re_erased.borrow_mut().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &StorageTenantId) -> bool {
        self.re_erased.borrow().contains(&subject.0)
    }
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

fn storage_kms_with_subject(subject: &SubjectId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(storage_tenant(), region()));
    ColumnCryptor::new(&kms, region())
        .encrypt(
            &storage_tenant(),
            Some(subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"to be re-erased on restore",
        )
        .expect("seal the per-subject column");
    kms
}

#[test]
fn cdc_gdpr_erasure_ledger_drives_storage_post_restore_reerase() {
    let subject_id = "u-post-pit";

    let ledger = ErasureLedger::new();
    let entry = complete_an_erase_and_record_to_ledger(&ledger, subject_id, 140, 1000);
    assert_eq!(
        entry.subject_token, subject_id,
        "the entry holds the opaque subject token (no PII)"
    );
    assert_eq!(entry.completed_at_offset, 140);

    let seam = GdprErasureLedgerSeam { ledger: &ledger };
    let post_pit = seam.erasures_completed_after(100);
    assert_eq!(
        post_pit.len(),
        1,
        "the seam reads ONE post-PIT erasure from the GDPR ledger"
    );
    assert_eq!(
        post_pit[0].subject,
        SubjectId::new(subject_id),
        "subject copied 1:1"
    );
    assert_eq!(post_pit[0].completed_at_offset, 140, "offset copied 1:1");

    let storage_subject = SubjectId::new(subject_id);
    let kms = storage_kms_with_subject(&storage_subject);
    let subject_dek = DekId::new(
        storage_tenant(),
        KeyClass::Subject(storage_subject.0.clone()),
    );
    assert!(
        kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the restore of T=100 RESURRECTED the subject's DEK (it was live at the older backup PIT)"
    );

    let arch = reachable_archiver(300);
    let report = restore_to_offset(
        &arch,
        100,
        &[],
        &BlobPresence::new(),
        &SourceLog::new(),
        &kms,
    )
    .unwrap();

    let seams = Seams::default();
    let holders = EraseHolders {
        pseudonym: &seams,
        search: &seams,
        refs: &seams,
        bus: &seams,
        ledger: &seams,
        git_reach: None,
    };
    let rep = ReErasePass::new(&kms, region())
        .run(&report, &seam, &holders, 1_000)
        .expect("the mandatory re-erasure pass succeeds");

    assert!(rep.is_green(), "0 resurrected → the restore is safe");
    assert_eq!(rep.resurrected_count, 0);
    assert!(
        rep.re_erased_subject(&storage_subject, &storage_tenant()),
        "the post-PIT subject was re-erased FROM the ledger"
    );
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the resurrected DEK is re-destroyed by the pass"
    );
}

#[test]
fn stor_d4_ga_face_erase_yields_zero_recoverable_in_backups() {
    let subject_id = "u-shred";
    let kms = seed_gdpr_kms(2000);
    let holders = gdpr_seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders
            .iter()
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect(),
    );
    let dsr = DsrOrchestrator::new(TestClock::at(500));
    let holds = LegalHoldRegistry::new();
    let ledger = ErasureLedger::new();
    let driver = FanOutDriver::with_ledger(&dsr, &holds, &ledger);

    let identity_handle = ShredKeyHandle {
        tenant: gdpr_tenant(),
        class: ShredKeyClass::Subject(holder_ids::IDENTITY.to_string()),
    };
    assert!(
        kms.recoverable_in_backup(&identity_handle) > 0,
        "before erase the key IS recoverable in the backup (the floor is not vacuous)"
    );

    let id = dsr.dsr_submit(
        DsrKind::Erasure,
        gdpr_tenant(),
        subject_ref(subject_id),
        subject_scope(subject_id),
        Posture::Controller,
        Initiator::Myelin,
    );
    assert!(dsr.validate(&id).unwrap());
    driver
        .drive(&id, &inventory(), &upstream, &EraseChecklist::new())
        .unwrap();
    assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);

    for hid in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ] {
        let handle = ShredKeyHandle {
            tenant: gdpr_tenant(),
            class: ShredKeyClass::Subject(hid.to_string()),
        };
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "STOR-D4-GA-face: holder `{hid}` has 0 recoverable PII in any backup (key destroyed, excluded)"
        );
    }

    assert_eq!(
        ledger.len(),
        1,
        "the erasure ledger recorded the completed erase"
    );
}

#[test]
fn stor_d3_id_d8_ga_face_restore_older_backup_re_erases_zero_resurrected() {
    let subject_id = "u-erased-after-backup";

    let ledger = ErasureLedger::new();
    complete_an_erase_and_record_to_ledger(&ledger, subject_id, 140, 3000);

    let storage_subject = SubjectId::new(subject_id);
    let kms = storage_kms_with_subject(&storage_subject);
    let subject_dek = DekId::new(
        storage_tenant(),
        KeyClass::Subject(storage_subject.0.clone()),
    );
    assert!(
        kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the restore of the OLDER backup resurrected the subject's DEK"
    );

    let arch = reachable_archiver(300);
    let report = restore_to_offset(
        &arch,
        100,
        &[],
        &BlobPresence::new(),
        &SourceLog::new(),
        &kms,
    )
    .unwrap();

    let seam = GdprErasureLedgerSeam { ledger: &ledger };
    let seams = Seams::default();
    let holders = EraseHolders {
        pseudonym: &seams,
        search: &seams,
        refs: &seams,
        bus: &seams,
        ledger: &seams,
        git_reach: None,
    };
    let rep = ReErasePass::new(&kms, region())
        .run(&report, &seam, &holders, 2_000)
        .expect("the post-restore re-erasure pass succeeds");

    assert!(
        rep.is_green(),
        "0 resurrected after restoring the older backup"
    );
    assert_eq!(
        rep.resurrected_count, 0,
        "STOR-D3/ID-D8-GA-face: 0 resurrected subjects"
    );
    assert_eq!(
        rep.re_erased_count(),
        1,
        "exactly the one post-PIT subject was re-erased"
    );
    assert!(
        rep.re_erased[0].was_resurrected_before_reapply,
        "the subject WAS resurrected by the restore, then re-killed (the re-erasure receipt)"
    );
    assert!(rep.re_erased_subject(&storage_subject, &storage_tenant()));
    assert!(
        seams.is_erased(&storage_subject, &storage_tenant()),
        "the re-erasure re-recorded the erasure"
    );
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the resurrected DEK is re-destroyed - the restore did NOT resurrect the subject"
    );
}

#[test]
fn the_erasure_ledger_survives_its_own_subjects_erase_and_still_drives_re_erasure() {
    let subject_id = "u-recursive";
    let ledger = ErasureLedger::new();
    complete_an_erase_and_record_to_ledger(&ledger, subject_id, 140, 4000);
    assert_eq!(ledger.len(), 1);

    ledger.erase(subject_scope(subject_id)).unwrap();
    assert_eq!(
        ledger.len(),
        1,
        "the ledger erase RETAINED the PII-free record (non-shred-erasable)"
    );

    let seam = GdprErasureLedgerSeam { ledger: &ledger };
    assert_eq!(
        seam.erasures_completed_after(100).len(),
        1,
        "the retained record STILL drives post-restore re-erasure after the subject's own erase"
    );
}
