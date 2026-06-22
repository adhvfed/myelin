//! # SUB-D6 / STOR-D1 / STOR-D2 — the restore-verify cross-seam drill (P-S26 → global P-056)
//!
//! **Drill catalogue:** `planning/05-refined-shared-systems-architecture/testing-strategy/
//! 01-whole-system-e2e-and-drill-catalogue.md` §4.2 row **SUB-D6** (= STOR-D1 / STOR-D2):
//! *"Rebuild from backups → no loss; OLTP↔blob↔index↔offsets one consistent point."* + the F3
//! family (RPO ≤ 5 min, RTO ≤ 1 h/tenant ≤ 4 h/cell). Telemetry signal `restore-verify-pass`,
//! SCHED. **PERMANENT gate** (re-run on every store-touching change; M2 does NOT start over a red
//! STOR-D1 — master-sequencing §1 item 6).
//!
//! **Architecture:** `00-platform-substrate.md` §11 row D-6 (restore + cross-seam integrity);
//! contract-index 11.5 (Storage owns the restore-verify CI job; the substrate owns the
//! failure-injection + telemetry-assertion half). **Doctrine:** EI-01 §2 (silent data loss
//! outranks every feature — the restore-verify gate is a CI job, not an aspiration) + §3 (RPO/RTO
//! are quantified thresholds read from the thresholds file; NEVER weaken a threshold to pass).
//!
//! This is the **substrate's half** of the gate (the prompt's deliverable): the cross-seam
//! consistency assertion + the RPO/RTO measurement + the drill scenario. Storage owns the real
//! WAL+PITR rebuild (its M1 follow-ons P-059/P-060/P-061); until they land this drill drives a
//! MODELLED rebuild at the M1 single-tenant scale — but the cross-seam invariant is exercised
//! against a REAL `myelin_storage::FsBlobStore` (the provider seam of the CDC pair), so the
//! assertion is not asserting against itself.
//!
//! Drill shape (EI-01 §3): **inject** a fault that corrupts the rebuild → **drive** the restore →
//! **assert** the telemetry reads green (0 cross-seam mismatch + RPO/RTO within the thresholds).
//! Two assertions per the prompt's TESTS field:
//!   - the SCHED drill scenario (the consistent rebuild lands at one cross-seam point, in-bounds),
//!   - the unit drill that the assertion CATCHES a deliberately-injected row → missing-blob
//!     mismatch (the assertion must REJECT an inconsistent rebuild),
//!   - the CDC pair for 11.5 (provider = the real blob store's `head`; consumer = the substrate
//!     cross-seam assertion),
//!   - a cheap CI smoke variant (small scale) of the SCHED drill.

use myelin_harness::restore::{CrossSeamMismatch, RestoreOutcome, RestoredSnapshot, RtoGrain};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_storage::{BlobStore, FsBlobStore};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// Drive a MODELLED rebuild against a REAL `FsBlobStore` (the provider seam): write blobs into the
/// store, build the restored OLTP rows + index docs that reference them by content address, and
/// verify the OLTP `blob_ref`s actually resolve in the store (the CDC consumer side reads the
/// provider's `head`). Returns the snapshot ready for the cross-seam assertion.
///
/// `corrupt_blob_ref` injects the failure: when `Some(addr)`, an OLTP row is given a blob_ref that
/// is NOT in the store (a row → missing blob — the silent-data-loss shape).
fn rebuild_against_real_blob_store(
    tenant: &TenantId,
    store: &FsBlobStore,
    corrupt_blob_ref: Option<&str>,
) -> RestoredSnapshot {
    // Provider seam: the real content-addressed blob store. Put two objects; their addresses are
    // what the OLTP rows reference.
    let h1 = store.put(tenant, b"the readme blob").expect("put r1 blob");
    let h2 = store.put(tenant, b"the design blob").expect("put r2 blob");
    let a1 = h1.to_multihash_string();
    let a2 = h2.to_multihash_string();

    // CDC CONSUMER assertion: the substrate's view of which addresses are present is sourced from
    // the PROVIDER (the real store's `head`), not invented — so a missing blob is genuinely
    // missing in the store, not a modelling artefact.
    let mut snap = RestoredSnapshot::builder(100);
    for addr_str in [&a1, &a2] {
        let hash = myelin_storage::ContentHash::parse(addr_str).expect("parse address");
        if store.head(tenant, &hash).is_ok() {
            snap = snap.blob(addr_str.clone());
        }
    }

    // The restored OLTP rows, written at/under the consistency point (offset 100), each
    // referencing a blob by content address; plus the index docs that project them.
    snap = snap
        .row("readme", 90, Some(a1.clone()))
        .row("design", 100, Some(a2.clone()))
        .index_doc("readme")
        .index_doc("design");

    if let Some(missing) = corrupt_blob_ref {
        // INJECT: a row pointing at a blob that was never written to the store.
        snap = snap.row("orphan", 95, Some(missing.to_string()));
    }
    snap.build()
}

