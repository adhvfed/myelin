//! # Contract 10.8 CDC pair + STOR-D4/STOR-D3-GA-face drills (P-GA-15 → P-115)
//!
//! **DATED GREEN ARTIFACT (2026-06-20).** This file is the dated green artifact the P-GA-15 GATE
//! requires — the cross-subsystem proof that the GDPR-owned **erasure ledger (10.8)** drives
//! Storage's **`post_restore_reerase` (11.5)** so a restore never resurrects an erased subject. The
//! control-plane is the ONLY crate that sees BOTH subsystems (the GDPR service is a leaf consumer
//! ABOVE the library DAG that cannot import Storage — the no-cross-store-read law — and Storage
//! cannot import the GDPR service — an upward DAG edge), so the seam that wires them is proven HERE
//! (the cell-orchestration / boot home that drives restores).
//!
//! ## The CDC pair (contract 10.8)
//! - **PROVIDER** = `myelin-gdpr-service` — the [`ErasureLedger`] this prompt ships: on a DSR
//!   completion the fan-out driver writes one PII-free [`ErasureLedgerEntry`] (the opaque subject +
//!   holders + destroyed key epochs + the cross-seam completion offset); the ledger exposes the
//!   post-PIT erasures via [`ErasureLedger::post_pit_records_after`].
//! - **CONSUMER** = `myelin-storage` — the `post_restore_reerase` mechanism (P-100): the
//!   [`ReErasePass`] re-applies every erasure the [`PostRestoreErasureLedger`] seam records as
//!   completed AFTER the restore's PIT and asserts 0 resurrected.
//!
//! The seam binding is the [`GdprErasureLedgerSeam`] adapter below — it implements Storage's
//! [`PostRestoreErasureLedger`] trait by reading the GDPR ledger's [`PostPitRecord`]s and mapping
//! them 1:1 to Storage's [`ErasureRecord`] (the field shapes — `{subject, tenant,
//! completed_at_offset}` — mirror exactly, so this is a field copy, not a translation that can
//! drift). If the ledger's read shape, the `PostPitRecord` fields, or Storage's `ErasureRecord` /
//! `PostRestoreErasureLedger` contract drift, this stops compiling/passing.
//!
//! ## The drills (the prompt's GATE)
//! - **STOR-D4 (GA face)** — erase a subject; attempt recovery from backups → per-subject ciphertext
//!   unrecoverable (key destroyed, excluded from backup); **0 recoverable PII in any backup**.
//! - **STOR-D3 / ID-D8 (GA face)** — erase; restore an *older* backup → the subject is still erased
//!   (post-restore re-erasure ran FROM the ledger); **0 resurrected**; the re-erasure receipt is the
//!   green artifact.
//!
//! Measured (below): STOR-D4-GA-face — **0 recoverable in the backup snapshot** after the shred;
//! STOR-D3/ID-D8-GA-face — a restore to an offset BEFORE the erasure resurrects the DEK, the ledger
//! drives re-erasure, **0 resurrected**, re-erasure receipt emitted.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3
//! - The drills run at **M1 scale** here; they re-run at **CELL scale at M5** (P-GA-32 → P-505, under
//!   world-scale load) + the full **H1–H18 fan-out (GA-D1)** is M5. NAMED.
//! - The live `pg_restore` + WAL-replay restore driver + the durable Postgres `erasure_ledger` table
//!   (excluded from the crypto-shred) is the named **P-S12/P-S15** storage floor; the seam shape
//!   ([`PostRestoreErasureLedger`]) + the ledger read shape do not change when it lands. On this floor
//!   the completion offset is the GDPR ledger's monotone completion-timestamp surrogate for the §7.3
//!   WAL cursor (Storage's `ErasureRecord.completed_at_offset` carries the same value).
//! - No `--features integration` live-stack leg is owed: this re-proves over the SAME faithful
//!   in-memory `KmsEngine` + `InMemoryPostPitLedger`-shaped seam the storage `reerase` module
//!   (P-100) already exercises; this prompt adds the GDPR-ledger PROVIDER side of the seam (the live
//!   `pg_restore` binding is the named storage floor, not a GDPR-owned DB contract).

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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// THE SEAM ADAPTER — the contract-10.8 ⇄ 11.5 binding the control-plane (boot/cell-orchestration)
// owns: read the GDPR-owned erasure ledger's post-PIT records and present them to Storage's
// `post_restore_reerase` mechanism as `ErasureRecord`s. A 1:1 field copy (the shapes mirror).
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The control-plane's binding of the GDPR erasure ledger (10.8) into Storage's
/// [`PostRestoreErasureLedger`] seam (11.5). It reads the GDPR ledger's [`PostPitRecord`]s and maps
/// each to a Storage [`ErasureRecord`] — `{subject, tenant, completed_at_offset}` copied verbatim.
/// This is the wiring the cell-orchestration restore driver runs at boot; here it is the CDC subject.
struct GdprErasureLedgerSeam<'a> {
    ledger: &'a ErasureLedger,
}

