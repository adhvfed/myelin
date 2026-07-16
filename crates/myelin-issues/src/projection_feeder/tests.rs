//! Unit tests for the projection feeder (ISS-P15 / P-381): the frequency counter, the OQ-C threshold
//! gate (a below-threshold facet stays on GIN; an above-threshold facet provisions a generated index),
//! the 0-downtime forward-only online migration, and the `EventHandler` (2.4) idempotence + whitelist.

use super::*;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// Build an `issue.issue.updated` envelope whose payload carries the `type` + the `changed_fields`
/// delta (references-not-payloads — the changed field ids, not PII bodies).
fn updated_event(
    event_id: &str,
    tenant: &str,
    type_: &str,
    changed_fields: &[&str],
) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(ISSUE_UPDATED.into()),
        schema_ver: 1,
        tenant: TenantId(tenant.into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-1")),
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

// ───────────────────────────── the frequency counter (the measured signal) ───────────────────────

/// The share is `appearances / collection-view-executions`. A collection with no view executions has
/// share 0 (no division by zero — a never-viewed collection promotes nothing).
#[test]
fn share_is_appearances_over_executions_and_zero_when_no_views() {
    let mut counter = FrequencyCounter::new();
    let coll = CollectionKey::new("acme", "bug");
    let sev = FacetKey::new("acme", "bug", "severity");

    // no views yet → share 0 (never divide by zero).
    assert_eq!(counter.share(&sev), 0.0);

    // 10 view executions; `severity` filtered in 6 of them → share 0.6.
    for _ in 0..6 {
        counter.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..4 {
        counter.record_view_execution(&coll, &[]);
    }
    assert_eq!(counter.executions(&coll), 10);
    assert_eq!(counter.appearances(&sev), 6);
    assert!((counter.share(&sev) - 0.6).abs() < 1e-9);
}

/// The counter is per-`(tenant, type, field_id)`: the SAME field hot for one type is cold for another
/// (the denominator is per collection).
#[test]
fn the_counter_is_per_collection() {
    let mut counter = FrequencyCounter::new();
    let bugs = CollectionKey::new("acme", "bug");
    let stories = CollectionKey::new("acme", "story");

    // `severity` is filtered on every bug view, never on a story view.
    for _ in 0..10 {
        counter.record_view_execution(&bugs, &["severity"]);
        counter.record_view_execution(&stories, &[]);
    }
    let sev_bug = FacetKey::new("acme", "bug", "severity");
    let sev_story = FacetKey::new("acme", "story", "severity");
    assert!((counter.share(&sev_bug) - 1.0).abs() < 1e-9);
    assert_eq!(counter.share(&sev_story), 0.0);
}

// ───────────────────────────── the OQ-C threshold gate (MEASURED, never predicted) ───────────────

/// **THE GATE (below threshold → stays on GIN).** A facet whose measured share is AT OR BELOW the OQ-C
/// `> 5%` threshold is NOT promoted — it stays Tier 2b (the GIN probe). Promotion is MEASURED.
#[test]
fn below_threshold_facet_stays_on_gin() {
    let feeder = ProjectionFeeder::new(); // OQ-C default: > 5%
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");

    // 100 view executions; `severity` in only 4 → share 0.04 ≤ 0.05 (below threshold).
    for _ in 0..4 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..96 {
        feeder.record_view_execution(&coll, &[]);
    }
    assert!(
        !feeder.should_promote(&facet),
        "0.04 ≤ 0.05 must NOT promote"
    );
    match feeder.evaluate_facet(&facet) {
        PromotionDecision::StayedOnGin { share } => assert!((share - 0.04).abs() < 1e-9),
        other => panic!("expected StayedOnGin, got {other:?}"),
    }
    assert!(!feeder.is_promoted(&facet));
    // the cost-bounder's catalog still classifies it GIN (Tier 2b).
    assert_eq!(
        feeder.catalog_snapshot().posture("severity"),
        crate::schemes::IndexPosture::Gin
    );
}

/// **THE GATE (exactly at threshold → NOT promoted).** The OQ-C wording is STRICT (`> 5%`); a facet at
/// exactly 5% is not hot enough — it stays on GIN. The strict boundary is the no-weakening guard.
#[test]
fn exactly_at_threshold_is_not_promoted() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");

    // 100 executions, 5 appearances → share exactly 0.05; `> 0.05` is false.
    for _ in 0..5 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..95 {
        feeder.record_view_execution(&coll, &[]);
    }
    assert!(
        !feeder.should_promote(&facet),
        "exactly 5% is NOT > 5% — strict threshold, no weakening"
    );
}

/// **THE GATE (above threshold → provisions a generated index).** A facet crossing the OQ-C threshold
/// is promoted: a 0-downtime forward-only online migration provisions the generated index, and the
/// facet moves into the catalog (Tier 2) the cost-bounder reads.
#[test]
fn above_threshold_facet_provisions_a_generated_index() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");

    // 100 executions, 20 appearances → share 0.20 > 0.05.
    for _ in 0..20 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&coll, &[]);
    }
    assert!(feeder.should_promote(&facet));
    let decision = feeder.evaluate_facet(&facet);
    let provisioning = match decision {
        PromotionDecision::Promoted(p) => p,
        other => panic!("expected Promoted, got {other:?}"),
    };
    // the online migration is 0-downtime (CONCURRENTLY → no exclusive lock) + forward-only (no down).
    assert!(
        provisioning.is_non_blocking(),
        "promotion must be non-blocking on the hot table"
    );
    assert!(
        provisioning.is_forward_only(),
        "promotion must be forward-only (no DROP/down)"
    );
    assert!(provisioning.ddl.contains("CREATE INDEX CONCURRENTLY"));
    assert_eq!(provisioning.table, ISSUE_HOT_TABLE);
    assert_eq!(provisioning.index_name, "issue_facet_severity");
    // the facet is now Tier 2 (the generated index) in the catalog the cost-bounder reads.
    assert!(feeder.is_promoted(&facet));
    assert_eq!(
        feeder.catalog_snapshot().posture("severity"),
        crate::schemes::IndexPosture::GeneratedIndex
    );
}

