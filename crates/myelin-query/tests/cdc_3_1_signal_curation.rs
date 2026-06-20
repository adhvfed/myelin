//! # The CDC pair for contract 3.1 — Signal curation (`define_signal_rule`) (EB-18 / P-138)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 3.1
//! (`define_signal_rule(SignalRule{matcher, severity, dedup_key_tpl, dedup_window})` — the
//! curated / deduped / severity-ranked subset published to `sig.<tenant>.<severity>.<rule>`;
//! consumers subscribe to Signals, never `evt.*`). Owning architecture:
//! `event-bus.md` §4.4 (the Signal engine — match / severity-rank / dedup-window collapse /
//! auto-resolve / publish). ADR-19.
//!
//! ## The seam this pair pins
//! Row 3.1 is the seam between:
//! - the **PROVIDER** — the Signal engine ([`myelin_query::SignalEngine`], an infra consumer on
//!   the raw `evt.*` firehose): it ingests events, curates them (match / rank / dedup-collapse /
//!   auto-resolve), and PUBLISHES `sig.<tenant>.<severity>.<rule>` drafts. Its promise: N
//!   identical events within the window collapse to ONE Signal `count=N`; a resolving event
//!   resolves the matching Signal; the publish subject is exactly `sig.<tenant>.<sev>.<rule>`.
//! - the **CONSUMER** — Notif / agents / reactive consumers: they subscribe to the curated
//!   `sig.*` subject and read `(severity, count, state)`. Their promise: they react to the
//!   curated Signal, never the raw firehose (the upstream defence, BUS-4).
//!
//! The pair asserts both sides agree on the curated shape: the subject grammar, the collapse
//! counter, the severity rank, and the resolve transition.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{
    Literal, ObjectType, Principal, PrincipalId, PrincipalKind, SetExpr,
};
use myelin_query::{
    define_signal_rule, CmpOp, DedupKeyTpl, DedupWindow, EventMatcher, Expr, Predicate, PublishKind,
    RuleId, Severity, SignalEngine, SignalState,
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

/// The "see everything" oracle — the relational arm is not exercised here (visibility is
/// supplied as `SetExpr::All`).
fn see_all(_m: &myelin_query::RelMembership) -> bool {
    false
}

/// **PROVIDER side of 3.1** — a `define_signal_rule` registration + the Signal engine that
/// curates. This models the emit side: an admin registers the rule, the engine ingests the
/// firehose and publishes curated `sig.*` drafts.
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

/// The 3.1 pair: a PROVIDER curates N identical failures into ONE `sig.acme.error.ci_run_failed`
/// Signal with `count=N`, and the CONSUMER reads exactly that curated subject + count (never the
/// raw `evt.*`).
#[test]
fn cdc_3_1_provider_curates_dedup_collapse_consumer_reads_count() {
    let mut engine = provider_engine();

    // The PROVIDER ingests 5 identical failures of the same run.
    let mut last_subject = String::new();
    let mut last_count = 0u64;
    for i in 0..5 {
        let env = envelope_at("ci.run.failed", "42", &format!("2026-06-20T00:00:0{i}Z"));
        let drafts = engine.ingest(&env, &SetExpr::All, &see_all);
        assert_eq!(drafts.len(), 1, "one rule → one curated draft per matching event");
        // The CONSUMER side reads the curated subject + count off the draft.
        last_subject = drafts[0].subject.clone();
        last_count = drafts[0].signal.count;
    }

    // CONSUMER promise: it subscribes to the curated `sig.<tenant>.<severity>.<rule>`, NOT evt.*.
    assert_eq!(
        last_subject, "sig.acme.error.ci_run_failed",
        "the publish subject is the frozen sig.<tenant>.<severity>.<rule>"
    );
    // And the dedup-window collapse gave it ONE Signal with count=N (the storm-control unit).
    assert_eq!(last_count, 5, "N=5 identical failures collapse to one Signal count=5");
}

/// The 3.1 pair, RESOLVE leg: a PROVIDER auto-resolves the curated Signal on the resolving
/// event, and the CONSUMER reads the `Resolved` transition off the curated subject.
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
    // CONSUMER reads the resolve transition + the same curated subject.
    assert_eq!(resolved[0].kind, PublishKind::Resolved);
    assert_eq!(resolved[0].subject, "sig.acme.error.ci_run_failed");
    assert_eq!(resolved[0].signal.state, SignalState::Resolved);
}

/// The 3.1 pair, SEVERITY-RANK leg: the curated subject's `<severity>` token reflects the
/// rule's rank, and the rank order `info<notice<warning<error<critical` is what a CONSUMER
/// filters on (`sig.acme.error.>` ⊂ the `>= error` band).
#[test]
fn cdc_3_1_severity_rank_in_subject_and_ordering() {
    // A `critical` rule publishes to the critical band; a CONSUMER subscribing `>= error`
    // (error|critical) sees it.
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
