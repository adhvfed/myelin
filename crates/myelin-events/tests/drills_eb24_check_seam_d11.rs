use myelin_events::{
    check_aggregate, check_subject, Actor, BusSignal, CheckSeamError, CheckSeamOrder, CiOverall,
    CiResult, CiResultWaitSubstrate, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    MetricRecorder, MetricSample, MetricsSink, Timestamp, Visibility, WakeOutcome,
};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "deadbeefcafe";

fn check_env(context: &str, run_attempt: u64, state: &str) -> EventEnvelope {
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

fn bridge(ordering_gap: i64, src: &mut SignalSource) {
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new(
            "consumer",
            format!("check-seam:{}", check_aggregate(REPO, COMMIT).0),
        )],
        ordering_gap,
    );
}

#[test]
fn d11_check_seam_per_aggregate_ordering_holds_under_interleave_and_rerun() {
    let mut order = CheckSeamOrder::new(REPO, COMMIT);

    let build1 = check_env("build", 1, "failure");
    let test1 = check_env("test", 1, "success");
    let lint1 = check_env("lint", 1, "success");
    let build2 = check_env("build", 2, "success");
    let test2 = check_env("test", 2, "success");

    assert!(order.ingest(&build2, 4).unwrap());
    assert!(order.ingest(&test1, 2).unwrap());
    assert!(order.ingest(&test2, 5).unwrap());
    assert!(order.ingest(&build1, 1).unwrap());
    assert!(order.ingest(&lint1, 3).unwrap());

    let consumed_seqs: Vec<u64> = order.in_order().iter().map(|c| c.seq).collect();
    assert_eq!(
        consumed_seqs,
        vec![1, 2, 3, 4, 5],
        "consumed in per-aggregate seq order, not arrival order - D-11 aggregate order preserved"
    );

    assert!(
        !order.ingest(&build1, 1).unwrap(),
        "the stale re-delivery is a duplicate, absorbed - droppable"
    );
    let after_stale: Vec<u64> = order.in_order().iter().map(|c| c.seq).collect();
    assert_eq!(
        after_stale,
        vec![1, 2, 3, 4, 5],
        "order preserved across the stale re-delivery"
    );

    let current_build_attempt = order
        .in_order()
        .iter()
        .filter(|c| c.subject.0.ends_with("check-build"))
        .map(|c| c.check_status["run_attempt"].as_u64().unwrap())
        .max()
        .unwrap();
    assert_eq!(
        current_build_attempt, 2,
        "current build = the re-run (highest run_attempt)"
    );
    let current_build_state = order
        .in_order()
        .iter()
        .filter(|c| c.subject.0.ends_with("check-build"))
        .max_by_key(|c| c.check_status["run_attempt"].as_u64().unwrap())
        .map(|c| c.check_status["state"].as_str().unwrap().to_string())
        .unwrap();
    assert_eq!(
        current_build_state, "success",
        "supersession: the re-run supersedes the failure"
    );

    let ordering_gap = order.ordering_gap();
    assert_eq!(
        ordering_gap, 0,
        "0 ops outstanding ⇒ contiguous, fully-ordered partition"
    );

    let mut rec = MetricRecorder::new();
    rec.emit(MetricSample::scalar(BusSignal::PublishLatencyMillis, 7));

    let mut src = SignalSource::new();
    bridge(ordering_gap as i64, &mut src);

    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new(
            "consumer",
            format!("check-seam:{}", check_aggregate(REPO, COMMIT).0),
        )],
        Predicate::Eq(0),
    )
    .expect_green();
    assert!(
        rec.scalar(BusSignal::PublishLatencyMillis).unwrap() <= 1_000,
        "per-aggregate publish latency bounded (ordering-health, §4.11 #4)"
    );
}

#[test]
fn d11_ci_result_wait_substrate_wakes_once_on_double_delivery() {
    let mut sub = CiResultWaitSubstrate::new();
    let idem = "merge-attempt-7";

    assert_eq!(
        sub.wait_for_signal(idem),
        None,
        "pending until the rollup arrives"
    );

    let result = CiResult {
        commit_oid: COMMIT.into(),
        overall: CiOverall::Success,
        contexts: vec!["build".into(), "test".into(), "lint".into()],
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
        "re-delivery absorbed"
    );
    assert_eq!(
        sub.wake_count(idem),
        1,
        "EXACTLY ONE wake on a doubly-delivered ci.result (9.4)"
    );
    assert_eq!(
        sub.wait_for_signal(idem),
        Some(result),
        "the workflow re-leases + reads the rollup"
    );
}

#[test]
fn d11_foreign_event_rejected_from_the_ordering_partition() {
    let mut order = CheckSeamOrder::new(REPO, COMMIT);
    let mut wrong = check_env("build", 1, "success");
    wrong.type_ = EventType("git.ref.updated".into());
    assert!(matches!(
        order.ingest(&wrong, 1),
        Err(CheckSeamError::WrongType(_))
    ));
}
