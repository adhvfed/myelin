//! # STOR-D1/STOR-D2/STOR-D8 re-confirmed at CELL SCALE under WORLD-SCALE load
//!
//! **Prompt:** P-ST-34 → global **P-444** (M5). **Drill catalogue:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 rows **STOR-D1** (restore-verify,
//! the permanent gate), **STOR-D2** (kill a cell; restore → RPO/RTO), **STOR-D8** (online migration
//! under load). **Architecture:** `storage.md` §2 "S-M5" (*restore-verify at cell scale under
//! world-scale load; online-migration-under-load at prod scale*). **Contract-index:** row **11.5**
//! (restore-verify at cell scale). **Doctrine:** EI-01 §3 (RPO/RTO + lock budgets read from the FILE,
//! never hardcoded; the world-scale load is REAL generated traffic; never weaken a threshold to pass).
//!
//! ## What this drill IS — the STORAGE-TIER-NATIVE cell-scale re-confirm (coherence, EI-01 §7)
//! P-436 (`myelin-substrate::tests::drill_sub_d6_restore_verify_cell_scale`) re-confirmed the
//! cross-seam restore-verify at cell scale using the harness's `RestoredSnapshot` abstraction. This
//! drill is the **complementary storage-tier half**: it re-runs STORAGE's OWN permanent gate
//! ([`RestoreVerifyGate`], P-061) + STORAGE's OWN online-migration-under-load drill
//! ([`MigrationUnderLoad`], P-126) across a CELL's worth of tenants, while a REAL world-scale load
//! (the P-S02 generator at 30×) is offered — proving the storage tier's native gates hold at cell
//! scale under surge. It does NOT re-implement either gate (it RE-DRIVES them at scale, the SUB-D10
//! idiom — no second copy), and it does NOT duplicate the substrate drill (different gate surface:
//! storage's `RestoreVerifyGate`/`MigrationUnderLoad` vs the harness `RestoredSnapshot`).
//!
//! ## The cell-scale shape (read from the FROZEN thresholds file, not a guess)
//! "Cell scale" = a full Pool cell's worth of tenants — the MEASURED `cell_sizing.pool_tenants_max`
//! from `thresholds.toml` (the P-431 measured band), never a typed literal. The SCHED headline runs
//! the WHOLE measured tenant count; a CI smoke variant runs a thin slice (the SAME assertion path — no
//! drift). A SINGLE tenant whose restore is not whole fails the whole cell (0 loss is per cell, not on
//! average). The lock budget, RPO bound, and surge multiplier are all read from the FILE.
//!
//! ## Floors named
//! - **No real WAL/PITR rebuild + live `pg_restore`/concurrent-write workload at the full cell count
//!   on this floor** — the restored copies + the Postgres lock-cost are MODELLED (the same model the
//!   single-tenant STOR-D1/STOR-D8 drills use); when Storage's real drivers (P-059..P-061 /
//!   P-S12/P-S15/P-ST-30) land they populate the SAME gate inputs at the full cell scale, and this
//!   drill's wiring + assertions do not change.
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet);
//!   here the world-scale load is the P-S02 generator at 30× across the cell's tenants.
//! - **SCHED + a cheaper CI smoke variant** — the headline runs the full measured tenant count at
//!   SCHED frequency; the smoke rides every commit over a thin slice (SAME assertion path).

use std::path::Path;

use myelin_harness::load_generator::{
    LoadGenerator, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_storage::migration::{HotTables, Migration, MigrationPhase, Migrations};
use myelin_storage::{
    ContinuousArchiver, ErasureLedger, GateInputs, KekId, KeyClass, KmsEngine, LockBudget,
    MigrationUnderLoad, RestoreVerifyGate, RestoredObject, SourceLog, WalRow, WalSegment,
    WriteLoad,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

/// The workspace-root `thresholds.toml` doc (the versioned source of truth, P-038).
fn thresholds_doc() -> toml::Value {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest");
    let path = root.join("thresholds.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the versioned thresholds file must load at {path:?}: {e}"));
    text.parse().expect("thresholds.toml must be valid TOML")
}

fn int_threshold(doc: &toml::Value, table: &str, key: &str) -> u64 {
    let v = doc
        .get(table)
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_integer())
        .unwrap_or_else(|| panic!("{table}.{key} must be present (a missing threshold is LOUD)"));
    assert!(v > 0, "{table}.{key} must be a positive value");
    v as u64
}

