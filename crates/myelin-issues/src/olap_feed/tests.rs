use super::*;
use crate::replay::IssueReplayKind;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventId, EventType, Timestamp,
    Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn ev(
    event_id: &str,
    type_token: &str,
    actor_id: &str,
    subject: &str,
    aggregate: &str,
    payload: serde_json::Value,
) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(type_token.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: region(),
        actor: Actor(principal(actor_id)),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(event_id.into()),
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

fn consumer() -> IssueOlapConsumer {
    IssueOlapConsumer::new(region(), RestrictionFlag::new())
}

#[test]
fn zero_oltp_reads_from_the_analytics_path() {
    let c = consumer();
    assert_eq!(c.oltp_read_count(), 0, "0 OLTP reads before any feed");
    let outcome = c.handle(&ev(
        "e1",
        events::ISSUE_TRANSITIONED,
        "psn:alice",
        "myelin://acme/issue/issue/ENG-1",
        "issue:ENG-1",
        serde_json::json!({ "category": "completed" }),
    ), &mut myelin_events::HandlerTx::none());
    assert_eq!(outcome, HandleOutcome::Done);
    assert_eq!(
        c.oltp_read_count(),
        0,
        "still 0 OLTP reads after feeding the bus stream"
    );
    assert_eq!(
        c.doc_count(),
        1,
        "the bus event projected one analytics doc"
    );
}

#[test]
fn consumer_is_idempotent_on_event_id() {
    let c = consumer();
    let e = ev(
        "e1",
        events::SLA_MET,
        "psn:alice",
        "myelin://acme/issue/issue/ENG-1",
        "issue:ENG-1",
        serde_json::json!({}),
    );
    assert_eq!(c.handle(&e, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    assert_eq!(c.handle(&e, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done, "redelivery is a no-op");
    assert_eq!(c.doc_count(), 1, "exactly one projected doc");
}

#[test]
fn non_analytics_token_is_dropped() {
    let c = consumer();
    let e = ev(
        "e1",
        events::ISSUE_CREATED,
        "psn:alice",
        "myelin://acme/issue/issue/ENG-1",
        "issue:ENG-1",
        serde_json::json!({}),
    );
    assert_eq!(c.handle(&e, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done, "dropped, not an error");
    assert_eq!(c.doc_count(), 0, "a non-analytics token projects nothing");
    assert!(
        c.subjects().iter().all(|s| s.0 != "*"),
        "the OLAP consumer never subscribes to `*` (BUS-3)"
    );
    assert!(!c.subjects().is_empty(), "the whitelist is non-empty");
}

#[test]
fn out_of_region_event_is_non_retryable_poison() {
    let c = consumer();
    let mut e = ev(
        "e1",
        events::SLA_MET,
        "psn:alice",
        "myelin://acme/issue/issue/ENG-1",
        "issue:ENG-1",
        serde_json::json!({}),
    );
    e.region = Region("us-east".into());
    assert!(
        matches!(c.handle(&e, &mut myelin_events::HandlerTx::none()), HandleOutcome::NonRetryable(_)),
        "an out-of-region event is poison (the residency boundary)"
    );
    assert_eq!(
        c.doc_count(),
        0,
        "nothing projected from an out-of-region event"
    );
}

#[test]
fn restricted_subject_excluded_from_every_aggregate() {
    let flag = RestrictionFlag::new();
    let c = IssueOlapConsumer::new(region(), flag.clone());
    for (id, subj) in [("a1", "psn:alice"), ("a2", "psn:alice"), ("b1", "psn:bob")] {
        c.handle(&ev(
            id,
            events::ISSUE_TRANSITIONED,
            subj,
            subj,
            &format!("issue:{id}"),
            serde_json::json!({ "category": "completed" }),
        ), &mut myelin_events::HandlerTx::none());
    }
    c.analytics(|a| {
        assert_eq!(a.velocity(), 3, "all three contribute unrestricted");
        assert_eq!(a.cfd().len(), 3, "three CFD rows unrestricted");
        assert_eq!(
            a.leak_audit().restricted_subject_leak,
            0,
            "no restriction → 0 leak"
        );
    });
    flag.set("psn:alice", true);
    c.handle(&ev(
        "b1",
        events::ISSUE_TRANSITIONED,
        "psn:bob",
        "psn:bob",
        "issue:b1",
        serde_json::json!({ "category": "completed" }),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| {
        assert_eq!(a.velocity(), 1, "alice's two rows excluded → only bob");
        assert_eq!(a.cfd().len(), 1, "only bob's CFD row survives");
        assert_eq!(a.cycle_time_sample_size(), 1, "alice out of cycle-time");
        assert_eq!(
            a.leak_audit().restricted_subject_leak,
            0,
            "alice genuinely excluded → 0 leak"
        );
    });
}

#[test]
fn restriction_lifts_subject_reappears() {
    let flag = RestrictionFlag::new();
    let c = IssueOlapConsumer::new(region(), flag.clone());
    for (id, subj) in [("a1", "psn:alice"), ("b1", "psn:bob")] {
        c.handle(&ev(
            id,
            events::ISSUE_TRANSITIONED,
            subj,
            subj,
            &format!("issue:{id}"),
            serde_json::json!({ "category": "completed" }),
        ), &mut myelin_events::HandlerTx::none());
    }
    flag.set("psn:alice", true);
    c.handle(&ev(
        "a1",
        events::ISSUE_TRANSITIONED,
        "psn:alice",
        "psn:alice",
        "issue:a1",
        serde_json::json!({ "category": "completed" }),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| assert_eq!(a.velocity(), 1, "alice withheld"));
    flag.set("psn:alice", false);
    c.handle(&ev(
        "a1",
        events::ISSUE_TRANSITIONED,
        "psn:alice",
        "psn:alice",
        "issue:a1",
        serde_json::json!({ "category": "completed" }),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| {
        assert_eq!(
            a.velocity(),
            2,
            "alice reappears the instant restriction lifts"
        )
    });
}

#[test]
fn sla_compliance_is_met_over_met_plus_breached() {
    let c = consumer();
    c.handle(&ev(
        "m1",
        events::SLA_MET,
        "psn:a",
        "issue:1",
        "issue:1",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.handle(&ev(
        "m2",
        events::SLA_MET,
        "psn:b",
        "issue:2",
        "issue:2",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.handle(&ev(
        "x1",
        events::SLA_BREACHED,
        "psn:c",
        "issue:3",
        "issue:3",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| {
        assert_eq!(a.sla_sample_size(), 3, "three SLA outcomes contribute");
        let compliance = a.sla_compliance().expect("a compliance ratio");
        assert!(
            (compliance - 2.0 / 3.0).abs() < 1e-9,
            "compliance = 2 met / 3 total = 0.667, got {compliance}"
        );
    });
}

#[test]
fn sla_compliance_one_when_all_met_and_drops_on_breach() {
    let c = consumer();
    c.handle(&ev(
        "m1",
        events::SLA_MET,
        "psn:a",
        "issue:1",
        "issue:1",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| assert_eq!(a.sla_compliance(), Some(1.0), "all met → 1.0"));
    c.handle(&ev(
        "x1",
        events::SLA_BREACHED,
        "psn:b",
        "issue:2",
        "issue:2",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| {
        assert_eq!(
            a.sla_compliance(),
            Some(0.5),
            "one met + one breached → 0.5"
        );
    });
}

#[test]
fn sla_compliance_is_none_without_outcomes() {
    let c = consumer();
    c.handle(&ev(
        "t1",
        events::ISSUE_TRANSITIONED,
        "psn:a",
        "issue:1",
        "issue:1",
        serde_json::json!({ "category": "started" }),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| {
        assert_eq!(a.sla_sample_size(), 0, "no SLA outcomes");
        assert_eq!(
            a.sla_compliance(),
            None,
            "no compliance to report (no divide-by-zero)"
        );
    });
}

#[test]
fn restricted_subject_excluded_from_sla_compliance() {
    let flag = RestrictionFlag::new();
    let c = IssueOlapConsumer::new(region(), flag.clone());
    c.handle(&ev(
        "m1",
        events::SLA_MET,
        "psn:alice",
        "psn:alice",
        "issue:1",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.handle(&ev(
        "x1",
        events::SLA_BREACHED,
        "psn:bob",
        "psn:bob",
        "issue:2",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| {
        assert_eq!(
            a.sla_compliance(),
            Some(0.5),
            "unrestricted: 1 met / 2 = 0.5"
        );
    });
    flag.set("psn:alice", true);
    c.handle(&ev(
        "m1",
        events::SLA_MET,
        "psn:alice",
        "psn:alice",
        "issue:1",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| {
        assert_eq!(a.sla_sample_size(), 1, "only bob's SLA outcome contributes");
        assert_eq!(
            a.sla_compliance(),
            Some(0.0),
            "alice's met is withheld → 0/1 = 0.0"
        );
        assert_eq!(
            a.leak_audit().restricted_subject_leak,
            0,
            "alice excluded from the SLA leg too → 0 leak"
        );
    });
}

#[test]
fn reindex_from_source_byte_matches_live() {
    let live = consumer();
    live.handle(&ev(
        "t1",
        events::ISSUE_TRANSITIONED,
        "psn:a",
        "myelin://acme/issue/issue/ENG-1",
        "myelin://acme/issue/issue/ENG-1",
        serde_json::json!({ "category": "completed" }),
    ), &mut myelin_events::HandlerTx::none());
    live.handle(&ev(
        "m1",
        events::SLA_MET,
        "psn:b",
        "myelin://acme/issue/issue/ENG-2",
        "myelin://acme/issue/issue/ENG-2",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());

    let mut source = IssueReindexSource::new();
    source.upsert(
        IssueReplayKind::Issue,
        "myelin://acme/issue/issue/ENG-1",
        1,
        "myelin://acme/issue/issue/ENG-1",
        serde_json::json!({ "olap_token": events::ISSUE_TRANSITIONED, "category": "completed" }),
    );
    source.upsert(
        IssueReplayKind::Issue,
        "myelin://acme/issue/issue/ENG-2",
        1,
        "myelin://acme/issue/issue/ENG-2",
        serde_json::json!({ "olap_token": events::SLA_MET }),
    );

    let cold = consumer();
    let n = cold.reindex_from(&source, &ReindexCtx::new(TenantId("acme".into()), region()));
    assert_eq!(
        n, 2,
        "two analytics snapshots projected on the cold rebuild"
    );

    assert_eq!(
        cold.doc_count(),
        live.doc_count(),
        "cold rebuild has the same doc count as live"
    );
    assert_eq!(
        cold.projection_fingerprint(),
        live.projection_fingerprint(),
        "cold reindex byte-matches the live projection's read model (ISS-D8b 0-drift)"
    );
}

#[test]
fn reindex_is_idempotent() {
    let mut source = IssueReindexSource::new();
    source.upsert(
        IssueReplayKind::Issue,
        "issue:1",
        1,
        "issue:1",
        serde_json::json!({ "olap_token": events::ISSUE_TRANSITIONED, "category": "started" }),
    );
    let c = consumer();
    let ctx = ReindexCtx::new(TenantId("acme".into()), region());
    c.reindex_from(&source, &ctx);
    let first = c.parity_bytes();
    c.reindex_from(&source, &ctx);
    assert_eq!(
        c.parity_bytes(),
        first,
        "a re-run rebuilds identically (idempotent)"
    );
}

#[test]
fn erased_aggregate_skipped_on_reindex() {
    let mut source = IssueReindexSource::new();
    source.upsert(
        IssueReplayKind::Issue,
        "issue:1",
        1,
        "issue:1",
        serde_json::json!({ "olap_token": events::ISSUE_TRANSITIONED, "category": "completed" }),
    );
    source.upsert(
        IssueReplayKind::Issue,
        "issue:2",
        1,
        "issue:2",
        serde_json::json!({ "olap_token": events::ISSUE_TRANSITIONED, "category": "completed" }),
    );
    assert!(source.erase("issue:1"), "issue:1 erased");
    let c = consumer();
    let n = c.reindex_from(&source, &ReindexCtx::new(TenantId("acme".into()), region()));
    assert_eq!(n, 1, "only the un-erased aggregate is re-projected");
    assert_eq!(
        c.doc_count(),
        1,
        "the erased aggregate stays out of analytics"
    );
}

#[test]
fn olap_feed_signal_is_green() {
    let flag = RestrictionFlag::new();
    let live = IssueOlapConsumer::new(region(), flag.clone());
    live.handle(&ev(
        "a1",
        events::SLA_MET,
        "psn:alice",
        "psn:alice",
        "issue:1",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    live.handle(&ev(
        "b1",
        events::SLA_MET,
        "psn:bob",
        "psn:bob",
        "issue:2",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());
    flag.set("psn:alice", true);
    live.handle(&ev(
        "a1",
        events::SLA_MET,
        "psn:alice",
        "psn:alice",
        "issue:1",
        serde_json::json!({}),
    ), &mut myelin_events::HandlerTx::none());

    let leak = live.analytics(|a| a.leak_audit().restricted_subject_leak);

    let mut source = IssueReindexSource::new();
    source.upsert(
        IssueReplayKind::Issue,
        "issue:1",
        1,
        "psn:alice",
        serde_json::json!({ "olap_token": events::SLA_MET }),
    );
    source.upsert(
        IssueReplayKind::Issue,
        "issue:2",
        1,
        "psn:bob",
        serde_json::json!({ "olap_token": events::SLA_MET }),
    );
    let cold = consumer();
    cold.reindex_from(&source, &ReindexCtx::new(TenantId("acme".into()), region()));
    let reindex_matches_live = cold.projection_fingerprint() == live.projection_fingerprint();

    let signal = IssueOlapFeedSignal {
        store: ISSUE_ANALYTICS_OLAP,
        oltp_read_count: live.oltp_read_count(),
        restricted_subject_leak: leak,
        subjects_restricted: 1,
        reindex_matches_live,
    };
    assert!(
        signal.is_green(),
        "the ISS-D8b OLAP-feed gate is green: {signal:?}"
    );
    assert_eq!(signal.oltp_read_count, 0, "the 0-OLTP-read headline zero");
    assert_eq!(
        signal.restricted_subject_leak, 0,
        "the restriction-exclusion headline zero"
    );
}

#[test]
fn olap_feed_signal_reads_red_when_any_invariant_fails() {
    let green = IssueOlapFeedSignal {
        store: ISSUE_ANALYTICS_OLAP,
        oltp_read_count: 0,
        restricted_subject_leak: 0,
        subjects_restricted: 1,
        reindex_matches_live: true,
    };
    assert!(green.is_green(), "the all-green baseline is green");
    assert!(
        !IssueOlapFeedSignal {
            oltp_read_count: 1,
            ..green.clone()
        }
        .is_green(),
        "an OLTP read reads RED"
    );
    assert!(
        !IssueOlapFeedSignal {
            restricted_subject_leak: 1,
            ..green.clone()
        }
        .is_green(),
        "a restriction leak reads RED"
    );
    assert!(
        !IssueOlapFeedSignal {
            reindex_matches_live: false,
            ..green.clone()
        }
        .is_green(),
        "a cold≠live divergence reads RED"
    );
    assert!(
        !IssueOlapFeedSignal {
            subjects_restricted: 0,
            ..green.clone()
        }
        .is_green(),
        "a vacuous run reads RED"
    );
}

#[test]
fn issue_analytics_adds_sla_compliance_aggregate() {
    let names = issue_analytics_aggregate_names();
    assert!(names.contains(&"cfd"));
    assert!(names.contains(&"cycle_time"));
    assert!(names.contains(&"velocity"));
    assert!(names.contains(&"delivery_health"));
    assert!(
        names.contains(&"sla_compliance"),
        "the Issues ask adds SLA-compliance"
    );
    assert_eq!(
        names.len(),
        5,
        "four cross-team aggregates + SLA-compliance"
    );
}

#[test]
fn floors_are_named() {
    assert!(IssueOlapFeedFloors::MONTE_CARLO_FORECAST.contains("ISS-P32"));
    assert!(IssueOlapFeedFloors::MONTE_CARLO_FORECAST.contains("Monte-Carlo"));
    assert!(IssueOlapFeedFloors::COLUMNAR_BACKEND.contains("OlapReadStore"));
    assert!(IssueOlapFeedFloors::WORKLOG_ELIGIBILITY.contains("OQ-H"));
}
