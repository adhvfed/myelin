//! # Read-state — `mark` / `snooze` / `mark_all_read` (the ONE read-state truth) (NOTIF-P6 / P-184, M2)
//!
//! **Owning architecture doc:** `notifications.md` §2.1 (ONE read-state store — the `state` column
//! is the SAME row across every view; `snooze_until` records the snooze until), §1.3 (the C-9
//! read-state truth: read it in a scoped view, it is read in the unified inbox — there is ONE store,
//! one read-state, never a second store to keep in sync). **Contract:** **7.2**
//! `mark / snooze / mark_all_read` (owned, the one read-state truth). **External insight:**
//! `01-process-and-quality-doctrine.md` §4 (chain mutations — mark-read in one view, assert read in
//! another).
//!
//! ## What this prompt (NOTIF-P6) ships — read-state, nothing else
//!
//! 1. **[`mark`]`(item_id, state)`** — flip the ONE read-state column on the row. Because the
//!    projection holds ONE row per item (keyed `(tenant, recipient, dedup_key)`, addressed by the
//!    opaque `item_id` 7.2 handle), the new state is visible in the unified inbox AND in every scoped
//!    view at once — there is no second store. A principal may only flip the state of their OWN items
//!    (the mutate is recipient-scoped); a not-for-me item or a missing id flips NOTHING.
//! 2. **[`snooze`]`(item_id, until)`** — set the row to [`ReadState::Snoozed`] and RECORD the
//!    `snooze_until`. A snoozed item is **suppressed from the active inbox** until its `until` (the
//!    active-inbox view excludes the [terminal/parked][`ReadState::is_active`] states). The until is
//!    persisted; the durable re-surface TIMER is the named floor below.
//! 3. **[`mark_all_read`]`(filter)`** — flip exactly the rows the C-9 [`InboxFilter`] selects (for
//!    THIS principal) to [`ReadState::Read`]. Same filter grammar as `list_inbox` (NOTIF-P5), so
//!    "mark everything in My Work read" flips precisely the My Work rows — and, because it is the one
//!    store, those rows read in the unified inbox too.
//!
//! ## The active-inbox suppression (the snooze semantics)
//!
//! [`ReadState::is_active`] is the predicate the active inbox uses: `unread`/`seen`/`read` are
//! ACTIVE (they show in the inbox — `read` items stay visible until archived, the inbox is not a
//! "mark read = vanish" surface); `snoozed`/`archived`/`done` are PARKED (suppressed from the active
//! inbox). [`active_inbox`] wraps [`list_inbox`](crate::list_inbox::list_inbox) and drops the parked
//! rows — so `snooze(item, until)` makes the item absent from the active inbox (the §2.1 snooze
//! semantic) while it stays in the ONE store (it is not deleted; it re-surfaces on its timer).
//!
//! ## FLOOR named (the durable re-surface timer)
//!
//! `snooze` RECORDS the `snooze_until` and parks the item; the **durable re-surface TIMER** that
//! flips a due snooze back to `unread` (so it re-enters the active inbox at its `until`) is the
//! `myelin-flow` durable timer wheel — **NOTIF-P14 / NOTIF-P18** (the same minute-bucket wheel that
//! serves SLA + escalation timers — one substrate, three uses). Here only the until is recorded; the
//! wheel is NOT wired. Named so this read-state slice is not mistaken for the re-surfacing snooze.
//!
//! ## Mutation floor (the read-state module — mandatory-core)
//!
//! Read-state is mandatory-core (the C-9 one-read-state truth). The mutation-tested core is the
//! decision logic: the recipient-scoped row targeting (a principal flips only their OWN items), the
//! state transition (`mark` sets exactly the requested state; `snooze` sets `Snoozed` AND records
//! the until), the `mark_all_read` filter-selection (it flips EXACTLY the filtered rows, no more no
//! less), and the [`ReadState::is_active`] active/parked partition (a snoozed item is suppressed).
//! **Floor: ≥ 80% line/branch mutation score on `read_state.rs`** (measured with `cargo mutants`;
//! reported in the P-184 commit body). The floor is stated and met by the unit + chained + CDC
//! tests: every transition is asserted, mark_all_read is asserted to hit exactly the filtered rows,
//! a not-for-me mutation is asserted to flip nothing, and the snooze suppression is asserted.
//!
//! **Measured (P-184):** `cargo mutants --file crates/myelin-notif/src/read_state.rs` → 22 mutants,
//! **20 caught / 0 missed / 2 unviable** = **100% on the 20 viable** (≥ 80% floor MET).

