use super::*;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const BUG_TYPE_ID: &str = "22222222-2222-2222-2222-222222222222";

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn updated_event(
    event_id: &str,
    tenant: &str,
    type_id: &str,
    changed_facets: &[&str],
) -> EventEnvelope {
    let issue = format!("myelin://{tenant}/issue/issue/ENG-1");
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(ISSUE_UPDATED.into()),
        schema_ver: 1,
        tenant: TenantId(tenant.into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(issue.clone()),
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
        payload: serde_json::json!({
            "issue": issue,
            "issue_local_id": "ENG-1",
            "type_id": type_id,
            "changed_facets": changed_facets,
        }),
    }
}

#[test]
fn share_is_appearances_over_executions_and_zero_when_no_views() {
    let mut counter = FrequencyCounter::new();
    let coll = CollectionKey::new("acme", "bug");
    let sev = FacetKey::new("acme", "bug", "severity");

    assert_eq!(counter.share(&sev), 0.0);

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

#[test]
fn the_counter_is_per_collection() {
    let mut counter = FrequencyCounter::new();
    let bugs = CollectionKey::new("acme", "bug");
    let stories = CollectionKey::new("acme", "story");

    for _ in 0..10 {
        counter.record_view_execution(&bugs, &["severity"]);
        counter.record_view_execution(&stories, &[]);
    }
    let sev_bug = FacetKey::new("acme", "bug", "severity");
    let sev_story = FacetKey::new("acme", "story", "severity");
    assert!((counter.share(&sev_bug) - 1.0).abs() < 1e-9);
    assert_eq!(counter.share(&sev_story), 0.0);
}

#[test]
fn below_threshold_facet_stays_on_gin() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");

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
    assert_eq!(
        feeder.catalog_snapshot().posture("severity"),
        crate::schemes::IndexPosture::Gin
    );
}

#[test]
fn exactly_at_threshold_is_not_promoted() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");

    for _ in 0..5 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..95 {
        feeder.record_view_execution(&coll, &[]);
    }
    assert!(
        !feeder.should_promote(&facet),
        "exactly 5% is NOT > 5% - strict threshold, no weakening"
    );
}

#[test]
fn above_threshold_facet_provisions_a_generated_index() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", "bug");
    let facet = FacetKey::new("acme", "bug", "severity");

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
    assert!(provisioning.index_name.starts_with("issue_facet_severity_"));
    assert!(provisioning.index_name.len() <= 63);
    assert!(feeder.is_promoted(&facet));
    assert_eq!(
        feeder.catalog_snapshot().posture("severity"),
        crate::schemes::IndexPosture::GeneratedIndex
    );
}

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
    assert_eq!(
        feeder.evaluate_facet(&facet),
        PromotionDecision::AlreadyPromoted
    );
    assert!(
        !feeder.should_promote(&facet),
        "already promoted → should_promote false"
    );
}

