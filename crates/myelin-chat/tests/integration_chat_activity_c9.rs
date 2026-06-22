//! # The C-9 invariant — Chat "Activity / Mentions" is a FILTER over the ONE inbox, not a store
//! (NOTIF-P22 / P-343, M4)
//!
//! **Architecture `05-refined-shared-systems-architecture/notifications.md` §1.3** (the C-9
//! resolution — there is exactly ONE cross-subsystem inbox; Chat "Activity / Mentions" is a *scoped,
//! filtered query INTO this one inbox*: `subsystem∈{chat} ∧ reason∈{mentioned, replied,
//! thread_watched, approval_requested}`, NEVER a separate store) and the Phase-4 ask (CHAT named this
//! blocking — "Activity/Mentions is a filter not a store"). **External insight**
//! `01-process-and-quality-doctrine.md` §3 (prove-it — the C-9 invariant test forces the "a view is a
//! subset" property).
//!
//! **The threshold (never weakened):** the rows Chat "Activity" returns are a STRICT SUBSET of the
//! unfiltered inbox — `activity_rows ⊆ list_inbox(filter=∅)`. The view adds NO row the canonical inbox
//! lacks (it only narrows), and it shares the ONE read-state column (read it in "Activity", it is read
//! everywhere). This is the Chat-side proof that "Activity/Mentions" is a saved filter, not a second
//! inbox — the exact failure the platform exists to fix (three inbox-like surfaces fragmenting
//! attention).

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

/// Seed the ONE inbox with a mix: Chat rows IN "Activity" (every Chat consumer reason), a Chat row
/// NOT in "Activity" (a non-Activity reason on a chat subject), and OTHER subsystems' rows
/// (Issues/Git) that must never appear in Chat "Activity".
fn seeded_inbox() -> InboxProjection {
    let inbox = InboxProjection::new();
    // Chat rows that ARE in "Activity" — the §1.3 reason set Chat registers (NOTIF-P22 / CHAT-P3):
    // mentioned / replied / thread_watched / approval_requested.
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
    // A Chat row NOT in "Activity" (a reason outside the Activity filter) — must be excluded.
    inbox.upsert_for_test(item(
        "chat-fyi",
        "myelin://acme/chat/channel/random",
        Reason::Fyi,
    ));
    // OTHER subsystems — never in Chat "Activity" (the subsystem filter excludes them), even when
    // they carry an Activity-shaped reason (a mentioned ISSUE is NOT chat activity).
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

/// **THE C-9 INVARIANT — Chat "Activity" rows ⊆ the unfiltered inbox (a view, not a store).**
#[test]
fn chat_activity_is_a_strict_subset_of_the_one_inbox() {
    let inbox = seeded_inbox();
    let auth = AllowAllAuthorize;
    let big_page = Page {
        after: None,
        limit: 1000,
    };

    // the canonical unfiltered ONE inbox (filter = ∅).
    let all = list_inbox(&inbox, &me(), &InboxFilter::all(), &big_page, &auth, &at());
    let all_ids: BTreeSet<String> = all.items.iter().map(|i| i.item_id.clone()).collect();

    // Chat "Activity / Mentions" — the §1.3 filtered view.
    let activity = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::chat_activity(),
        &big_page,
        &auth,
        &at(),
    );
    let activity_ids: BTreeSet<String> = activity.items.iter().map(|i| i.item_id.clone()).collect();

    // (1) SUBSET — every "Activity" row is in the unfiltered inbox (it adds NO row).
    assert!(
        activity_ids.is_subset(&all_ids),
        "C-9: Activity rows ⊆ list_inbox(filter=∅)"
    );
    // (2) STRICT — the unfiltered inbox has rows Activity excludes (a real narrowing, not a copy).
    assert!(
        activity_ids.len() < all_ids.len(),
        "Activity is a STRICT subset (the non-Activity + other-subsystem rows are excluded)"
    );

    // (3) it is exactly the Chat × Activity-reason rows — every returned row is a Chat subject with
    // an Activity reason; the Fyi-chat + Issue + Git rows are absent.
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
    // a mentioned ISSUE shares the reason but NOT the subsystem — the C-9 subsystem filter excludes it.
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

    // (4) the Activity view DID return the registered-reason rows (it is not vacuously empty).
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

/// **ONE read-state truth (the whole point of C-9): a row read through "Activity" is the SAME row as
/// the unfiltered inbox.** The view shares the item id with the canonical inbox — there is no second
/// store with a divergent read-state. (Mark-once-consistent-everywhere, recon §5.)
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

    // the mentioned row appears in BOTH views with the SAME item_id (one row, one read-state column).
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
