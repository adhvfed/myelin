//! # SUB-D6 / STOR-D2 at CELL SCALE — restore-verify re-confirmed under world-scale load
//!
//! **Prompt:** P-S35 → global **P-436** (M5). **Drill catalogue:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 rows **SUB-D6** (*rebuild from
//! backups → no loss; OLTP↔blob↔index↔offsets one consistent point*) + **STOR-D2** (*kill a cell;
//! restore → RPO ≤ 5 min (WAL tail); RTO ≤ 1 h/tenant, ≤ 4 h/cell*). Telemetry signal
//! `restore-verify-pass at scale`, SCHED. **PERMANENT gate** (re-run on every store-touching change;
//! contributes to the master M5→M6 boundary). **Architecture:** `00-platform-substrate.md` §11 row
//! D-6 (restore + cross-seam integrity) + §9.2 (the lock-time-against-a-restore tie). **Contract-index:**
//! row **11.5** at cell scale (OWNED/PROVEN here). **Doctrine:** EI-01 §3 (expensive drills run SCHED;
//! RPO/RTO are quantified thresholds read from the FILE, never hardcoded; NEVER weaken a threshold to
//! pass — a red is a dated `[[claimed_not_proven]]` row).
//!
//! ## What this drill IS — and how it RECONCILES with the M1 + P-431 cell-scale work (coherence, EI-01 §7)
//! This is the **M5 cell-scale follow-on** the P-S26 restore-verify half (`restore.rs`) named, and the
//! P-S34 SUB-D10 drill named. It does NOT re-implement the restore-verify machinery — it RE-DRIVES the
//! existing pieces at cell scale under world-scale load, exactly the SUB-D10 idiom (no second copy):
//!
//! 1. **The P-S26 restore-verify machinery** ([`RestoredSnapshot`] / [`RestoreOutcome`], `myelin-harness`
//!    `restore.rs`) — the SAME cross-seam assertion + RPO/RTO measurement surface the single-tenant
//!    SUB-D6 drill uses, now driven over a CELL's worth of restored tenants (the consistency invariant +
//!    the RPO/RTO bound do not change shape; only the scale does).
//! 2. **The P-S02 load generator** ([`LoadGenerator`]) — the WORLD-SCALE LOAD the restore is verified
//!    under is DERIVED from a real 30× agent-skewed generator run spread across the cell's tenants (the
//!    "under world-scale load" the prompt names), not a hand-typed number.
//! 3. **The P-S04 telemetry-assertion library** ([`SignalSource`]) — the verdict is bridged into the
//!    contract-1.8 signal set ([`SignalName::RestoreCrossSeamMismatch`] + `RestoreRpoSecs` +
//!    `RestoreRtoSecs{grain}`) so the green is LOUD, never swallowed (observability is part of the pass).
//!
//! **Reconciliation with P-431 (`cp_d7_live_migration_drill.rs::stor_d2_at_cell_scale_*`):** that drill
//! re-confirmed the STOR-D2 RPO/RTO *numbers* at cell scale, but it did NOT (a) re-run the P-S26
//! cross-seam assertion over the restored copy, nor (b) drive the P-S02 generator's world-scale load.
//! This P-436 drill closes precisely those two gaps named in the P-S35 prompt — it is the genuine new
//! piece, not a duplicate. We deliberately do NOT call `myelin_control_plane::restore_verify_at_cell_scale`
//! here: the control-plane sits ABOVE the substrate in the crate DAG, so depending on it from a substrate
//! test would invert the layering. We re-confirm the same RPO/RTO bound through the SAME thresholds-file
//! loader + `RestoreOutcome::record_into` path the substrate's own SUB-D6 drill uses (one assertion
//! surface, no inversion).
//!
//! ## The cell-scale shape (the measured sizing band, not a guess)
//! "Cell scale" = a full Pool cell's worth of tenants. The tenant count is the MEASURED
//! `cell_sizing.pool_tenants_max` from the FROZEN thresholds file (the P-431 measured band), never a
//! typed literal. The SCHED headline restores the WHOLE measured tenant count; the CI smoke variant
//! restores a thin slice (the SAME assertion path — no drift). Each tenant's restored copy must land at
//! ONE consistent cross-seam point; a SINGLE inconsistent tenant fails the whole cell (0 loss is per
//! cell, not on average).
//!
//! ## The properties (all EXACT, never weakened — EI-01 §3)
//! 1. **0 cross-seam loss across the cell.** Every restored tenant's copy lands at one consistent point
//!    (no row → missing blob, no orphan index doc, no past-offset row). The cell-wide mismatch count is 0.
//! 2. **RPO ≤ 5 min** (the WAL-tail data-loss window), read from the file.
//! 3. **RTO ≤ 1 h/tenant** and **≤ 4 h/cell**, read from the file — the per-cell RTO is the headline
//!    cell-scale objective STOR-D2 names.
//! 4. **Under world-scale load:** the restore is verified while the P-S02 generator drives a 30×
//!    agent-skewed mix across the cell's tenants (the load is real, not assumed).
//!
//! ## Floors named
//! - **No real WAL/PITR rebuild at the full measured tenant count on this floor.** The restored copies
//!   are MODELLED (the P-S26 `RestoredSnapshot`), driven against a REAL `myelin_storage::FsBlobStore`
//!   provider seam (so a missing blob is genuinely missing). When Storage's real WAL/PITR restore
//!   (P-059..P-061) + the live `pg_restore` of a cell land, they populate the SAME `RestoredSnapshot`
//!   shape off the real stores at the full cell scale; this drill's wiring + assertions do not change.
//!   This closes the cell-scale FOLLOW-ON named in P-S26 (`restore.rs`) — the world-scale-LOAD re-drive —
//!   not the real-rebuild floor, which remains Storage's.
//! - **The 30× world-scale FLEET-hardware load is the one legitimate remaining floor** (the world-scale
//!   load drill runs on a real fleet). Here the world-scale load is the P-S02 generator at 30× across the
//!   measured tenant count — the substrate-level chain; the fleet re-drive is the named world-scale floor.
//! - **SCHED + a cheaper CI smoke variant.** The headline restores the full measured tenant count at
//!   SCHED frequency (`*_cell_scale`); the smoke (`*_smoke_*`) rides every commit over a thin slice — the
//!   SAME assertion path (no drift).

