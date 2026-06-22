//! P-ST-13 (global P-061) GATE / DRILL — **THE HEADLINE: the CI-wired restore-verify gate (STOR-D1,
//! the permanent gate)** — dated green artifact.
//!
//! **The GATE (storage.md §7.4 / testing-strategy STOR-D1):** spin a clean target, restore T1/T2/T5,
//! reindex T4/Search/Refs from source to T, assert **no loss** (checksum parity), **cross-seam**
//! (every restored row's blob hash present + integrity-verified; derived == source-replay), and
//! **erasure held** (a subject erased before the backup is still erased after restore). Green artifact
//! on pass; RED gate FAILs CI. Telemetry: `restore_verify_pass`, `dangling_ref_count == 0`,
//! `RestoreCrossSeamMismatch == 0`.
//!
//! **This is one of the two PERMANENT gates (master §4): it re-runs on every store-touching change,
//! forever — loud-never-swallowed (no `|| true`).** Never weaken a threshold to pass (EI-01 §3): a red
//! gate becomes a dated "claimed, not proven" thresholds-file row, never a lowered bar.
//!
//! This drill drives the real [`RestoreVerifyGate`] and, on the green path, ALSO feeds the same
//! restore into the harness cross-seam assertion (`myelin_harness::RestoredSnapshot::verify_cross_seam`,
//! P-056 — the SAME one SUB-D6 / STOR-D1 drive) and asserts the two AGREE (coherence, EI-01 §7 — the
//! gate's storage-native check and the substrate's cross-seam invariant land on ONE consistent point).
//! The measured signals are emitted on the SAME [`SignalSource`] every drill uses (observability is
//! part of the pass, EI-01 §3): `RestoreCrossSeamMismatch == 0`.
//!
//! ## Scope (named, EI-01 §4)
//! M1 single-tenant-scale restore-verify against the modeled WAL/PITR machinery + a modeled clean
//! target (the real `pg_restore` + the provisioned DB / object store are the P-S12/P-S15 / P-ST-30
//! floors). The real CI-runner wiring (a CI invocation calling [`RestoreVerifyGate::run_or_fail_ci`]
//! on every store-touching change) lands with the CI subsystem (M2+); this drill IS that gate, run as
//! a `cargo test` until then. Post-restore RE-ERASURE (STOR-D3, per-subject) + the cell-kill RTO
//! (STOR-D2) are the sibling **P-ST-14 (global P-100)**; the prod-scale restored copy for
//! online-migration-under-load is **P-ST-21 (global P-126, STOR-D8)**. All named in the prompt + crate
//! docs.

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

/// Backups covering offsets `0..=tail` (a base at 0 + the WAL tail archived to `tail`).
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

/// Map a storage [`RestoreReport`] + the restored object set into the harness [`RestoredSnapshot`] so
/// the SAME cross-seam assertion SUB-D6 uses cross-validates the gate's storage-native check
/// (coherence, EI-01 §7 — the drill proves the two agree, not a parallel runtime assertion).
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

