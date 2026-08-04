use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, Visibility,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::list_inbox::{list_inbox_ranked, AllowAllAuthorize, InboxFilter, Page};
use myelin_notif::ranking::DeterministicV1;
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::{build_router, Class, Reason};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn me() -> Principal {
    Principal::stub(PrincipalId("u-week".into()), PrincipalKind::Human, tenant())
}
fn strong() -> Consistency {
    Consistency {
        at_least: Zookie("zk-notif-d1".into()),
        mode: ConsistencyMode::Strong,
    }
}

fn item(item_id: &str, subject: &str, reason: Reason) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: region(),
        item_id: item_id.into(),
        recipient: "u-week".into(),
        subject: ArtifactRef(subject.into()),
        reason,
        class: myelin_notif::ranking::class_for(reason),
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
        dedup_key: item_id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

fn mixed_week() -> InboxProjection {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item("a-fyi-1", "myelin://acme/issue/issue/F1", Reason::Fyi));
    inbox.upsert_for_test(item("a-fyi-2", "myelin://acme/chat/thread/F2", Reason::Fyi));
    inbox.upsert_for_test(item("a-fyi-3", "myelin://acme/git/pr/F3", Reason::Fyi));
    inbox.upsert_for_test(item("a-fyi-4", "myelin://acme/issue/issue/F4", Reason::Fyi));
    inbox.upsert_for_test(item("a-fyi-5", "myelin://acme/ci/run/F5", Reason::Fyi));
    inbox.upsert_for_test(item(
        "b-watched",
        "myelin://acme/git/pr/W1",
        Reason::Watched,
    ));
    inbox.upsert_for_test(item(
        "b-state",
        "myelin://acme/issue/issue/W2",
        Reason::StateChanged,
    ));
    inbox.upsert_for_test(item(
        "c-replied",
        "myelin://acme/chat/thread/P1",
        Reason::Replied,
    ));
    inbox.upsert_for_test(item(
        "c-agent",
        "myelin://acme/issue/issue/P2",
        Reason::AgentProposal,
    ));
    inbox.upsert_for_test(item(
        "d-review",
        "myelin://acme/git/pr/D1",
        Reason::ReviewRequested,
    ));
    inbox.upsert_for_test(item(
        "d-assigned",
        "myelin://acme/issue/issue/D2",
        Reason::Assigned,
    ));
    inbox.upsert_for_test(item(
        "d-mention",
        "myelin://acme/chat/thread/D3",
        Reason::Mentioned,
    ));
    inbox.upsert_for_test(item(
        "e-approval",
        "myelin://acme/issue/issue/C1",
        Reason::ApprovalRequested,
    ));
    inbox.upsert_for_test(item(
        "e-escalated",
        "myelin://acme/ci/run/C2",
        Reason::Escalated,
    ));
    inbox.upsert_for_test(item("e-sla", "myelin://acme/issue/issue/C3", Reason::Sla));
    inbox
}