use myelin_identity::Principal;

use crate::list_inbox::InboxFilter;
use crate::router::{InboxProjection, RoutedInboxItem};

/// **The ONE read-state column's value space (architecture §2.1).** The `state` column is the SAME
/// row across every view (the C-9 read-state truth). `mark`/`snooze`/`mark_all_read` flip it. The
/// six frozen states partition into ACTIVE (show in the inbox) and PARKED (suppressed from the
/// active inbox) by [`ReadState::is_active`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadState {
    /// Never seen — the fresh state the router UPSERTs (the `+N more` collapse keeps it unread).
    Unread,
    /// Surfaced to the recipient but not acted on (the inbox showed it). ACTIVE.
    Seen,
    /// Marked read by the recipient. ACTIVE — a read item stays in the inbox until archived (the
    /// inbox is not a "mark read → vanish" surface; reading is not archiving).
    Read,
    /// Snoozed until a recorded `snooze_until` — PARKED (suppressed from the active inbox) until the
    /// re-surface timer (NOTIF-P14/P18) flips it back to `unread`.
    Snoozed,
    /// Archived out of the active inbox (kept for history/audit) — PARKED.
    Archived,
    /// Done / dismissed — PARKED.
    Done,
}

impl ReadState {
    /// The persisted lowercase token (the `state` column value; the CLI/wire token). Stable: a
    /// rename breaks the round-trip test (the column is a contract).
    pub fn token(self) -> &'static str {
        match self {
            ReadState::Unread => "unread",
            ReadState::Seen => "seen",
            ReadState::Read => "read",
            ReadState::Snoozed => "snoozed",
            ReadState::Archived => "archived",
            ReadState::Done => "done",
        }
    }

    /// Parse a persisted token back into a [`ReadState`]. An unknown token is `None` (a malformed
    /// state never silently becomes a valid one — the column is a closed vocabulary).
    pub fn parse(token: &str) -> Option<ReadState> {
        match token {
            "unread" => Some(ReadState::Unread),
            "seen" => Some(ReadState::Seen),
            "read" => Some(ReadState::Read),
            "snoozed" => Some(ReadState::Snoozed),
            "archived" => Some(ReadState::Archived),
            "done" => Some(ReadState::Done),
            _ => None,
        }
    }

    /// **Is this state ACTIVE (shown in the inbox) or PARKED (suppressed)?** `unread`/`seen`/`read`
    /// are ACTIVE; `snoozed`/`archived`/`done` are PARKED. This is the load-bearing snooze-semantic
    /// predicate: a snoozed item is suppressed from the active inbox (§2.1) — it is not deleted, it
    /// re-surfaces on its timer (NOTIF-P14/P18).
    pub fn is_active(self) -> bool {
        match self {
            ReadState::Unread | ReadState::Seen | ReadState::Read => true,
            ReadState::Snoozed | ReadState::Archived | ReadState::Done => false,
        }
    }
}

/// Why a read-state mutation did not apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadStateError {
    /// No row addressed to the calling principal has this `item_id` (a missing item, or an item that
    /// belongs to ANOTHER principal — a principal can only flip the read-state of their OWN items).
    /// Held, not leaked: the error does NOT distinguish "missing" from "not yours" (so it never
    /// confirms the existence of another principal's item).
    NotFound,
}

