use std::collections::{BTreeMap, BTreeSet};

use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_gdpr_service::datamap::{Inventory, InventoryEntry};
use myelin_gdpr_service::holders::{
    AuditCarveOutHolder, CryptoShredKms, GdprOwnStoreHolder, InMemoryShredKms, ShredKeyClass,
    ShredKeyHandle, AUDIT_CARVE_OUT_STORE, GDPR_OWN_STORE,
};
use myelin_gdpr_service::orchestration::{
    holder_ids, CanonicalErasePhase, RegisteredHolder, SeamHolder,
};
use myelin_gdpr_service::{
    DsrKind, DsrOrchestrator, DsrState, EraseChecklist, FanOutDriver, FanOutOutcome, Initiator,
    LegalHoldRegistry, Posture, UpstreamHolderOrchestrator, DSR_DEADLINE_SECS,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::TestClock;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(s),
        tenant: tenant(),
    }
}

fn all_m1_holder_ids() -> Vec<&'static str> {
    vec![
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
        GDPR_OWN_STORE,
        AUDIT_CARVE_OUT_STORE,
    ]
}

fn seed_kms_for(subject_id: &str, base_epoch: u64) -> InMemoryShredKms {
    let kms = InMemoryShredKms::new();
    let t = tenant();
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
                tenant: t.clone(),
                class: ShredKeyClass::Subject((*id).to_string()),
            },
            base_epoch + i as u64,
        );
    }
    kms.provision(
        ShredKeyHandle {
            tenant: t.clone(),
            class: ShredKeyClass::Subject(subject_id.to_string()),
        },
        base_epoch + 100,
    );
    kms
}

fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
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

fn full_m1_orchestrator<'a>(
    upstream_seams: &'a [(&'static str, SeamHolder<'a>)],
    h18: &'a GdprOwnStoreHolder<'a>,
    h16: &'a AuditCarveOutHolder<'a>,
) -> UpstreamHolderOrchestrator<'a> {
    let mut registered: Vec<RegisteredHolder<'a>> = upstream_seams
        .iter()
        .map(|(id, h)| RegisteredHolder {
            id,
            phase: myelin_gdpr_service::orchestration::canonical_phase_of(id).unwrap(),
            holder: h as &dyn PersonalDataHolder,
        })
        .collect();
    registered.push(RegisteredHolder {
        id: GDPR_OWN_STORE,
        phase: CanonicalErasePhase::CryptoShredDek,
        holder: h18,
    });
    registered.push(RegisteredHolder {
        id: AUDIT_CARVE_OUT_STORE,
        phase: CanonicalErasePhase::CachesAndDerivedCopies,
        holder: h16,
    });
    UpstreamHolderOrchestrator::new(registered)
}

