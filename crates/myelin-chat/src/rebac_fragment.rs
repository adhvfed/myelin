//! # `rebac_fragment` — the Chat ReBAC namespace fragment (contract 4.9, FROZEN, CHAT-P2 / P-244)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md` §5 (the
//! verbatim frozen Chat fragment: the `channel` + `message` definitions, `channel.read = member +
//! parent_project->read`, the `watcher` Notif read-fanout relation) +
//! `03-events-contracts-and-glue.md` §5 (the load-bearing glue: membership writes project tuples in
//! the same transaction stamping the zookie; `list_subjects(channel, watcher)` resolves read-fanout;
//! the per-viewer unfurl permission is NOT a chat permission — it is a `check` against the *target
//! artifact's* namespace, asked via Refs).
//!
//! **Reconciliation (FROZEN):**
//! `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §1 — *"The frozen
//! per-subsystem fragments: … Chat (`channel.read = member + parent_project->read`). Each declares a
//! `watcher` relation per watchable type (Notif read-fanout)."*
//!
//! **Contract-index row 4.9 (OWNED here — the Chat fragment slice):** the per-subsystem ReBAC
//! namespace fragment. Identity owns the *engine + admit-contract + core org→team→project hierarchy*
//! (`myelin-identity-service::namespace`); Chat owns *this fragment's definition*. The contract
//! boundary Identity compiles against is the frozen names-only ABI carrier
//! [`myelin_identity::NamespaceFragment`] — this module emits exactly that, one carrier per Chat
//! object type, so **Identity's cell schema compiles against the Chat fragment** (the GATE of this
//! prompt — a build-time property, not a runtime drill).
//!
//! ## What this prompt (CHAT-P2 / P-244) ships — and what it deliberately does NOT (VISION §3)
//! **Ships:** the FROZEN Chat fragment as data — the two Chat object types and their relations +
//! permission NAMES, in the frozen [`myelin_identity::NamespaceFragment`] shape Identity admits:
//!
//! | object type | relations                          | permission names         |
//! |-------------|------------------------------------|--------------------------|
//! | `channel`   | `parent_project` `member` `watcher`| `read` `post` `manage`   |
//! | `message`   | `parent_channel`                   | `view`                   |
//!
//! Every relation/permission the architecture §5 names is present:
//! - **`member`** — membership IS the ACL for private kinds; the `read`/`post` grant arm.
//! - **`parent_project`** — the inheritance edge into the org hierarchy (public channels inherit
//!   project read; `manage` intersects project admin).
//! - **`watcher`** — the Notif read-fanout relation Chat owes every watchable type (contract 4.9):
//!   `list_subjects(channel, watcher)` resolves who gets read-fanout against the same authz reverse
//!   index that serves `list_objects` (performant at 50k-member density, contract 4.4 / §5).
//!
//! The frozen permission **rewrites** (names freeze here; the rewrite STRUCTURE is documented below
//! and proven admissible by the CDC against the real engine — the LIVE wiring is the Chat M4 spine):
//! - `channel.read   = member + parent_project->read`   ← the frozen Chat clause (recon §1).
//! - `channel.post   = member`
//! - `channel.manage = member & parent_project->admin`  (invite / archive / settings — the
//!   consequential mutations).
//! - `message.view   = parent_channel->read`            (a message inherits its channel's read).
//!
//! **Does NOT ship (FLOORS named — VISION §3):** *no Chat feature.* No tuples are written, no
//! `check`/`list_objects` is served, no membership row is persisted, no `watcher` is resolved. This
//! is a **contract-fragment freeze** — the relation/permission SHAPES Identity compiles against,
//! nothing more.
//! - **The runtime membership tuple writes** (the `write_tuples([Δtuple], precondition) → zookie` in
//!   the SAME transaction as the membership row + the `chat.channel.member_*` outbox event, stamping
//!   the returned zookie as the new-enemy guard, §5 / contract 4.6/4.10) are the **CHAT-P8 follow-on**
//!   — this prompt ships the FRAGMENT DEFINITION, not the runtime writes.
//! - **The `project(ref, viewer)` per-kind projection + the per-viewer unfurl gate** (a `check`
//!   against the target's namespace via Refs, §3/§5) lands with the Chat projection spine.
//!
//! ## Why names-only here (the DAG, EI-01 §7 — extend, never re-define)
//! `myelin-chat` is a consumer SUBSYSTEM crate; it depends on the frozen contract surface
//! `myelin-identity` (which carries the names-only [`myelin_identity::NamespaceFragment`]), NOT on
//! `myelin-identity-service` (the rich `FragmentDef`/`Userset` engine — a service crate). So the
//! *runtime* fragment Chat ships is the names-only carrier Identity's `admit_fragment` consumes; the
//! rich rewrite structure (the `+ parent_project->read` TTU inheritance, the `& parent_project->admin`
//! intersection) is exercised only by the CDC TEST (a dev-dependency on the engine), never re-defined
//! here. This keeps the §2.9 crate DAG acyclic (no consumer→service edge) while still freezing the
//! full fragment shape Identity must compile.