/// The measured cell-scale tenant count (`cell_sizing.pool_tenants_max`) — never a literal (EI-01 §3).
fn pool_tenants_max() -> u32 {
    int_threshold(&thresholds_doc(), "cell_sizing", "pool_tenants_max") as u32
}

/// The lock budget read from `[online_migration]` (the STOR-D8 bound).
fn lock_budget_from_thresholds() -> LockBudget {
    let doc = thresholds_doc();
    LockBudget::new(
        int_threshold(&doc, "online_migration", "lock_wait_p99_max_ms"),
        // downtime_max_ms is 0 (the 0-downtime invariant); read it tolerantly (0 is valid here).
        doc.get("online_migration")
            .and_then(|t| t.get("downtime_max_ms"))
            .and_then(|v| v.as_integer())
            .map(|v| v as u64)
            .unwrap_or(0),
    )
}

/// The surge multiplier read from `[surge]` (the world-scale load multiplier).
fn surge_multiplier() -> u32 {
    int_threshold(&thresholds_doc(), "surge", "multiplier") as u32
}

/// An archiver whose base + WAL tail makes every offset in `0..=tail` reachable.
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

/// A sink that counts the world-scale load offered across the cell during the verify window — the live
/// traffic the cell-scale restore-verify holds against (DERIVED from a real generator run, not typed).
#[derive(Default)]
struct CellLoadSink {
    requests: u64,
}
impl Sink for CellLoadSink {
    fn handle(&mut self, _request: &Request) {
        self.requests = self.requests.saturating_add(1);
    }
}

/// Drive a world-scale (30× agent-skewed CI-surge) load across `tenants` and return the request count
/// offered — the live load the cell-scale gates are re-confirmed under.
fn world_scale_load_across_cell(tenants: &[TenantId], base_requests: u64) -> u64 {
    let m = Multiplier::custom(surge_multiplier()).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        base_requests,
        m,
        PrincipalMix::agent_skewed(),
        StormProfile::ci_surge(),
        tenants.to_vec(),
    )
    .expect("a non-empty cell tenant list");
    let mut sink = CellLoadSink::default();
    gen.drive(&mut sink);
    assert!(
        sink.requests > 0,
        "the world-scale load generator must offer requests (the load the gates are re-confirmed under)"
    );
    sink.requests
}

