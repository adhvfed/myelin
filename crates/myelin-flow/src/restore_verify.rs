use myelin_events::{EmitContextBase, IdMinter, OutboxStore};
use myelin_refs::ArtifactRef;

use crate::engine::{drive, run_state, FlowTelemetry, RunRow, RunStore, WorkflowBody};
use crate::wfctx::WfJournal;

pub type ConsistentOffset = i64;

#[derive(Clone)]
pub struct WfRestore {
    to_offset: ConsistentOffset,
}

impl WfRestore {
    pub fn to_offset(to_offset: ConsistentOffset) -> WfRestore {
        WfRestore { to_offset }
    }

    pub fn consistent_offset(&self) -> ConsistentOffset {
        self.to_offset
    }

    pub fn apply(
        &self,
        live_runs: &RunStore,
        live_journal: &WfJournal,
        live_outbox: &OutboxStore,
    ) -> RestoredFlow {
        let t = self.to_offset;

        let restored_journal = WfJournal::new();
        let mut retained_rows = 0usize;
        for row in live_journal.all_history_in_seq_order() {
            if row.seq <= t {
                restored_journal.append_history_for_test(row);
                retained_rows += 1;
            }
        }

        let restored_outbox = OutboxStore::new();
        let mut retained_outbox = 0usize;
        let mut max_outbox_seq: i64 = -1;
        for row in live_outbox.committed_rows() {
            if (row.seq as i64) <= t {
                restored_outbox.restore_committed_row_for_test(row.clone());
                retained_outbox += 1;
                max_outbox_seq = max_outbox_seq.max(row.seq as i64);
            }
        }

        let restored_runs = RunStore::new();
        let mut resumable_runs = 0usize;
        for run in live_runs.all_runs() {
            let restored_depth =
                restored_journal.history_for(&run.tenant, &run.run_id).len() as i64;
            let mut restored = run.clone();
            restored.cursor = restored.cursor.min(restored_depth);
            restored.lease_owner = None;
            restored.lease_expires = None;
            if !run_state::is_terminal(&restored.state) {
                resumable_runs += 1;
            }
            restored_runs.put(restored);
        }

        RestoredFlow {
            runs: restored_runs,
            journal: restored_journal,
            outbox: restored_outbox,
            to_offset: t,
            retained_history_rows: retained_rows,
            retained_outbox_rows: retained_outbox,
            max_outbox_seq,
            resumable_runs,
        }
    }
}

pub struct RestoredFlow {
    runs: RunStore,
    journal: WfJournal,
    outbox: OutboxStore,
    to_offset: ConsistentOffset,
    retained_history_rows: usize,
    retained_outbox_rows: usize,
    max_outbox_seq: i64,
    resumable_runs: usize,
}

impl RestoredFlow {
    pub fn runs(&self) -> &RunStore {
        &self.runs
    }

    pub fn journal(&self) -> &WfJournal {
        &self.journal
    }

    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    pub fn consistent_offset(&self) -> ConsistentOffset {
        self.to_offset
    }

    pub fn resumable_runs(&self) -> usize {
        self.resumable_runs
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreVerifyFailure {
    VanishedResult {
        run_id: String,
        history_seq: i64,
        vanished_ref: ArtifactRef,
    },
    RunDidNotResume {
        run_id: String,
        reason: String,
    },
    DoubleEffectOnResume {
        count: u64,
    },
    OffsetsUnreconciled {
        detail: String,
    },
}

impl core::fmt::Display for RestoreVerifyFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RestoreVerifyFailure::VanishedResult {
                run_id,
                history_seq,
                vanished_ref,
            } => write!(
                f,
                "FLOW-D10 FAIL - VANISHED RESULT: run {run_id} wf_history seq {history_seq} points at \
                 result {} produced PAST the consistent point - the restore left a dangling reference",
                vanished_ref.0
            ),
            RestoreVerifyFailure::RunDidNotResume { run_id, reason } => write!(
                f,
                "FLOW-D10 FAIL - RUN DID NOT RESUME: in-flight run {run_id} did not resume after the \
                 restore: {reason}"
            ),
            RestoreVerifyFailure::DoubleEffectOnResume { count } => write!(
                f,
                "FLOW-D10 FAIL - DOUBLE EFFECT ON RESUME: {count} journaled side effect(s) re-executed on \
                 resume - the restored journal prefix was NOT short-circuited (exactly-once-in-effect broke)"
            ),
            RestoreVerifyFailure::OffsetsUnreconciled { detail } => write!(
                f,
                "FLOW-D10 FAIL - OFFSETS UNRECONCILED: store ↔ outbox offsets did not land at one \
                 consistent point: {detail}"
            ),
        }
    }
}

