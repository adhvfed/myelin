//! # The C-9 invariant — Issues "My Work" is a FILTER over the ONE inbox, not a second store
//! (NOTIF-P21 / P-342, M4)
//!
//! **Architecture `05-refined-shared-systems-architecture/notifications.md` §1.3** (the C-9 resolution
//! — there is exactly ONE cross-subsystem inbox; Issues "My Work" is a *scoped, filtered query INTO
//! this one inbox*, a `filter` over the item's structured `reason` + `subject`, NEVER a separate
//! store) and **recon §5** ("My Work" = `list_inbox(principal, filter)` over the ONE inbox; never a
//! second store). **External insight** `01-process-and-quality-doctrine.md` §3 (prove-it — the C-9
//! invariant test forces the "a view is a subset" property).
//!
//! **The threshold (never weakened):** the rows Issues "My Work" returns are a STRICT SUBSET of the
//! unfiltered inbox — `my_work_rows ⊆ list_inbox(filter=∅)`. The view adds NO row the canonical inbox
//! lacks (it only narrows), and it shares the ONE read-state column (read it in "My Work", it is read
//! everywhere). This is the Issues-side proof that "My Work" is a saved filter, not a fourth inbox.

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

/// Seed the ONE inbox with a mix: Issues rows IN "My Work" (every Issues consumer reason), Issues rows
/// NOT in "My Work" (a non-My-Work reason on an issue), and OTHER subsystems' rows (Git/Chat) that
/// must never appear in Issues "My Work".
fn seeded_inbox() -> InboxProjection {
    let inbox = InboxProjection::new();
    // Issues rows that ARE in "My Work" — the §1.3 reason set Issues now registers (NOTIF-P21):
    // assigned / blocked / sla / approval_requested (+ mentioned/review_requested/watched).
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
    // An Issues row NOT in "My Work" (a reason outside the My Work filter) — must be excluded.
    inbox.upsert_for_test(item(
        "iss-fyi",
        "myelin://acme/issue/issue/E-6",
        Reason::Fyi,
    ));
    // OTHER subsystems — never in Issues "My Work" (the subsystem filter excludes them).
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

/// **THE C-9 INVARIANT — Issues "My Work" rows ⊆ the unfiltered inbox (a view, not a store).**
#[test]
fn issues_my_work_is_a_strict_subset_of_the_one_inbox() {
    let inbox = seeded_inbox();
    let auth = AllowAllAuthorize;
    let big_page = Page {
        after: None,
        limit: 1000,
    };

    // the canonical unfiltered ONE inbox (filter = ∅).
    let all = list_inbox(&inbox, &me(), &InboxFilter::all(), &big_page, &auth, &at());
    let all_ids: BTreeSet<String> = all.items.iter().map(|i| i.item_id.clone()).collect();

    // Issues "My Work" — the §1.3 filtered view.
    let my_work = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::issues_my_work(),
        &big_page,
        &auth,
        &at(),
    );
    let my_work_ids: BTreeSet<String> = my_work.items.iter().map(|i| i.item_id.clone()).collect();

    // (1) SUBSET — every "My Work" row is in the unfiltered inbox (it adds NO row).
    assert!(
        my_work_ids.is_subset(&all_ids),
        "C-9: My Work rows ⊆ list_inbox(filter=∅)"
    );
    // (2) STRICT — the unfiltered inbox has rows My Work excludes (it is a real narrowing, not a copy).
    assert!(
        my_work_ids.len() < all_ids.len(),
        "My Work is a STRICT subset (the non-My-Work + other-subsystem rows are excluded)"
    );

    // (3) it is exactly the Issues × My-Work-reason rows — every returned row is an Issues subject
    // with a My-Work reason; the Fyi-issue + Git + Chat rows are absent.
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

    // (4) the My Work view DID return the registered-reason rows (it is not vacuously empty).
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

/// **ONE read-state truth (the whole point of C-9): a row read through "My Work" is the SAME row as
/// the unfiltered inbox.** The view shares the item id with the canonical inbox — there is no second
/// store with a divergent read-state. (Mark-once-consistent-everywhere, recon §5.)
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

    // the assigned row appears in BOTH views with the SAME item_id (one row, one read-state column).
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
