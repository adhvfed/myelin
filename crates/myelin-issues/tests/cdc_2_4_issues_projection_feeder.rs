//! # The CDC pair for contract 2.4 — the **`issue.updated` projection-feeder consumer** (ISS-P15 / P-381)
//!
//! **Contract-index row 2.4** is the `EventHandler` consumer template — a `subjects()` whitelist
//! (never `*`) plus `handle → {Done | NonRetryable | Retry}`; durable-bind-by-name, ack-after-enqueue,
//! dedup ledger, bounded prefetch, lag metric. The Bus OWNS the template plus the runtime
//! ([`myelin_events::consume`] / [`myelin_events::Consumer`]); each subsystem AUTHORS a consumer
//! `handle` body. THIS file pins the Issues slice: the projection feeder
//! ([`myelin_issues::projection_feeder::ProjectionFeeder`]) is the consumer ISS-P15 ships, watching
//! `issue.issue.updated`.
//!
//! The **PRODUCER** (the provider side) is **Issues authoring an `EventHandler`** whose `subjects()`
//! whitelist is `issue.issue.updated` ONLY (never `*`) and whose `handle` returns a terminal
//! [`myelin_events::HandleOutcome`]. The producer's promise: it binds a `*`-free whitelist and its
//! handle is idempotent on `event_id` (the §3 measured-promotion path, never a second consumer
//! template — EI-01 §7).
//!
//! The **CONSUMER** is the **Bus consumer runtime admitting the handler** ([`myelin_events::consume`])
//! into a live [`myelin_events::Consumer`] WITHOUT a wildcard-subscription rejection, then delivering
//! an `issue.updated` message through the seven rules (the only honest definition of "accepted" — the
//! Bus is the authority over the consumer template).
//!
//! The two sides are pinned here so a drift on either (Issues widens its subscription to `*`, or
//! authors a non-idempotent handle; the Bus renames the template surface) fails this test in the same
//! CI job. The measured-threshold promotion + the 0-downtime online migration are the projection
//! feeder's OWN unit gate (`src/projection_feeder/tests.rs`); THIS CDC is the mechanical evidence that
//! the frozen 2.4 consumer shape REGISTERS (a `*`-free whitelist) and is ADMITTED + DRIVEN by the Bus
//! runtime.

use myelin_events::{
    consume, Actor, AggregateKey, ArtifactRef, ConsumerName, ConsumerSpec, CorrelationId, DataRole,
    DedupLedger, Delivered, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome,
    Message, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events::ISSUE_UPDATED;
use myelin_issues::projection_feeder::{CollectionKey, FacetKey, ProjectionFeeder};
use myelin_tenancy::{Region, TenantId};

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn updated_event(event_id: &str, type_: &str, changed_fields: &[&str]) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(ISSUE_UPDATED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        subject: ArtifactRef("myelin://acme/issue/issue/ENG-1".into()),
        aggregate: AggregateKey("issue:ENG-1".into()),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
        payload: serde_json::json!({ "type": type_, "changed_fields": changed_fields }),
    }
}

/// **PRODUCER side — Issues authors a `*`-free `EventHandler` whose subjects whitelist is
/// `issue.issue.updated`.** Pins the frozen 2.4 promise: the feeder binds a whitelist (never `*`) and
/// returns a terminal outcome.
#[test]
fn producer_issues_authors_a_star_free_issue_updated_handler() {
    let feeder = ProjectionFeeder::new();
    let subjects = feeder.subjects();
    assert_eq!(subjects.len(), 1, "the feeder binds exactly one subject");
    assert_eq!(subjects[0].0, ISSUE_UPDATED);
    assert_eq!(subjects[0].0, "issue.issue.updated");
    assert!(
        subjects.iter().all(|s| s.0 != "*" && s.0 != ">"),
        "the feeder NEVER binds a wildcard subscription (BUS-3)"
    );
    // handle returns a terminal HandleOutcome (Done for a well-formed issue.updated).
    let outcome = feeder.handle(&updated_event("p-1", "bug", &["severity"]));
    assert_eq!(outcome, HandleOutcome::Done);
}

/// **CONSUMER side — the Bus runtime ADMITS the feeder (no wildcard rejection) + DRIVES it.** The
/// only honest "accepted": the feeder's `*`-free whitelist binds into a live [`Consumer`], and an
/// `issue.updated` message delivers through the seven rules to a terminal `Acked`.
#[test]
fn consumer_bus_admits_and_drives_the_feeder() {
    let feeder = ProjectionFeeder::new();
    let spec = ConsumerSpec::new(
        ConsumerName("issues.projection_feeder".into()),
        &[ISSUE_UPDATED],
    );
    let consumer =
        consume(spec, feeder, DedupLedger::new()).expect("the *-free feeder whitelist must bind");

    let msg = Message {
        subject: ISSUE_UPDATED.into(),
        envelope: updated_event("c-1", "bug", &["severity"]),
    };
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Acked,
        "a well-formed issue.updated acks (Done)"
    );
    // rule 1 (dedup): the SAME event redelivered is deduplicated, not re-handled.
    assert_eq!(consumer.deliver(&msg), Delivered::Deduplicated);
    assert_eq!(consumer.lag(), 0, "no backlog after a clean ack + dedup");
}

/// **The consumer drives the MEASURED promotion through the Bus runtime end-to-end.** A facet driven
/// hot (via view executions) is promoted when its `issue.updated` delta is DELIVERED by the runtime —
/// the feeder's catalog moves the facet to Tier 2 (the generated index the cost-bounder reads).
#[test]
fn consumer_drives_the_measured_promotion_through_the_runtime() {
    let feeder = ProjectionFeeder::new();
    // drive `severity` hot in `acme`/`bug` (20% share > 5% OQ-C threshold) BEFORE wiring the consumer.
    let coll = CollectionKey::new("acme", "bug");
    for _ in 0..20 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&coll, &[]);
    }
    let spec = ConsumerSpec::new(
        ConsumerName("issues.projection_feeder".into()),
        &[ISSUE_UPDATED],
    );
    let consumer = consume(spec, feeder, DedupLedger::new()).expect("binds");

    // the issue.updated delta over the hot facet — delivered by the Bus runtime → promotion.
    let msg = Message {
        subject: ISSUE_UPDATED.into(),
        envelope: updated_event("c-2", "bug", &["severity"]),
    };
    assert_eq!(consumer.deliver(&msg), Delivered::Acked);
    // the handler (the feeder) has promoted the facet to the generated index (Tier 2).
    let facet = FacetKey::new("acme", "bug", "severity");
    assert!(
        consumer.handler().is_promoted(&facet),
        "the hot facet is promoted off the bus (Tier 2) once its issue.updated delivers"
    );
    assert_eq!(
        consumer.handler().catalog_snapshot().posture("severity"),
        myelin_issues::schemes::IndexPosture::GeneratedIndex
    );
}

/// A `*` subscription is REJECTED at bind (the structural BUS-3 guard) — a consumer CANNOT widen the
/// feeder's whitelist to a wildcard. (The provider never asks for one; this pins that the runtime
/// would refuse if it did.)
#[test]
fn a_wildcard_subscription_is_rejected() {
    let feeder = ProjectionFeeder::new();
    let spec = ConsumerSpec::new(ConsumerName("issues.bad".into()), &["*"]);
    assert!(
        consume(spec, feeder, DedupLedger::new()).is_err(),
        "a `*` subscription must be rejected at registration (BUS-3)"
    );
}
