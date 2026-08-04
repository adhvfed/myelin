use myelin_events::ArtifactRef;
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::cli::{inbox_read, inbox_snooze, CliView};
use myelin_notif::list_inbox::{list_inbox, AllowAllAuthorize, InboxFilter, Page};
use myelin_notif::read_state::{
    active_inbox, mark, mark_all_read, snooze, ReadState, ReadStateError,
};
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::{cli, Class, Reason};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong() -> Consistency {
    Consistency {
        at_least: Zookie("zk-7.2".into()),
        mode: ConsistencyMode::Strong,
    }
}

fn item(recipient: &str, id: &str, subject: &str, reason: Reason) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: Region("fr-par".into()),
        item_id: id.into(),
        recipient: recipient.into(),
        subject: ArtifactRef(subject.into()),
        reason,
        class: Class::Direct,
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{id}")),
        dedup_key: id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

fn seeded(me: &str) -> InboxProjection {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item(
        me,
        "iss-assigned",
        "myelin://acme/issue/issue/PROJ-1",
        Reason::Assigned,
    ));
    inbox.upsert_for_test(item(
        me,
        "iss-state",
        "myelin://acme/issue/issue/PROJ-2",
        Reason::StateChanged,
    ));
    inbox.upsert_for_test(item(
        me,
        "chat-ment",
        "myelin://acme/chat/thread/T1",
        Reason::Mentioned,
    ));
    inbox.upsert_for_test(item(
        me,
        "git-review",
        "myelin://acme/git/pr/9",
        Reason::ReviewRequested,
    ));
    inbox
}

fn state_in_view(
    inbox: &InboxProjection,
    p: &Principal,
    filter: &InboxFilter,
    item_id: &str,
) -> Option<String> {
    let page = list_inbox(
        inbox,
        p,
        filter,
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
    );
    page.items
        .into_iter()
        .find(|r| r.item_id == item_id)
        .map(|r| r.state)
}

#[test]
fn read_state_is_one_truth_across_views_zero_divergence() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);

    mark(&inbox, &p, "iss-assigned", ReadState::Read).expect("mark my own item");

    assert_eq!(
        state_in_view(&inbox, &p, &InboxFilter::issues_my_work(), "iss-assigned").as_deref(),
        Some("read"),
        "read in the scoped view"
    );
    assert_eq!(
        state_in_view(&inbox, &p, &InboxFilter::all(), "iss-assigned").as_deref(),
        Some("read"),
        "read in the unified inbox (same row)"
    );

    mark(&inbox, &p, "chat-ment", ReadState::Read).unwrap();
    assert_eq!(
        state_in_view(&inbox, &p, &InboxFilter::chat_activity(), "chat-ment").as_deref(),
        Some("read"),
        "read in the Chat view (same row)"
    );
}

#[test]
fn snooze_suppresses_from_active_inbox_and_records_until() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);
    let until = "2026-06-25T09:00:00Z";

    snooze(&inbox, &p, "git-review", until).expect("snooze my own item");

    let row = inbox
        .snapshot_for_tenant(&tenant())
        .into_iter()
        .find(|r| r.item_id == "git-review")
        .unwrap();
    assert_eq!(row.state, "snoozed");
    assert_eq!(
        row.snooze_until.as_deref(),
        Some(until),
        "the until is persisted (7.2)"
    );

    let full = list_inbox(
        &inbox,
        &p,
        &InboxFilter::all(),
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
    );
    assert!(
        full.items.iter().any(|r| r.item_id == "git-review"),
        "still in the store (re-surfaces on its timer)"
    );
    let active = active_inbox(full.items);
    assert!(
        !active.iter().any(|r| r.item_id == "git-review"),
        "the snoozed item is ABSENT from the active inbox"
    );
}

#[test]
fn mark_all_read_flips_exactly_the_filtered_rows_across_views() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);

    let n = mark_all_read(&inbox, &p, &InboxFilter::issues_my_work());
    assert_eq!(n, 1, "exactly the one My Work row flipped");

    assert_eq!(
        state_in_view(&inbox, &p, &InboxFilter::issues_my_work(), "iss-assigned").as_deref(),
        Some("read")
    );
    assert_eq!(
        state_in_view(&inbox, &p, &InboxFilter::all(), "iss-assigned").as_deref(),
        Some("read"),
        "same row, read in the unified inbox"
    );

    assert_eq!(
        state_in_view(&inbox, &p, &InboxFilter::all(), "chat-ment").as_deref(),
        Some("unread")
    );
    assert_eq!(
        state_in_view(&inbox, &p, &InboxFilter::all(), "git-review").as_deref(),
        Some("unread")
    );
    assert_eq!(
        state_in_view(&inbox, &p, &InboxFilter::all(), "iss-state").as_deref(),
        Some("unread")
    );
}

#[test]
fn cli_read_and_snooze_drive_the_read_state_contract() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);

    inbox_read(&inbox, &p, "iss-assigned").expect("read my own item");
    assert_eq!(
        cli::inbox_show(&inbox, &p, "iss-assigned", &AllowAllAuthorize, &strong())
            .unwrap()
            .state,
        "read"
    );

    inbox_snooze(&inbox, &p, "chat-ment", "2026-06-25T09:00:00Z").expect("snooze my own item");
    let page = cli::inbox_list(
        &inbox,
        &p,
        CliView::All,
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
    );
    let active = active_inbox(page.items);
    assert!(
        !active.iter().any(|r| r.item_id == "chat-ment"),
        "the snoozed item left the active inbox"
    );

    assert_eq!(
        inbox_read(&inbox, &principal("u2"), "iss-assigned"),
        Err(ReadStateError::NotFound),
        "you can only read your own items"
    );
}

#[test]
fn chained_ingest_mark_all_read_relist_consistent() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);

    mark_all_read(&inbox, &p, &InboxFilter::issues_my_work());

    let view = list_inbox(
        &inbox,
        &p,
        &InboxFilter::issues_my_work(),
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
    );
    for r in &view.items {
        assert_eq!(r.state, "read", "every My Work row reads `read`");
    }
    let full = list_inbox(
        &inbox,
        &p,
        &InboxFilter::all(),
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
    );
    let assigned = full
        .items
        .iter()
        .find(|r| r.item_id == "iss-assigned")
        .unwrap();
    assert_eq!(
        assigned.state, "read",
        "the My Work row is read in the unified inbox too"
    );
    let chat = full
        .items
        .iter()
        .find(|r| r.item_id == "chat-ment")
        .unwrap();
    assert_eq!(
        chat.state, "unread",
        "a row outside the marked view stays unread in every view"
    );
}