/// **`mark(item_id, state)` — flip the ONE read-state column (contract 7.2).** Sets the `state` of
/// the calling `principal`'s item `item_id` to `state` (clearing `snooze_until` unless the new state
/// is `Snoozed`). Because there is ONE store / ONE row per item, the new state is read in the
/// unified inbox AND in every scoped view at once (the C-9 read-state truth — no second store to
/// keep in sync). A row not addressed to `principal`, or a missing `item_id`, mutates NOTHING and
/// returns [`ReadStateError::NotFound`] (a principal can only mark their OWN items).
pub fn mark(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
    state: ReadState,
) -> Result<(), ReadStateError> {
    let recipient = principal.principal_id.0.as_str();
    let found = inbox.mutate_state(&principal.tenant, recipient, item_id, |row| {
        row.state = state.token().to_string();
        // Clearing the snooze handle on any non-snooze transition keeps the row consistent: a
        // marked-read item is not still "snoozed until X". `snooze` (below) sets it explicitly.
        if state != ReadState::Snoozed {
            row.snooze_until = None;
        }
    });
    if found {
        Ok(())
    } else {
        Err(ReadStateError::NotFound)
    }
}

/// **`snooze(item_id, until)` — park the item until `until` and RECORD the until (contract 7.2).**
/// Sets the calling `principal`'s item `item_id` to [`ReadState::Snoozed`] and records
/// `snooze_until = Some(until)`. The item is then SUPPRESSED from the active inbox ([`active_inbox`]
/// drops parked rows) until its `until` — it is NOT deleted; it stays in the ONE store and
/// re-surfaces on its timer.
///
/// **FLOOR (named):** the durable re-surface TIMER that flips a due snooze back to `unread` is the
/// `myelin-flow` durable timer wheel — **NOTIF-P14 / NOTIF-P18**. Here ONLY the `until` is recorded
/// and the item parked; the wheel is not wired. A row not addressed to `principal`, or a missing
/// `item_id`, snoozes NOTHING (returns [`ReadStateError::NotFound`]).
pub fn snooze(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
    until: &str,
) -> Result<(), ReadStateError> {
    let recipient = principal.principal_id.0.as_str();
    let found = inbox.mutate_state(&principal.tenant, recipient, item_id, |row| {
        row.state = ReadState::Snoozed.token().to_string();
        row.snooze_until = Some(until.to_string());
    });
    if found {
        Ok(())
    } else {
        Err(ReadStateError::NotFound)
    }
}

/// **`mark_all_read(filter)` — flip exactly the C-9-filtered rows to `read` (contract 7.2).** Marks
/// EVERY active item addressed to `principal` that passes `filter` (the same `subsystem ∧ reason`
/// grammar as `list_inbox`, NOTIF-P5) as [`ReadState::Read`]. So "mark My Work read" flips precisely
/// the My Work rows — and, because it is the ONE store, those rows read in the unified inbox too.
///
/// Only ALREADY-active rows are flipped (an already-snoozed/archived/done row is left parked — you
/// do not un-snooze by marking-all-read). Returns the count flipped. It is recipient-scoped — it
/// NEVER touches another principal's inbox.
pub fn mark_all_read(inbox: &InboxProjection, principal: &Principal, filter: &InboxFilter) -> usize {
    let recipient = principal.principal_id.0.as_str();
    inbox.mutate_matching(
        &principal.tenant,
        recipient,
        |row: &RoutedInboxItem| {
            // Select exactly the filtered, currently-ACTIVE rows (a parked row stays parked — marking
            // all read does not resurrect a snoozed item). The filter is the C-9 grammar.
            filter.matches(row) && row_is_active(row)
        },
        |row| {
            row.state = ReadState::Read.token().to_string();
            row.snooze_until = None;
        },
    )
}