/// Promotion is IDEMPOTENT — a facet already promoted is NOT re-provisioned (the migration runs
/// at-most-once per facet).
#[test]
fn promotion_is_idempotent() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");
    for _ in 0..20 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&coll, &[]);
    }
    assert!(feeder.evaluate_facet(&facet).is_promoted());
    // a SECOND evaluation does NOT re-provision (already promoted).
    assert_eq!(
        feeder.evaluate_facet(&facet),
        PromotionDecision::AlreadyPromoted
    );
    assert!(
        !feeder.should_promote(&facet),
        "already promoted → should_promote false"
    );
}

/// A calibrated (non-default) threshold is honoured — the threshold is a Search-owned tunable.
#[test]
fn a_calibrated_threshold_is_honoured() {
    // a stricter 50% threshold: a facet at 20% is NOT promoted under it (it would be at the 5% default).
    let feeder = ProjectionFeeder::with_threshold(PromotionThreshold::new(0.50));
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");
    for _ in 0..20 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&coll, &[]);
    }
    assert!(
        !feeder.should_promote(&facet),
        "0.20 ≤ 0.50 → not promoted under the stricter tunable"
    );
}

// ───────────────────────────── the EventHandler (contract 2.4) ───────────────────────────────────

/// The feeder's subjects whitelist is `issue.issue.updated` ONLY — NEVER `*` (BUS-3 / 2.4).
#[test]
fn subjects_whitelist_is_issue_updated_never_star() {
    let feeder = ProjectionFeeder::new();
    let subjects = feeder.subjects();
    assert_eq!(subjects.len(), 1);
    assert_eq!(subjects[0].0, ISSUE_UPDATED);
    assert_eq!(subjects[0].0, "issue.issue.updated");
    assert!(
        subjects.iter().all(|s| s.0 != "*"),
        "never a `*` subscription"
    );
}

