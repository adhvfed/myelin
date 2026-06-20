//! # `list_inbox` — the ONE inbox + the scoped-view filter grammar (the C-9 invariant) (NOTIF-P5 / P-183, M2)
//!
//! **Owning architecture doc:** `notifications.md` §1.3 (the C-9 resolution — there is exactly ONE
//! cross-subsystem inbox; Issues **"My Work"**, Chat **"Activity/Mentions"**, Git **"Review
//! requests"** are *scoped, filtered queries INTO this one inbox*, each a `filter` over the item's
//! structured `reason` + `subject`, **never a second store** — one store → one read-state truth),
//! §1.4 (agents have inboxes too — an agent is a `Principal`), §3.4 step-0 (AUTHORIZE: a
//! notification is a *read* of the subject on the recipient's behalf; it obeys `check` exactly —
//! ADR-03, never leak). **Contracts:** **7.1** `list_inbox(principal, filter?, page?) → [InboxItem]`
//! (owned), **4.2** `check` (consumed, the step-0 read authorize), **4.10** zookie (consumed, the
//! consistency token a security-sensitive read carries). **External insight:**
//! `01-process-and-quality-doctrine.md` §3 (prove-it — the C-9 invariant test forces the "a view is
//! a subset" property), §5 (an uncommitted contract test is no contract test).
//!
//! ## What this prompt (NOTIF-P5) ships — the read surface + the filter grammar, nothing else
//!
//! 1. **`list_inbox(principal, filter?, page?)` — the ONE inbox.** It reads the SAME
//!    [`InboxProjection`](crate::router::InboxProjection) the router (NOTIF-P3) UPSERTs into (no
//!    second store): it selects the rows whose `recipient` is the calling `principal`, applies the
//!    optional [`InboxFilter`] (the C-9 scoped-view grammar), runs **step-0 read authorize** over
//!    each candidate's `subject` (an item the recipient cannot see is NOT returned, ADR-03), and
//!    returns the survivors in a **stable order** with a page [`Cursor`].
//!
//! 2. **The scoped-view filter grammar (the C-9 invariant).** [`InboxFilter`] is a filter over
//!    `subsystem` (derived from the item's `subject` `ArtifactRef`) **and** `reason` — never a
//!    second store. The three frozen platform views ([`InboxFilter::issues_my_work`],
//!    [`InboxFilter::chat_activity`], [`InboxFilter::git_review_requests`]) are exactly the §1.3
//!    table. **A subsystem that wants its own "my X" surface adds a filtered view, never a second
//!    store** — proven by the C-9 invariant test (every view's rows ⊆ `list_inbox(filter=∅)`).
//!
//! ## FLOORS named (this read surface is NOT the ranked inbox)
//!
//! - **Ranking is NOTIF-P7.** Here items return in a **stable, deterministic order** (the
//!   unranked-but-stable order: `(occurred-tiebreak via dedup_key, item_id)`), with the page cursor.
//!   The deterministic explainable priority-0..100 ranking layers in as the ORDERING in NOTIF-P7 —
//!   the function plugs into this same `list_inbox` body. Named so the read surface is not mistaken
//!   for the ranked inbox.
//! - **The durable OLTP `SELECT … WHERE tenant=$1 AND recipient=$2` over the `notif_inbox_item`
//!   table** (behind the in-memory [`InboxProjection`]) is the substrate floor (P-007 / P-S12 — the
//!   OLTP client wiring into `serve`); the in-memory projection models exactly that read
//!   (tenant-scoped, recipient-scoped), and the filter/authorize/order logic is byte-identical when
//!   the read moves to SQL (the filter lowers to a `WHERE reason = ANY(...)` + the authorize lowers
//!   to the `list_objects` `SetExpr` JOIN, contract 4.3 — the read-fanout push-down is NOTIF-P13).
//! - **read-state (`mark`/`snooze`/`mark_all_read`) is NOTIF-P6**; **prefs/quiet-hours** NOTIF-P10;
//!   **the inbox `watch` live transport** NOTIF-P15. This is the read surface only.
//!
//! ## Mutation floor (the list-inbox module — mandatory-core)
//! `list_inbox` is mandatory-core (the platform's ONE read surface). The mutation-tested core is the
//! decision logic: the `recipient`-scoping (an item NOT for the principal is not returned), the
//! `InboxFilter::matches` predicate (subsystem ∧ reason — the C-9 grammar), the
//! [`subsystem_of`]-from-`subject` derivation, the step-0 authorize gate (a denied `check` drops the
//! item — never leaked, ADR-03), and the stable order + page slice. **Floor: ≥ 80% line/branch
//! mutation score on `list_inbox.rs`** (measured with `cargo mutants`; reported in the P-183 commit
//! body). The floor is **stated and met** by the unit + chained + CDC tests: every view is asserted
//! a subset, an unauthorized item is asserted dropped, a not-for-me item is asserted excluded, and a
//! mutant that widens a filter, skips the authorize, mis-derives the subsystem, breaks the recipient
//! scope, or dangles the page cursor is caught.
//!
//! **Measured (P-183):** `cargo mutants --file crates/myelin-notif/src/list_inbox.rs` → 24 mutants,
//! **20 caught / 1 missed / 3 unviable** = **95.2% on the 21 viable** (≥ 80% floor MET). The single
//! miss is the **provably-equivalent** mutant `InboxFilter::all -> Default::default()` — `all()` IS
//! defined as `InboxFilter::default()`, so the two are byte-identical and no test can distinguish
//! them (an equivalent mutant, not a coverage gap).

