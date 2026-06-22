//! # `maintenance` — resumable maintenance activities + the history-rewrite invalidation fan-out
//! (P-FLOW-20 → P-265, M3)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §6.6 (resumable maintenance
//! activities — Git GC / repack / bundle-gen / history-rewrite run as journaled activities, or
//! `SCHEDULE_AND_RUN_JOB` long-parks for the heavy ones, on a workflow; a crash mid-repack replays to
//! the un-journaled step §4.1 with no re-executed side effect; the history-rewrite invalidation
//! fan-out — fork/mirror/clone-cache → the trust-scoped cache namespaces, contract 11.2 — is a
//! sequence of journaled activities) + §4.1 (deterministic replay/recovery: replay-to-the-un-journaled
//! step) + §4.4 (the activity model the maintenance rides). Carried from recon §7 / change-requests §7.
//!
//! **Contract-index cluster:** CONSUMES contract 10.6 (the history-rewrite erasure-admin op — the
//! audited op Git invokes, GDPR/Audit-owned) + 11.2 (the trust-scoped cache namespaces the
//! invalidation fan-out touches — Storage-owned: an `UntrustedFork` write cannot reach the trusted
//! cache scope). This crate ships the **myelin-flow helper**; Git's GC / repack / bundle-gen /
//! history-rewrite are the co-built CONSUMERS (Git's M3 prompts wire the call sites — GIT-D9 is Git's
//! gate).
//!
//! ## What this prompt (P-FLOW-20) ships — NO NEW ENGINE PRIMITIVE
//!
//! This is the **application of the existing activity model (§4.4)** to Git's M3 maintenance work. It
//! adds NO new engine primitive and NO new table — every maintenance step is an ordinary journaled
//! [`WfCtx::activity`](crate::WfCtx::activity) (or a [`WfCtx::schedule_and_run_job`](crate::WfCtx::
//! schedule_and_run_job) long-park for a heavy one), so the durable-execution guarantees (replay to
//! the un-journaled step, 0 re-executed side effect, at-least-once + idempotent) hold by construction.
//!
//! ### (a) Resumable maintenance activities — [`WfCtx::run_maintenance`]
//!
//! Git's GC / repack / bundle-gen / history-rewrite run as a **sequence of journaled maintenance
//! steps** on a workflow. Each step is one journaled activity: the step's side effect (the pack
//! rewrite, the bundle generation) runs exactly once, the `activity_completed` row is journaled, and a
//! crash mid-sequence **replays to the un-journaled step** (§4.1) — the journaled prefix short-circuits
//! with 0 re-execution, and the sequence resumes from the first un-journaled step. A crash mid-repack
//! does NOT re-run the already-journaled pack rewrite (no re-executed side effect, the FLOW-D1
//! property reused on a maintenance workflow).
//!
//! The heavy ops (a multi-hour repack of a giant repo) ride the [`WfCtx::schedule_and_run_job`] long-
//! park instead of a synchronous activity — they dispatch the job to the unified runner and park on
//! `job.done`, holding no runtime while the runner works. [`WfCtx::run_heavy_maintenance`] is the
//! long-park form.
//!
//! ### (b) The history-rewrite invalidation fan-out — [`WfCtx::run_history_rewrite_invalidation`]
//!
//! When Git performs the audited history-rewrite (the rare commit-body expunge, contract 10.6), the
//! caches keyed on the old history must be invalidated across the **trust-scoped cache namespaces**
//! (contract 11.2): the fork caches, the mirror caches, the clone/bundle caches. The fan-out is a
//! **sequence of journaled activities** — one journaled step per namespace — so a crash mid-fan-out
//! **replays from the last journaled step** (§4.1): the namespaces already invalidated are NOT
//! re-invalidated, and the fan-out resumes from the first un-invalidated namespace. The fan-out is
//! deterministic + resumable, never re-running an already-journaled invalidation.
//!
//! ## references-not-payloads
//!
//! Every maintenance step carries only opaque machine descriptors (a repo ref, a cache-namespace key)
//! — NO inline PII. The journaled `activity_completed` rows carry references-not-payloads markers
//! ([`maintenance_step_marker`] / [`invalidation_marker`]) recording WHICH step / namespace was done,
//! so a journal scan can attribute the maintenance without leaking subject data.
//!
//! ## FLOORS named (recorded, not owned here)
//!
//! - **Git's GC / repack / bundle-gen / history-rewrite CALL SITES** (the consumers that invoke this
//!   helper) → Git's M3 prompts (co-built; GIT-D9 is Git's gate). This prompt ships the myelin-flow
//!   helper + a reference maintenance fixture; the production call sites are Git's.
//! - **The history-rewrite erasure-admin op BODY** (the audited commit-body expunge, contract 10.6) →
//!   GDPR/Audit-owned (the audited op) + Git-owned (the byte-mutation). This helper expresses the
//!   INVALIDATION FAN-OUT that follows the rewrite; the rewrite op itself is the consumer's.
//! - **The dispatch into the runner for a heavy maintenance op** is GATED by AG-D4 (Agent-Fabric /
//!   CI-owned — no untrusted code runs until the sandbox-escape gate is green). The heavy form rides
//!   [`WfCtx::schedule_and_run_job`], which records that gate.