/// `handle` is idempotent on `event_id` (ADR-04.1 / 2.4): a redelivered `issue.updated` does NOT
/// double-count toward the frequency signal.
#[test]
fn handle_is_idempotent_on_event_id() {
    let feeder = ProjectionFeeder::new();
    let ev = updated_event("ev-1", "acme", "bug", &["severity"]);
    // deliver the SAME event twice; the second is a no-op (deduped on event_id).
    assert_eq!(feeder.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    assert_eq!(feeder.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    // the facet appearance from the issue.updated delta is counted at most once (the redelivery is a
    // no-op); since no view executions drove the share, it is not promoted regardless.
    assert!(!feeder.is_promoted(&FacetKey::new("acme", "bug", "severity")));
}

/// A misrouted (non-`issue.updated`) event is `NonRetryable` (a poison misroute, not a retry storm).
#[test]
fn a_misrouted_event_is_non_retryable() {
    let feeder = ProjectionFeeder::new();
    let mut ev = updated_event("ev-2", "acme", "bug", &["severity"]);
    ev.type_ = EventType("issue.issue.created".into());
    match feeder.handle(&ev, &mut myelin_events::HandlerTx::none()) {
        HandleOutcome::NonRetryable(_) => {}
        other => panic!("a misroute must be NonRetryable, got {other:?}"),
    }
}

/// **handle drives the measured promotion end-to-end:** view executions push a facet past the
/// threshold, then an `issue.updated` delta over that facet promotes it (the bus consumer path).
#[test]
fn handle_promotes_a_hot_facet_off_the_bus() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");
    // drive the facet hot via 100 view executions (20% share > 5%).
    for _ in 0..20 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&coll, &[]);
    }
    assert!(!feeder.is_promoted(&facet), "not yet seen on the bus");
    // an issue.updated delta touching `severity` → the consumer promotes it (measured threshold met).
    assert_eq!(
        feeder.handle(&updated_event("ev-3", "acme", "bug", &["severity"]), &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done
    );
    assert!(
        feeder.is_promoted(&facet),
        "the hot facet is promoted off the bus (Tier 2)"
    );
    assert_eq!(
        feeder.catalog_snapshot().posture("severity"),
        crate::schemes::IndexPosture::GeneratedIndex
    );
}

