#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::check_seam::{
    check_aggregate, check_updated_draft, ci_result_draft, ci_result_subject, rollup_ci_result,
    CiOverall,
};
use myelin_events::nats::NatsJetStreamBus;
use myelin_events::relay::{BusTransport, Delivery};
use myelin_events::taxonomy::new_tokens::{CI_CHECK_UPDATED, CI_RESULT};
use myelin_events::{
    Actor, ArtifactRef, CorrelationId, DataRole, EventDraft, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "abc123def";

fn envelope(draft: EventDraft, event_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.to_string()),
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

fn check_env(context: &str, attempt: u64, state: &str, event_id: &str) -> EventEnvelope {
    let draft = check_updated_draft(
        REPO,
        COMMIT,
        context,
        serde_json::json!({ "context": context, "run_attempt": attempt, "state": state }),
    );
    envelope(draft, event_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_seam_producer_carriage_over_real_nats() {
    let cfg = MyelinConfig::dev();
    let suffix = std::process::id();
    let stream = format!("MYELIN_EB27_{suffix}");
    let subject_root = format!("myelin_eb27_{suffix}");
    let consumer = format!("{stream}_pull");

    let bus = NatsJetStreamBus::connect(
        &cfg.nats_url,
        &stream,
        &subject_root,
        &consumer,
        tokio::runtime::Handle::current(),
    )
    .expect("connect NATS JetStream bus (is the stack up with -js?)");

    let build1 = check_env("build", 1, "failure", "eb27-build-a1");
    let test1 = check_env("test", 1, "success", "eb27-test-a1");
    let build2 = check_env("build", 2, "success", "eb27-build-a2");

    let subj = |e: &EventEnvelope| ArtifactRef(format!("{subject_root}.{}", e.subject.0));

    for e in [&build1, &test1, &build2] {
        assert_eq!(
            bus.put(&subj(e), e, &e.event_id).expect("put check fact"),
            Delivery::Accepted
        );
    }

    let mut current = BTreeMap::new();
    current.insert("build".to_string(), true);
    current.insert("test".to_string(), true);
    let required = vec!["build".to_string(), "test".to_string()];
    let rollup = rollup_ci_result(COMMIT, &current, &required, "merge-attempt-eb27");
    assert_eq!(rollup.overall, CiOverall::Success);

    let rollup_env = envelope(ci_result_draft(REPO, &rollup), "eb27-ci-result");
    assert_eq!(
        bus.put(&subj(&rollup_env), &rollup_env, &rollup_env.event_id)
            .expect("put ci.result"),
        Delivery::Accepted
    );
    assert_eq!(
        bus.put(&subj(&rollup_env), &rollup_env, &rollup_env.event_id)
            .expect("re-put ci.result"),
        Delivery::Deduplicated,
        "a re-published ci.result (same event_id) is deduplicated - at-least-once → effectively-once"
    );

    let delivered = bus.consume(&subject_root);
    assert_eq!(
        delivered.len(),
        4,
        "3 checks + 1 ci.result (the duplicate rollup was deduped)"
    );

    let expected_agg = check_aggregate(REPO, COMMIT);
    let mut saw_rollup = false;
    let mut saw_checks = 0;
    for d in &delivered {
        assert_eq!(
            d.aggregate, expected_agg,
            "carried aggregate = (repo, commit_oid) for BOTH the checks AND the rollup"
        );
        match d.type_.0.as_str() {
            CI_CHECK_UPDATED => saw_checks += 1,
            CI_RESULT => {
                saw_rollup = true;
                assert_eq!(d.subject, ci_result_subject(REPO, COMMIT));
                assert_eq!(d.payload["overall"], "success");
                assert_eq!(d.payload["idem_token"], "merge-attempt-eb27");
            }
            other => panic!("unexpected carried type: {other}"),
        }
        bus.ack(&consumer, &d.event_id);
    }
    assert_eq!(saw_checks, 3, "all 3 per-context facts carried");
    assert!(
        saw_rollup,
        "the ci.result rollup carried (the merge-queue wait substrate's signal)"
    );

    bus.purge();
}
