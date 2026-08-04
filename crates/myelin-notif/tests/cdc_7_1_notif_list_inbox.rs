use myelin_events::ArtifactRef;
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::cli::{inbox_list, inbox_show, CliView};
use myelin_notif::list_inbox::{
    list_inbox, AllowAllAuthorize, InboxFilter, Page, ReadAuthorizePort,
};
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::{Class, Reason};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong() -> Consistency {
    Consistency {
        at_least: Zookie("zk-7.1".into()),
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
    inbox.upsert_for_test(item(
        me,
        "git-watched",
        "myelin://acme/git/pr/10",
        Reason::Watched,
    ));
    inbox
}

fn id_set(items: &[RoutedInboxItem]) -> std::collections::BTreeSet<String> {
    items.iter().map(|i| i.item_id.clone()).collect()
}

#[test]
fn list_inbox_scoped_views_are_strict_subsets_of_the_one_inbox() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);
    let big = Page {
        after: None,
        limit: 1000,
    };

    let full = list_inbox(
        &inbox,
        &p,
        &InboxFilter::all(),
        &big,
        &AllowAllAuthorize,
        &strong(),
    );
    let full_ids = id_set(&full.items);
    assert_eq!(
        full_ids.len(),
        5,
        "the unfiltered inbox is the ONE inbox (all 5 rows for u1)"
    );

    for view in [
        InboxFilter::issues_my_work(),
        InboxFilter::chat_activity(),
        InboxFilter::git_review_requests(),
    ] {
        let v = list_inbox(&inbox, &p, &view, &big, &AllowAllAuthorize, &strong());
        let view_ids = id_set(&v.items);
        assert!(
            view_ids.is_subset(&full_ids),
            "C-9: the view {view:?} ⊆ the unfiltered inbox"
        );
        assert!(
            view_ids.len() < full_ids.len(),
            "a scoped view is STRICTLY smaller (a filter, not the store)"
        );
        for vid in &view_ids {
            assert!(
                full_ids.contains(vid),
                "the view row {vid} is the SAME row as the unfiltered inbox (no second store)"
            );
        }
    }
}

#[test]
fn cli_views_read_the_contract_filtered_views() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);
    let big = Page {
        after: None,
        limit: 1000,
    };

    let my_work = id_set(
        &inbox_list(
            &inbox,
            &p,
            CliView::MyWork,
            &big,
            &AllowAllAuthorize,
            &strong(),
        )
        .items,
    );
    assert_eq!(my_work, ["iss-assigned".to_string()].into_iter().collect());

    let activity = id_set(
        &inbox_list(
            &inbox,
            &p,
            CliView::Activity,
            &big,
            &AllowAllAuthorize,
            &strong(),
        )
        .items,
    );
    assert_eq!(activity, ["chat-ment".to_string()].into_iter().collect());

    let reviews = id_set(
        &inbox_list(
            &inbox,
            &p,
            CliView::ReviewRequests,
            &big,
            &AllowAllAuthorize,
            &strong(),
        )
        .items,
    );
    assert_eq!(reviews, ["git-review".to_string()].into_iter().collect());

    let all = inbox_list(
        &inbox,
        &p,
        CliView::All,
        &big,
        &AllowAllAuthorize,
        &strong(),
    );
    assert_eq!(all.items.len(), 5, "the default CLI view is the ONE inbox");
}

#[test]
fn list_inbox_obeys_step0_authorize_held_not_leaked() {
    struct DenyOne(String);
    impl ReadAuthorizePort for DenyOne {
        fn can_read(&self, _v: &Principal, subject: &ArtifactRef, _at: &Consistency) -> Decision {
            if subject.0 == self.0 {
                Decision::Deny
            } else {
                Decision::Allow
            }
        }
    }
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);
    let deny = DenyOne("myelin://acme/issue/issue/PROJ-1".into());
    let big = Page {
        after: None,
        limit: 1000,
    };

    let page = list_inbox(&inbox, &p, &InboxFilter::all(), &big, &deny, &strong());
    let got = id_set(&page.items);
    assert!(
        !got.contains("iss-assigned"),
        "the denied item is HELD, not leaked (ADR-03)"
    );
    assert_eq!(got.len(), 4, "the other 4 visible items surface");

    assert!(
        inbox_show(&inbox, &p, "iss-assigned", &deny, &strong()).is_none(),
        "show obeys the same authorize"
    );
}

#[test]
fn list_inbox_is_recipient_scoped() {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item(
        "u1",
        "mine",
        "myelin://acme/issue/issue/P1",
        Reason::Assigned,
    ));
    inbox.upsert_for_test(item(
        "u2",
        "theirs",
        "myelin://acme/issue/issue/P2",
        Reason::Assigned,
    ));
    let page = list_inbox(
        &inbox,
        &principal("u1"),
        &InboxFilter::all(),
        &Page::default(),
        &AllowAllAuthorize,
        &strong(),
    );
    let got = id_set(&page.items);
    assert_eq!(
        got,
        ["mine".to_string()].into_iter().collect(),
        "only the caller's items (recipient scope)"
    );
}