use std::collections::HashSet;

use myelin_events::ArtifactRef;
use myelin_identity::{Consistency, Decision, Principal};

use crate::router::{InboxProjection, RoutedInboxItem};
use crate::Reason;

/// The **subsystem** an inbox item belongs to — derived from the item's `subject`
/// [`ArtifactRef`] (`myelin://<tenant>/<subsystem>/<type>/<id>`), the second path segment. The C-9
/// scoped-view filter pins on `subsystem ∧ reason` (§1.3): Issues "My Work" is `subsystem∈{issue}`,
/// Chat "Activity" is `subsystem∈{chat}`, Git "Review requests" is `subsystem∈{git}`. This is NOT a
/// stored column on the row — it is *derived from the ref* (references-not-payloads, NOTIF-1), so a
/// view stays a filter over the structured `subject`, never a second store.
///
/// An unknown / malformed ref derives [`Subsystem::Unknown`] (it is never silently bucketed into a
/// known subsystem — a filter over a known subsystem set must never accidentally admit it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subsystem {
    /// The Issues subsystem (`myelin://<tenant>/issue/...` / `.../issues/...`).
    Issue,
    /// The Chat subsystem (`myelin://<tenant>/chat/...`).
    Chat,
    /// The Git-hosting subsystem (`myelin://<tenant>/git/...`).
    Git,
    /// The Knowledge subsystem (`myelin://<tenant>/kn/...` / `.../knowledge/...`).
    Knowledge,
    /// The CI subsystem (`myelin://<tenant>/ci/...`).
    Ci,
    /// Any other / unrecognised subsystem (the ref's second segment is not a known subsystem). A
    /// view over a KNOWN subsystem set NEVER admits this (the filter is a strict membership test).
    Unknown,
}

/// **Derive the [`Subsystem`] from an item's `subject` [`ArtifactRef`]** — the second path segment
/// of `myelin://<tenant>/<subsystem>/<type>/<id>`. The C-9 filter pins on this *derived* value, so
/// the scoped view is a filter over the structured `subject` (references-not-payloads), never a
/// second store. A malformed / unknown ref → [`Subsystem::Unknown`] (never silently a known
/// subsystem — a filter over `{issue}` must not accidentally admit a `myelin://acme/foo/...` ref).
pub fn subsystem_of(subject: &ArtifactRef) -> Subsystem {
    // myelin://<tenant>/<subsystem>/<type>/<id> — strip the scheme, take the segment AFTER tenant.
    let rest = match subject.0.strip_prefix("myelin://") {
        Some(r) => r,
        None => return Subsystem::Unknown,
    };
    // segments: [tenant, subsystem, type, id, ...] — the subsystem is the SECOND segment.
    let mut segs = rest.split('/');
    let _tenant = segs.next(); // the tenant segment (already the partition key; not the subsystem).
    match segs.next() {
        Some("issue") | Some("issues") => Subsystem::Issue,
        Some("chat") => Subsystem::Chat,
        Some("git") => Subsystem::Git,
        Some("kn") | Some("knowledge") => Subsystem::Knowledge,
        Some("ci") => Subsystem::Ci,
        _ => Subsystem::Unknown,
    }
}

