use myelin_identity::Principal;

use crate::list_inbox::InboxFilter;
use crate::router::{InboxProjection, RoutedInboxItem};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadState {
    Unread,
    Seen,
    Read,
    Snoozed,
    Archived,
    Done,
}

impl ReadState {
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

    pub fn is_active(self) -> bool {
        match self {
            ReadState::Unread | ReadState::Seen | ReadState::Read => true,
            ReadState::Snoozed | ReadState::Archived | ReadState::Done => false,
        }
    }

    /// How urgently this state belongs in front of a person's inbox.
    /// Reason priority orders work *within* one attention state; it must not
    /// let completed critical work bury something new.
    pub fn attention_rank(self) -> u8 {
        match self {
            ReadState::Unread => 3,
            ReadState::Seen => 2,
            ReadState::Read => 1,
            ReadState::Snoozed | ReadState::Archived | ReadState::Done => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadStateError {
    NotFound,
}

pub fn mark(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
    state: ReadState,
) -> Result<(), ReadStateError> {
    let recipient = principal.principal_id.0.as_str();
    let found = inbox.mutate_state(&principal.tenant, recipient, item_id, |row| {
        row.state = state.token().to_string();
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

pub fn mark_all_read(
    inbox: &InboxProjection,
    principal: &Principal,
    filter: &InboxFilter,
) -> usize {
    let recipient = principal.principal_id.0.as_str();
    inbox.mutate_matching(
        &principal.tenant,
        recipient,
        |row: &RoutedInboxItem| filter.matches(row) && row_is_active(row),
        |row| {
            row.state = ReadState::Read.token().to_string();
            row.snooze_until = None;
        },
    )
}

fn row_is_active(row: &RoutedInboxItem) -> bool {
    ReadState::parse(&row.state)
        .map(ReadState::is_active)
        .unwrap_or(true)
}

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
        Consistency {
            at_least: Zookie("zk".into()),
            mode: ConsistencyMode::Strong,
        }
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

    fn state_of(inbox: &InboxProjection, recipient: &str, item_id: &str) -> Option<String> {
        inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .find(|r| r.recipient == recipient && r.item_id == item_id)
            .map(|r| r.state)
    }

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
            assert_eq!(
                ReadState::parse(s.token()),
                Some(s),
                "{s:?} round-trips through its token"
            );
        }
        assert_eq!(
            ReadState::parse("bogus"),
            None,
            "an unknown token is None (closed vocabulary)"
        );
        assert!(ReadState::Unread.is_active());
        assert!(ReadState::Seen.is_active());
        assert!(
            ReadState::Read.is_active(),
            "a read item stays in the inbox (read ≠ archived)"
        );
        assert!(
            !ReadState::Snoozed.is_active(),
            "a snoozed item is suppressed from the active inbox"
        );
        assert!(!ReadState::Archived.is_active());
        assert!(!ReadState::Done.is_active());
        assert_eq!(ReadState::Unread.attention_rank(), 3);
        assert_eq!(ReadState::Seen.attention_rank(), 2);
        assert_eq!(ReadState::Read.attention_rank(), 1);
        for parked in [ReadState::Snoozed, ReadState::Archived, ReadState::Done] {
            assert_eq!(parked.attention_rank(), 0);
        }
    }

    #[test]
    fn mark_read_is_one_read_state_truth_across_views() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);

        mark(&inbox, &p, "iss-assigned", ReadState::Read).expect("mark my own item");

        let full = list_inbox(
            &inbox,
            &p,
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 100,
            },
            &AllowAllAuthorize,
            &strong(),
        );
        let in_full = full
            .items
            .iter()
            .find(|r| r.item_id == "iss-assigned")
            .unwrap();
        assert_eq!(
            in_full.state, "read",
            "the item is read in the unified inbox"
        );

        let view = list_inbox(
            &inbox,
            &p,
            &InboxFilter::issues_my_work(),
            &Page {
                after: None,
                limit: 100,
            },
            &AllowAllAuthorize,
            &strong(),
        );
        let in_view = view
            .items
            .iter()
            .find(|r| r.item_id == "iss-assigned")
            .unwrap();
        assert_eq!(
            in_view.state, "read",
            "the SAME row reads `read` in the scoped view too (C-9)"
        );

        assert_eq!(
            state_of(&inbox, me, "chat-ment").unwrap(),
            "unread",
            "other rows untouched"
        );
    }

    #[test]
    fn mark_is_recipient_scoped_cannot_touch_anothers_item() {
        let inbox = seeded("u1");
        assert_eq!(
            mark(&inbox, &principal("u2"), "iss-assigned", ReadState::Read),
            Err(ReadStateError::NotFound)
        );
        assert_eq!(
            state_of(&inbox, "u1", "iss-assigned").unwrap(),
            "unread",
            "another principal did NOT flip my item"
        );
        assert_eq!(
            mark(&inbox, &principal("u1"), "no-such", ReadState::Read),
            Err(ReadStateError::NotFound)
        );
    }

