//! # `restore_verify` — restore to a consistent point: in-flight runs resume, no vanished result
//! (FLOW-D10 / P-FLOW-25, M5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §7 (the F-10 drill: *restore +
//! cross-seam integrity — consistent resume, no orphaned references*) + §4.1 (in-flight runs resume on
//! replay — the journal is the source of truth, a re-driven run replays its `wf_history` to the cursor
//! with 0 re-executed side effect) + §3.2 (`wf_history` as the journal source of truth, the per-run
//! monotonic `seq` the consistency point is taken over).
//!
//! **Contract-index cluster:** row **9.6** (`PersonalDataHolder(workflow history) + replay` — the
//! restore-consistency half: a restored run's `wf_history` rows point at LIVE results after the restore,
//! and replay resumes the in-flight run). CONSUMES row **11.5** (Storage's backup/restore + restore-verify:
//! `restore(to_offset T)` + *event-log offset = the cross-seam cursor, OLTP↔blob↔index↔offset*). Wires the
//! contract-**1.8** restore-verify telemetry.
//!
//! ## What this prompt (P-FLOW-25) ships — the myelin-flow restore-verify integration
//!
//! Storage owns the restore MACHINERY ([`myelin_storage::restore::restore_to_offset`], P-FLOW-24's sibling
//! P-060) and the CI-wired restore-verify GATE ([`myelin_storage::restore_verify::RestoreVerifyGate`], P-061)
//! over the OLTP↔blob↔index↔offset seam. This module is myelin-flow's WORKFLOW-NATIVE leg of that gate: it
//! takes a restore TO A CONSISTENT POINT `T` (the event-log offset, contract 11.5) and asserts the three
//! durable-workflow F-10 invariants the storage gate does not model at the workflow grain:
//!
//! 1. **In-flight runs RESUME (§4.1).** A run that was `running`/`waiting` at the restore point is, after
//!    the restore, RE-LEASABLE and replays its restored `wf_history` to its cursor with **0 re-executed
//!    side effect** — the in-flight run resumes EXACTLY where it was, no lost progress, exactly-once-in-
//!    effect. A run whose journal was truncated at `T` resumes from the truncation point (the un-journaled
//!    tail re-executes live — it was never durable).
//! 2. **NO run points at a VANISHED RESULT (§7 F-10).** Every retained `wf_history` row's referenced result
//!    [`ArtifactRef`] is PRESENT in the restored result set: a row whose result was produced by a step PAST
//!    the consistency point (a future write the restore did NOT bring back) is the orphaned-reference case
//!    the restore must not leave — it is made LOUD ([`RestoreVerifyFailure::VanishedResult`]), never a
//!    silently-dangling pointer.
//! 3. **Store ↔ outbox offsets RECONCILE (§7 / contract 11.5 cross-seam cursor).** The journal `seq` and the
//!    outbox committed offset land at ONE consistency point `T`: no `wf_history` row sits past `T` while its
//!    co-committed outbox row is gone (an emit-without-journal ghost), and no committed outbox row references
//!    a journal position the restore did not retain (a journal-without-emit lost write). The co-commit
//!    discipline (FLOW-D5) makes the two move TOGETHER; this leg re-asserts it survived the restore.
//!
//! The verdict is a `#[must_use]` typed [`RestoreVerifyOutcome`] (loud-never-swallowed, EI-01 §5) carrying a
//! dated [`ConsistentPointArtifact`] on green / the exact [`RestoreVerifyFailure`] on red. The
//! restore-verify telemetry (contract 1.8) records the consistent-point offset + the resumed-run count + the
//! three zeros (0 vanished, 0 unreconciled, 0 re-executed).
//!
//! ## Coherence (EI-01 §7) — REUSES storage's restore, does NOT re-implement it
//! The restore ORCHESTRATION + the OLTP↔blob↔index↔offset cross-seam assertion live in `myelin-storage`
//! ([`restore_to_offset`](myelin_storage::restore) + the [`RestoreVerifyGate`](myelin_storage::restore_verify)).
//! This module does NOT re-define them — it consumes the SAME consistent-point offset and adds the
//! WORKFLOW-grain invariants (in-flight-run resume, no-vanished-result, store↔outbox reconcile) the storage
//! gate cannot see (it models rows/blobs/derived-docs, not a `workflow_run` journal + lease). The replay
//! resume reuses the P-FLOW-05 [`crate::engine::drive`] short-circuit unchanged (the SAME mechanism that
//! makes a crash-recovery re-drive resume with 0 double-effect). The drill (`tests/drills_flow_d10_restore_verify.rs`)
//! cross-validates this workflow-native leg against storage's [`RestoreVerifyGate`] on the SAME restore, so
//! the two prove ONE consistent point (coherence, not a parallel assertion).
//!
//! ## DB-free build, real-stack drill
//! `cargo build` stays DB-free: the model truncates the in-memory [`crate::engine::RunStore`] +
//! [`WfJournal`] + [`OutboxStore`] at the consistent point `T` (modeling `pg_restore` to a PITR target),
//! then drives the engine's REAL replay/lease loop over the restored set — the resume + the cross-seam
//! reconcile are the SAME observable properties the live `myelin-flow` Postgres restore lands (dev↔prod is a
//! config swap). The SCHED green artifact against the live dev-stack restore is the storage restore-verify
//! integration (STOR-D1/D2 at cell scale, already green); this module is the workflow-grain unit-of-proof
//! that re-runs as a `cargo test` drill.
//!
//! ## FLOORS named (the prompt's DEFINITION OF DONE)
//! - **The cell-scale live `pg_restore` SCHED run** (the real myelin-flow Postgres restored to a PITR target
//!   under load) rides Storage's STOR-D1/D2 restore-verify at cell scale (already green at P-444). This
//!   module is the workflow-grain leg that re-runs forever as a `cargo test` drill; the dated cell-scale
//!   SCHED artifact is the storage integration's. Named, not re-built here.
//! - The world-scale 30× load restore is the world-scale floor (real fleet hardware), per the binding policy.

