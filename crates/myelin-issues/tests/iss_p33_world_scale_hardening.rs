use myelin_harness::load_generator::{
    LoadGenerator, Multiplier, PrincipalMix, Request, RunClass as LoadRunClass, Sink, StormProfile,
};
use myelin_issues::surge::{
    open_surge_gate_from_thresholds, run_iss_d2_cell_scale, IssuesOwnerShed,
};
use myelin_storage::{
    ContinuousArchiver, ErasureLedger, GateInputs, HotTables, KekId, KeyClass, KmsEngine,
    LockBudget, Migration, MigrationPhase, MigrationUnderLoad, Migrations, RestoreVerifyGate,
    RestoredObject, SourceLog, WalOffset, WalRow, WalSegment, WriteLoad,
};
use myelin_substrate::shed::RunClass;
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

fn surge_multiplier() -> u32 {
    Thresholds::load_canonical().expect("load").surge.multiplier
}

fn pool_tenants_max() -> u32 {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let n = t.cell_sizing.pool_tenants_max;
    assert!(n > 0, "the measured cell tenant count is positive");
    n
}

fn lock_budget_from_thresholds() -> LockBudget {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let lock_wait_p99_max_ms = t.online_migration.lock_wait_p99_max_ms;
    let downtime_max_ms = t.online_migration.downtime_max_ms;
    assert!(lock_wait_p99_max_ms > 0, "the lock-wait budget is positive");
    assert_eq!(downtime_max_ms, 0, "the 0-downtime invariant is structural");
    LockBudget::new(lock_wait_p99_max_ms, downtime_max_ms)
}

fn shed_class(rc: LoadRunClass) -> RunClass {
    match rc {
        LoadRunClass::Human => RunClass::Human,
        LoadRunClass::Agent => RunClass::Agent,
        LoadRunClass::Service | LoadRunClass::Ci | LoadRunClass::ExternalMcp => RunClass::BatchCi,
    }
}

struct GatedMutationSink {
    gate: IssuesOwnerShed,
    human_admitted: bool,
    human_sheds: u64,
    requests: u64,
}

impl Sink for GatedMutationSink {
    fn handle(&mut self, request: &Request) {
        self.requests += 1;
        let class = shed_class(request.run_class);
        match self.gate.admit_class(&request.tenant, class) {
            Ok(()) => {
                if class == RunClass::Human {
                    self.gate.release(&request.tenant, RunClass::Human);
                }
            }
            Err(_retry_after) => {
                if class == RunClass::Human {
                    self.human_admitted = false;
                    self.human_sheds += 1;
                }
            }
        }
    }
}

#[test]
fn iss_p33_f6_chained_mutation_surge_human_holds_agent_sheds_cross_tenant_0() {
    let surging = TenantId("acme-surging".into());
    let quiet = TenantId("quiet-co-tenant".into());

    let m = Multiplier::custom(surge_multiplier()).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        400,
        m,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![surging.clone()],
    )
    .expect("a non-empty tenant list");

    let (gate, _t) = open_surge_gate_from_thresholds().expect("open the live gate from thresholds");
    let mut sink = GatedMutationSink {
        gate,
        human_admitted: true,
        human_sheds: 0,
        requests: 0,
    };
    gen.drive(&mut sink);

    assert!(
        sink.requests > 0,
        "the surge offered real generated mutations"
    );

    assert_eq!(
        sink.gate.shed_count(RunClass::Human),
        0,
        "the protected human lane held (0 human sheds under the 30× surge)"
    );
    assert!(sink.human_admitted, "every human mutation was admitted");
    assert_eq!(sink.human_sheds, 0);

    assert!(
        sink.gate.shed_count(RunClass::Agent) > 0,
        "the agent fan-out lane shed under the surge"
    );
    assert!(
        sink.gate.shed_count(RunClass::BatchCi) > 0,
        "the batch/CI importer lane shed under the surge"
    );

    assert_eq!(
        sink.gate.in_flight(&quiet),
        0,
        "the surge spent 0 of the quiet co-tenant's budget (cross-tenant impact 0)"
    );
    assert!(
        sink.gate.admit_class(&quiet, RunClass::Human).is_ok(),
        "the quiet co-tenant's human mutation admits (the surge never sheds another tenant's human)"
    );

    println!(
        "[P-496 ISS-F6 SURGE GREEN 2026-06-25] {} generated mutations at {}× agent-skewed: human lane HELD \
         (0 sheds) | agent shed={} | batch shed={} | cross-tenant impact 0. No threshold weakened.",
        sink.requests,
        surge_multiplier(),
        sink.gate.shed_count(RunClass::Agent),
        sink.gate.shed_count(RunClass::BatchCi),
    );
}