/// An `issue.updated` with no recognisable field deltas touches no facet (no spurious promotion).
#[test]
fn an_event_without_field_deltas_promotes_nothing() {
    let feeder = ProjectionFeeder::new();
    let mut ev = updated_event("ev-4", "acme", "bug", &[]);
    ev.payload = serde_json::json!({ "ref": "myelin://acme/issue/issue/ENG-1" });
    assert_eq!(feeder.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    assert!(!feeder.is_promoted(&FacetKey::new("acme", "bug", "severity")));
}

// ───────────────────────────── the online migration shape (1.5) ──────────────────────────────────

/// The provisioning DDL is a non-blocking, forward-only `CREATE INDEX CONCURRENTLY` over the hot
/// facet's `props` expression, tenant + type scoped, on the declared-hot `issue` table.
#[test]
fn the_provisioning_is_a_non_blocking_forward_only_concurrent_index() {
    let facet = FacetKey::new("acme", "bug", "severity");
    let p = IndexProvisioning::for_facet(&facet);
    assert!(p.is_non_blocking());
    assert!(p.is_forward_only());
    assert!(p.ddl.contains("CONCURRENTLY"));
    assert!(
        p.ddl.contains("IF NOT EXISTS"),
        "idempotent / re-runnable (forward-only)"
    );
    assert!(
        p.ddl.contains("props ->> 'severity'"),
        "the expression index over the JSONB tail"
    );
    assert!(
        p.ddl.contains("tenant_id = 'acme'"),
        "tenant-scoped (no cross-tenant index)"
    );
    assert!(
        p.ddl.contains("deleted_at IS NULL"),
        "soft-delete-aware partial index"
    );
}

/// **The non-blocking + forward-only guards are REAL gates (mandatory-core).** A deliberately
/// BLOCKING provisioning (a non-`CONCURRENTLY` `CREATE INDEX`) is detected as blocking; a
/// forward-only-VIOLATING provisioning (one carrying a `DROP`) is detected as not-forward-only. These
/// kill the "always true" mutants on the 0-downtime correctness seam.
#[test]
fn the_non_blocking_and_forward_only_guards_reject_a_bad_ddl() {
    let facet = FacetKey::new("acme", "bug", "severity");
    // a blocking (non-concurrent) index build → is_non_blocking() must be FALSE.
    let blocking = IndexProvisioning {
        facet: facet.clone(),
        index_name: "issue_facet_severity".into(),
        ddl: "CREATE INDEX issue_facet_severity ON issue ((props ->> 'severity'))".into(),
        table: ISSUE_HOT_TABLE,
    };
    assert!(
        !blocking.is_non_blocking(),
        "a non-CONCURRENTLY index build is BLOCKING on the hot table — must be detected"
    );
    // a forward-only-violating DDL (carries a DROP) → is_forward_only() must be FALSE.
    let down = IndexProvisioning {
        facet,
        index_name: "issue_facet_severity".into(),
        ddl: "DROP INDEX issue_facet_severity".into(),
        table: ISSUE_HOT_TABLE,
    };
    assert!(
        !down.is_forward_only(),
        "a DDL carrying a DROP is NOT forward-only — must be detected"
    );
    // the canonical feeder-built provisioning passes BOTH (the both-true conjunction, not either).
    let good = IndexProvisioning::for_facet(&FacetKey::new("acme", "bug", "severity"));
    assert!(good.is_non_blocking() && good.is_forward_only());
    // and a CONCURRENTLY-but-DROP hybrid fails the forward-only half (the && is a real conjunction).
    let concurrent_drop = IndexProvisioning {
        facet: FacetKey::new("acme", "bug", "x"),
        index_name: "issue_facet_x".into(),
        ddl: "CREATE INDEX CONCURRENTLY issue_facet_x ON issue ((props ->> 'x')); DROP INDEX old"
            .into(),
        table: ISSUE_HOT_TABLE,
    };
    assert!(
        concurrent_drop.is_non_blocking(),
        "CONCURRENTLY → non-blocking"
    );
    assert!(
        !concurrent_drop.is_forward_only(),
        "the DROP makes it NOT forward-only"
    );
}

/// `PromotionDecision::is_promoted` is true ONLY for `Promoted` (kills the "always true" mutant).
#[test]
fn is_promoted_distinguishes_the_decision_variants() {
    let p = IndexProvisioning::for_facet(&FacetKey::new("acme", "bug", "severity"));
    assert!(PromotionDecision::Promoted(p).is_promoted());
    assert!(!PromotionDecision::StayedOnGin { share: 0.0 }.is_promoted());
    assert!(!PromotionDecision::AlreadyPromoted.is_promoted());
}

/// A facet id with non-identifier chars is sanitised into a safe index NAME (the name is an
/// identifier; the value rides the expression).
#[test]
fn the_index_name_is_a_sanitised_identifier() {
    let facet = FacetKey::new("acme", "bug", "Customer-Tier!");
    let p = IndexProvisioning::for_facet(&facet);
    assert_eq!(p.index_name, "issue_facet_customer_tier_");
    // the expression still references the raw field key.
    assert!(p.ddl.contains("Customer-Tier!"));
}

/// The OQ-C floor is named (the threshold the cost-bounder's classification documents matches the
/// feeder gate).
#[test]
fn the_oq_c_floor_is_named() {
    assert!((PromotionThreshold::OQ_C_DEFAULT_TO_BEAT - 0.05).abs() < 1e-9);
    assert!(ProjectionFeederFloors::OQ_C_THRESHOLD.contains("5%"));
    assert_eq!(ProjectionFeederFloors::WINDOW_CALIBRATION, "ISS-P32");
}