impl PostRestoreErasureLedger for GdprErasureLedgerSeam<'_> {
    fn erasures_completed_after(&self, pit: myelin_storage::WalOffset) -> Vec<ErasureRecord> {
        // Read the GDPR ledger's post-PIT projection and map 1:1 to Storage's record (the field
        // shapes mirror — a copy, not a translation). `WalOffset` is the same u64 the GDPR ledger's
        // `completed_at_offset` carries (the §7.3 cursor / completion-timestamp surrogate).
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

// ───────────────────────────── shared fixtures ─────────────────────────────

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

/// A KMS (GDPR holder seam) seeded with one per-subject key per upstream holder.
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

/// A real-shaped data-map inventory naming the upstream holders (the map drives the checklist).
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

/// Run a full DSR erase through the fan-out driver WITH the erasure ledger, returning the ledger
/// entry written on completion (the §4.4 step-5 write). The completion offset is `clock_secs`.
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

// ── the cross-holder re-erasure seams (the §7.5 re-purge / re-tombstone / re-emit), recorded ──

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

/// Stand up a storage KMS with a per-subject column sealed (so a restore of the older backup
/// RESURRECTS the subject's DEK — the §7.5 setup).
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// CDC: PROVIDER (GDPR ledger writes) ⇄ CONSUMER (Storage re-erases from the seam).
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **PROVIDER ⇄ CONSUMER (contract 10.8):** a completed GDPR erase writes a PII-free ledger entry;
/// the control-plane seam presents the ledger's post-PIT records to Storage's `post_restore_reerase`;
/// the re-erasure pass re-erases the resurrected subject to 0 recoverable. The seam binding is the
/// load-bearing contract property: the GDPR ledger drives Storage's restore re-erasure.
#[test]
fn cdc_gdpr_erasure_ledger_drives_storage_post_restore_reerase() {
    let subject_id = "u-post-pit";

    // PROVIDER: a completed GDPR erase records a ledger entry at completion offset 140.
    let ledger = ErasureLedger::new();
    let entry = complete_an_erase_and_record_to_ledger(&ledger, subject_id, 140, 1000);
    assert_eq!(
        entry.subject_token, subject_id,
        "the entry holds the opaque subject token (no PII)"
    );
    assert_eq!(entry.completed_at_offset, 140);

    // The seam (control-plane wiring) presents the ledger's post-PIT records as Storage ErasureRecords.
    let seam = GdprErasureLedgerSeam { ledger: &ledger };
    let post_pit = seam.erasures_completed_after(100); // a restore to PIT T=100 (BEFORE the erasure).
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

    // CONSUMER: Storage re-erases the resurrected subject from the seam → 0 resurrected.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// STOR-D4 (GA face): erase → recover-from-backup = 0 recoverable PII (key destroyed, excluded).
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **STOR-D4-GA-face (the dated green artifact) — erase a subject; attempt recovery from backups →
/// 0 recoverable PII (key destroyed, excluded from backup).** The GDPR erase crypto-shreds the
/// per-subject key; the destroyed key is excluded from the backup snapshot by construction, so the
/// ciphertext is unrecoverable. Measured: 0 recoverable in any backup snapshot over every driven holder.
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

    // Before erase: the per-subject keys are recoverable in the backup snapshot (the gate is not vacuous).
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

    // STOR-D4-GA-face: 0 recoverable PII in any backup snapshot over EVERY driven holder.
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

    // The erasure ledger recorded the completion (the source that drives STOR-D3 re-erasure).
    assert_eq!(
        ledger.len(),
        1,
        "the erasure ledger recorded the completed erase"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// STOR-D3 / ID-D8 (GA face): erase → restore an OLDER backup → still erased (re-erasure ran), 0 resurrected.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **STOR-D3 / ID-D8-GA-face (the dated green artifact) — erase; restore an OLDER backup → still
/// erased; 0 resurrected; re-erasure receipt.** A subject is erased at completion offset 140 (AFTER
/// the backup PIT T=100); restoring T=100 resurrects the subject's DEK; the post-restore re-erasure
/// pass reads the GDPR erasure ledger (via the seam) and re-erases the subject — 0 resurrected, and
/// the re-erasure receipt (the green artifact) records the re-kill.
#[test]
fn stor_d3_id_d8_ga_face_restore_older_backup_re_erases_zero_resurrected() {
    let subject_id = "u-erased-after-backup";

    // The GDPR erasure ledger records the erasure as completed at offset 140 (AFTER T=100).
    let ledger = ErasureLedger::new();
    complete_an_erase_and_record_to_ledger(&ledger, subject_id, 140, 3000);

    // The storage copy holds the subject's pre-erasure DEK alive (resurrected by a restore of T=100).
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

    // Restore the OLDER backup (PIT T=100, BEFORE the erasure).
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

    // Post-restore re-erasure runs FROM the GDPR erasure ledger (the seam drives it).
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

    // STOR-D3/ID-D8-GA-face: the subject is STILL ERASED — 0 resurrected; the re-erasure receipt is green.
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
    // the resurrected DEK is gone from the restored copy.
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the resurrected DEK is re-destroyed — the restore did NOT resurrect the subject"
    );
}

/// **The PII-free, non-shred-erasable property end-to-end: the ledger SURVIVES the subject's own
/// erase and STILL drives re-erasure.** Even after the subject is erased through the recursive
/// ledger holder, the completion record is retained (it holds no PII), so a later restore re-erases
/// the subject FROM it. This is why the ledger must NOT be crypto-shred-erasable (§2.3).
#[test]
fn the_erasure_ledger_survives_its_own_subjects_erase_and_still_drives_re_erasure() {
    let subject_id = "u-recursive";
    let ledger = ErasureLedger::new();
    complete_an_erase_and_record_to_ledger(&ledger, subject_id, 140, 4000);
    assert_eq!(ledger.len(), 1);

    // Erase the subject THROUGH the recursive ledger holder (the per-tenant crypto-shred fan-out
    // would reach the ledger as a registered holder). The ledger RETAINS the record (non-shred-erasable).
    ledger.erase(subject_scope(subject_id)).unwrap();
    assert_eq!(
        ledger.len(),
        1,
        "the ledger erase RETAINED the PII-free record (non-shred-erasable)"
    );

    // The retained record STILL drives re-erasure on a restore to before the erasure.
    let seam = GdprErasureLedgerSeam { ledger: &ledger };
    assert_eq!(
        seam.erasures_completed_after(100).len(),
        1,
        "the retained record STILL drives post-restore re-erasure after the subject's own erase"
    );
}