use crate::job::{JobKind, JobOutcome, JobRunner, JobSpec};
use crate::wfctx::{ActivityError, RetryPolicy, WfCtx, WfResult};

/// **A Git maintenance op kind (§6.6).** The routing discriminator for which maintenance the workflow
/// runs — GC (loose-object prune), repack (pack consolidation), bundle-gen (clone-bundle generation),
/// or history-rewrite (the audited commit-body expunge, contract 10.6). A machine token, no PII.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaintenanceOp {
    /// `git gc` — loose-object prune + reflog expiry. Runs as a sequence of journaled activities.
    Gc,
    /// `git repack` — pack consolidation. The heavy one (a giant repo's repack is a multi-hour job →
    /// the [`WfCtx::run_heavy_maintenance`] long-park form).
    Repack,
    /// clone-bundle generation — the within-EU CDN clone/bundle blob class (contract 11.2). Resumable
    /// journaled activities.
    BundleGen,
    /// the audited history-rewrite (the rare commit-body expunge, contract 10.6) — followed by the
    /// invalidation fan-out ([`WfCtx::run_history_rewrite_invalidation`]).
    HistoryRewrite,
}

impl MaintenanceOp {
    /// The machine token for the op (stamped on the journaled marker / a long-park job target — no
    /// PII, a routing discriminator).
    pub fn as_str(self) -> &'static str {
        match self {
            MaintenanceOp::Gc => "gc",
            MaintenanceOp::Repack => "repack",
            MaintenanceOp::BundleGen => "bundle-gen",
            MaintenanceOp::HistoryRewrite => "history-rewrite",
        }
    }
}

/// **A trust-scoped cache namespace the history-rewrite invalidation fan-out touches (contract 11.2,
/// §6.6).** The fan-out invalidates the caches keyed on the rewritten history across each namespace —
/// the fork caches, the mirror caches, the clone/bundle caches. The trust scope is the point: an
/// `UntrustedFork` write cannot reach the trusted cache scope (contract 11.2), so the namespaces are
/// invalidated as DISTINCT journaled steps (each is its own erasable scope). A machine key, no PII.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheNamespace {
    /// the fork caches (the trust-scoped fork blob/delta namespace — the largest fan-out leg).
    Fork,
    /// the within-EU mirror caches (the outbound push-mirror clone caches, residency-gated).
    Mirror,
    /// the clone/bundle caches (the within-EU CDN clone/bundle blob class, contract 11.2).
    CloneBundle,
}

