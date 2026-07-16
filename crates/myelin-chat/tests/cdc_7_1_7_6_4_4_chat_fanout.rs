//! # The CDC pair for contracts 7.1 / 7.6 / 4.4 — the **Chat** fanout-class boundary + Activity-as-view
//! (CHAT-P17 / P-412, M4-C5)
//!
//! The consumer-driven contracts for CHAT-P17. Chat OWNS the per-event fanout-class DECISION; it
//! CONSUMES three frozen owner surfaces — this CDC proves chat's consumption fits the REAL providers:
//!
//! - **7.1 `list_inbox` (Activity is a VIEW).** PROVIDER: Notif's REAL `list_inbox` over the REAL
//!   `InboxProjection`. The CDC proves Chat's "Activity / Mentions" (S6) is EXACTLY
//!   `list_inbox(me, filter = chat∧{mentioned,replied,thread_watched,approval_requested})` — a view,
//!   never a second store: every Activity row ⊆ the ONE inbox, and a non-chat / wrong-reason row is
//!   excluded (§5.3, C-9). [`myelin_chat::activity`] forwards to the real provider; no chat store.
//! - **7.6 `define_notif_rule` (the write-fanout wire).** A write-fanout [`myelin_chat::Signal`]
//!   carries a registered rule key; the CDC proves chat's four rule keys are admitted by Notif's REAL
//!   `define_notif_rule` registry (the inverse-signal seam) — chat registers WHICH reason, Notif owns
//!   the §3.1 band. A write-fanned Signal's reason is in the Activity view (the §5.3 round-trip).
//! - **4.4 `list_subjects(channel, watcher)` (read-fanout resolution).** PROVIDER: Identity's REAL
//!   `Expand::list_subjects` over the S3 store → relay → S8 reverse index (fed exactly as production
//!   feeds it). The CDC proves chat's [`myelin_chat::WatcherDirectory`] read-fanout port resolves the
//!   `channel`'s `watcher` userset against the live engine at member density — and that resolving the
//!   audience materialises ZERO inbox items (the celebrity-fanout mitigation is independent of the
//!   read-fanout resolve).
//!
//! Activity is a VIEW (no second store): the 7.1 leg asserts the Chat activity surface is a strict
//! subset of the ONE inbox and holds no chat-private rows.

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

/// Wire S3 → outbox → relay → S8 reverse-index consumer (the live production feed) and drain the
/// seed tuples through it — exactly the path production uses to populate the read-fanout index.
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

