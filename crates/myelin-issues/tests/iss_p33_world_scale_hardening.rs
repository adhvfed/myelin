//! # ISS-P33 → global P-496 (M5): Issues world-scale hardening — the F6 surge family + scale benchmarks
//!
//! **Prompt:** ISS-P33 → global **P-496** (M5). **Drill catalogue:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` — the **F6 surge family** (SUB-D3-shaped:
//! the protected human lane holds, the agent lane sheds `429 + Retry-After`, cross-tenant impact 0) +
//! **ISS-D2 at cell scale** (the 1M+-issue board under the `<1s` keyboard budget under world-scale load) +
//! **online-migration-under-load** (STOR-D8, 0 downtime on the hot issue tables) + **restore-verify at
//! cell scale** (STOR-D2). **Contracts:** 1.11 (the per-surface shed budgets the surge stresses), 11.6
//! (the OLAP at cell scale), 3.5 (the firehose under surge — re-confirmed via the chained-mutation storm).
//! **Doctrine:** EI-01 §3 (the 30× surge with quantified thresholds, the multiplier read from the FROZEN
//! file, never hardcoded; never weaken a threshold to pass), §7 (REUSE — re-drive the substrate/storage
//! OWN gates, never a second copy).
//!
//! ## What this drill IS (coherence, EI-01 §7)
//! It composes Issues' world-scale hardening over engines the prior prompts already shipped:
//! - the **F6 surge** drives the REAL harness `LoadGenerator` at the 30× agent-skewed mix (the M1
//!   1×/10×/30× generator) THROUGH the LIVE `IssuesOwnerShed` gate (the substrate `ShedLane` over the
//!   `HttpIntake` mutation-intake surface, budget read from `thresholds.toml`); it asserts the three F6
//!   properties on the gate's per-lane shed signals — never a second shed order;
//! - the **ISS-D2-at-cell-scale** re-confirm drives the LIVE cost-bounder (`plan_board_query`) over a
//!   1M+-issue board × 50+ custom fields and asserts NO unbounded JSONB scan — never a second cost engine;
//! - **online-migration-under-load** re-drives STORAGE's OWN `MigrationUnderLoad` gate over an
//!   expand→backfill→contract migration on Issues' hot `issue` table; **restore-verify at cell scale**
//!   re-drives STORAGE's OWN `RestoreVerifyGate` over a cell's worth of Issues tenants' restorable state.
//!
//! ## Floors named
//! - **No new floor** (this prompt hardens; the floor follow-ons were promoted in ISS-P32 / P-495). The
//!   surge RE-RUNS the F1 leak-free family (the cost-bounder's ACL pre-filter still gates every admitted
//!   query) + the reorder-0-clobber family UNDER load.
//! - **The world-scale 30× run on real FLEET hardware** (a real multi-node cell) is the ONE legitimate
//!   remaining floor; here the world-scale load is the harness generator at 30× across the cell's tenants.
//! - **The real WAL/PITR rebuild + live PG ALTER at the full cell count** is the storage-tier floor
//!   (P-059..P-061 / P-444); here the restorable state is MODELLED with the SAME `GateInputs` shape the
//!   storage drills use, and the migration lock-cost is MODELLED by `lock_cost_ms` — the wiring +
//!   assertions do not change when the real drivers land.

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

/// The surge multiplier read from the FROZEN `[surge]` (the world-scale mutation-load multiplier).
fn surge_multiplier() -> u32 {
    Thresholds::load_canonical().expect("load").surge.multiplier
}

/// The MEASURED cell-scale tenant count (`cell_sizing.pool_tenants_max`) — never a literal (EI-01 §3).
/// Read via the TYPED `Thresholds` loader (the single source of truth; a missing field is a LOUD error).
fn pool_tenants_max() -> u32 {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let n = t.cell_sizing.pool_tenants_max;
    assert!(n > 0, "the measured cell tenant count is positive");
    n
}