impl CacheNamespace {
    /// The machine key for the namespace (the cache scope the invalidation step purges — no PII).
    pub fn as_str(self) -> &'static str {
        match self {
            CacheNamespace::Fork => "fork",
            CacheNamespace::Mirror => "mirror",
            CacheNamespace::CloneBundle => "clone-bundle",
        }
    }

    /// **The FROZEN fan-out ORDER over the trust-scoped cache namespaces (§6.6, contract 11.2).** The
    /// invalidation visits the namespaces in a STABLE order so a re-drive's journaled prefix lines up
    /// step-for-step (replay-to-the-last-journaled-step requires a deterministic step sequence). Fork
    /// first (the largest, most-write-exposed scope an `UntrustedFork` could have polluted), then the
    /// within-EU mirror caches, then the clone/bundle CDN class.
    pub const FANOUT_ORDER: [CacheNamespace; 3] = [
        CacheNamespace::Fork,
        CacheNamespace::Mirror,
        CacheNamespace::CloneBundle,
    ];
}

/// **The seam a maintenance step's side effect runs through (§4.4/§6.6).** Git's maintenance consumer
/// implements this: each call PERFORMS one maintenance step's side effect (the pack rewrite, the
/// bundle write, one cache namespace's invalidation) exactly once. The engine wraps each call in a
/// journaled activity, so the side effect runs once and a crash mid-sequence replays to the
/// un-journaled step WITHOUT re-running an already-journaled step (§4.1).
///
/// A step returns `Ok(())` on a performed step, or an [`ActivityError`] if the step failed (the
/// activity RETRIES it, §4.4, reusing the deterministic `idem_token` so an idempotent re-attempt is
/// broker-deduped). The `op`/`step_index` identify the step; `idem_token` is the deterministic
/// per-step dedup key (so the consumer's own downstream write is idempotent, §3.5).
pub trait MaintenancePerformer {
    /// Perform one maintenance step's side effect (the `step_index`-th step of `op`). Returns `Ok(())`
    /// on a performed step or an [`ActivityError`] (retried). `idem_token` is the deterministic
    /// per-step dedup key the consumer keys its own downstream effect on.
    fn perform_step(
        &self,
        op: MaintenanceOp,
        step_index: usize,
        idem_token: &str,
    ) -> Result<(), ActivityError>;

    /// **Invalidate one trust-scoped cache namespace (the history-rewrite fan-out, contract 11.2,
    /// §6.6).** Called once per namespace in the fan-out; purges the caches keyed on the rewritten
    /// history within that trust scope. Returns `Ok(())` on a purged namespace or an [`ActivityError`]
    /// (retried). `idem_token` is the deterministic per-namespace dedup key.
    fn invalidate_namespace(
        &self,
        namespace: CacheNamespace,
        idem_token: &str,
    ) -> Result<(), ActivityError>;
}

