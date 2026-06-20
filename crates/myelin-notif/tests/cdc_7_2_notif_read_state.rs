//! # CDC — contract 7.2 `mark / snooze / mark_all_read` (the ONE read-state truth) (P-184)
//!
//! **Architecture:** `notifications.md` §2.1 (ONE read-state store — the `state` column is the SAME
//! row across every view; `snooze_until` records the snooze until), §1.3 (the C-9 read-state truth —
//! read it in a scoped view, it is read in the unified inbox). **Contract:** **7.2**
//! `mark / snooze / mark_all_read`.
//!
//! This CDC pins the 7.2 seam from BOTH sides:
//!
//! - **PROVIDER (Notif owns 7.2):** `mark`/`snooze`/`mark_all_read` flip the ONE read-state column;
//!   the flip is visible in the unified inbox AND in every scoped view at once (one store, one
//!   read-state — 0 divergence). `snooze` records the until + suppresses the item from the active
//!   inbox. `mark_all_read(filter)` flips EXACTLY the filtered rows.
//! - **CONSUMER (the inbox UI / the CLI):** marks read in one view and reads it back in another (the
//!   one-read-state-truth wire), and snoozes through the CLI seam (the item leaves the active inbox).
//!
//! The two halves agree on the WIRE: the read-state column is ONE row (no second store), the snooze
//! records the until + parks the item, and mark_all_read takes the SAME C-9 `InboxFilter` grammar as
//! `list_inbox` (7.1). A drift on either side (a second read-state store, a snooze that does not
//! suppress, a mark_all_read that hits unfiltered rows) breaks THIS build.

use myelin_events::ArtifactRef;
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::cli::{inbox_read, inbox_snooze, CliView};
use myelin_notif::list_inbox::{list_inbox, AllowAllAuthorize, InboxFilter, Page};
use myelin_notif::read_state::{active_inbox, mark, mark_all_read, snooze, ReadState, ReadStateError};
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
    Consistency { at_least: Zookie("zk-7.2".into()), mode: ConsistencyMode::Strong }
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

/// A mixed batch for `me`: an Issues "My Work" row, a non-My-Work issue, a Chat "Activity" row, a
/// Git "Review" row.
fn seeded(me: &str) -> InboxProjection {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item(me, "iss-assigned", "myelin://acme/issue/issue/PROJ-1", Reason::Assigned));
    inbox.upsert_for_test(item(me, "iss-state", "myelin://acme/issue/issue/PROJ-2", Reason::StateChanged));
    inbox.upsert_for_test(item(me, "chat-ment", "myelin://acme/chat/thread/T1", Reason::Mentioned));
    inbox.upsert_for_test(item(me, "git-review", "myelin://acme/git/pr/9", Reason::ReviewRequested));
    inbox
}

fn state_in_view(inbox: &InboxProjection, p: &Principal, filter: &InboxFilter, item_id: &str) -> Option<String> {
    let page = list_inbox(inbox, p, filter, &Page { after: None, limit: 1000 }, &AllowAllAuthorize, &strong());
    page.items.into_iter().find(|r| r.item_id == item_id).map(|r| r.state)
}

/// **PROVIDER + CONSUMER (the one-read-state truth): mark an item read in a SCOPED view → it is read
/// in the unified inbox (and vice versa) — the `state` column is the SAME row.** Threshold: 1 state
/// per item across all views; 0 divergence.
#[test]
fn read_state_is_one_truth_across_views_zero_divergence() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);

    // mark read (the CONSUMER side: a view marks it).
    mark(&inbox, &p, "iss-assigned", ReadState::Read).expect("mark my own item");

    // it reads `read` in the scoped My Work view AND in the unified inbox — the SAME row (0 divergence).
    assert_eq!(state_in_view(&inbox, &p, &InboxFilter::issues_my_work(), "iss-assigned").as_deref(), Some("read"), "read in the scoped view");
    assert_eq!(state_in_view(&inbox, &p, &InboxFilter::all(), "iss-assigned").as_deref(), Some("read"), "read in the unified inbox (same row)");

    // the inverse: mark a chat item read through the unified inbox → read in the Chat Activity view.
    mark(&inbox, &p, "chat-ment", ReadState::Read).unwrap();
    assert_eq!(state_in_view(&inbox, &p, &InboxFilter::chat_activity(), "chat-ment").as_deref(), Some("read"), "read in the Chat view (same row)");
}

