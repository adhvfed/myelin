use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Literal, ObjectType, Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_query::{
    define_signal_rule, CmpOp, DedupKeyTpl, DedupWindow, EventMatcher, Expr, Predicate,
    PublishKind, RuleId, Severity, SignalEngine, SignalState,
};
use myelin_tenancy::{Region, TenantId};

fn type_matcher(type_: &str) -> EventMatcher {
    EventMatcher::compile(
        ObjectType("run".into()),
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("event.type".into()),
            rhs: Expr::Lit(Literal::Str(type_.into())),
        },
    )
    .unwrap()
}

fn envelope_at(type_: &str, id: &str, recorded_at: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("evt-{id}-{recorded_at}")),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef(format!("myelin://acme/ci/run/{id}")),
        aggregate: AggregateKey(format!("ci:{id}")),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp(recorded_at.into()),
        recorded_at: Timestamp(recorded_at.into()),
        payload: serde_json::json!({}),
    }
}

fn see_all(_m: &myelin_query::RelMembership) -> bool {
    false
}

fn provider_engine() -> SignalEngine {
    let mut engine = SignalEngine::new();
    engine.add_rule(define_signal_rule(
        RuleId("ci_run_failed".into()),
        type_matcher("ci.run.failed"),
        Severity::Error,
        DedupKeyTpl("ci.run.failed:{event.subject}".into()),
        DedupWindow { seconds: 0 },
        Some(type_matcher("ci.run.passed")),
    ));
    engine
}

#[test]
fn cdc_3_1_provider_curates_dedup_collapse_consumer_reads_count() {
    let mut engine = provider_engine();

    let mut last_subject = String::new();
    let mut last_count = 0u64;
    for i in 0..5 {
        let env = envelope_at("ci.run.failed", "42", &format!("2026-06-20T00:00:0{i}Z"));
        let drafts = engine.ingest(&env, &SetExpr::All, &see_all);
        assert_eq!(
            drafts.len(),
            1,
            "one rule → one curated draft per matching event"
        );
        last_subject = drafts[0].subject.clone();
        last_count = drafts[0].signal.count;
    }

    assert_eq!(
        last_subject, "sig.acme.error.ci_run_failed",
        "the publish subject is the frozen sig.<tenant>.<severity>.<rule>"
    );
    assert_eq!(
        last_count, 5,
        "N=5 identical failures collapse to one Signal count=5"
    );
}

#[test]
fn cdc_3_1_provider_auto_resolves_consumer_reads_resolved() {
    let mut engine = provider_engine();
    engine.ingest(
        &envelope_at("ci.run.failed", "42", "2026-06-20T00:00:00Z"),
        &SetExpr::All,
        &see_all,
    );
    let resolved = engine.ingest(
        &envelope_at("ci.run.passed", "42", "2026-06-20T00:05:00Z"),
        &SetExpr::All,
        &see_all,
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].kind, PublishKind::Resolved);
    assert_eq!(resolved[0].subject, "sig.acme.error.ci_run_failed");
    assert_eq!(resolved[0].signal.state, SignalState::Resolved);
}

#[test]
fn cdc_3_1_severity_rank_in_subject_and_ordering() {
    let mut engine = SignalEngine::new();
    engine.add_rule(define_signal_rule(
        RuleId("disk_full".into()),
        type_matcher("ci.run.failed"),
        Severity::Critical,
        DedupKeyTpl("disk_full:{event.subject}".into()),
        DedupWindow { seconds: 0 },
        None,
    ));
    let drafts = engine.ingest(
        &envelope_at("ci.run.failed", "9", "2026-06-20T00:00:00Z"),
        &SetExpr::All,
        &see_all,
    );
    assert_eq!(drafts[0].subject, "sig.acme.critical.disk_full");
    assert!(
        drafts[0].signal.severity >= Severity::Error,
        "critical ranks at/above error (the consumer's >= filter band)"
    );
}