/// **THE DRILL (dated green artifact): the restore-verify gate spins a clean target, restores,
/// reindexes from source, and asserts no-loss (checksum parity) + cross-seam (0 dangling, one point) +
/// erasure-held → GREEN with measured numbers.**
///
/// The scenario: a tenant with state at offsets 90/100 (each referencing a content-addressed,
/// checksum-integral object), source events that project those rows, a live KEK + a tenant erased
/// BEFORE the backup (which must stay erased), and a future row at offset 250 the restore must drop.
/// Run the gate to T=100; assert (a) the gate GREENs with the measured artifact, and (b) the harness
/// cross-seam assertion (the SUB-D6 one) AGREES — 0 mismatches on the mapped snapshot.
#[test]
fn stor_d1_restore_verify_gate_greens_a_whole_restore() {
    let live = tenant("acme");
    let erased = tenant("offboarded");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(live.clone(), region()));
    kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();
    kms.ensure_kek(&KekId::new(erased.clone(), region()));
    kms.ensure_dek(&erased, &region(), KeyClass::Tenant)
        .unwrap();
    // The erased tenant was crypto-shredded BEFORE the backup — it must stay dead across the restore.
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
        }, // > T → dropped
    ];
    let mut ledger = ErasureLedger::new();
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

    // (a) the gate GREENs with the measured artifact.
    let gate = RestoreVerifyGate::new();
    let artifact = gate
        .run_or_fail_ci(&inputs)
        .expect("a whole restore must GREEN — the permanent gate passes");
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

    // (b) the harness cross-seam assertion (the SUB-D6 one) AGREES with the gate's native check.
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

    // The green artifact: emit the cross-seam telemetry observably (the SAME signal every drill uses).
    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        cross_seam.mismatch_count(),
    );
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-061 GATE GREEN 2026-06-19] {} restore-verify is the PERMANENT gate (STOR-D1, master §4) — \
         re-runs on every store-touching change, forever; loud-never-swallowed. Harness SUB-D6 \
         cross-seam assertion AGREES: {} mismatch(es). Post-restore re-erasure (STOR-D3) + cell-kill \
         RTO (STOR-D2) -> P-ST-14 (P-100); prod-scale restored copy for STOR-D8 -> P-ST-21 (P-126).",
        artifact.summary(),
        cross_seam.mismatch_count(),
    );
}

/// **The gate CATCHES a deliberately-CORRUPTED backup (a row → MISSING blob) → FAILs CI** (the §7.3
/// silent-corruption / no-loss floor). The drill proves the gate WOULD fail on a regression (EI-01 §3:
/// a drill that cannot go red is not a gate). NO silent pass, NO `|| true`.
#[test]
fn stor_d1_gate_fails_ci_on_a_corrupted_backup() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    // "present" is restored; "missing" is NOT — a row references a blob the restore did not bring back.
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

/// **The gate CATCHES silent corruption a presence check would MISS: a present-but-CORRUPT object
/// (bytes that no longer re-hash to the address) → FAILs CI** (the checksum-parity half of §7.4). This
/// is the leg the bare restore (presence-only) does NOT cover — the gate adds it.
#[test]
fn stor_d1_gate_fails_ci_on_a_checksum_mismatch() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
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

/// **The gate CATCHES a resurrected erased subject → FAILs CI** (the erasure-held leg / §7.5 — the
/// gravest failure: un-erasing a person). A tenant the ledger marks erased-before-the-backup whose key
/// the restore brought back is rejected.
#[test]
fn stor_d1_gate_fails_ci_on_a_resurrected_erased_subject() {
    let resurrected = tenant("should-be-dead");
    let kms = KmsEngine::new();
    // The key IS in the KMS (the restore WILL bring it back) — but the ledger says it was erased.
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

/// **The gate is wired LOUD-NEVER-SWALLOWED (EI-01 §5): a swallowing wrapper is structurally
/// rejected.** The `#[must_use]` on `GateVerdict` makes a dropped RED a compile warning; here we prove
/// at runtime that the ONLY non-panicking way to consume a red verdict still surfaces the failure —
/// `run_or_fail_ci` returns `Err` (never `Ok`), and a hand-written "swallow" (mapping the red to a
/// `bool` and discarding it) would LOSE the failure, which this drill demonstrates is WRONG by showing
/// the blessed path keeps it. A `|| true`-style swallow is exactly what `run_or_fail_ci`'s `?` forbids.
#[test]
fn stor_d1_gate_is_loud_never_swallowed() {
    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
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

    // The blessed CI path: `?` propagates the failure (process exits non-zero). A swallow that turns
    // this into a silent pass is the EI-01 §5 violation the gate forbids — the verdict is `#[must_use]`,
    // and the only Ok-yielding consumer (`run_or_fail_ci`) returns Err on a red.
    let result = RestoreVerifyGate::new().run_or_fail_ci(&inputs);
    assert!(
        result.is_err(),
        "a red restore MUST surface as Err — never swallowed into Ok/true"
    );

    // Demonstrate the swallow would be a BUG: if a caller wrote `let _ = gate.run(&inputs);` the
    // #[must_use] warns; if they coerced to a bool and ignored it, the red is lost. We assert the
    // verdict carries the failure so a correct caller cannot miss it.
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
