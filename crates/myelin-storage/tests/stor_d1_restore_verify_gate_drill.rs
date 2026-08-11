use myelin_harness::{Predicate, RestoredSnapshot, SignalName, SignalSource};
use myelin_storage::{
    ContentHash, ContinuousArchiver, ErasureLedger, GateFailure, GateInputs, KekId, KeyClass,
    KmsEngine, RestoreError, RestoreReport, RestoreVerifyGate, RestoredObject, SourceLog, WalRow,
    WalSegment,
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

fn to_harness_snapshot(report: &RestoreReport, objects: &[RestoredObject]) -> RestoredSnapshot {
    let mut b = RestoredSnapshot::builder(report.restored_to_offset);
    for obj in objects {
        b = b.blob(obj.content_address.to_multihash_string());
    }
    for row in &report.oltp_rows {
        b = b.row(
            row.id.clone(),
            row.written_at,
            row.blob_ref.as_ref().map(|h| h.to_multihash_string()),
        );
    }
    for doc in report.derived.docs() {
        b = b.index_doc(doc.clone());
    }
    b.build()
}

#[test]
fn stor_d1_restore_verify_gate_greens_a_whole_restore() {
    let live = tenant("acme");
    let erased = tenant("offboarded");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(live.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();
    kms.ensure_kek(&KekId::new(erased.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&erased, &region(), KeyClass::Tenant)
        .unwrap();
    assert!(kms.destroy_kek(&KekId::new(erased.clone(), region())));

    let arch = reachable_archiver(300);
    let objects = vec![
        RestoredObject::integral(b"blob-90".to_vec()),
        RestoredObject::integral(b"blob-100".to_vec()),
    ];
    let mut source = SourceLog::new();
    source.append(90, "r90").append(100, "r100");
    let rows = vec![
        WalRow {
            id: "r90".into(),
            written_at: 90,
            blob_ref: Some(objects[0].content_address.clone()),
        },
        WalRow {
            id: "r100".into(),
            written_at: 100,
            blob_ref: Some(objects[1].content_address.clone()),
        },
        WalRow {
            id: "r-future".into(),
            written_at: 250,
            blob_ref: None,
        },
    ];
    let ledger = ErasureLedger::new();
    ledger.record_erased(erased.clone());

    let target = 100;
    let inputs = GateInputs {
        archiver: &arch,
        target,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let gate = RestoreVerifyGate::new();
    let artifact = gate
        .run_or_fail_ci(&inputs)
        .expect("a whole restore must GREEN - the permanent gate passes");
    assert_eq!(
        artifact.restored_to_offset, target,
        "restore_verify landed at T"
    );
    assert_eq!(artifact.oltp_row_count, 2, "the future row was dropped");
    assert_eq!(
        artifact.objects_verified, 2,
        "both referenced objects checksum-parity-verified"
    );
    assert_eq!(artifact.dangling_ref_count, 0, "dangling_ref_count == 0");
    assert_eq!(artifact.checksum_mismatches, 0, "checksum parity holds");
    assert_eq!(
        artifact.cross_seam_mismatches, 0,
        "one consistent cross-seam point"
    );
    assert_eq!(
        artifact.resurrected_subjects, 0,
        "the erased tenant stayed erased"
    );

    let report = myelin_storage::restore_to_offset(
        &arch,
        target,
        &rows,
        &{
            let mut p = myelin_storage::BlobPresence::new();
            for o in &objects {
                p.insert(o.content_address.clone());
            }
            p
        },
        &source,
        &kms,
    )
    .expect("the restore the gate drove");
    let snapshot = to_harness_snapshot(&report, &objects);
    let cross_seam = snapshot.verify_cross_seam();
    assert!(
        cross_seam.is_consistent(),
        "the harness cross-seam assertion must AGREE the restore is consistent, got {:?}",
        cross_seam.mismatches
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        cross_seam.mismatch_count(),
    );
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-061 GATE GREEN 2026-06-19] {} restore-verify is the PERMANENT gate (STOR-D1, master §4) - \
         re-runs on every store-touching change, forever; loud-never-swallowed. Harness SUB-D6 \
         cross-seam assertion AGREES: {} mismatch(es). Post-restore re-erasure (STOR-D3) + cell-kill \
         RTO (STOR-D2) -> P-ST-14 (P-100); prod-scale restored copy for STOR-D8 -> P-ST-21 (P-126).",
        artifact.summary(),
        cross_seam.mismatch_count(),
    );
}

#[test]
fn stor_d1_gate_fails_ci_on_a_corrupted_backup() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let present = RestoredObject::integral(b"present".to_vec());
    let missing_addr = ContentHash::blake3(b"missing");
    let objects = vec![present.clone()];
    let source = SourceLog::new();
    let rows = vec![
        WalRow {
            id: "ok".into(),
            written_at: 50,
            blob_ref: Some(present.content_address.clone()),
        },
        WalRow {
            id: "corrupt".into(),
            written_at: 90,
            blob_ref: Some(missing_addr),
        },
    ];
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

    let err = RestoreVerifyGate::new()
        .run_or_fail_ci(&inputs)
        .expect_err("a corrupted backup MUST fail CI, never silently pass");
    assert!(
        matches!(&err, GateFailure::RestoreFailed(RestoreError::DanglingBlobRef { row_id, .. }) if row_id == "corrupt"),
        "the gate surfaces the restore's hard dangling-ref FAIL: {err}"
    );
}

#[test]
fn stor_d1_gate_fails_ci_on_a_checksum_mismatch() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let address = ContentHash::blake3(b"good-bytes");
    let corrupt = RestoredObject {
        content_address: address.clone(),
        bytes: b"TAMPERED".to_vec(),
    };
    let objects = vec![corrupt];
    let source = SourceLog::new();
    let rows = vec![WalRow {
        id: "r1".into(),
        written_at: 50,
        blob_ref: Some(address.clone()),
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

    let err = RestoreVerifyGate::new()
        .run_or_fail_ci(&inputs)
        .expect_err("a present-but-corrupt object MUST fail CI (checksum parity)");
    assert!(matches!(err, GateFailure::ChecksumMismatch { .. }), "{err}");
    assert!(
        err.to_string().contains("CHECKSUM MISMATCH"),
        "loud + specific: {err}"
    );
}

#[test]
fn stor_d1_gate_fails_ci_on_a_resurrected_erased_subject() {
    let resurrected = tenant("should-be-dead");
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

    let err = RestoreVerifyGate::new()
        .run_or_fail_ci(&inputs)
        .expect_err("a resurrected erased subject MUST fail CI");
    assert_eq!(
        err,
        GateFailure::ErasureResurrected {
            tenant: resurrected
        }
    );
}

#[test]
fn stor_d1_gate_is_loud_never_swallowed() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let address = ContentHash::blake3(b"good");
    let corrupt = RestoredObject {
        content_address: address.clone(),
        bytes: b"BAD".to_vec(),
    };
    let objects = vec![corrupt];
    let source = SourceLog::new();
    let rows = vec![WalRow {
        id: "r1".into(),
        written_at: 50,
        blob_ref: Some(address),
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

    let result = RestoreVerifyGate::new().run_or_fail_ci(&inputs);
    assert!(
        result.is_err(),
        "a red restore MUST surface as Err - never swallowed into Ok/true"
    );

    let verdict = RestoreVerifyGate::new().run(&inputs);
    assert!(
        !verdict.is_green(),
        "the red verdict is observable, never a hidden pass"
    );
    assert!(
        verdict.failure().is_some(),
        "a red verdict names the failure (loud)"
    );
}
