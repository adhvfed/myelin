use myelin_identity::{Consistency, Principal};

use crate::list_inbox::{list_inbox, InboxFilter, InboxPage, Page, ReadAuthorizePort, Subsystem};
use crate::prefs::{get_prefs, route, set_prefs, PrefStore, PrefView, QuietHours};
use crate::read_state::{mark, snooze, ReadState, ReadStateError};
use crate::router::{InboxProjection, RoutedInboxItem};
use crate::watch::{watch_open, watch_resume, InboxFrame, WatchOutcome};
use crate::{Class, NotifPrefs, Reason};
use myelin_events::firehose::Firehose;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliView {
    All,
    MyWork,
    Activity,
    ReviewRequests,
}

impl CliView {
    pub fn parse(name: Option<&str>) -> Result<CliView, String> {
        match name {
            None | Some("all") => Ok(CliView::All),
            Some("my-work") => Ok(CliView::MyWork),
            Some("activity") => Ok(CliView::Activity),
            Some("review-requests") => Ok(CliView::ReviewRequests),
            Some(other) => Err(format!(
                "unknown view '{other}' (expected: all | my-work | activity | review-requests)"
            )),
        }
    }

    pub fn filter(self) -> InboxFilter {
        match self {
            CliView::All => InboxFilter::all(),
            CliView::MyWork => InboxFilter::issues_my_work(),
            CliView::Activity => InboxFilter::chat_activity(),
            CliView::ReviewRequests => InboxFilter::git_review_requests(),
        }
    }
}