/// **The scoped-view filter grammar (the C-9 invariant).** A filter over `subsystem` (derived from
/// `subject`) **and** `reason` — never a second store. `subsystems = None` means "any subsystem";
/// `reasons = None` means "any reason"; the empty/`None` filter ([`InboxFilter::all`]) is the
/// canonical unfiltered inbox. A view is a STRICT SUBSET of the unfiltered inbox by construction:
/// [`InboxFilter::matches`] only ever NARROWS (it never adds a row), so every view's rows ⊆
/// `list_inbox(filter=∅)` — the property the C-9 invariant test forces.
///
/// The three frozen platform views (§1.3) are the constructors [`InboxFilter::issues_my_work`],
/// [`InboxFilter::chat_activity`], [`InboxFilter::git_review_requests`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxFilter {
    /// The subsystem set the view admits (`None` = any subsystem). A membership test over the
    /// *derived* [`subsystem_of`]`(subject)` — never a second store.
    pub subsystems: Option<HashSet<Subsystem>>,
    /// The reason set the view admits (`None` = any reason). A membership test over the structured
    /// `reason` (the §1.3 reason filter).
    pub reasons: Option<HashSet<Reason>>,
}

impl InboxFilter {
    /// The empty filter — the canonical unfiltered ONE inbox (`filter = ∅`, §1.3). Every scoped
    /// view's rows are a subset of THIS.
    pub fn all() -> InboxFilter {
        InboxFilter::default()
    }

    /// Issues **"My Work"** (§1.3): `subsystem∈{issue} ∧ reason∈{assigned, mentioned,
    /// review_requested, sla, watched, blocked, approval_requested}`. A *view*, not a store.
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

    /// Chat **"Activity / Mentions"** (§1.3): `subsystem∈{chat} ∧ reason∈{mentioned, replied,
    /// thread_watched, approval_requested}`. A *view*, not a store.
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

    /// Git **"Review requests"** (§1.3): `subsystem∈{git} ∧ reason∈{review_requested, mentioned}`.
    /// A *view*, not a store.
    pub fn git_review_requests() -> InboxFilter {
        InboxFilter {
            subsystems: Some([Subsystem::Git].into_iter().collect()),
            reasons: Some([Reason::ReviewRequested, Reason::Mentioned].into_iter().collect()),
        }
    }

    /// **Does `item` pass this filter?** A pure NARROWING predicate over the *structured*
    /// `subsystem` (derived from `subject`) ∧ `reason`. `None` on a dimension = "any" (no narrowing
    /// on it). Because it only ever narrows — it NEVER admits a row the unfiltered inbox lacks — a
    /// view's result is a strict subset of `list_inbox(filter=∅)` by construction (the C-9
    /// invariant). This is the load-bearing predicate the mutation floor pins.
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

/// **The step-0 read-authorize port (contract 4.2 — `check`).** `list_inbox` is a *read* of each
/// item's `subject` on the recipient's behalf; it obeys `check` exactly (§3.4 step-0, ADR-03 —
/// never leak). An item whose subject the recipient can no longer see is **not returned** — held,
/// not leaked. A security-sensitive read carries the `zookie` (contract 4.10) so it does not serve
/// from the fail-static cache; the port evaluates the `check` at that consistency snapshot.
///
/// This mirrors the search subsystem's `BoundedCheckPort` seam (a thin trait over `check` so the
/// crate does not link the full `IdentityService`). The body — the real Identity `check` /
/// `list_objects` push-down (the read-fanout JOIN over the `authz_visible` reverse index) — is
/// wired through this port; the **read-fanout watcher resolution** is NOTIF-P13. Here the seam is
/// frozen and a denying check is PROVEN to drop the item.
pub trait ReadAuthorizePort {
    /// Can `viewer` READ `subject` at consistency `at` (contract 4.2 / 4.10)? `Decision::Allow`
    /// ⇒ surface the item; `Decision::Deny` / `Decision::Conditional` ⇒ DROP it (fail-closed,
    /// ADR-03 — a `Conditional` the read path cannot satisfy is treated as a deny: never a silent
    /// leak). The default permission is `read` (a notification is a read of the subject).
    fn can_read(&self, viewer: &Principal, subject: &ArtifactRef, at: &Consistency) -> Decision;
}

/// **A page through the ONE inbox (contract 7.1 `page?`).** A bounded slice over the stable order
/// (NOTIF-P7 ranking plugs into the same order). `after` is the exclusive start cursor (the
/// `item_id` of the last item of the previous page; `None` = the first page); `limit` bounds the
/// page size (so a 50k-item inbox never returns unboundedly — the read is always bounded).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// The exclusive start cursor — return items strictly AFTER this `item_id` in the stable order.
    /// `None` starts at the first item.
    pub after: Option<String>,
    /// The maximum page size (the read is always bounded — never an unbounded `SELECT *`).
    pub limit: usize,
}