#[test]
fn a_calibrated_threshold_is_honoured() {
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

#[test]
fn handle_is_idempotent_on_event_id() {
    let feeder = ProjectionFeeder::new();
    let ev = updated_event("ev-1", "acme", BUG_TYPE_ID, &["severity"]);
    assert_eq!(
        feeder.handle(&ev, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done
    );
    assert_eq!(
        feeder.handle(&ev, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done
    );
    assert!(!feeder.is_promoted(&FacetKey::new("acme", BUG_TYPE_ID, "severity")));
}

#[test]
fn a_misrouted_event_is_non_retryable() {
    let feeder = ProjectionFeeder::new();
    let mut ev = updated_event("ev-2", "acme", BUG_TYPE_ID, &["severity"]);
    ev.type_ = EventType("issue.issue.created".into());
    match feeder.handle(&ev, &mut myelin_events::HandlerTx::none()) {
        HandleOutcome::NonRetryable(_) => {}
        other => panic!("a misroute must be NonRetryable, got {other:?}"),
    }
}

#[test]
fn handle_promotes_a_hot_facet_off_the_bus() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", BUG_TYPE_ID);
    let facet = FacetKey::new("acme", BUG_TYPE_ID, "severity");
    for _ in 0..20 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&coll, &[]);
    }
    assert!(!feeder.is_promoted(&facet), "not yet seen on the bus");
    assert_eq!(
        feeder.handle(
            &updated_event("ev-3", "acme", BUG_TYPE_ID, &["severity"]),
            &mut myelin_events::HandlerTx::none()
        ),
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

#[test]
fn an_event_without_projection_metadata_is_rejected() {
    let feeder = ProjectionFeeder::new();
    let mut ev = updated_event("ev-4", "acme", BUG_TYPE_ID, &[]);
    ev.payload = serde_json::json!({ "ref": "myelin://acme/issue/issue/ENG-1" });
    assert!(matches!(
        feeder.handle(&ev, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::NonRetryable(_)
    ));
    assert!(!feeder.is_promoted(&FacetKey::new("acme", BUG_TYPE_ID, "severity")));
}

#[test]
fn a_malformed_facet_array_has_no_partial_effect_and_does_not_poison_replay() {
    let feeder = ProjectionFeeder::new();
    let collection = CollectionKey::new("acme", BUG_TYPE_ID);
    let severity = FacetKey::new("acme", BUG_TYPE_ID, "severity");
    for _ in 0..20 {
        feeder.record_view_execution(&collection, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&collection, &[]);
    }

    let mut malformed = updated_event("ev-poison", "acme", BUG_TYPE_ID, &["severity"]);
    malformed.payload["changed_facets"] = serde_json::json!(["severity", 7]);
    assert!(matches!(
        feeder.handle(&malformed, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::NonRetryable(_)
    ));
    assert!(
        !feeder.is_promoted(&severity),
        "the valid prefix of a malformed array must have no effect"
    );

    let corrected = updated_event("ev-poison", "acme", BUG_TYPE_ID, &["severity"]);
    assert_eq!(
        feeder.handle(&corrected, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done,
        "validation must happen before the feeder records the event id"
    );
    assert!(feeder.is_promoted(&severity));
}

#[test]
fn an_issue_subject_cannot_cross_the_envelope_tenant() {
    let feeder = ProjectionFeeder::new();
    let mut event = updated_event("ev-foreign", "acme", BUG_TYPE_ID, &["severity"]);
    event.subject = ArtifactRef("myelin://globex/issue/issue/ENG-1".into());
    event.payload["issue"] = serde_json::Value::String(event.subject.0.clone());

    let outcome = feeder.handle(&event, &mut myelin_events::HandlerTx::none());
    assert!(matches!(outcome, HandleOutcome::NonRetryable(_)));
    assert!(!feeder.is_promoted(&FacetKey::new("acme", BUG_TYPE_ID, "severity")));
}

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

#[test]
fn the_non_blocking_and_forward_only_guards_reject_a_bad_ddl() {
    let facet = FacetKey::new("acme", "bug", "severity");
    let blocking = IndexProvisioning {
        facet: facet.clone(),
        index_name: "issue_facet_severity".into(),
        ddl: "CREATE INDEX issue_facet_severity ON issue ((props ->> 'severity'))".into(),
        table: ISSUE_HOT_TABLE,
    };
    assert!(
        !blocking.is_non_blocking(),
        "a non-CONCURRENTLY index build is BLOCKING on the hot table - must be detected"
    );
    let down = IndexProvisioning {
        facet,
        index_name: "issue_facet_severity".into(),
        ddl: "DROP INDEX issue_facet_severity".into(),
        table: ISSUE_HOT_TABLE,
    };
    assert!(
        !down.is_forward_only(),
        "a DDL carrying a DROP is NOT forward-only - must be detected"
    );
    let good = IndexProvisioning::for_facet(&FacetKey::new("acme", "bug", "severity"));
    assert!(good.is_non_blocking() && good.is_forward_only());
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

#[test]
fn is_promoted_distinguishes_the_decision_variants() {
    let p = IndexProvisioning::for_facet(&FacetKey::new("acme", "bug", "severity"));
    assert!(PromotionDecision::Promoted(p).is_promoted());
    assert!(!PromotionDecision::StayedOnGin { share: 0.0 }.is_promoted());
    assert!(!PromotionDecision::AlreadyPromoted.is_promoted());
}

#[test]
fn the_index_name_is_a_sanitised_identifier() {
    let facet = FacetKey::new("acme", "bug", "Customer-Tier!");
    let p = IndexProvisioning::for_facet(&facet);
    assert!(p.index_name.starts_with("issue_facet_customer_tier__"));
    assert!(
        p.index_name.len() <= 63,
        "PostgreSQL identifiers are bounded"
    );
    assert!(p
        .index_name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'));
    assert!(p.ddl.contains("Customer-Tier!"));
}

#[test]
fn index_names_are_stable_and_unique_across_the_whole_facet_scope() {
    let original =
        IndexProvisioning::for_facet(&FacetKey::new("acme", BUG_TYPE_ID, "Customer-Tier!"));
    let replay = IndexProvisioning::for_facet(&original.facet);
    let same_stem =
        IndexProvisioning::for_facet(&FacetKey::new("acme", BUG_TYPE_ID, "Customer@Tier!"));
    let other_tenant =
        IndexProvisioning::for_facet(&FacetKey::new("globex", BUG_TYPE_ID, "Customer-Tier!"));

    assert_eq!(original.index_name, replay.index_name);
    assert_ne!(original.index_name, same_stem.index_name);
    assert_ne!(original.index_name, other_tenant.index_name);
}

#[test]
fn every_dynamic_sql_value_is_quoted_as_data() {
    let p = IndexProvisioning::for_facet(&FacetKey::new(
        "tenant' OR TRUE --",
        "type' OR TRUE --",
        "owner'); DROP TABLE issue; --",
    ));
    assert!(p.ddl.contains("tenant_id = 'tenant'' OR TRUE --'"));
    assert!(p.ddl.contains("type_id::text = 'type'' OR TRUE --'"));
    assert!(p.ddl.contains("props ->> 'owner''); DROP TABLE issue; --'"));
}

#[test]
fn the_oq_c_floor_is_named() {
    assert!((PromotionThreshold::OQ_C_DEFAULT_TO_BEAT - 0.05).abs() < 1e-9);
    assert!(ProjectionFeederFloors::OQ_C_THRESHOLD.contains("5%"));
    assert_eq!(ProjectionFeederFloors::WINDOW_CALIBRATION, "ISS-P32");
}