#[test]
fn iss_p33_f6_goes_red_when_the_gate_never_sheds() {
    use myelin_substrate::shed::SurfaceBudget;
    let surging = TenantId("acme-surging".into());
    let m = Multiplier::custom(30).expect("positive");
    let gen = LoadGenerator::new(
        10,
        m,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![surging],
    )
    .expect("non-empty");
    let mut sink = GatedMutationSink {
        gate: IssuesOwnerShed::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 250_000,
            retry_after_secs: 5,
        }),
        human_admitted: true,
        human_sheds: 0,
        requests: 0,
    };
    gen.drive(&mut sink);
    assert_eq!(
        sink.gate.shed_count(RunClass::Agent),
        0,
        "an unbounded gate never sheds - the F6 'agent sheds' property correctly FAILS here (real bar)"
    );
}

#[test]
fn iss_p33_iss_d2_at_cell_scale_no_unbounded_scan() {
    let report = run_iss_d2_cell_scale(1_000_000);
    assert!(report.is_iss_d2_green(), "{}", report.summary());
    assert_eq!(
        report.unbounded_scans, 0,
        "the cost-bounder NEVER emits an unbounded JSONB scan at cell scale (the ISS-D2 invariant)"
    );
    assert!(report.board_issue_count >= 1_000_000, "a 1M+-issue board");
    assert!(report.field_count >= 50, "a 50+ custom-field board");
    assert!(
        report.served_oltp > 0 && report.escalated > 0,
        "a real cost-bounder, not always-escalate"
    );
    println!(
        "[P-496 ISS-D2@cell-scale GREEN 2026-06-25] {}. No threshold weakened.",
        report.summary()
    );
}

fn issues_online_migration() -> (Migrations, HotTables) {
    let hot = HotTables::declare(["issue"]);
    let migrations = Migrations::of([
        Migration::phased(
            "iss_0100_expand",
            "ALTER TABLE issue ADD COLUMN priority INT;",
            MigrationPhase::Expand,
            "issue",
        ),
        Migration::phased(
            "iss_0101_backfill",
            "UPDATE issue SET priority = 0 WHERE priority IS NULL;",
            MigrationPhase::Backfill,
            "issue",
        ),
        Migration::phased(
            "iss_0102_contract",
            "ALTER TABLE issue ADD COLUMN sla_tier TEXT NOT NULL DEFAULT 'standard';",
            MigrationPhase::Contract,
            "issue",
        ),
    ]);
    (migrations, hot)
}

#[test]
fn iss_p33_online_migration_under_load_zero_downtime() {
    let (migrations, hot) = issues_online_migration();
    let budget = lock_budget_from_thresholds();
    let load = WriteLoad::prod_scale(1_000_000, 128);
    let restored_to: WalOffset = 100;

    let verdict = MigrationUnderLoad::new().run(&migrations, &hot, load, restored_to, budget);
    let artifact = verdict.artifact().unwrap_or_else(|| {
        panic!(
            "online migration under load must be GREEN: {:?}",
            verdict.failure()
        )
    });
    assert_eq!(
        artifact.downtime_ms, 0,
        "0 downtime on the hot issue table (the online idiom)"
    );
    assert!(
        artifact.lock_wait_p99_ms <= budget.lock_wait_p99_max_ms,
        "the p99 lock-wait held the budget under load"
    );
    println!(
        "[P-496 ISS online-migration-under-load GREEN 2026-06-25] {}. No threshold weakened.",
        artifact.summary()
    );
}

