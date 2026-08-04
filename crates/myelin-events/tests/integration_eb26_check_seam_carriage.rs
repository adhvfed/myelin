#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::check_seam::{check_aggregate, check_subject, CheckSeamOrder};
use myelin_events::nats::NatsJetStreamBus;
use myelin_events::relay::{BusTransport, Delivery};
use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{
    Actor, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "abc123def";

fn check_env(context: &str, attempt: u64, state: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("eb26-{context}-a{attempt}")),
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
        payload: serde_json::json!({ "context": context, "run_attempt": attempt, "state": state }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_seam_carriage_over_real_nats() {
    let cfg = MyelinConfig::dev();
    let suffix = std::process::id();
    let stream = format!("MYELIN_EB26_{suffix}");
    let subject_root = format!("myelin_eb26_{suffix}");
    let consumer = format!("{stream}_pull");

    let bus = NatsJetStreamBus::connect(
        &cfg.nats_url,
        &stream,
        &subject_root,
        &consumer,
        tokio::runtime::Handle::current(),
    )
    .expect("connect NATS JetStream bus (is the stack up with -js?)");

    let build1 = check_env("build", 1, "failure");
    let test1 = check_env("test", 1, "success");
    let build2 = check_env("build", 2, "success");

    use myelin_events::ArtifactRef;
    let subj = |e: &EventEnvelope| ArtifactRef(format!("{subject_root}.{}", e.subject.0));

    assert_eq!(
        bus.put(&subj(&build1), &build1, &build1.event_id)
            .expect("put build1"),
        Delivery::Accepted
    );
    assert_eq!(
        bus.put(&subj(&test1), &test1, &test1.event_id)
            .expect("put test1"),
        Delivery::Accepted
    );
    assert_eq!(
        bus.put(&subj(&build2), &build2, &build2.event_id)
            .expect("put build2"),
        Delivery::Accepted
    );
    assert_eq!(
        bus.put(&subj(&build2), &build2, &build2.event_id).expect("re-put build2"),
        Delivery::Deduplicated,
        "a re-published ci.check.updated (same event_id) is deduplicated - at-least-once → effectively-once"
    );

    let delivered = bus.consume(&subject_root);
    assert_eq!(
        delivered.len(),
        3,
        "exactly 3 distinct facts (the duplicate build2 was deduped)"
    );

    let expected_agg = check_aggregate(REPO, COMMIT);
    let mut order = CheckSeamOrder::new(REPO, COMMIT);
    for (i, d) in delivered.iter().enumerate() {
        assert_eq!(
            d.type_.0, CI_CHECK_UPDATED,
            "carried type is ci.check.updated"
        );
        assert_eq!(
            d.aggregate, expected_agg,
            "carried aggregate = (repo, commit_oid)"
        );
        order
            .ingest(d, (i + 1) as u64)
            .expect("ingest into the per-aggregate order");
        bus.ack(&consumer, &d.event_id);
    }

    let build_attempts: Vec<u64> = order
        .in_order()
        .iter()
        .filter(|c| c.subject.0.ends_with("check-build"))
        .map(|c| c.check_status["run_attempt"].as_u64().unwrap())
        .collect();
    assert_eq!(
        build_attempts.iter().max().copied(),
        Some(2),
        "the carriage preserved both build attempts → supersession picks attempt 2 (the re-run)"
    );
    assert_eq!(
        order.ordering_gap(),
        0,
        "contiguous - 0 ops lost (at-least-once delivered every fact)"
    );

    bus.purge();
}