impl WfCtx {
    /// **`run_maintenance(op, step_count, performer)` (§6.6/§4.4) — a resumable maintenance op as a
    /// sequence of journaled activities.** Runs `step_count` maintenance steps, EACH a journaled
    /// [`WfCtx::activity`]: the step's side effect runs exactly once, the `activity_completed` row is
    /// journaled, and a crash mid-sequence **replays to the un-journaled step** (§4.1) — the journaled
    /// prefix short-circuits with 0 re-execution and the sequence resumes from the first un-journaled
    /// step.
    ///
    /// **The replay property (§4.1, the FLOW-D1 property on a maintenance workflow):** a crash mid-
    /// repack (say after step 3 of 8 journaled) re-drives to find steps 0..=3 already journaled — they
    /// SHORT-CIRCUIT (the pack rewrite is NOT re-run), and the run resumes at step 4. No re-executed
    /// side effect; the maintenance is resumable, not restart-from-scratch.
    ///
    /// **Returns** the count of steps RAN this drive (the LIVE steps — the journaled-prefix steps that
    /// short-circuited are NOT counted), or a [`crate::WfError`] if a step exhausted its retries / the
    /// body diverged from its journal.
    ///
    /// **NAMED FLOOR:** the call site (Git's GC / repack / bundle-gen) is Git's M3 prompt (GIT-D9 is
    /// Git's gate); this is the myelin-flow helper. A HEAVY op (a multi-hour repack) should ride
    /// [`WfCtx::run_heavy_maintenance`] (the long-park form) instead of this synchronous form.
    pub fn run_maintenance<P>(
        &mut self,
        op: MaintenanceOp,
        step_count: usize,
        performer: &P,
    ) -> WfResult<usize>
    where
        P: MaintenancePerformer,
    {
        let ran_before = self.side_effects_executed();
        for step_index in 0..step_count {
            // Each step is an ordinary journaled activity. On replay the activity SHORT-CIRCUITS the
            // journaled step (the side effect is NOT re-run, §4.1); only the first un-journaled step
            // executes live. The closure performs the step's side effect, keyed on the activity's
            // deterministic BUS-2 idem_token so the consumer's own downstream write is idempotent.
            let marker = maintenance_step_marker(op, step_index);
            self.activity(RetryPolicy::default_policy(), move |idem, _attempt| {
                performer.perform_step(op, step_index, idem)?;
                Ok(vec![marker.clone()])
            })?;
        }
        // The steps RAN this drive = the side-effect delta (the journaled prefix short-circuited, so it
        // did NOT bump the side-effect counter — §4.1). This is the resumable proof: a re-drive of a
        // partially-journaled maintenance runs only the un-journaled tail.
        Ok((self.side_effects_executed() - ran_before) as usize)
    }

    /// **`run_heavy_maintenance(op, target, runner, timeout)` (§6.6/§4.9) — a HEAVY maintenance op as a
    /// `SCHEDULE_AND_RUN_JOB` long-park.** The heavy ops (a multi-hour repack of a giant repo) ride the
    /// long-park idiom instead of a synchronous activity: dispatch the job to the unified runner +
    /// park on `job.done`, holding NO runtime while the runner works for hours. A crash while parked
    /// replays to the wait (the dispatch short-circuits — the job is NOT re-dispatched, §4.1).
    ///
    /// `target` is the opaque job descriptor (a repo ref / a maintenance command — references-not-
    /// payloads). The dispatch into `runner` is GATED by AG-D4 (Agent-Fabric / CI-owned — no untrusted
    /// code until the sandbox-escape gate is green). Returns the [`JobOutcome`] (Completed / Parked /
    /// TimedOut) of the long-park.
    pub fn run_heavy_maintenance<R>(
        &mut self,
        op: MaintenanceOp,
        target: impl Into<String>,
        runner: &R,
        timeout_secs: Option<i64>,
    ) -> WfResult<JobOutcome>
    where
        R: JobRunner,
    {
        // A heavy maintenance op is a unified-runner job (kind=ci — it runs on the CI runner pool's
        // batch lane, not an agent lane). The dispatch + park + idempotent completion is the existing
        // §4.9 idiom — no new primitive. The job target carries the op + the opaque repo descriptor.
        let spec = JobSpec::new(
            JobKind::Ci,
            format!("maintenance:{}:{}", op.as_str(), target.into()),
        );
        self.schedule_and_run_job(spec, runner, timeout_secs)
    }

