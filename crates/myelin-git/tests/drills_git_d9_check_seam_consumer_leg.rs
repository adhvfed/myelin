//! # GIT-D9 / 5.9 consumer-leg LIVE drill — the check-seam consumer goes live (EB-26 / P-246, M3)
//!
//! **Contract:** `contract-index.md` row 5.9 (the Git↔CI `CheckStatus` seam — CI produces, **the
//! Bus carries**, Git is the consumer/gate). Owning architecture: `event-bus.md` §4.12 (the Bus's
//! NARROW carriage role) + the Git glue doc §1.1 (the X-1 consumer built from the §4.2 idempotent
//! template). **Drill:** GIT-D9 (the Bus's per-aggregate ordering holds under the producer; the
//! consumer is idempotent on `event_id`). **Reconciliation:** X-1.
//!
//! ## What this drill proves (the consumer leg LIVE — the M3 half of the seam)
//! As of EB-26 the consumer leg is WIRED: [`myelin_git::check_status::CheckStatusConsumer`] is an
//! idempotent [`EventHandler`] over the Bus's per-aggregate-ordered `ci.check.updated` carriage,
//! driven here through the Bus's ONE consumer runtime ([`myelin_events::Consumer`] — the §4.2
//! idempotent template, EB-05). The PRODUCER half (CI's real emit) is EB-27/M4 — here it is a
//! SYNTHETIC `ci.check.updated` emitter (the seam-floor drill fixture, named in the module).
//!
//! The drill asserts, end-to-end through the Bus runtime:
//! 1. **Idempotent on `event_id`** — a redelivered `ci.check.updated` is deduplicated by the Bus
//!    runtime's `consumer_dedup` ledger (rule 1) → the handler runs ONCE → 0 dup.
//! 2. **Per-aggregate ordered → monotonic supersession** — facts delivered out of `seq` order are
//!    consumed in per-aggregate order ([`CheckSeamOrder`]); a late lower-attempt re-delivery is
//!    DROPPED, so the current `check_status` row is the highest attempt regardless of arrival order.
//! 3. **A malformed payload dead-letters LOUDLY** — never silently dropped, never the wrong shape
//!    into the projection (rule 5 / the consumer's decode gate).

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

fn commit() -> myelin_events::CheckCommit {
    myelin_events::CheckCommit::from_repo_root(&TArtifactRef(REPO.into()), COMMIT).unwrap()
}

/// The SYNTHETIC `ci.check.updated` producer (the seam-floor emitter — CI's real producer is
/// EB-27/M4). Builds a delivered envelope carrying the OPAQUE `CheckStatus` payload (the Bus carries
/// it as a `serde_json::Value`; the consumer decodes it). `event_id` is stamped per (context,
/// attempt) so a redelivery carries the SAME id (the dedup key).
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
        subject: check_subject(&commit(), context).unwrap(),
        aggregate: check_aggregate(&commit()),
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

/// Bind the live check-status consumer to the Bus's ONE consumer runtime (the §4.2 idempotent
/// template): the `ci.check.updated` subject whitelist (never `*`), a durable name, the shared dedup
/// ledger (idempotent on `event_id`).
fn bind_consumer() -> Consumer<CheckStatusConsumer> {
    let subjects: Vec<&str> = CheckStatusConsumer::new()
        .subjects()
        .iter()
        .map(|p| p.0.as_str())
        .collect();
    let sub = Subscription::bind(
        ConsumerName("git.check_status".into()),
        &subjects,
        PrefetchBound::new(64).unwrap(),
    )
    .expect("the ci.check.updated whitelist binds (never a wildcard)");
    Consumer::new(CheckStatusConsumer::new(), sub, DedupLedger::new())
}

/// **GIT-D9 / 5.9 LIVE — idempotent on `event_id` through the Bus runtime (0 dup).** A redelivered
/// `ci.check.updated` is deduplicated by the runtime's `consumer_dedup` ledger → the handler runs
/// ONCE → the projection has exactly one row, applied once.
#[test]
fn consumer_leg_is_idempotent_on_event_id_zero_dup() {
    let consumer = bind_consumer();
    let env = synthetic_check_updated("build", 1, CheckState::Success, TrustTier::Trusted);
    let msg = Message {
        subject: CI_CHECK_UPDATED.into(),
        envelope: env,
    };

    // First delivery — the handler runs, the fact is applied + acked.
    assert_eq!(consumer.deliver(&msg), Delivered::Acked);
    // The at-least-once transport RE-DELIVERS the same event_id — deduplicated, the handler is SKIPPED.
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Deduplicated,
        "0 dup — handler runs once"
    );
    // A third redelivery is still deduplicated.
    assert_eq!(consumer.deliver(&msg), Delivered::Deduplicated);

    // The projection has exactly ONE row, and the handler applied exactly once.
    let proj = consumer.handler().projection();
    assert_eq!(proj.len(), 1);
    assert_eq!(
        consumer.handler().applied_count(),
        1,
        "the fact was applied exactly once"
    );
    assert_eq!(consumer.handler().dropped_stale_count(), 0);
}