impl Default for Page {
    /// The default page: the first page, bounded to 50 items (a sensible inbox page; the read is
    /// never unbounded).
    fn default() -> Page {
        Page { after: None, limit: 50 }
    }
}

/// **The opaque forward cursor a page returns (contract 7.1 `page`).** `Some(item_id)` ⇒ there may
/// be more items after this one (pass it as the next [`Page::after`]); `None` ⇒ the last page (the
/// inbox is exhausted). PII-free (an opaque `item_id`, never a payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor(pub Option<String>);

/// **One page of `list_inbox` results (contract 7.1).** The selected [`RoutedInboxItem`]s in the
/// stable order + the forward [`Cursor`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxPage {
    /// The items on this page (recipient-scoped, filtered, authorized, ordered). Refs-not-payloads.
    pub items: Vec<RoutedInboxItem>,
    /// The forward cursor (`Some` ⇒ more pages; `None` ⇒ exhausted).
    pub cursor: Cursor,
}

/// **`list_inbox(principal, filter, page, authorize, at)` — the ONE inbox (contract 7.1).**
///
/// Reads the SAME [`InboxProjection`] the router (NOTIF-P3) UPSERTs into (no second store) and, in
/// order:
/// 1. **scopes to the recipient** — only rows whose `recipient` is `principal` (an item is in the
///    principal's inbox iff it is addressed to them; an agent is a `Principal` too, §1.4);
/// 2. **applies the C-9 `filter`** — [`InboxFilter::matches`] narrows by `subsystem ∧ reason` (a
///    scoped view; `InboxFilter::all()` = the unfiltered inbox);
/// 3. **step-0 read authorize** — drops any item whose `subject` the recipient cannot `check`-READ
///    at the consistency snapshot `at` (ADR-03, never leak — held, not leaked);
/// 4. **orders stably** — by `(item_id)` (the unranked-but-stable order; the NOTIF-P7 ranking plugs
///    into this same slot), then **pages** the bounded slice.
///
/// Returns the [`InboxPage`] (the items + the forward cursor). The recipient scope + the authorize
/// are the two non-negotiables: a row not addressed to the caller is never returned, and a row the
/// caller cannot see is never leaked.
pub fn list_inbox(
    inbox: &InboxProjection,
    principal: &Principal,
    filter: &InboxFilter,
    page: &Page,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> InboxPage {
    // (1) Recipient scope — the inbox is read per-recipient. The projection snapshot is already
    // tenant-scoped (the model of `WHERE tenant = $1`); we narrow to `recipient = principal`.
    let me = principal.principal_id.0.as_str();
    let mut candidates: Vec<RoutedInboxItem> = inbox
        .snapshot_for_tenant(&principal.tenant)
        .into_iter()
        .filter(|row| row.recipient == me)
        // (2) The C-9 scoped-view filter — narrows by subsystem ∧ reason (a view, not a store).
        .filter(|row| filter.matches(row))
        // (3) Step-0 read authorize — a denied/conditional check DROPS the item (ADR-03, never
        // leak). A notification is a read of the subject on the recipient's behalf (§3.4 step-0).
        .filter(|row| authorize.can_read(principal, &row.subject, at) == Decision::Allow)
        .collect();

    // (4) Stable order — by item_id (the unranked-but-stable order; the NOTIF-P7 ranking plugs in
    // here). Deterministic so paging is consistent across calls (no random/HashMap order leaks).
    candidates.sort_by(|a, b| a.item_id.cmp(&b.item_id));

    // Page: skip past the `after` cursor (exclusive), take `limit`, and compute the forward cursor.
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
    // The forward cursor: Some(last item_id) iff there are more rows after this page.
    let cursor = if end < candidates.len() {
        Cursor(items.last().map(|row| row.item_id.clone()))
    } else {
        Cursor(None)
    };
    InboxPage { items, cursor }
}

/// **A permissive read-authorize port (every read ALLOWED).** The seam Notif uses until the live
/// Identity `check` client is wired into `serve` (P-007 / P-S12) — and the port a single-cell
/// self-host with no per-item ACL narrowing uses. It is the IDENTITY seam shape; it is NOT a
/// security bypass: the production wiring substitutes the real `check` resolver behind the SAME
/// [`ReadAuthorizePort`]. Named explicitly so a deployment never mistakes it for the enforced path.
///
/// **Floor:** the live `check` / `list_objects` push-down (the read-fanout JOIN over the
/// `authz_visible` reverse index, contract 4.3/4.4) is NOTIF-P13; here the SEAM is frozen and the
/// denying-check drop is proven (see the `denied_item_is_not_returned` test) against a denying port.
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
        Consistency { at_least: Zookie("zk-1".into()), mode: ConsistencyMode::Strong }
    }

    /// Build a routed inbox row addressed to `recipient`, about `subject`, with `reason`.
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
        }
    }

    /// Seed a projection with a mixed batch addressed to `me` across the three subsystems.
    fn seeded_inbox(me: &str) -> InboxProjection {
        let inbox = InboxProjection::new();
        // Issues — in "My Work" (assigned) + one NOT in it (state_changed).
        inbox.upsert_for_test(item(me, "itm-iss-assigned", "myelin://acme/issue/issue/PROJ-1", Reason::Assigned));
        inbox.upsert_for_test(item(me, "itm-iss-state", "myelin://acme/issue/issue/PROJ-2", Reason::StateChanged));
        // Chat — in "Activity" (mentioned) + one NOT (state_changed).
        inbox.upsert_for_test(item(me, "itm-chat-ment", "myelin://acme/chat/thread/T1", Reason::Mentioned));
        inbox.upsert_for_test(item(me, "itm-chat-state", "myelin://acme/chat/thread/T2", Reason::StateChanged));
        // Git — in "Review requests" (review_requested) + one NOT (watched).
        inbox.upsert_for_test(item(me, "itm-git-review", "myelin://acme/git/pr/9", Reason::ReviewRequested));
        inbox.upsert_for_test(item(me, "itm-git-watched", "myelin://acme/git/pr/10", Reason::Watched));
        inbox
    }

    fn ids(page: &InboxPage) -> BTreeSet<String> {
        page.items.iter().map(|i| i.item_id.clone()).collect()
    }

    // --- subsystem derivation (the C-9 filter pins on the DERIVED subsystem, refs-not-payloads) ---

    /// **`subsystem_of` derives the subsystem from the second ref segment** — and a malformed /
    /// unknown ref is `Unknown` (NEVER silently a known subsystem). A mutant that mis-buckets a ref
    /// or defaults an unknown ref into a known subsystem is caught.
    #[test]
    fn subsystem_is_derived_from_the_subject_ref_unknown_is_not_a_known_subsystem() {
        assert_eq!(subsystem_of(&ArtifactRef("myelin://acme/issue/issue/PROJ-1".into())), Subsystem::Issue);
        assert_eq!(subsystem_of(&ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())), Subsystem::Issue);
        assert_eq!(subsystem_of(&ArtifactRef("myelin://acme/chat/thread/T1".into())), Subsystem::Chat);
        assert_eq!(subsystem_of(&ArtifactRef("myelin://acme/git/pr/9".into())), Subsystem::Git);
        assert_eq!(subsystem_of(&ArtifactRef("myelin://acme/kn/doc/D1".into())), Subsystem::Knowledge);
        assert_eq!(subsystem_of(&ArtifactRef("myelin://acme/ci/run/42".into())), Subsystem::Ci);
        // an unknown subsystem / a malformed ref → Unknown (never silently a known one).
        assert_eq!(subsystem_of(&ArtifactRef("myelin://acme/mystery/x/1".into())), Subsystem::Unknown);
        assert_eq!(subsystem_of(&ArtifactRef("not-a-ref".into())), Subsystem::Unknown);
        assert_eq!(subsystem_of(&ArtifactRef("myelin://acme".into())), Subsystem::Unknown);
    }

    // --- THE C-9 INVARIANT: every scoped view ⊆ the unfiltered inbox (a view is a filter) ---

    /// **THE C-9 INVARIANT (the gate): every scoped view returns a STRICT SUBSET of
    /// `list_inbox(filter=∅)` — a view is a filter, not a store.** For each of the three frozen
    /// views: every row in the view is also in the unfiltered inbox, and 0 rows in the view are
    /// absent from it. This is the load-bearing property the prompt names.
    #[test]
    fn c9_invariant_every_view_is_a_strict_subset_of_the_unfiltered_inbox() {
        let me = "u1";
        let inbox = seeded_inbox(me);
        let p = principal(me);
        let big = Page { after: None, limit: 1000 };

        let full = list_inbox(&inbox, &p, &InboxFilter::all(), &big, &AllowAllAuthorize, &strong());
        let full_ids = ids(&full);
        assert_eq!(full_ids.len(), 6, "the unfiltered inbox is the ONE inbox (all 6 rows for u1)");

        for view in [
            InboxFilter::issues_my_work(),
            InboxFilter::chat_activity(),
            InboxFilter::git_review_requests(),
        ] {
            let v = list_inbox(&inbox, &p, &view, &big, &AllowAllAuthorize, &strong());
            let view_ids = ids(&v);
            // STRICT SUBSET: every view row is in the unfiltered inbox; 0 rows absent from it.
            assert!(
                view_ids.is_subset(&full_ids),
                "C-9: the view {view:?} is a SUBSET of the unfiltered inbox"
            );
            assert!(!view_ids.is_empty(), "the seeded batch put ≥1 row in every view");
            assert!(view_ids.len() < full_ids.len(), "a scoped view is STRICTLY smaller than the ONE inbox");
        }
    }

    /// **Each frozen view selects EXACTLY its §1.3 rows** (the filter narrows correctly — it does
    /// not over- or under-select). Issues "My Work" = the assigned issue (not the state-changed
    /// one); Chat "Activity" = the mention; Git "Review requests" = the review request.
    #[test]
    fn the_three_frozen_views_select_exactly_their_rows() {
        let me = "u1";
        let inbox = seeded_inbox(me);
        let p = principal(me);
        let big = Page { after: None, limit: 1000 };

        let my_work = ids(&list_inbox(&inbox, &p, &InboxFilter::issues_my_work(), &big, &AllowAllAuthorize, &strong()));
        assert!(my_work.contains("itm-iss-assigned"), "assigned is in My Work");
        assert!(!my_work.contains("itm-iss-state"), "a state_changed issue is NOT in My Work");
        // a chat row never leaks into the issues view (the subsystem clause is real).
        assert!(!my_work.contains("itm-chat-ment"), "a chat mention is NOT in the Issues view (subsystem clause)");

        let activity = ids(&list_inbox(&inbox, &p, &InboxFilter::chat_activity(), &big, &AllowAllAuthorize, &strong()));
        assert_eq!(activity, ["itm-chat-ment".to_string()].into_iter().collect());

        let reviews = ids(&list_inbox(&inbox, &p, &InboxFilter::git_review_requests(), &big, &AllowAllAuthorize, &strong()));
        assert_eq!(reviews, ["itm-git-review".to_string()].into_iter().collect());
    }

    /// **The `InboxFilter::matches` predicate only ever NARROWS** — the all-filter matches every
    /// row; a view-filter matches a strict subset. (The structural basis of the C-9 subset
    /// property: a mutant that makes a filter ADMIT a non-matching row is caught.)
    #[test]
    fn filter_matches_only_narrows() {
        let issue_assigned = item("u1", "a", "myelin://acme/issue/issue/PROJ-1", Reason::Assigned);
        let chat_mention = item("u1", "b", "myelin://acme/chat/thread/T1", Reason::Mentioned);
        // the all-filter admits everything.
        assert!(InboxFilter::all().matches(&issue_assigned));
        assert!(InboxFilter::all().matches(&chat_mention));
        // My Work admits the issue-assigned, rejects the chat (wrong subsystem).
        assert!(InboxFilter::issues_my_work().matches(&issue_assigned));
        assert!(!InboxFilter::issues_my_work().matches(&chat_mention), "wrong subsystem → rejected");
        // a right-subsystem / wrong-reason row is rejected (the reason clause bites).
        let issue_state = item("u1", "c", "myelin://acme/issue/issue/PROJ-2", Reason::StateChanged);
        assert!(!InboxFilter::issues_my_work().matches(&issue_state), "wrong reason → rejected");
    }

    // --- recipient scope: an item NOT for me is never in my inbox ---

    /// **`list_inbox` is recipient-scoped: an item addressed to ANOTHER principal is never
    /// returned.** A mutant that drops the recipient filter (returning the whole tenant's inbox) is
    /// caught — this is a confidentiality non-negotiable (you never see someone else's inbox).
    #[test]
    fn list_inbox_is_recipient_scoped_others_items_are_not_returned() {
        let inbox = InboxProjection::new();
        inbox.upsert_for_test(item("u1", "mine", "myelin://acme/issue/issue/P1", Reason::Assigned));
        inbox.upsert_for_test(item("u2", "theirs", "myelin://acme/issue/issue/P2", Reason::Assigned));
        let page = list_inbox(&inbox, &principal("u1"), &InboxFilter::all(), &Page::default(), &AllowAllAuthorize, &strong());
        let got = ids(&page);
        assert!(got.contains("mine"), "my item is returned");
        assert!(!got.contains("theirs"), "another principal's item is NEVER in my inbox (recipient scope)");
        assert_eq!(got.len(), 1);
    }

    // --- step-0 authorize: a denied item is held, not leaked (ADR-03) ---

    /// A read-authorize port that DENIES exactly the subjects in its deny-set (else allows). The
    /// seam shape the live Identity `check` plugs into.
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

    /// **Step-0 authorize (ADR-03): an item the recipient cannot `check`-READ is NOT returned —
    /// held, not leaked.** A denying port drops exactly the unseeable item; the rest surface. A
    /// mutant that skips the authorize (leaks the denied item) is caught — the non-negotiable.
    #[test]
    fn denied_item_is_not_returned() {
        let me = "u1";
        let inbox = seeded_inbox(me);
        let deny = DenySubjects(["myelin://acme/issue/issue/PROJ-1".to_string()].into_iter().collect());
        let big = Page { after: None, limit: 1000 };
        let page = list_inbox(&inbox, &principal(me), &InboxFilter::all(), &big, &deny, &strong());
        let got = ids(&page);
        assert!(!got.contains("itm-iss-assigned"), "the denied subject's item is HELD, not leaked (ADR-03)");
        assert_eq!(got.len(), 5, "the other 5 visible items surface");
    }

    /// **A `Conditional` decision the read path cannot satisfy is treated as a DENY (fail-closed).**
    /// list_inbox surfaces iff `check` ALLOWS — never on a `Conditional`/`Deny` (deny-when-unsure).
    #[test]
    fn conditional_check_is_failclosed_not_leaked() {
        struct AlwaysConditional;
        impl ReadAuthorizePort for AlwaysConditional {
            fn can_read(&self, _v: &Principal, _s: &ArtifactRef, _at: &Consistency) -> Decision {
                Decision::Conditional
            }
        }
        let inbox = seeded_inbox("u1");
        let page = list_inbox(&inbox, &principal("u1"), &InboxFilter::all(), &Page::default(), &AlwaysConditional, &strong());
        assert!(page.items.is_empty(), "a Conditional check is fail-closed (deny-when-unsure, ADR-03)");
    }

    // --- stable order + paging ---

    /// **The order is stable + deterministic, and paging walks it exactly once (no dup, no skip).**
    /// Two pages of limit 2 over 6 items + a final page; the cursor chains; the union is the full
    /// ordered set with no overlap. A mutant that breaks the order or the cursor math is caught.
    #[test]
    fn stable_order_and_paging_is_exhaustive_and_non_overlapping() {
        let me = "u1";
        let inbox = seeded_inbox(me);
        let p = principal(me);

        let mut seen: Vec<String> = Vec::new();
        let mut after: Option<String> = None;
        // A hard iteration guard: 6 items at limit 2 is ≤ 4 pages. A mutant that breaks the cursor
        // math (a phantom next page / a stuck cursor) trips this BOUND instead of hanging forever —
        // so the cursor-arithmetic mutants surface as a FAST assertion failure, not a 20s timeout.
        let mut guard = 0;
        loop {
            guard += 1;
            assert!(guard <= 8, "paging must terminate within the page bound (the cursor advances)");
            let page = list_inbox(
                &inbox,
                &p,
                &InboxFilter::all(),
                &Page { after: after.clone(), limit: 2 },
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
        // exhaustive: all 6 items, in stable (sorted item_id) order, no dup.
        let mut expected = ids(&list_inbox(&inbox, &p, &InboxFilter::all(), &Page { after: None, limit: 1000 }, &AllowAllAuthorize, &strong()))
            .into_iter()
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(seen.len(), 6, "paging visited every item once (no skip)");
        let unique: BTreeSet<_> = seen.iter().cloned().collect();
        assert_eq!(unique.len(), 6, "paging never returned a duplicate");
        assert_eq!(seen, expected, "the page order is the stable (sorted) order");
    }

    /// **A page is bounded by `limit` and reports a forward cursor iff more rows remain.** The first
    /// page of limit 2 over 6 items returns 2 items + a `Some` cursor; the read is never unbounded.
    #[test]
    fn page_is_bounded_and_reports_more() {
        let inbox = seeded_inbox("u1");
        let page = list_inbox(
            &inbox,
            &principal("u1"),
            &InboxFilter::all(),
            &Page { after: None, limit: 2 },
            &AllowAllAuthorize,
            &strong(),
        );
        assert_eq!(page.items.len(), 2, "the page is bounded to the limit (never unbounded)");
        assert!(page.cursor.0.is_some(), "there are more rows → a forward cursor");
    }

    /// **A page that EXACTLY exhausts the inbox reports NO forward cursor** (`end == len` → `None`,
    /// not a dangling cursor onto an empty next page). The 6-item inbox read with limit 6 returns
    /// all 6 and a `None` cursor; a limit of exactly the remaining count is the last page. A mutant
    /// that loosens the cursor's `end < len` to `end <= len` (a phantom next page) is caught.
    #[test]
    fn page_that_exactly_exhausts_reports_no_cursor() {
        let inbox = seeded_inbox("u1"); // 6 items for u1.
        // limit exactly == the item count → the last page, no dangling cursor.
        let page = list_inbox(
            &inbox,
            &principal("u1"),
            &InboxFilter::all(),
            &Page { after: None, limit: 6 },
            &AllowAllAuthorize,
            &strong(),
        );
        assert_eq!(page.items.len(), 6, "all 6 items on the page");
        assert_eq!(page.cursor, Cursor(None), "an exactly-exhausting page has NO forward cursor (end == len)");

        // and a SECOND page after the last item is empty with no cursor (the cursor did not dangle).
        let last_id = page.items.last().unwrap().item_id.clone();
        let next = list_inbox(
            &inbox,
            &principal("u1"),
            &InboxFilter::all(),
            &Page { after: Some(last_id), limit: 6 },
            &AllowAllAuthorize,
            &strong(),
        );
        assert!(next.items.is_empty(), "no items after the last one");
    }

    /// **An empty inbox / a principal with no items returns an empty page with no cursor.** The
    /// edge: `list_inbox` over a principal with zero rows is `{items: [], cursor: None}`.
    #[test]
    fn empty_inbox_returns_empty_page_no_cursor() {
        let inbox = InboxProjection::new();
        let page = list_inbox(&inbox, &principal("nobody"), &InboxFilter::all(), &Page::default(), &AllowAllAuthorize, &strong());
        assert!(page.items.is_empty());
        assert_eq!(page.cursor, Cursor(None), "no more rows → no cursor");
    }

    /// **The AllowAll authorize port allows every read** (the documented non-bypass seam). The
    /// production wiring substitutes the real `check` resolver behind the SAME port.
    #[test]
    fn allow_all_authorize_allows() {
        let port = AllowAllAuthorize;
        assert_eq!(
            port.can_read(&principal("u1"), &ArtifactRef("myelin://acme/issue/issue/P1".into()), &strong()),
            Decision::Allow
        );
    }
}
