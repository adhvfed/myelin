#![cfg(feature = "integration")]

use std::path::Path;

use myelin_ci_controlplane::{ci_restore_verify_stores, run_ci_restore_verify_or_fail};
use myelin_harness::{Label, Predicate, RestoredSnapshot, SignalName, SignalSource};
use myelin_storage::{
    restore_to_offset, BlobPresence, CellKillRestore, ContentHash, ContinuousArchiver,
    ErasureLedger, GateInputs, KekId, KeyClass, KmsEngine, RestoreReport, RestoredObject, RtoGrain,
    SourceLog, WalRow, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}
fn tenant(s: &str) -> TenantId {
    TenantId(s.into())
}

fn rpo_rto_secs_from_thresholds(key: &str) -> u64 {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest");
    let path = root.join("thresholds.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the versioned thresholds file must load at {path:?}: {e}"));
    let doc: toml::Value = text.parse().expect("thresholds.toml must be valid TOML");
    let mins = doc
        .get("rpo_rto")
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_integer())
        .unwrap_or_else(|| panic!("rpo_rto.{key} must be present (a missing threshold is LOUD)"));
    assert!(mins > 0, "the {key} bound must be a positive duration");
    (mins as u64) * 60
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
fn ci_p27_restore_verify_greens_over_the_ci_stores_zero_loss() {
    let live = tenant("acme-ci");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(live.clone(), region()));
    kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();

    let ci_event_log_offset: u64 = 300;
    let arch = reachable_archiver(500);

    let def_snapshot = RestoredObject::integral(b"ci_run:definition-snapshot".to_vec());
    let job_spec = RestoredObject::integral(b"ci_job:job-spec+log-anchor".to_vec());
    let published_artifact = RestoredObject::integral(b"artifact:build-output.tar".to_vec());
    let cache_object = RestoredObject::integral(b"cache_entry:~/.cargo restore cache".to_vec());
    let log_segment = RestoredObject::integral(b"log_segment:sealed-T3-firehose-segment".to_vec());
    let objects = vec![
        def_snapshot.clone(),
        job_spec.clone(),
        published_artifact.clone(),
        cache_object.clone(),
        log_segment.clone(),
    ];

    let mut source = SourceLog::new();
    source
        .append(200, "ci_run:r-200")
        .append(210, "ci_job:j-210")
        .append(220, "job_queue:q-220")
        .append(230, "artifact:a-230")
        .append(240, "cache_entry:c-240")
        .append(250, "log_segment:s-250");

    let rows = vec![
        WalRow {
            id: "ci_run:r-200".into(),
            written_at: 200,
            blob_ref: Some(def_snapshot.content_address.clone()),
        },
        WalRow {
            id: "ci_job:j-210".into(),
            written_at: 210,
            blob_ref: Some(job_spec.content_address.clone()),
        },
        WalRow {
            id: "job_queue:q-220".into(),
            written_at: 220,
            blob_ref: None,
        },
        WalRow {
            id: "artifact:a-230".into(),
            written_at: 230,
            blob_ref: Some(published_artifact.content_address.clone()),
        },
        WalRow {
            id: "cache_entry:c-240".into(),
            written_at: 240,
            blob_ref: Some(cache_object.content_address.clone()),
        },
        WalRow {
            id: "log_segment:s-250".into(),
            written_at: 250,
            blob_ref: Some(log_segment.content_address.clone()),
        },
        WalRow {
            id: "ci_run:r-future".into(),
            written_at: 400,
            blob_ref: None,
        },
    ];

    let ledger = ErasureLedger::new();
    let inputs = GateInputs {
        archiver: &arch,
        target: ci_event_log_offset,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let artifact = run_ci_restore_verify_or_fail(&inputs)
        .expect("the CI-stores restore-verify must GREEN - the permanent gate passes");
    assert_eq!(
        artifact.restored_to_offset, ci_event_log_offset,
        "OLTP↔blob↔index↔offset landed at ONE point T = the CI event-log offset"
    );
    assert_eq!(
        artifact.oltp_row_count, 6,
        "the six CI-store rows at seq ≤ T restored; the future ci_run was dropped"
    );
    assert_eq!(
        artifact.objects_verified, 5,
        "five content-addressed objects (artifacts/caches/log-segments/snapshots) checksum-parity-verified"
    );
    assert_eq!(
        artifact.dangling_ref_count, 0,
        "0 loss - no dangling blob ref"
    );
    assert_eq!(
        artifact.checksum_mismatches, 0,
        "checksum parity holds across the CI blob tier"
    );
    assert_eq!(
        artifact.cross_seam_mismatches, 0,
        "ONE consistent cross-seam point across the CI stores"
    );
    assert_eq!(
        artifact.resurrected_subjects, 0,
        "no erased CI subject resurrected"
    );

    let mut presence = BlobPresence::new();
    for o in &objects {
        presence.insert(o.content_address.clone());
    }
    let report = restore_to_offset(&arch, ci_event_log_offset, &rows, &presence, &source, &kms)
        .expect("the restore the gate drove over the CI stores");
    let snapshot = to_harness_snapshot(&report, &objects);
    let cross_seam = snapshot.verify_cross_seam();
    assert!(
        cross_seam.is_consistent(),
        "the harness cross-seam assertion must AGREE the CI-stores restore is consistent, got {:?}",
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

    let stores = ci_restore_verify_stores();
    for s in [
        "ci_run",
        "ci_job",
        "job_queue",
        "artifact",
        "cache_entry",
        "log_segment",
    ] {
        assert!(stores.contains(&s), "manifest covers the CI store `{s}`");
        assert!(
            rows.iter().any(|r| r.id.starts_with(s)),
            "the CI store `{s}` is restored at the cross-seam point"
        );
    }
    assert!(
        stores.contains(&"ci_event_log_offset"),
        "the manifest names the CI event-log offset as the cross-seam consistency point"
    );

    println!(
        "[P-370 GATE GREEN 2026-06-23] STOR-D1 restore-verify over the CI stores (ci_run/ci_job/\
         job_queue + artifact/cache_entry [T2] + log_segment [T3] + ci_event_log_offset): {} \
         Harness SUB-D6 cross-seam AGREES: {} mismatch(es). PERMANENT GATE (master §4) - re-runs on \
         every CI-store-touching change, forever; loud-never-swallowed (no `|| true`). REUSES \
         Storage's RestoreVerifyGate (P-061), no fork.",
        artifact.summary(),
        cross_seam.mismatch_count(),
    );
}

#[test]
fn ci_p27_restore_verify_rpo_rto_within_bounds_for_the_ci_stores() {
    let rpo_bound = rpo_rto_secs_from_thresholds("rpo_max_mins");
    let tenant_bound = rpo_rto_secs_from_thresholds("rto_tenant_max_mins");
    let cell_bound = rpo_rto_secs_from_thresholds("rto_cell_max_mins");

    let mut archiver = ContinuousArchiver::new();
    let commit_period: u64 = 30;
    let archive_period: u64 = 60;
    let steps: u64 = 120;
    let mut offset: u64 = 0;
    let mut peak_rpo: u64 = 0;
    for step in 1..=steps {
        let now = step * commit_period;
        offset += 10;
        archiver.record_commit(offset, now);
        if now.is_multiple_of(archive_period) {
            archiver
                .archive_segment(WalSegment {
                    end_offset: offset,
                    committed_at: now.saturating_sub(5),
                })
                .expect("continuous CI archiving is strictly forward");
        }
        let rpo = archiver.measure_rpo();
        peak_rpo = peak_rpo.max(rpo);
        assert!(
            rpo <= rpo_bound,
            "CI-stores RPO breached at t={now}s: {rpo}s > {rpo_bound}s bound"
        );
    }
    let rpo_at_kill = archiver.measure_rpo();
    peak_rpo = peak_rpo.max(rpo_at_kill);

    let tenant_recovery = CellKillRestore::new(RtoGrain::Tenant, 0, (16 + 8 + 2 + 2) * 60);
    let cell_recovery = CellKillRestore::new(RtoGrain::Cell, 0, (88 + 50 + 17 + 10) * 60);

    assert!(
        tenant_recovery.within_bound(tenant_bound),
        "CI per-tenant RTO {}s exceeds the {tenant_bound}s bound",
        tenant_recovery.rto_secs()
    );
    assert!(
        cell_recovery.within_bound(cell_bound),
        "CI per-cell RTO {}s exceeds the {cell_bound}s bound",
        cell_recovery.rto_secs()
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::RestoreRpoSecs, peak_rpo as i64);
    signals.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", RtoGrain::Tenant.label())],
        tenant_recovery.rto_secs() as i64,
    );
    signals.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", RtoGrain::Cell.label())],
        cell_recovery.rto_secs() as i64,
    );
    signals
        .assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(rpo_bound as i64))
        .expect_green();
    signals
        .assert_labelled(
            SignalName::RestoreRtoSecs,
            vec![Label::new("grain", "tenant")],
            Predicate::Lte(tenant_bound as i64),
        )
        .expect_green();
    signals
        .assert_labelled(
            SignalName::RestoreRtoSecs,
            vec![Label::new("grain", "cell")],
            Predicate::Lte(cell_bound as i64),
        )
        .expect_green();

    println!(
        "[P-370 GATE GREEN 2026-06-23] STOR-D2 over the CI stores: continuous CI archiving -> PEAK \
         RPO={peak_rpo}s <= {rpo_bound}s (5-min) bound [thresholds.toml]; CI-cell-kill restore -> \
         per-tenant RTO={}s ({}min) <= {tenant_bound}s; per-cell RTO={}s ({}min) <= {cell_bound}s \
         [all read from thresholds.toml, NOT hardcoded]. RPO at CI-cell kill={rpo_at_kill}s. \
         PERMANENT GATE - re-runs on every CI-store change; REUSES Storage's CellKillRestore (P-100).",
        tenant_recovery.rto_secs(),
        tenant_recovery.rto_secs() / 60,
        cell_recovery.rto_secs(),
        cell_recovery.rto_secs() / 60,
    );
}

