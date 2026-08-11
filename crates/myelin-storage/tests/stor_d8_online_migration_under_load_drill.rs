use std::path::Path;

use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::migration::{HotTables, Migration, MigrationPhase, Migrations};
use myelin_storage::{
    ContinuousArchiver, ErasureLedger, GateInputs, KekId, KeyClass, KmsEngine, LockBudget,
    MigrationUnderLoad, RestoreVerifyGate, RestoredObject, SourceLog, WalRow, WalSegment,
    WriteLoad,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("eu-west".into())
}
fn tenant(s: &str) -> TenantId {
    TenantId(s.into())
}

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
    assert!(
        lock_wait_p99_max_ms > 0,
        "the lock-wait budget must be a positive duration"
    );
    assert_eq!(
        downtime_max_ms, 0,
        "the 0-downtime invariant is structural (STOR-D8)"
    );
    LockBudget::new(lock_wait_p99_max_ms as u64, downtime_max_ms as u64)
}

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

fn kms_with_tenant(t: &TenantId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(t, &region(), KeyClass::Tenant).unwrap();
    kms
}

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

#[test]
fn stor_d8_online_migration_on_restored_copy_holds_lock_budget_under_load() {
    let mut signals = SignalSource::new();

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
    assert_eq!(
        restored_to, 100,
        "the restored copy lands at the consistency point T"
    );
    signals.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        artifact.cross_seam_mismatches as i64,
    );
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();

    let budget = lock_budget_from_thresholds();
    let (migrations, hot) = online_migration();
    let load = WriteLoad::prod_scale(50_000_000, 256);

    let drill = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &hot, load, restored_to, budget)
        .expect("STOR-D8: the online idiom must hold the lock budget under prod-scale load");

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
    let summary = drill.summary();
    assert!(
        summary.contains("STOR-D8 PASS"),
        "dated green artifact: {summary}"
    );
    println!("[P-126 STOR-D8 GREEN] {summary}");

    let rpo_bound_secs = rpo_max_secs_from_thresholds();
    arch.record_commit(400, 320);
    arch.archive_segment(WalSegment {
        end_offset: 400,
        committed_at: 350,
    })
    .unwrap();
    let rpo_secs = arch.measure_rpo();
    signals.set_scalar(SignalName::RestoreRpoSecs, rpo_secs as i64);
    signals
        .assert_signal(
            SignalName::RestoreRpoSecs,
            Predicate::Lte(rpo_bound_secs as i64),
        )
        .expect_green();
}

#[test]
fn stor_d8_a_blocking_alter_at_prod_scale_fails_the_drill() {
    let budget = lock_budget_from_thresholds();
    let migrations = Migrations::of([Migration::phased(
        "0010_blocking",
        "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;",
        MigrationPhase::Expand,
        "issue",
    )]);
    let load = WriteLoad::prod_scale(50_000_000, 256);

    let err = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &HotTables::none(), load, 100, budget)
        .expect_err(
            "STOR-D8: a blocking ALTER at prod scale MUST fail the drill, never pass silently",
        );
    assert!(
        err.to_string().contains("STOR-D8 FAIL"),
        "loud + specific: {err}"
    );
}
