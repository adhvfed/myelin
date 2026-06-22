//! # CDC — contract 7.1 `list_inbox` (the ONE inbox + the C-9 scoped-view filter) (P-183)
//!
//! **Architecture:** `notifications.md` §1.3 (the C-9 resolution — exactly ONE inbox; Issues "My
//! Work" / Chat "Activity" / Git "Review requests" are `filter`s over `reason`/`subject`, never a
//! second store), §3.4 step-0 (AUTHORIZE — a notification is a read of the subject; obeys `check`,
//! ADR-03). **Contract:** **7.1** `list_inbox(principal, filter?, page?) → [InboxItem]`.
//!
//! This CDC pins the 7.1 seam from BOTH sides:
//!
//! - **PROVIDER (Notif owns 7.1):** `list_inbox` reads the ONE inbox; a scoped `filter` (subsystem
//!   ∧ reason) returns a STRICT SUBSET (a view is a filter, not a store), and step-0 authorize
//!   drops an item the recipient cannot see (held, not leaked).
//! - **CONSUMER (a subsystem scoped view / the inbox UI / the CLI):** a subsystem that wants its own
//!   "my X" surface adds a *filtered view* over 7.1, never a second store — proven here by the
//!   three frozen views all reading the SAME rows as the unfiltered inbox (the C-9 invariant), and
//!   the CLI `inbox list|show` reading through the SAME `list_inbox` path (no back-door read).
//!
//! The two halves agree on the WIRE: the `InboxFilter` grammar (subsystem ∧ reason) + the step-0
//! authorize seam (`ReadAuthorizePort`, contract 4.2/4.10). A drift on either side (a filter that
//! becomes a store, an authorize that is skipped) breaks THIS build.

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

/// A mixed batch for `me` across the three subsystems — some rows in each scoped view, some not.
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

/// **PROVIDER + CONSUMER (the C-9 invariant): every scoped view ⊆ `list_inbox(filter=∅)` — a view
/// is a filter over the ONE inbox, never a second store.** The provider (Notif) returns the
/// unfiltered inbox; each consumer view (a subsystem's "my X" surface) returns a STRICT SUBSET — 0
/// rows in a view absent from the unfiltered inbox.
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
        // every view row carries the SAME read-state row identity as the unfiltered inbox (one
        // store → one read-state truth): the item_id in the view is the item_id in the full inbox.
        for vid in &view_ids {
            assert!(
                full_ids.contains(vid),
                "the view row {vid} is the SAME row as the unfiltered inbox (no second store)"
            );
        }
    }
}

/// **CONSUMER (the CLI scoped views): `myelin inbox list --view <name>` reads the contract view.**
/// The CLI my-work/activity/review-requests views select exactly the §1.3 rows, all through the
/// ONE `list_inbox` path — a CLI view is the contract view, never a second store.
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

    // the default CLI view is the ONE inbox.
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

/// **PROVIDER (step-0 authorize, contract 4.2/4.10): an item the recipient cannot READ is NOT
/// returned by `list_inbox` — held, not leaked (ADR-03).** A denying `ReadAuthorizePort` (the seam
/// the live Identity `check` plugs into) drops exactly the unseeable item; the CLI `show` of that
/// item is `None` (no back-door read).
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

    // the CLI `show` of the denied item is None (no back-door read through show).
    assert!(
        inbox_show(&inbox, &p, "iss-assigned", &deny, &strong()).is_none(),
        "show obeys the same authorize"
    );
}

/// **The 7.1 wire is recipient-scoped: another principal's items are never returned.** The provider
/// scopes to the calling principal; a consumer can only ever read its own inbox.
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
