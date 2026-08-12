use crate::taxonomy::new_tokens::{CI_CHECK_UPDATED, CI_RESULT};
use crate::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventType, Visibility,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn check_aggregate(repo: &str, commit_oid: &str) -> AggregateKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin.check.aggregate.v1\0");
    hasher.update(repo.as_bytes());
    hasher.update(b"\0");
    hasher.update(commit_oid.as_bytes());
    AggregateKey(format!("check:v1-{}", hasher.finalize().to_hex()))
}

pub fn check_subject(repo: &str, commit_oid: &str, context: &str) -> ArtifactRef {
    ArtifactRef(format!("{repo}#commit-{commit_oid}/check-{context}"))
}

pub fn check_updated_draft(
    repo: &str,
    commit_oid: &str,
    context: &str,
    check_status: serde_json::Value,
) -> EventDraft {
    EventDraft {
        type_: EventType(CI_CHECK_UPDATED.to_string()),
        subject: check_subject(repo, commit_oid, context),
        aggregate: check_aggregate(repo, commit_oid),
        payload: check_status,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedCheck {
    pub seq: u64,
    pub subject: ArtifactRef,
    pub check_status: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct CheckSeamOrder {
    aggregate: String,
    by_seq: BTreeMap<u64, OrderedCheck>,
}

impl CheckSeamOrder {
    pub fn new(repo: &str, commit_oid: &str) -> CheckSeamOrder {
        CheckSeamOrder {
            aggregate: check_aggregate(repo, commit_oid).0,
            by_seq: BTreeMap::new(),
        }
    }

    pub fn aggregate(&self) -> &str {
        &self.aggregate
    }

    pub fn ingest(&mut self, env: &EventEnvelope, seq: u64) -> Result<bool, CheckSeamError> {
        if env.type_.0 != CI_CHECK_UPDATED {
            return Err(CheckSeamError::WrongType(env.type_.0.clone()));
        }
        if env.aggregate.0 != self.aggregate {
            return Err(CheckSeamError::WrongAggregate {
                expected: self.aggregate.clone(),
                got: env.aggregate.0.clone(),
            });
        }
        match self.by_seq.entry(seq) {
            std::collections::btree_map::Entry::Occupied(_) => Ok(false),
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(OrderedCheck {
                    seq,
                    subject: env.subject.clone(),
                    check_status: env.payload.clone(),
                });
                Ok(true)
            }
        }
    }

    pub fn in_order(&self) -> Vec<OrderedCheck> {
        self.by_seq.values().cloned().collect()
    }

    pub fn observed_seqs(&self) -> Vec<u64> {
        self.by_seq.keys().copied().collect()
    }

    pub fn ordering_gap(&self) -> u64 {
        match self.by_seq.keys().next_back() {
            None => 0,
            Some(&high) => high - self.by_seq.len() as u64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckSeamError {
    WrongType(String),
    WrongAggregate { expected: String, got: String },
}

impl std::fmt::Display for CheckSeamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckSeamError::WrongType(t) => {
                write!(f, "not a ci.check.updated event: type_={t}")
            }
            CheckSeamError::WrongAggregate { expected, got } => {
                write!(f, "wrong aggregate: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for CheckSeamError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiOverall {
    Success,
    Failure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiResult {
    pub commit_oid: String,
    pub overall: CiOverall,
    pub contexts: Vec<String>,
    pub idem_token: String,
}

pub fn ci_result_subject(repo: &str, commit_oid: &str) -> ArtifactRef {
    ArtifactRef(format!("{repo}#commit-{commit_oid}/ci-result"))
}

pub fn ci_result_draft(repo: &str, result: &CiResult) -> EventDraft {
    EventDraft {
        type_: EventType(CI_RESULT.to_string()),
        subject: ci_result_subject(repo, &result.commit_oid),
        aggregate: check_aggregate(repo, &result.commit_oid),
        payload: serde_json::to_value(result).expect("CiResult serialises (closed shape)"),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn rollup_ci_result(
    commit_oid: &str,
    current: &BTreeMap<String, bool>,
    required: &[String],
    idem_token: &str,
) -> CiResult {
    let overall = if required
        .iter()
        .all(|ctx| current.get(ctx).copied().unwrap_or(false))
    {
        CiOverall::Success
    } else {
        CiOverall::Failure
    };
    let mut contexts: Vec<String> = required.to_vec();
    contexts.sort();
    CiResult {
        commit_oid: commit_oid.to_string(),
        overall,
        contexts,
        idem_token: idem_token.to_string(),
    }
}

#[derive(Debug, Default)]
pub struct CiResultWaitSubstrate {
    delivered: BTreeMap<String, CiResult>,
    wakes: BTreeMap<String, u32>,
}

impl CiResultWaitSubstrate {
    pub const SIGNAL_NAME: &'static str = CI_RESULT;

    pub fn new() -> CiResultWaitSubstrate {
        CiResultWaitSubstrate::default()
    }

    pub fn wait_for_signal(&mut self, idem_key: &str) -> Option<CiResult> {
        self.delivered.get(idem_key).cloned()
    }

    pub fn deliver(&mut self, result: CiResult) -> WakeOutcome {
        let key = result.idem_token.clone();
        if self.delivered.contains_key(&key) {
            return WakeOutcome::Duplicate;
        }
        self.delivered.insert(key.clone(), result);
        *self.wakes.entry(key).or_insert(0) += 1;
        WakeOutcome::Woke
    }

    pub fn wake_count(&self, idem_key: &str) -> u32 {
        self.wakes.get(idem_key).copied().unwrap_or(0)
    }

    pub fn is_resolved(&self, idem_key: &str) -> bool {
        self.delivered.contains_key(idem_key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeOutcome {
    Woke,
    Duplicate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, CorrelationId, DataRole as DR, EventId, Timestamp, Visibility as Vis};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn ci_actor() -> Actor {
        Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        ))
    }

    fn check_env(
        repo: &str,
        commit_oid: &str,
        context: &str,
        run_attempt: u64,
        state: &str,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("evt-{context}-{run_attempt}")),
            type_: EventType(CI_CHECK_UPDATED.to_string()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: ci_actor(),
            subject: check_subject(repo, commit_oid, context),
            aggregate: check_aggregate(repo, commit_oid),
            causation_id: None,
            correlation_id: CorrelationId(format!("corr-{commit_oid}")),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DR::Controller,
            visibility: Vis::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({ "context": context, "run_attempt": run_attempt, "state": state }),
        }
    }

    #[test]
    fn envelope_conformance_subject_and_aggregate_grammar() {
        let draft = check_updated_draft(
            "myelin://acme/git/repo/core",
            "abc123",
            "build",
            serde_json::json!({ "state": "success", "run_attempt": 1 }),
        );
        assert_eq!(draft.type_.0, "ci.check.updated");
        assert_eq!(
            draft.subject.0, "myelin://acme/git/repo/core#commit-abc123/check-build",
            "subject = repo#commit-<oid>/check-<context> (§4.12)"
        );
        assert_eq!(
            draft.aggregate,
            check_aggregate("myelin://acme/git/repo/core", "abc123"),
            "aggregate = (repo, commit_oid) - all contexts for one commit share it"
        );
        assert_eq!(draft.payload["run_attempt"], 1);
        assert!(!draft.contains_personal_data, "references-not-payloads");
    }

    #[test]
    fn all_contexts_of_a_commit_share_one_aggregate() {
        let a_build = check_aggregate("repo:core", "deadbeef");
        let a_test = check_aggregate("repo:core", "deadbeef");
        let a_other_commit = check_aggregate("repo:core", "cafef00d");
        assert_eq!(
            a_build, a_test,
            "build + test of one commit → one aggregate"
        );
        assert_ne!(
            a_build, a_other_commit,
            "a different commit → a different aggregate"
        );
    }

    #[test]
    fn dotted_repository_ref_encodes_to_one_structured_stream_token() {
        let envelope = check_env(
            "myelin://acme/git/repo/myelin.rs",
            "deadbeef",
            "build",
            1,
            "success",
        );
        assert!(envelope.aggregate.0.starts_with("check:v1-"));
        assert_eq!(envelope.aggregate.0.len(), "check:v1-".len() + 64);
        let aggregate_id = envelope
            .aggregate
            .0
            .strip_prefix("check:")
            .expect("structured aggregate prefix");
        assert_eq!(
            crate::partition::StreamSubject::of(&envelope)
                .expect("every valid dotted Git repository is broker-routable")
                .to_subject(),
            format!("evt.acme.ci.check.{aggregate_id}.updated")
        );
    }

    #[test]
    fn interleaved_and_late_arrivals_stay_per_aggregate_ordered() {
        let mut order = CheckSeamOrder::new("repo:core", "deadbeef");

        let build1 = check_env("repo:core", "deadbeef", "build", 1, "failure");
        let test1 = check_env("repo:core", "deadbeef", "test", 1, "success");
        let build2 = check_env("repo:core", "deadbeef", "build", 2, "success");
        let test2 = check_env("repo:core", "deadbeef", "test", 2, "success");

        assert!(order.ingest(&build2, 3).unwrap());
        assert!(order.ingest(&build1, 1).unwrap());
        assert!(order.ingest(&test2, 4).unwrap());
        assert!(order.ingest(&test1, 2).unwrap());

        let seqs: Vec<u64> = order.in_order().iter().map(|c| c.seq).collect();
        assert_eq!(
            seqs,
            vec![1, 2, 3, 4],
            "consumed in per-aggregate seq order, not arrival order"
        );
        assert_eq!(order.ordering_gap(), 0, "contiguous: no gap, fully ordered");

        let build_attempts: Vec<u64> = order
            .in_order()
            .iter()
            .filter(|c| c.subject.0.ends_with("check-build"))
            .map(|c| c.check_status["run_attempt"].as_u64().unwrap())
            .collect();
        assert_eq!(
            build_attempts,
            vec![1, 2],
            "build attempts appear in monotonic order"
        );
    }

    #[test]
    fn stale_redelivery_is_droppable_order_preserved() {
        let mut order = CheckSeamOrder::new("repo:core", "deadbeef");
        let build1 = check_env("repo:core", "deadbeef", "build", 1, "failure");
        let build2 = check_env("repo:core", "deadbeef", "build", 2, "success");

        assert!(order.ingest(&build1, 1).unwrap(), "first build is new");
        assert!(order.ingest(&build2, 2).unwrap(), "the re-run is new");

        assert!(
            !order.ingest(&build1, 1).unwrap(),
            "the stale re-delivery is a duplicate, absorbed"
        );

        let seqs: Vec<u64> = order.in_order().iter().map(|c| c.seq).collect();
        assert_eq!(
            seqs,
            vec![1, 2],
            "order preserved across the stale re-delivery (droppable)"
        );
        assert_eq!(order.ordering_gap(), 0);
    }

    #[test]
    fn ordering_gap_counts_in_flight_seqs() {
        let mut order = CheckSeamOrder::new("repo:core", "deadbeef");
        order
            .ingest(
                &check_env("repo:core", "deadbeef", "build", 1, "success"),
                1,
            )
            .unwrap();
        order
            .ingest(&check_env("repo:core", "deadbeef", "lint", 1, "success"), 3)
            .unwrap();
        assert_eq!(order.ordering_gap(), 1, "seq 2 in flight → gap of 1");
        order
            .ingest(&check_env("repo:core", "deadbeef", "test", 1, "success"), 2)
            .unwrap();
        assert_eq!(
            order.ordering_gap(),
            0,
            "every op delivered → contiguous, 0 gap"
        );
        assert_eq!(order.observed_seqs(), vec![1, 2, 3]);
    }

    #[test]
    fn ingest_rejects_foreign_type_and_aggregate() {
        let mut order = CheckSeamOrder::new("repo:core", "deadbeef");

        let mut wrong_type = check_env("repo:core", "deadbeef", "build", 1, "success");
        wrong_type.type_ = EventType("git.ref.updated".into());
        assert!(matches!(
            order.ingest(&wrong_type, 1),
            Err(CheckSeamError::WrongType(_))
        ));

        let wrong_agg = check_env("repo:core", "cafef00d", "build", 1, "success");
        assert!(matches!(
            order.ingest(&wrong_agg, 1),
            Err(CheckSeamError::WrongAggregate { .. })
        ));
    }

    #[test]
    fn wait_for_signal_wakes_exactly_once_on_double_delivery() {
        let mut sub = CiResultWaitSubstrate::new();
        let idem = "merge-attempt-42";

        assert_eq!(
            sub.wait_for_signal(idem),
            None,
            "no signal yet → genuinely pending"
        );
        assert!(!sub.is_resolved(idem));

        let result = CiResult {
            commit_oid: "deadbeef".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: idem.into(),
        };

        assert_eq!(
            sub.deliver(result.clone()),
            WakeOutcome::Woke,
            "first delivery wakes"
        );
        assert_eq!(
            sub.deliver(result.clone()),
            WakeOutcome::Duplicate,
            "re-delivery is one wake"
        );
        assert_eq!(sub.deliver(result.clone()), WakeOutcome::Duplicate);

        assert_eq!(
            sub.wake_count(idem),
            1,
            "EXACTLY ONE wake on a doubly-delivered ci.result"
        );
        assert!(sub.is_resolved(idem));
        assert_eq!(sub.wait_for_signal(idem), Some(result));
    }

    #[test]
    fn distinct_idem_keys_wake_independently() {
        let mut sub = CiResultWaitSubstrate::new();
        let r1 = CiResult {
            commit_oid: "c1".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into()],
            idem_token: "attempt-1".into(),
        };
        let r2 = CiResult {
            commit_oid: "c2".into(),
            overall: CiOverall::Failure,
            contexts: vec!["build".into()],
            idem_token: "attempt-2".into(),
        };
        assert_eq!(sub.deliver(r1), WakeOutcome::Woke);
        assert_eq!(sub.deliver(r2), WakeOutcome::Woke);
        assert_eq!(sub.wake_count("attempt-1"), 1);
        assert_eq!(sub.wake_count("attempt-2"), 1);
        assert_eq!(
            sub.wake_count("attempt-3"),
            0,
            "an unparked key has no wake"
        );
    }

    #[test]
    fn signal_name_is_the_ci_result_token() {
        assert_eq!(CiResultWaitSubstrate::SIGNAL_NAME, "ci.result");
    }

    #[test]
    fn ci_result_payload_shape() {
        let result = CiResult {
            commit_oid: "deadbeef".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: "merge-7".into(),
        };
        let v = serde_json::to_value(&result).unwrap();
        assert_eq!(v["commit_oid"], "deadbeef");
        assert_eq!(
            v["overall"], "success",
            "overall is snake_case success|failure"
        );
        assert_eq!(v["contexts"], serde_json::json!(["build", "test"]));
        assert_eq!(v["idem_token"], "merge-7");
        let back: CiResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, result);
    }

    #[test]
    fn ci_result_draft_follows_the_grammar() {
        let result = CiResult {
            commit_oid: "abc123".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: "merge-7".into(),
        };
        let draft = ci_result_draft("myelin://acme/git/repo/core", &result);
        assert_eq!(draft.type_.0, "ci.result");
        assert_eq!(
            draft.subject.0, "myelin://acme/git/repo/core#commit-abc123/ci-result",
            "subject = repo#commit-<oid>/ci-result (§4.12 #sub)"
        );
        assert_eq!(
            draft.aggregate,
            check_aggregate("myelin://acme/git/repo/core", "abc123"),
            "the rollup shares the per-commit aggregate so it linearises after its checks"
        );
        assert_eq!(
            draft.aggregate,
            check_aggregate("myelin://acme/git/repo/core", "abc123"),
            "the rollup aggregate IS the checks' aggregate"
        );
        assert!(!draft.contains_personal_data, "references-not-payloads");
        let back: CiResult = serde_json::from_value(draft.payload).unwrap();
        assert_eq!(back, result);
    }

    #[test]
    fn rollup_ci_result_is_success_iff_all_required_pass() {
        let required = vec!["build".to_string(), "test".to_string()];

        let mut current = BTreeMap::new();
        current.insert("build".to_string(), true);
        current.insert("test".to_string(), true);
        current.insert("lint".to_string(), false);
        let r = rollup_ci_result("abc123", &current, &required, "merge-1");
        assert_eq!(r.overall, CiOverall::Success);
        assert_eq!(
            r.contexts,
            vec!["build".to_string(), "test".to_string()],
            "the rolled-up set is the required gate set, sorted (byte-stable)"
        );

        let mut current = BTreeMap::new();
        current.insert("build".to_string(), true);
        current.insert("test".to_string(), false);
        let r = rollup_ci_result("abc123", &current, &required, "merge-1");
        assert_eq!(r.overall, CiOverall::Failure);

        let mut current = BTreeMap::new();
        current.insert("build".to_string(), true);
        let r = rollup_ci_result("abc123", &current, &required, "merge-1");
        assert_eq!(
            r.overall,
            CiOverall::Failure,
            "a missing required context never implicitly passes"
        );
    }

    #[test]
    fn rollup_ci_result_is_deterministic() {
        let required = vec!["test".to_string(), "build".to_string()];
        let mut current = BTreeMap::new();
        current.insert("build".to_string(), true);
        current.insert("test".to_string(), true);
        let a = rollup_ci_result("abc123", &current, &required, "merge-1");
        let b = rollup_ci_result("abc123", &current, &required, "merge-1");
        assert_eq!(a, b, "same inputs → byte-identical rollup");
        assert_eq!(
            a.contexts,
            vec!["build".to_string(), "test".to_string()],
            "contexts always sorted regardless of input order"
        );
    }
}
