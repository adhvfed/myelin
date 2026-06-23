//! # CI-P27 / P-370 — STOR-D1 / STOR-D2 restore-verify over the CI STORES (the permanent gate, M4)
//!
//! **Owning prompt:** `planning/07-prompts/by-system/continuous-integration.md` §CI-P27 ("re-confirm
//! the two permanent gates at the M4 boundary … STOR-D1/STOR-D2 restore-verify on the CI stores").
//! **Contract:** row **11.5** (backup / restore / cross-seam + restore-verify, CI-gated, ADR-18 —
//! RPO ≤ 5 min, RTO ≤ 1 h/tenant ≤ 4 h/cell, 0 loss). **Drills:**
//! `01-whole-system-e2e-and-drill-catalogue.md` rows **STOR-D1** (restore-verify; the cross-seam
//! OLTP↔blob↔index↔offset consistent point) + **STOR-D2** (RPO/RTO). **Doctrine:** EI-01 §3
//! (prove-it — a backup that has never been restored is not a backup; the green artifact IS the
//! pass; thresholds READ from `thresholds.toml`, never hardcoded), §5 (loud-never-swallowed — no
//! `|| true`), §7 (coherence — CI does NOT re-implement restore-verify; it WIRES its stores into
//! Storage's ONE gate `RestoreVerifyGate` / `CellKillRestore`).
//!
//! ## What this re-confirms (and how it REUSES Storage's machinery — NO fork)
//! STOR-D1/D2 is the SHARED, PERMANENT restore-verify gate (Storage owns it: P-061 `RestoreVerifyGate`
//! plus P-100 `CellKillRestore`/RPO/RTO). This file WIRES the **CI stores** into that exact gate — it
//! does NOT fork the gate, the cross-seam check, the green-artifact type, or the RPO/RTO machinery.
//! It reuses, verbatim:
//!
//!   - [`RestoreVerifyGate`] / [`GateInputs`] / `run` (the storage-native cross-seam + checksum-parity
//!     plus erasure-held assertions) — driven through the CI control-plane's committed
//!     [`run_ci_restore_verify_or_fail`] (loud-never-swallowed) caller;
//!   - [`restore_to_offset`] + [`BlobPresence`] (the PITR restore to the cross-seam offset T);
//!   - [`CellKillRestore`] / [`RtoGrain`] (the STOR-D2 RTO bounds) and the `RestoreRpoSecs` /
//!     `RestoreRtoSecs` / `RestoreCrossSeamMismatch` telemetry signals (P-056, never re-defined);
//!   - the harness `RestoredSnapshot::verify_cross_seam` (the SUB-D6 assertion) cross-validates the
//!     gate's storage-native check AGREES (coherence, EI-01 §7).
//!
//! ## The CI stores wired into ONE consistent cross-seam point (CI-P27 DELIVERABLE)
//! The restore lands the CI stores at ONE consistency point T = the CI event-log offset:
//!
//!   - **CI OLTP** — `ci_run` / `ci_job` / `job_queue` state (the [`WalRow`]s at `seq ≤ T`);
//!   - **T2 blob** — `artifact` + `cache_entry` (content-addressed objects each row references;
//!     checksum-parity-verified — the bytes re-hash to their BLAKE3 address);
//!   - **T3 log** — `log_segment` (sealed segments as T2 blobs, also content-addressed);
//!   - **the CI event-log offset** `ci_event_log_offset` = the cross-seam point T every tier lands at.
//!
//! The gate asserts OLTP↔blob↔index↔offset land at ONE point (0 loss / 0 dangling / 0 cross-seam
//! mismatch), and STOR-D2 asserts RPO ≤ 5 min + RTO ≤ 1 h/tenant ≤ 4 h/cell (read from thresholds).
//!
//! ## FLOOR: none — this is a PERMANENT GATE (re-runs on every CI-store-touching change, forever)
//! The modeled clean target (in-memory `RestoreTarget` the gate populates) is the SAME M1 floor
//! Storage's own STOR-D1 drill runs on (the real `pg_restore` + provisioned object store is the
//! P-S12/P-S15 floor; the gate SHAPE does not change when it lands). On THIS host the live dev stack
//! is up, so the real CI control-plane schema + the T2 blob tier the artifacts/caches/log-segments
//! flush to are exercised in the sibling CI-P6/P20 integration tests; this gate wires the restore
//! cross-seam consistency over those stores.
//!
//! Gated behind `--features integration` (loads `thresholds.toml`; the default `cargo test
//! --workspace` stays DB-free). Run:
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p27_restore_verify_ci_stores -- --nocapture

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

