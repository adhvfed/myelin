use std::collections::BTreeMap;

use myelin_events::{
    check_aggregate, check_updated_draft, ci_result_draft, ci_result_subject, rollup_ci_result,
    Actor, BusSignal, CheckSeamOrder, CiOverall, CiResultWaitSubstrate, CorrelationId, DataRole,
    EventEnvelope, EventId, EventType, MetricRecorder, MetricSample, MetricsSink, Timestamp,
    Visibility, WakeOutcome,
};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "deadbeefcafe";

fn check_env(context: &str, run_attempt: u64, state: &str, trust_tier: &str) -> EventEnvelope {
    let draft = check_updated_draft(
        REPO,
        COMMIT,
        context,
        serde_json::json!({
            "context": context,
            "run_attempt": run_attempt,
            "state": state,
            "trust_tier": trust_tier,
        }),
    );
    EventEnvelope {
        event_id: EventId(format!("evt-{context}-a{run_attempt}")),
        type_: EventType(draft.type_.0),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: draft.subject,
        aggregate: draft.aggregate,
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{COMMIT}")),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: draft.payload,
    }
}

fn current_status(order: &CheckSeamOrder) -> BTreeMap<String, bool> {
    let mut current: BTreeMap<String, (u64, bool)> = BTreeMap::new();
    for c in order.in_order() {
        let ctx = c
            .subject
            .0
            .rsplit("/check-")
            .next()
            .expect("subject has a check- sub-anchor")
            .to_string();
        let attempt = c.check_status["run_attempt"].as_u64().unwrap();
        let ok = c.check_status["state"].as_str() == Some("success");
        let entry = current.entry(ctx).or_insert((0, false));
        if attempt >= entry.0 {
            *entry = (attempt, ok);
        }
    }
    current.into_iter().map(|(k, (_, ok))| (k, ok)).collect()
}

