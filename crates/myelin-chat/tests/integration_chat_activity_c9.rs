use std::collections::BTreeSet;

use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    list_inbox, AllowAllAuthorize, Class, InboxFilter, InboxProjection, Page, Reason,
    RoutedInboxItem,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn me() -> Principal {
    Principal::stub(PrincipalId("psn:me".into()), PrincipalKind::Human, tenant())
}

fn at() -> Consistency {
    Consistency {
        at_least: Zookie("zk-0".into()),
        mode: ConsistencyMode::Strong,
    }
}

fn item(item_id: &str, subject: &str, reason: Reason) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: Region("fr-par".into()),
        item_id: item_id.into(),
        recipient: "psn:me".into(),
        subject: ArtifactRef(subject.into()),
        reason,
        class: Class::Direct,
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
        dedup_key: item_id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

fn seeded_inbox() -> InboxProjection {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item(
        "chat-mentioned",
        "myelin://acme/chat/channel/eng",
        Reason::Mentioned,
    ));
    inbox.upsert_for_test(item(
        "chat-replied",
        "myelin://acme/chat/thread/T1",
        Reason::Replied,
    ));
    inbox.upsert_for_test(item(
        "chat-watched",
        "myelin://acme/chat/thread/T2",
        Reason::ThreadWatched,
    ));
    inbox.upsert_for_test(item(
        "chat-approval",
        "myelin://acme/chat/message/M9",
        Reason::ApprovalRequested,
    ));
    inbox.upsert_for_test(item(
        "chat-fyi",
        "myelin://acme/chat/channel/random",
        Reason::Fyi,
    ));
    inbox.upsert_for_test(item(
        "iss-mentioned",
        "myelin://acme/issue/issue/E-1",
        Reason::Mentioned,
    ));
    inbox.upsert_for_test(item(
        "git-review",
        "myelin://acme/git/pr/9",
        Reason::ReviewRequested,
    ));
    inbox
}

#[test]
fn chat_activity_is_a_strict_subset_of_the_one_inbox() {
    let inbox = seeded_inbox();
    let auth = AllowAllAuthorize;
    let big_page = Page {
        after: None,
        limit: 1000,
    };

    let all = list_inbox(&inbox, &me(), &InboxFilter::all(), &big_page, &auth, &at());
    let all_ids: BTreeSet<String> = all.items.iter().map(|i| i.item_id.clone()).collect();

    let activity = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::chat_activity(),
        &big_page,
        &auth,
        &at(),
    );
    let activity_ids: BTreeSet<String> = activity.items.iter().map(|i| i.item_id.clone()).collect();

    assert!(
        activity_ids.is_subset(&all_ids),
        "C-9: Activity rows ⊆ list_inbox(filter=∅)"
    );
    assert!(
        activity_ids.len() < all_ids.len(),
        "Activity is a STRICT subset (the non-Activity + other-subsystem rows are excluded)"
    );

    let activity_reasons = InboxFilter::chat_activity().reasons.unwrap();
    for row in &activity.items {
        assert!(
            row.subject.0.contains("/chat/"),
            "every Activity row is a Chat subject: {}",
            row.subject.0
        );
        assert!(
            activity_reasons.contains(&row.reason),
            "every Activity row carries an Activity reason: {:?}",
            row.reason
        );
    }
    assert!(
        !activity_ids.contains("iss-mentioned"),
        "a mentioned Issue is NOT chat Activity (the subsystem filter bites, not just the reason)"
    );
    assert!(
        !activity_ids.contains("chat-fyi"),
        "a non-Activity Chat reason is excluded"
    );
    assert!(
        !activity_ids.contains("git-review"),
        "a Git row is not in Chat Activity"
    );

    assert_eq!(
        activity_ids,
        [
            "chat-mentioned",
            "chat-replied",
            "chat-watched",
            "chat-approval"
        ]
        .into_iter()
        .map(String::from)
        .collect::<BTreeSet<String>>(),
        "Activity returns exactly the Chat rows with a registered Activity reason"
    );
}

#[test]
fn activity_shares_the_one_read_state_row() {
    let inbox = seeded_inbox();
    let auth = AllowAllAuthorize;
    let big_page = Page {
        after: None,
        limit: 1000,
    };

    let all = list_inbox(&inbox, &me(), &InboxFilter::all(), &big_page, &auth, &at());
    let activity = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::chat_activity(),
        &big_page,
        &auth,
        &at(),
    );

    let in_all = all
        .items
        .iter()
        .find(|i| i.item_id == "chat-mentioned")
        .unwrap();
    let in_view = activity
        .items
        .iter()
        .find(|i| i.item_id == "chat-mentioned")
        .unwrap();
    assert_eq!(
        in_all.item_id, in_view.item_id,
        "same row across the two views"
    );
    assert_eq!(
        in_all.state, in_view.state,
        "same read-state column (not a second store)"
    );
}