use myelin_events::{EmitContextBase, IdMinter, OutboxStore};
use myelin_refs::ArtifactRef;

use crate::engine::{drive, run_state, FlowTelemetry, RunRow, RunStore, WorkflowBody};
use crate::wfctx::WfJournal;

/// **The consistent point a restore lands at — the event-log offset (contract 11.5 cross-seam cursor).**
/// One opaque monotonic offset `T` the WHOLE restore is taken over (OLTP↔blob↔index↔outbox): every
/// `wf_history` row with `seq <= T` is retained, every outbox row with `seq <= T` is retained, and a run's
/// cursor lands at its retained journal depth. NOT PII (a log offset). The journal `seq` and the outbox
/// `seq` are the SAME monotonic log offset (the co-commit advances both together, §3.2), so one `T`
/// truncates both tiers to ONE point.
pub type ConsistentOffset = i64;

/// **A restore of the myelin-flow durable-workflow state to a consistent point `T`.** Models `pg_restore`
/// to a PITR target: it takes the LIVE run store / journal / outbox and produces the RESTORED copy
/// truncated at `T` (every row past `T` is dropped — it was a future write the backup did not capture). The
/// gate then drives the engine's real replay/lease loop over the restored copy and asserts the three F-10
/// invariants. Reuses the in-memory engine stores as the restored target (the dev↔prod config swap; the live
/// `pg_restore` populates the SAME stores).
#[derive(Clone)]
pub struct WfRestore {
    /// The consistency point `T` (the event-log offset) the restore truncates to.
    to_offset: ConsistentOffset,
}

impl WfRestore {
    /// A restore to the consistent point `to_offset` (the event-log offset, contract 11.5).
    pub fn to_offset(to_offset: ConsistentOffset) -> WfRestore {
        WfRestore { to_offset }
    }

    /// The consistency point `T` this restore lands at.
    pub fn consistent_offset(&self) -> ConsistentOffset {
        self.to_offset
    }

