use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, HandleOutcome, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events;
use myelin_issues::replay::{IssueReindexSource, IssueReplayKind};
use myelin_issues::{IssueOlapConsumer, ReindexCtx, RestrictionFlag, ISSUE_ANALYTICS_OLAP};
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
fn restrict_drops_from_analytics_then_replay_is_drift_free() {
    let flag = RestrictionFlag::new();
    let c = IssueOlapConsumer::new(region(), flag.clone());

    c.handle(&ev(
        "a-sla",
        events::SLA_MET,
        "psn:alice",
        "issue:A",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.handle(&ev(
        "a-tr",
        events::ISSUE_TRANSITIONED,
        "psn:alice",
        "issue:A2",
        serde_json::json!({ "category": "completed" }),
    ), &mut myelin_events::HandlerTx::none());
    c.handle(&ev(
        "b-sla",
        events::SLA_MET,
        "psn:bob",
        "issue:B",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.handle(&ev(
        "b-tr",
        events::ISSUE_TRANSITIONED,
        "psn:bob",
        "issue:B2",
        serde_json::json!({ "category": "completed" }),
    ), &mut myelin_events::HandlerTx::none());

    assert_eq!(
        c.oltp_read_count(),
        0,
        "the analytics path reads 0 from the OLTP table"
    );

    c.analytics(|a| {
        assert_eq!(a.velocity(), 4, "all four rows contribute unrestricted");
        assert_eq!(a.cfd().len(), 4, "four CFD rows unrestricted");
        assert_eq!(a.sla_sample_size(), 2, "two SLA outcomes unrestricted");
        assert_eq!(a.sla_compliance(), Some(1.0), "all met → 1.0 compliance");
        assert_eq!(
            a.leak_audit().restricted_subject_leak,
            0,
            "no restriction → 0 leak"
        );
    });

    flag.set("psn:alice", true);
    c.analytics(|a| {
        assert_eq!(a.velocity(), 2, "alice's rows excluded from velocity");
        assert_eq!(a.cfd().len(), 2, "alice's CFD rows excluded");
        assert_eq!(a.sla_sample_size(), 1, "alice out of the SLA sample");
        assert_eq!(
            a.sla_compliance(),
            Some(1.0),
            "bob's met SLA → 1.0 over the contributing rows"
        );
        assert_eq!(
            a.leak_audit().restricted_subject_leak,
            0,
            "alice genuinely excluded from analytics → 0 restricted_subject_leak"
        );
    });

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
        serde_json::json!({ "olap_token": events::SLA_MET }),
    );
    source.upsert(
        IssueReplayKind::Issue,
        "issue:B2",
        1,
        "psn:bob",
        serde_json::json!({ "olap_token": events::ISSUE_TRANSITIONED, "category": "completed" }),
    );

    let cold = IssueOlapConsumer::new(region(), flag.clone());
    let n = cold.reindex_from(&source, &ReindexCtx::new(TenantId("acme".into()), region()));
    assert_eq!(
        n, 4,
        "four analytics snapshots projected on the cold rebuild"
    );

    assert_eq!(
        cold.projection_fingerprint(),
        c.projection_fingerprint(),
        "the cold reindex byte-matches the live projection's read model (0 drift)"
    );

    cold.analytics(|a| {
        assert_eq!(
            a.velocity(),
            2,
            "alice still excluded after the rebuild (the flag drives it live)"
        );
        assert_eq!(
            a.leak_audit().restricted_subject_leak,
            0,
            "still 0 leak after the rebuild"
        );
    });

    flag.set("psn:alice", false);
    cold.analytics(|a| {
        assert_eq!(
            a.velocity(),
            4,
            "alice reappears the instant restriction lifts (the rows stayed)"
        );
    });
}

#[test]
fn the_feed_addresses_the_one_shared_olap_warehouse() {
    assert_eq!(ISSUE_ANALYTICS_OLAP, "issue_analytics_olap");
}

#[test]
fn a_non_analytics_token_is_dropped_end_to_end() {
    let c = IssueOlapConsumer::new(region(), RestrictionFlag::new());
    let outcome = c.handle(&ev(
        "c1",
        events::ISSUE_CREATED,
        "psn:alice",
        "issue:A",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    assert_eq!(outcome, HandleOutcome::Done);
    assert_eq!(c.doc_count(), 0, "a non-analytics token projects nothing");
}