#[test]
fn notif_d1_important_never_buried_with_explain_trace_per_rank() {
    let inbox = mixed_week();
    let ranker = DeterministicV1::default();
    let page = list_inbox_ranked(
        &inbox,
        &me(),
        &InboxFilter::all(),
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
        &ranker,
    );
    assert_eq!(
        page.items.len(),
        15,
        "the whole mixed week is read back (the ONE inbox)"
    );

    let with_trace = page
        .items
        .iter()
        .filter(|r| !r.trace.render().is_empty() && r.trace.final_priority == r.priority)
        .count();
    let trace_coverage = with_trace as f64 / page.items.len() as f64;
    assert_eq!(
        trace_coverage, 1.0,
        "100% of ranks carry a deterministic explain-trace (NOTIF-2)"
    );

    let classes: Vec<Class> = page.items.iter().map(|r| r.trace.class).collect();
    let first_fyi = classes.iter().position(|c| *c == Class::Fyi);
    let last_important = classes
        .iter()
        .rposition(|c| matches!(c, Class::Critical | Class::Direct));
    let mut buried = 0usize;
    let mut important_total = 0usize;
    for (idx, class) in classes.iter().enumerate() {
        if matches!(class, Class::Critical | Class::Direct) {
            important_total += 1;
            if let Some(ff) = first_fyi {
                if ff < idx {
                    buried += 1;
                }
            }
        }
    }
    let important_buried_rate = buried as f64 / important_total as f64;
    assert_eq!(
        important_buried_rate, 0.0,
        "NOTIF-D1: important-buried-rate = 0 (no critical/direct ranks below any fyi) - never weakened"
    );
    if let (Some(li), Some(ff)) = (last_important, first_fyi) {
        assert!(
            li < ff,
            "every critical/direct ranks above every fyi (the band invariant)"
        );
    }

    let first_important_pos = classes
        .iter()
        .position(|c| matches!(c, Class::Critical | Class::Direct))
        .expect("the week has critical/direct items");
    let higher_ahead = page.items[..first_important_pos]
        .iter()
        .filter(|r| r.priority > page.items[first_important_pos].priority)
        .count();
    assert_eq!(
        higher_ahead, 0,
        "first-important latency in budget: nothing outranks the first critical"
    );
    assert_eq!(
        first_important_pos, 0,
        "the first important item is at the TOP of the ranked inbox"
    );
    assert_eq!(
        page.items[0].trace.class,
        Class::Critical,
        "the very first ranked item is a critical (the must-see-first)"
    );

    let order: Vec<&str> = page.items.iter().map(|r| r.item.item_id.as_str()).collect();
    assert_eq!(
        &order[..6],
        &[
            "e-approval",
            "e-escalated",
            "e-sla",
            "d-assigned",
            "d-mention",
            "d-review"
        ],
        "the criticals (90) then the directs (70), each in stable item_id order"
    );
    assert!(
        order[order.len() - 5..]
            .iter()
            .all(|id| id.starts_with("a-fyi-")),
        "the five fyis (15) sink to the bottom of the ranked inbox"
    );
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

fn signal_msg(id: &str, sig: &Signal) -> Message {
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
    let env = EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(me()),
        subject: ArtifactRef(subject.clone()),
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
    };
    Message {
        subject,
        envelope: env,
    }
}

#[test]
fn notif_d1_ranking_reads_the_routed_inbox_with_trace_per_rank() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    for (i, sig) in [
        signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/1",
            "run-1",
        ),
        signal(
            "ci_run_failed",
            Severity::Warning,
            "myelin://acme/git/pr/9",
            "pr-9",
        ),
        signal(
            "ci_run_failed",
            Severity::Critical,
            "myelin://acme/issue/issue/PROJ-1",
            "sla-1",
        ),
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            consumer.deliver(&signal_msg(&format!("evt-{i}"), sig)),
            Delivered::Acked
        );
    }
    assert_eq!(
        inbox.len(),
        3,
        "the router UPSERTed three rows into the ONE projection"
    );

    let recipient = inbox.snapshot_for_tenant(&tenant())[0].recipient.clone();
    let viewer = Principal::stub(PrincipalId(recipient), PrincipalKind::Human, tenant());
    let page = list_inbox_ranked(
        &inbox,
        &viewer,
        &InboxFilter::all(),
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
        &DeterministicV1::default(),
    );
    assert_eq!(
        page.items.len(),
        3,
        "the ranked read reads the SAME projection the router wrote"
    );
    for r in &page.items {
        assert_eq!(
            r.priority, r.trace.final_priority,
            "the trace IS the rank's provenance"
        );
        assert!(
            !r.trace.render().is_empty(),
            "every routed rank carries an explain-trace"
        );
    }
    let priorities: Vec<u8> = page.items.iter().map(|r| r.priority).collect();
    let mut sorted = priorities.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(
        priorities, sorted,
        "the ranked read is priority-descending end to end"
    );
}