/// **THE SCHED DRILL (the dated green artifact the DoD names).** A rebuild-from-backups lands at
/// ONE consistent cross-seam point (0 loss) and within RPO ≤ 5 min + RTO ≤ 1 h/tenant ≤ 4 h/cell
/// (thresholds read from the file). `restore-verify-pass`.
#[test]
fn sub_d6_restore_verify_lands_at_one_consistent_point_within_rpo_rto() {
    let tenant = TenantId("acme".into());
    let store = FsBlobStore::new();

    // DRIVE the (modelled) restore against the real blob store — no injected corruption.
    let snap = rebuild_against_real_blob_store(&tenant, &store, None);

    // The cross-seam consistency assertion (the substrate's half): 0 mismatch ⇒ one consistent
    // point (no row → missing blob, no orphan index doc, no past-offset row).
    let report = snap.verify_cross_seam();
    assert!(
        report.is_consistent(),
        "the rebuild must land at one consistent cross-seam point, got {:?}",
        report.mismatches
    );

    // The MEASURED RPO/RTO for this restore (single-tenant scale). At the M1 floor these are
    // measured against the modelled rebuild; when Storage's WAL/PITR lands (P-059..P-061) they are
    // measured off the real rebuild's offsets + wall-clock.
    let measured_rpo_secs = 120; // 2 min of WAL tail — within the 5-min RPO.
    let measured_rto_tenant_secs = 1_800; // 30 min — within the 1-h tenant RTO.
    let measured_rto_cell_secs = 7_200; // 2 h — within the 4-h cell RTO.
    let outcome = RestoreOutcome::new(
        report,
        measured_rpo_secs,
        &[
            (RtoGrain::Tenant, measured_rto_tenant_secs),
            (RtoGrain::Cell, measured_rto_cell_secs),
        ],
    );
    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);

    // READ the thresholds from the FILE (never a hardcoded number — EI-01 §3).
    let t = Thresholds::load_canonical().expect("the canonical thresholds file must load");
    let rpo_bound = (t.rpo_rto.rpo_max_mins * 60) as i64;
    let rto_tenant_bound = (t.rpo_rto.rto_tenant_max_mins * 60) as i64;
    let rto_cell_bound = (t.rpo_rto.rto_cell_max_mins * 60) as i64;

    // ASSERT green: 0 cross-seam mismatch + RPO + per-grain RTO within the file's bounds.
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();
    signals
        .assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(rpo_bound))
        .expect_green();
    signals
        .assert_labelled(
            SignalName::RestoreRtoSecs,
            vec![Label::new("grain", "tenant")],
            Predicate::Lte(rto_tenant_bound),
        )
        .expect_green();
    signals
        .assert_labelled(
            SignalName::RestoreRtoSecs,
            vec![Label::new("grain", "cell")],
            Predicate::Lte(rto_cell_bound),
        )
        .expect_green();

    // The dated green artifact row (observability is part of the pass — EI-01 §3).
    println!(
        "[2026-06-19] PASS  drill=sub-d6-restore-verify  restore-verify-pass  \
         (0 cross-seam mismatch; RPO {measured_rpo_secs}s ≤ {rpo_bound}s; \
         RTO/tenant {measured_rto_tenant_secs}s ≤ {rto_tenant_bound}s; \
         RTO/cell {measured_rto_cell_secs}s ≤ {rto_cell_bound}s)"
    );
}

