//! P-ST-21 (global P-126) GATE / DRILL — **STOR-D8: online-migration safety on the restored
//! prod-scale copy under load** — dated green artifact.
//!
//! **STOR-D8 (storage.md §3.1 / testing-strategy §4.2 row STOR-D8):** *expand→backfill→contract on a
//! restored prod-scale copy under load → no blocking lock beyond budget; 0 downtime. Telemetry:
//! `lock-wait p99`; 0 downtime.* Forward-only online migrations have their lock time **measured against
//! a RESTORED copy** — now that restore-verify (STOR-D1) exists to produce that copy (§7.4: the gate
//! "produces the production-scale restored copy that online migrations rehearse lock-time against").
//!
//! This drill exercises the WHOLE chain the prompt names:
//! 1. **Produce + verify the restored prod-scale copy** — drive the real
//!    [`RestoreVerifyGate`] (P-061) to restore a prod-scale store to a consistent point T and assert it
//!    is WHOLE (STOR-D1 GREEN). This re-runs the permanent gate over the store the migration touches.
//! 2. **STOR-D8: migrate it under load** — run an expand→backfill→contract migration over that restored
//!    copy with a prod-scale write load in flight ([`MigrationUnderLoad`]), asserting the **lock-wait
//!    p99 stays within budget** and **downtime is 0**. The blocking-ALTER counter-case is proven to
//!    BLOW the budget (the dynamic proof the static admission gate cannot make).
//! 3. **STOR-D2 remains green** — re-run the RPO measure over the same archiver (the permanent gate
//!    ratchets — master §4: every store-touching change re-runs restore-verify).
//!
//! ## The threshold is READ, never hardcoded (EI-01 §3)
//! The lock budget comes from the versioned `thresholds.toml` (`[online_migration]
//! lock_wait_p99_max_ms` + `downtime_max_ms`, the single source of truth, P-038) — NOT a magic number
//! in the test. Weakening it to pass is forbidden; a red is a dated `[[claimed_not_proven]]` scorecard
//! row. The RPO bound comes from `[rpo_rto] rpo_max_mins` (the same discipline as the STOR-D2 drill).
//!
//! ## Scope (named, EI-01 §4)
//! M2 prod-scale online-migration-under-load against the modeled restored copy + the modeled
//! Postgres lock-cost model (the real `pg_restore` restored copy + a real concurrent write workload are
//! the P-S12/P-S15/P-ST-30 drivers; the model becomes the measured `pg_locks` wait when they land). The
//! **cell-scale re-confirm** of this lock budget under WORLD-SCALE load is the named follow-on
//! **P-ST-34 (M5)**. STOR-D1/STOR-D2 are re-run here (they must REMAIN green across this store-touching
//! change).

use std::path::Path;

use myelin_harness::{Predicate, SignalName, SignalSource};
// `mut` archiver below records a commit + archives it (the STOR-D2 RPO re-run).
use myelin_storage::{
    ContinuousArchiver, ErasureLedger, GateInputs, KekId, KeyClass, KmsEngine, LockBudget,
    MigrationUnderLoad, RestoreVerifyGate, RestoredObject, SourceLog, WalRow, WalSegment, WriteLoad,
};
use myelin_storage::migration::{HotTables, Migration, MigrationPhase, Migrations};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("eu-west".into())
}
fn tenant(s: &str) -> TenantId {
    TenantId(s.into())
}

/// The workspace-root `thresholds.toml` path (two levels above the crate manifest).
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

/// **Read the STOR-D8 lock budget from the versioned `thresholds.toml`** (the single source of truth,
/// P-038). A missing threshold is a LOUD failure — never a silent default (EI-01 §3). NEVER hardcoded.
fn lock_budget_from_thresholds() -> LockBudget {
    let doc = thresholds_doc();
    let section = doc
        .get("online_migration")
        .expect("online_migration section must be present (a missing threshold is a LOUD error)");
    let lock_wait_p99_max_ms = section
        .get("lock_wait_p99_max_ms")
        .and_then(|v| v.as_integer())
        .expect("online_migration.lock_wait_p99_max_ms must be present");
    let downtime_max_ms = section
        .get("downtime_max_ms")
        .and_then(|v| v.as_integer())
        .expect("online_migration.downtime_max_ms must be present");
    assert!(lock_wait_p99_max_ms > 0, "the lock-wait budget must be a positive duration");
    assert_eq!(downtime_max_ms, 0, "the 0-downtime invariant is structural (STOR-D8)");
    LockBudget::new(lock_wait_p99_max_ms as u64, downtime_max_ms as u64)
}

/// Read `[rpo_rto] rpo_max_mins` (seconds) — the STOR-D2 re-run bound.
fn rpo_max_secs_from_thresholds() -> u64 {
    let doc = thresholds_doc();
    let mins = doc
        .get("rpo_rto")
        .and_then(|t| t.get("rpo_max_mins"))
        .and_then(|v| v.as_integer())
        .expect("rpo_rto.rpo_max_mins must be present");
    assert!(mins > 0, "the RPO bound must be a positive duration");
    (mins as u64) * 60
}

/// Backups covering offsets `0..=tail` (a base at 0 + the WAL tail archived to `tail`).
fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment { end_offset: 0, committed_at: 0 }).unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment { end_offset: tail, committed_at: 10 }).unwrap();
    arch
}

/// A KMS engine with a live tenant whose KEK + DEK exist (so the restored copy brings back a key).
fn kms_with_tenant(t: &TenantId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
    kms.ensure_dek(t, &region(), KeyClass::Tenant).unwrap();
    kms
}