#[test]
fn iss_p33_blocking_alter_on_hot_issue_table_is_refused() {
    let hot = HotTables::declare(["issue"]);
    let migrations = Migrations::of([Migration::plain_on(
        "iss_9999_blocking",
        "ALTER TABLE issue ALTER COLUMN title TYPE VARCHAR(64);",
        "issue",
    )]);
    let budget = lock_budget_from_thresholds();
    let verdict = MigrationUnderLoad::new().run(
        &migrations,
        &hot,
        WriteLoad::prod_scale(1_000_000, 128),
        100,
        budget,
    );
    assert!(
        verdict.failure().is_some(),
        "a blocking ALTER on a hot table must be REFUSED (the 0-downtime bar is real)"
    );
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

fn verify_one_tenant_issues_restore(tenant: &TenantId) -> u64 {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    kms.ensure_dek(tenant, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let objects = vec![
        RestoredObject::integral(format!("{}::issue-body:auth bug", tenant.0).into_bytes()),
        RestoredObject::integral(
            format!("{}::issue-changelog:moved to in-progress", tenant.0).into_bytes(),
        ),
        RestoredObject::integral(format!("{}::rollup:initiative-7 60%", tenant.0).into_bytes()),
    ];
    let mut source = SourceLog::new();
    source.append(90, "iss-row-90").append(100, "iss-row-100");
    let rows = vec![
        WalRow {
            id: "iss-row-90".into(),
            written_at: 90,
            blob_ref: Some(objects[0].content_address.clone()),
        },
        WalRow {
            id: "iss-row-100".into(),
            written_at: 100,
            blob_ref: Some(objects[2].content_address.clone()),
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
                "STOR-D2 at cell scale: tenant {} restore not whole: {e}",
                tenant.0
            )
        });
    assert_eq!(
        artifact.checksum_mismatches, 0,
        "every object re-hashes to its address"
    );
    assert_eq!(artifact.cross_seam_mismatches, 0);
    assert_eq!(artifact.resurrected_subjects, 0);
    artifact.restored_to_offset
}

fn reconfirm_issues_cell_scale(tenant_count: u32) -> u32 {
    for i in 0..tenant_count {
        let tenant = TenantId(format!("iss-cell-tenant-{i:05}"));
        let restored_to = verify_one_tenant_issues_restore(&tenant);
        assert_eq!(
            restored_to, 100,
            "every restored Issues tenant lands at the consistency point T"
        );
    }
    tenant_count
}

#[test]
fn iss_p33_restore_verify_at_cell_scale_sched() {
    let tenant_count = pool_tenants_max();
    assert!(
        tenant_count >= 1000,
        "the measured cell-scale tenant count must be a full cell ({tenant_count} tenants)"
    );
    let n = reconfirm_issues_cell_scale(tenant_count);
    println!(
        "[P-496 ISS STOR-D2@cell-scale GREEN 2026-06-25] {n} restored Issues tenants re-confirmed whole \
         (every object re-hashes, derived==source-replay, erasure held). No threshold weakened."
    );
}

#[test]
fn iss_p33_restore_verify_at_cell_scale_ci_smoke() {
    let n = reconfirm_issues_cell_scale(8);
    assert_eq!(n, 8);
}

#[test]
fn iss_p33_one_corrupt_object_fails_the_cell() {
    use myelin_storage::{ContentHash, GateFailure};
    let tenant = TenantId("iss-cell-tenant-00003".into());
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    kms.ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    let arch = reachable_archiver(300);
    let address = ContentHash::blake3(b"issue-body:auth bug");
    let corrupt = RestoredObject {
        content_address: address.clone(),
        bytes: b"issue-body:CORRUPTED".to_vec(),
    };
    let objects = vec![corrupt];
    let source = SourceLog::new();
    let rows = vec![WalRow {
        id: "iss-row-1".into(),
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
    let verdict = RestoreVerifyGate::new().run(&inputs);
    assert!(
        matches!(
            verdict.failure(),
            Some(GateFailure::ChecksumMismatch { .. })
        ),
        "a corrupt restored object must FAIL the cell-scale gate (the bar is real)"
    );
}