/// The STOR-D8 lock budget read from the versioned `thresholds.toml` (the single source of truth) via the
/// TYPED loader. A future field rename is caught here too. NEVER hardcoded.
fn lock_budget_from_thresholds() -> LockBudget {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let lock_wait_p99_max_ms = t.online_migration.lock_wait_p99_max_ms;
    let downtime_max_ms = t.online_migration.downtime_max_ms;
    assert!(lock_wait_p99_max_ms > 0, "the lock-wait budget is positive");
    assert_eq!(downtime_max_ms, 0, "the 0-downtime invariant is structural");
    LockBudget::new(lock_wait_p99_max_ms, downtime_max_ms)
}

// ───────────────────────────── (1) the F6 chained-mutation surge e2e ──────────────────────────────────

/// Map the harness load-generator's five-kind run-class onto the substrate shed lane's four classes.
/// A `Service`/`Ci`/`ExternalMcp` machine client is the batch/CI lane; an `Agent` the agent lane; a
/// `Human` the protected lane (the human lane is structurally unspoofable — only a real human kind reaches
/// it). This is the §7.2 projection, not a second classifier.
fn shed_class(rc: LoadRunClass) -> RunClass {
    match rc {
        LoadRunClass::Human => RunClass::Human,
        LoadRunClass::Agent => RunClass::Agent,
        LoadRunClass::Service | LoadRunClass::Ci | LoadRunClass::ExternalMcp => RunClass::BatchCi,
    }
}

/// A sink that feeds every generated mutation THROUGH the live Issues owner shed gate. The machine lanes
/// keep their slot (the storm is sustained — it pressures the cap and sheds); the human lane releases
/// each slot (a short interactive mutation), so the protected lane holds across the WHOLE surge.
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
                    // a short interactive mutation returns its slot immediately (so a LATER human admits).
                    self.gate.release(&request.tenant, RunClass::Human);
                }
                // machine lanes KEEP their slot — the storm is sustained, pressuring the cap.
            }
            Err(_retry_after) => {
                if class == RunClass::Human {
                    self.human_admitted = false;
                    self.human_sheds += 1;
                }
                // a shed machine request backed off with 429 + Retry-After (counted on the gate's lane).
            }
        }
    }
}

/// **THE F6 CHAINED-MUTATION SURGE E2E (the dated green artifact the DoD names).** Drive the REAL harness
/// generator at the 30× agent-skewed mix THROUGH the live Issues owner gate; assert the three F6
/// properties: the human lane HELD (0 human sheds), both machine lanes SHED (`429 + Retry-After`),
/// cross-tenant impact 0 (a quiet co-tenant's human still admits within its independent budget).
#[test]
fn iss_p33_f6_chained_mutation_surge_human_holds_agent_sheds_cross_tenant_0() {
    let surging = TenantId("acme-surging".into());
    let quiet = TenantId("quiet-co-tenant".into());

    // The 30× agent-skewed mutation storm on the SURGING tenant (the F6 mix read from the file).
    let m = Multiplier::custom(surge_multiplier()).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        400, // base mutations/multiplier-unit: ×30 well past the HttpIntake per-tenant cap → the lanes shed.
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

    // (1) the human mutation lane HELD — 0 human sheds across the whole 30× storm.
    assert_eq!(
        sink.gate.shed_count(RunClass::Human),
        0,
        "the protected human lane held (0 human sheds under the 30× surge)"
    );
    assert!(sink.human_admitted, "every human mutation was admitted");
    assert_eq!(sink.human_sheds, 0);

    // (2) the agent + batch/CI machine lanes SHED (429 + Retry-After, shed-count > 0).
    assert!(
        sink.gate.shed_count(RunClass::Agent) > 0,
        "the agent fan-out lane shed under the surge"
    );
    assert!(
        sink.gate.shed_count(RunClass::BatchCi) > 0,
        "the batch/CI importer lane shed under the surge"
    );

    // (3) cross-tenant impact 0 — a quiet co-tenant's human mutation admits within ITS independent budget,
    //     and the storm spent 0 of the quiet tenant's in-flight (the per-tenant bulkhead).
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

/// **MANDATORY inversion guard (EI-01 §3): the F6 green is a REAL property.** An unbounded gate (a huge
/// budget) NEVER sheds the machine lanes → the F6 property must FAIL (not vacuously pass). This proves the
/// surge assertion can go red.
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
        "an unbounded gate never sheds — the F6 'agent sheds' property correctly FAILS here (real bar)"
    );
}

