use crate::wfctx::{WaitOutcome, WfCtx, WfError, WfResult};
use myelin_events::check_seam::{CiOverall, CiResult};
use myelin_events::{
    AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EventDraft, EventType, Visibility,
};
use myelin_refs::ArtifactRef;

pub const CI_RESULT_SIGNAL: &str = myelin_events::check_seam::CiResultWaitSubstrate::SIGNAL_NAME;

pub const GIT_PR_MERGED_EVENT: &str = "git.pr.merged";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeRequest {
    pub pr_ref: String,
    pub target_ref: String,
    pub speculative_commit_oid: String,
    pub required_contexts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiDispatch {
    pub commit_oid: String,
    pub required_contexts: Vec<String>,
    pub merge_attempt_id: String,
}

pub trait CiDispatcher {
    fn dispatch(&self, ci: &CiDispatch) -> Result<(), crate::ActivityError>;
}

pub trait MergePerformer {
    fn merge(&self, request: &MergeRequest) -> Result<String, crate::ActivityError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    Merged {
        merge_attempt_id: String,
        merged_commit_oid: String,
    },
    Dequeued {
        reason: String,
    },
    Parked,
    TimedOut,
}

pub fn humanise_dequeue_reason(cause: DequeueCause) -> String {
    match cause {
        DequeueCause::CiFailure { ref failing } if !failing.is_empty() => format!(
            "CI failed: the required check(s) {} did not pass. The pull request was removed from \
             the merge queue; push a fix and re-queue.",
            failing.join(", ")
        ),
        DequeueCause::CiFailure { .. } => "CI reported a failure for this pull request. It was \
             removed from the merge queue; push a fix and re-queue."
            .to_string(),
        DequeueCause::MissingRequiredContext { ref missing } => format!(
            "CI did not report the required check(s) {}. The pull request was removed from the \
             merge queue; ensure those checks run and re-queue.",
            missing.join(", ")
        ),
        DequeueCause::CiVanished => "CI did not report a result before the time limit (the run \
             may have stalled). The pull request was removed from the merge queue; re-queue to try \
             again."
            .to_string(),
        DequeueCause::MergeConflict => "The merge could not be completed (the branch likely \
             conflicts with the target). The pull request was removed from the merge queue; rebase \
             and re-queue."
            .to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DequeueCause {
    CiFailure { failing: Vec<String> },
    MissingRequiredContext { missing: Vec<String> },
    CiVanished,
    MergeConflict,
}

pub fn merge_attempt_id(run_id: &str, dispatch_command_id: &str) -> String {
    format!("{run_id}/{dispatch_command_id}/merge")
}

const CI_RESULT_VERDICT_PREFIX: &str = "ci.result:verdict:";
const CI_RESULT_CONTEXT_PREFIX: &str = "ci.result:context:";
const CI_RESULT_COMMIT_PREFIX: &str = "ci.result:commit:";

pub fn encode_ci_result(result: &CiResult) -> Vec<ArtifactRef> {
    let mut refs = Vec::with_capacity(result.contexts.len() + 2);
    let verdict = match result.overall {
        CiOverall::Success => "success",
        CiOverall::Failure => "failure",
    };
    refs.push(ArtifactRef(format!("{CI_RESULT_VERDICT_PREFIX}{verdict}")));
    refs.push(ArtifactRef(format!(
        "{CI_RESULT_COMMIT_PREFIX}{}",
        result.commit_oid
    )));
    for ctx in &result.contexts {
        refs.push(ArtifactRef(format!("{CI_RESULT_CONTEXT_PREFIX}{ctx}")));
    }
    refs
}

pub fn decode_ci_result(refs: &[ArtifactRef], idem_token: &str) -> Option<CiResult> {
    let mut overall = None;
    let mut commit_oid = String::new();
    let mut contexts = Vec::new();
    for r in refs {
        if let Some(v) = r.0.strip_prefix(CI_RESULT_VERDICT_PREFIX) {
            overall = match v {
                "success" => Some(CiOverall::Success),
                "failure" => Some(CiOverall::Failure),
                _ => return None,
            };
        } else if let Some(c) = r.0.strip_prefix(CI_RESULT_COMMIT_PREFIX) {
            commit_oid = c.to_string();
        } else if let Some(ctx) = r.0.strip_prefix(CI_RESULT_CONTEXT_PREFIX) {
            contexts.push(ctx.to_string());
        }
    }
    Some(CiResult {
        commit_oid,
        overall: overall?,
        contexts,
        idem_token: idem_token.to_string(),
    })
}

impl WfCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn run_merge_attempt<D, M>(
        &mut self,
        request: &MergeRequest,
        ci: &D,
        merger: &M,
        timeout_secs: Option<i64>,
        cost: myelin_storage::reserve_settle::MicroUsd,
        units: Vec<myelin_storage::reserve_settle::MeteredUnit>,
    ) -> WfResult<MergeOutcome>
    where
        D: CiDispatcher,
        M: MergePerformer,
    {
        let dispatch_command_id = self.peek_next_command_id();
        let attempt_id = merge_attempt_id(self.run_id(), &dispatch_command_id);

        let dispatch = CiDispatch {
            commit_oid: request.speculative_commit_oid.clone(),
            required_contexts: request.required_contexts.clone(),
            merge_attempt_id: attempt_id.clone(),
        };
        let marker = ci_dispatch_marker(&attempt_id, &request.speculative_commit_oid);
        let dispatch_for_closure = dispatch.clone();
        let marker_for_closure = marker.clone();

        self.metered_activity(
            crate::RetryPolicy::default_policy(),
            cost,
            units,
            move |_act_idem, _attempt| {
                ci.dispatch(&dispatch_for_closure)?;
                Ok(vec![marker_for_closure.clone()])
            },
        )?;

        let outcome = self.wait_for_signal(CI_RESULT_SIGNAL, timeout_secs)?;

        match outcome {
            WaitOutcome::Signalled {
                idem_key,
                payload,
                payload_key_ref: _,
            } => {
                if idem_key != attempt_id {
                    return Err(WfError::CoCommit(format!(
                        "ci.result idem_key `{idem_key}` does not match the dispatched \
                         merge_attempt_id `{attempt_id}` (CI did not echo the no-coordination dedup \
                         key, §6.5)"
                    )));
                }
                let result = decode_ci_result(&payload, &idem_key).ok_or_else(|| {
                    WfError::CoCommit(format!(
                        "ci.result for merge_attempt `{attempt_id}` carried no decodable verdict \
                         (a producer protocol violation, §6.5)"
                    ))
                })?;

                if result.overall == CiOverall::Failure {
                    let failing: Vec<String> = request.required_contexts.clone();
                    let reason = humanise_dequeue_reason(DequeueCause::CiFailure { failing });
                    return Ok(MergeOutcome::Dequeued { reason });
                }
                let missing: Vec<String> = request
                    .required_contexts
                    .iter()
                    .filter(|req| !result.contexts.iter().any(|c| c == *req))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    let reason =
                        humanise_dequeue_reason(DequeueCause::MissingRequiredContext { missing });
                    return Ok(MergeOutcome::Dequeued { reason });
                }

                let request_for_merge = request.clone();
                let merge_result = self.activity(
                    crate::RetryPolicy::default_policy(),
                    move |_act_idem, _attempt| {
                        let oid = merger.merge(&request_for_merge)?;
                        Ok(vec![ArtifactRef(format!("git:merged:{oid}"))])
                    },
                );
                let merged_commit_oid = match merge_result {
                    Ok(refs) => refs
                        .first()
                        .and_then(|r| r.0.strip_prefix("git:merged:"))
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    Err(WfError::ActivityExhausted(_)) => {
                        let reason = humanise_dequeue_reason(DequeueCause::MergeConflict);
                        return Ok(MergeOutcome::Dequeued { reason });
                    }
                    Err(other) => return Err(other),
                };

                self.emit(git_pr_merged_draft(request, &merged_commit_oid), None)?;

                Ok(MergeOutcome::Merged {
                    merge_attempt_id: attempt_id,
                    merged_commit_oid,
                })
            }
            WaitOutcome::Parked => Ok(MergeOutcome::Parked),
            WaitOutcome::TimedOut => Ok(MergeOutcome::TimedOut),
        }
    }
}

pub fn ci_dispatch_marker(merge_attempt_id: &str, commit_oid: &str) -> ArtifactRef {
    ArtifactRef(format!("ci:dispatched:{merge_attempt_id}:{commit_oid}"))
}

pub fn git_pr_merged_draft(request: &MergeRequest, merged_commit_oid: &str) -> EventDraft {
    EventDraft {
        type_: EventType(GIT_PR_MERGED_EVENT.to_string()),
        subject: EvArtifactRef(request.pr_ref.clone()),
        aggregate: AggregateKey(request.pr_ref.clone()),
        payload: serde_json::json!({
            "pr_ref": request.pr_ref,
            "target_ref": request.target_ref,
            "merged_commit_oid": merged_commit_oid,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub struct MockCiResultProducer<'a> {
    signals: &'a crate::SignalStore,
    tenant: myelin_tenancy::TenantId,
    region: myelin_tenancy::Region,
    run_id: String,
}

impl<'a> MockCiResultProducer<'a> {
    pub fn new(
        signals: &'a crate::SignalStore,
        tenant: myelin_tenancy::TenantId,
        region: myelin_tenancy::Region,
        run_id: impl Into<String>,
    ) -> Self {
        Self {
            signals,
            tenant,
            region,
            run_id: run_id.into(),
        }
    }

    pub fn deliver(
        &self,
        merge_attempt_id: &str,
        commit_oid: &str,
        overall: CiOverall,
        contexts: Vec<String>,
    ) -> bool {
        let result = CiResult {
            commit_oid: commit_oid.to_string(),
            overall,
            contexts,
            idem_token: merge_attempt_id.to_string(),
        };
        self.signals.deliver(crate::SignalRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: self.run_id.clone(),
            signal_name: CI_RESULT_SIGNAL.to_string(),
            idem_key: merge_attempt_id.to_string(),
            payload: encode_ci_result(&result),
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        })
    }
}

pub struct RealCiResultProducer<'a> {
    signals: &'a crate::SignalStore,
    tenant: myelin_tenancy::TenantId,
    region: myelin_tenancy::Region,
    run_id: String,
    repo: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckFact {
    pub context: String,
    pub run_attempt: u64,
    pub success: bool,
    pub seq: u64,
}

impl<'a> RealCiResultProducer<'a> {
    pub fn new(
        signals: &'a crate::SignalStore,
        tenant: myelin_tenancy::TenantId,
        region: myelin_tenancy::Region,
        run_id: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            signals,
            tenant,
            region,
            run_id: run_id.into(),
            repo: repo.into(),
        }
    }

    pub fn rollup(
        &self,
        commit_oid: &str,
        facts: &[CheckFact],
        required_contexts: &[String],
        merge_attempt_id: &str,
    ) -> CiResult {
        use myelin_events::check_seam::{check_updated_draft, rollup_ci_result, CheckSeamOrder};
        use myelin_events::{Actor, CorrelationId, EventEnvelope, EventId, Timestamp};
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use std::collections::BTreeMap;

        let ci_actor = Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            self.tenant.clone(),
        ));

        let mut order = CheckSeamOrder::new(&self.repo, commit_oid);
        for fact in facts {
            let draft = check_updated_draft(
                &self.repo,
                commit_oid,
                &fact.context,
                serde_json::json!({
                    "context": fact.context,
                    "run_attempt": fact.run_attempt,
                    "state": if fact.success { "success" } else { "failure" },
                }),
            );
            let env = EventEnvelope {
                event_id: EventId(format!("evt-{}-{}", fact.context, fact.run_attempt)),
                type_: draft.type_,
                schema_ver: 1,
                tenant: self.tenant.clone(),
                region: self.region.clone(),
                actor: ci_actor.clone(),
                subject: draft.subject,
                aggregate: draft.aggregate,
                causation_id: None,
                correlation_id: CorrelationId(format!("corr-{commit_oid}")),
                caused_by: None,
                depth: 0,
                contains_personal_data: draft.contains_personal_data,
                data_role: draft.data_role,
                visibility: draft.visibility,
                pii_key_ref: draft.pii_key_ref,
                occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
                recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
                payload: draft.payload,
            };
            let _ = order.ingest(&env, fact.seq);
        }

        let mut current: BTreeMap<String, (u64, bool)> = BTreeMap::new();
        for check in order.in_order() {
            let attempt = check.check_status["run_attempt"].as_u64().unwrap_or(0);
            let success = check.check_status["state"].as_str() == Some("success");
            let ctx = check
                .subject
                .0
                .rsplit_once("/check-")
                .map(|(_, c)| c.to_string())
                .unwrap_or_default();
            current
                .entry(ctx)
                .and_modify(|(a, s)| {
                    if attempt >= *a {
                        *a = attempt;
                        *s = success;
                    }
                })
                .or_insert((attempt, success));
        }
        let post_supersession: BTreeMap<String, bool> =
            current.into_iter().map(|(c, (_, s))| (c, s)).collect();

        rollup_ci_result(
            commit_oid,
            &post_supersession,
            required_contexts,
            merge_attempt_id,
        )
    }

    pub fn deliver(
        &self,
        commit_oid: &str,
        facts: &[CheckFact],
        required_contexts: &[String],
        merge_attempt_id: &str,
    ) -> bool {
        let result = self.rollup(commit_oid, facts, required_contexts, merge_attempt_id);
        self.signals.deliver(crate::SignalRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: self.run_id.clone(),
            signal_name: CI_RESULT_SIGNAL.to_string(),
            idem_key: merge_attempt_id.to_string(),
            payload: encode_ci_result(&result),
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::SignalStore;
    use crate::WfJournal;
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::reserve_settle::MicroUsd;
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
    fn begin(outbox: &OutboxStore, journal: WfJournal, signals: SignalStore) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
    }

    fn request() -> MergeRequest {
        MergeRequest {
            pr_ref: "myelin://acme/git/repo/core#pr-7".into(),
            target_ref: "refs/heads/main".into(),
            speculative_commit_oid: "deadbeef".into(),
            required_contexts: vec!["build".into(), "test".into()],
        }
    }

    #[derive(Default)]
    struct RecordingCi {
        dispatched: Mutex<Vec<CiDispatch>>,
        calls: AtomicUsize,
        fail_first: bool,
    }
    impl CiDispatcher for RecordingCi {
        fn dispatch(&self, ci: &CiDispatch) -> Result<(), crate::ActivityError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && n == 0 {
                return Err(crate::ActivityError::retryable(
                    "CI runner transiently unreachable",
                ));
            }
            self.dispatched.lock().unwrap().push(ci.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingMerger {
        merges: AtomicUsize,
        conflict: bool,
    }
    impl MergePerformer for RecordingMerger {
        fn merge(&self, request: &MergeRequest) -> Result<String, crate::ActivityError> {
            self.merges.fetch_add(1, Ordering::SeqCst);
            if self.conflict {
                return Err(crate::ActivityError::retryable("merge conflict"));
            }
            Ok(format!("merged-{}", request.speculative_commit_oid))
        }
    }

    fn no_cost() -> (MicroUsd, Vec<myelin_storage::reserve_settle::MeteredUnit>) {
        (MicroUsd(0), vec![])
    }

    #[test]
    fn dispatch_mints_deterministic_attempt_id_and_parks() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("dispatch + park");
        assert_eq!(
            out,
            MergeOutcome::Parked,
            "no ci.result yet → the run parks"
        );
        assert!(
            ctx.parked_on_signal(),
            "parked on ci.result (holds no runtime)"
        );
        assert_eq!(
            ci.calls.load(Ordering::SeqCst),
            1,
            "CI dispatched exactly once"
        );
        assert_eq!(
            merger.merges.load(Ordering::SeqCst),
            0,
            "no merge - CI still running"
        );

        let dispatched = ci.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1);
        let consumer_id = merge_attempt_id("R1", "merge.queue:0");
        assert_eq!(
            dispatched[0].merge_attempt_id, consumer_id,
            "producer + consumer derive the SAME merge_attempt_id without coordination"
        );
        assert_eq!(dispatched[0].merge_attempt_id, "R1/merge.queue:0/merge");
    }

    #[test]
    fn success_for_all_required_contexts_merges_and_emits_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(
            &attempt,
            "deadbeef",
            CiOverall::Success,
            vec!["build".into(), "test".into()],
        );

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("dispatch + merge");
        match out {
            MergeOutcome::Merged {
                merge_attempt_id: id,
                merged_commit_oid,
            } => {
                assert_eq!(id, attempt, "CI echoed the dispatch id");
                assert_eq!(merged_commit_oid, "merged-deadbeef");
            }
            other => panic!("expected Merged, got {other:?}"),
        }
        assert_eq!(merger.merges.load(Ordering::SeqCst), 1, "EXACTLY one merge");
        assert_eq!(
            ctx.staged_emit_len(),
            1,
            "EXACTLY one git.pr.merged emitted"
        );
    }

    #[test]
    fn double_delivered_ci_result_wakes_once_zero_double_merge() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        let first = producer.deliver(
            &attempt,
            "deadbeef",
            CiOverall::Success,
            vec!["build".into(), "test".into()],
        );
        let second = producer.deliver(
            &attempt,
            "deadbeef",
            CiOverall::Success,
            vec!["build".into(), "test".into()],
        );
        assert!(first, "first delivery is new");
        assert!(
            !second,
            "the at-least-once double-delivery deduped (ON CONFLICT DO NOTHING)"
        );
        assert_eq!(signals.buffered_depth(), 1, "ONE buffered row");

        let mut ctx = begin(&outbox, journal, signals.clone());
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("merge");
        assert!(
            matches!(out, MergeOutcome::Merged { .. }),
            "merged, got {out:?}"
        );
        assert_eq!(ctx.consumed_signals().len(), 1, "ONE wake per attempt");
        assert_eq!(merger.merges.load(Ordering::SeqCst), 1, "0 double-merge");
        assert_eq!(ctx.staged_emit_len(), 1, "ONE git.pr.merged");
    }

    #[test]
    fn ci_failure_dequeues_with_humanised_reason() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(
            &attempt,
            "deadbeef",
            CiOverall::Failure,
            vec!["build".into(), "test".into()],
        );

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("dequeue");
        match out {
            MergeOutcome::Dequeued { reason } => {
                assert!(reason.contains("CI failed"), "humanised: {reason}");
                assert!(
                    reason.contains("build"),
                    "names the failing checks: {reason}"
                );
                assert!(
                    !reason.contains("ActivityError"),
                    "no raw error code: {reason}"
                );
            }
            other => panic!("expected Dequeued, got {other:?}"),
        }
        assert_eq!(
            merger.merges.load(Ordering::SeqCst),
            0,
            "no merge on failure"
        );
        assert_eq!(ctx.staged_emit_len(), 0, "no git.pr.merged on failure");
    }

    #[test]
    fn success_missing_a_required_context_dequeues() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(
            &attempt,
            "deadbeef",
            CiOverall::Success,
            vec!["build".into()],
        );

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("dequeue");
        match out {
            MergeOutcome::Dequeued { reason } => {
                assert!(
                    reason.contains("test"),
                    "names the missing required context: {reason}"
                );
            }
            other => panic!("expected Dequeued, got {other:?}"),
        }
        assert_eq!(
            merger.merges.load(Ordering::SeqCst),
            0,
            "no merge - not all required green"
        );
    }

    #[test]
    fn vanished_ci_run_times_out_and_bounds_the_wait() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::timer::TimerStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();

        let mut c1 =
            begin(&outbox, journal.clone(), signals.clone()).with_timers(timers.clone(), 0, 1000);
        let out1 = c1
            .run_merge_attempt(&request(), &ci, &merger, Some(100), MicroUsd(0), vec![])
            .expect("dispatch + park");
        assert_eq!(
            out1,
            MergeOutcome::Parked,
            "dispatched, parked with an SLA timer"
        );
        c1.commit()
            .expect("co-commit the dispatch + the timeout-timer");
        assert_eq!(
            timers.armed_count(),
            1,
            "the vanished-CI SLA timeout-timer is armed"
        );
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone())
        .with_timers(timers.clone(), 0, 2000);
        let out2 = c2
            .run_merge_attempt(&request(), &ci, &merger, Some(100), MicroUsd(0), vec![])
            .expect("the timeout drive");
        assert_eq!(
            out2,
            MergeOutcome::TimedOut,
            "the SLA fired before CI reported → TimedOut"
        );
        assert_eq!(
            ci.calls.load(Ordering::SeqCst),
            1,
            "CI dispatched ONCE - the replay short-circuit did not re-dispatch it"
        );
        assert_eq!(
            merger.merges.load(Ordering::SeqCst),
            0,
            "no merge on a vanished CI run"
        );
    }

    #[test]
    fn a_merge_conflict_dequeues() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger {
            conflict: true,
            ..Default::default()
        };

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(
            &attempt,
            "deadbeef",
            CiOverall::Success,
            vec!["build".into(), "test".into()],
        );

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
            .expect("dequeue on conflict");
        match out {
            MergeOutcome::Dequeued { reason } => {
                assert!(
                    reason.contains("merge could not be completed"),
                    "humanised: {reason}"
                );
            }
            other => panic!("expected Dequeued, got {other:?}"),
        }
        assert_eq!(
            ctx.staged_emit_len(),
            0,
            "no git.pr.merged on a failed merge"
        );
    }

    #[test]
    fn ci_result_with_mismatched_idem_key_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        producer.deliver(
            "the-wrong-attempt-id",
            "deadbeef",
            CiOverall::Success,
            vec!["build".into(), "test".into()],
        );

        let mut ctx = begin(&outbox, journal, signals);
        let err = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
            .expect_err("a mismatched ci.result idem_key is a loud error");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("does not match the dispatched")),
            "loud CoCommit, got {err:?}"
        );
    }

    #[test]
    fn a_failed_ci_dispatch_retries_reusing_the_same_attempt_id() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi {
            fail_first: true,
            ..Default::default()
        };
        let merger = RecordingMerger::default();

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
            .expect("the retried dispatch parks");
        assert_eq!(
            out,
            MergeOutcome::Parked,
            "the retried dispatch parks on ci.result"
        );
        assert_eq!(
            ci.calls.load(Ordering::SeqCst),
            2,
            "one failure + one retry"
        );
        let dispatched = ci.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1, "one accepted dispatch (the retry)");
        assert_eq!(
            dispatched[0].merge_attempt_id, "R1/merge.queue:0/merge",
            "the retry reused the SAME merge_attempt_id (CI dedups on it)"
        );
    }

    #[test]
    fn replay_short_circuits_dispatch_wait_and_merge() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(
            &attempt,
            "deadbeef",
            CiOverall::Success,
            vec!["build".into(), "test".into()],
        );

        let mut c1 = begin(&outbox, journal.clone(), signals.clone());
        let out1 = c1
            .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
            .expect("drive 1");
        assert!(matches!(out1, MergeOutcome::Merged { .. }));
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone());
        let out2 = c2
            .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
            .expect("the replay drive");
        match out2 {
            MergeOutcome::Merged {
                merge_attempt_id: id,
                ..
            } => {
                assert_eq!(id, attempt, "replay returns the SAME journaled merge")
            }
            other => panic!("expected the journaled Merged, got {other:?}"),
        }
        assert_eq!(
            ci.calls.load(Ordering::SeqCst),
            1,
            "0 RE-DISPATCH on replay"
        );
        assert_eq!(
            merger.merges.load(Ordering::SeqCst),
            1,
            "0 RE-MERGE on replay"
        );
        assert_eq!(
            c2.consumed_signals().len(),
            0,
            "replay consumed NOTHING new"
        );
    }

    #[test]
    fn ci_result_codec_round_trips() {
        let result = CiResult {
            commit_oid: "deadbeef".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: "R1/merge.queue:0/merge".into(),
        };
        let refs = encode_ci_result(&result);
        assert!(
            refs.iter().all(|r| r.0.starts_with("ci.result:")),
            "machine tokens only"
        );
        let back = decode_ci_result(&refs, "R1/merge.queue:0/merge").expect("decodable");
        assert_eq!(back, result, "encode → decode round-trips the verdict");
    }

    #[test]
    fn ci_result_with_no_verdict_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();

        let attempt = merge_attempt_id("R1", "merge.queue:0");
        signals.deliver(crate::SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: CI_RESULT_SIGNAL.into(),
            idem_key: attempt,
            payload: vec![ArtifactRef("ci.result:context:build".into())],
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        });

        let mut ctx = begin(&outbox, journal, signals);
        let err = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, MicroUsd(0), vec![])
            .expect_err("a verdict-less ci.result is a loud error");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("no decodable verdict")),
            "loud CoCommit, got {err:?}"
        );
    }

    #[test]
    fn ci_result_signal_name_is_the_named_token() {
        assert_eq!(CI_RESULT_SIGNAL, "ci.result");
    }

    #[test]
    fn git_pr_merged_draft_is_references_not_payloads() {
        let draft = git_pr_merged_draft(&request(), "merged-deadbeef");
        assert_eq!(draft.type_.0, "git.pr.merged");
        assert!(!draft.contains_personal_data, "references-not-payloads");
        assert_eq!(draft.payload["merged_commit_oid"], "merged-deadbeef");
    }

    #[test]
    fn every_dequeue_cause_humanises() {
        let causes = [
            DequeueCause::CiFailure {
                failing: vec!["build".into()],
            },
            DequeueCause::CiFailure { failing: vec![] },
            DequeueCause::MissingRequiredContext {
                missing: vec!["test".into()],
            },
            DequeueCause::CiVanished,
            DequeueCause::MergeConflict,
        ];
        for cause in causes {
            let reason = humanise_dequeue_reason(cause.clone());
            assert!(
                !reason.is_empty(),
                "humanised reason is non-empty for {cause:?}"
            );
            assert!(
                !reason.contains("ActivityError"),
                "no raw error code in {reason:?}"
            );
            assert!(
                !reason.contains("Err("),
                "no debug formatting in {reason:?}"
            );
        }
    }

    const REPO: &str = "myelin://acme/git/repo/core";

    fn required() -> Vec<String> {
        vec!["build".into(), "test".into()]
    }

    #[test]
    fn real_producer_derives_green_rollup_from_out_of_order_checks() {
        let signals = SignalStore::new();
        let producer = RealCiResultProducer::new(&signals, tenant(), region(), "R1", REPO);
        let facts = vec![
            CheckFact {
                context: "build".into(),
                run_attempt: 2,
                success: true,
                seq: 3,
            },
            CheckFact {
                context: "test".into(),
                run_attempt: 1,
                success: true,
                seq: 2,
            },
            CheckFact {
                context: "build".into(),
                run_attempt: 1,
                success: false,
                seq: 1,
            },
            CheckFact {
                context: "test".into(),
                run_attempt: 1,
                success: true,
                seq: 2,
            },
        ];
        let rollup = producer.rollup("deadbeef", &facts, &required(), "R1/merge.queue:0/merge");
        assert_eq!(
            rollup.overall,
            CiOverall::Success,
            "build's CURRENT attempt is success#2 (the stale failure#1 was superseded)"
        );
        assert_eq!(
            rollup.contexts,
            vec!["build".to_string(), "test".to_string()],
            "the rollup is over Git's required gate set, sorted (byte-stable)"
        );
        assert_eq!(rollup.idem_token, "R1/merge.queue:0/merge");
    }

    #[test]
    fn real_producer_derives_failure_on_a_superseding_failure() {
        let signals = SignalStore::new();
        let producer = RealCiResultProducer::new(&signals, tenant(), region(), "R1", REPO);
        let facts = vec![
            CheckFact {
                context: "build".into(),
                run_attempt: 1,
                success: true,
                seq: 1,
            },
            CheckFact {
                context: "test".into(),
                run_attempt: 1,
                success: true,
                seq: 2,
            },
            CheckFact {
                context: "test".into(),
                run_attempt: 2,
                success: false,
                seq: 3,
            },
        ];
        let rollup = producer.rollup("deadbeef", &facts, &required(), "R1/merge.queue:0/merge");
        assert_eq!(
            rollup.overall,
            CiOverall::Failure,
            "test's CURRENT attempt failed → the rollup is failure (0 spurious unblock)"
        );
    }

    #[test]
    fn real_producer_missing_required_context_is_failure() {
        let signals = SignalStore::new();
        let producer = RealCiResultProducer::new(&signals, tenant(), region(), "R1", REPO);
        let facts = vec![CheckFact {
            context: "build".into(),
            run_attempt: 1,
            success: true,
            seq: 1,
        }];
        let rollup = producer.rollup("deadbeef", &facts, &required(), "R1/merge.queue:0/merge");
        assert_eq!(
            rollup.overall,
            CiOverall::Failure,
            "a missing required context keeps the gate closed (fork self-green is neutral)"
        );
    }

    #[test]
    fn real_producer_rollup_is_deterministic_across_arrival_order() {
        let signals = SignalStore::new();
        let producer = RealCiResultProducer::new(&signals, tenant(), region(), "R1", REPO);
        let ordered = vec![
            CheckFact {
                context: "build".into(),
                run_attempt: 1,
                success: true,
                seq: 1,
            },
            CheckFact {
                context: "test".into(),
                run_attempt: 1,
                success: true,
                seq: 2,
            },
        ];
        let mut scrambled = ordered.clone();
        scrambled.reverse();
        let a = producer.rollup("deadbeef", &ordered, &required(), "R1/merge.queue:0/merge");
        let b = producer.rollup(
            "deadbeef",
            &scrambled,
            &required(),
            "R1/merge.queue:0/merge",
        );
        assert_eq!(
            a, b,
            "same facts, any arrival order → byte-identical rollup"
        );
    }

    #[test]
    fn real_producer_double_delivery_buffers_one_row() {
        let signals = SignalStore::new();
        let producer = RealCiResultProducer::new(&signals, tenant(), region(), "R1", REPO);
        let facts = vec![
            CheckFact {
                context: "build".into(),
                run_attempt: 1,
                success: true,
                seq: 1,
            },
            CheckFact {
                context: "test".into(),
                run_attempt: 1,
                success: true,
                seq: 2,
            },
        ];
        let attempt = "R1/merge.queue:0/merge";
        let first = producer.deliver("deadbeef", &facts, &required(), attempt);
        let second = producer.deliver("deadbeef", &facts, &required(), attempt);
        assert!(first, "first delivery is new");
        assert!(!second, "the at-least-once double-delivery deduped");
        assert_eq!(signals.buffered_depth(), 1, "ONE buffered ci.result row");
    }
}