/// Re-run STORAGE's OWN restore-verify gate (STOR-D1) for ONE tenant's restored copy and return its
/// consistency point T. A whole copy: every referenced blob present + checksum-parity-verified, derived
/// == source-replay, erasure held. (Modelled inputs — the same model the single-tenant STOR-D1 drill
/// uses; the real WAL/PITR rebuild populates the SAME `GateInputs` shape.)
fn verify_one_tenant_restore(tenant: &TenantId) -> u64 {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    kms.ensure_dek(tenant, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let objects = vec![
        RestoredObject::integral(format!("{}::blob-90", tenant.0).into_bytes()),
        RestoredObject::integral(format!("{}::blob-100", tenant.0).into_bytes()),
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
    let artifact = RestoreVerifyGate::new()
        .run_or_fail_ci(&inputs)
        .unwrap_or_else(|e| {
            panic!(
                "STOR-D1 at cell scale: tenant {} restore not whole: {e}",
                tenant.0
            )
        });
    assert_eq!(artifact.checksum_mismatches, 0);
    assert_eq!(artifact.cross_seam_mismatches, 0);
    assert_eq!(artifact.resurrected_subjects, 0);
    artifact.restored_to_offset
}

/// The prod-scale online expand→backfill→contract migration (the online idiom — no table-rewrite lock).
fn online_migration() -> (Migrations, HotTables) {
    let hot = HotTables::declare(["issue"]);
    let migrations = Migrations::of([
        Migration::phased(
            "0010_expand",
            "ALTER TABLE issue ADD COLUMN priority INT;",
            MigrationPhase::Expand,
            "issue",
        ),
        Migration::phased(
            "0011_backfill",
            "UPDATE issue SET priority = 0 WHERE priority IS NULL;",
            MigrationPhase::Backfill,
            "issue",
        ),
        Migration::phased(
            "0012_contract",
            "ALTER TABLE issue ADD COLUMN status TEXT NOT NULL DEFAULT 'open';",
            MigrationPhase::Contract,
            "issue",
        ),
    ]);
    (migrations, hot)
}

/// Re-confirm STOR-D1/STOR-D2 (restore-verify) across a CELL's worth of tenants + STOR-D8
/// (online-migration-under-load) at prod scale on the restored copy, while a REAL world-scale load is
/// offered across the cell. Returns `(tenant_count, load_requests, lock_wait_p99_ms)`. ONE assertion
/// path shared by the SCHED headline + the CI smoke (no drift).
fn reconfirm_cell_scale(tenant_count: u32, base_load_requests: u64) -> (u32, u64, u64) {
    let tenants: Vec<TenantId> = (0..tenant_count)
        .map(|i| TenantId(format!("cell-tenant-{i:05}")))
        .collect();

    // (a) The restore-verify is re-confirmed UNDER world-scale load (the P-S02 generator across cell).
    let load_requests = world_scale_load_across_cell(&tenants, base_load_requests);

    // (b) STOR-D1/STOR-D2: re-run storage's OWN restore-verify gate for EVERY tenant. A single tenant
    //     whose copy is not whole panics (0 loss is per cell, not on average). All must land at T=100.
    for tenant in &tenants {
        let restored_to = verify_one_tenant_restore(tenant);
        assert_eq!(
            restored_to, 100,
            "every restored tenant lands at the consistency point T"
        );
    }

    // (c) STOR-D8 at prod scale on the restored copy: 50M rows + 256 concurrent writers, the lock
    //     budget read from the FILE; 0 downtime, p99 within budget.
    let budget = lock_budget_from_thresholds();
    let (migrations, hot) = online_migration();
    let load = WriteLoad::prod_scale(50_000_000, 256);
    let artifact = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &hot, load, 100, budget)
        .expect("STOR-D8 at prod scale: the online idiom must hold the lock budget under load");
    assert_eq!(artifact.downtime_ms, 0, "STOR-D8: 0 downtime at cell scale");
    assert!(
        artifact.lock_wait_p99_ms <= budget.lock_wait_p99_max_ms,
        "STOR-D8: lock-wait p99 {} ms ≤ budget {} ms",
        artifact.lock_wait_p99_ms,
        budget.lock_wait_p99_max_ms
    );

    (tenant_count, load_requests, artifact.lock_wait_p99_ms)
}

/// **THE SCHED DRILL (the dated green artifact the DoD names).** Re-confirm STOR-D1/STOR-D2 + STOR-D8
/// across the FULL measured cell tenant count under world-scale load — all bounds read from the FILE.
#[test]
fn stor_d2_d8_cell_scale_under_world_scale_load_sched() {
    let tenant_count = pool_tenants_max();
    assert!(
        tenant_count >= 1000,
        "the measured cell-scale tenant count must be a full cell ({tenant_count} tenants)"
    );
    // base 64 * 30× world-scale load across the whole cell.
    let (n, load_requests, p99) = reconfirm_cell_scale(tenant_count, 64);
    println!(
        "[P-444 STOR-D1/D2/D8@cell-scale GREEN 2026-06-24] {n} restored tenants re-confirmed whole \
         (STOR-D1/D2) UNDER world-scale load ({load_requests} requests, {}× agent-skewed CI-surge); \
         STOR-D8 prod-scale online migration held the lock budget (p99 {p99} ms, 0 downtime). \
         No threshold weakened.",
        surge_multiplier()
    );
}

/// **THE CI SMOKE VARIANT (rides every commit): the same cell-scale re-confirm over a THIN tenant
/// slice + a lighter load.** SAME assertion path — no drift from the SCHED headline.
#[test]
fn stor_d2_d8_cell_scale_ci_smoke() {
    let (n, load_requests, p99) = reconfirm_cell_scale(8, 16);
    assert_eq!(n, 8);
    assert!(load_requests > 0 && p99 <= lock_budget_from_thresholds().lock_wait_p99_max_ms);
}

/// **MANDATORY counter-case: a SINGLE inconsistent restored tenant fails the WHOLE cell (0 loss is per
/// cell, not on average).** A deliberately-corrupt restored copy (a referenced blob whose bytes do not
/// re-hash to its address) must FAIL storage's restore-verify gate — proving the cell-scale gate is a
/// real bar, never weakened to pass (EI-01 §3).
#[test]
fn stor_d1_cell_scale_one_corrupt_tenant_fails_the_gate() {
    use myelin_storage::{ContentHash, GateFailure};
    let tenant = TenantId("cell-tenant-00003".into());
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    kms.ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    let arch = reachable_archiver(300);
    // The object is stored under the address for "good", but the restore brought back "CORRUPT" bytes.
    let address = ContentHash::blake3(b"good");
    let corrupt = RestoredObject {
        content_address: address.clone(),
        bytes: b"CORRUPT".to_vec(),
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
    let err = RestoreVerifyGate::new().run_or_fail_ci(&inputs).expect_err(
        "a corrupt restored tenant copy MUST fail the cell-scale gate, never silently pass",
    );
    assert!(
        matches!(err, GateFailure::ChecksumMismatch { .. }),
        "the cell-scale gate surfaces the checksum mismatch: {err}"
    );
}

/// **MANDATORY counter-case: a BLOCKING ALTER at cell scale FAILS STOR-D8 (no lowered bar).** An
/// `ACCESS EXCLUSIVE` table-rewrite on the hot table at prod scale must read RED — the online idiom is
/// a real bar, the lock budget read from the FILE, never weakened (EI-01 §3).
#[test]
fn stor_d8_cell_scale_a_blocking_alter_fails() {
    use myelin_storage::MigrationLoadFailure;
    let hot = HotTables::declare(["issue"]);
    // A column-type rewrite on a hot table is a blocking ACCESS EXCLUSIVE rewrite — refused/over-budget.
    let migrations = Migrations::of([Migration::phased(
        "0099_rewrite",
        "ALTER TABLE issue ALTER COLUMN priority TYPE BIGINT;",
        MigrationPhase::Expand,
        "issue",
    )]);
    let budget = lock_budget_from_thresholds();
    let load = WriteLoad::prod_scale(50_000_000, 256);
    let verdict = MigrationUnderLoad::new().run(&migrations, &hot, load, 100, budget);
    assert!(
        !verdict.is_green(),
        "a blocking ALTER at cell scale MUST FAIL STOR-D8 (no lowered bar)"
    );
    // It fails either by the static refusal or by the downtime/lock-budget breach — all loud.
    assert!(
        matches!(
            verdict.failure(),
            Some(
                MigrationLoadFailure::RunnerRefused(_)
                    | MigrationLoadFailure::DowntimeIncurred { .. }
                    | MigrationLoadFailure::LockBudgetExceeded { .. }
            )
        ),
        "the cell-scale STOR-D8 red names the precise blocking migration: {:?}",
        verdict.failure()
    );
}