// ───────────────────────────── (2) ISS-D2 at cell scale (1M+ board under <1s) ─────────────────────────

/// **ISS-D2 re-confirmed at CELL SCALE (the dated green artifact).** The 1M+-issue board (50+ custom
/// fields) under the `<1s` keyboard budget: the live cost-bounder NEVER emits an unbounded JSONB scan
/// across the full field × cell-scale-fan-out sweep (contract 11.6 the OLAP at cell scale).
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

// ───────────────────────────── (3) online-migration-under-load on the hot issue tables ────────────────

/// An expand→backfill→contract migration on Issues' hot `issue` table — the online idiom (no
/// `ACCESS EXCLUSIVE` table-rewrite lock). The SAME shape the storage STOR-D8 drill uses, scoped here to
/// Issues' declared-hot table (`issues_hot_tables` flags `issue`).
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

/// **online-migration-under-load on the hot issue tables (STOR-D8, 0 downtime).** Re-drive STORAGE's OWN
/// `MigrationUnderLoad` gate over the expand→backfill→contract migration on Issues' hot `issue` table at
/// prod scale (1M+ rows under concurrent writers): the p99 lock-wait holds the budget AND total downtime
/// is 0 (an online migration NEVER takes the table offline).
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

/// **MANDATORY counter-case: a BLOCKING ALTER on the hot issue table is REFUSED (0 downtime is real).** A
/// destructive/blocking migration on a declared-hot table must FAIL the gate — proving the 0-downtime bar
/// is never weakened (EI-01 §3).
#[test]
fn iss_p33_blocking_alter_on_hot_issue_table_is_refused() {
    let hot = HotTables::declare(["issue"]);
    // a table-rewrite ALTER (NOT NULL without a default + a type change) on a hot table — refused.
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

// ───────────────────────────── (4) restore-verify at cell scale (STOR-D2) ─────────────────────────────

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

/// Re-run STORAGE's OWN restore-verify gate (STOR-D2) for ONE tenant's restored ISSUES state and return
/// its consistency point T. A whole restore: every authoritative content-addressed object present +
/// checksum-parity-verified, the derived projection == source-replay, erasure held. Modelled objects —
/// the SAME `GateInputs` shape the storage STOR-D2 drill uses.
fn verify_one_tenant_issues_restore(tenant: &TenantId) -> u64 {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    kms.ensure_dek(tenant, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    // a tenant's authoritative Issues state — an issue body, its change-log entry, its rollup snapshot
    // (the content-addressed bytes the object tier holds). The restore brought them back whole.
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

/// Re-confirm STOR-D2 across a CELL's worth of Issues tenants. A single tenant whose restore is not whole
/// fails the whole cell (0 loss is per cell, not on average). ONE assertion path shared by SCHED + smoke.
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

/// **THE STOR-D2-at-cell-scale SCHED DRILL (the dated green artifact the DoD names).** Re-confirm STOR-D2
/// over Issues' restorable state across the FULL measured cell tenant count (bounds read from the FILE).
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

/// **THE CI SMOKE VARIANT (rides every commit): the same cell-scale re-confirm over a THIN slice.** SAME
/// assertion path — no drift from the SCHED headline.
#[test]
fn iss_p33_restore_verify_at_cell_scale_ci_smoke() {
    let n = reconfirm_issues_cell_scale(8);
    assert_eq!(n, 8);
}

/// **MANDATORY counter-case: a SINGLE corrupt restored object fails the WHOLE cell (0 loss is per cell).**
/// An object whose restored bytes do not re-hash to its address must FAIL the gate — proving the
/// cell-scale gate is a real bar, never weakened (EI-01 §3).
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