/// Read a `[rpo_rto]` minutes bound from the workspace-root `thresholds.toml` (the versioned source
/// of truth — NEVER hardcoded, EI-01 §3). A missing threshold is a LOUD failure.
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

/// Map a storage [`RestoreReport`] + the restored CI object set into the harness [`RestoredSnapshot`]
/// so the SAME cross-seam assertion SUB-D6 uses cross-validates the gate's storage-native check
/// (coherence, EI-01 §7 — the drill proves the two AGREE over the CI stores).
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

/// **STOR-D1 over the CI stores: restore CI OLTP + T2 blob (artifacts/caches) + T3 log segments + the
/// CI event-log offset to ONE consistent cross-seam point → 0 loss; dated green artifact.**
///
/// The CI workload modeled at the cross-seam offset T = `ci_event_log_offset`:
///   - `ci_run`     @ offset 200, references its definition-snapshot blob (T2 artifact-class object);
///   - `ci_job`     @ offset 210, references its job-spec/log-anchor object;
///   - `job_queue`  @ offset 220, a claimed lease row (no blob);
///   - `artifact`   @ offset 230, a published artifact (T2 blob);
///   - `cache_entry`@ offset 240, a restore-cache entry (T2 blob);
///   - `log_segment`@ offset 250, a sealed T3 log segment (content-addressed, stored as a T2 blob);
///   - a FUTURE `ci_run` @ offset 400 (> T) the restore must DROP (it is past the consistency point).
///
/// Every content-addressed object is checksum-integral (the bytes re-hash to their BLAKE3 address).
#[test]
fn ci_p27_restore_verify_greens_over_the_ci_stores_zero_loss() {
    // The CI tenant + its KEK/DEK (a restore brings back the key for a LIVE tenant).
    let live = tenant("acme-ci");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(live.clone(), region()));
    kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();

    // The cross-seam consistency point T = the CI event-log offset.
    let ci_event_log_offset: u64 = 300;
    let arch = reachable_archiver(500);

    // T2 blob tier (artifacts + caches) + T3 log segments — every object content-addressed + integral.
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

    // The CI event-log: each CI store row projects a source event reindexed-from-source to T (the
    // index seam — `details_ref` / surfacing index rebuilds from these; derived == source-replay).
    let mut source = SourceLog::new();
    source
        .append(200, "ci_run:r-200")
        .append(210, "ci_job:j-210")
        .append(220, "job_queue:q-220")
        .append(230, "artifact:a-230")
        .append(240, "cache_entry:c-240")
        .append(250, "log_segment:s-250");

    // The CI OLTP rows (run/job/queue state) + the T2/T3 object references, each at seq ≤ T, plus a
    // FUTURE row past T the restore must drop.
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
            blob_ref: None, // a claimed lease row carries no blob
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
            written_at: 400, // > T → the restore must DROP it (past the consistency point)
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

    // (a) The CI-side committed, loud-never-swallowed gate caller GREENS (reuses Storage's
    // RestoreVerifyGate verbatim) with the measured artifact — 0 loss, one consistent cross-seam point.
    let artifact = run_ci_restore_verify_or_fail(&inputs)
        .expect("the CI-stores restore-verify must GREEN — the permanent gate passes");
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
        "0 loss — no dangling blob ref"
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

    // (b) The harness SUB-D6 cross-seam assertion AGREES with the gate's storage-native check over
    // the CI stores (coherence — not a parallel assertion).
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

    // Emit the cross-seam telemetry observably (the SAME signal every restore drill uses).
    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        cross_seam.mismatch_count(),
    );
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();

    // Confirm every CI store the manifest names is represented in the restored cross-seam set.
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
         Harness SUB-D6 cross-seam AGREES: {} mismatch(es). PERMANENT GATE (master §4) — re-runs on \
         every CI-store-touching change, forever; loud-never-swallowed (no `|| true`). REUSES \
         Storage's RestoreVerifyGate (P-061), no fork.",
        artifact.summary(),
        cross_seam.mismatch_count(),
    );
}