pub fn inbox_list(
    inbox: &InboxProjection,
    principal: &Principal,
    view: CliView,
    page: &Page,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> InboxPage {
    list_inbox(inbox, principal, &view.filter(), page, authorize, at)
}

pub fn inbox_show(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> Option<InboxShow> {
    let page = list_inbox(
        inbox,
        principal,
        &InboxFilter::all(),
        &Page {
            after: None,
            limit: usize::MAX,
        },
        authorize,
        at,
    );
    page.items
        .into_iter()
        .find(|row| row.item_id == item_id)
        .map(InboxShow::from_row)
}

pub fn inbox_read(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
) -> Result<(), ReadStateError> {
    mark(inbox, principal, item_id, ReadState::Read)
}

pub fn inbox_snooze(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
    until: &str,
) -> Result<(), ReadStateError> {
    snooze(inbox, principal, item_id, until)
}

pub fn inbox_watch(
    firehose: &mut Firehose,
    principal: &Principal,
    cursor: Option<u64>,
) -> WatchView {
    let outcome = match cursor {
        None => watch_open(firehose, principal),
        Some(last_seq) => watch_resume(firehose, principal, last_seq),
    };
    match outcome {
        Err(_) => WatchView::ResyncRequired,
        Ok(WatchOutcome::ResyncRequired { .. }) => WatchView::ResyncRequired,
        Ok(WatchOutcome::Live(watch)) => {
            let frames = watch.drain();
            WatchView::Live {
                last_seq: watch.last_seq(),
                frames,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchView {
    Live {
        last_seq: u64,
        frames: Vec<InboxFrame>,
    },
    ResyncRequired,
}

pub fn render_watch(view: &WatchView) -> String {
    match view {
        WatchView::ResyncRequired => {
            "RESYNC_REQUIRED cursor too old - run `myelin inbox list` to cold-rebuild".to_string()
        }
        WatchView::Live { last_seq, frames } => {
            let mut out = format!("WATCHING cursor={last_seq}\n");
            for f in frames {
                out.push_str(&format!("WATCH {} {}\n", f.seq, f.item_id));
            }
            out
        }
    }
}

pub fn notify_prefs(store: &PrefStore, principal: &Principal) -> PrefView {
    get_prefs(store, principal)
}

pub fn notify_prefs_set(
    store: &PrefStore,
    principal: &Principal,
    prefs: NotifPrefs,
    quiet: QuietHours,
) -> PrefView {
    set_prefs(store, principal, prefs, quiet)
}

pub fn notify_test(
    store: &PrefStore,
    principal: &Principal,
    reason: Reason,
    class: Class,
    subsystem: Subsystem,
    utc_minute_of_day: i32,
    utc_weekday: u8,
) -> Vec<String> {
    let view = get_prefs(store, principal);
    route(
        &view.prefs,
        &view.quiet,
        reason,
        class,
        subsystem,
        utc_minute_of_day,
        utc_weekday,
    )
    .into_iter()
    .map(|c| c.token().to_string())
    .collect()
}

pub fn render_prefs(view: &PrefView) -> String {
    let mut out = String::new();
    out.push_str("routing:\n");
    for rule in &view.prefs.routing {
        let m = if rule.matcher.source().is_empty() {
            "<compiled matcher>".to_string()
        } else {
            rule.matcher.source().to_string()
        };
        out.push_str(&format!("  {} <- {}\n", rule.channel.token(), m));
    }
    out.push_str(&format!("digest: {}\n", view.prefs.digest.cadence));
    out.push_str(&format!(
        "quiet-hours (tz offset {}m):\n",
        view.quiet.tz.offset_minutes
    ));
    for w in &view.quiet.windows {
        out.push_str(&format!(
            "  [{:02}:{:02}, {:02}:{:02})  days={:?}\n",
            w.from / 60,
            w.from % 60,
            w.to / 60,
            w.to % 60,
            w.days
        ));
    }
    let pierces: Vec<&str> = view
        .quiet
        .pierce_classes
        .iter()
        .map(|c| crate::prefs::class_token(*c))
        .collect();
    out.push_str(&format!("pierce-classes: {}\n", pierces.join(",")));
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxShow {
    pub item_id: String,
    pub reason: String,
    pub class: String,
    pub subject: String,
    pub state: String,
    pub coalesce_count: i32,
}

impl InboxShow {
    fn from_row(row: RoutedInboxItem) -> InboxShow {
        InboxShow {
            item_id: row.item_id,
            reason: reason_token(row.reason),
            class: class_token(row.class),
            subject: row.subject.0,
            state: row.state,
            coalesce_count: row.coalesce_count,
        }
    }
}

pub fn render_list(page: &InboxPage) -> String {
    let mut out = String::new();
    for row in &page.items {
        out.push_str(&format!(
            "{}  [{}/{}]  {}  ({})\n",
            row.item_id,
            reason_token(row.reason),
            class_token(row.class),
            row.subject.0,
            row.state,
        ));
    }
    if page.cursor.0.is_some() {
        out.push_str("… (more - pass the cursor to page)\n");
    }
    out
}

fn reason_token(reason: crate::Reason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{reason:?}"))
}

fn class_token(class: crate::Class) -> String {
    serde_json::to_value(class)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{class:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_inbox::AllowAllAuthorize;
    use myelin_events::ArtifactRef;
    use myelin_identity::{ConsistencyMode, Decision, PrincipalId, PrincipalKind, Zookie};
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
    fn item(
        recipient: &str,
        item_id: &str,
        subject: &str,
        reason: crate::Reason,
    ) -> RoutedInboxItem {
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
            "iss-1",
            "myelin://acme/issue/issue/PROJ-1",
            crate::Reason::Assigned,
        ));
        inbox.upsert_for_test(item(
            me,
            "chat-1",
            "myelin://acme/chat/thread/T1",
            crate::Reason::Mentioned,
        ));
        inbox.upsert_for_test(item(
            me,
            "git-1",
            "myelin://acme/git/pr/9",
            crate::Reason::ReviewRequested,
        ));
        inbox
    }

    #[test]
    fn view_parse_maps_names_and_rejects_unknown() {
        assert_eq!(CliView::parse(None).unwrap(), CliView::All);
        assert_eq!(CliView::parse(Some("all")).unwrap(), CliView::All);
        assert_eq!(CliView::parse(Some("my-work")).unwrap(), CliView::MyWork);
        assert_eq!(CliView::parse(Some("activity")).unwrap(), CliView::Activity);
        assert_eq!(
            CliView::parse(Some("review-requests")).unwrap(),
            CliView::ReviewRequests
        );
        assert!(
            CliView::parse(Some("everything")).is_err(),
            "an unknown view is rejected (never the ALL inbox)"
        );
    }

    #[test]
    fn inbox_list_my_work_selects_only_the_issues_row() {
        let inbox = seeded("u1");
        let page = inbox_list(
            &inbox,
            &principal("u1"),
            CliView::MyWork,
            &Page::default(),
            &AllowAllAuthorize,
            &strong(),
        );
        let ids: Vec<_> = page.items.iter().map(|i| i.item_id.clone()).collect();
        assert_eq!(
            ids,
            vec!["iss-1".to_string()],
            "the My Work view selects only the assigned issue"
        );
    }

    #[test]
    fn inbox_list_default_is_the_one_inbox() {
        let inbox = seeded("u1");
        let page = inbox_list(
            &inbox,
            &principal("u1"),
            CliView::All,
            &Page::default(),
            &AllowAllAuthorize,
            &strong(),
        );
        assert_eq!(
            page.items.len(),
            3,
            "the default view is the ONE inbox (all rows for u1)"
        );
    }

    #[test]
    fn inbox_show_returns_refs_not_payloads() {
        let inbox = seeded("u1");
        let show = inbox_show(
            &inbox,
            &principal("u1"),
            "iss-1",
            &AllowAllAuthorize,
            &strong(),
        )
        .unwrap();
        assert_eq!(show.item_id, "iss-1");
        assert_eq!(
            show.reason, "assigned",
            "the reason is the snake_case token (PII-free)"
        );
        assert_eq!(show.class, "direct");
        assert_eq!(
            show.subject, "myelin://acme/issue/issue/PROJ-1",
            "the subject is a REF, never a title"
        );
        assert_eq!(show.state, "unread");
    }

    #[test]
    fn inbox_show_obeys_authorize_and_recipient_scope_no_backdoor() {
        struct DenyAll;
        impl ReadAuthorizePort for DenyAll {
            fn can_read(&self, _v: &Principal, _s: &ArtifactRef, _a: &Consistency) -> Decision {
                Decision::Deny
            }
        }
        let inbox = seeded("u1");
        assert!(
            inbox_show(&inbox, &principal("u1"), "iss-1", &DenyAll, &strong()).is_none(),
            "denied → not shown"
        );
        assert!(
            inbox_show(
                &inbox,
                &principal("u2"),
                "iss-1",
                &AllowAllAuthorize,
                &strong()
            )
            .is_none(),
            "not my item → not shown"
        );
        assert!(inbox_show(
            &inbox,
            &principal("u1"),
            "no-such",
            &AllowAllAuthorize,
            &strong()
        )
        .is_none());
    }

    #[test]
    fn inbox_read_marks_read_recipient_scoped() {
        let inbox = seeded("u1");
        inbox_read(&inbox, &principal("u1"), "iss-1").expect("read my own item");
        let show = inbox_show(
            &inbox,
            &principal("u1"),
            "iss-1",
            &AllowAllAuthorize,
            &strong(),
        )
        .unwrap();
        assert_eq!(show.state, "read", "the item is read after `inbox read`");
        assert_eq!(
            inbox_read(&inbox, &principal("u2"), "iss-1"),
            Err(crate::read_state::ReadStateError::NotFound)
        );
        assert_eq!(
            inbox_read(&inbox, &principal("u1"), "no-such"),
            Err(crate::read_state::ReadStateError::NotFound)
        );
    }

    #[test]
    fn inbox_snooze_records_until_and_parks() {
        let inbox = seeded("u1");
        inbox_snooze(&inbox, &principal("u1"), "chat-1", "2026-06-25T09:00:00Z")
            .expect("snooze my own item");
        let show = inbox_show(
            &inbox,
            &principal("u1"),
            "chat-1",
            &AllowAllAuthorize,
            &strong(),
        )
        .unwrap();
        assert_eq!(
            show.state, "snoozed",
            "the item is snoozed after `inbox snooze`"
        );
        let page = inbox_list(
            &inbox,
            &principal("u1"),
            CliView::All,
            &Page::default(),
            &AllowAllAuthorize,
            &strong(),
        );
        let active = crate::read_state::active_inbox(page.items);
        assert!(
            !active.iter().any(|r| r.item_id == "chat-1"),
            "the snoozed item is absent from the active inbox"
        );
    }

    #[test]
    fn render_list_is_pii_free_lines() {
        let inbox = seeded("u1");
        let page = inbox_list(
            &inbox,
            &principal("u1"),
            CliView::All,
            &Page::default(),
            &AllowAllAuthorize,
            &strong(),
        );
        let out = render_list(&page);
        assert!(
            out.contains("iss-1  [assigned/direct]  myelin://acme/issue/issue/PROJ-1  (unread)")
        );
        assert!(out.contains("myelin://acme/git/pr/9"));
    }

    #[test]
    fn inbox_watch_streams_live_frames_pii_free() {
        let mut fh = myelin_events::firehose::Firehose::new();
        let me = principal("u1");
        let _open = inbox_watch(&mut fh, &me, None);
        crate::watch::publish_inbox_frame(&mut fh, &me, "itm-1").unwrap();
        crate::watch::publish_inbox_frame(&mut fh, &me, "itm-2").unwrap();
        let view = inbox_watch(&mut fh, &me, Some(0));
        let out = render_watch(&view);
        assert!(
            out.contains("WATCH 1 itm-1"),
            "the first live frame renders as a pii-free pointer line"
        );
        assert!(
            out.contains("WATCH 2 itm-2"),
            "the second live frame renders"
        );
        if let WatchView::Live { last_seq, .. } = view {
            assert_eq!(last_seq, 2, "the resume cursor is the last delivered seq");
        } else {
            panic!("expected a live watch");
        }
    }

    #[test]
    fn inbox_watch_over_old_cursor_directs_a_cold_rebuild() {
        let mut fh = myelin_events::firehose::Firehose::with_limits(2, 1024);
        let me = principal("u1");
        for i in 1..=5 {
            crate::watch::publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
        }
        let view = inbox_watch(&mut fh, &me, Some(1));
        assert_eq!(view, WatchView::ResyncRequired);
        assert!(
            render_watch(&view).contains("RESYNC_REQUIRED"),
            "the resync directive is surfaced, NAMED"
        );
    }
}