    /// **`run_history_rewrite_invalidation(namespaces, performer)` (§6.6, contract 10.6/11.2) — the
    /// history-rewrite invalidation fan-out as a sequence of journaled activities.** After Git performs
    /// the audited history-rewrite (the commit-body expunge, contract 10.6), the caches keyed on the
    /// old history must be invalidated across the trust-scoped cache namespaces (contract 11.2). The
    /// fan-out is a **sequence of journaled activities — one journaled step per namespace** — so a
    /// crash mid-fan-out **replays from the last journaled step** (§4.1): the namespaces already
    /// invalidated SHORT-CIRCUIT (they are NOT re-invalidated), and the fan-out resumes from the first
    /// un-invalidated namespace.
    ///
    /// The `namespaces` visit order MUST be stable across re-drives (use [`CacheNamespace::FANOUT_ORDER`]
    /// for the full fan-out) so the journaled prefix lines up step-for-step on replay. Each namespace's
    /// invalidation runs as one journaled activity (the trust scope is its own erasable step — an
    /// `UntrustedFork` write cannot reach the trusted scope, contract 11.2).
    ///
    /// **Returns** the count of namespaces invalidated LIVE this drive (the journaled prefix that
    /// short-circuited is NOT counted) — so a re-drive of a partial fan-out invalidates only the
    /// un-journaled tail.
    pub fn run_history_rewrite_invalidation<P>(
        &mut self,
        namespaces: &[CacheNamespace],
        performer: &P,
    ) -> WfResult<usize>
    where
        P: MaintenancePerformer,
    {
        let ran_before = self.side_effects_executed();
        for &namespace in namespaces {
            // Each namespace's invalidation is its own journaled activity. On replay the already-
            // invalidated namespaces SHORT-CIRCUIT (0 re-invalidation, §4.1); the fan-out resumes from
            // the first un-journaled namespace — replay-FROM-the-last-journaled-step.
            let marker = invalidation_marker(namespace);
            self.activity(RetryPolicy::default_policy(), move |idem, _attempt| {
                performer.invalidate_namespace(namespace, idem)?;
                Ok(vec![marker.clone()])
            })?;
        }
        Ok((self.side_effects_executed() - ran_before) as usize)
    }
}

/// **The references-not-payloads marker a journaled maintenance step carries (§6.6).** A single
/// [`myelin_refs::ArtifactRef`] encoding the op + the step index — no PII, a machine token recording
/// WHICH maintenance step was performed, so a journal / holder scan can attribute the maintenance.
/// Exposed so a consumer fixture / a journal-attribution scan can reconstruct the same marker.
pub fn maintenance_step_marker(op: MaintenanceOp, step_index: usize) -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef(format!("maintenance:{}:step:{step_index}", op.as_str()))
}

