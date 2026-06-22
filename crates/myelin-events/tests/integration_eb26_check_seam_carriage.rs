//! # EB-26 / P-246 (M3) — the check-seam CONSUMER-leg carriage over the REAL durable bus
//!
//! **Contract:** `contract-index.md` row 5.9 (the Git↔CI CheckStatus seam — **the Bus carries it**).
//! Owning architecture: `event-bus.md` §4.12 (the Bus's NARROW carriage role: envelope conformance,
//! per-aggregate ordering on `(repo, commit_oid)`, at-least-once delivery). **Drill:** GIT-D9 (the
//! Bus's per-aggregate ordering holds under the producer; broker-side dedup → 0 dup).
//!
//! This proves the EB-26 consumer-leg CARRIAGE against the LIVE NATS JetStream stack (NOT a mock —
//! the binding policy floor is over for anything Docker can run). It publishes `ci.check.updated`
//! envelopes carrying the OPAQUE `CheckStatus` payload through the real `BusTransport`, consumes
//! them, and asserts:
//! 1. **envelope conformance** — the §4.12 subject/aggregate grammar survives the round-trip;
//! 2. **broker-side dedup (at-least-once → effectively-once)** — a re-publish of the same `event_id`
//!    is `Deduplicated` (0 ghost), the carriage half of the consumer's idempotency;
//! 3. **per-aggregate ordering** — facts for one `(repo, commit_oid)` are carried so the consumer can
//!    apply monotonic `run_attempt` supersession (the highest attempt is the current state).
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-events --features integration --test integration_eb26_check_seam_carriage
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

/// A `ci.check.updated` envelope carrying the OPAQUE CheckStatus payload (run_attempt + state), at a
/// stable `event_id` per (context, attempt) so a re-publish carries the SAME id (the dedup key).
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
        // The CI-owned CheckStatus rides OPAQUE — the Bus carries it untouched.
        payload: serde_json::json!({ "context": context, "run_attempt": attempt, "state": state }),
    }
}

/// **The check-seam consumer-leg carriage over the LIVE durable bus.** Publish interleaved
/// `ci.check.updated` facts (incl. a re-run + a duplicate re-publish), consume them through the real
/// transport, and prove envelope conformance + broker dedup + per-aggregate ordering — the substrate
/// Git's monotonic supersession rests on.
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

    // The producer (synthetic — CI's real producer is EB-27/M4) publishes the facts for one commit:
    // build#1 (failure), test#1 (success), build#2 (a re-run, success). Plus a DUPLICATE re-publish
    // of build#2 (the at-least-once transport) — broker dedup must suppress it (0 ghost).
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
    // The DUPLICATE re-publish of build2 (same event_id) — broker-side dedup → 0 ghost.
    assert_eq!(
        bus.put(&subj(&build2), &build2, &build2.event_id).expect("re-put build2"),
        Delivery::Deduplicated,
        "a re-published ci.check.updated (same event_id) is deduplicated — at-least-once → effectively-once"
    );

    // The consumer leg drains the carriage and feeds the Bus's per-aggregate ordering substrate.
    let delivered = bus.consume(&subject_root);
    assert_eq!(
        delivered.len(),
        3,
        "exactly 3 distinct facts (the duplicate build2 was deduped)"
    );

    // Envelope conformance survived the round-trip: every fact carries the §4.12 aggregate.
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
        // Feed the ordering substrate at the delivery seq (the carriage's per-aggregate order key).
        order
            .ingest(d, (i + 1) as u64)
            .expect("ingest into the per-aggregate order");
        bus.ack(&consumer, &d.event_id);
    }

    // The Bus carried every fact for the commit → the consumer can apply monotonic supersession: the
    // current build status is the HIGHEST run_attempt (the re-run, build2/attempt-2 success).
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
        "contiguous — 0 ops lost (at-least-once delivered every fact)"
    );

    bus.purge();
}