/// The prod-scale expand→backfill→contract migration on the hot `issue` table — the online idiom: a
/// nullable expand (catalog metadata lock), a throttled off-hot-path backfill (row locks), a validated
/// contract (catalog metadata lock). None takes an `ACCESS EXCLUSIVE` table-rewrite lock.
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

/// **STOR-D8 (the headline): expand→backfill→contract on a restored prod-scale copy under load holds
/// the lock budget with 0 downtime — dated green artifact. STOR-D1/STOR-D2 re-run green.**
#[test]
fn stor_d8_online_migration_on_restored_copy_holds_lock_budget_under_load() {
    let mut signals = SignalSource::new();

    // ── (1) Produce + VERIFY the restored prod-scale copy (STOR-D1 GREEN re-run) ──
    let t = tenant("acme");
    let kms = kms_with_tenant(&t);
    let mut arch = reachable_archiver(300);
    let objects = vec![
        RestoredObject::integral(b"blob-90".to_vec()),
        RestoredObject::integral(b"blob-100".to_vec()),
    ];
    let mut source = SourceLog::new();
    source.append(90, "r90").append(100, "r100");
    let rows = vec![
        WalRow { id: "r90".into(), written_at: 90, blob_ref: Some(objects[0].content_address.clone()) },
        WalRow { id: "r100".into(), written_at: 100, blob_ref: Some(objects[1].content_address.clone()) },
        WalRow { id: "r-future".into(), written_at: 250, blob_ref: None }, // > T → dropped
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
        .expect("STOR-D1: the restored prod-scale copy must be WHOLE (the permanent gate re-runs)");
    let restored_to = artifact.restored_to_offset;
    assert_eq!(restored_to, 100, "the restored copy lands at the consistency point T");
    // The cross-seam zero the permanent gate asserts (observability is part of the pass).
    signals.set_scalar(SignalName::RestoreCrossSeamMismatch, artifact.cross_seam_mismatches as i64);
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green(); // STOR-D1: the restored copy is at ONE consistent cross-seam point.

    // ── (2) STOR-D8: migrate the restored copy UNDER LOAD; lock-wait p99 ≤ budget; 0 downtime ──
    let budget = lock_budget_from_thresholds(); // READ from thresholds.toml, never hardcoded.
    let (migrations, hot) = online_migration();
    // Prod-scale load: 50M live rows on the hot table + 256 concurrent writers — a table-rewrite lock
    // here would stall every writer; the online idiom does not take one.
    let load = WriteLoad::prod_scale(50_000_000, 256);

    let drill = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &hot, load, restored_to, budget)
        .expect("STOR-D8: the online idiom must hold the lock budget under prod-scale load");

    // The 0-downtime invariant + the lock-wait p99 within budget (the STOR-D8 telemetry).
    assert_eq!(drill.downtime_ms, 0, "STOR-D8: 0 downtime");
    assert!(
        drill.lock_wait_p99_ms <= budget.lock_wait_p99_max_ms,
        "STOR-D8: lock-wait p99 {} ms ≤ budget {} ms",
        drill.lock_wait_p99_ms,
        budget.lock_wait_p99_max_ms
    );
    assert_eq!(drill.steps.len(), 3, "expand→backfill→contract = 3 steps");
    assert!(
        drill.steps.iter().all(|s| !s.lock_class.blocks_writers()),
        "STOR-D8: no step took an ACCESS EXCLUSIVE table-rewrite lock"
    );
    // The dated green-artifact line (observability is part of the pass).
    let summary = drill.summary();
    assert!(summary.contains("STOR-D8 PASS"), "dated green artifact: {summary}");
    println!("[P-126 STOR-D8 GREEN] {summary}");

    // ── (3) STOR-D2 remains green: the RPO measure over the same archiver ──
    // A new write commits past the tail and the archiver ships the covering segment (continuous
    // archiving stays caught up) — the RPO is the bounded commit↔archive freshness gap.
    let rpo_bound_secs = rpo_max_secs_from_thresholds();
    arch.record_commit(400, 320); // a write committed at offset 400, t=320s.
    arch.archive_segment(WalSegment { end_offset: 400, committed_at: 350 }).unwrap(); // shipped at t=350s.
    let rpo_secs = arch.measure_rpo(); // caught up (archived ≥ committed) → RPO 0, within bound.
    signals.set_scalar(SignalName::RestoreRpoSecs, rpo_secs as i64);
    signals
        .assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(rpo_bound_secs as i64))
        .expect_green(); // STOR-D2: RPO remains within bound across this store-touching change.
}

/// **MANDATORY-CORE: a BLOCKING ALTER at prod scale BLOWS the budget — the drill FAILs (never silently
/// passes).** The dynamic counter-case the static admission gate cannot make: a table left non-hot lets
/// the static runner admit the blocking ALTER, and the UNDER-LOAD lock budget catches it. Proves the
/// budget is a real bar, not a rubber stamp (weakening it to pass is forbidden, EI-01 §3).
#[test]
fn stor_d8_a_blocking_alter_at_prod_scale_fails_the_drill() {
    let budget = lock_budget_from_thresholds();
    // A blocking ADD COLUMN … NOT NULL with no DEFAULT → ACCESS EXCLUSIVE rewrite. Table NOT declared
    // hot so the static runner admits it; the DYNAMIC budget must catch it.
    let migrations = Migrations::of([Migration::phased(
        "0010_blocking",
        "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;",
        MigrationPhase::Expand,
        "issue",
    )]);
    let load = WriteLoad::prod_scale(50_000_000, 256);

    let err = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &HotTables::none(), load, 100, budget)
        .expect_err("STOR-D8: a blocking ALTER at prod scale MUST fail the drill, never pass silently");
    assert!(err.to_string().contains("STOR-D8 FAIL"), "loud + specific: {err}");
}
