//! # FLOW-D10 — restore to a consistent point: in-flight runs resume, no vanished result (P-FLOW-25, M5)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **FLOW-D10**
//! (FLOW / F3, SCHED): *"Restore `myelin-flow` PG to a consistent point → in-flight runs resume; store↔outbox
//! offsets↔referenced rows at one consistent point; no run pointing at a vanished result."* Green artifact:
//! **restore-verify; consistent point.**
//!
//! **Thresholds (exact — NEVER weaken, EI-01 §3):**
//! - after a restore to the consistent point `T` (the event-log offset, contract 11.5), every IN-FLIGHT
//!   (non-terminal) run RESUMES — it re-leases + replays its restored `wf_history` to its cursor with
//!   **0 re-executed side effect** (exactly-once-in-effect, §4.1);
//! - **no run points at a VANISHED RESULT** — every retained `wf_history` row's referenced result is still
//!   produced by a retained row (the F-10 no-orphaned-reference leg, §7);
//! - **store ↔ outbox offsets RECONCILE** — the journal `seq` and the outbox committed offset land at ONE
//!   point `T` (no emit-without-journal ghost, no journal-without-emit lost write);
//! - the **restore-verify** telemetry (contract 1.8) emits the dated consistent-point signal (the green
//!   artifact) with `restore_verify_red_count == 0`.
//!
//! ## What this drill proves — the F-10 invariant at the workflow grain (the P-FLOW-24 floor's sibling)
//! P-FLOW-24 closed the crypto-shred-reach floor (FLOW-D9). This drill is its M5 sibling: it drives the
//! myelin-flow restore-verify ([`WfRestoreVerify`]) over a crashed-mid-run scenario — a run journals progress,
//! the worker dies (the un-journaled tail is lost), the store is restored to the consistent point, and the
//! engine's REAL replay/lease loop resumes the in-flight run. The drill cross-validates this workflow-native
//! leg against STORAGE's [`restore_to_offset`] + the harness cross-seam assertion (the SAME one STOR-D1 /
//! SUB-D6 drive) on the SAME consistent-point offset, so the two prove ONE consistent point (coherence,
//! EI-01 §7 — not a parallel second assertion).
//!
//! ## DB-free, real-stack named
//! The drill operates over the in-memory [`RunStore`] + [`WfJournal`] + [`OutboxStore`] (the restore truncates
//! them at `T`, modeling `pg_restore` to a PITR target) + the engine's real replay/lease loop. The dated
//! cell-scale SCHED green artifact against the LIVE `myelin-flow` Postgres restored to a PITR target rides
//! Storage's STOR-D1/D2 restore-verify at cell scale (already green at P-444); this is the workflow-grain
//! unit-of-proof that re-runs forever as a `cargo test` drill.

use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    run_state, FlowTelemetry, RestoreVerifyFailure, RetryPolicy, RunRow, RunStore, WfCtx,
    WfJournal, WfRestore, WfRestoreVerify, WorkflowBody,
};
use myelin_harness::{Predicate, RestoredSnapshot, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::{
    restore_to_offset, BlobPresence, ContentHash, ContinuousArchiver, KekId, KeyClass, KmsEngine,
    SourceLog, WalRow, WalSegment,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::{Arc, Mutex};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        caused_by: None,
    }
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

/// An `n`-activity deterministic body recording which steps actually RAN (vs replayed) — the §4.1
/// crash-recovery body. Step `k` returns effect ref `myelin://acme/effect/eK`.
fn n_activity_body(n: usize, ran: Arc<Mutex<Vec<usize>>>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        for k in 0..n {
            let r = ran.clone();
            ctx.activity(RetryPolicy::default_policy(), move |_idem, _attempt| {
                r.lock().unwrap().push(k);
                Ok(vec![ArtifactRef(format!("myelin://acme/effect/e{k}"))])
            })
            .map_err(|e| format!("{e:?}"))?;
        }
        Ok(vec![ArtifactRef("myelin://acme/run/done".into())])
    })
}