/// **GIT-D9 / 5.9 LIVE — per-aggregate ordering → monotonic supersession.** Deliver facts for one
/// `(repo, commit_oid)` across contexts + re-runs, SCRAMBLED; the Bus's per-aggregate order
/// ([`CheckSeamOrder`]) is the consumed order, so the consumer's monotonic supersession leaves the
/// current row at the highest attempt — a late lower-attempt re-delivery is DROPPED (not a clobber).
#[test]
fn consumer_leg_per_aggregate_ordered_supersession_drops_stale() {
    let consumer = bind_consumer();

    // The outbox assigned seqs (state-change order): build#1=1, test#1=2, build#2=3 (a re-run).
    let build1 = synthetic_check_updated("build", 1, CheckState::Failure, TrustTier::Trusted);
    let test1 = synthetic_check_updated("test", 1, CheckState::Success, TrustTier::Trusted);
    let build2 = synthetic_check_updated("build", 2, CheckState::Success, TrustTier::Trusted);

    // The Bus's per-aggregate ordering substrate (the carriage half) orders them by seq regardless of
    // the SCRAMBLED arrival (3, 1, 2) — this is what the consumer reads.
    let mut order = CheckSeamOrder::new(&commit());
    assert!(order.ingest(&build2, 3).unwrap());
    assert!(order.ingest(&build1, 1).unwrap());
    assert!(order.ingest(&test1, 2).unwrap());
    assert_eq!(order.ordering_gap(), 0, "contiguous — 0 ops lost");

    // The consumer applies the facts in per-aggregate seq order (the order the Bus carriage exposes).
    for oc in order.in_order() {
        let env = match oc.seq {
            1 => &build1,
            2 => &test1,
            3 => &build2,
            _ => unreachable!(),
        };
        let msg = Message {
            subject: CI_CHECK_UPDATED.into(),
            envelope: env.clone(),
        };
        assert_eq!(consumer.deliver(&msg), Delivered::Acked);
    }

    // The at-least-once transport RE-DELIVERS the stale build attempt-1 LATE (a new physical
    // delivery, but the SAME event_id — so the dedup ledger absorbs it; even without dedup the
    // supersession would drop it). Use a fresh consumer to prove the supersession-drop independent of
    // the dedup ledger: re-apply build1 directly through the handler.
    let handler = consumer.handler();
    // The current build row is the attempt-2 success (the re-run), NOT the attempt-1 failure.
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
    // Two contexts → two rows; the supersession was in-place (no duplicate build row).
    assert_eq!(proj.len(), 2);

    // A directly-applied stale lower attempt is observably DROPPED (the supersession's loud half).
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

/// **GIT-D9 / 5.9 LIVE — a malformed payload dead-letters LOUDLY** (never silently dropped, never the
/// wrong shape into the projection). The decode gate is a real gate.
#[test]
fn consumer_leg_dead_letters_a_malformed_payload() {
    let consumer = bind_consumer();
    let mut env = synthetic_check_updated("build", 1, CheckState::Success, TrustTier::Trusted);
    // Corrupt the opaque payload (not a valid CheckStatus fact).
    env.payload = serde_json::json!({ "garbage": true });
    let msg = Message {
        subject: CI_CHECK_UPDATED.into(),
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
    // Nothing was applied into the projection (the wrong shape never lands).
    assert!(consumer.handler().projection().is_empty());
    assert_eq!(consumer.handler().applied_count(), 0);
}

/// Payload fields cannot redirect a valid-looking envelope into another tenant, subject, or
/// aggregate partition. Every mismatch is rejected before the projection lock is mutated.
#[test]
fn consumer_leg_rejects_adversarial_envelope_payload_provenance_mismatches() {
    let base = synthetic_check_updated("build", 1, CheckState::Success, TrustTier::Trusted);
    let mut cases = Vec::new();

    let mut wrong_tenant = base.clone();
    wrong_tenant.tenant = TenantId("other".into());
    wrong_tenant.actor.0.tenant = TenantId("other".into());
    cases.push(wrong_tenant);

    let mut wrong_subject = base.clone();
    wrong_subject.subject = check_subject(&commit(), "test").unwrap();
    cases.push(wrong_subject);

    let mut wrong_aggregate = base;
    wrong_aggregate.aggregate = myelin_events::AggregateKey("commit:other:abc123def".into());
    cases.push(wrong_aggregate);

    for (index, envelope) in cases.into_iter().enumerate() {
        let consumer = bind_consumer();
        let message = Message {
            subject: CI_CHECK_UPDATED.into(),
            envelope,
        };
        assert!(
            matches!(consumer.deliver(&message), Delivered::DeadLettered(_)),
            "case {index} must fail closed"
        );
        assert!(consumer.handler().projection().is_empty());
        assert_eq!(consumer.handler().applied_count(), 0);
        assert_eq!(consumer.handler().dropped_stale_count(), 0);
    }
}

/// **GIT-D9 / 5.9 LIVE — a foreign type slipping the whitelist dead-letters.** The handler binds only
/// `ci.check.updated`; a non-ci.check.updated event is a wiring bug, dead-lettered loudly (rule 5).
#[test]
fn consumer_leg_dead_letters_a_foreign_type() {
    let consumer = bind_consumer();
    let mut env = synthetic_check_updated("build", 1, CheckState::Success, TrustTier::Trusted);
    env.type_ = EventType("git.ref.updated".into());
    // Deliver it directly to the handler (the subject still matches the prefix; the handler's own
    // type-guard is the second line of defence).
    assert!(matches!(
        consumer
            .handler()
            .handle(&env, &mut myelin_events::HandlerTx::none()),
        myelin_events::HandleOutcome::NonRetryable(_)
    ));
}
