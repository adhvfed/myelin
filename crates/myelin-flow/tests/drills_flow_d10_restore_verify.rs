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

#[test]
fn drill_flow_d10_consistent_point_resume() {
    let live_runs = RunStore::new();
    let live_journal = WfJournal::new();
    let live_outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

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

    assert_eq!(tele.restore_verify_consistent_offset(), 1000);
    assert_eq!(tele.restore_verify_runs_resumed(), 1);
    assert_eq!(tele.restore_verify_green_count(), 1);
    assert_eq!(
        tele.restore_verify_red_count(),
        0,
        "0 red - the restore landed at one consistent point"
    );

    let resumed = live_runs.get(&tenant(), "R1");
    assert!(resumed.is_some());

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

#[test]
fn drill_flow_d10_vanished_result_fails_loudly() {
    use myelin_flow::schema::WfHistoryRow;

    let live_runs = RunStore::new();
    let live_journal = WfJournal::new();
    let live_outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

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

    let restore = WfRestore::to_offset(5);
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

#[test]
fn drill_flow_d10_cross_validates_storage_restore_at_one_point() {
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
    kms.ensure_kek(&KekId::new(tenant(), region()))
        .expect("seed the in-memory KEK");
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
        },
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
        "0 dangling - every referenced blob present"
    );

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
