//! # `chat_fragment` — Id's compiled **Chat** ReBAC namespace fragment (contract 4.9,
//! P-ID-30 → P-323)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §5 (the **frozen Chat fragment**: `channel`, `message`, `unfurl`;
//! `channel.read = member + parent_project->read`; `message.view = parent_channel->read`;
//! the per-viewer permission-aware **unfurl** is a Refs concern — Refs asks Id
//! `check(viewer, view, target)` per unfurl target, so an unfurl of a confidential issue degrades to a
//! **tombstone** for a viewer lacking `issue.view` — unfurls cannot leak),
//! §7.3 (the Chat **via_column** = `channel.id` / `message.id` — the ambient channel-list conjoin
//! column the `list_objects` `Filter` JOINs against, one query, no N+1),
//! §7.5 (the **50k-member-channel `list_subjects(channel, watcher)` density** — Notif's read-fanout,
//! served by the SAME S8 reverse index; the `watcher` relation is what makes it an ordinary Expand).
//!
//! **Contract-index rows:** **4.9** (the Chat fragment — OWNED here), **4.3** (the ambient channel-list
//! conjoin — consumed via [`crate::list_objects`]: `list_objects(subject, read, channel)` keyed on
//! `channel.id`), **4.4** (the channel watcher density — consumed via
//! [`crate::StoreBackedCheck::list_watchers_in`], the one `list_subjects(channel, watcher)` Expand).
//!
//! This is the **FIFTH and FINAL of the five per-subsystem fragments** (P-ID-24/26/27/29/30) that
//! promote the M1 engine-only floor (P-068): Git is [`crate::git_fragment`], Knowledge is
//! [`crate::knowledge_fragment`], CI is [`crate::ci_fragment`], Issues is [`crate::issue_fragment`].
//! **With the Chat fragment admitted, the M1 engine-only floor is CLOSED — all five subsystem
//! fragments now exist in the engine.** Like the other four it is the canonical **rich**
//! [`crate::namespace::FragmentDef`] declaration of the Chat authz vocabulary, with the permission
//! **rewrites** wired over the Zanzibar userset operators so `check`/`list_objects` resolve the Chat
//! permissions through the SAME engine the core hierarchy uses (one primitive — no bespoke Chat check
//! path, the §5 design rule). The Chat data model (the channel/message tables themselves) is the
//! Chat-subsystem prompts'; this module ships only the Id-side authz content.
//!
//! ## Why the rich fragment lives HERE (not in `myelin-chat`)
//! Same DAG discipline as the other four fragments: `myelin-identity-service` (the engine) does NOT
//! depend on a subsystem leaf crate (§2.9 acyclic DAG). The Chat subsystem declares the **names-only**
//! ABI carrier ([`myelin_identity::NamespaceFragment`]) — the shape Identity's `admit_fragment`
//! consumes at the contract boundary. But the names-only carrier cannot carry the rewrite STRUCTURE
//! (`channel.read = member + parent_project->view`, the `parent_channel->read` inheritance); only the
//! engine's rich `FragmentDef` can. So **Id owns the compiled rewrites** (this module), declared from
//! the architecture §5 frozen vocabulary directly, and the CDC test (`tests/cdc_4_9_chat_fragment.rs`)
//! pins that the two sides agree on the relation/permission NAMES.
//!
//! ## The compiled Chat fragment (§5)
//!
//! | object type | relations                        | permissions (rewrite)                                       |
//! |-------------|----------------------------------|-------------------------------------------------------------|
//! | `channel`   | `parent_project` `member` `watcher` | **`read = member ∪ parent_project->view`** (watchable, C8)  |
//! | `message`   | `parent_channel`                 | **`view = parent_channel->read`**                           |
//! | `unfurl`    | `parent_message` `target`        | **`view = parent_message->view`** (the per-target gate is the Refs `check(viewer, view, target)`; an unfurl of a confidential target degrades to a tombstone) |
//!
//! - **`channel.read = member ∪ parent_project->view` (§5, the headline)** — a channel is readable by a
//!   direct `member` OR anyone who can read the parent project (the ambient project-membership arm). So
//!   `list_objects(subject, read, channel)` push-down keyed on `channel.id` (§7.3) emits exactly the
//!   subject's readable channels; a **non-member's channel/message search returns 0 results** because the
//!   Filter conjoins the channel's ACL and a non-member is in neither arm (the search-requires-acl-filter
//!   leak gate). The channel is **watchable** (C8) so Notif's `list_subjects(channel, watcher)` read-
//!   fanout at 50k-member density is an ordinary Expand over S8 (§7.5).
//! - **`message.view = parent_channel->read` (§5)** — a message inherits its channel's readability via a
//!   tuple-to-userset rewrite into `channel.read`. A non-member of the channel cannot view its messages
//!   (the inheritance terminates in the `read` arms above), so a message search by a non-member returns
//!   0 results by construction.
//! - **`unfurl` + the tombstone degradation (§5, the chat-unfurl drill)** — an unfurl is a per-message
//!   render of a `target` (an issue / page / PR link). The unfurl's OWN visibility inherits the message's
//!   (`unfurl.view = parent_message->view`), but the **per-viewer permission-aware render** is a Refs
//!   concern: Refs asks Id `check(viewer, view, target)` per unfurl target, and an unfurl of a
//!   **confidential** target (e.g. an `issue` the viewer lacks `issue.view` on) **degrades to a
//!   tombstone** — 0 title leak. The `target` relation is declared so the unfurl carries which object it
//!   points at as an ordinary tuple; Identity never recomputes the target's ACL — it runs the SAME
//!   `check` over the target's own fragment (the confidential-issue exclusion, [`crate::issue_fragment`]).

