use myelin_chat::fanout::{
    activity, no_second_activity_store, resolve_watchers, write_fanout, AddressedRecipient, Signal,
    SignalSink, WatcherDirectory, WriteFanoutReason,
};

use myelin_events::{BusTransport, EventHandler, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, ObjectType, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::expand::Expand;
use myelin_identity_service::namespace::NamespaceEngine;
use myelin_identity_service::reverse_index::{ReverseIndex, ReverseIndexConsumer};
use myelin_identity_service::tuple_store::TupleStore;
use myelin_notif::list_inbox::AllowAllAuthorize;
use myelin_notif::{
    InboxFilter, InboxProjection, NotifRuleRegistry, Page, Reason, RoutedInboxItem,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const TENANT: &str = "acme";

fn scope() -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(TENANT.into()),
    );
    TenantScope::from_verified_token(&p, Region("fr-par".into()))
}

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(TENANT.into()),
    )
}

fn strong() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn seed_engine(deltas: &[TupleDelta]) -> Expand {
    let scope = scope();
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    store
        .write_tuples(
            &scope,
            &principal("p-admin"),
            deltas,
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("seed write_tuples");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    Expand::new(store, NamespaceEngine::with_core_hierarchy(), index)
}

struct EngineWatchers {
    expand: Expand,
    scope: TenantScope,
}
impl WatcherDirectory for EngineWatchers {
    fn list_watchers(&self, channel: &ObjectId, at: &Consistency) -> Vec<PrincipalId> {
        self.expand
            .list_subjects(
                &self.scope,
                channel,
                &ObjectType("channel".into()),
                &Permission(myelin_chat::WATCHER_RELATION.into()),
                at,
            )
            .members
    }
}

#[derive(Default)]
struct CountingSink {
    count: std::cell::RefCell<usize>,
}
impl SignalSink for CountingSink {
    fn emit_signal(&self, _signal: &Signal) {
        *self.count.borrow_mut() += 1;
    }
}

#[test]
fn cdc_4_4_read_fanout_watchers_resolve_via_the_real_list_subjects() {
    let expand = seed_engine(&[
        add("channel:general", "watcher", "p:alice"),
        add("channel:general", "watcher", "p:bob"),
        add("channel:general", "watcher", "p:carol"),
        add("channel:random", "watcher", "p:dave"),
    ]);
    let dir = EngineWatchers {
        expand,
        scope: scope(),
    };
    let watchers = resolve_watchers(&dir, &ObjectId("channel:general".into()), &strong());
    let got: Vec<String> = watchers.iter().map(|p| p.0.clone()).collect();
    assert_eq!(
        got,
        vec!["p:alice".to_string(), "p:bob".into(), "p:carol".into()],
        "the read-fanout audience is #general's watchers (via the REAL list_subjects, 4.4)"
    );
    assert!(
        !got.contains(&"p:dave".to_string()),
        "another channel's watcher never leaks into the read-fanout set"
    );
}

#[test]
fn cdc_4_4_resolving_watchers_writes_zero_inbox_items_on_an_ambient_post() {
    let deltas: Vec<TupleDelta> = (0..500)
        .map(|i| add("channel:announce", "watcher", &format!("p:{i}")))
        .collect();
    let expand = seed_engine(&deltas);
    let dir = EngineWatchers {
        expand,
        scope: scope(),
    };
    let watchers = resolve_watchers(&dir, &ObjectId("channel:announce".into()), &strong());
    assert_eq!(
        watchers.len(),
        500,
        "the dense read-fanout audience resolves via the real index"
    );

    let sink = CountingSink::default();
    let writes = write_fanout(
        &sink,
        "chat.message.created",
        "myelin://acme/chat/channel/announce",
        &[],
    );
    assert_eq!(
        writes, 0,
        "knowing 500 watchers, the ambient post writes 0 per-member items"
    );
    assert_eq!(*sink.count.borrow(), 0);
}

#[test]
fn cdc_7_6_write_fanout_signal_rule_keys_are_admitted_by_define_notif_rule() {
    let mut registry = NotifRuleRegistry::new();
    myelin_chat::glue::register_chat_notif_rules(&mut registry);
    for rule_key in [
        "chat.message.mentioned",
        "chat.thread.replied",
        "chat.thread.watched",
        "chat.approval.requested",
    ] {
        assert!(
            registry.rule(rule_key).is_some(),
            "Notif's REAL registry admits chat's `{rule_key}` rule (7.6)"
        );
    }

    let sink = CountingSink::default();
    let addressed = [AddressedRecipient {
        principal: PrincipalId("p:alice".into()),
        reason: WriteFanoutReason::Mentioned,
    }];
    let writes = write_fanout(
        &sink,
        "chat.message.mentioned",
        "myelin://acme/chat/message/m1",
        &addressed,
    );
    assert_eq!(
        writes, 1,
        "the mention write-fans its 1 addressed recipient (7.6 rule-keyed Signal)"
    );
}

fn item(recipient: &str, item_id: &str, subject: &str, reason: Reason) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: TenantId(TENANT.into()),
        region: Region("fr-par".into()),
        item_id: item_id.into(),
        recipient: recipient.into(),
        subject: myelin_events::ArtifactRef(subject.into()),
        reason,
        class: myelin_notif::Class::Direct,
        origin_event: myelin_events::ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
        dedup_key: item_id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

#[test]
fn cdc_7_1_chat_activity_is_a_list_inbox_view_not_a_second_store() {
    let me = "u1";
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item(
        me,
        "c-ment",
        "myelin://acme/chat/message/M1",
        Reason::Mentioned,
    ));
    inbox.upsert_for_test(item(
        me,
        "c-reply",
        "myelin://acme/chat/thread/T1",
        Reason::Replied,
    ));
    inbox.upsert_for_test(item(
        me,
        "c-watch",
        "myelin://acme/chat/thread/T2",
        Reason::ThreadWatched,
    ));
    inbox.upsert_for_test(item(
        me,
        "c-appr",
        "myelin://acme/chat/message/M2",
        Reason::ApprovalRequested,
    ));
    inbox.upsert_for_test(item(
        me,
        "c-state",
        "myelin://acme/chat/channel/C1",
        Reason::StateChanged,
    ));
    inbox.upsert_for_test(item(
        me,
        "i-ment",
        "myelin://acme/issue/issue/P1",
        Reason::Mentioned,
    ));

    let big = Page {
        after: None,
        limit: 1000,
    };
    let full = myelin_notif::list_inbox(
        &inbox,
        &principal(me),
        &InboxFilter::all(),
        &big,
        &AllowAllAuthorize,
        &strong(),
    );
    let full_ids: std::collections::BTreeSet<String> =
        full.items.iter().map(|i| i.item_id.clone()).collect();
    assert_eq!(full_ids.len(), 6, "the ONE inbox holds all 6 rows for u1");

    let act = activity(&inbox, &principal(me), &big, &AllowAllAuthorize, &strong());
    let act_ids: std::collections::BTreeSet<String> =
        act.items.iter().map(|i| i.item_id.clone()).collect();
    assert_eq!(
        act_ids,
        ["c-ment", "c-reply", "c-watch", "c-appr"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "Activity = the four chat-activity rows (mentioned/replied/thread_watched/approval_requested)"
    );
    assert!(
        act_ids.is_subset(&full_ids),
        "C-9: the Activity view is a SUBSET of the ONE inbox"
    );
    assert!(
        act_ids.len() < full_ids.len(),
        "the Activity view is STRICTLY smaller than the ONE inbox"
    );
    assert!(
        !act_ids.contains("c-state"),
        "an ambient chat row is not in Activity"
    );
    assert!(
        !act_ids.contains("i-ment"),
        "a foreign-subsystem mention is not in the chat Activity view (subsystem clause)"
    );

    assert!(
        no_second_activity_store(),
        "Activity is a list_inbox filter, never a second store (the CI gate)"
    );
}

#[test]
fn cdc_7_1_activity_consumes_the_frozen_filter_one_read_state_truth() {
    assert_eq!(
        myelin_chat::activity_filter(),
        InboxFilter::chat_activity(),
        "chat's Activity filter IS the frozen one (no chat-private filter shape, one read-state truth)"
    );
}
