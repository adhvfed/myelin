use myelin_events::check_seam::{check_aggregate, check_subject, CheckSeamOrder};
use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{
    Actor, ConsumerName, CorrelationId, DataRole, DedupLedger, Delivered, EventEnvelope, EventId,
    EventType, Message, PrefetchBound, Subscription, Timestamp, Visibility,
};
use myelin_events::{Consumer, EventHandler};
use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusConsumer, GitOid, HumanisedRef,
    Timestamp as GitTimestamp, TrustTier,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{ArtifactRef as TArtifactRef, Region, TenantId};
use std::collections::BTreeMap;

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "abc123def";

fn synthetic_check_updated(
    context: &str,
    attempt: u32,
    state: CheckState,
    trust: TrustTier,
) -> EventEnvelope {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    let fact = CheckStatus {
        tenant: TenantId("acme".into()),
        repo: TArtifactRef(REPO.into()),
        commit_oid: GitOid(COMMIT.into()),
        context: CheckContext::ci(context),
        state,
        required: true,
        run: TArtifactRef("myelin://acme/ci/run/9".into()),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: TArtifactRef("myelin://acme/ci/run/9#step-2".into()),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args,
        },
        started_at: GitTimestamp("2026-06-21T00:00:00Z".into()),
        completed_at: Some(GitTimestamp("2026-06-21T00:01:00Z".into())),
        cost_settled: true,
    };
    let payload = serde_json::to_value(&fact).expect("the 5.9 fact serialises opaque");
    EventEnvelope {
        event_id: EventId(format!("evt-{context}-a{attempt}")),
        type_: EventType(CI_CHECK_UPDATED.into()),
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
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload,
    }
}

fn bind_consumer() -> Consumer<CheckStatusConsumer> {
    let handler = CheckStatusConsumer::new();
    let subjects: Vec<&str> = handler.subjects().iter().map(|p| p.0.as_str()).collect();
    let sub = Subscription::bind(
        ConsumerName("git.check_status".into()),
        &subjects,
        PrefetchBound::new(64).unwrap(),
    )
    .expect("the ci.check.updated whitelist binds (never a wildcard)");
    Consumer::new(handler, sub, DedupLedger::new())
}

#[test]
fn consumer_leg_is_idempotent_on_event_id_zero_dup() {
    let consumer = bind_consumer();
    let env = synthetic_check_updated("build", 1, CheckState::Success, TrustTier::Trusted);
    let msg = Message {
        subject: env.subject.0.clone(),
        envelope: env,
    };

    assert_eq!(consumer.deliver(&msg), Delivered::Acked);
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Deduplicated,
        "0 dup - handler runs once"
    );
    assert_eq!(consumer.deliver(&msg), Delivered::Deduplicated);

    let proj = consumer.handler().projection();
    assert_eq!(proj.len(), 1);
    assert_eq!(
        consumer.handler().applied_count(),
        1,
        "the fact was applied exactly once"
    );
    assert_eq!(consumer.handler().dropped_stale_count(), 0);
}

#[test]
fn consumer_leg_per_aggregate_ordered_supersession_drops_stale() {
    let consumer = bind_consumer();

    let build1 = synthetic_check_updated("build", 1, CheckState::Failure, TrustTier::Trusted);
    let test1 = synthetic_check_updated("test", 1, CheckState::Success, TrustTier::Trusted);
    let build2 = synthetic_check_updated("build", 2, CheckState::Success, TrustTier::Trusted);

    let mut order = CheckSeamOrder::new(REPO, COMMIT);
    assert!(order.ingest(&build2, 3).unwrap());
    assert!(order.ingest(&build1, 1).unwrap());
    assert!(order.ingest(&test1, 2).unwrap());
    assert_eq!(order.ordering_gap(), 0, "contiguous - 0 ops lost");

    for oc in order.in_order() {
        let env = match oc.seq {
            1 => &build1,
            2 => &test1,
            3 => &build2,
            _ => unreachable!(),
        };
        let msg = Message {
            subject: env.subject.0.clone(),
            envelope: env.clone(),
        };
        assert_eq!(consumer.deliver(&msg), Delivered::Acked);
    }

    let handler = consumer.handler();
    let proj = handler.projection();
    let build_key = myelin_git::check_status::CheckKey {
        commit_oid: GitOid(COMMIT.into()),
        context: CheckContext::ci("build"),
    };
    let row = proj.current(&build_key).expect("build row present");
    assert_eq!(
        row.run_attempt, 2,
        "the current build row is the highest attempt"
    );
    assert_eq!(
        row.state,
        CheckState::Success,
        "the re-run success is current, not the stale failure"
    );
    assert_eq!(proj.len(), 2);

    let stale = CheckStatusConsumer::decode(&build1.payload).unwrap();
    let mut proj2 = handler.projection();
    assert_eq!(
        proj2.apply(&stale),
        myelin_git::check_status::ApplyOutcome::DroppedStale {
            incoming_attempt: 1,
            current_attempt: 2
        }
    );
}

#[test]
fn consumer_leg_dead_letters_a_malformed_payload() {
    let consumer = bind_consumer();
    let mut env = synthetic_check_updated("build", 1, CheckState::Success, TrustTier::Trusted);
    env.payload = serde_json::json!({ "garbage": true });
    let msg = Message {
        subject: env.subject.0.clone(),
        envelope: env,
    };

    match consumer.deliver(&msg) {
        Delivered::DeadLettered(reason) => {
            assert!(
                reason.0.contains("CheckStatus"),
                "the dead-letter reason names the decode failure"
            );
        }
        other => panic!("a malformed payload must dead-letter, got {other:?}"),
    }
    assert!(consumer.handler().projection().is_empty());
    assert_eq!(consumer.handler().applied_count(), 0);
}

#[test]
fn consumer_leg_dead_letters_a_foreign_type() {
    let consumer = bind_consumer();
    let mut env = synthetic_check_updated("build", 1, CheckState::Success, TrustTier::Trusted);
    env.type_ = EventType("git.ref.updated".into());
    assert!(matches!(
        consumer
            .handler()
            .handle(&env, &mut myelin_events::HandlerTx::none()),
        myelin_events::HandleOutcome::NonRetryable(_)
    ));
}
