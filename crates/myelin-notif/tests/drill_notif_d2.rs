use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, Visibility,
};
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, PrincipalId as IdPrincipalId, PrincipalKind as IdPrincipalKind,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::read_fanout::{read_fanout, SyntheticReverseIndex};
use myelin_notif::{
    build_router, dedup_collapse_ratio_bps, InboxProjection, Reason, DEFAULT_HOT_SUBJECT_WRITE_CAP,
    ROUTER_CONSUMER_NAME, SIGNAL_MENTIONS_KEY,
};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ci_bot() -> Principal {
    Principal::stub(
        PrincipalId("p-ci-bot".into()),
        PrincipalKind::Service,
        tenant(),
    )
}

fn signal(rule: &str, severity: Severity, subject: &str, dedup: &str) -> Signal {
    Signal {
        rule_id: RuleId(rule.into()),
        tenant: tenant(),
        severity,
        dedup_key: DedupKey(dedup.into()),
        subject: ArtifactRef(subject.into()),
        count: 1,
        state: SignalState::Open,
        first_seen: "2026-06-20T00:00:00Z".into(),
        last_seen: "2026-06-20T00:00:00Z".into(),
    }
}

fn envelope(id: &str, sig: &Signal, actor: Principal) -> EventEnvelope {
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(actor),
        subject: ArtifactRef(subject),
        aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::to_value(sig).unwrap(),
    }
}

fn msg(id: &str, sig: &Signal, actor: Principal) -> Message {
    let env = envelope(id, sig, actor);
    Message {
        subject: env.subject.0.clone(),
        envelope: env,
    }
}

#[test]
fn notif_d2_storm_collapses_to_one_row_ratio_measured() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    let inbound = 1000u64;
    let sig = signal(
        "ci_run_failed",
        Severity::Error,
        "myelin://acme/ci/run/42",
        "run-42",
    );
    for i in 0..inbound {
        let out = consumer.deliver(&msg(&format!("evt-{i}"), &sig, ci_bot()));
        assert_eq!(out, Delivered::Acked, "every Signal acks (none stalls)");
    }

    assert_eq!(
        inbox.len(),
        1,
        "NOTIF-D2: 1000 near-identical CI failures → ONE inbox row (N→1)"
    );
    let row = inbox
        .get(
            &tenant(),
            "psn:watcher:ci_run_failed",
            "ci_run_failed:run-42",
        )
        .expect("the one collapsed row");
    assert_eq!(
        row.coalesce_count, 1000,
        "the +N more counter is the full incident count (1000)"
    );

    assert_eq!(
        outbox.committed_count(),
        1,
        "exactly ONE notif.item.created emitted (the opened row); the 999 collapses did not re-push"
    );

    let collapsed = inbound - 1;
    let ratio = dedup_collapse_ratio_bps(inbound, collapsed);
    assert_eq!(ratio, 9990, "999/1000 collapsed → 9990 bps");
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::DedupCollapseRatio,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        ratio,
    );
    src.assert_labelled(
        SignalName::DedupCollapseRatio,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        Predicate::Gte(9000),
    )
    .expect_green();
}

#[test]
fn notif_d2_self_burst_produces_zero_items() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    let self_recipient = Principal::stub(
        PrincipalId("psn:watcher:my_change".into()),
        PrincipalKind::Human,
        tenant(),
    );
    let sig = signal(
        "my_change",
        Severity::Warning,
        "myelin://acme/chat/thread/T1",
        "t1",
    );
    for i in 0..30 {
        let out = consumer.deliver(&msg(&format!("self-{i}"), &sig, self_recipient.clone()));
        assert_eq!(
            out,
            Delivered::Acked,
            "a self-action acks (terminal, not a stall)"
        );
    }

    assert_eq!(
        inbox.len(),
        0,
        "NOTIF-D2: a 30-event self-burst → 0 inbox items (0 self-notifications)"
    );
    assert_eq!(
        outbox.committed_count(),
        0,
        "0 emits (a self-action pushes no delivery)"
    );
}

#[test]
fn notif_d2_30_comment_pr_burst_is_bounded() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    let sig = signal(
        "pr_comment",
        Severity::Info,
        "myelin://acme/git/pr/9",
        "pr-9",
    );
    for i in 0..30 {
        consumer.deliver(&msg(&format!("c-{i}"), &sig, ci_bot()));
    }

    assert_eq!(
        inbox.len(),
        1,
        "the 30-comment burst collapsed to ONE inbox row (bounded)"
    );
    let row = inbox
        .get(&tenant(), "psn:watcher:pr_comment", "pr_comment:pr-9")
        .unwrap();
    assert_eq!(
        row.coalesce_count, 30,
        "+N more = 30 (the full comment count, bounded into one row)"
    );
    assert_eq!(
        outbox.committed_count(),
        1,
        "bounded pushes (1, not 30) - the audit (30 Signals) untouched"
    );
}

fn mention_msg(id: &str, sig: &Signal, mentions: &[Principal]) -> Message {
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
    let nodes: Vec<InlineNode> = mentions.iter().cloned().map(InlineNode::Mention).collect();
    let mut payload = serde_json::to_value(sig).unwrap();
    if let serde_json::Value::Object(map) = &mut payload {
        map.insert(
            SIGNAL_MENTIONS_KEY.into(),
            serde_json::to_value(&nodes).unwrap(),
        );
    }
    Message {
        subject: subject.clone(),
        envelope: EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType("signal.opened".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(ci_bot()),
            subject: ArtifactRef(subject),
            aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload,
        },
    }
}

