//! P-ST-35 (global P-446) GATE / DRILL — **E2E-4 (the DSAR fan-out, STORAGE SPINE): the full
//! DSAR / crypto-shred fan-out reaches every H1–H18 holder incl. vectors incl. backups; 0 holders
//! missed; 0 recoverable PII; residual == the one documented posture; certificate sealed.** A dated
//! green artifact.
//!
//! **The GATE (testing-strategy E2E-4 §2.4/§4.4 + storage.md §5.2/§7):** a single DSAR fan-out reaches
//! **every** holder across all subsystems (the data-map-driven fan-out), crypto-shreds reliably
//! (per-subject DEK + per-tenant blob DEK + pseudonym shred), the plaintext-derived Search index incl.
//! **vector embeddings** is PURGED (not hidden), the crypto-shred reaches **backups by construction**,
//! a restore of an older backup re-erases (STOR-D3), the residual is EXACTLY the one documented posture
//! (10.9), and a Merkle-proven certificate is sealed. Gate: **0 holders missed; 0 recoverable PII
//! (incl. vectors, incl. backups); residual == the one documented posture; certificate sealed.**
//! **STOR-D1/STOR-D2 remain green** (re-run by the restore-verify CI job; this drill exercises the
//! STOR-D3/STOR-D4 holder-coverage legs). **Never weaken a threshold to pass.**
//!
//! **The load-bearing zero (EI-01 §2):** a missed holder in an erasure fan-out un-erases a person. The
//! completeness defence is STRUCTURAL: the fan-out iterates the CLOSED [`HolderClass::ALL`] H1–H18
//! catalogue, and a holder NOT reached is recorded as MISSED (never silently dropped), so
//! `holders_missed == 0` is a real proof of completeness.
//!
//! **This drill proves the gate can go RED** (a withheld holder makes `holders_missed > 0` AND leaves a
//! recoverable key) **AND green** (the full fan-out misses 0 holders, 0 recoverable incl. vectors incl.
//! backups, residual documented, certificate sealed), emits the E2E-4 result on the SAME
//! [`SignalSource`] every drill uses (the `CrossTenantCount`-class miss counter), and confirms the
//! fan-out is IDEMPOTENT (a second pass is a no-op across holders) + the STOR-D3 re-erasure leg holds.
//!
//! **Relationship to the GDPR-service orchestrator (no duplication — EI-01 §7):** `myelin-gdpr-service`
//! owns the DSR *orchestration* across abstract holder ids (the `data_map()` fan-out + the
//! `MerkleProvenBundle` seal, GA-D1). THIS is the STORAGE spine — the real crypto-shred that runs IN the
//! data layer and proves the storage holders are reached with 0 recoverable. The CDC pair
//! `cdc_e2e4_holder_coverage.rs` pins that the catalogue covers the orchestrator's storage-owned ids.
//!
//! **FLOORS (named, VISION §3 / prompt DoD):** the HYOK per-content-class policy + the KMIP adapter
//! remain `[OPEN → P6/LEGAL]` in the honesty register (the structural reach ships regardless); the ONE
//! free-text/immutable residual posture (10.9) is `[OPEN — LEGAL]` (reported by reference, not
//! restated); the E2E-3 reindex-parity half is the sibling P-ST-36 (P-447).

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

// ── the always-ok cross-holder seams (the six-step crypto-shred drives these) ──
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

/// The cross-holder seams WITH the git crypto-shred reach wired — so the per-tenant blob DEK (the
/// object/git holders' key) is destroyed in the SAME crypto-shred step (a subject who authored
/// blobs/git content).
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

