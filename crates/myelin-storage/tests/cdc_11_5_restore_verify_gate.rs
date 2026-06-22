//! Contract 11.5 CDC pair — the **CI-wired restore-verify GATE** caller (P-ST-13 / global P-061).
//!
//! The prompt requires "CDC: provider+consumer pair for 11.5 (the CI durability-gate caller)". This is
//! the consumer-driven contract test for the GATE half of row 11.5 (the BACKUP half is
//! `cdc_11_5_backup`, the RESTORE half is `cdc_11_5_restore`, this is the CI-GATE half):
//!
//! - the **PROVIDER** is `myelin-storage` — the [`RestoreVerifyGate`] this prompt ships: it spins a
//!   clean target, drives `restore(to_offset T)`, and runs the storage.md §7.4 assertions (no-loss /
//!   checksum-parity, cross-seam / one consistent point, erasure-held), emitting a dated
//!   [`GreenArtifact`] on pass + a typed [`GateFailure`] on red, with the loud-never-swallowed
//!   [`RestoreVerifyGate::run_or_fail_ci`] CI entrypoint;
//! - the **CONSUMER** is the **CI durability-gate caller** — the platform's CI graph (the real wiring
//!   lands with the CI subsystem, M2+) modelled here as a tiny `CiDurabilityGate` that invokes
//!   `run_or_fail_ci` on every store-touching change and FAILs the build on a red verdict. This is
//!   exactly the call shape the real CI gate relies on — if the [`GateInputs`] shape / the
//!   `run_or_fail_ci` signature / the `Ok(GreenArtifact)` vs `Err(GateFailure)` contract drift, this
//!   stops compiling/passing.
//!
//! It pins the load-bearing contract properties the consumer depends on: a GREEN run yields a dated
//! artifact with the measured numbers (the CI build records it), and a RED run (a corrupted backup, a
//! checksum mismatch, or a resurrected erased subject) returns `Err` so the CI build FAILS — never a
//! silent pass (the permanent gate, loud-never-swallowed).

use myelin_storage::{
    ContentHash, ContinuousArchiver, ErasureLedger, GateFailure, GateInputs, GreenArtifact, KekId,
    KeyClass, KmsEngine, RestoreVerifyGate, RestoredObject, SourceLog, WalRow, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("eu-west".into())
}
fn tenant(s: &str) -> TenantId {
    TenantId(s.into())
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

/// THE CONSUMER: the CI durability-gate caller. It runs the restore-verify gate on a store-touching
/// change and FAILs the build on a red verdict (the permanent gate, loud-never-swallowed). The real CI
/// graph (M2+) wires THIS shape; modelled here so the gate's caller contract is pinned now.
struct CiDurabilityGate;

impl CiDurabilityGate {
    /// Run the permanent restore-verify gate; `Ok(artifact)` lets the build proceed (recording the
    /// dated green artifact), `Err(failure)` FAILs CI. Exactly `run_or_fail_ci` — no `|| true`.
    fn run_on_change(&self, inputs: &GateInputs<'_>) -> Result<GreenArtifact, GateFailure> {
        RestoreVerifyGate::new().run_or_fail_ci(inputs)
    }
}

/// PROVIDER ⇄ CONSUMER: on a whole restore, the CI gate caller gets `Ok(GreenArtifact)` with the
/// measured numbers — the build proceeds + records the dated proof.
#[test]
fn ci_gate_caller_gets_a_green_artifact_on_a_whole_restore() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let objects = vec![RestoredObject::integral(b"obj".to_vec())];
    let mut source = SourceLog::new();
    source.append(90, "r1");
    let rows = vec![WalRow {
        id: "r1".into(),
        written_at: 90,
        blob_ref: Some(objects[0].content_address.clone()),
    }];
    let ledger = ErasureLedger::new();
    let inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let artifact = CiDurabilityGate
        .run_on_change(&inputs)
        .expect("a whole restore must let the CI build proceed");
    assert_eq!(artifact.restored_to_offset, 100);
    assert_eq!(artifact.oltp_row_count, 1);
    assert_eq!(artifact.checksum_mismatches, 0);
    assert_eq!(artifact.cross_seam_mismatches, 0);
    assert_eq!(artifact.resurrected_subjects, 0);
}

/// PROVIDER ⇄ CONSUMER: a corrupted backup (a row → missing blob) makes the CI gate caller FAIL the
/// build (`Err`) — the silent-data-loss floor, never a silent pass.
#[test]
fn ci_gate_caller_fails_ci_on_a_corrupted_backup() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let missing = ContentHash::blake3(b"missing");
    let objects: Vec<RestoredObject> = vec![];
    let source = SourceLog::new();
    let rows = vec![WalRow {
        id: "r1".into(),
        written_at: 50,
        blob_ref: Some(missing),
    }];
    let ledger = ErasureLedger::new();
    let inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let err = CiDurabilityGate
        .run_on_change(&inputs)
        .expect_err("a corrupted backup MUST fail the CI build");
    assert!(matches!(err, GateFailure::RestoreFailed(_)), "{err}");
}

/// PROVIDER ⇄ CONSUMER: a resurrected erased subject makes the CI gate caller FAIL the build — the
/// erasure-held contract the consumer depends on (a shred stays dead across a restore, §7.5).
#[test]
fn ci_gate_caller_fails_ci_on_a_resurrected_erased_subject() {
    let resurrected = tenant("erased");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(resurrected.clone(), region()));
    kms.ensure_dek(&resurrected, &region(), KeyClass::Tenant)
        .unwrap();
    let arch = reachable_archiver(300);
    let objects: Vec<RestoredObject> = vec![];
    let source = SourceLog::new();
    let rows: Vec<WalRow> = vec![];
    let mut ledger = ErasureLedger::new();
    ledger.record_erased(resurrected.clone());
    let inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let err = CiDurabilityGate
        .run_on_change(&inputs)
        .expect_err("a resurrected erased subject MUST fail the CI build");
    assert_eq!(
        err,
        GateFailure::ErasureResurrected {
            tenant: resurrected
        }
    );
}