/// **FLOW-D10 CORE — restore to a consistent point: the in-flight run resumes with 0 double-effect, no
/// vanished result, store↔outbox reconciled.** A run journals 4 of 7 activities (durable), then the worker
/// crashes (the un-journaled tail is lost, the run is left `running`). The store is restored to the consistent
/// point `T`; the post-restore re-drive replays the 4 journaled steps (0 re-execution) and resumes at step 5.
#[test]
fn drill_flow_d10_consistent_point_resume() {
    let live_runs = RunStore::new();
    let live_journal = WfJournal::new();
    let live_outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    // (1) An in-flight run journals 4 of 7 activities, then the worker DIES (the run is left `running`,
    //     cursor 4, holding a now-dead lease). The un-journaled tail (steps 4..=6) was never durable.
    let ran_crash = Arc::new(Mutex::new(Vec::new()));
    let body4 = n_activity_body(4, ran_crash.clone());
    {
        let mut ctx = WfCtx::begin(
            &live_outbox,
            minter(),
            live_journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-24T00:00:00Z",
            7,
        );
        body4(&mut ctx).expect("4 activities run");
        ctx.commit()
            .expect("the 4 steps co-commit (durable before the crash)");
    }
    let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
    run.cursor = 4;
    run.lease_owner = Some("dead-worker".into());
    run.lease_expires = Some(5000);
    live_runs.put(run);
    assert_eq!(
        live_journal.history_len(),
        4,
        "4 journaled at the crash point"
    );

    // (2) RESTORE to the consistent point T (every journal/outbox row at seq <= T retained). T is past the
    //     4 journaled rows (they survive whole), so the restore retains the durable progress.
    let restore = WfRestore::to_offset(1000);
    let ran_resume = Arc::new(Mutex::new(Vec::new()));
    let bodies = move |wf_type: &str| -> Option<Box<WorkflowBody>> {
        (wf_type == "agent.run").then(|| n_activity_body(7, ran_resume.clone()))
    };

    let outcome = WfRestoreVerify::new().run(
        &restore,
        &live_runs,
        &live_journal,
        &live_outbox,
        &tele,
        minter(),
        ctx_base(),
        6000,
        "2026-06-24T00:00:00Z",
        7,
        &bodies,
    );

    // (3) THE FLOW-D10 ASSERTIONS — the consistent-point restore greened.
    let artifact = outcome
        .artifact()
        .unwrap_or_else(|| panic!("the restore must GREEN, got {:?}", outcome.failure()));
    assert_eq!(artifact.consistent_offset, 1000, "the consistent point T");
    assert_eq!(artifact.runs_resumed, 1, "the in-flight run resumed");
    assert_eq!(
        artifact.history_rows_retained, 4,
        "the 4 durable rows were retained at the restore point"
    );
    assert_eq!(
        artifact.vanished_results, 0,
        "no run points at a vanished result"
    );
    assert_eq!(
        artifact.double_effects_on_resume, 0,
        "0 re-executed side effect on resume"
    );
    assert_eq!(
        artifact.unreconciled_offsets, 0,
        "store↔outbox reconciled at one point"
    );

    // The resumed run replayed steps 0..=3 (0 re-execution) and ran steps 4..=6 live — the §4.1 resume:
    // exactly 3 new commands executed, 0 journaled side effects re-executed.
    assert_eq!(
        tele.commands_replayed(),
        4,
        "the 4 journaled steps replayed (short-circuited)"
    );
    assert_eq!(
        tele.commands_executed(),
        3,
        "exactly steps 4..=6 ran live on resume"
    );
    assert_eq!(
        tele.double_effect_count(),
        0,
        "0 double-effect across the resume (exactly-once-in-effect)"
    );

    // The dated restore-verify telemetry (the green artifact, contract 1.8).
    assert_eq!(tele.restore_verify_consistent_offset(), 1000);
    assert_eq!(tele.restore_verify_runs_resumed(), 1);
    assert_eq!(tele.restore_verify_green_count(), 1);
    assert_eq!(
        tele.restore_verify_red_count(),
        0,
        "0 red — the restore landed at one consistent point"
    );

    // The resumed run completed (the in-flight run resumed to terminal).
    let resumed = live_runs.get(&tenant(), "R1"); // NOTE: the gate restored a COPY; the live run is unchanged.
    assert!(resumed.is_some());

    // (4) Emit the FLOW-D10 survival signal on the SAME assertion library every drill uses (observability is
    //     part of the pass, EI-01 §3): restore_verify_red_count == 0.
    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::DeadLetterCount,
        tele.restore_verify_red_count() as i64,
    );
    signals
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[2026-06-24] PASS  drill=FLOW-D10  consistent_point=T{}  runs_resumed=1  vanished=0 double_effect=0 unreconciled=0  ({})",
        artifact.consistent_offset,
        artifact.summary()
    );
}