fn inventory_for_all_holders() -> Inventory {
    let mut holders = BTreeSet::new();
    for id in all_m1_holder_ids() {
        holders.insert(id.to_string());
    }
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

fn zero_recoverable_locate(holder: &str, subject_id: &str) -> Receipt {
    Receipt::content_addressed(
        "locate",
        holder,
        subject_id,
        &tenant().0,
        "located:0-recoverable",
        None,
        0,
    )
}

fn locate(h: &dyn PersonalDataHolder, s: &SubjectRef) -> LocateReport {
    h.locate(s, tenant()).expect("locate never errors")
}

#[test]
fn ga_d1_m1_floor_subject_erasure_yields_zero_recoverable_over_every_holder() {
    let subject_id = "u-floor";
    let kms = seed_kms_for(subject_id, 1000);
    let upstream_seams = seam_holders(&kms);
    let h18 = GdprOwnStoreHolder::new(&kms);
    let h16 = AuditCarveOutHolder::new(&kms);
    let subj = subject(subject_id);

    let upstream = full_m1_orchestrator(&upstream_seams, &h18, &h16);
    assert_eq!(
        upstream.registered_count(),
        8,
        "all eight M1 holders are registered"
    );
    assert_eq!(
        upstream.holder_ids_in_order()[0],
        holder_ids::IDENTITY,
        "Identity (phase 0 - pseudonym map) is erased FIRST even with the GDPR-owned holders mixed in"
    );

    let before_h18 = locate(&h18, &subj);
    assert_ne!(
        before_h18.receipt.content_hash,
        zero_recoverable_locate(GDPR_OWN_STORE, subject_id).content_hash,
        "H18 finds the subject PRESENT before erase (the floor reading is not vacuous)"
    );

    let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
    let holds = LegalHoldRegistry::new();
    let driver = FanOutDriver::new(&dsr, &holds);
    let id = dsr.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subj.clone(),
        subject_scope(subject_id),
        Posture::Controller,
        Initiator::Myelin,
    );
    assert!(
        dsr.validate(&id).unwrap(),
        "the controller-posture erase is admitted"
    );

    let checklist = EraseChecklist::new();
    let outcome = driver
        .drive(&id, &inventory_for_all_holders(), &upstream, &checklist)
        .expect("the fan-out drives to completion");

    let status_holders: BTreeSet<String> = dsr
        .dsr_status(&id)
        .unwrap()
        .checklist
        .iter()
        .map(|c| c.holder_id.clone())
        .collect();
    for id in all_m1_holder_ids() {
        assert!(
            status_holders.contains(id),
            "the checklist (from the map) names holder `{id}`"
        );
    }

    assert_eq!(
        upstream.fanout_coverage(&checklist),
        1.0,
        "erasure_fanout_coverage = 1.0 (100% of the 8 existing M1 holders)"
    );
    let receipt = match &outcome {
        FanOutOutcome::Erased(r) => r,
        other => panic!("expected an Erased outcome, got {other:?}"),
    };
    assert_eq!(
        receipt.holder_receipts.len(),
        8,
        "all eight holders receipted"
    );
    assert_eq!(
        receipt.holder_receipts[0].holder_id,
        holder_ids::IDENTITY,
        "Identity erased FIRST"
    );

    let mut zero_recoverable = 0usize;
    for (hid, h) in &upstream_seams {
        let after = locate(h, &subj);
        assert_eq!(
            after.receipt.content_hash,
            zero_recoverable_locate(hid, subject_id).content_hash,
            "post-erase: holder `{hid}` reports 0 recoverable PII (the subject's key is shredded)"
        );
        zero_recoverable += 1;
    }
    let after_h18 = locate(&h18, &subj);
    assert_eq!(
        after_h18.receipt.content_hash,
        zero_recoverable_locate(GDPR_OWN_STORE, subject_id).content_hash,
        "post-erase: H18 (GDPR own store) reports 0 recoverable PII (consent DEK shredded)"
    );
    zero_recoverable += 1;
    let h16_receipt = receipt
        .holder_receipts
        .iter()
        .find(|hr| hr.holder_id == AUDIT_CARVE_OUT_STORE)
        .expect("H16 is in the fan-out");
    assert_eq!(
        h16_receipt.receipt.receipt.key_epoch_destroyed, None,
        "H16's per-subject erase is a RETAIN-minimised carve-out (no key shredded now) - §6.4"
    );
    zero_recoverable += 1;

    assert_eq!(
        zero_recoverable, 8,
        "0 recoverable PII over ALL EIGHT M1 holders (6 shredded + H18 shredded + H16 minimised)"
    );

    for hid in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ] {
        let handle = ShredKeyHandle {
            tenant: tenant(),
            class: ShredKeyClass::Subject(hid.to_string()),
        };
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "holder `{hid}`: 0 recoverable in backup"
        );
    }
    let consent_handle = ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Subject(subject_id.to_string()),
    };
    assert_eq!(
        kms.recoverable_in_backup(&consent_handle),
        0,
        "H18 consent DEK: 0 recoverable in backup (crypto-shred reaches the backup - P-GA-15 leg)"
    );

    assert_eq!(
        dsr.state_of(&id).unwrap(),
        DsrState::Completed,
        "the DSR reached Completed"
    );
    let cert = dsr.dsr_certificate(&id).unwrap();
    assert_eq!(
        cert.receipts.len(),
        8,
        "the certificate seals all eight holder receipts"
    );
    assert!(
        cert.bundle_digest.starts_with("blake3:"),
        "the certificate is content-addressed"
    );
    assert!(
        cert.merkle_inclusion.is_none(),
        "the Merkle seal is P-GA-20 (named floor)"
    );
}

#[test]
fn ga_d1_m1_floor_tracks_the_coarse_one_month_deadline() {
    let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
    let id = dsr.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("u-deadline"),
        subject_scope("u-deadline"),
        Posture::Controller,
        Initiator::Myelin,
    );
    let status = dsr.dsr_status(&id).unwrap();
    assert_eq!(
        status.deadline_secs,
        1_700_000_000 + DSR_DEADLINE_SECS,
        "the coarse deadline is submitted_at + 1 month (30 days)"
    );
    assert_eq!(status.deadline_secs - 1_700_000_000, 30 * 24 * 60 * 60);
}