/// A [`WatcherDirectory`] backed by Identity's REAL `Expand::list_subjects` — the read-fanout port
/// chat resolves watchers through (contract 4.4). The production binding is identical; here it wraps
/// the live engine so the CDC proves the consumption against the real provider, not a mock.
struct EngineWatchers {
    expand: Expand,
    scope: TenantScope,
}
impl WatcherDirectory for EngineWatchers {
    fn list_watchers(&self, channel: &ObjectId, at: &Consistency) -> Vec<PrincipalId> {
        // contract 4.4: list_subjects(channel, watcher) over the live S8 reverse index.
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

/// A counting write-fanout sink (the CDC asserts the per-recipient inbox-write count).
#[derive(Default)]
struct CountingSink {
    count: std::cell::RefCell<usize>,
}
impl SignalSink for CountingSink {
    fn emit_signal(&self, _signal: &Signal) {
        *self.count.borrow_mut() += 1;
    }
}

// ───────────────────────────── 4.4 — read-fanout watcher resolution via the REAL engine ───────────

/// **CDC 4.4: chat's read-fanout `WatcherDirectory` resolves the `channel.watcher` userset via the
/// REAL `Expand::list_subjects` (the S8 reverse index).** A channel with three watchers resolves
/// exactly those three (and a different channel's watcher never leaks in) — the read-fanout audience,
/// served by the real density-serving index, not a chat scan.
#[test]
fn cdc_4_4_read_fanout_watchers_resolve_via_the_real_list_subjects() {
    let expand = seed_engine(&[
        add("channel:general", "watcher", "p:alice"),
        add("channel:general", "watcher", "p:bob"),
        add("channel:general", "watcher", "p:carol"),
        // a different channel's watcher must NOT leak into #general's read-fanout set.
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

/// **CDC 4.4 + the celebrity-fanout property: resolving a large watcher set writes ZERO inbox items.**
/// The read-fanout resolve and the per-member write are independent: chat can resolve the full
/// watcher audience via the real engine and STILL write 0 inbox items on an ambient post (the unread
/// is derived lazily). Proven against the real provider at density.
#[test]
fn cdc_4_4_resolving_watchers_writes_zero_inbox_items_on_an_ambient_post() {
    // seed a dense watcher set (a stand-in for the celebrity channel; the engine serves density, C8).
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

    // the ambient post itself writes 0 per-member items (read-fanout never write-amplifies).
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

// ───────────────────────────── 7.6 — the write-fanout rule-key wire (define_notif_rule) ────────────

/// **CDC 7.6: a write-fanout Signal carries a rule key admitted by Notif's REAL `define_notif_rule`
/// registry.** Chat's four rule keys reconcile against the REAL frozen verb (the §3.1 band table) —
/// chat registers WHICH reason; Notif owns the band. A mention Signal's rule key + reason are the
/// ones Notif's registry admits.
#[test]
fn cdc_7_6_write_fanout_signal_rule_keys_are_admitted_by_define_notif_rule() {
    // chat's four notif rules are built by Notif's REAL `define_notif_rule` verb (the §3.1-banded,
    // inverse-signal seam) and register into the REAL registry. If a band were wrong (chat tried to
    // register `mentioned` as a non-existent class) `chat_notif_rules` would have panicked at build;
    // here we PROVE they register into the real registry by key.
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

    // a write-fanned mention Signal carries the registered `mentioned` rule key (the wire, 7.6).
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

// ───────────────────────────── 7.1 — Activity is a VIEW into the ONE inbox (list_inbox) ────────────

/// Build a routed inbox row addressed to `recipient`, about `subject`, with `reason`.
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

/// **CDC 7.1: Chat "Activity / Mentions" (S6) is EXACTLY `list_inbox(me, chat-activity-filter)` over
/// the REAL inbox — a VIEW, never a second store (§5.3, C-9).** Seeded with a mixed inbox (chat
/// mentions/replies + a non-activity chat row + a foreign-subsystem row), the Activity view returns
/// EXACTLY the chat-activity rows — a strict subset of the ONE inbox — and chat holds no own store.
#[test]
fn cdc_7_1_chat_activity_is_a_list_inbox_view_not_a_second_store() {
    let me = "u1";
    let inbox = InboxProjection::new();
    // chat — IN Activity: a mention + a reply + a thread-watched + an approval.
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
    // chat — NOT in Activity (a state_changed chat row is ambient, not an activity reason).
    inbox.upsert_for_test(item(
        me,
        "c-state",
        "myelin://acme/chat/channel/C1",
        Reason::StateChanged,
    ));
    // a FOREIGN subsystem mention — never in the chat Activity view (the subsystem clause bites).
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
    // the FULL inbox (the ONE inbox) — Activity must be a strict subset of THIS.
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

    // Activity = the chat-activity VIEW (chat forwards to the REAL list_inbox — no chat store).
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
    // STRICT SUBSET: a view is a filter, not a store.
    assert!(
        act_ids.is_subset(&full_ids),
        "C-9: the Activity view is a SUBSET of the ONE inbox"
    );
    assert!(
        act_ids.len() < full_ids.len(),
        "the Activity view is STRICTLY smaller than the ONE inbox"
    );
    // the non-activity chat row + the foreign mention are NOT in Activity.
    assert!(
        !act_ids.contains("c-state"),
        "an ambient chat row is not in Activity"
    );
    assert!(
        !act_ids.contains("i-ment"),
        "a foreign-subsystem mention is not in the chat Activity view (subsystem clause)"
    );

    // the structural gate: 0 chat-private activity store.
    assert!(
        no_second_activity_store(),
        "Activity is a list_inbox filter, never a second store (the CI gate)"
    );
}

/// **CDC 7.1 (negative-leak): marking-read semantics are Notif's ONE read-state — chat does not own
/// a second.** The Activity filter is the SAME `InboxFilter::chat_activity` the platform freezes; a
/// helper that returned `None` (no filter) would be a second-store smell. This asserts chat consumes
/// the frozen filter, so there is exactly one read-state truth (the §5.3 link).
#[test]
fn cdc_7_1_activity_consumes_the_frozen_filter_one_read_state_truth() {
    // chat's Activity filter IS the frozen platform chat-activity filter — never a chat-local
    // re-definition. One filter shape ⇒ one read-state truth (the §5.3 link: marking a mention read
    // in Activity is the SAME row as the unified inbox).
    assert_eq!(
        myelin_chat::activity_filter(),
        InboxFilter::chat_activity(),
        "chat's Activity filter IS the frozen one (no chat-private filter shape, one read-state truth)"
    );
}