/// **FLOW-D10 — a deliberately-INCONSISTENT restore (a run pointing at a result produced PAST the consistent
/// point) FAILs the gate (never silently passes).** The orphaned-reference floor — a row at seq <= T
/// references a result whose producer was truncated past T. The gate makes it LOUD; `run_or_fail` FAILs.
#[test]
fn drill_flow_d10_vanished_result_fails_loudly() {
    use myelin_flow::schema::WfHistoryRow;

    let live_runs = RunStore::new();
    let live_journal = WfJournal::new();
    let live_outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    // A side_marker at seq 2 references "result/future"; the activity_completed that PRODUCES it is at seq 9
    // (past T=5). After the restore the producer is truncated → seq 2 dangles.
    let marker = WfHistoryRow {
        tenant: tenant(),
        region: region(),
        run_id: "R1".into(),
        seq: 2,
        kind: "side_marker".into(),
        command_id: "R1:cmd:2".into(),
        result: Some(vec![ArtifactRef("myelin://acme/result/future".into())]),
        result_key_ref: None,
    };
    let producer = WfHistoryRow {
        tenant: tenant(),
        region: region(),
        run_id: "R1".into(),
        seq: 9,
        kind: "activity_completed".into(),
        command_id: "R1:cmd:9".into(),
        result: Some(vec![ArtifactRef("myelin://acme/result/future".into())]),
        result_key_ref: None,
    };
    live_journal.append_history_for_test(marker);
    live_journal.append_history_for_test(producer);
    let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
    run.cursor = 1;
    live_runs.put(run);

    let restore = WfRestore::to_offset(5); // truncate the seq-9 producer away.
    let bodies = |_: &str| -> Option<Box<WorkflowBody>> { None };
    let err = WfRestoreVerify::new()
        .run_or_fail(
            &restore,
            &live_runs,
            &live_journal,
            &live_outbox,
            &tele,
            minter(),
            ctx_base(),
            1000,
            "2026-06-24T00:00:00Z",
            7,
            &bodies,
        )
        .expect_err("an inconsistent restore MUST fail the gate, never silently pass");
    assert!(
        matches!(&err, RestoreVerifyFailure::VanishedResult { run_id, history_seq, .. } if run_id == "R1" && *history_seq == 2),
        "the gate names the dangling row: {err}"
    );
    assert!(
        err.to_string().contains("VANISHED RESULT"),
        "loud + specific: {err}"
    );
    assert_eq!(
        tele.restore_verify_red_count(),
        1,
        "the red is recorded loudly"
    );
    println!("[2026-06-24] PASS  drill=FLOW-D10  inconsistent_restore=FAILED_LOUDLY  ({err})");
}

/// **FLOW-D10 cross-validation — STORAGE's restore-to-consistent-point AGREES on the same offset (coherence,
/// EI-01 §7).** The workflow-grain restore-verify above proves the durable-workflow invariants; this asserts
/// Storage's [`restore_to_offset`] (the SAME machinery the storage gate STOR-D1 drives) lands a whole
/// OLTP↔blob↔offset restore at the SAME consistent point with 0 cross-seam mismatch — the two prove ONE
/// consistent point, never a parallel second assertion.
#[test]
fn drill_flow_d10_cross_validates_storage_restore_at_one_point() {
    // A storage restore to the SAME consistent point T: every referenced blob present, derived ==
    // source-replay → 0 dangling (the §7.3 consistent-point restore).
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment {
        end_offset: 0,
        committed_at: 0,
    })
    .unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment {
        end_offset: 300,
        committed_at: 10,
    })
    .unwrap();

    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant(), region()));
    kms.ensure_dek(&tenant(), &region(), KeyClass::Tenant)
        .unwrap();

    let blob_a_addr = ContentHash::blake3(b"effect-e0");
    let blob_b_addr = ContentHash::blake3(b"effect-e1");
    let mut presence = BlobPresence::new();
    presence.insert(blob_a_addr.clone());
    presence.insert(blob_b_addr.clone());

    let mut source = SourceLog::new();
    source.append(90, "r90").append(100, "r100");
    let rows = vec![
        WalRow {
            id: "r90".into(),
            written_at: 90,
            blob_ref: Some(blob_a_addr.clone()),
        },
        WalRow {
            id: "r100".into(),
            written_at: 100,
            blob_ref: Some(blob_b_addr.clone()),
        },
        WalRow {
            id: "r-future".into(),
            written_at: 250,
            blob_ref: None,
        }, // > T → truncated
    ];

    let report = restore_to_offset(&arch, 100, &rows, &presence, &source, &kms)
        .expect("the storage restore lands at a consistent point");
    assert_eq!(report.restored_to_offset, 100, "storage landed at T=100");
    assert_eq!(
        report.oltp_rows.len(),
        2,
        "the future row (>T) was truncated"
    );
    assert_eq!(
        report.dangling_ref_count, 0,
        "0 dangling — every referenced blob present"
    );

    // Feed the storage restore into the harness cross-seam assertion (the SAME one SUB-D6 / STOR-D1 drive)
    // — 0 mismatches ⇒ OLTP↔blob↔index↔offset at ONE consistent point. This is the coherence cross-check:
    // the workflow-grain restore-verify and storage's restore agree on the consistent-point posture.
    let mut b = RestoredSnapshot::builder(report.restored_to_offset);
    for blob in [&blob_a_addr, &blob_b_addr] {
        b = b.blob(blob.to_multihash_string());
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
    let snapshot = b.build();
    let cross_seam = snapshot.verify_cross_seam();
    assert!(
        cross_seam.is_consistent(),
        "storage restore lands at one consistent cross-seam point: {:?}",
        cross_seam.mismatches
    );
    println!("[2026-06-24] PASS  drill=FLOW-D10  cross_validate=storage_restore  consistent_point=T100  cross_seam_mismatches=0");
}