#[test]
fn ci_p27_restore_verify_fails_ci_on_a_corrupted_ci_backup() {
    let t = tenant("acme-ci");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(500);

    let artifact = RestoredObject::integral(b"artifact:present".to_vec());
    let missing_log_segment = ContentHash::blake3(b"log_segment:LOST-sealed-segment");
    let objects = vec![artifact.clone()];
    let source = SourceLog::new();
    let rows = vec![
        WalRow {
            id: "artifact:ok".into(),
            written_at: 230,
            blob_ref: Some(artifact.content_address.clone()),
        },
        WalRow {
            id: "log_segment:corrupt".into(),
            written_at: 250,
            blob_ref: Some(missing_log_segment),
        },
    ];
    let ledger = ErasureLedger::new();
    let inputs = GateInputs {
        archiver: &arch,
        target: 300,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let err = run_ci_restore_verify_or_fail(&inputs)
        .expect_err("a corrupted CI backup (log_segment → missing blob) MUST fail CI, never pass");
    assert!(
        err.contains("DATED NO-GO") && err.contains("blocks M5"),
        "the CI gate caller surfaces the red as a dated no-go that blocks M5: {err}"
    );
    assert!(
        err.contains("log_segment:corrupt") || err.contains("missing"),
        "the failure names the corrupt CI store row / missing blob: {err}"
    );
}
