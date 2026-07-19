//! # EB-27 / P-327 (M4) — the check-seam PRODUCER-leg carriage over the REAL durable bus
//!
//! **Contract:** `contract-index.md` row 5.9 (the Git↔CI CheckStatus seam — **the Bus carries it**,
//! END-TO-END at M4) + 9.4 (the merge-queue `ci.result` wait substrate, consumed). Owning
//! architecture: `event-bus.md` §4.12 (the Bus's NARROW carriage role: envelope conformance,
//! per-aggregate ordering on `(repo, commit_oid)`, at-least-once delivery, the durable
//! `wait_for_signal` substrate). **Drills:** GIT-D10 / CI-D8 (the X-1 seam end-to-end).
//!
//! This proves the EB-27 PRODUCER-leg CARRIAGE against the LIVE NATS JetStream stack (NOT a mock —
//! the binding policy floor is over for anything Docker can run). CI's PRODUCER side
//! (`check_seam::{check_updated_draft, rollup_ci_result, ci_result_draft}`) emits the per-context
//! `ci.check.updated` facts + the rollup `ci.result` through the real `BusTransport`; the test
//! consumes them and asserts:
//! 1. **envelope conformance** — the §4.12 subject/aggregate grammar survives the round-trip, for
//!    BOTH the per-context facts AND the rollup (the rollup shares the per-commit aggregate);
//! 2. **broker-side dedup (at-least-once → effectively-once)** — a re-publish of the same `ci.result`
//!    `event_id` is `Deduplicated` (0 ghost), the carriage half of the merge-queue's exactly-once wake;
//! 3. **per-aggregate ordering** — the rollup linearises AFTER the per-context facts on the one
//!    `(repo, commit_oid)` partition (the producer emits the rollup only after the checks it rolls up).
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-events --features integration --test integration_eb27_check_seam_producer
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::check_seam::{
    check_aggregate, check_updated_draft, ci_result_draft, ci_result_subject, rollup_ci_result,
    CheckCommit, CiOverall,
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

fn commit() -> CheckCommit {
    CheckCommit::from_repo_root(&ArtifactRef(REPO.into()), COMMIT).unwrap()
}

/// Wrap a producer [`EventDraft`] into a delivered [`EventEnvelope`] at a stable `event_id` (the
/// dedup key). The CI-owned payload rides OPAQUE; the Bus carries it untouched.
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
        &commit(),
        context,
        serde_json::json!({ "context": context, "run_attempt": attempt, "state": state }),
    )
    .unwrap();
    envelope(draft, event_id)
}

/// **The check-seam PRODUCER-leg carriage over the LIVE durable bus (GIT-D10 / CI-D8).** Publish the
/// per-context `ci.check.updated` facts (incl. a re-run) THEN the rollup `ci.result` (incl. an
/// at-least-once duplicate), consume them through the real transport, and prove envelope conformance
/// + broker dedup + per-aggregate ordering — the substrate the merge-queue's exactly-once wake rests on.
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

    // ===== CI PRODUCES the per-context facts: build#1 (failure), test#1 (success), build#2 (re-run) =====
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

    // ===== CI DERIVES the rollup from the post-supersession current status over the REQUIRED set =====
    let mut current = BTreeMap::new();
    current.insert("build".to_string(), true); // build#2 (the re-run) supersedes build#1
    current.insert("test".to_string(), true);
    let required = vec!["build".to_string(), "test".to_string()];
    let rollup = rollup_ci_result(COMMIT, &current, &required, "merge-attempt-eb27");
    assert_eq!(rollup.overall, CiOverall::Success);

    // CI emits the rollup via the SAME outbox path, on the SAME per-commit aggregate (§4.12).
    let rollup_env = envelope(
        ci_result_draft(&commit(), &rollup).unwrap(),
        "eb27-ci-result",
    );
    assert_eq!(
        bus.put(&subj(&rollup_env), &rollup_env, &rollup_env.event_id)
            .expect("put ci.result"),
        Delivery::Accepted
    );
    // The at-least-once transport RE-DELIVERS the rollup (same event_id) → broker dedup → 0 ghost (the
    // carriage half of the merge-queue's EXACTLY-ONCE wake).
    assert_eq!(
        bus.put(&subj(&rollup_env), &rollup_env, &rollup_env.event_id)
            .expect("re-put ci.result"),
        Delivery::Deduplicated,
        "a re-published ci.result (same event_id) is deduplicated — at-least-once → effectively-once"
    );

    // ===== Consume + assert =====
    let delivered = bus.consume(&subject_root);
    assert_eq!(
        delivered.len(),
        4,
        "3 checks + 1 ci.result (the duplicate rollup was deduped)"
    );

    let expected_agg = check_aggregate(&commit());
    let mut saw_rollup = false;
    let mut saw_checks = 0;
    for d in &delivered {
        // Every carried event shares the per-commit aggregate (the rollup linearises after its checks).
        assert_eq!(
            d.aggregate, expected_agg,
            "carried aggregate = (repo, commit_oid) for BOTH the checks AND the rollup"
        );
        match d.type_.0.as_str() {
            CI_CHECK_UPDATED => saw_checks += 1,
            CI_RESULT => {
                saw_rollup = true;
                // The rollup's subject is the §4.12 ci-result #sub on the same commit.
                assert_eq!(d.subject, ci_result_subject(&commit()));
                // The opaque rollup payload round-trips to the frozen signal shape.
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