use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

/// The two frozen Chat object-type names (the §5 `definition` blocks). Public so the live-wiring
/// prompts (CHAT-P8 / the Chat M4 spine) and the CDC test reference the SAME canonical strings (one
/// source of truth — a typo here is a typo everywhere, caught by the admit).
pub mod object_types {
    /// A Conversation of any kind (channel / DM / group-DM / thread root) — the root Chat authz
    /// object (§5 `definition channel`).
    pub const CHANNEL: &str = "channel";
    /// A single message within a channel — inherits its channel's read (§5 `definition message`).
    pub const MESSAGE: &str = "message";
}

/// Build a [`NamespaceFragment`] (the frozen names-only ABI carrier) from `&str` slices — a small
/// constructor that keeps the two fragment definitions below declarative.
fn fragment(object_type: &str, relations: &[&str], permissions: &[&str]) -> NamespaceFragment {
    NamespaceFragment {
        object_type: ObjectType(object_type.to_string()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions: permissions.iter().map(|p| Permission(p.to_string())).collect(),
    }
}

/// **The `channel` fragment** (§5 `definition channel`).
///
/// Relations: `parent_project` (the inheritance edge into the org hierarchy — `read` inherits, `manage`
/// intersects), `member` (membership IS the ACL for private kinds), **`watcher`** (the Notif
/// read-fanout relation, contract 4.9 obligation). Permissions (names frozen here; rewrites — wired
/// LIVE in the Chat M4 spine — documented):
/// - `read   = member + parent_project->read`   (public channels inherit project read).
/// - `post   = member`
/// - `manage = member & parent_project->admin`  (invite / archive / settings).
pub fn channel_fragment() -> NamespaceFragment {
    fragment(
        object_types::CHANNEL,
        &["parent_project", "member", "watcher"],
        &["read", "post", "manage"],
    )
}

/// **The `message` fragment** (§5 `definition message`) — a single message whose row-level visibility
/// is the plain inheritance of its channel's read (no field/transition ABAC caveat on Chat's hot
/// path, §5).
///
/// Relation: `parent_channel`. Permission (name frozen; rewrite the live-wiring floor):
/// - `view = parent_channel->read`.
pub fn message_fragment() -> NamespaceFragment {
    fragment(object_types::MESSAGE, &["parent_channel"], &["view"])
}

/// **The complete frozen Chat ReBAC namespace fragment** — the two [`NamespaceFragment`] carriers
/// Identity admits into the one cell schema (contract 4.9). The order is channel → message
/// (parent-before-child, the order Identity admits them so `message`'s `parent_channel` inheritance
/// edge has its parent type already in the schema). This is the SINGLE entry point the live-wiring
/// prompts + the CDC test consume.
pub fn chat_fragment() -> Vec<NamespaceFragment> {
    vec![channel_fragment(), message_fragment()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen relation set per object type (§5) — the well-formedness witness. A relation dropped
    /// or renamed here is caught by this test BEFORE it reaches Identity's admit.
    #[test]
    fn each_definition_declares_its_frozen_relations() {
        let channel = channel_fragment();
        let channel_rels: Vec<&str> = channel.relations.iter().map(|r| r.0.as_str()).collect();
        // §5 channel: parent_project, member, watcher.
        for expected in ["parent_project", "member", "watcher"] {
            assert!(
                channel_rels.contains(&expected),
                "channel must declare the `{expected}` relation (§5)"
            );
        }
        // message inherits through `parent_channel`.
        assert!(message_fragment()
            .relations
            .contains(&RelName("parent_channel".into())));
    }

    /// The `member` relation (the ACL for private kinds — the `read`/`post` grant arm) and the
    /// `parent_project` inheritance edge (the `+ parent_project->read` / `& parent_project->admin`
    /// arms) are BOTH declared on `channel` — the frozen `read = member + parent_project->read`
    /// rewrite the engine compiles references both, so a fragment missing either would be REJECTED at
    /// admit (UndeclaredRelation). This is the frozen Chat clause's structural anchor (recon §1, §5).
    #[test]
    fn the_channel_read_rewrite_relations_are_declared() {
        let channel = channel_fragment();
        assert!(
            channel.relations.contains(&RelName("member".into())),
            "`member` (the ACL / the `read` + arm) must be declared (§5)"
        );
        assert!(
            channel.relations.contains(&RelName("parent_project".into())),
            "`parent_project` (the inheritance edge — `read` inherits, `manage` intersects) must be \
             declared (§5)"
        );
    }

    /// The `watcher` read-fanout relation is on `channel` (Notif resolves `list_subjects(channel,
    /// watcher)` for the unbounded ambient set — §5 / contract 4.4/4.9). The watchable type is
    /// `channel`; per-thread watch derives from it (§5), so `message` does NOT carry its own watcher.
    #[test]
    fn watcher_is_declared_on_the_watchable_channel_type() {
        assert!(
            channel_fragment()
                .relations
                .contains(&RelName("watcher".into())),
            "the `channel` watchable type declares `watcher` (Notif read-fanout, contract 4.9)"
        );
        assert!(
            !message_fragment()
                .relations
                .contains(&RelName("watcher".into())),
            "`message` does not carry its own watcher — per-thread watch derives from channel (§5)"
        );
    }

    /// The two object types are frozen + non-empty + carry their permission names. This is the shape
    /// Identity compiles; a name dropped here breaks the cell-schema compile.
    #[test]
    fn the_two_chat_object_types_are_frozen() {
        let frag = chat_fragment();
        let types: Vec<&str> = frag.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, vec!["channel", "message"]);
        // the three channel permissions are present (read / post / manage).
        for p in ["read", "post", "manage"] {
            assert!(
                channel_fragment().permissions.contains(&Permission(p.into())),
                "channel declares the `{p}` permission (§5)"
            );
        }
        // the message permission.
        assert!(message_fragment()
            .permissions
            .contains(&Permission("view".into())));
    }

    /// No fragment smuggles an object id (Id never invents object ids): every type/relation/
    /// permission NAME is a bare identifier (no `:`/`/`/`#`). This mirrors the engine's
    /// `mints_object_id` admit check — a fragment that tripped it would be REJECTED at admit.
    #[test]
    fn no_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in chat_fragment() {
            assert!(!mints(&f.object_type.0), "type name is a bare identifier");
            for r in &f.relations {
                assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
            }
            for p in &f.permissions {
                assert!(!mints(&p.0), "permission `{}` is a bare identifier", p.0);
            }
        }
    }
}