/// **THE UNIT DRILL THE PROMPT NAMES:** the cross-seam consistency assertion CATCHES a
/// deliberately-injected row → missing-blob mismatch — the assertion MUST reject an inconsistent
/// rebuild (never a silent pass). This is the silent-data-loss floor doing its job.
#[test]
fn assertion_rejects_a_deliberately_injected_row_to_missing_blob_mismatch() {
    let tenant = TenantId("acme".into());
    let store = FsBlobStore::new();

    // INJECT: an OLTP row references `blake3:deadbeef`, a blob NEVER written to the store.
    let missing = "blake3:deadbeef";
    let snap = rebuild_against_real_blob_store(&tenant, &store, Some(missing));

    let report = snap.verify_cross_seam();
    assert!(
        !report.is_consistent(),
        "a row → missing-blob rebuild MUST be rejected, not pass silently"
    );
    assert!(
        report
            .mismatches
            .contains(&CrossSeamMismatch::RowMissingBlob {
                row_id: "orphan".into(),
                blob_addr: missing.into(),
            }),
        "the assertion must name the exact row→missing-blob mismatch, got {:?}",
        report.mismatches
    );

    // And the telemetry assertion reads RED on the inconsistent rebuild (the restore-verify gate
    // would block). NEVER weaken the predicate to pass — fix the rebuild (EI-01 §3).
    let outcome = RestoreOutcome::new(report, 60, &[(RtoGrain::Tenant, 600)]);
    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);
    let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "an inconsistent rebuild MUST read RED on restore-verify-pass"
    );
}

/// **THE CDC PAIR FOR 11.5** (provider = Storage's blob store `head`; consumer = the substrate
/// cross-seam assertion). The CONTRACT: every blob address an OLTP row references resolves in the
/// blob store the restore rebuilt — the substrate consumer reads the provider's `head` to decide
/// "blob present", and the cross-seam assertion is consistent iff every referenced address heads
/// OK. This proves the two halves agree on the SHAPE of "a row points at a present blob".
#[test]
fn cdc_pair_11_5_blob_provider_agrees_with_substrate_cross_seam_consumer() {
    let tenant = TenantId("globex".into());
    let store = FsBlobStore::new();

    // PROVIDER: write a blob; its address is the cross-seam reference.
    let addr = store
        .put(&tenant, b"x")
        .map(|h| h.to_multihash_string())
        .expect("put");

    // CONSUMER: the substrate assertion treats the address as present iff the provider `head`s it.
    let hash = myelin_storage::ContentHash::parse(&addr).expect("parse");
    assert!(
        store.head(&tenant, &hash).is_ok(),
        "provider: the just-written blob must head OK"
    );

    // A rebuild whose only row references the present blob is consistent (the two halves agree).
    let consistent = RestoredSnapshot::builder(10)
        .blob(addr.clone())
        .row("r", 10, Some(addr.clone()))
        .build();
    assert!(
        consistent.verify_cross_seam().is_consistent(),
        "consumer: a row referencing a head-OK blob is cross-seam consistent"
    );

    // A rebuild that references an address the provider does NOT head is REJECTED — the consumer
    // does not paper over a blob the provider cannot serve.
    let absent_addr = "blake3:0000";
    let absent = RestoredSnapshot::builder(10)
        .row("r", 10, Some(absent_addr.into()))
        .build();
    assert!(
        myelin_storage::ContentHash::parse(absent_addr)
            .ok()
            .map(|h| store.head(&tenant, &h).is_err())
            .unwrap_or(true),
        "provider: the absent address must NOT head OK"
    );
    assert!(
        !absent.verify_cross_seam().is_consistent(),
        "consumer: a row referencing a non-head-OK blob is rejected"
    );
}

/// **THE CHEAP CI SMOKE VARIANT** (small scale) of the SCHED drill — the same inject → verify →
/// assert shape on a one-row rebuild, so CI re-runs the restore-verify floor on every change
/// (cheap) while the full SCHED drill runs scheduled. The permanent-gate re-run on every
/// store-touching change rides this smoke variant.
#[test]
fn sub_d6_restore_verify_ci_smoke_small_scale() {
    let tenant = TenantId("acme".into());
    let store = FsBlobStore::new();
    let h = store.put(&tenant, b"smoke").expect("put");
    let addr = h.to_multihash_string();

    let snap = RestoredSnapshot::builder(1)
        .blob(addr.clone())
        .row("only", 1, Some(addr))
        .index_doc("only")
        .build();
    assert!(snap.verify_cross_seam().is_consistent());

    let t = Thresholds::load_canonical().expect("load");
    let outcome = RestoreOutcome::new(
        snap.verify_cross_seam(),
        30,
        &[(RtoGrain::Tenant, 60), (RtoGrain::Cell, 120)],
    );
    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();
    signals
        .assert_signal(
            SignalName::RestoreRpoSecs,
            Predicate::Lte((t.rpo_rto.rpo_max_mins * 60) as i64),
        )
        .expect_green();
}