/// **The references-not-payloads marker a journaled invalidation step carries (§6.6, contract 11.2).**
/// A single [`myelin_refs::ArtifactRef`] encoding the invalidated cache namespace — no PII, a machine
/// token recording WHICH trust-scoped namespace was invalidated, so the fan-out's journal attributes
/// each step. Exposed so a consumer fixture can reconstruct the same marker.
pub fn invalidation_marker(namespace: CacheNamespace) -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef(format!(
        "history-rewrite:invalidated:{}",
        namespace.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history_kind, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
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
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }
    fn begin(outbox: &OutboxStore, journal: WfJournal) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "git.maintenance",
            "2026-06-21T00:00:00Z",
            7,
        )
    }
    fn resume(
        outbox: &OutboxStore,
        journal: WfJournal,
        history: Vec<crate::schema::WfHistoryRow>,
    ) -> WfCtx {
        WfCtx::resume(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "git.maintenance",
            "2026-06-21T00:00:00Z",
            7,
            history,
        )
    }

    /// **A recording maintenance performer (Git's consumer side).** RECORDS each performed step / each
    /// invalidated namespace (so a test asserts which RAN vs replayed) and counts the calls (so a
    /// replay's 0-re-execution is provable). `fail_step` makes that step index fail once (to drive the
    /// activity retry).
    #[derive(Default)]
    struct RecordingPerformer {
        steps: Mutex<Vec<usize>>,
        namespaces: Mutex<Vec<CacheNamespace>>,
        step_calls: AtomicUsize,
        ns_calls: AtomicUsize,
        fail_step_once: Option<usize>,
        failed: Mutex<Vec<usize>>,
    }
    impl MaintenancePerformer for RecordingPerformer {
        fn perform_step(
            &self,
            _op: MaintenanceOp,
            step_index: usize,
            _idem: &str,
        ) -> Result<(), ActivityError> {
            self.step_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_step_once == Some(step_index) {
                let mut failed = self.failed.lock().unwrap();
                if !failed.contains(&step_index) {
                    failed.push(step_index);
                    return Err(ActivityError("transient maintenance failure".into()));
                }
            }
            self.steps.lock().unwrap().push(step_index);
            Ok(())
        }
        fn invalidate_namespace(
            &self,
            namespace: CacheNamespace,
            _idem: &str,
        ) -> Result<(), ActivityError> {
            self.ns_calls.fetch_add(1, Ordering::SeqCst);
            self.namespaces.lock().unwrap().push(namespace);
            Ok(())
        }
    }

    /// **A resumable maintenance op runs each step as a journaled activity (§6.6/§4.4).** An 8-step
    /// repack runs all 8 steps in one drive, each journaled `activity_completed` carrying the step
    /// marker.
    #[test]
    fn maintenance_runs_each_step_as_a_journaled_activity() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let performer = RecordingPerformer::default();

        let mut ctx = begin(&outbox, journal.clone());
        let ran = ctx
            .run_maintenance(MaintenanceOp::Repack, 8, &performer)
            .expect("the repack runs");
        assert_eq!(ran, 8, "all 8 steps ran live this drive");
        assert_eq!(
            *performer.steps.lock().unwrap(),
            vec![0, 1, 2, 3, 4, 5, 6, 7]
        );
        ctx.commit().expect("co-commit the journaled steps");

        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 8, "8 journaled activity_completed rows");
        assert!(
            hist.iter()
                .all(|r| r.kind == history_kind::ACTIVITY_COMPLETED),
            "each step is a journaled activity"
        );
        assert_eq!(
            hist[0].result.as_ref().unwrap()[0],
            maintenance_step_marker(MaintenanceOp::Repack, 0),
            "the journaled step carries the references-not-payloads marker"
        );
    }

    /// **THE CORE PROPERTY: a crash mid-repack replays to the un-journaled step with 0 re-execution
    /// (§4.1).** Drive 1 journals steps 0..=2 then crashes (only 3 of 8 committed). Drive 2 re-drives
    /// the SAME body: steps 0..=2 SHORT-CIRCUIT (the pack rewrite is NOT re-run), and the maintenance
    /// resumes at step 3 — running only 3..=7. 0 re-executed side effect.
    #[test]
    fn crash_mid_repack_replays_to_the_un_journaled_step_zero_re_execution() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();

        // DRIVE 1: run a 3-step prefix (the worker journaled 3 steps, then crashed).
        let performer1 = RecordingPerformer::default();
        let mut c1 = begin(&outbox, journal.clone());
        let ran1 = c1
            .run_maintenance(MaintenanceOp::Repack, 3, &performer1)
            .expect("drive 1");
        assert_eq!(ran1, 3, "3 steps ran before the crash");
        c1.commit()
            .expect("the 3 steps co-commit (durable before the crash)");
        assert_eq!(*performer1.steps.lock().unwrap(), vec![0, 1, 2]);
        let history = journal.history_for(&tenant(), "R1");
        assert_eq!(history.len(), 3, "3 journaled at the crash point");

        // DRIVE 2 (re-drive): the FULL 8-step body. Steps 0..=2 replay (short-circuit), 3..=7 run live.
        let performer2 = RecordingPerformer::default();
        let mut c2 = resume(&outbox, journal.clone(), history);
        let ran2 = c2
            .run_maintenance(MaintenanceOp::Repack, 8, &performer2)
            .expect("the resume drive");

        assert_eq!(
            ran2, 5,
            "resumed at step 3 — only steps 3..=7 ran live (5 steps)"
        );
        assert_eq!(
            *performer2.steps.lock().unwrap(),
            vec![3, 4, 5, 6, 7],
            "0..=2 replayed (0 re-execution), 3..=7 ran — replay to the un-journaled step"
        );
        assert_eq!(
            performer2.step_calls.load(Ordering::SeqCst),
            5,
            "0 re-executed side effect — the journaled prefix's perform_step was NEVER called"
        );
        c2.commit().expect("co-commit the resumed tail");
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            8,
            "8 journaled, 0 lost, 0 duplicate"
        );
    }

    /// **The history-rewrite invalidation fan-out is a journaled sequence (§6.6, contract 11.2).** The
    /// full fan-out over the 3 trust-scoped namespaces journals one activity per namespace, in the
    /// FROZEN order.
    #[test]
    fn invalidation_fan_out_is_a_journaled_sequence() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let performer = RecordingPerformer::default();

        let mut ctx = begin(&outbox, journal.clone());
        let invalidated = ctx
            .run_history_rewrite_invalidation(&CacheNamespace::FANOUT_ORDER, &performer)
            .expect("the fan-out runs");
        assert_eq!(invalidated, 3, "all 3 namespaces invalidated live");
        assert_eq!(
            *performer.namespaces.lock().unwrap(),
            vec![
                CacheNamespace::Fork,
                CacheNamespace::Mirror,
                CacheNamespace::CloneBundle
            ],
            "the fan-out visits the trust-scoped namespaces in the FROZEN order"
        );
        ctx.commit().expect("co-commit the fan-out");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 3, "3 journaled invalidation activities");
        assert_eq!(
            hist[0].result.as_ref().unwrap()[0],
            invalidation_marker(CacheNamespace::Fork),
            "the journaled step carries the namespace marker"
        );
    }

    /// **THE FAN-OUT REPLAYS FROM THE LAST JOURNALED STEP (§4.1, contract 11.2).** Drive 1 invalidates
    /// the Fork namespace then crashes (1 of 3 journaled). Drive 2 re-drives the full fan-out: Fork
    /// SHORT-CIRCUITS (NOT re-invalidated), and the fan-out resumes from Mirror — invalidating only
    /// Mirror + CloneBundle. 0 re-invalidation of an already-purged trust scope.
    #[test]
    fn invalidation_fan_out_replays_from_the_last_journaled_step() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();

        // DRIVE 1: invalidate only the Fork namespace (then crash).
        let performer1 = RecordingPerformer::default();
        let mut c1 = begin(&outbox, journal.clone());
        let n1 = c1
            .run_history_rewrite_invalidation(&[CacheNamespace::Fork], &performer1)
            .expect("drive 1");
        assert_eq!(n1, 1, "Fork invalidated before the crash");
        c1.commit().expect("the Fork invalidation co-commits");
        let history = journal.history_for(&tenant(), "R1");
        assert_eq!(history.len(), 1, "1 journaled at the crash point");

        // DRIVE 2 (re-drive): the FULL 3-namespace fan-out. Fork replays (short-circuit), Mirror +
        // CloneBundle run live.
        let performer2 = RecordingPerformer::default();
        let mut c2 = resume(&outbox, journal.clone(), history);
        let n2 = c2
            .run_history_rewrite_invalidation(&CacheNamespace::FANOUT_ORDER, &performer2)
            .expect("the resume drive");

        assert_eq!(
            n2, 2,
            "resumed from Mirror — only Mirror + CloneBundle ran live"
        );
        assert_eq!(
            *performer2.namespaces.lock().unwrap(),
            vec![CacheNamespace::Mirror, CacheNamespace::CloneBundle],
            "Fork replayed (0 re-invalidation), the fan-out resumed from the last journaled step"
        );
        assert_eq!(
            performer2.ns_calls.load(Ordering::SeqCst),
            2,
            "0 re-invalidation — the already-purged Fork scope's invalidate was NEVER re-called"
        );
        c2.commit().expect("co-commit the resumed fan-out tail");
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            3,
            "3 journaled, 0 duplicate"
        );
    }

    /// **A failed maintenance step RETRIES (the step is an ordinary activity, §4.4).** Step 2 fails
    /// once (transiently); the activity retries it and the second attempt succeeds — the step is
    /// journaled exactly once.
    #[test]
    fn a_failed_maintenance_step_retries() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let performer = RecordingPerformer {
            fail_step_once: Some(2),
            ..Default::default()
        };

        let mut ctx = begin(&outbox, journal.clone());
        let ran = ctx
            .run_maintenance(MaintenanceOp::Gc, 4, &performer)
            .expect("the gc runs despite a transient failure");
        assert_eq!(ran, 4, "all 4 steps complete (step 2 retried)");
        assert_eq!(
            *performer.steps.lock().unwrap(),
            vec![0, 1, 2, 3],
            "step 2 succeeded on retry"
        );
        // 4 steps + 1 retry of step 2 = 5 perform_step calls.
        assert_eq!(
            performer.step_calls.load(Ordering::SeqCst),
            5,
            "one retry of step 2"
        );
        ctx.commit().expect("co-commit");
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            4,
            "step 2 journaled exactly once"
        );
    }

    /// **A heavy maintenance op rides the `SCHEDULE_AND_RUN_JOB` long-park (§6.6/§4.9).** A repack
    /// dispatched as a heavy job parks on `job.done` holding no runtime — the dispatch returns Parked.
    #[test]
    fn heavy_maintenance_rides_the_long_park() {
        use crate::engine::SignalStore;

        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();

        struct AcceptingRunner {
            dispatched: Mutex<Vec<JobSpec>>,
        }
        impl JobRunner for AcceptingRunner {
            fn dispatch(&self, spec: &JobSpec) -> Result<(), ActivityError> {
                self.dispatched.lock().unwrap().push(spec.clone());
                Ok(())
            }
        }
        let runner = AcceptingRunner {
            dispatched: Mutex::new(Vec::new()),
        };

        let mut ctx = begin(&outbox, journal).with_signals(signals);
        let out = ctx
            .run_heavy_maintenance(
                MaintenanceOp::Repack,
                "repo://acme/giant",
                &runner,
                Some(7200),
            )
            .expect("dispatch + park");
        assert_eq!(
            out,
            JobOutcome::Parked,
            "the heavy repack parks on job.done (holds no runtime)"
        );
        assert!(ctx.parked_on_signal(), "the run is waiting on the runner");
        let dispatched = runner.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1, "one heavy job dispatched");
        assert_eq!(
            dispatched[0].target, "maintenance:repack:repo://acme/giant",
            "the job target carries the op + the opaque repo descriptor"
        );
        assert_eq!(
            dispatched[0].kind,
            JobKind::Ci,
            "a heavy maintenance job runs on the CI batch lane"
        );
    }

    /// The op / namespace machine tokens are stable (no PII; the journaled markers depend on them).
    #[test]
    fn op_and_namespace_tokens_are_stable() {
        assert_eq!(MaintenanceOp::Gc.as_str(), "gc");
        assert_eq!(MaintenanceOp::Repack.as_str(), "repack");
        assert_eq!(MaintenanceOp::BundleGen.as_str(), "bundle-gen");
        assert_eq!(MaintenanceOp::HistoryRewrite.as_str(), "history-rewrite");
        assert_eq!(CacheNamespace::Fork.as_str(), "fork");
        assert_eq!(CacheNamespace::Mirror.as_str(), "mirror");
        assert_eq!(CacheNamespace::CloneBundle.as_str(), "clone-bundle");
        assert_eq!(
            CacheNamespace::FANOUT_ORDER.len(),
            3,
            "the three trust-scoped namespaces"
        );
    }
}
