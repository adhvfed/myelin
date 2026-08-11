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

struct CiDurabilityGate;

impl CiDurabilityGate {
    fn run_on_change(&self, inputs: &GateInputs<'_>) -> Result<GreenArtifact, GateFailure> {
        RestoreVerifyGate::new().run_or_fail_ci(inputs)
    }
}

#[test]
fn ci_gate_caller_gets_a_green_artifact_on_a_whole_restore() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
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

#[test]
fn ci_gate_caller_fails_ci_on_a_corrupted_backup() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
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

#[test]
fn ci_gate_caller_fails_ci_on_a_resurrected_erased_subject() {
    let resurrected = tenant("erased");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(resurrected.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&resurrected, &region(), KeyClass::Tenant)
        .unwrap();
    let arch = reachable_archiver(300);
    let objects: Vec<RestoredObject> = vec![];
    let source = SourceLog::new();
    let rows: Vec<WalRow> = vec![];
    let ledger = ErasureLedger::new();
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
