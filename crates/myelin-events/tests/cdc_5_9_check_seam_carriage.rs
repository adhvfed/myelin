use myelin_events::{
    check_aggregate, check_subject, check_updated_draft, validate_event_type, Actor,
    CheckSeamOrder, CiOverall, CiResult, CiResultWaitSubstrate, CorrelationId, DataRole,
    EventEnvelope, EventId, EventType, Timestamp, Visibility, WakeOutcome,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "abc123def";

#[test]
fn provider_ci_emits_ci_check_updated_with_the_412_envelope_shape() {
    let check_status = serde_json::json!({
        "context": "build",
        "state": "success",
        "required": true,
        "run": "myelin://acme/ci/run/01J",
        "run_attempt": 2,
        "trust_tier": "trusted",
        "details_ref": "myelin://acme/ci/run/01J#step-3",
    });
    let draft = check_updated_draft(REPO, COMMIT, "build", check_status.clone());

    assert_eq!(draft.type_.0, "ci.check.updated");
    assert!(
        validate_event_type("ci.check.updated").is_ok(),
        "the type is a §6.1 canonical name"
    );

    assert_eq!(
        draft.subject.0,
        format!("{REPO}#commit-{COMMIT}/check-build")
    );

    assert_eq!(draft.aggregate, check_aggregate(REPO, COMMIT));

    assert_eq!(draft.payload, check_status);
    assert!(!draft.contains_personal_data);
}

fn delivered(context: &str, run_attempt: u64, state: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("evt-{context}-a{run_attempt}")),
        type_: EventType("ci.check.updated".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: check_subject(REPO, COMMIT, context),
        aggregate: check_aggregate(REPO, COMMIT),
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{COMMIT}")),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "context": context, "run_attempt": run_attempt, "state": state }),
    }
}

#[test]
fn consumer_git_receives_ci_check_updated_per_aggregate_ordered() {
    let mut order = CheckSeamOrder::new(REPO, COMMIT);

    assert!(order.ingest(&delivered("build", 2, "success"), 3).unwrap());
    assert!(order.ingest(&delivered("build", 1, "failure"), 1).unwrap());
    assert!(order.ingest(&delivered("test", 1, "success"), 2).unwrap());

    let seqs: Vec<u64> = order.in_order().iter().map(|c| c.seq).collect();
    assert_eq!(
        seqs,
        vec![1, 2, 3],
        "per-aggregate ordered on (repo, commit_oid)"
    );
    assert_eq!(
        order.ordering_gap(),
        0,
        "contiguous: at-least-once delivered every op (0 lost)"
    );
}

#[test]
fn cdc_9_4_ci_result_rollup_signal_wakes_merge_queue_exactly_once() {
    let result = CiResult {
        commit_oid: COMMIT.into(),
        overall: CiOverall::Success,
        contexts: vec!["build".into(), "test".into()],
        idem_token: "merge-attempt-99".into(),
    };
    assert_eq!(CiResultWaitSubstrate::SIGNAL_NAME, "ci.result");
    assert!(
        validate_event_type("ci.result").is_ok(),
        "ci.result is a §6.1 canonical name"
    );

    let mut sub = CiResultWaitSubstrate::new();
    assert_eq!(
        sub.wait_for_signal("merge-attempt-99"),
        None,
        "pending while CI runs (9.4)"
    );

    assert_eq!(sub.deliver(result.clone()), WakeOutcome::Woke);
    assert_eq!(sub.deliver(result.clone()), WakeOutcome::Duplicate);
    assert_eq!(
        sub.wake_count("merge-attempt-99"),
        1,
        "exactly one wake (9.1 idem on idem_key)"
    );

    let read = sub.wait_for_signal("merge-attempt-99").unwrap();
    assert_eq!(read.overall, CiOverall::Success);
    assert_eq!(read, result);
}