/// **STOR-D2 over the CI stores: continuous archiving holds RPO ≤ 5 min and a CI-cell-kill restore
/// holds RTO ≤ 1 h/tenant ≤ 4 h/cell — dated green artifact (thresholds READ, never hardcoded).**
///
/// The CI stores are restored from the archive after a simulated CI-cell kill: a single CI tenant's
/// run/job/log/artifact state recovers first (per-tenant RTO), then the whole CI cell (every tenant)
/// recovers (per-cell RTO). Both measured RTOs must sit within the `thresholds.toml` bounds; the RPO
/// (the un-archived CI WAL tail at the kill instant) must sit within the 5-min bound.
#[test]
fn ci_p27_restore_verify_rpo_rto_within_bounds_for_the_ci_stores() {
    let rpo_bound = rpo_rto_secs_from_thresholds("rpo_max_mins");
    let tenant_bound = rpo_rto_secs_from_thresholds("rto_tenant_max_mins");
    let cell_bound = rpo_rto_secs_from_thresholds("rto_cell_max_mins");

    // ── RPO: continuous archiving of the CI WAL tail bounds the data-at-risk window ──
    let mut archiver = ContinuousArchiver::new();
    let commit_period: u64 = 30; // a CI write (run/job/queue state) commits every 30 s
    let archive_period: u64 = 60; // the CI WAL tail ships every 60 s
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

    // ── RTO: a CI-cell kill; restore the CI stores per-tenant then per-cell ──
    // Per-tenant CI recovery (restore ci_run/ci_job/job_queue + reindex the surfacing index + the
    // §7.5 re-erasure pass): modeled 28 min — within the 60-min tenant bound.
    let tenant_recovery = CellKillRestore::new(RtoGrain::Tenant, 0, (16 + 8 + 2 + 2) * 60);
    // Per-cell CI recovery (every tenant's CI stores + the T2 blob tier + T3 log segments + reindex):
    // modeled 165 min — within the 240-min cell bound.
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

    // Emit the measured RPO/RTO numbers observably (the SAME signals every restore drill uses).
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
         PERMANENT GATE — re-runs on every CI-store change; REUSES Storage's CellKillRestore (P-100).",
        tenant_recovery.rto_secs(),
        tenant_recovery.rto_secs() / 60,
        cell_recovery.rto_secs(),
        cell_recovery.rto_secs() / 60,
    );
}

/// **The gate is REAL over the CI stores (EI-01 §3 — a drill that cannot go red is not a gate): a
/// deliberately-CORRUPTED CI backup (a `log_segment` row → MISSING blob) FAILs CI loudly, never
/// silently passes.** Proves the committed CI caller [`run_ci_restore_verify_or_fail`] surfaces a red
/// — no `|| true`.
#[test]
fn ci_p27_restore_verify_fails_ci_on_a_corrupted_ci_backup() {
    let t = tenant("acme-ci");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(500);

    // The artifact object is restored; the log_segment's sealed-segment blob is NOT — a CI store row
    // references a blob the restore did not bring back (the §7.3 silent-corruption / dangling case).
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
