//! # `myelin inbox list | show` — the read-surface CLI (NOTIF-P5 / P-183, M2)
//!
//! **Owning architecture doc:** `notifications.md` §1.3 (the ONE inbox; scoped views are filters),
//! §1.4 (agents have inboxes too). **Contract:** **7.1** `list_inbox` (the read surface the CLI
//! drives). The CLI is a thin presentation seam over [`list_inbox`](crate::list_inbox::list_inbox):
//! it maps a named scoped view ([`CliView`]) to the frozen [`InboxFilter`], calls `list_inbox`, and
//! renders the page as PII-free lines (an opaque `item_id`, the reason/class token, the subject
//! ref — never a rendered string; humanise is per-viewer at read time, NOTIF-P9).
//!
//! ## What this ships
//! - [`inbox_list`] — `myelin inbox list [--view my-work|activity|review-requests]`: the ONE inbox
//!   (default `--view all`) or a named scoped view, paged, authorized.
//! - [`inbox_show`] — `myelin inbox show <item_id>`: one item's read-surface detail (reason, class,
//!   subject ref, read-state), for the principal (recipient-scoped + authorized through the SAME
//!   `list_inbox` path, so `show` can never reveal an item `list` would not).
//!
//! ## What this ships (read-state added at NOTIF-P6 / P-184)
//! - [`inbox_read`] — `myelin inbox read <item_id>`: mark the caller's item read (the ONE
//!   read-state truth — it is read in every view at once). Recipient-scoped (you can only read your
//!   own items).
//! - [`inbox_snooze`] — `myelin inbox snooze <item_id> --until <ts>`: park the item until `<ts>`
//!   (suppressed from the active inbox; the until recorded). The durable re-surface timer is
//!   NOTIF-P14/P18 (named floor).
//!
//! ## FLOORS named
//! - **The argv parse + the wired binary** (the actual `myelin` CLI command tree / the gateway
//!   route) is the driver's (the CLI binary lands with the gateway wiring, P-S15+). Here the CLI is
//!   the LIBRARY surface a binary calls — keeping `cargo build --workspace` DB-free and the read
//!   logic unit-testable. The presentation (humanised per-viewer strings) is NOTIF-P9; here the CLI
//!   renders the structured refs/tokens (the read surface, not the humanised render).
//! - **The durable snooze re-surface TIMER** (the `myelin-flow` wheel that flips a due snooze back to
//!   the active inbox) is **NOTIF-P14 / NOTIF-P18**; `inbox snooze` records the until only.
//! - **watch** (`myelin inbox watch`) is NOTIF-P15.

use myelin_identity::{Consistency, Principal};

use crate::list_inbox::{list_inbox, InboxFilter, InboxPage, Page, ReadAuthorizePort};
use crate::read_state::{mark, snooze, ReadState, ReadStateError};
use crate::router::{InboxProjection, RoutedInboxItem};

/// **The named scoped view a `myelin inbox list --view <name>` selects** (the C-9 §1.3 surfaces).
/// Each maps to a frozen [`InboxFilter`] — a filter over the ONE inbox, never a second store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliView {
    /// `--view all` (the default) — the unified ONE inbox (`filter = ∅`).
    All,
    /// `--view my-work` — Issues "My Work" (`subsystem∈{issue} ∧ reason∈{assigned, …}`).
    MyWork,
    /// `--view activity` — Chat "Activity / Mentions" (`subsystem∈{chat} ∧ reason∈{mentioned, …}`).
    Activity,
    /// `--view review-requests` — Git "Review requests" (`subsystem∈{git} ∧ reason∈{review_requested, …}`).
    ReviewRequests,
}

impl CliView {
    /// Parse the `--view` flag value into a [`CliView`] (`None` ⇒ the default `all`). An unknown
    /// view name is `Err` (a typo'd view never silently degrades to the ALL inbox — that would over-
    /// share; loud, never silent).
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