/// **PROVIDER (the snooze-state test): `snooze(item, until)` → the item is suppressed from the
/// active inbox; the until is recorded.** Threshold: snoozed item absent from the active inbox; until
/// persisted.
#[test]
fn snooze_suppresses_from_active_inbox_and_records_until() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);
    let until = "2026-06-25T09:00:00Z";

    snooze(&inbox, &p, "git-review", until).expect("snooze my own item");

    // the until is persisted on the SAME row.
    let row = inbox.snapshot_for_tenant(&tenant()).into_iter().find(|r| r.item_id == "git-review").unwrap();
    assert_eq!(row.state, "snoozed");
    assert_eq!(row.snooze_until.as_deref(), Some(until), "the until is persisted (7.2)");

    // ABSENT from the active inbox (but still in the ONE store — not deleted).
    let full = list_inbox(&inbox, &p, &InboxFilter::all(), &Page { after: None, limit: 1000 }, &AllowAllAuthorize, &strong());
    assert!(full.items.iter().any(|r| r.item_id == "git-review"), "still in the store (re-surfaces on its timer)");
    let active = active_inbox(full.items);
    assert!(!active.iter().any(|r| r.item_id == "git-review"), "the snoozed item is ABSENT from the active inbox");
}

/// **PROVIDER (mark_all_read(filter)): flips EXACTLY the filtered rows — across views.** mark_all_read
/// the My Work view flips the assigned issue; the chat/git rows + the non-My-Work issue are
/// untouched. The flipped row reads `read` in the unified inbox too (one store).
#[test]
fn mark_all_read_flips_exactly_the_filtered_rows_across_views() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);

    let n = mark_all_read(&inbox, &p, &InboxFilter::issues_my_work());
    assert_eq!(n, 1, "exactly the one My Work row flipped");

    // read in BOTH the scoped view and the unified inbox.
    assert_eq!(state_in_view(&inbox, &p, &InboxFilter::issues_my_work(), "iss-assigned").as_deref(), Some("read"));
    assert_eq!(state_in_view(&inbox, &p, &InboxFilter::all(), "iss-assigned").as_deref(), Some("read"), "same row, read in the unified inbox");

    // everything outside the filter is untouched.
    assert_eq!(state_in_view(&inbox, &p, &InboxFilter::all(), "chat-ment").as_deref(), Some("unread"));
    assert_eq!(state_in_view(&inbox, &p, &InboxFilter::all(), "git-review").as_deref(), Some("unread"));
    assert_eq!(state_in_view(&inbox, &p, &InboxFilter::all(), "iss-state").as_deref(), Some("unread"));
}

/// **CONSUMER (the CLI seam): `inbox read` / `inbox snooze` drive the SAME read-state path.** The CLI
/// is the consumer; it reads/snoozes through the contract, recipient-scoped (you can only touch your
/// own items).
#[test]
fn cli_read_and_snooze_drive_the_read_state_contract() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);

    // CLI read.
    inbox_read(&inbox, &p, "iss-assigned").expect("read my own item");
    assert_eq!(cli::inbox_show(&inbox, &p, "iss-assigned", &AllowAllAuthorize, &strong()).unwrap().state, "read");

    // CLI snooze — leaves the active inbox.
    inbox_snooze(&inbox, &p, "chat-ment", "2026-06-25T09:00:00Z").expect("snooze my own item");
    let page = cli::inbox_list(&inbox, &p, CliView::All, &Page { after: None, limit: 1000 }, &AllowAllAuthorize, &strong());
    let active = active_inbox(page.items);
    assert!(!active.iter().any(|r| r.item_id == "chat-ment"), "the snoozed item left the active inbox");

    // recipient-scoped: u2 cannot read u1's item.
    assert_eq!(inbox_read(&inbox, &principal("u2"), "iss-assigned"), Err(ReadStateError::NotFound), "you can only read your own items");
}

/// **THE CHAINED PROPERTY (EI-01 §4): ingest a batch → mark_all_read(filter) → re-list both the view
/// AND the full inbox → assert read-state CONSISTENT across views.** The committed contract test the
/// prompt requires.
#[test]
fn chained_ingest_mark_all_read_relist_consistent() {
    let me = "u1";
    let inbox = seeded(me);
    let p = principal(me);

    mark_all_read(&inbox, &p, &InboxFilter::issues_my_work());

    // the view's row reads `read`.
    let view = list_inbox(&inbox, &p, &InboxFilter::issues_my_work(), &Page { after: None, limit: 1000 }, &AllowAllAuthorize, &strong());
    for r in &view.items {
        assert_eq!(r.state, "read", "every My Work row reads `read`");
    }
    // the SAME row reads `read` in the full inbox; rows outside the view stay `unread` (0 divergence).
    let full = list_inbox(&inbox, &p, &InboxFilter::all(), &Page { after: None, limit: 1000 }, &AllowAllAuthorize, &strong());
    let assigned = full.items.iter().find(|r| r.item_id == "iss-assigned").unwrap();
    assert_eq!(assigned.state, "read", "the My Work row is read in the unified inbox too");
    let chat = full.items.iter().find(|r| r.item_id == "chat-ment").unwrap();
    assert_eq!(chat.state, "unread", "a row outside the marked view stays unread in every view");
}