impl std::error::Error for RestoreVerifyFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsistentPointArtifact {
    pub consistent_offset: ConsistentOffset,
    pub runs_resumed: usize,
    pub history_rows_retained: usize,
    pub outbox_rows_retained: usize,
    pub vanished_results: u64,
    pub double_effects_on_resume: u64,
    pub unreconciled_offsets: u64,
}

impl ConsistentPointArtifact {
    pub fn summary(&self) -> String {
        format!(
            "FLOW-D10 restore-verify PASS: restored myelin-flow to consistent point T={} - {} in-flight \
             run(s) resumed, {} wf_history rows + {} outbox rows retained at ONE point; vanished_results={}, \
             double_effects_on_resume={}, unreconciled_offsets={} (all 0). store↔outbox↔referenced-rows \
             consistent; no run points at a vanished result.",
            self.consistent_offset,
            self.runs_resumed,
            self.history_rows_retained,
            self.outbox_rows_retained,
            self.vanished_results,
            self.double_effects_on_resume,
            self.unreconciled_offsets,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a restore-verify outcome must be checked - a dropped RED is a SWALLOWED cross-seam-integrity \
              failure (FLOW-D10, EI-01 §5: loud-never-swallowed)"]
pub enum RestoreVerifyOutcome {
    Green(ConsistentPointArtifact),
    Red(RestoreVerifyFailure),
}

impl RestoreVerifyOutcome {
    pub fn is_green(&self) -> bool {
        matches!(self, RestoreVerifyOutcome::Green(_))
    }

    pub fn artifact(&self) -> Option<&ConsistentPointArtifact> {
        match self {
            RestoreVerifyOutcome::Green(a) => Some(a),
            RestoreVerifyOutcome::Red(_) => None,
        }
    }

    pub fn failure(&self) -> Option<&RestoreVerifyFailure> {
        match self {
            RestoreVerifyOutcome::Red(f) => Some(f),
            RestoreVerifyOutcome::Green(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WfRestoreVerify;

impl WfRestoreVerify {
    pub fn new() -> WfRestoreVerify {
        WfRestoreVerify
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        restore: &WfRestore,
        live_runs: &RunStore,
        live_journal: &WfJournal,
        live_outbox: &OutboxStore,
        telemetry: &FlowTelemetry,
        minter: std::sync::Arc<dyn IdMinter>,
        ctx_base: EmitContextBase,
        now: i64,
        now_clock: &str,
        rand_seed: u64,
        body_for: &dyn Fn(&str) -> Option<Box<WorkflowBody>>,
    ) -> RestoreVerifyOutcome {
        let restored = restore.apply(live_runs, live_journal, live_outbox);
        let t = restored.to_offset;

        let mut producer_seq: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for row in live_journal.all_history_in_seq_order() {
            if row.kind != crate::wfctx::history_kind::ACTIVITY_COMPLETED {
                continue;
            }
            if let Some(refs) = &row.result {
                for r in refs {
                    producer_seq
                        .entry(r.0.clone())
                        .and_modify(|s| *s = (*s).min(row.seq))
                        .or_insert(row.seq);
                }
            }
        }
        for row in restored.journal.all_history_in_seq_order() {
            if let Some(refs) = &row.result {
                for r in refs {
                    let producer = producer_seq.get(&r.0).copied().unwrap_or(i64::MAX);
                    if producer > t {
                        let failure = RestoreVerifyFailure::VanishedResult {
                            run_id: row.run_id.clone(),
                            history_seq: row.seq,
                            vanished_ref: r.clone(),
                        };
                        telemetry.record_restore_verify_red();
                        return RestoreVerifyOutcome::Red(failure);
                    }
                }
            }
        }

        let max_history_seq = restored
            .journal
            .all_history_in_seq_order()
            .iter()
            .map(|r| r.seq)
            .max()
            .unwrap_or(-1);
        if max_history_seq > t {
            let failure = RestoreVerifyFailure::OffsetsUnreconciled {
                detail: format!(
                    "wf_history max seq {max_history_seq} sits past the consistent point T={t} (a journal \
                     row survived past the restore offset)"
                ),
            };
            telemetry.record_restore_verify_red();
            return RestoreVerifyOutcome::Red(failure);
        }
        if restored.max_outbox_seq > t {
            let failure = RestoreVerifyFailure::OffsetsUnreconciled {
                detail: format!(
                    "outbox max seq {} sits past the consistent point T={t} (an emitted event survived past \
                     the restore offset - an emit-without-journal ghost)",
                    restored.max_outbox_seq
                ),
            };
            telemetry.record_restore_verify_red();
            return RestoreVerifyOutcome::Red(failure);
        }

        let double_effects_before = telemetry.double_effect_count();
        let mut runs_resumed = 0usize;
        let mut in_flight: Vec<RunRow> = restored
            .runs
            .all_runs()
            .into_iter()
            .filter(|r| !run_state::is_terminal(&r.state))
            .collect();
        in_flight.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        for run in in_flight {
            let leased =
                match restored
                    .runs
                    .lease_runnable(run.partition, "restore-verify", now, 300)
                {
                    Some(l) if l.run_id == run.run_id => l,
                    _ => match lease_specific(&restored.runs, &run, now) {
                        Some(l) => l,
                        None => {
                            let failure = RestoreVerifyFailure::RunDidNotResume {
                                run_id: run.run_id.clone(),
                                reason: "the in-flight run was not re-leasable after the restore"
                                    .into(),
                            };
                            telemetry.record_restore_verify_red();
                            return RestoreVerifyOutcome::Red(failure);
                        }
                    },
                };
            let body = match body_for(&leased.wf_type) {
                Some(b) => b,
                None => {
                    let failure = RestoreVerifyFailure::RunDidNotResume {
                        run_id: leased.run_id.clone(),
                        reason: format!("no registered body for wf_type {}", leased.wf_type),
                    };
                    telemetry.record_restore_verify_red();
                    return RestoreVerifyOutcome::Red(failure);
                }
            };
            let _ = drive(
                &restored.runs,
                &restored.outbox,
                &restored.journal,
                telemetry,
                minter.clone(),
                ctx_base.clone(),
                &leased,
                now_clock,
                rand_seed,
                body.as_ref(),
            );
            runs_resumed += 1;
        }

        let double_effects_on_resume = telemetry.double_effect_count() - double_effects_before;
        if double_effects_on_resume > 0 {
            let failure = RestoreVerifyFailure::DoubleEffectOnResume {
                count: double_effects_on_resume,
            };
            telemetry.record_restore_verify_red();
            return RestoreVerifyOutcome::Red(failure);
        }

        let artifact = ConsistentPointArtifact {
            consistent_offset: t,
            runs_resumed,
            history_rows_retained: restored.retained_history_rows,
            outbox_rows_retained: restored.retained_outbox_rows,
            vanished_results: 0,
            double_effects_on_resume: 0,
            unreconciled_offsets: 0,
        };
        telemetry.record_restore_verify_green(t, runs_resumed as u64);
        RestoreVerifyOutcome::Green(artifact)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_or_fail(
        &self,
        restore: &WfRestore,
        live_runs: &RunStore,
        live_journal: &WfJournal,
        live_outbox: &OutboxStore,
        telemetry: &FlowTelemetry,
        minter: std::sync::Arc<dyn IdMinter>,
        ctx_base: EmitContextBase,
        now: i64,
        now_clock: &str,
        rand_seed: u64,
        body_for: &dyn Fn(&str) -> Option<Box<WorkflowBody>>,
    ) -> Result<ConsistentPointArtifact, RestoreVerifyFailure> {
        match self.run(
            restore,
            live_runs,
            live_journal,
            live_outbox,
            telemetry,
            minter,
            ctx_base,
            now,
            now_clock,
            rand_seed,
            body_for,
        ) {
            RestoreVerifyOutcome::Green(a) => Ok(a),
            RestoreVerifyOutcome::Red(f) => Err(f),
        }
    }
}

fn lease_specific(runs: &RunStore, target: &RunRow, now: i64) -> Option<RunRow> {
    runs.with_run_mut(&target.tenant, &target.run_id, |run| {
        if run_state::is_terminal(&run.state) {
            return None;
        }
        run.lease_owner = Some("restore-verify".into());
        run.lease_expires = Some(now + 300);
        Some(run.clone())
    })
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::WfHistoryRow;
    use crate::wfctx::{history_kind, RetryPolicy, WfCtx};
    use myelin_events::{Actor, MonotonicMinter, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
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

    fn history_row(run_id: &str, seq: i64, result: Vec<&str>) -> WfHistoryRow {
        history_row_kind(run_id, seq, history_kind::ACTIVITY_COMPLETED, result)
    }

    fn history_row_kind(run_id: &str, seq: i64, kind: &str, result: Vec<&str>) -> WfHistoryRow {
        WfHistoryRow {
            tenant: tenant(),
            region: region(),
            run_id: run_id.into(),
            seq,
            kind: kind.into(),
            command_id: format!("{run_id}:cmd:{seq}"),
            result: Some(result.into_iter().map(|r| ArtifactRef(r.into())).collect()),
            result_key_ref: None,
        }
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
    fn restore_resumes_in_flight_run_with_zero_double_effect() {
        let live_runs = RunStore::new();
        let live_journal = WfJournal::new();
        let live_outbox = OutboxStore::new();
        let tele = FlowTelemetry::new();

        let ran1 = Arc::new(Mutex::new(Vec::new()));
        let body3 = n_activity_body(3, ran1.clone());
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
            body3(&mut ctx).expect("3 activities run");
            ctx.commit().expect("the 3 steps co-commit");
        }
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.cursor = 3;
        run.lease_owner = Some("dead-worker".into());
        run.lease_expires = Some(5000);
        live_runs.put(run);
        assert_eq!(
            live_journal.history_len(),
            3,
            "3 journaled before the crash"
        );

        let restore = WfRestore::to_offset(100);
        let ran2 = Arc::new(Mutex::new(Vec::new()));
        let bodies = move |wf_type: &str| -> Option<Box<WorkflowBody>> {
            if wf_type == "agent.run" {
                Some(n_activity_body(5, ran2.clone()))
            } else {
                None
            }
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
            "a consistent-point restore must GREEN, got {:?}",
            outcome.failure()
        );
        let artifact = outcome.artifact().expect("green artifact");
        assert_eq!(artifact.consistent_offset, 100);
        assert_eq!(artifact.runs_resumed, 1, "the in-flight run resumed");
        assert_eq!(artifact.vanished_results, 0);
        assert_eq!(
            artifact.double_effects_on_resume, 0,
            "0 re-executed side effect"
        );
        assert_eq!(artifact.unreconciled_offsets, 0);
        assert_eq!(tele.restore_verify_consistent_offset(), 100);
        assert_eq!(tele.restore_verify_runs_resumed(), 1);
        assert_eq!(tele.restore_verify_green_count(), 1);
        assert_eq!(tele.restore_verify_red_count(), 0);
        assert!(artifact.summary().contains("restore-verify PASS"));
    }

    #[test]
    fn a_row_pointing_at_a_vanished_result_fails() {
        let live_runs = RunStore::new();
        let live_journal = WfJournal::new();
        let live_outbox = OutboxStore::new();
        let tele = FlowTelemetry::new();

        live_journal.append_history_for_test(history_row_kind(
            "R1",
            5,
            history_kind::SIDE_MARKER,
            vec!["myelin://acme/result/future"],
        ));
        live_journal.append_history_for_test(history_row(
            "R1",
            9,
            vec!["myelin://acme/result/future"],
        ));
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.cursor = 2;
        live_runs.put(run);

        let restore = WfRestore::to_offset(7);
        let bodies = |_: &str| -> Option<Box<WorkflowBody>> { None };
        let outcome = WfRestoreVerify::new().run(
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
        );

        assert!(
            !outcome.is_green(),
            "a vanished result MUST fail the restore-verify"
        );
        match outcome.failure() {
            Some(RestoreVerifyFailure::VanishedResult {
                run_id,
                history_seq,
                vanished_ref,
            }) => {
                assert_eq!(run_id, "R1");
                assert_eq!(*history_seq, 5);
                assert_eq!(vanished_ref.0, "myelin://acme/result/future");
            }
            other => panic!("expected VanishedResult, got {other:?}"),
        }
        assert_eq!(
            tele.restore_verify_red_count(),
            1,
            "the red is recorded loudly"
        );
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
            .expect_err("must fail");
        assert!(
            err.to_string().contains("VANISHED RESULT"),
            "loud + specific: {err}"
        );
    }

    #[test]
    fn a_terminal_run_is_not_re_driven() {
        let live_runs = RunStore::new();
        let live_journal = WfJournal::new();
        let live_outbox = OutboxStore::new();
        let tele = FlowTelemetry::new();

        live_journal.append_history_for_test(history_row("R1", 0, vec!["myelin://acme/result/a"]));
        live_journal.append_history_for_test(history_row("R1", 1, vec!["myelin://acme/result/a"]));
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::COMPLETED.into();
        run.cursor = 2;
        live_runs.put(run);

        let restore = WfRestore::to_offset(100);
        let bodies = |_: &str| -> Option<Box<WorkflowBody>> { None };
        let outcome = WfRestoreVerify::new().run(
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
        assert_eq!(outcome.artifact().unwrap().history_rows_retained, 2);
    }

    #[test]
    fn restore_truncates_the_future_journal_tail() {
        let live_runs = RunStore::new();
        let live_journal = WfJournal::new();
        let live_outbox = OutboxStore::new();
        let tele = FlowTelemetry::new();

        for seq in 0..3 {
            live_journal.append_history_for_test(history_row(
                "R1",
                seq,
                vec!["myelin://acme/result/a"],
            ));
        }
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.cursor = 3;
        live_runs.put(run);

        let restored = WfRestore::to_offset(1).apply(&live_runs, &live_journal, &live_outbox);
        assert_eq!(
            restored.journal().history_len(),
            2,
            "only seq 0,1 retained (seq 2 truncated)"
        );
        let r = restored.runs().get(&tenant(), "R1").expect("run");
        assert_eq!(
            r.cursor, 2,
            "the cursor clamped to the restored journal depth"
        );
        assert!(
            r.lease_owner.is_none(),
            "the lease cleared - re-leasable on restore"
        );
        let _ = &tele;
    }

    #[test]
    fn the_verdict_is_must_use_and_never_a_silent_green() {
        let red = RestoreVerifyOutcome::Red(RestoreVerifyFailure::OffsetsUnreconciled {
            detail: "test".into(),
        });
        assert!(!red.is_green());
        assert!(red.artifact().is_none());
        assert!(red.failure().is_some());
    }
}