    /// The frozen [`InboxFilter`] this view selects (the §1.3 mapping — a filter, not a store).
    pub fn filter(self) -> InboxFilter {
        match self {
            CliView::All => InboxFilter::all(),
            CliView::MyWork => InboxFilter::issues_my_work(),
            CliView::Activity => InboxFilter::chat_activity(),
            CliView::ReviewRequests => InboxFilter::git_review_requests(),
        }
    }
}

/// **`myelin inbox list [--view <name>]` — list the ONE inbox (or a scoped view), paged.** Drives
/// [`list_inbox`] for `principal` with the [`CliView`]'s frozen filter, returning the
/// [`InboxPage`] (recipient-scoped, filtered, authorized, ordered). The caller renders it via
/// [`render_list`]. The read goes through the ONE `list_inbox` path — a CLI view can never reveal
/// an item the contract read would not.
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

/// **`myelin inbox show <item_id>` — one item's read-surface detail.** Looks the item up THROUGH
/// the unfiltered [`list_inbox`] path (recipient-scoped + authorized), so `show` can never reveal
/// an item `list` would not (no back-door read): an item not in the caller's authorized inbox →
/// `None` (held, not leaked). Returns the [`InboxShow`] structured detail (refs/tokens, never a
/// rendered string — humanise is NOTIF-P9).
pub fn inbox_show(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> Option<InboxShow> {
    // Read the whole authorized inbox (bounded scan; the live OLTP form is a `WHERE item_id = $1`
    // re-checked through the SAME authorize seam — same visibility, no back-door). A large limit so
    // the item is found if present; the live form looks it up by key then authorizes it.
    let page = list_inbox(
        inbox,
        principal,
        &InboxFilter::all(),
        &Page { after: None, limit: usize::MAX },
        authorize,
        at,
    );
    page.items
        .into_iter()
        .find(|row| row.item_id == item_id)
        .map(InboxShow::from_row)
}

/// **`myelin inbox read <item_id>` — mark the caller's item read (the ONE read-state truth).** A
/// thin seam over [`mark`]`(.., ReadState::Read)`: it flips the `state` of the calling principal's
/// item to `read`, visible in the unified inbox AND every scoped view at once (one store). A row not
/// addressed to the caller / a missing id is [`ReadStateError::NotFound`] (you can only read your own
/// items; held, not leaked).
pub fn inbox_read(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
) -> Result<(), ReadStateError> {
    mark(inbox, principal, item_id, ReadState::Read)
}

/// **`myelin inbox snooze <item_id> --until <ts>` — park the caller's item until `<ts>`.** A thin
/// seam over [`snooze`]: it sets the item to `snoozed` and records the `until`; the item is then
/// suppressed from the active inbox. A row not addressed to the caller / a missing id is
/// [`ReadStateError::NotFound`]. **FLOOR:** the durable re-surface timer is NOTIF-P14/P18 — here only
/// the until is recorded.
pub fn inbox_snooze(
    inbox: &InboxProjection,
    principal: &Principal,
    item_id: &str,
    until: &str,
) -> Result<(), ReadStateError> {
    snooze(inbox, principal, item_id, until)
}

/// **One item's structured read-surface detail (`myelin inbox show`).** PII-free: the opaque
/// `item_id`, the reason/class tokens, the subject ref, the read-state — NEVER a rendered string
/// (the per-viewer humanised render is NOTIF-P9; the CLI presents the structured refs/tokens).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxShow {
    /// The opaque inbox-item id (the read-state handle, contract 7.2).
    pub item_id: String,
    /// The structured why-it-fired token (the C-9 filter basis) — e.g. `assigned`.
    pub reason: String,
    /// The routing class token — e.g. `direct`.
    pub class: String,
    /// The subject `ArtifactRef` (a ref, never a payload — humanise resolves it per-viewer, P9).
    pub subject: String,
    /// The ONE read-state column value (unread|seen|read|…).
    pub state: String,
    /// The "+N more" write-time-collapse counter.
    pub coalesce_count: i32,
}