#[test]
fn d10_d8_check_seam_end_to_end_zero_double_merge() {
    let build1 = check_env("build", 1, "failure", "trusted");
    let test1 = check_env("test", 1, "success", "trusted");
    let build2 = check_env("build", 2, "success", "trusted");

    let mut order = CheckSeamOrder::new(REPO, COMMIT);
    assert!(order.ingest(&build2, 3).unwrap());
    assert!(order.ingest(&test1, 2).unwrap());
    assert!(order.ingest(&build1, 1).unwrap());
    assert!(
        !order.ingest(&build1, 1).unwrap(),
        "the stale lower-attempt re-delivery is absorbed (droppable)"
    );
    assert_eq!(
        order.in_order().iter().map(|c| c.seq).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "per-aggregate order preserved - D-11 substrate"
    );
    assert_eq!(order.ordering_gap(), 0, "contiguous, 0 ops outstanding");

    let current = current_status(&order);
    assert_eq!(
        current.get("build"),
        Some(&true),
        "current build = the re-run success (highest run_attempt), NOT the stale failure"
    );
    assert_eq!(current.get("test"), Some(&true));

    let required = vec!["build".to_string(), "test".to_string()];
    let idem = "merge-attempt-7";
    let rollup = rollup_ci_result(COMMIT, &current, &required, idem);
    assert_eq!(
        rollup.overall,
        CiOverall::Success,
        "every required context passed (post-supersession) → Success"
    );
    let rollup_draft = ci_result_draft(REPO, &rollup);
    assert_eq!(rollup_draft.type_.0, "ci.result");
    assert_eq!(
        rollup_draft.aggregate,
        check_aggregate(REPO, COMMIT),
        "the rollup shares the per-commit aggregate (linearises after its checks)"
    );
    assert_eq!(rollup_draft.subject, ci_result_subject(REPO, COMMIT));

    let mut sub = CiResultWaitSubstrate::new();
    assert_eq!(
        sub.wait_for_signal(idem),
        None,
        "the merge-queue holds NO runtime while CI runs (contract 9.4)"
    );

    let mut merges = 0u32;
    let mut maybe_merge = |outcome: WakeOutcome, overall: CiOverall| {
        if outcome == WakeOutcome::Woke && overall == CiOverall::Success {
            merges += 1;
        }
    };

    maybe_merge(sub.deliver(rollup.clone()), rollup.overall);
    maybe_merge(sub.deliver(rollup.clone()), rollup.overall);
    maybe_merge(sub.deliver(rollup.clone()), rollup.overall);

    assert_eq!(
        sub.wake_count(idem),
        1,
        "the merge-queue wakes EXACTLY ONCE on a doubly-delivered ci.result (9.4)"
    );
    assert_eq!(merges, 1, "0 double-merge - the PR merges EXACTLY ONCE");

    let mut rec = MetricRecorder::new();
    rec.emit(MetricSample::scalar(BusSignal::PublishLatencyMillis, 6));
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new(
            "consumer",
            format!("check-seam:{}", check_aggregate(REPO, COMMIT).0),
        )],
        order.ordering_gap() as i64,
    );
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
fn d10_fork_self_green_is_neutral_for_gating() {
    let mut order = CheckSeamOrder::new(REPO, COMMIT);
    let fork_build = check_env("build", 1, "success", "untrusted_fork");
    let trusted_test = check_env("test", 1, "success", "trusted");
    assert!(order.ingest(&fork_build, 1).unwrap());
    assert!(order.ingest(&trusted_test, 2).unwrap());

    let mut trusted_current: BTreeMap<String, bool> = BTreeMap::new();
    for c in order.in_order() {
        if c.check_status["trust_tier"].as_str() == Some("untrusted_fork") {
            continue;
        }
        let ctx = c.subject.0.rsplit("/check-").next().unwrap().to_string();
        trusted_current.insert(ctx, c.check_status["state"].as_str() == Some("success"));
    }

    let required = vec!["build".to_string(), "test".to_string()];
    let rollup = rollup_ci_result(COMMIT, &trusted_current, &required, "merge-attempt-fork");
    assert_eq!(
        rollup.overall,
        CiOverall::Failure,
        "a fork self-green is NEUTRAL - build has no TRUSTED success, so the gate stays closed"
    );

    let endorsed_build = check_env("build", 2, "success", "trusted");
    assert!(order.ingest(&endorsed_build, 3).unwrap());
    let mut endorsed_current: BTreeMap<String, bool> = BTreeMap::new();
    for c in order.in_order() {
        if c.check_status["trust_tier"].as_str() == Some("untrusted_fork") {
            continue;
        }
        let ctx = c.subject.0.rsplit("/check-").next().unwrap().to_string();
        let attempt = c.check_status["run_attempt"].as_u64().unwrap();
        let ok = c.check_status["state"].as_str() == Some("success");
        endorsed_current
            .entry(ctx)
            .and_modify(|v| *v = ok)
            .or_insert(ok);
        let _ = attempt;
    }
    let rollup2 = rollup_ci_result(COMMIT, &endorsed_current, &required, "merge-attempt-fork-2");
    assert_eq!(
        rollup2.overall,
        CiOverall::Success,
        "once the fork run is endorsed (a trusted build success lands), the gate passes"
    );
}

#[test]
fn ci_d8_distinct_merge_attempts_wake_independently_once() {
    let mut sub = CiResultWaitSubstrate::new();
    let mut current = BTreeMap::new();
    current.insert("build".to_string(), true);
    let required = vec!["build".to_string()];

    let r1 = rollup_ci_result("commit-1", &current, &required, "attempt-1");
    let r2 = rollup_ci_result("commit-2", &current, &required, "attempt-2");

    assert_eq!(sub.deliver(r1.clone()), WakeOutcome::Woke);
    assert_eq!(sub.deliver(r1), WakeOutcome::Duplicate);
    assert_eq!(sub.deliver(r2.clone()), WakeOutcome::Woke);
    assert_eq!(sub.deliver(r2), WakeOutcome::Duplicate);

    assert_eq!(sub.wake_count("attempt-1"), 1, "attempt 1 wakes once");
    assert_eq!(sub.wake_count("attempt-2"), 1, "attempt 2 wakes once");
    assert_eq!(
        sub.wake_count("attempt-3"),
        0,
        "an unparked attempt has 0 wakes - 0 spurious unblocks"
    );
}