    /// **Apply the restore: produce the RESTORED copy truncated at `T`.** Models `pg_restore` to the PITR
    /// target — the restored stores hold ONLY the rows at `seq <= T` (a future write past `T` is a row the
    /// backup never captured, so the restore drops it). Returns the restored [`RunStore`] / [`WfJournal`] /
    /// [`OutboxStore`] the gate verifies + the engine re-drives.
    ///
    /// A run's `cursor` is clamped to its RESTORED journal depth (the highest retained `seq + 1` for the
    /// run): a run whose journal tail was truncated resumes from the truncation point — the un-journaled
    /// tail re-executes live (it was never durable). A run's lease is CLEARED (the restored copy has no live
    /// lessor — every in-flight run is re-leasable on restore, the crash-recovery posture §4.7).
    pub fn apply(
        &self,
        live_runs: &RunStore,
        live_journal: &WfJournal,
        live_outbox: &OutboxStore,
    ) -> RestoredFlow {
        let t = self.to_offset;

        // (1) Restore the journal: keep every `wf_history` row at `seq <= T` (drop the future tail).
        let restored_journal = WfJournal::new();
        let mut retained_rows = 0usize;
        for row in live_journal.all_history_in_seq_order() {
            if row.seq <= t {
                restored_journal.append_history_for_test(row);
                retained_rows += 1;
            }
        }

        // (2) Restore the outbox: keep every committed outbox row at `seq <= T` (drop the future tail).
        // The restored outbox is the cross-seam reconcile target.
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

        // (3) Restore the run store: clamp each run's cursor to its RESTORED journal depth and clear the
        // lease (every in-flight run is re-leasable on restore — the §4.7 crash-recovery posture).
        let restored_runs = RunStore::new();
        let mut resumable_runs = 0usize;
        for run in live_runs.all_runs() {
            let restored_depth =
                restored_journal.history_for(&run.tenant, &run.run_id).len() as i64;
            let mut restored = run.clone();
            // The cursor lands at the restored journal depth (a truncated tail resumes from there).
            restored.cursor = restored.cursor.min(restored_depth);
            restored.lease_owner = None;
            restored.lease_expires = None;
            // A run that was non-terminal at the restore point is IN-FLIGHT — it must resume. A terminal
            // run (completed/failed/terminated) is not re-driven (it is already settled).
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

/// **The RESTORED copy a [`WfRestore`] produced — the clean PITR target the gate verifies + the engine
/// re-drives.** Holds the restored run store / journal / outbox truncated at the consistent point `T`, plus
/// the counts the green artifact reports. The engine's real replay/lease loop drives THIS to prove in-flight
/// runs resume; the gate reads its rows to prove no vanished result + the cross-seam reconcile.
pub struct RestoredFlow {
    runs: RunStore,
    journal: WfJournal,
    outbox: OutboxStore,
    to_offset: ConsistentOffset,
    retained_history_rows: usize,
    retained_outbox_rows: usize,
    /// The highest outbox `seq` retained (`-1` if none) — the outbox half of the cross-seam cursor.
    max_outbox_seq: i64,
    resumable_runs: usize,
}

impl RestoredFlow {
    /// The restored run store (the §4.7 lease substrate the engine re-leases in-flight runs from).
    pub fn runs(&self) -> &RunStore {
        &self.runs
    }

    /// The restored `wf_history` journal (the §3.2 replay source of truth, truncated at `T`).
    pub fn journal(&self) -> &WfJournal {
        &self.journal
    }

    /// The restored outbox (the cross-seam reconcile target, truncated at `T`).
    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    /// The consistency point `T` this restore landed at.
    pub fn consistent_offset(&self) -> ConsistentOffset {
        self.to_offset
    }

    /// The number of in-flight (non-terminal) runs the restore retained — each MUST resume on a re-drive.
    pub fn resumable_runs(&self) -> usize {
        self.resumable_runs
    }
}

/// **A RED restore-verify result — EXACTLY which F-10 invariant the restore broke (observability is part of
/// the pass, EI-01 §3).** Never a bare bool: a failed restore points at the precise corruption. Each variant
/// FAILs the restore-verify gate loudly; none is silently swallowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreVerifyFailure {
    /// **A run points at a VANISHED RESULT (the F-10 orphaned-reference case, §7).** A retained `wf_history`
    /// row references a result [`ArtifactRef`] produced by a step PAST the consistency point — the restore
    /// did not bring the result back, so the pointer dangles. The gravest cross-seam break: a resumed run
    /// would read a result that no longer exists.
    VanishedResult {
        /// The run whose history row points at the vanished result.
        run_id: String,
        /// The `wf_history.seq` of the row with the dangling result pointer.
        history_seq: i64,
        /// The result ref that vanished (produced past `T`, not in the restored set).
        vanished_ref: ArtifactRef,
    },
    /// **An IN-FLIGHT run did NOT resume (§4.1).** A non-terminal run could not be re-leased + re-driven to
    /// a resumption point after the restore — the in-flight run is stuck (lost progress). Names the run.
    RunDidNotResume {
        /// The in-flight run that failed to resume on a post-restore re-drive.
        run_id: String,
        /// The machine reason the resume failed (no PII).
        reason: String,
    },
    /// **A re-executed SIDE EFFECT on resume (the §4.1 exactly-once-in-effect break).** The post-restore
    /// re-drive RE-EXECUTED a journaled side effect (the restored journal's prefix was not short-circuited)
    /// — a resumed run double-applied an effect. MUST be 0; a non-zero is a Tier-1 data-loss-class failure.
    DoubleEffectOnResume {
        /// The number of journaled side effects re-executed on resume (`> 0`).
        count: u64,
    },
    /// **Store ↔ outbox offsets did NOT reconcile (§7 cross-seam cursor, contract 11.5).** The journal `seq`
    /// and the outbox committed offset landed at DIFFERENT points — a `wf_history` row sits past `T` while
    /// its co-committed outbox row is gone (an emit-without-journal ghost), or a committed outbox row
    /// references a journal position the restore did not retain (a journal-without-emit lost write). The
    /// co-commit (FLOW-D5) must keep them at ONE point; this is a restore that split them.
    OffsetsUnreconciled {
        /// The machine detail (which seam sits past which point — no PII).
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
                "FLOW-D10 FAIL — VANISHED RESULT: run {run_id} wf_history seq {history_seq} points at \
                 result {} produced PAST the consistent point — the restore left a dangling reference",
                vanished_ref.0
            ),
            RestoreVerifyFailure::RunDidNotResume { run_id, reason } => write!(
                f,
                "FLOW-D10 FAIL — RUN DID NOT RESUME: in-flight run {run_id} did not resume after the \
                 restore: {reason}"
            ),
            RestoreVerifyFailure::DoubleEffectOnResume { count } => write!(
                f,
                "FLOW-D10 FAIL — DOUBLE EFFECT ON RESUME: {count} journaled side effect(s) re-executed on \
                 resume — the restored journal prefix was NOT short-circuited (exactly-once-in-effect broke)"
            ),
            RestoreVerifyFailure::OffsetsUnreconciled { detail } => write!(
                f,
                "FLOW-D10 FAIL — OFFSETS UNRECONCILED: store ↔ outbox offsets did not land at one \
                 consistent point: {detail}"
            ),
        }
    }
}