impl InboxShow {
    /// Project a [`RoutedInboxItem`] into the PII-free read-surface detail (tokens + refs only).
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

/// Render a [`InboxPage`] as PII-free CLI lines (`<item_id>  [<reason>/<class>]  <subject-ref>
/// (<state>)`) — never a rendered string (humanise is NOTIF-P9). One line per item, stable order.
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
        out.push_str("… (more — pass the cursor to page)\n");
    }
    out
}

/// The PII-free `reason` token for CLI/output (the serde snake_case wire token).
fn reason_token(reason: crate::Reason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{reason:?}"))
}

/// The PII-free `class` token for CLI/output (the serde snake_case wire token).
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
        Consistency { at_least: Zookie("zk".into()), mode: ConsistencyMode::Strong }
    }
    fn item(recipient: &str, item_id: &str, subject: &str, reason: crate::Reason) -> RoutedInboxItem {
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
        inbox.upsert_for_test(item(me, "iss-1", "myelin://acme/issue/issue/PROJ-1", crate::Reason::Assigned));
        inbox.upsert_for_test(item(me, "chat-1", "myelin://acme/chat/thread/T1", crate::Reason::Mentioned));
        inbox.upsert_for_test(item(me, "git-1", "myelin://acme/git/pr/9", crate::Reason::ReviewRequested));
        inbox
    }

    /// **`--view` parses the named scoped views and rejects an unknown one** (a typo never silently
    /// becomes the ALL inbox — that would over-share).
    #[test]
    fn view_parse_maps_names_and_rejects_unknown() {
        assert_eq!(CliView::parse(None).unwrap(), CliView::All);
        assert_eq!(CliView::parse(Some("all")).unwrap(), CliView::All);
        assert_eq!(CliView::parse(Some("my-work")).unwrap(), CliView::MyWork);
        assert_eq!(CliView::parse(Some("activity")).unwrap(), CliView::Activity);
        assert_eq!(CliView::parse(Some("review-requests")).unwrap(), CliView::ReviewRequests);
        assert!(CliView::parse(Some("everything")).is_err(), "an unknown view is rejected (never the ALL inbox)");
    }

    /// **`inbox list --view my-work` drives `list_inbox` with the frozen My Work filter** — exactly
    /// the issues row, never the chat/git rows (the CLI view is the contract view).
    #[test]
    fn inbox_list_my_work_selects_only_the_issues_row() {
        let inbox = seeded("u1");
        let page = inbox_list(&inbox, &principal("u1"), CliView::MyWork, &Page::default(), &AllowAllAuthorize, &strong());
        let ids: Vec<_> = page.items.iter().map(|i| i.item_id.clone()).collect();
        assert_eq!(ids, vec!["iss-1".to_string()], "the My Work view selects only the assigned issue");
    }

    /// **`inbox list` (the default view) is the unified ONE inbox** — all three rows for the
    /// principal, recipient-scoped.
    #[test]
    fn inbox_list_default_is_the_one_inbox() {
        let inbox = seeded("u1");
        let page = inbox_list(&inbox, &principal("u1"), CliView::All, &Page::default(), &AllowAllAuthorize, &strong());
        assert_eq!(page.items.len(), 3, "the default view is the ONE inbox (all rows for u1)");
    }

    /// **`inbox show <id>` returns the item's PII-free detail — refs/tokens, never a rendered
    /// string.** The detail carries the subject REF (humanise is NOTIF-P9), not a title.
    #[test]
    fn inbox_show_returns_refs_not_payloads() {
        let inbox = seeded("u1");
        let show = inbox_show(&inbox, &principal("u1"), "iss-1", &AllowAllAuthorize, &strong()).unwrap();
        assert_eq!(show.item_id, "iss-1");
        assert_eq!(show.reason, "assigned", "the reason is the snake_case token (PII-free)");
        assert_eq!(show.class, "direct");
        assert_eq!(show.subject, "myelin://acme/issue/issue/PROJ-1", "the subject is a REF, never a title");
        assert_eq!(show.state, "unread");
    }

    /// **`inbox show` can NEVER reveal an item `list` would not** — it reads through the same
    /// authorize seam. A denied item is `None` from `show` (no back-door read), and another
    /// principal's item is `None` (recipient scope).
    #[test]
    fn inbox_show_obeys_authorize_and_recipient_scope_no_backdoor() {
        struct DenyAll;
        impl ReadAuthorizePort for DenyAll {
            fn can_read(&self, _v: &Principal, _s: &ArtifactRef, _a: &Consistency) -> Decision {
                Decision::Deny
            }
        }
        let inbox = seeded("u1");
        // a denied item is not shown (held, not leaked — same as list).
        assert!(inbox_show(&inbox, &principal("u1"), "iss-1", &DenyAll, &strong()).is_none(), "denied → not shown");
        // another principal cannot show u1's item (recipient scope).
        assert!(inbox_show(&inbox, &principal("u2"), "iss-1", &AllowAllAuthorize, &strong()).is_none(), "not my item → not shown");
        // a missing id is None.
        assert!(inbox_show(&inbox, &principal("u1"), "no-such", &AllowAllAuthorize, &strong()).is_none());
    }

    /// **`inbox read <id>` marks the caller's item read (the ONE read-state truth), recipient-
    /// scoped.** After `read`, `show` reports `read`. u2 cannot read u1's item (NotFound).
    #[test]
    fn inbox_read_marks_read_recipient_scoped() {
        let inbox = seeded("u1");
        inbox_read(&inbox, &principal("u1"), "iss-1").expect("read my own item");
        let show = inbox_show(&inbox, &principal("u1"), "iss-1", &AllowAllAuthorize, &strong()).unwrap();
        assert_eq!(show.state, "read", "the item is read after `inbox read`");
        // u2 cannot read u1's item.
        assert_eq!(inbox_read(&inbox, &principal("u2"), "iss-1"), Err(crate::read_state::ReadStateError::NotFound));
        // a missing id is NotFound.
        assert_eq!(inbox_read(&inbox, &principal("u1"), "no-such"), Err(crate::read_state::ReadStateError::NotFound));
    }

    /// **`inbox snooze <id> --until <ts>` records the until + parks the item.** After snooze, `show`
    /// reports `snoozed`; the active inbox no longer contains it.
    #[test]
    fn inbox_snooze_records_until_and_parks() {
        let inbox = seeded("u1");
        inbox_snooze(&inbox, &principal("u1"), "chat-1", "2026-06-25T09:00:00Z").expect("snooze my own item");
        let show = inbox_show(&inbox, &principal("u1"), "chat-1", &AllowAllAuthorize, &strong()).unwrap();
        assert_eq!(show.state, "snoozed", "the item is snoozed after `inbox snooze`");
        // suppressed from the active inbox.
        let page = inbox_list(&inbox, &principal("u1"), CliView::All, &Page::default(), &AllowAllAuthorize, &strong());
        let active = crate::read_state::active_inbox(page.items);
        assert!(!active.iter().any(|r| r.item_id == "chat-1"), "the snoozed item is absent from the active inbox");
    }

    /// **`render_list` renders PII-free lines (item_id + reason/class token + subject ref + state),
    /// never a rendered string.** The output carries the ref, never a humanised title (P9).
    #[test]
    fn render_list_is_pii_free_lines() {
        let inbox = seeded("u1");
        let page = inbox_list(&inbox, &principal("u1"), CliView::All, &Page::default(), &AllowAllAuthorize, &strong());
        let out = render_list(&page);
        assert!(out.contains("iss-1  [assigned/direct]  myelin://acme/issue/issue/PROJ-1  (unread)"));
        // the subject is a ref, never a title — there is no humanised string in the CLI output.
        assert!(out.contains("myelin://acme/git/pr/9"));
    }
}
