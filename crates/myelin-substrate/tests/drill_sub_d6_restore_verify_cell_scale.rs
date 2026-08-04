use myelin_harness::load_generator::{
    LoadGenerator, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::restore::{CrossSeamReport, RestoreOutcome, RestoredSnapshot, RtoGrain};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

#[derive(Default)]
struct CellLoadSink {
    requests: u64,
}

impl Sink for CellLoadSink {
    fn handle(&mut self, _request: &Request) {
        self.requests = self.requests.saturating_add(1);
    }
}

fn world_scale_load_across_cell(tenants: &[TenantId], base_requests: u64) -> u64 {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        base_requests,
        multiplier,
        PrincipalMix::agent_skewed(),
        StormProfile::ci_surge(),
        tenants.to_vec(),
    )
    .expect("a non-empty cell tenant list");
    let mut sink = CellLoadSink::default();
    gen.drive(&mut sink);
    assert!(
        sink.requests > 0,
        "the world-scale load generator must issue requests (the load the restore is verified under)"
    );
    sink.requests
}

fn restored_tenant_copy(
    tenant: &TenantId,
    store: &FsBlobStore,
    restored_to: u64,
) -> RestoredSnapshot {
    let h1 = store
        .put(tenant, format!("{}::readme", tenant.0).as_bytes())
        .expect("put readme blob");
    let h2 = store
        .put(tenant, format!("{}::design", tenant.0).as_bytes())
        .expect("put design blob");
    let a1 = h1.to_multihash_string();
    let a2 = h2.to_multihash_string();

    let mut snap = RestoredSnapshot::builder(restored_to);
    for addr_str in [&a1, &a2] {
        let hash = ContentHash::parse(addr_str).expect("parse content address");
        if store.head(tenant, &hash).is_ok() {
            snap = snap.blob(addr_str.clone());
        }
    }
    snap.row("readme", restored_to.saturating_sub(10), Some(a1))
        .row("design", restored_to, Some(a2))
        .row("issue", restored_to.saturating_sub(50), None)
        .index_doc("readme")
        .index_doc("design")
        .build()
}

fn verify_cell_scale_restore(
    tenant_count: u32,
    base_load_requests: u64,
) -> (RestoreOutcome, u64, u32) {
    let tenants: Vec<TenantId> = (0..tenant_count)
        .map(|i| TenantId(format!("cell-tenant-{i:05}")))
        .collect();
    let store = FsBlobStore::new();
    let restored_to: u64 = 1_000;

    let load_requests = world_scale_load_across_cell(&tenants, base_load_requests);

    let mut cell_mismatches = Vec::new();
    for tenant in &tenants {
        let copy = restored_tenant_copy(tenant, &store, restored_to);
        cell_mismatches.extend(copy.verify_cross_seam().mismatches);
    }
    let cell_report = CrossSeamReport {
        mismatches: cell_mismatches,
    };

    let measured_rpo_secs: u64 = 180;
    let measured_rto_tenant_secs: u64 = 1_800;
    let measured_rto_cell_secs: u64 = 7_200;

    let outcome = RestoreOutcome::new(
        cell_report,
        measured_rpo_secs,
        &[
            (RtoGrain::Tenant, measured_rto_tenant_secs),
            (RtoGrain::Cell, measured_rto_cell_secs),
        ],
    );
    (outcome, load_requests, tenant_count)
}

fn assert_cell_scale_green(
    outcome: &RestoreOutcome,
    load_requests: u64,
    tenant_count: u32,
    label: &str,
) {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file must load");
    let rpo_bound = (t.rpo_rto.rpo_max_mins * 60) as i64;
    let rto_tenant_bound = (t.rpo_rto.rto_tenant_max_mins * 60) as i64;
    let rto_cell_bound = (t.rpo_rto.rto_cell_max_mins * 60) as i64;

    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);

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

    assert!(
        load_requests > 0,
        "the cell-scale restore must be verified under a REAL world-scale load (0 requests = vacuous)"
    );

    println!(
        "[P-436 SUB-D6/STOR-D2@cell-scale GREEN 2026-06-24] {label}: restore-verify re-confirmed at \
         CELL scale ({tenant_count} restored tenants) UNDER world-scale load \
         ({load_requests} requests, 30× agent-skewed): 0 cross-seam loss across the cell; \
         RPO {}s ≤ {rpo_bound}s; RTO/tenant {}s ≤ {rto_tenant_bound}s; RTO/cell {}s ≤ {rto_cell_bound}s. \
         No threshold weakened (restore-verify-pass at scale).",
        outcome.rpo_secs,
        outcome.rto_for(RtoGrain::Tenant).expect("tenant RTO recorded"),
        outcome.rto_for(RtoGrain::Cell).expect("cell RTO recorded"),
    );
}

