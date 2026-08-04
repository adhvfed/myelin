use std::collections::BTreeSet;

use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    list_inbox, AllowAllAuthorize, InboxFilter, InboxProjection, Page, Reason, RoutedInboxItem,
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
        class: myelin_notif::Class::Direct,
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
        "iss-assigned",
        "myelin://acme/issue/issue/E-1",
        Reason::Assigned,
    ));
    inbox.upsert_for_test(item(
        "iss-blocked",
        "myelin://acme/issue/issue/E-2",
        Reason::Blocked,
    ));
    inbox.upsert_for_test(item(
        "iss-sla",
        "myelin://acme/issue/issue/E-3",
        Reason::Sla,
    ));
    inbox.upsert_for_test(item(
        "iss-approval",
        "myelin://acme/issue/issue/E-4",
        Reason::ApprovalRequested,
    ));
    inbox.upsert_for_test(item(
        "iss-watched",
        "myelin://acme/issue/issue/E-5",
        Reason::Watched,
    ));
    inbox.upsert_for_test(item(
        "iss-fyi",
        "myelin://acme/issue/issue/E-6",
        Reason::Fyi,
    ));
    inbox.upsert_for_test(item(
        "git-review",
        "myelin://acme/git/pr/9",
        Reason::ReviewRequested,
    ));
    inbox.upsert_for_test(item(
        "chat-ment",
        "myelin://acme/chat/thread/T1",
        Reason::Mentioned,
    ));
    inbox
}

#[test]
fn issues_my_work_is_a_strict_subset_of_the_one_inbox() {
    let inbox = seeded_inbox();
    let auth = AllowAllAuthorize;
    let big_page = Page {
        after: None,
        limit: 1000,
    };

    let all = list_inbox(&inbox, &me(), &InboxFilter::all(), &big_page, &auth, &at());
    let all_ids: BTreeSet<String> = all.items.iter().map(|i| i.item_id.clone()).collect();

    let my_work = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::issues_my_work(),
        &big_page,
        &auth,
        &at(),
    );
    let my_work_ids: BTreeSet<String> = my_work.items.iter().map(|i| i.item_id.clone()).collect();

    assert!(
        my_work_ids.is_subset(&all_ids),
        "C-9: My Work rows ⊆ list_inbox(filter=∅)"
    );
    assert!(
        my_work_ids.len() < all_ids.len(),
        "My Work is a STRICT subset (the non-My-Work + other-subsystem rows are excluded)"
    );

    let my_work_reasons = InboxFilter::issues_my_work().reasons.unwrap();
    for row in &my_work.items {
        assert!(
            row.subject.0.contains("/issue/"),
            "every My Work row is an Issues subject: {}",
            row.subject.0
        );
        assert!(
            my_work_reasons.contains(&row.reason),
            "every My Work row carries a My-Work reason: {:?}",
            row.reason
        );
    }
    assert!(
        !my_work_ids.contains("iss-fyi"),
        "a non-My-Work Issues reason is excluded"
    );
    assert!(
        !my_work_ids.contains("git-review"),
        "a Git row is not in Issues My Work"
    );
    assert!(
        !my_work_ids.contains("chat-ment"),
        "a Chat row is not in Issues My Work"
    );

    assert_eq!(
        my_work_ids,
        [
            "iss-assigned",
            "iss-blocked",
            "iss-sla",
            "iss-approval",
            "iss-watched"
        ]
        .into_iter()
        .map(String::from)
        .collect::<BTreeSet<String>>(),
        "My Work returns exactly the Issues rows with a registered My-Work reason"
    );
}

#[test]
fn my_work_shares_the_one_read_state_row() {
    let inbox = seeded_inbox();
    let auth = AllowAllAuthorize;
    let big_page = Page {
        after: None,
        limit: 1000,
    };

    let all = list_inbox(&inbox, &me(), &InboxFilter::all(), &big_page, &auth, &at());
    let my_work = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::issues_my_work(),
        &big_page,
        &auth,
        &at(),
    );

    let in_all = all
        .items
        .iter()
        .find(|i| i.item_id == "iss-assigned")
        .unwrap();
    let in_view = my_work
        .items
        .iter()
        .find(|i| i.item_id == "iss-assigned")
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