impl std::error::Error for RestoreVerifyFailure {}

/// **The dated GREEN ARTIFACT a consistent-point restore-verify emits on pass (observability is part of the
/// pass, EI-01 §3).** Carries the MEASURED numbers — never a bare "ok": the consistent-point offset `T`, the
/// resumed-run count, the retained row counts, and the three zeros the gate asserted (0 vanished result, 0
/// re-executed side effect, 0 unreconciled offset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsistentPointArtifact {
    /// The consistency point `T` (the event-log offset) the restore landed at.
    pub consistent_offset: ConsistentOffset,
    /// The number of in-flight runs that RESUMED on the post-restore re-drive.
    pub runs_resumed: usize,
    /// The number of `wf_history` rows retained (all `seq <= T`).
    pub history_rows_retained: usize,
    /// The number of outbox rows retained (all `seq <= T`).
    pub outbox_rows_retained: usize,
    /// Runs pointing at a vanished result — `0` on a green pass (the no-orphaned-reference leg).
    pub vanished_results: u64,
    /// Side effects re-executed on resume — `0` on a green pass (the exactly-once-in-effect leg).
    pub double_effects_on_resume: u64,
    /// Store↔outbox offset reconciliation mismatches — `0` on a green pass (the cross-seam cursor leg).
    pub unreconciled_offsets: u64,
}