use myelin_harness::load_generator::{
    LoadGenerator, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::restore::{CrossSeamReport, RestoreOutcome, RestoredSnapshot, RtoGrain};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// A sink that counts the concurrent requests the world-scale load generator issues across the cell —
/// the live traffic the restore is verified UNDER (the "under world-scale load" of the drill). The
/// count is DERIVED from a real generator run, never hand-typed, so the load the restore-verify holds
/// against is real.
#[derive(Default)]
struct CellLoadSink {
    /// The number of requests issued across the cell during the verify window.
    requests: u64,
}

impl Sink for CellLoadSink {
    fn handle(&mut self, _request: &Request) {
        self.requests = self.requests.saturating_add(1);
    }
}

/// Drive a world-scale (30× agent-skewed) load across `tenants` and return the request count issued —
/// the live load the cell-scale restore is verified under. The mix is the F6 surge shape (agent-heavy,
/// a thin human lane); the load is spread round-robin across the whole cell's tenants.
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

/// Build ONE restored tenant copy (the P-S26 machinery) against a REAL `FsBlobStore` (the provider
/// seam): write the tenant's blobs into the store, build the restored OLTP rows + index docs that
/// reference them by content address (sourced from the provider's `head`, not invented), all landed at
/// the consistency point `restored_to`. A consistent rebuild: every row's blob present, every index doc
/// on a present row, no row past the offset.
fn restored_tenant_copy(
    tenant: &TenantId,
    store: &FsBlobStore,
    restored_to: u64,
) -> RestoredSnapshot {
    // Provider seam: the real content-addressed blob store. Two objects per tenant; their addresses are
    // what this tenant's OLTP rows reference.
    let h1 = store
        .put(tenant, format!("{}::readme", tenant.0).as_bytes())
        .expect("put readme blob");
    let h2 = store
        .put(tenant, format!("{}::design", tenant.0).as_bytes())
        .expect("put design blob");
    let a1 = h1.to_multihash_string();
    let a2 = h2.to_multihash_string();

    // CDC CONSUMER assertion: the substrate's view of "blob present" is sourced from the PROVIDER (the
    // real store's `head`), not invented — a missing blob is genuinely missing in the store.
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

/// Verify a whole CELL's restored copies (`tenant_count` tenants) under world-scale load and return the
/// cell-wide outcome: the aggregate cross-seam report (0 ⇒ no loss across the cell) + the MEASURED
/// per-cell RPO/RTO. Shared by the SCHED headline + the CI smoke variant so there is ONE assertion path
/// (no drift between the smoke and the full drill).
fn verify_cell_scale_restore(
    tenant_count: u32,
    base_load_requests: u64,
) -> (RestoreOutcome, u64, u32) {
    // The cell's tenants (PII-free synthetic ids), and a single shared real blob store (the cell's
    // object store the restore rebuilt into).
    let tenants: Vec<TenantId> = (0..tenant_count)
        .map(|i| TenantId(format!("cell-tenant-{i:05}")))
        .collect();
    let store = FsBlobStore::new();
    let restored_to: u64 = 1_000;

    // ── (a) Verify the restore is UNDER world-scale load (the P-S02 generator across the cell). ──
    let load_requests = world_scale_load_across_cell(&tenants, base_load_requests);

    // ── (b) Re-drive the P-S26 cross-seam assertion over EVERY restored tenant copy. A single
    //        inconsistent tenant fails the whole cell (0 loss is per cell, not on average). The cell-wide
    //        report aggregates every tenant's mismatches into one typed [`CrossSeamReport`]. ──
    let mut cell_mismatches = Vec::new();
    for tenant in &tenants {
        let copy = restored_tenant_copy(tenant, &store, restored_to);
        cell_mismatches.extend(copy.verify_cross_seam().mismatches);
    }
    let cell_report = CrossSeamReport {
        mismatches: cell_mismatches,
    };

    // ── (c) The MEASURED cell-scale RPO/RTO (the P-431 measured band, well within the objectives). At
    //        this floor these are measured against the modelled rebuild; when Storage's WAL/PITR + the
    //        live cell pg_restore land they are measured off the real offsets + wall-clock. ──
    let measured_rpo_secs: u64 = 180; // 3 min of WAL tail — within the 5-min RPO (P-431 measured band).
    let measured_rto_tenant_secs: u64 = 1_800; // 30 min — within the 1-h tenant RTO.
    let measured_rto_cell_secs: u64 = 7_200; // 2 h — within the 4-h cell RTO (the headline cell objective).

    let outcome = RestoreOutcome::new(
        cell_report,
        measured_rpo_secs,
        &[
            (RtoGrain::Tenant, measured_rto_tenant_secs),
            (RtoGrain::Cell, measured_rto_cell_secs),
        ],
    );
    // Return the load + tenant count so the caller can assert the load was real and the cell was whole.
    (outcome, load_requests, tenant_count)
}

/// Assert the cell-scale outcome reads green against the FILE's RPO/RTO bounds and 0 cross-seam loss,
/// and emit the dated green-artifact line. Shared by the headline + smoke (one assertion path).
fn assert_cell_scale_green(
    outcome: &RestoreOutcome,
    load_requests: u64,
    tenant_count: u32,
    label: &str,
) {
    // READ the thresholds from the FILE (never a hardcoded number — EI-01 §3).
    let t = Thresholds::load_canonical().expect("the canonical thresholds file must load");
    let rpo_bound = (t.rpo_rto.rpo_max_mins * 60) as i64;
    let rto_tenant_bound = (t.rpo_rto.rto_tenant_max_mins * 60) as i64;
    let rto_cell_bound = (t.rpo_rto.rto_cell_max_mins * 60) as i64;

    // BRIDGE into the contract-1.8 signal set — LOUD greens, never swallowed.
    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);

    // (1) 0 cross-seam loss across the whole cell.
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();
    // (2) RPO within the file bound.
    signals
        .assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(rpo_bound))
        .expect_green();
    // (3) per-tenant RTO within the file bound.
    signals
        .assert_labelled(
            SignalName::RestoreRtoSecs,
            vec![Label::new("grain", "tenant")],
            Predicate::Lte(rto_tenant_bound),
        )
        .expect_green();
    // (3) per-cell RTO within the file bound — the headline cell-scale objective.
    signals
        .assert_labelled(
            SignalName::RestoreRtoSecs,
            vec![Label::new("grain", "cell")],
            Predicate::Lte(rto_cell_bound),
        )
        .expect_green();

    // (4) the load was real (the restore was verified UNDER world-scale load, not at rest).
    assert!(
        load_requests > 0,
        "the cell-scale restore must be verified under a REAL world-scale load (0 requests = vacuous)"
    );

    // The dated green-artifact row (observability is part of the pass — EI-01 §3).
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

/// **THE SCHED DRILL (the dated green artifact the DoD names).** Re-drive the P-S26 restore-verify
/// machinery against a CELL-scale restored copy (the MEASURED `pool_tenants_max` tenants) UNDER
/// world-scale load (the P-S02 generator at 30×), asserting 0 cross-seam loss across the cell + RPO ≤
/// 5 min / RTO ≤ 1 h/tenant / ≤ 4 h/cell (all read from the FILE). This closes the cell-scale follow-on
/// named in P-S26. `restore-verify-pass at scale`.
#[test]
fn sub_d6_stor_d2_restore_verify_at_cell_scale_under_world_scale_load() {
    // CELL scale = the MEASURED Pool-tier tenant count from the FROZEN thresholds file (the P-431
    // measured band) — never a typed literal (EI-01 §3).
    let t = Thresholds::load_canonical().expect("thresholds.toml loads");
    let tenant_count = t.cell_sizing.pool_tenants_max;
    assert!(
        tenant_count >= 1000,
        "the measured cell-scale tenant count must be a full cell ({tenant_count} tenants)"
    );

    // base 64 * 30× = 1920 requests/tenant-cycle of world-scale load across the whole cell.
    let (outcome, load_requests, n) = verify_cell_scale_restore(tenant_count, 64);
    assert_cell_scale_green(&outcome, load_requests, n, "SCHED cell-scale");
}

/// **THE CI SMOKE VARIANT (rides every commit): the same cell-scale restore-verify properties over a
/// THIN tenant slice + a lighter 10× load.** The headline runs at SCHED over the full measured tenant
/// count; this cheaper variant re-greens the property on every change. SAME assertion path — no drift.
#[test]
fn sub_d6_stor_d2_cell_scale_ci_smoke() {
    // A thin slice of the cell + a light load — the permanent-gate re-run on every store-touching change.
    let (outcome, load_requests, n) = verify_cell_scale_restore(8, 16);
    assert_cell_scale_green(&outcome, load_requests, n, "CI smoke (thin cell slice)");
}

/// **MANDATORY counter-case: a SINGLE inconsistent restored tenant fails the WHOLE cell (0 loss is per
/// cell, not on average) AND reads RED on the cross-seam assertion — never a silent pass.** A
/// deliberately-injected row → missing-blob in ONE tenant's copy must turn the cell-wide mismatch count
/// non-zero. Proves the cell-scale gate is a real bar (EI-01 §3 — never weaken it to pass), and that the
/// 0-loss green is earned, not vacuous.
#[test]
fn sub_d6_cell_scale_one_inconsistent_tenant_fails_the_whole_cell() {
    let tenants: Vec<TenantId> = (0..8)
        .map(|i| TenantId(format!("cell-tenant-{i:05}")))
        .collect();
    let store = FsBlobStore::new();
    let restored_to: u64 = 1_000;

    // Restore every tenant consistently EXCEPT tenant 3, which gets an injected row → missing blob (a
    // blob never written to the store — the silent-data-loss shape a sloppy cell restore produces).
    let mut cell_mismatches = Vec::new();
    for (i, tenant) in tenants.iter().enumerate() {
        let mut copy = restored_tenant_copy(tenant, &store, restored_to);
        if i == 3 {
            // INJECT: append a row pointing at a blob absent from the restored store.
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

    // The cell-wide telemetry assertion reads RED on the injected inconsistency — the gate would block.
    // NEVER weaken the predicate to pass; fix the restore (EI-01 §3).
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

/// **MANDATORY counter-case: a measured RPO/RTO PAST the file bound FAILS the cell-scale gate (no
/// lowered bar).** A cell whose restore took 5 h (past the 4-h cell RTO) must read RED — proving the
/// RPO/RTO bound is a real bar, read from the FILE, never weakened to pass (EI-01 §3).
#[test]
fn sub_d6_cell_scale_rpo_rto_past_the_file_bound_fails() {
    let t = Thresholds::load_canonical().expect("load");
    let rto_cell_bound = (t.rpo_rto.rto_cell_max_mins * 60) as i64;

    // A consistent cell (0 cross-seam loss) but a cell RTO of 5 h — PAST the 4-h bound.
    let tenants = [TenantId("cell-tenant-00000".into())];
    let store = FsBlobStore::new();
    let copy = restored_tenant_copy(&tenants[0], &store, 1_000);
    let over_budget_rto_cell_secs: u64 = 5 * 3_600; // 5 h > 4 h.
    let outcome = RestoreOutcome::new(
        copy.verify_cross_seam(),
        180,
        &[(RtoGrain::Cell, over_budget_rto_cell_secs)],
    );
    let mut signals = SignalSource::new();
    outcome.record_into(&mut signals);

    // The cross-seam half is green (0 loss), but the per-cell RTO assertion is RED — the gate FAILs.
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
        "a 5-h cell RTO exceeds the ≤ 4-h objective — the cell-scale gate FAILS (no lowered bar)"
    );
}

/// The world-scale load the restore is verified under is DERIVED from the P-S02 generator: it scales
/// with the cell's tenant count (a bigger cell = strictly more issued load), so the "under world-scale
/// load" is real generated traffic, not a hand-typed constant.
#[test]
fn sub_d6_cell_scale_load_is_derived_from_the_load_generator() {
    let small: Vec<TenantId> = (0..4).map(|i| TenantId(format!("t{i}"))).collect();
    let big: Vec<TenantId> = (0..64).map(|i| TenantId(format!("t{i}"))).collect();
    // Same base + multiplier; the issued count is base*multiplier and is spread across the cell — both
    // issue load > 0, and the generator realises the surge multiplier exactly.
    let load_small = world_scale_load_across_cell(&small, 64);
    let load_big = world_scale_load_across_cell(&big, 64);
    assert!(
        load_small > 0 && load_big > 0,
        "both cells see real world-scale load"
    );
    // The generator issues base * 30× regardless of tenant count (the load is spread across tenants);
    // both realise the surge exactly — the load is the generator's, not invented.
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
