use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events;
use myelin_issues::replay::{IssueReindexSource, IssueReplayKind};
use myelin_issues::{
    IssueOlapConsumer, IssueOlapFeedSignal, ReindexCtx, RestrictionFlag, ISSUE_ANALYTICS_OLAP,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

fn ev(
    id: &str,
    type_token: &str,
    subject: &str,
    aggregate: &str,
    payload: serde_json::Value,
) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_token.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId(subject.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
        payload,
    }
}

#[test]
fn iss_d8b_olap_feed_drill_is_green() {
    let flag = RestrictionFlag::new();
    let live = IssueOlapConsumer::new(region(), flag.clone());

    live.handle(
        &ev(
            "a-sla",
            events::SLA_MET,
            "psn:alice",
            "issue:A",
            serde_json::json!({}),
        ),
        &mut myelin_events::HandlerTx::none(),
    );
    live.handle(
        &ev(
            "a-tr",
            events::ISSUE_TRANSITIONED,
            "psn:alice",
            "issue:A2",
            serde_json::json!({ "category": "completed" }),
        ),
        &mut myelin_events::HandlerTx::none(),
    );
    live.handle(
        &ev(
            "b-sla",
            events::SLA_BREACHED,
            "psn:bob",
            "issue:B",
            serde_json::json!({}),
        ),
        &mut myelin_events::HandlerTx::none(),
    );
    live.handle(
        &ev(
            "b-tr",
            events::ISSUE_TRANSITIONED,
            "psn:bob",
            "issue:B2",
            serde_json::json!({ "category": "started" }),
        ),
        &mut myelin_events::HandlerTx::none(),
    );

    flag.set("psn:alice", true);
    let leak = live.analytics(|a| {
        assert_eq!(a.velocity(), 2, "alice's rows excluded");
        a.leak_audit().restricted_subject_leak
    });
    assert_eq!(leak, 0, "the restriction flag excludes alice → 0 leak");

    let mut source = IssueReindexSource::new();
    source.upsert(
        IssueReplayKind::Issue,
        "issue:A",
        1,
        "psn:alice",
        serde_json::json!({ "olap_token": events::SLA_MET }),
    );
    source.upsert(
        IssueReplayKind::Issue,
        "issue:A2",
        1,
        "psn:alice",
        serde_json::json!({ "olap_token": events::ISSUE_TRANSITIONED, "category": "completed" }),
    );
    source.upsert(
        IssueReplayKind::Issue,
        "issue:B",
        1,
        "psn:bob",
        serde_json::json!({ "olap_token": events::SLA_BREACHED }),
    );
    source.upsert(
        IssueReplayKind::Issue,
        "issue:B2",
        1,
        "psn:bob",
        serde_json::json!({ "olap_token": events::ISSUE_TRANSITIONED, "category": "started" }),
    );
    let cold = IssueOlapConsumer::new(region(), flag.clone());
    cold.reindex_from(&source, &ReindexCtx::new(TenantId("acme".into()), region()));

    let reindex_matches_live = cold.projection_fingerprint() == live.projection_fingerprint();
    assert!(
        reindex_matches_live,
        "the cold rebuild byte-matches the live projection (0 drift)"
    );

    let signal = IssueOlapFeedSignal {
        store: ISSUE_ANALYTICS_OLAP,
        oltp_read_count: live.oltp_read_count(),
        restricted_subject_leak: leak,
        subjects_restricted: 1,
        reindex_matches_live,
    };
    assert!(
        signal.is_green(),
        "the ISS-D8b OLAP-feed gate is GREEN: {signal:?}"
    );
    assert_eq!(
        signal.oltp_read_count, 0,
        "0 OLTP reads from the analytics path"
    );
    assert_eq!(
        signal.restricted_subject_leak, 0,
        "0 restricted_subject_leak"
    );
}

#[test]
fn iss_d8b_drill_is_non_vacuous() {
    let live = IssueOlapConsumer::new(region(), RestrictionFlag::new());
    live.handle(
        &ev(
            "a1",
            events::SLA_MET,
            "psn:alice",
            "issue:A",
            serde_json::json!({}),
        ),
        &mut myelin_events::HandlerTx::none(),
    );
    let signal = IssueOlapFeedSignal {
        store: ISSUE_ANALYTICS_OLAP,
        oltp_read_count: live.oltp_read_count(),
        restricted_subject_leak: 0,
        subjects_restricted: 0,
        reindex_matches_live: true,
    };
    assert!(
        !signal.is_green(),
        "a vacuous run (0 subjects restricted) must read RED"
    );
}