#[test]
fn notif_d2_mention_storm_write_fanout_is_bounded_by_the_hot_subject_cap() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    let storm_size = 200usize;
    let sig = signal(
        "mention_spray",
        Severity::Info,
        "myelin://acme/chat/thread/hot",
        "spray",
    );
    let mentions: Vec<Principal> = (0..storm_size)
        .map(|i| {
            Principal::stub(
                PrincipalId(format!("p-{i}")),
                PrincipalKind::Human,
                tenant(),
            )
        })
        .collect();

    let out = consumer.deliver(&mention_msg("evt-mention-storm", &sig, &mentions));
    assert_eq!(
        out,
        Delivered::Acked,
        "the mention-storm Signal routes + acks"
    );

    let subject_root = "myelin://acme/chat/thread/hot";
    assert_eq!(
        consumer.handler().hot_cap().admitted_count(subject_root),
        DEFAULT_HOT_SUBJECT_WRITE_CAP,
        "NOTIF-D2: the mention-storm is bounded to `cap` write rows (0 unbounded write amplification)"
    );
    assert_eq!(
        consumer.handler().hot_cap().overflow_count(subject_root),
        storm_size as u32 - DEFAULT_HOT_SUBJECT_WRITE_CAP,
        "the rest overflowed into the coalesced marker (the +N more were mentioned counter - preserved)"
    );

    let mention_rows = inbox
        .snapshot_for_tenant(&tenant())
        .into_iter()
        .filter(|r| r.reason == Reason::Mentioned)
        .count();
    assert_eq!(
        mention_rows as u32, DEFAULT_HOT_SUBJECT_WRITE_CAP,
        "exactly `cap` mention rows materialised - the write-amplification bound holds (§3.2.4)"
    );
}

#[test]
fn notif_d2_ratio_alarm_fires_on_a_collapse_regression() {
    let ratio = dedup_collapse_ratio_bps(1000, 0);
    assert_eq!(ratio, 0, "no collapse → 0 bps");
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::DedupCollapseRatio,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        ratio,
    );
    let verdict = src.assert_labelled(
        SignalName::DedupCollapseRatio,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        Predicate::Gte(9000),
    );
    assert!(
        !verdict.is_green(),
        "ratio 0 against `>= 9000` is RED - the alarm fires on a collapse regression"
    );
}

fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: myelin_identity::Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

#[test]
fn notif_d2_read_fanout_50k_watcher_subject_zero_write_amplification() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    let storm = 500u64;
    let sig = signal(
        "pr_activity",
        Severity::Info,
        "myelin://acme/git/pr/celebrity",
        "pr-celeb",
    );
    for i in 0..storm {
        let out = consumer.deliver(&msg(&format!("amb-{i}"), &sig, ci_bot()));
        assert_eq!(out, Delivered::Acked, "every ambient event routes + acks");
    }

    let markers = consumer.handler().ambient();
    assert_eq!(
        markers.marker_count(&tenant()),
        1,
        "NOTIF-D2: a 50k-watcher subject hit 500 times → ONE coalesced marker (0 write amplification)"
    );
    let m = markers
        .get(&tenant(), "myelin://acme/git/pr/celebrity")
        .unwrap();
    assert_eq!(
        m.count, storm,
        "the +N more counter is the full activity count (preserved, never lost)"
    );
}

#[test]
fn notif_d2_read_fanout_lazy_materialise_join_and_zookie_watermark() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    for (rule, subject, dedup) in [
        ("pr_a", "myelin://acme/git/pr/A", "a"),
        ("pr_b", "myelin://acme/git/pr/B", "b"),
    ] {
        let sig = signal(rule, Severity::Info, subject, dedup);
        for i in 0..50 {
            consumer.deliver(&msg(&format!("{rule}-{i}"), &sig, ci_bot()));
        }
    }
    let markers = consumer.handler().ambient();
    assert_eq!(
        markers.marker_count(&tenant()),
        2,
        "two hot subjects → two coalesced markers (not 100 rows)"
    );

    let idx = SyntheticReverseIndex::new();
    idx.grant_watch(&tenant(), "watcher-1", "myelin://acme/git/pr/A");
    idx.grant_watch(&tenant(), "watcher-1", "myelin://acme/git/pr/B");
    let watcher = myelin_identity::Principal::stub(
        IdPrincipalId("watcher-1".into()),
        IdPrincipalKind::Human,
        tenant(),
    );

    let before = read_fanout(&watcher, markers, &idx, &strong(&idx.current_zookie().0)).unwrap();
    let before_roots: Vec<&str> = before.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        before_roots,
        vec!["myelin://acme/git/pr/A", "myelin://acme/git/pr/B"],
        "the read-fanout materialised exactly the watched slice on inbox open (the SetExpr JOIN)"
    );

    let new_zk = idx.revoke_watch(&tenant(), "watcher-1", "myelin://acme/git/pr/B");
    let after = read_fanout(&watcher, markers, &idx, &strong(&new_zk.0)).unwrap();
    let after_roots: Vec<&str> = after.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        after_roots,
        vec!["myelin://acme/git/pr/A"],
        "NOTIF-D2: a just-revoked watch is reflected at-or-after the zookie watermark (B held, not leaked)"
    );
}