/// Stand up a KMS engine with the tenant KEK + a sealed per-subject column (the free-text holders'
/// DEK) + a per-tenant blob column (the object/git holders' DEK), so the fan-out has real keys to
/// destroy and a real backup snapshot to probe across the WHOLE holder set.
fn engine_seeded_across_holders(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    let cryptor = ColumnCryptor::new(&kms, region());
    cryptor
        .encrypt(
            tenant,
            Some(subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"alice PII seeded into every holder (oltp/chat/ci/agent/knowledge free-text)",
        )
        .expect("seal the per-subject free-text column");
    // The per-tenant blob DEK (the object store + git pack tier holders' key) — created so the blob
    // holders' recoverable reading is a REAL KMS read (not vacuously 0 for want of a key).
    kms.ensure_dek(tenant, &region(), KeyClass::Blob)
        .expect("create the per-tenant blob DEK");
    kms
}

/// **E2E-4 GREEN: the full DSAR fan-out reaches every H1–H18 holder, 0 missed, 0 recoverable (incl.
/// vectors, incl. backups), residual documented, certificate sealed.**
#[test]
fn e2e4_full_holder_fanout_is_green() {
    let tenant = TenantId::from_token("01J0ACME");
    let subject = SubjectId::new("u-e2e4-green");
    let kms = engine_seeded_across_holders(&tenant, &subject);
    let fanout = FullHolderFanOut::new(&kms, region());
    let seams = Seams::default();
    // The subject authored blobs/git content → wire the git reach so the per-tenant blob DEK is
    // destroyed in the SAME crypto-shred step (the object/git holders read 0 recoverable).
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

    // The E2E-4 gate readings — NONE weakened to pass.
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

    // The certificate is sealed (the storage face of the MerkleProvenBundle).
    let cert = set.seal_certificate();
    assert!(
        cert.sealed,
        "the E2E-4 certificate is sealed on a green fan-out"
    );
    assert!(cert.is_green(), "the certificate is green (0/0, sealed)");
    // It is deterministic + tamper-evident (re-sealing yields the same digest).
    assert_eq!(cert.digest, set.seal_certificate().digest);

    // Every reached holder is green; the audit carve-out (H16) is the documented residual, not a miss.
    for cov in &set.coverages {
        assert!(
            cov.is_green(),
            "{} ({}) green",
            cov.holder.h_number(),
            cov.holder.holder_id()
        );
    }

    // ── Emit the E2E-4 gate result on the SAME SignalSource every drill uses (holders-missed is the
    //    completeness projection — the CrossTenantCount-class load-bearing zero). ──
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, holders_missed as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    // The recoverable-PII zero on the same signal class.
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

/// **The E2E-4 gate is NOT vacuous: a withheld holder makes `holders_missed > 0` (RED) AND leaves a
/// recoverable key.** A holder the fan-out does not reach is recorded as MISSED, never silently
/// dropped (a gate that cannot go red is not a gate, EI-01 §3).
#[test]
fn e2e4_gate_is_not_vacuous_a_withheld_holder_reads_red() {
    let tenant = TenantId::from_token("01J0ACME");
    let subject = SubjectId::new("u-e2e4-red");
    let kms = engine_seeded_across_holders(&tenant, &subject);
    let fanout = FullHolderFanOut::new(&kms, region());
    let seams = Seams::default();

    // Withhold the CI-logs holder (H4) — the drill seam proving the gate can go red.
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

    // The certificate seals RED.
    let cert = set.seal_certificate();
    assert!(!cert.sealed, "a red fan-out seals a non-sealed certificate");
    assert!(!cert.is_green());

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, holders_missed as i64);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a missed holder in a DSAR fan-out MUST read RED — the E2E-4 zero is a real tripwire"
    );
}

/// **The full-holder fan-out is IDEMPOTENT: a second pass is a no-op SUCCESS across holders** — the
/// underlying crypto-shred is itself idempotent (a re-erase is a no-op success), so the second fan-out
/// re-affirms 0 missed / 0 recoverable / complete with `re_run == true`.
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

/// **STOR-D3 (across the full holder set): a restore of an OLDER backup re-erases every holder → 0
/// resurrected.** The E2E-4 mid-flight leg: restore a pre-erasure backup, run the post-restore
/// re-erasure pass across the full holder set, assert the subject is STILL erased (0 resurrected).
#[test]
fn e2e4_post_restore_reerase_across_full_holder_set_is_green() {
    use myelin_storage::{
        restore_to_offset, BlobPresence, ContinuousArchiver, SourceLog, WalSegment,
    };
    use myelin_storage::{DekId, KeyClass};
    use myelin_storage::{ErasureRecord, InMemoryPostPitLedger};

    let tenant = TenantId::from_token("01J0ACME");
    let subject = SubjectId::new("u-erased-after-backup");
    // The restored copy resurrected the subject DEK (the erasure happened AFTER the backup PIT T=100).
    let kms = engine_seeded_across_holders(&tenant, &subject);
    let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
    assert!(
        kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the restore resurrected the subject DEK (it was live at the backup PIT)"
    );

    // The restore lands at T=100.
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

    // The ledger records the erasure as completed at offset 140 (AFTER T=100).
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
        !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
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