/// `true` iff a row's persisted `state` token is an ACTIVE state (a parse failure is treated as
/// active — an unknown token is shown, never silently suppressed; the closed vocabulary is enforced
/// at write time by [`ReadState`]).
fn row_is_active(row: &RoutedInboxItem) -> bool {
    ReadState::parse(&row.state).map(ReadState::is_active).unwrap_or(true)
}

/// **`active_inbox` — the active-inbox view that SUPPRESSES parked (snoozed/archived/done) rows.**
/// A thin filter over a page of [`list_inbox`](crate::list_inbox::list_inbox) results: it keeps only
/// rows whose `state` is ACTIVE ([`ReadState::is_active`]). This is the §2.1 snooze semantic — a
/// snoozed item is absent from the active inbox while it stays in the ONE store. The caller runs
/// `list_inbox` (recipient-scoped, filtered, authorized) and passes the rows here.
pub fn active_inbox(items: Vec<RoutedInboxItem>) -> Vec<RoutedInboxItem> {
    items.into_iter().filter(row_is_active).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_inbox::{list_inbox, AllowAllAuthorize, Page};
    use crate::Reason;
    use myelin_events::ArtifactRef;
    use myelin_identity::{Consistency, ConsistencyMode, PrincipalId, PrincipalKind, Zookie};
    use myelin_tenancy::{Region, TenantId};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn principal(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn strong() -> Consistency {
        Consistency { at_least: Zookie("zk".into()), mode: ConsistencyMode::Strong }
    }

    fn item(recipient: &str, item_id: &str, subject: &str, reason: Reason) -> RoutedInboxItem {
        RoutedInboxItem {
            tenant: tenant(),
            region: Region("fr-par".into()),
            item_id: item_id.into(),
            recipient: recipient.into(),
            subject: ArtifactRef(subject.into()),
            reason,
            class: crate::Class::Direct,
            origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
            dedup_key: item_id.into(),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
        }
    }

    /// Seed `me`'s inbox: an Issues "My Work" row, a Chat "Activity" row, a Git "Review" row, and an
    /// issues row NOT in My Work — across the three subsystems.
    fn seeded(me: &str) -> InboxProjection {
        let inbox = InboxProjection::new();
        inbox.upsert_for_test(item(me, "iss-assigned", "myelin://acme/issue/issue/PROJ-1", Reason::Assigned));
        inbox.upsert_for_test(item(me, "iss-state", "myelin://acme/issue/issue/PROJ-2", Reason::StateChanged));
        inbox.upsert_for_test(item(me, "chat-ment", "myelin://acme/chat/thread/T1", Reason::Mentioned));
        inbox.upsert_for_test(item(me, "git-review", "myelin://acme/git/pr/9", Reason::ReviewRequested));
        inbox
    }

    fn state_of(inbox: &InboxProjection, recipient: &str, item_id: &str) -> Option<String> {
        inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .find(|r| r.recipient == recipient && r.item_id == item_id)
            .map(|r| r.state)
    }

    // --- ReadState vocabulary round-trip + the active/parked partition ---

    /// **The read-state token round-trips and the active/parked partition is the frozen one.** A
    /// mutant that mis-maps a token or flips a state's active-ness is caught.
    #[test]
    fn read_state_tokens_round_trip_and_active_partition_is_frozen() {
        for s in [
            ReadState::Unread,
            ReadState::Seen,
            ReadState::Read,
            ReadState::Snoozed,
            ReadState::Archived,
            ReadState::Done,
        ] {
            assert_eq!(ReadState::parse(s.token()), Some(s), "{s:?} round-trips through its token");
        }
        assert_eq!(ReadState::parse("bogus"), None, "an unknown token is None (closed vocabulary)");
        // ACTIVE: unread/seen/read show in the inbox.
        assert!(ReadState::Unread.is_active());
        assert!(ReadState::Seen.is_active());
        assert!(ReadState::Read.is_active(), "a read item stays in the inbox (read ≠ archived)");
        // PARKED: snoozed/archived/done are suppressed.
        assert!(!ReadState::Snoozed.is_active(), "a snoozed item is suppressed from the active inbox");
        assert!(!ReadState::Archived.is_active());
        assert!(!ReadState::Done.is_active());
    }

    // --- mark: the ONE read-state column, the SAME row across every view ---

    /// **`mark` flips the ONE read-state column — and it is read in EVERY view (the C-9 truth).**
    /// Mark the assigned issue read; it is `read` whether read through the unified inbox OR the My
    /// Work scoped view — the SAME row, one store. (EI-01 §4: mark in one view, assert in another.)
    #[test]
    fn mark_read_is_one_read_state_truth_across_views() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);

        mark(&inbox, &p, "iss-assigned", ReadState::Read).expect("mark my own item");

        // read through the unified inbox.
        let full = list_inbox(&inbox, &p, &InboxFilter::all(), &Page { after: None, limit: 100 }, &AllowAllAuthorize, &strong());
        let in_full = full.items.iter().find(|r| r.item_id == "iss-assigned").unwrap();
        assert_eq!(in_full.state, "read", "the item is read in the unified inbox");

        // read through the My Work SCOPED view — the SAME row, same read-state (no second store).
        let view = list_inbox(&inbox, &p, &InboxFilter::issues_my_work(), &Page { after: None, limit: 100 }, &AllowAllAuthorize, &strong());
        let in_view = view.items.iter().find(|r| r.item_id == "iss-assigned").unwrap();
        assert_eq!(in_view.state, "read", "the SAME row reads `read` in the scoped view too (C-9)");

        // a DIFFERENT row was untouched (mark targets exactly one item).
        assert_eq!(state_of(&inbox, me, "chat-ment").unwrap(), "unread", "other rows untouched");
    }

    /// **`mark` is recipient-scoped: a principal can only flip the state of their OWN items.** u2
    /// cannot mark u1's item (NotFound, nothing mutates); a missing id is NotFound. Held, not leaked:
    /// the NotFound does not distinguish "missing" from "not yours".
    #[test]
    fn mark_is_recipient_scoped_cannot_touch_anothers_item() {
        let inbox = seeded("u1");
        // u2 tries to mark u1's item — refused, and u1's row is UNCHANGED.
        assert_eq!(mark(&inbox, &principal("u2"), "iss-assigned", ReadState::Read), Err(ReadStateError::NotFound));
        assert_eq!(state_of(&inbox, "u1", "iss-assigned").unwrap(), "unread", "another principal did NOT flip my item");
        // a missing id is NotFound too (no leak of existence).
        assert_eq!(mark(&inbox, &principal("u1"), "no-such", ReadState::Read), Err(ReadStateError::NotFound));
    }

    // --- snooze: records the until + suppresses from the active inbox ---

    /// **`snooze(item, until)` records the until and SUPPRESSES the item from the active inbox.** The
    /// snoozed item is `snoozed` with `snooze_until = Some(until)`; the active inbox view (which
    /// drops parked rows) no longer contains it — while it STAYS in the ONE store (it is not
    /// deleted). This is the §2.1 snooze semantic; the durable re-surface timer is the named floor.
    #[test]
    fn snooze_records_until_and_suppresses_from_active_inbox() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);
        let until = "2026-06-25T09:00:00Z";

        snooze(&inbox, &p, "iss-assigned", until).expect("snooze my own item");

        // the until is recorded + the state is snoozed (persisted).
        let row = inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .find(|r| r.item_id == "iss-assigned")
            .unwrap();
        assert_eq!(row.state, "snoozed");
        assert_eq!(row.snooze_until.as_deref(), Some(until), "the until is persisted");

        // suppressed from the ACTIVE inbox (but still in the ONE store).
        let full = list_inbox(&inbox, &p, &InboxFilter::all(), &Page { after: None, limit: 100 }, &AllowAllAuthorize, &strong());
        assert!(full.items.iter().any(|r| r.item_id == "iss-assigned"), "the snoozed item is STILL in the store (not deleted)");
        let active = active_inbox(full.items);
        assert!(!active.iter().any(|r| r.item_id == "iss-assigned"), "the snoozed item is ABSENT from the active inbox (§2.1)");
        // the other rows still show in the active inbox.
        assert!(active.iter().any(|r| r.item_id == "chat-ment"), "an un-snoozed item still shows");
    }

    /// **`mark` to a non-snooze state CLEARS a previously-recorded snooze_until.** Snooze then mark
    /// read → the until is cleared (the item is not still "snoozed until X").
    #[test]
    fn mark_clears_a_previously_recorded_snooze_until() {
        let inbox = seeded("u1");
        let p = principal("u1");
        snooze(&inbox, &p, "iss-assigned", "2026-06-25T09:00:00Z").unwrap();
        mark(&inbox, &p, "iss-assigned", ReadState::Read).unwrap();
        let row = inbox.snapshot_for_tenant(&tenant()).into_iter().find(|r| r.item_id == "iss-assigned").unwrap();
        assert_eq!(row.state, "read");
        assert!(row.snooze_until.is_none(), "marking read cleared the stale snooze_until");
    }

    // --- mark_all_read(filter): flips EXACTLY the filtered rows ---

    /// **`mark_all_read(filter)` flips EXACTLY the filtered rows — no more, no less.** Mark My Work
    /// read: the assigned issue flips to `read`; the chat mention, the git review (other subsystems),
    /// and the non-My-Work issue (`state_changed`) are UNTOUCHED. A mutant that widens the filter
    /// (hits rows outside it) or narrows it (misses a filtered row) is caught.
    #[test]
    fn mark_all_read_flips_exactly_the_filtered_rows() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);

        let n = mark_all_read(&inbox, &p, &InboxFilter::issues_my_work());
        assert_eq!(n, 1, "exactly the one My Work row was flipped");
        assert_eq!(state_of(&inbox, me, "iss-assigned").unwrap(), "read", "the My Work row is read");
        // everything NOT in My Work is untouched.
        assert_eq!(state_of(&inbox, me, "iss-state").unwrap(), "unread", "a non-My-Work issue is untouched (reason clause)");
        assert_eq!(state_of(&inbox, me, "chat-ment").unwrap(), "unread", "a chat row is untouched (subsystem clause)");
        assert_eq!(state_of(&inbox, me, "git-review").unwrap(), "unread", "a git row is untouched");
    }

    /// **`mark_all_read(all)` flips the whole ACTIVE inbox; a snoozed (parked) row is left parked.**
    /// Snooze one row, then mark-all-read the unfiltered inbox: the three active rows flip to `read`,
    /// the snoozed row STAYS snoozed (marking all read does not un-snooze).
    #[test]
    fn mark_all_read_all_leaves_a_parked_snoozed_row_parked() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);
        snooze(&inbox, &p, "git-review", "2026-06-25T09:00:00Z").unwrap();

        let n = mark_all_read(&inbox, &p, &InboxFilter::all());
        assert_eq!(n, 3, "the three ACTIVE rows flipped (the snoozed one was skipped)");
        assert_eq!(state_of(&inbox, me, "git-review").unwrap(), "snoozed", "mark_all_read did NOT un-snooze the parked row");
        assert_eq!(state_of(&inbox, me, "iss-assigned").unwrap(), "read");
    }

    /// **`mark_all_read` is recipient-scoped: it NEVER touches another principal's inbox.** u1 and u2
    /// both have a My Work row; u1's mark_all_read flips only u1's row.
    #[test]
    fn mark_all_read_is_recipient_scoped() {
        let inbox = InboxProjection::new();
        inbox.upsert_for_test(item("u1", "u1-iss", "myelin://acme/issue/issue/P1", Reason::Assigned));
        inbox.upsert_for_test(item("u2", "u2-iss", "myelin://acme/issue/issue/P2", Reason::Assigned));
        let n = mark_all_read(&inbox, &principal("u1"), &InboxFilter::issues_my_work());
        assert_eq!(n, 1, "only u1's row flipped");
        assert_eq!(state_of(&inbox, "u1", "u1-iss").unwrap(), "read");
        assert_eq!(state_of(&inbox, "u2", "u2-iss").unwrap(), "unread", "u2's inbox was NEVER touched");
    }

    /// **THE CHAINED PROPERTY (EI-01 §4): ingest a batch → mark_all_read(My Work view) → re-list both
    /// the view AND the full inbox → the read-state is CONSISTENT across views.** The flipped rows
    /// read `read` in the scoped view AND in the unified inbox (one store, one read-state truth — 0
    /// divergence); the non-flipped rows read `unread` in both.
    #[test]
    fn chained_mark_all_read_then_relist_is_consistent_across_views() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);

        // mark_all_read in the My Work VIEW.
        mark_all_read(&inbox, &p, &InboxFilter::issues_my_work());

        // re-list the My Work view: its row reads `read`.
        let view = list_inbox(&inbox, &p, &InboxFilter::issues_my_work(), &Page { after: None, limit: 100 }, &AllowAllAuthorize, &strong());
        for r in &view.items {
            assert_eq!(r.state, "read", "every My Work row reads `read` after mark_all_read");
        }

        // re-list the FULL inbox: the SAME row reads `read` (no divergence); the others read `unread`.
        let full = list_inbox(&inbox, &p, &InboxFilter::all(), &Page { after: None, limit: 100 }, &AllowAllAuthorize, &strong());
        let assigned = full.items.iter().find(|r| r.item_id == "iss-assigned").unwrap();
        assert_eq!(assigned.state, "read", "the My Work row reads `read` in the unified inbox too (0 divergence)");
        let chat = full.items.iter().find(|r| r.item_id == "chat-ment").unwrap();
        assert_eq!(chat.state, "unread", "a row outside the marked view stays unread in every view");
    }

    /// **Every [`ReadState`] token is a value the real `notif_inbox_item.state` CHECK constraint
    /// admits (the in-memory read-state vocabulary is byte-aligned with the live Postgres column).**
    /// A drift — a new in-memory state the DDL CHECK would reject, or a renamed token — breaks THIS
    /// build (DB-free: it reads the DDL string constant, proven against live Postgres at NOTIF-P2).
    #[test]
    fn read_state_tokens_match_the_inbox_item_ddl_check() {
        let ddl = crate::migrations::INBOX_ITEM_DDL;
        for s in [
            ReadState::Unread,
            ReadState::Seen,
            ReadState::Read,
            ReadState::Snoozed,
            ReadState::Archived,
            ReadState::Done,
        ] {
            assert!(
                ddl.contains(&format!("'{}'", s.token())),
                "the state token {:?} ({}) must be in the notif_inbox_item.state CHECK constraint",
                s,
                s.token()
            );
        }
    }

    /// **`active_inbox` keeps active rows and drops parked rows.** A direct unit over the predicate
    /// the active inbox uses — a mutant that flips the active/parked partition is caught here.
    #[test]
    fn active_inbox_keeps_active_drops_parked() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);
        mark(&inbox, &p, "iss-assigned", ReadState::Read).unwrap(); // ACTIVE
        snooze(&inbox, &p, "chat-ment", "2026-06-25T09:00:00Z").unwrap(); // PARKED
        mark(&inbox, &p, "git-review", ReadState::Done).unwrap(); // PARKED

        let all = inbox.snapshot_for_tenant(&tenant());
        let active = active_inbox(all);
        let ids: std::collections::BTreeSet<_> = active.iter().map(|r| r.item_id.clone()).collect();
        assert!(ids.contains("iss-assigned"), "a read item is still active");
        assert!(ids.contains("iss-state"), "an unread item is active");
        assert!(!ids.contains("chat-ment"), "a snoozed item is parked");
        assert!(!ids.contains("git-review"), "a done item is parked");
    }
}