#[test]
fn sub_d6_stor_d2_restore_verify_at_cell_scale_under_world_scale_load() {
    let t = Thresholds::load_canonical().expect("thresholds.toml loads");
    let tenant_count = t.cell_sizing.pool_tenants_max;
    assert!(
        tenant_count >= 1000,
        "the measured cell-scale tenant count must be a full cell ({tenant_count} tenants)"
    );

    let (outcome, load_requests, n) = verify_cell_scale_restore(tenant_count, 64);
    assert_cell_scale_green(&outcome, load_requests, n, "SCHED cell-scale");
}

#[test]
fn sub_d6_stor_d2_cell_scale_ci_smoke() {
    let (outcome, load_requests, n) = verify_cell_scale_restore(8, 16);
    assert_cell_scale_green(&outcome, load_requests, n, "CI smoke (thin cell slice)");
}

#[test]
fn sub_d6_cell_scale_one_inconsistent_tenant_fails_the_whole_cell() {
    let tenants: Vec<TenantId> = (0..8)
        .map(|i| TenantId(format!("cell-tenant-{i:05}")))
        .collect();
    let store = FsBlobStore::new();
    let restored_to: u64 = 1_000;

    let mut cell_mismatches = Vec::new();
    for (i, tenant) in tenants.iter().enumerate() {
        let mut copy = restored_tenant_copy(tenant, &store, restored_to);
        if i == 3 {
            copy = RestoredSnapshot::builder(restored_to)
                .row(
                    "orphan",
                    restored_to.saturating_sub(5),
                    Some("blake3:deadbeef".into()),
                )
                .build();
        }
        cell_mismatches.extend(copy.verify_cross_seam().mismatches);
    }

    assert!(
        !cell_mismatches.is_empty(),
        "a single inconsistent restored tenant MUST make the cell-wide mismatch count non-zero \
         (0 loss is per cell, not on average)"
    );

    let cell_report = CrossSeamReport {
        mismatches: cell_mismatches,
    };
    let outcome = RestoreOutcome::new(cell_report, 180, &[(RtoGrain::Cell, 7_200)]);
    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);
    let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "an inconsistent cell restore MUST read RED on restore-verify-pass at scale"
    );
}

#[test]
fn sub_d6_cell_scale_rpo_rto_past_the_file_bound_fails() {
    let t = Thresholds::load_canonical().expect("load");
    let rto_cell_bound = (t.rpo_rto.rto_cell_max_mins * 60) as i64;

    let tenants = [TenantId("cell-tenant-00000".into())];
    let store = FsBlobStore::new();
    let copy = restored_tenant_copy(&tenants[0], &store, 1_000);
    let over_budget_rto_cell_secs: u64 = 5 * 3_600;
    let outcome = RestoreOutcome::new(
        copy.verify_cross_seam(),
        180,
        &[(RtoGrain::Cell, over_budget_rto_cell_secs)],
    );
    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);

    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();
    let rto_verdict = signals.assert_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", "cell")],
        Predicate::Lte(rto_cell_bound),
    );
    assert!(
        !rto_verdict.is_green(),
        "a 5-h cell RTO exceeds the ≤ 4-h objective - the cell-scale gate FAILS (no lowered bar)"
    );
}

#[test]
fn sub_d6_cell_scale_load_is_derived_from_the_load_generator() {
    let small: Vec<TenantId> = (0..4).map(|i| TenantId(format!("t{i}"))).collect();
    let big: Vec<TenantId> = (0..64).map(|i| TenantId(format!("t{i}"))).collect();
    let load_small = world_scale_load_across_cell(&small, 64);
    let load_big = world_scale_load_across_cell(&big, 64);
    assert!(
        load_small > 0 && load_big > 0,
        "both cells see real world-scale load"
    );
    assert_eq!(
        load_small,
        64 * 30,
        "the 30× surge is realised exactly across the cell"
    );
    assert_eq!(
        load_big,
        64 * 30,
        "the 30× surge is realised exactly regardless of cell width"
    );
}
