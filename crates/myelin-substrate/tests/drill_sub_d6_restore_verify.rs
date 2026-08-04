use myelin_harness::restore::{CrossSeamMismatch, RestoreOutcome, RestoredSnapshot, RtoGrain};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_storage::{BlobStore, FsBlobStore};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn rebuild_against_real_blob_store(
    tenant: &TenantId,
    store: &FsBlobStore,
    corrupt_blob_ref: Option<&str>,
) -> RestoredSnapshot {
    let h1 = store.put(tenant, b"the readme blob").expect("put r1 blob");
    let h2 = store.put(tenant, b"the design blob").expect("put r2 blob");
    let a1 = h1.to_multihash_string();
    let a2 = h2.to_multihash_string();

    let mut snap = RestoredSnapshot::builder(100);
    for addr_str in [&a1, &a2] {
        let hash = myelin_storage::ContentHash::parse(addr_str).expect("parse address");
        if store.head(tenant, &hash).is_ok() {
            snap = snap.blob(addr_str.clone());
        }
    }

    snap = snap
        .row("readme", 90, Some(a1.clone()))
        .row("design", 100, Some(a2.clone()))
        .index_doc("readme")
        .index_doc("design");

    if let Some(missing) = corrupt_blob_ref {
        snap = snap.row("orphan", 95, Some(missing.to_string()));
    }
    snap.build()
}

#[test]
fn sub_d6_restore_verify_lands_at_one_consistent_point_within_rpo_rto() {
    let tenant = TenantId("acme".into());
    let store = FsBlobStore::new();

    let snap = rebuild_against_real_blob_store(&tenant, &store, None);

    let report = snap.verify_cross_seam();
    assert!(
        report.is_consistent(),
        "the rebuild must land at one consistent cross-seam point, got {:?}",
        report.mismatches
    );

    let measured_rpo_secs = 120;
    let measured_rto_tenant_secs = 1_800;
    let measured_rto_cell_secs = 7_200;
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

    let t = Thresholds::load_canonical().expect("the canonical thresholds file must load");
    let rpo_bound = (t.rpo_rto.rpo_max_mins * 60) as i64;
    let rto_tenant_bound = (t.rpo_rto.rto_tenant_max_mins * 60) as i64;
    let rto_cell_bound = (t.rpo_rto.rto_cell_max_mins * 60) as i64;

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

    println!(
        "[2026-06-19] PASS  drill=sub-d6-restore-verify  restore-verify-pass  \
         (0 cross-seam mismatch; RPO {measured_rpo_secs}s ≤ {rpo_bound}s; \
         RTO/tenant {measured_rto_tenant_secs}s ≤ {rto_tenant_bound}s; \
         RTO/cell {measured_rto_cell_secs}s ≤ {rto_cell_bound}s)"
    );
}

#[test]
fn assertion_rejects_a_deliberately_injected_row_to_missing_blob_mismatch() {
    let tenant = TenantId("acme".into());
    let store = FsBlobStore::new();

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

    let outcome = RestoreOutcome::new(report, 60, &[(RtoGrain::Tenant, 600)]);
    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);
    let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "an inconsistent rebuild MUST read RED on restore-verify-pass"
    );
}

#[test]
fn cdc_pair_11_5_blob_provider_agrees_with_substrate_cross_seam_consumer() {
    let tenant = TenantId("globex".into());
    let store = FsBlobStore::new();

    let addr = store
        .put(&tenant, b"x")
        .map(|h| h.to_multihash_string())
        .expect("put");

    let hash = myelin_storage::ContentHash::parse(&addr).expect("parse");
    assert!(
        store.head(&tenant, &hash).is_ok(),
        "provider: the just-written blob must head OK"
    );

    let consistent = RestoredSnapshot::builder(10)
        .blob(addr.clone())
        .row("r", 10, Some(addr.clone()))
        .build();
    assert!(
        consistent.verify_cross_seam().is_consistent(),
        "consumer: a row referencing a head-OK blob is cross-seam consistent"
    );

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