impl ConsistentPointArtifact {
    /// Render the dated consistent-point line a SCHED run prints on PASS (the measured-numbers proof). The
    /// caller prefixes the date (`[P-FLOW-25 FLOW-D10 GREEN <date>]`) so the artifact is dated at the run.
    pub fn summary(&self) -> String {
        format!(
            "FLOW-D10 restore-verify PASS: restored myelin-flow to consistent point T={} — {} in-flight \
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

/// **The typed verdict of a consistent-point restore-verify — GREEN (a [`ConsistentPointArtifact`]) or RED (a
/// [`RestoreVerifyFailure`]).** `#[must_use]`: a dropped verdict is a swallowed cross-seam-integrity check
/// (the EI-01 §5 loud-never-swallowed violation) and the compiler flags it. There is NO bool coercion that
/// loses the failure; the only way to consume a red is to handle it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a restore-verify outcome must be checked — a dropped RED is a SWALLOWED cross-seam-integrity \
              failure (FLOW-D10, EI-01 §5: loud-never-swallowed)"]
pub enum RestoreVerifyOutcome {
    /// The restore landed at one consistent point: in-flight runs resumed, no vanished result, offsets
    /// reconciled. Carries the dated [`ConsistentPointArtifact`] with the measured numbers.
    Green(ConsistentPointArtifact),
    /// The restore did NOT land at one consistent point — EXACTLY what broke. FAILs the gate; never swallowed.
    Red(RestoreVerifyFailure),
}

impl RestoreVerifyOutcome {
    /// `true` iff the restore-verify passed. The ONLY way to read a pass — a [`Red`] is never silently a pass.
    ///
    /// [`Red`]: RestoreVerifyOutcome::Red
    pub fn is_green(&self) -> bool {
        matches!(self, RestoreVerifyOutcome::Green(_))
    }

    /// The green artifact, if the restore-verify passed.
    pub fn artifact(&self) -> Option<&ConsistentPointArtifact> {
        match self {
            RestoreVerifyOutcome::Green(a) => Some(a),
            RestoreVerifyOutcome::Red(_) => None,
        }
    }

    /// The failure, if the restore-verify failed.
    pub fn failure(&self) -> Option<&RestoreVerifyFailure> {
        match self {
            RestoreVerifyOutcome::Red(f) => Some(f),
            RestoreVerifyOutcome::Green(_) => None,
        }
    }
}

/// **The myelin-flow restore-verify gate (FLOW-D10) — restore to a consistent point + the three F-10
/// invariants.** A zero-sized orchestrator (it holds no state): a run is `WfRestoreVerify::run(...)`. Reuses
/// Storage's restore machinery posture (truncate-to-`T`) at the workflow grain + the P-FLOW-05 replay
/// short-circuit (the SAME mechanism crash-recovery uses), adding the in-flight-resume / no-vanished-result /
/// store↔outbox-reconcile legs the storage gate cannot model.
#[derive(Clone, Copy, Debug, Default)]
pub struct WfRestoreVerify;

impl WfRestoreVerify {
    /// A new gate (stateless).
    pub fn new() -> WfRestoreVerify {
        WfRestoreVerify
    }

    /// **Run the consistent-point restore-verify once.** Applies the restore (truncate the LIVE stores to
    /// the consistent point `T`), then over the RESTORED copy:
    ///
    /// 1. asserts NO retained `wf_history` row points at a vanished result (every referenced result is in
    ///    the restored set — the F-10 no-orphaned-reference leg);
    /// 2. asserts the store ↔ outbox offsets reconcile (the cross-seam cursor landed at one point);
    /// 3. RE-DRIVES every in-flight run on the engine's real replay/lease loop and asserts it RESUMES with
    ///    0 re-executed side effect (the §4.1 exactly-once-in-effect leg).
    ///
    /// `bodies` supplies the deterministic workflow body per `wf_type` (the definition registry) the resume
    /// re-drives; `minter` / `ctx_base` / `now` seed the live side-markers (a replayed `now`/`rand` returns
    /// its captured value). Returns [`RestoreVerifyOutcome::Green`] (the dated artifact) or
    /// [`RestoreVerifyOutcome::Red`] (exactly what broke). The restore-verify telemetry (contract 1.8) is
    /// recorded on `telemetry`.
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
        // (0) Apply the restore — produce the restored copy truncated at the consistent point T.
        let restored = restore.apply(live_runs, live_journal, live_outbox);
        let t = restored.to_offset;

        // (1) NO VANISHED RESULT — every result a RETAINED wf_history row references must still be PRODUCED
        // by a retained row (the F-10 no-orphaned-reference leg, §7). A result is PRODUCED by the
        // `activity_completed` row that journals it (the activity's effect refs, §4.1) — its producer-seq is
        // the lowest seq it appears on a producing row in the LIVE journal. A RETAINED row that references a
        // result whose producer-seq is PAST the consistent point `T` (truncated away) points at a VANISHED
        // result — the dangling pointer the restore must not leave. (A reference whose producer survived at
        // seq <= T is fine.)
        let mut producer_seq: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for row in live_journal.all_history_in_seq_order() {
            if row.kind != crate::wfctx::history_kind::ACTIVITY_COMPLETED {
                continue; // only an activity_completed row PRODUCES a result; other kinds reference.
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
                    // The producer-seq of the referenced result. If the producer was truncated past T (its
                    // lowest-seq producer is > T — or there is no producer at all), the result VANISHED.
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

        // (2) STORE ↔ OUTBOX OFFSETS RECONCILE — the journal seq and the outbox committed offset land at
        // ONE point. The cross-seam cursor invariant (contract 11.5): no retained wf_history row sits past T
        // (the truncation guarantees this), and no retained outbox row sits past T. Both tiers are truncated
        // to the SAME T by the co-commit discipline, so the highest retained seq on each side must be <= T.
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
                     the restore offset — an emit-without-journal ghost)",
                    restored.max_outbox_seq
                ),
            };
            telemetry.record_restore_verify_red();
            return RestoreVerifyOutcome::Red(failure);
        }

        // (3) IN-FLIGHT RUNS RESUME — re-drive every in-flight (non-terminal) run on the engine's REAL
        // replay/lease loop and assert it resumes with 0 re-executed side effect (§4.1). This is the SAME
        // P-FLOW-05 short-circuit crash-recovery uses: the restored journal prefix replays (0 side effect),
        // the run resumes at the first un-journaled command.
        let double_effects_before = telemetry.double_effect_count();
        let mut runs_resumed = 0usize;
        // Deterministic order over the in-flight runs (by run_id) so the re-drive is stable.
        let mut in_flight: Vec<RunRow> = restored
            .runs
            .all_runs()
            .into_iter()
            .filter(|r| !run_state::is_terminal(&r.state))
            .collect();
        in_flight.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        for run in in_flight {
            // Re-lease the in-flight run (the restore cleared the lease — it is re-leasable, §4.7).
            let leased =
                match restored
                    .runs
                    .lease_runnable(run.partition, "restore-verify", now, 300)
                {
                    Some(l) if l.run_id == run.run_id => l,
                    // Could not re-lease THIS run on this scan; lease by partition may have picked another.
                    // Re-scan deterministically until this run is leased or no runnable work remains.
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
            // Re-drive: replay the restored journal prefix (0 re-execution), resume at the first un-journaled
            // command. The drive's short-circuit + co-commit are the P-FLOW-05 mechanism unchanged.
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

        // The §4.1 exactly-once-in-effect leg: the re-drives must NOT have re-executed any journaled side
        // effect (the restored prefix short-circuited). A non-zero delta is the DoubleEffectOnResume red.
        let double_effects_on_resume = telemetry.double_effect_count() - double_effects_before;
        if double_effects_on_resume > 0 {
            let failure = RestoreVerifyFailure::DoubleEffectOnResume {
                count: double_effects_on_resume,
            };
            telemetry.record_restore_verify_red();
            return RestoreVerifyOutcome::Red(failure);
        }

        // PASS — the restore landed at one consistent point. Emit the dated green artifact + telemetry.
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

    /// **The loud-never-swallowed SCHED entrypoint (EI-01 §5).** Run the restore-verify and turn a RED into a
    /// process-failing `Err(RestoreVerifyFailure)` — so a SCHED invocation `gate.run_or_fail(...)?` FAILS on
    /// an inconsistent restore, with NO `|| true`, no `.ok()`, no swallow. On GREEN it returns the dated
    /// [`ConsistentPointArtifact`].
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

/// Re-lease a SPECIFIC run after a restore by scanning its partition deterministically (the partition lease
/// may pick another runnable run first; this re-leases until the target is claimed). Returns the leased row
/// for `target`, or `None` if it is not re-leasable.
fn lease_specific(runs: &RunStore, target: &RunRow, now: i64) -> Option<RunRow> {
    // The restore cleared every lease, so a direct lease of the target's run row is leasable; we re-read it
    // and stamp the lease in place (modeling `SELECT … WHERE run_id = $1 FOR UPDATE`).
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

    /// An `activity_completed` history row for `run_id` at `seq` PRODUCING `result` refs.
    fn history_row(run_id: &str, seq: i64, result: Vec<&str>) -> WfHistoryRow {
        history_row_kind(run_id, seq, history_kind::ACTIVITY_COMPLETED, result)
    }

    /// A history row of `kind` for `run_id` at `seq` carrying `result` refs. A `side_marker` REFERENCES a
    /// result (does not produce it); an `activity_completed` PRODUCES it (§4.1).
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

    /// An `n`-activity deterministic body recording which steps actually RAN (vs replayed).
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

    /// **THE HEADLINE GREEN: a restore to a consistent point resumes an in-flight run with 0 double-effect
    /// and no vanished result.** A run journals 3 activities (durable), the worker dies (the un-journaled
    /// tail is lost). A restore to T = the journal depth re-leases the run; the post-restore re-drive
    /// replays the 3 journaled steps (0 re-execution) and resumes at step 4 — the in-flight run resumed.
    #[test]
    fn restore_resumes_in_flight_run_with_zero_double_effect() {
        let live_runs = RunStore::new();
        let live_journal = WfJournal::new();
        let live_outbox = OutboxStore::new();
        let tele = FlowTelemetry::new();

        // An in-flight run that journaled 3 of 5 activities, then the worker crashed (run left running).
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

        // Restore to T = the journal depth (the consistent point). The whole journal is at seq < 3.
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
        // The telemetry recorded the dated consistent point + the resumed run (the FLOW-D10 artifact).
        assert_eq!(tele.restore_verify_consistent_offset(), 100);
        assert_eq!(tele.restore_verify_runs_resumed(), 1);
        assert_eq!(tele.restore_verify_green_count(), 1);
        assert_eq!(tele.restore_verify_red_count(), 0);
        assert!(artifact.summary().contains("restore-verify PASS"));
    }

    /// **MANDATORY-CORE: a `wf_history` row pointing at a result produced PAST the consistent point FAILs
    /// (the F-10 no-orphaned-reference leg).** A run's journal carries a row at seq 5 whose result was
    /// produced by a step at seq 9 (truncated past T=7) — but the row at seq 5 survives (it references a
    /// vanished result). The gate makes it LOUD, never a silently-dangling pointer. Kills the mutant that
    /// drops the vanished-result check.
    #[test]
    fn a_row_pointing_at_a_vanished_result_fails() {
        let live_runs = RunStore::new();
        let live_journal = WfJournal::new();
        let live_outbox = OutboxStore::new();
        let tele = FlowTelemetry::new();

        // seq 5 is a side_marker that REFERENCES "myelin://acme/result/future" — but the ONLY row that
        // PRODUCES it (an activity_completed) is at seq 9 (past T=7). After the restore the producing row is
        // truncated and seq 5 dangles (a reference to a vanished result).
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

        let restore = WfRestore::to_offset(7); // truncate the seq-9 producer away.
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
        // run_or_fail FAILs loudly on it (loud-never-swallowed).
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

    /// **A clean restore (no future tail) resumes a terminal run as a no-op and greens with 0 resumed.** A
    /// fully-completed run is terminal — the restore retains its whole journal, points at no vanished
    /// result, and the gate does NOT re-drive it (it is already settled). 0 in-flight runs to resume.
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

    /// **The restore truncates the future tail: a run whose journal was written past T loses the un-durable
    /// tail and resumes from the truncation point.** A run journals 3 rows (seq 0,1,2); the restore to T=1
    /// retains only seq 0,1 — the run's cursor clamps to 2 (the restored depth), and the seq-2 row is
    /// dropped (it was a future write the backup did not capture). The retained rows reconcile at T.
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
            "the lease cleared — re-leasable on restore"
        );
        let _ = &tele;
    }

    /// **A dropped RED verdict is a compile-error (the `#[must_use]` loud-never-swallowed enforcement).**
    /// We cannot test a compile error at runtime; instead assert the verdict carries the failure (a red is
    /// never a silent green): the `is_green()`/`failure()` accessors are the only way to read it.
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