#[test]
fn ga_d1_worker_kill_mid_fan_out_resumes_only_un_receipted_holders_zero_double_erase() {
    let subject_id = "u-resume";
    let kms = seed_kms_for(subject_id, 2000);
    let upstream_seams = seam_holders(&kms);
    let h18 = GdprOwnStoreHolder::new(&kms);
    let h16 = AuditCarveOutHolder::new(&kms);

    let upstream = full_m1_orchestrator(&upstream_seams, &h18, &h16);

    let checklist = EraseChecklist::new();
    let scope = subject_scope(subject_id);

    let first_three: Vec<(&'static str, &dyn PersonalDataHolder)> = upstream_seams
        .iter()
        .filter(|(id, _)| {
            *id == holder_ids::IDENTITY
                || *id == holder_ids::BLOB
                || *id == holder_ids::AUTHZ_TUPLES
        })
        .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
        .collect();
    let crashing = UpstreamHolderOrchestrator::register_m1_upstream(first_three);
    crashing.fan_out_erase(&scope, &checklist).unwrap();
    assert_eq!(
        checklist.done_count(),
        3,
        "the worker died after receipting three holders"
    );
    let calls_at_crash: BTreeMap<&str, u32> = upstream_seams
        .iter()
        .map(|(id, h)| (*id, h.erase_call_count()))
        .collect();

    let dsr2 = DsrOrchestrator::new(TestClock::at(2_000_000_000));
    let holds2 = LegalHoldRegistry::new();
    let driver2 = FanOutDriver::new(&dsr2, &holds2);
    let id2 = dsr2.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject(subject_id),
        scope.clone(),
        Posture::Controller,
        Initiator::Myelin,
    );
    assert!(dsr2.validate(&id2).unwrap());

    let outcome = driver2
        .drive(&id2, &inventory_for_all_holders(), &upstream, &checklist)
        .expect("the restarted worker resumes the fan-out to completion");
    assert!(
        matches!(outcome, FanOutOutcome::Erased(_)),
        "the resumed DSR erases to completion"
    );

    for id in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
    ] {
        let h = &upstream_seams.iter().find(|(hid, _)| *hid == id).unwrap().1;
        assert_eq!(
            h.erase_call_count(),
            calls_at_crash[id],
            "0 double-erase: holder `{id}` was already receipted ⇒ NOT re-called on restart"
        );
    }
    for id in [holder_ids::BUS, holder_ids::CACHE, holder_ids::BACKUP] {
        let h = &upstream_seams.iter().find(|(hid, _)| *hid == id).unwrap().1;
        assert_eq!(
            h.erase_call_count(),
            1,
            "holder `{id}` driven exactly once (on the resume)"
        );
    }
    assert_eq!(
        upstream.fanout_coverage(&checklist),
        1.0,
        "0 missed: erasure_fanout_coverage = 1.0 after the resume (all 8 holders receipted)"
    );
    assert_eq!(
        checklist.done_count(),
        8,
        "all eight holders are receipted exactly once"
    );
    assert_eq!(
        dsr2.state_of(&id2).unwrap(),
        DsrState::Completed,
        "the resumed DSR reached Completed"
    );

    for hid in all_m1_holder_ids() {
        if hid == AUDIT_CARVE_OUT_STORE {
            continue;
        }
        let class = if hid == GDPR_OWN_STORE {
            ShredKeyClass::Subject(subject_id.to_string())
        } else {
            ShredKeyClass::Subject(hid.to_string())
        };
        let handle = ShredKeyHandle {
            tenant: tenant(),
            class,
        };
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "post-resume: holder `{hid}` has 0 recoverable PII (key shredded, backups included)"
        );
    }
}

#[test]
fn ga_d1_a_restart_after_completion_is_an_idempotent_no_op() {
    let subject_id = "u-idem";
    let kms = seed_kms_for(subject_id, 3000);
    let upstream_seams = seam_holders(&kms);
    let h18 = GdprOwnStoreHolder::new(&kms);
    let h16 = AuditCarveOutHolder::new(&kms);
    let upstream = full_m1_orchestrator(&upstream_seams, &h18, &h16);
    let checklist = EraseChecklist::new();
    let inv = inventory_for_all_holders();

    let dsr1 = DsrOrchestrator::new(TestClock::at(10));
    let holds1 = LegalHoldRegistry::new();
    let driver1 = FanOutDriver::new(&dsr1, &holds1);
    let id1 = dsr1.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject(subject_id),
        subject_scope(subject_id),
        Posture::Controller,
        Initiator::Myelin,
    );
    dsr1.validate(&id1).unwrap();
    let first = driver1.drive(&id1, &inv, &upstream, &checklist).unwrap();
    let calls_after_first: Vec<u32> = upstream_seams
        .iter()
        .map(|(_, h)| h.erase_call_count())
        .collect();

    let dsr2 = DsrOrchestrator::new(TestClock::at(10));
    let holds2 = LegalHoldRegistry::new();
    let driver2 = FanOutDriver::new(&dsr2, &holds2);
    let id2 = dsr2.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject(subject_id),
        subject_scope(subject_id),
        Posture::Controller,
        Initiator::Myelin,
    );
    dsr2.validate(&id2).unwrap();
    let second = driver2.drive(&id2, &inv, &upstream, &checklist).unwrap();
    let calls_after_second: Vec<u32> = upstream_seams
        .iter()
        .map(|(_, h)| h.erase_call_count())
        .collect();

    assert_eq!(
        calls_after_first, calls_after_second,
        "0 double-erase: a restart after completion re-calls NO holder"
    );
    assert_eq!(
        first.receipt().content_hash,
        second.receipt().content_hash,
        "an idempotent restart re-affirms the SAME content-addressed completion receipt"
    );
}