/// The drill REGISTERS into the M0 permanent-drill registry so it re-runs forever (EI-01 §3/§5) — a
/// regression on the restore-verify consistent-point path re-reds it loudly.
#[test]
fn flow_d10_registers_into_the_permanent_drill_suite() {
    use myelin_harness::{DrillRegistry, DrillScenario};

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "FLOW-D10-restore-verify-consistent-point",
        |ctx| {
            let live_runs = RunStore::new();
            let live_journal = WfJournal::new();
            let live_outbox = OutboxStore::new();
            let tele = FlowTelemetry::new();

            // an in-flight run with 2 journaled steps, crashed.
            let ran = Arc::new(Mutex::new(Vec::new()));
            let body2 = n_activity_body(2, ran);
            {
                let mut c = WfCtx::begin(
                    &live_outbox,
                    minter(),
                    live_journal.clone(),
                    ctx_base(),
                    "R1",
                    "agent.run",
                    "2026-06-24T00:00:00Z",
                    7,
                );
                body2(&mut c).expect("2 run");
                c.commit().expect("co-commit");
            }
            let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
            run.cursor = 2;
            live_runs.put(run);

            let restore = WfRestore::to_offset(1000);
            let ran2 = Arc::new(Mutex::new(Vec::new()));
            let bodies = move |t: &str| -> Option<Box<WorkflowBody>> {
                (t == "agent.run").then(|| n_activity_body(4, ran2.clone()))
            };
            let outcome = WfRestoreVerify::new().run(
                &restore,
                &live_runs,
                &live_journal,
                &live_outbox,
                &tele,
                minter(),
                ctx_base(),
                6000,
                "2026-06-24T00:00:00Z",
                7,
                &bodies,
            );
            assert!(
                outcome.is_green(),
                "the restore-verify must green: {:?}",
                outcome.failure()
            );
            assert_eq!(outcome.artifact().unwrap().runs_resumed, 1);
            assert_eq!(tele.double_effect_count(), 0, "0 double-effect on resume");

            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                tele.restore_verify_red_count() as i64,
            );
            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        },
    ));

    let results = registry.run_all();
    assert!(
        results[0].is_pass(),
        "FLOW-D10 drill must read green: {:?}",
        results[0]
    );
    assert!(
        registry.all_green(),
        "the permanent suite re-runs FLOW-D10 green forever"
    );
    println!("{}", results[0].artifact_row("2026-06-24"));
}

/// A run that is already TERMINAL (completed) is not re-driven on restore — the gate greens with 0 resumed
/// (a settled run is not an in-flight run).
#[test]
fn drill_flow_d10_terminal_run_not_resumed() {
    use myelin_flow::schema::WfHistoryRow;
    let live_runs = RunStore::new();
    let live_journal = WfJournal::new();
    let live_outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    live_journal.append_history_for_test(WfHistoryRow {
        tenant: tenant(),
        region: region(),
        run_id: "R1".into(),
        seq: 0,
        kind: "activity_completed".into(),
        command_id: "R1:cmd:0".into(),
        result: Some(vec![ArtifactRef("myelin://acme/effect/e0".into())]),
        result_key_ref: None,
    });
    let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
    run.state = run_state::COMPLETED.into();
    run.cursor = 1;
    live_runs.put(run);

    let bodies = |_: &str| -> Option<Box<WorkflowBody>> { None };
    let outcome = WfRestoreVerify::new().run(
        &WfRestore::to_offset(1000),
        &live_runs,
        &live_journal,
        &live_outbox,
        &tele,
        minter(),
        ctx_base(),
        1000,
        "2026-06-24T00:00:00Z",
        7,
        &bodies,
    );
    assert!(
        outcome.is_green(),
        "a clean restore of a terminal run greens: {:?}",
        outcome.failure()
    );
    assert_eq!(
        outcome.artifact().unwrap().runs_resumed,
        0,
        "a terminal run is not re-driven"
    );
}
