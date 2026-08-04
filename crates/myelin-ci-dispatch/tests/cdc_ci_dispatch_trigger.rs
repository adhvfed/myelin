use myelin_ci_dispatch::dispatch::{
    classify_trust, compile_trigger, stamp_trust, trigger_matches, DedupLedger, OnTrigger,
    RunProvenance, TrustTier, RUN_OBJECT_TYPE, TRIGGER_CONSUMER,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_git::check_status::TrustTier as GitTrustTier;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_tenancy::{Region, TenantId};

fn envelope(type_: &str, run_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev-{run_id}")),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: TenantId("t1".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )),
        subject: ArtifactRef(format!("myelin://t1/ci/run/{run_id}")),
        aggregate: AggregateKey("agg".into()),
        causation_id: None,
        correlation_id: CorrelationId("corr".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

#[test]
fn cdc_3_4_consumer_trigger_is_the_one_event_matcher() {
    let m = compile_trigger(&OnTrigger::PullRequest).expect("compiles to the frozen QueryAst");
    assert_eq!(m.object_type().0, RUN_OBJECT_TYPE, "selects the run object");
    assert!(
        trigger_matches(
            &m,
            &envelope(myelin_git::events::GIT_PR_OPENED, "r1"),
            &SetExpr::All,
            &|_| false,
        )
        .unwrap(),
        "the consumer's pull_request trigger fires on git.pr.opened"
    );
    assert!(
        !trigger_matches(
            &m,
            &envelope(myelin_git::events::GIT_REF_UPDATED, "r1"),
            &SetExpr::All,
            &|_| false,
        )
        .unwrap(),
        "and NOT on a push (the matcher discriminates by event.type)"
    );
}

#[test]
fn cdc_2_5_consumer_dedup_yields_exactly_one_run() {
    let mut ledger = DedupLedger::new();
    let ev = EventId("ev-push-1".into());
    let mut runs = 0u32;
    for _ in 0..3 {
        if ledger.record(TRIGGER_CONSUMER, &ev) {
            runs += 1;
        }
    }
    assert_eq!(
        runs, 1,
        "one push = exactly one run under at-least-once delivery"
    );
    assert_eq!(ledger.effect_count(), 1, "0 duplicate runs");
}

#[test]
fn cdc_4_9_consumer_trust_tier_stamped_consistently() {
    let fork = RunProvenance {
        is_fork: true,
        targets_self_hosted: false,
        read_excludes_fork: false,
    };
    assert_eq!(classify_trust(&fork), TrustTier::UntrustedFork);
    let stamp = stamp_trust(&fork);
    assert_eq!(stamp.job_tier, TrustTier::UntrustedFork, "JobSpec tier");
    assert_eq!(
        stamp.check_tier,
        GitTrustTier::UntrustedFork,
        "CheckStatus tier - the SAME value (X-1, 0 divergence)"
    );
    assert!(stamp.is_consistent());

    let member = RunProvenance {
        is_fork: false,
        targets_self_hosted: false,
        read_excludes_fork: true,
    };
    let stamp = stamp_trust(&member);
    assert_eq!(stamp.job_tier, TrustTier::Trusted);
    assert_eq!(stamp.check_tier, GitTrustTier::Trusted);
    assert!(stamp.is_consistent());
}