    #[test]
    fn snooze_records_until_and_suppresses_from_active_inbox() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);
        let until = "2026-06-25T09:00:00Z";

        snooze(&inbox, &p, "iss-assigned", until).expect("snooze my own item");

        let row = inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .find(|r| r.item_id == "iss-assigned")
            .unwrap();
        assert_eq!(row.state, "snoozed");
        assert_eq!(
            row.snooze_until.as_deref(),
            Some(until),
            "the until is persisted"
        );

        let full = list_inbox(
            &inbox,
            &p,
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 100,
            },
            &AllowAllAuthorize,
            &strong(),
        );
        assert!(
            full.items.iter().any(|r| r.item_id == "iss-assigned"),
            "the snoozed item is STILL in the store (not deleted)"
        );
        let active = active_inbox(full.items);
        assert!(
            !active.iter().any(|r| r.item_id == "iss-assigned"),
            "the snoozed item is ABSENT from the active inbox (§2.1)"
        );
        assert!(
            active.iter().any(|r| r.item_id == "chat-ment"),
            "an un-snoozed item still shows"
        );
    }

    #[test]
    fn mark_clears_a_previously_recorded_snooze_until() {
        let inbox = seeded("u1");
        let p = principal("u1");
        snooze(&inbox, &p, "iss-assigned", "2026-06-25T09:00:00Z").unwrap();
        mark(&inbox, &p, "iss-assigned", ReadState::Read).unwrap();
        let row = inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .find(|r| r.item_id == "iss-assigned")
            .unwrap();
        assert_eq!(row.state, "read");
        assert!(
            row.snooze_until.is_none(),
            "marking read cleared the stale snooze_until"
        );
    }

    #[test]
    fn mark_all_read_flips_exactly_the_filtered_rows() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);

        let n = mark_all_read(&inbox, &p, &InboxFilter::issues_my_work());
        assert_eq!(n, 1, "exactly the one My Work row was flipped");
        assert_eq!(
            state_of(&inbox, me, "iss-assigned").unwrap(),
            "read",
            "the My Work row is read"
        );
        assert_eq!(
            state_of(&inbox, me, "iss-state").unwrap(),
            "unread",
            "a non-My-Work issue is untouched (reason clause)"
        );
        assert_eq!(
            state_of(&inbox, me, "chat-ment").unwrap(),
            "unread",
            "a chat row is untouched (subsystem clause)"
        );
        assert_eq!(
            state_of(&inbox, me, "git-review").unwrap(),
            "unread",
            "a git row is untouched"
        );
    }

    #[test]
    fn mark_all_read_all_leaves_a_parked_snoozed_row_parked() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);
        snooze(&inbox, &p, "git-review", "2026-06-25T09:00:00Z").unwrap();

        let n = mark_all_read(&inbox, &p, &InboxFilter::all());
        assert_eq!(
            n, 3,
            "the three ACTIVE rows flipped (the snoozed one was skipped)"
        );
        assert_eq!(
            state_of(&inbox, me, "git-review").unwrap(),
            "snoozed",
            "mark_all_read did NOT un-snooze the parked row"
        );
        assert_eq!(state_of(&inbox, me, "iss-assigned").unwrap(), "read");
    }

    #[test]
    fn mark_all_read_is_recipient_scoped() {
        let inbox = InboxProjection::new();
        inbox.upsert_for_test(item(
            "u1",
            "u1-iss",
            "myelin://acme/issue/issue/P1",
            Reason::Assigned,
        ));
        inbox.upsert_for_test(item(
            "u2",
            "u2-iss",
            "myelin://acme/issue/issue/P2",
            Reason::Assigned,
        ));
        let n = mark_all_read(&inbox, &principal("u1"), &InboxFilter::issues_my_work());
        assert_eq!(n, 1, "only u1's row flipped");
        assert_eq!(state_of(&inbox, "u1", "u1-iss").unwrap(), "read");
        assert_eq!(
            state_of(&inbox, "u2", "u2-iss").unwrap(),
            "unread",
            "u2's inbox was NEVER touched"
        );
    }

    #[test]
    fn chained_mark_all_read_then_relist_is_consistent_across_views() {
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
                limit: 100,
            },
            &AllowAllAuthorize,
            &strong(),
        );
        for r in &view.items {
            assert_eq!(
                r.state, "read",
                "every My Work row reads `read` after mark_all_read"
            );
        }

        let full = list_inbox(
            &inbox,
            &p,
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 100,
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
            "the My Work row reads `read` in the unified inbox too (0 divergence)"
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

    #[test]
    fn active_inbox_keeps_active_drops_parked() {
        let me = "u1";
        let inbox = seeded(me);
        let p = principal(me);
        mark(&inbox, &p, "iss-assigned", ReadState::Read).unwrap();
        snooze(&inbox, &p, "chat-ment", "2026-06-25T09:00:00Z").unwrap();
        mark(&inbox, &p, "git-review", ReadState::Done).unwrap();

        let all = inbox.snapshot_for_tenant(&tenant());
        let active = active_inbox(all);
        let ids: std::collections::BTreeSet<_> = active.iter().map(|r| r.item_id.clone()).collect();
        assert!(ids.contains("iss-assigned"), "a read item is still active");
        assert!(ids.contains("iss-state"), "an unread item is active");
        assert!(!ids.contains("chat-ment"), "a snoozed item is parked");
        assert!(!ids.contains("git-review"), "a done item is parked");
    }
}