use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{ObjectType, Permission, RelName};

/// The three frozen Chat object-type names (§5; mirrors the Chat subsystem's names-only carrier).
/// Public so the CDC test + a live-wiring caller reference the SAME canonical strings.
pub mod object_types {
    /// The channel — the root Chat authz object (§5 `definition channel`). Watchable (C8).
    pub const CHANNEL: &str = "channel";
    /// A message in a channel — inherits the channel's readability (§5 `definition message`).
    pub const MESSAGE: &str = "message";
    /// A per-message unfurl of a link target — the per-viewer render degrades to a tombstone when the
    /// viewer lacks access to the target (§5; the Refs `check(viewer, view, target)` concern).
    pub const UNFURL: &str = "unfurl";
}

/// **The `member` relation on `channel` (§5).** A direct channel member — the first arm of
/// `channel.read = member ∪ parent_project->view`. Exposed so the CDC + a live caller reference the
/// canonical name, not a stringly-typed literal.
pub const MEMBER: &str = "member";

/// **The `target` relation on `unfurl` (§5).** Which object the unfurl points at (an issue / page / PR),
/// carried as an ordinary tuple so the per-viewer render gate is the ordinary
/// `check(viewer, view, target)` over the TARGET's own fragment — Identity never recomputes the
/// target's ACL.
pub const TARGET: &str = "target";

/// **The `read` permission name on `channel`** — the channel-visibility permission
/// `list_objects(subject, read, channel)` pushes down (keyed on `channel.id`, §7.3) and
/// `check(subject, read, channel)` resolves through `member ∪ parent_project->view`.
pub const READ: &str = "read";

/// **The `view` permission name on `message` / `unfurl`** — `message.view = parent_channel->read`;
/// `unfurl.view = parent_message->view`.
pub const VIEW: &str = "view";

fn rel(n: &str) -> Userset {
    Userset::Relation(RelName(n.into()))
}

fn ttu(tupleset: &str, computed: &str) -> Userset {
    Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    }
}

fn perm(name: &str, rewrite: Userset) -> PermissionRule {
    PermissionRule {
        permission: Permission(name.into()),
        rewrite,
    }
}

