use std::collections::HashSet;

use myelin_events::ArtifactRef;
use myelin_identity::{Consistency, Decision, Principal};

use crate::router::{InboxProjection, RoutedInboxItem};
use crate::Reason;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subsystem {
    Issue,
    Chat,
    Git,
    Knowledge,
    Ci,
    Unknown,
}

pub fn subsystem_of(subject: &ArtifactRef) -> Subsystem {
    let rest = match subject.0.strip_prefix("myelin://") {
        Some(r) => r,
        None => return Subsystem::Unknown,
    };
    let mut segs = rest.split('/');
    let _tenant = segs.next();
    match segs.next() {
        Some("issue") | Some("issues") => Subsystem::Issue,
        Some("chat") => Subsystem::Chat,
        Some("git") => Subsystem::Git,
        Some("kn") | Some("knowledge") => Subsystem::Knowledge,
        Some("ci") => Subsystem::Ci,
        _ => Subsystem::Unknown,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxFilter {
    pub subsystems: Option<HashSet<Subsystem>>,
    pub reasons: Option<HashSet<Reason>>,
}

impl InboxFilter {
    pub fn all() -> InboxFilter {
        InboxFilter::default()
    }

    pub fn issues_my_work() -> InboxFilter {
        InboxFilter {
            subsystems: Some([Subsystem::Issue].into_iter().collect()),
            reasons: Some(
                [
                    Reason::Assigned,
                    Reason::Mentioned,
                    Reason::ReviewRequested,
                    Reason::Sla,
                    Reason::Watched,
                    Reason::Blocked,
                    Reason::ApprovalRequested,
                ]
                .into_iter()
                .collect(),
            ),
        }
    }

    pub fn chat_activity() -> InboxFilter {
        InboxFilter {
            subsystems: Some([Subsystem::Chat].into_iter().collect()),
            reasons: Some(
                [
                    Reason::Mentioned,
                    Reason::Replied,
                    Reason::ThreadWatched,
                    Reason::ApprovalRequested,
                ]
                .into_iter()
                .collect(),
            ),
        }
    }

    pub fn git_review_requests() -> InboxFilter {
        InboxFilter {
            subsystems: Some([Subsystem::Git].into_iter().collect()),
            reasons: Some(
                [Reason::ReviewRequested, Reason::Mentioned]
                    .into_iter()
                    .collect(),
            ),
        }
    }

    pub fn matches(&self, item: &RoutedInboxItem) -> bool {
        if let Some(subs) = &self.subsystems {
            if !subs.contains(&subsystem_of(&item.subject)) {
                return false;
            }
        }
        if let Some(reasons) = &self.reasons {
            if !reasons.contains(&item.reason) {
                return false;
            }
        }
        true
    }
}

pub trait ReadAuthorizePort {
    fn can_read(&self, viewer: &Principal, subject: &ArtifactRef, at: &Consistency) -> Decision;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub after: Option<String>,
    pub limit: usize,
}

impl Default for Page {
    fn default() -> Page {
        Page {
            after: None,
            limit: 50,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor(pub Option<String>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxPage {
    pub items: Vec<RoutedInboxItem>,
    pub cursor: Cursor,
}

pub fn list_inbox(
    inbox: &InboxProjection,
    principal: &Principal,
    filter: &InboxFilter,
    page: &Page,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> InboxPage {
    let me = principal.principal_id.0.as_str();
    let mut candidates: Vec<RoutedInboxItem> = inbox
        .snapshot_for_tenant(&principal.tenant)
        .into_iter()
        .filter(|row| row.recipient == me)
        .filter(|row| filter.matches(row))
        .filter(|row| authorize.can_read(principal, &row.subject, at) == Decision::Allow)
        .collect();

    let ranker = crate::ranking::DeterministicV1::default();
    candidates = crate::ranking::rank_and_order(candidates, principal, &ranker)
        .into_iter()
        .map(|ranked| ranked.item)
        .collect();

    let start = match &page.after {
        Some(after) => candidates
            .iter()
            .position(|row| &row.item_id == after)
            .map(|i| i + 1)
            .unwrap_or(candidates.len()),
        None => 0,
    };
    let end = start.saturating_add(page.limit).min(candidates.len());
    let items: Vec<RoutedInboxItem> = candidates[start..end].to_vec();
    let cursor = if end < candidates.len() {
        Cursor(items.last().map(|row| row.item_id.clone()))
    } else {
        Cursor(None)
    };
    InboxPage { items, cursor }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedPage {
    pub items: Vec<crate::ranking::RankedItem>,
    pub cursor: Cursor,
}

pub fn list_inbox_ranked(
    inbox: &InboxProjection,
    principal: &Principal,
    filter: &InboxFilter,
    page: &Page,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
    strategy: &dyn crate::ranking::RankStrategy,
) -> RankedPage {
    let me = principal.principal_id.0.as_str();
    let candidates: Vec<RoutedInboxItem> = inbox
        .snapshot_for_tenant(&principal.tenant)
        .into_iter()
        .filter(|row| row.recipient == me)
        .filter(|row| filter.matches(row))
        .filter(|row| authorize.can_read(principal, &row.subject, at) == Decision::Allow)
        .collect();

    let ranked = crate::ranking::rank_and_order(candidates, principal, strategy);

    let start = match &page.after {
        Some(after) => ranked
            .iter()
            .position(|r| &r.item.item_id == after)
            .map(|i| i + 1)
            .unwrap_or(ranked.len()),
        None => 0,
    };
    let end = start.saturating_add(page.limit).min(ranked.len());
    let items = ranked[start..end].to_vec();
    let cursor = if end < ranked.len() {
        Cursor(items.last().map(|r| r.item.item_id.clone()))
    } else {
        Cursor(None)
    };
    RankedPage { items, cursor }
}

pub struct AllowAllAuthorize;

impl ReadAuthorizePort for AllowAllAuthorize {
    fn can_read(&self, _viewer: &Principal, _subject: &ArtifactRef, _at: &Consistency) -> Decision {
        Decision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use myelin_identity::{ConsistencyMode, PrincipalId, PrincipalKind, Zookie};
    use myelin_tenancy::{Region, TenantId};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn principal(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn strong() -> Consistency {
        Consistency {
            at_least: Zookie("zk-1".into()),
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

    fn seeded_inbox(me: &str) -> InboxProjection {
        let inbox = InboxProjection::new();
        inbox.upsert_for_test(item(
            me,
            "itm-iss-assigned",
            "myelin://acme/issue/issue/PROJ-1",
            Reason::Assigned,
        ));
        inbox.upsert_for_test(item(
            me,
            "itm-iss-state",
            "myelin://acme/issue/issue/PROJ-2",
            Reason::StateChanged,
        ));
        inbox.upsert_for_test(item(
            me,
            "itm-chat-ment",
            "myelin://acme/chat/thread/T1",
            Reason::Mentioned,
        ));
        inbox.upsert_for_test(item(
            me,
            "itm-chat-state",
            "myelin://acme/chat/thread/T2",
            Reason::StateChanged,
        ));
        inbox.upsert_for_test(item(
            me,
            "itm-git-review",
            "myelin://acme/git/pr/9",
            Reason::ReviewRequested,
        ));
        inbox.upsert_for_test(item(
            me,
            "itm-git-watched",
            "myelin://acme/git/pr/10",
            Reason::Watched,
        ));
        inbox
    }

    fn ids(page: &InboxPage) -> BTreeSet<String> {
        page.items.iter().map(|i| i.item_id.clone()).collect()
    }

    #[test]
    fn subsystem_is_derived_from_the_subject_ref_unknown_is_not_a_known_subsystem() {
        assert_eq!(
            subsystem_of(&ArtifactRef("myelin://acme/issue/issue/PROJ-1".into())),
            Subsystem::Issue
        );
        assert_eq!(
            subsystem_of(&ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())),
            Subsystem::Issue
        );
        assert_eq!(
            subsystem_of(&ArtifactRef("myelin://acme/chat/thread/T1".into())),
            Subsystem::Chat
        );
        assert_eq!(
            subsystem_of(&ArtifactRef("myelin://acme/git/pr/9".into())),
            Subsystem::Git
        );
        assert_eq!(
            subsystem_of(&ArtifactRef("myelin://acme/kn/doc/D1".into())),
            Subsystem::Knowledge
        );
        assert_eq!(
            subsystem_of(&ArtifactRef("myelin://acme/ci/run/42".into())),
            Subsystem::Ci
        );
        assert_eq!(
            subsystem_of(&ArtifactRef("myelin://acme/mystery/x/1".into())),
            Subsystem::Unknown
        );
        assert_eq!(
            subsystem_of(&ArtifactRef("not-a-ref".into())),
            Subsystem::Unknown
        );
        assert_eq!(
            subsystem_of(&ArtifactRef("myelin://acme".into())),
            Subsystem::Unknown
        );
    }

    #[test]
    fn c9_invariant_every_view_is_a_strict_subset_of_the_unfiltered_inbox() {
        let me = "u1";
        let inbox = seeded_inbox(me);
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
        let full_ids = ids(&full);
        assert_eq!(
            full_ids.len(),
            6,
            "the unfiltered inbox is the ONE inbox (all 6 rows for u1)"
        );

        for view in [
            InboxFilter::issues_my_work(),
            InboxFilter::chat_activity(),
            InboxFilter::git_review_requests(),
        ] {
            let v = list_inbox(&inbox, &p, &view, &big, &AllowAllAuthorize, &strong());
            let view_ids = ids(&v);
            assert!(
                view_ids.is_subset(&full_ids),
                "C-9: the view {view:?} is a SUBSET of the unfiltered inbox"
            );
            assert!(
                !view_ids.is_empty(),
                "the seeded batch put ≥1 row in every view"
            );
            assert!(
                view_ids.len() < full_ids.len(),
                "a scoped view is STRICTLY smaller than the ONE inbox"
            );
        }
    }

    #[test]
    fn the_three_frozen_views_select_exactly_their_rows() {
        let me = "u1";
        let inbox = seeded_inbox(me);
        let p = principal(me);
        let big = Page {
            after: None,
            limit: 1000,
        };

        let my_work = ids(&list_inbox(
            &inbox,
            &p,
            &InboxFilter::issues_my_work(),
            &big,
            &AllowAllAuthorize,
            &strong(),
        ));
        assert!(
            my_work.contains("itm-iss-assigned"),
            "assigned is in My Work"
        );
        assert!(
            !my_work.contains("itm-iss-state"),
            "a state_changed issue is NOT in My Work"
        );
        assert!(
            !my_work.contains("itm-chat-ment"),
            "a chat mention is NOT in the Issues view (subsystem clause)"
        );

        let activity = ids(&list_inbox(
            &inbox,
            &p,
            &InboxFilter::chat_activity(),
            &big,
            &AllowAllAuthorize,
            &strong(),
        ));
        assert_eq!(
            activity,
            ["itm-chat-ment".to_string()].into_iter().collect()
        );

        let reviews = ids(&list_inbox(
            &inbox,
            &p,
            &InboxFilter::git_review_requests(),
            &big,
            &AllowAllAuthorize,
            &strong(),
        ));
        assert_eq!(
            reviews,
            ["itm-git-review".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn filter_matches_only_narrows() {
        let issue_assigned = item(
            "u1",
            "a",
            "myelin://acme/issue/issue/PROJ-1",
            Reason::Assigned,
        );
        let chat_mention = item("u1", "b", "myelin://acme/chat/thread/T1", Reason::Mentioned);
        assert!(InboxFilter::all().matches(&issue_assigned));
        assert!(InboxFilter::all().matches(&chat_mention));
        assert!(InboxFilter::issues_my_work().matches(&issue_assigned));
        assert!(
            !InboxFilter::issues_my_work().matches(&chat_mention),
            "wrong subsystem → rejected"
        );
        let issue_state = item(
            "u1",
            "c",
            "myelin://acme/issue/issue/PROJ-2",
            Reason::StateChanged,
        );
        assert!(
            !InboxFilter::issues_my_work().matches(&issue_state),
            "wrong reason → rejected"
        );
    }

    #[test]
    fn list_inbox_is_recipient_scoped_others_items_are_not_returned() {
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
        let got = ids(&page);
        assert!(got.contains("mine"), "my item is returned");
        assert!(
            !got.contains("theirs"),
            "another principal's item is NEVER in my inbox (recipient scope)"
        );
        assert_eq!(got.len(), 1);
    }

    struct DenySubjects(BTreeSet<String>);
    impl ReadAuthorizePort for DenySubjects {
        fn can_read(&self, _v: &Principal, subject: &ArtifactRef, _at: &Consistency) -> Decision {
            if self.0.contains(&subject.0) {
                Decision::Deny
            } else {
                Decision::Allow
            }
        }
    }

    #[test]
    fn denied_item_is_not_returned() {
        let me = "u1";
        let inbox = seeded_inbox(me);
        let deny = DenySubjects(
            ["myelin://acme/issue/issue/PROJ-1".to_string()]
                .into_iter()
                .collect(),
        );
        let big = Page {
            after: None,
            limit: 1000,
        };
        let page = list_inbox(
            &inbox,
            &principal(me),
            &InboxFilter::all(),
            &big,
            &deny,
            &strong(),
        );
        let got = ids(&page);
        assert!(
            !got.contains("itm-iss-assigned"),
            "the denied subject's item is HELD, not leaked (ADR-03)"
        );
        assert_eq!(got.len(), 5, "the other 5 visible items surface");
    }

    #[test]
    fn conditional_check_is_failclosed_not_leaked() {
        struct AlwaysConditional;
        impl ReadAuthorizePort for AlwaysConditional {
            fn can_read(&self, _v: &Principal, _s: &ArtifactRef, _at: &Consistency) -> Decision {
                Decision::Conditional
            }
        }
        let inbox = seeded_inbox("u1");
        let page = list_inbox(
            &inbox,
            &principal("u1"),
            &InboxFilter::all(),
            &Page::default(),
            &AlwaysConditional,
            &strong(),
        );
        assert!(
            page.items.is_empty(),
            "a Conditional check is fail-closed (deny-when-unsure, ADR-03)"
        );
    }

    #[test]
    fn ranked_order_and_paging_is_exhaustive_and_non_overlapping() {
        let me = "u1";
        let inbox = seeded_inbox(me);
        let p = principal(me);

        let mut seen: Vec<String> = Vec::new();
        let mut after: Option<String> = None;
        let mut guard = 0;
        loop {
            guard += 1;
            assert!(
                guard <= 8,
                "paging must terminate within the page bound (the cursor advances)"
            );
            let page = list_inbox(
                &inbox,
                &p,
                &InboxFilter::all(),
                &Page {
                    after: after.clone(),
                    limit: 2,
                },
                &AllowAllAuthorize,
                &strong(),
            );
            for it in &page.items {
                seen.push(it.item_id.clone());
            }
            match page.cursor.0 {
                Some(c) => after = Some(c),
                None => break,
            }
        }
        let expected: Vec<String> = list_inbox(
            &inbox,
            &p,
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 1000,
            },
            &AllowAllAuthorize,
            &strong(),
        )
        .items
        .iter()
        .map(|i| i.item_id.clone())
        .collect();
        assert_eq!(
            expected,
            vec![
                "itm-chat-ment".to_string(),
                "itm-git-review".to_string(),
                "itm-iss-assigned".to_string(),
                "itm-chat-state".to_string(),
                "itm-git-watched".to_string(),
                "itm-iss-state".to_string(),
            ],
            "the order is (priority DESC, item_id ASC): the three direct(70) above the three watching(35)"
        );
        assert_eq!(seen.len(), 6, "paging visited every item once (no skip)");
        let unique: BTreeSet<_> = seen.iter().cloned().collect();
        assert_eq!(unique.len(), 6, "paging never returned a duplicate");
        assert_eq!(
            seen, expected,
            "the page order is the RANKED order (priority DESC, item_id ASC)"
        );
    }

    #[test]
    fn list_inbox_ranked_carries_priority_and_trace_per_item() {
        use crate::ranking::DeterministicV1;
        let me = "u1";
        let inbox = seeded_inbox(me);
        let p = principal(me);
        let ranker = DeterministicV1::default();
        let page = list_inbox_ranked(
            &inbox,
            &p,
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 100,
            },
            &AllowAllAuthorize,
            &strong(),
            &ranker,
        );
        assert_eq!(page.items.len(), 6, "all 6 visible items are ranked");
        for r in &page.items {
            assert_eq!(
                r.priority, r.trace.final_priority,
                "the trace's final == the priority"
            );
            assert!(
                !r.trace.render().is_empty(),
                "every rank carries a non-empty explain-trace"
            );
        }
        let priorities: Vec<u8> = page.items.iter().map(|r| r.priority).collect();
        let mut sorted = priorities.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(priorities, sorted, "ranked page is priority-descending");
        let deny = DenySubjects(
            ["myelin://acme/issue/issue/PROJ-1".to_string()]
                .into_iter()
                .collect(),
        );
        let denied_page = list_inbox_ranked(
            &inbox,
            &p,
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 100,
            },
            &deny,
            &strong(),
            &ranker,
        );
        assert!(
            !denied_page
                .items
                .iter()
                .any(|r| r.item.item_id == "itm-iss-assigned"),
            "the denied subject's item is held, not ranked (authorize before rank)"
        );
        assert_eq!(denied_page.items.len(), 5);
    }

    #[test]
    fn page_is_bounded_and_reports_more() {
        let inbox = seeded_inbox("u1");
        let page = list_inbox(
            &inbox,
            &principal("u1"),
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 2,
            },
            &AllowAllAuthorize,
            &strong(),
        );
        assert_eq!(
            page.items.len(),
            2,
            "the page is bounded to the limit (never unbounded)"
        );
        assert!(
            page.cursor.0.is_some(),
            "there are more rows → a forward cursor"
        );
    }

    #[test]
    fn page_that_exactly_exhausts_reports_no_cursor() {
        let inbox = seeded_inbox("u1");
        let page = list_inbox(
            &inbox,
            &principal("u1"),
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 6,
            },
            &AllowAllAuthorize,
            &strong(),
        );
        assert_eq!(page.items.len(), 6, "all 6 items on the page");
        assert_eq!(
            page.cursor,
            Cursor(None),
            "an exactly-exhausting page has NO forward cursor (end == len)"
        );

        let last_id = page.items.last().unwrap().item_id.clone();
        let next = list_inbox(
            &inbox,
            &principal("u1"),
            &InboxFilter::all(),
            &Page {
                after: Some(last_id),
                limit: 6,
            },
            &AllowAllAuthorize,
            &strong(),
        );
        assert!(next.items.is_empty(), "no items after the last one");
    }

    #[test]
    fn empty_inbox_returns_empty_page_no_cursor() {
        let inbox = InboxProjection::new();
        let page = list_inbox(
            &inbox,
            &principal("nobody"),
            &InboxFilter::all(),
            &Page::default(),
            &AllowAllAuthorize,
            &strong(),
        );
        assert!(page.items.is_empty());
        assert_eq!(page.cursor, Cursor(None), "no more rows → no cursor");
    }

    #[test]
    fn allow_all_authorize_allows() {
        let port = AllowAllAuthorize;
        assert_eq!(
            port.can_read(
                &principal("u1"),
                &ArtifactRef("myelin://acme/issue/issue/P1".into()),
                &strong()
            ),
            Decision::Allow
        );
    }
}