fn frag(object_type: &str, relations: &[&str], permissions: Vec<PermissionRule>) -> FragmentDef {
    FragmentDef {
        object_type: ObjectType(object_type.into()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions,
    }
}

/// **The `channel` fragment** (§5 `definition channel`) — the root Chat authz object, the headline
/// rewrite + the watcher-density carrier.
///
/// **`read = member ∪ parent_project->view`** — a channel is readable by a direct `member` OR anyone
/// who can read the parent project (the ambient project-membership arm). The base is the project-read
/// inheritance (`parent_project->view`, the compiled core `project.view`) UNIONed with the direct
/// `member` arm. So `list_objects(subject, read, channel)` emits exactly the subject's readable
/// channels (keyed on `channel.id`, §7.3), and a non-member's channel search returns 0 results (the
/// subject is in neither arm). The channel is **watchable** (C8) so Notif's read-fanout
/// `list_subjects(channel, watcher)` at 50k-member density (§7.5) is an ordinary Expand over S8.
///
/// **Reconciliation (EI-01 §1):** §5 names the inheritance edge `parent_project->read`. The shipped
/// **core hierarchy** ([`crate::namespace::core_hierarchy`]) compiles a single `project.view`
/// permission, so the edge resolves through `parent_project->view` — EXACTLY the same reconciliation
/// [`crate::git_fragment`] / [`crate::issue_fragment`] make (`repo.pull` / `issue.view` both inherit
/// `parent_project->view`). The §5 `read` name denotes the project's read CAPABILITY, which the engine
/// surfaces as the one compiled `project.view`; declaring a second core project permission would fork
/// the core hierarchy, which the engine-only floor forbids.
pub fn channel_fragment() -> FragmentDef {
    frag(
        object_types::CHANNEL,
        &["parent_project", MEMBER, "watcher"],
        vec![
            // read = member ∪ parent_project->view (the §5 channel-visibility rewrite).
            perm(
                READ,
                Userset::Union(vec![rel(MEMBER), ttu("parent_project", "view")]),
            ),
        ],
    )
    .watchable()
}

/// **The `message` fragment** (§5 `definition message`).
///
/// **`view = parent_channel->read`** — a message inherits its channel's readability via a tuple-to-
/// userset rewrite into `channel.read`. A non-member of the channel cannot view its messages (the
/// inheritance terminates in the channel's `read` arms), so a message search by a non-member returns 0
/// results by construction.
pub fn message_fragment() -> FragmentDef {
    frag(
        object_types::MESSAGE,
        &["parent_channel"],
        vec![perm(VIEW, ttu("parent_channel", READ))],
    )
}

/// **The `unfurl` fragment** (§5 `definition unfurl`) — the per-message link-render sub-object.
///
/// **`view = parent_message->view`** — an unfurl's own visibility inherits the message it renders in.
/// But the per-viewer permission-aware render is a Refs concern: Refs asks Id
/// `check(viewer, view, target)` per unfurl `target`, and an unfurl of a **confidential** target
/// degrades to a **tombstone** for a viewer lacking the target's `view` (0 title leak). The `target`
/// relation carries which object the unfurl points at, so that render gate is an ordinary `check` over
/// the TARGET's own fragment (e.g. the confidential-issue exclusion) — Identity never recomputes the
/// target's ACL.
pub fn unfurl_fragment() -> FragmentDef {
    frag(
        object_types::UNFURL,
        &["parent_message", TARGET],
        vec![perm(VIEW, ttu("parent_message", VIEW))],
    )
}

/// **The complete compiled Chat ReBAC namespace fragment (contract 4.9)** — the three rich
/// [`FragmentDef`]s Identity admits into the one cell schema, in parent-before-child order (`channel` →
/// `message` → `unfurl`) so each child's inheritance edge has its parent type already in the schema
/// when it admits. This is the SINGLE entry point [`crate::StoreBackedCheck::admit_chat_fragment`] and
/// the CDC test consume.
pub fn chat_fragment_defs() -> Vec<FragmentDef> {
    vec![channel_fragment(), message_fragment(), unfurl_fragment()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::NamespaceEngine;
    use myelin_identity::FragmentAdmit;

    /// **The compiled Chat fragment admits into the cell schema (the engine-only-floor closure).**
    /// Every Chat object type admits on top of the core org/team/project hierarchy; the three types
    /// enter the compiled vocabulary; `channel.read` + the sub-object permissions resolve as compiled
    /// permissions.
    #[test]
    fn chat_fragment_admits_into_the_cell_schema() {
        let mut eng = NamespaceEngine::with_core_hierarchy();
        for def in chat_fragment_defs() {
            let admit = eng.admit(&def);
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the Chat `{}` fragment admits into the cell schema: {admit:?}",
                def.object_type.0
            );
        }
        for ty in ["channel", "message", "unfurl"] {
            assert!(
                eng.object_types().contains(&ty.to_string()),
                "`{ty}` is admitted"
            );
        }
        assert!(
            eng.resolve_permission("channel", READ).is_some(),
            "channel.read is a compiled permission"
        );
        assert!(
            eng.resolve_permission("message", VIEW).is_some(),
            "message.view is a compiled permission"
        );
        assert!(
            eng.resolve_permission("unfurl", VIEW).is_some(),
            "unfurl.view is a compiled permission"
        );
    }

    /// **`channel.read = member ∪ parent_project->view` (§5, the headline).** The rewrite MUST union the
    /// direct `member` arm with the project-read inheritance — a mutation dropping either arm (e.g.
    /// member-only, severing the ambient project arm; or project-only, severing direct membership) is
    /// caught HERE structurally and behaviourally in the drill.
    #[test]
    fn channel_read_is_member_union_parent_project_view() {
        let channel = channel_fragment();
        let read = channel
            .permissions
            .iter()
            .find(|p| p.permission.0 == READ)
            .expect("channel declares read");
        assert_eq!(
            read.rewrite,
            Userset::Union(vec![rel(MEMBER), ttu("parent_project", "view")]),
            "channel.read = member ∪ parent_project->view (§5)"
        );
    }

    /// **`message.view = parent_channel->read` (§5).** A message inherits its channel's readability via
    /// the tuple-to-userset rewrite into `channel.read` — so a non-member of the channel cannot view its
    /// messages (a message search by a non-member returns 0 results by construction).
    #[test]
    fn message_view_inherits_parent_channel_read() {
        let message = message_fragment();
        let view = message
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .expect("message declares view");
        assert_eq!(
            view.rewrite,
            ttu("parent_channel", READ),
            "message.view = parent_channel->read (§5)"
        );
    }

    /// **`unfurl.view = parent_message->view` (§5).** An unfurl inherits the message it renders in; the
    /// per-viewer permission-aware target render (the tombstone degradation) is the Refs
    /// `check(viewer, view, target)` over the TARGET's own fragment, not a bespoke Chat check. The
    /// `target` relation carries which object the unfurl points at.
    #[test]
    fn unfurl_view_inherits_parent_message_and_carries_target() {
        let unfurl = unfurl_fragment();
        let view = unfurl
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .expect("unfurl declares view");
        assert_eq!(
            view.rewrite,
            ttu("parent_message", VIEW),
            "unfurl.view = parent_message->view (§5)"
        );
        assert!(
            unfurl.relations.iter().any(|r| r.0 == TARGET),
            "unfurl declares the `target` relation (the Refs render-gate edge)"
        );
    }

    /// **`channel` is WATCHABLE (C8): it declares the `watcher` relation** so Notif's read-fanout
    /// `list_subjects(channel, watcher)` at 50k-member density (§7.5) is an ordinary Expand. The
    /// sub-objects (`message`/`unfurl`) are not independently watchable (they inherit the channel's
    /// ACL; a watcher fans out at channel granularity).
    #[test]
    fn channel_is_watchable() {
        assert!(
            channel_fragment().is_watchable(),
            "channel is watchable (C8 — the 50k-density read-fanout)"
        );
        assert!(
            !message_fragment().is_watchable(),
            "message is not independently watchable"
        );
        assert!(
            !unfurl_fragment().is_watchable(),
            "unfurl is not independently watchable"
        );
    }

    /// **No Chat fragment name smuggles an object id (Id never invents object ids).** Every
    /// type/relation/permission name is a bare identifier — the engine's `mints_object_id` admit check
    /// would reject one that wasn't.
    #[test]
    fn no_chat_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in chat_fragment_defs() {
            assert!(
                !mints(&f.object_type.0),
                "type `{}` is a bare identifier",
                f.object_type.0
            );
            for r in &f.relations {
                assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
            }
            for p in &f.permissions {
                assert!(
                    !mints(&p.permission.0),
                    "permission `{}` is a bare identifier",
                    p.permission.0
                );
            }
        }
    }
}
