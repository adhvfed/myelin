//! # `rebac_fragment` — Knowledge's names-only **write-side** ReBAC carrier (contract 4.9, the KN
//! producer-tool cap vocabulary; AG-P19 → P-268 consumes it)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §5 (the frozen Knowledge
//! fragment vocabulary — `space` / `page` / `block` / `database_row`) +
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §6.3 (the producer-tool gating:
//! KN `publish` / `edit(confidential_page)` = gated, `draft` / `comment` = not). Mirrors the Git
//! names-only carrier [`myelin_git::rebac_fragment`].
//!
//! ## Why a names-only carrier lives HERE (the dual-fragment pattern, same as Git)
//! Identity owns the COMPILED **read-side** rich fragment (the page-tree-with-overrides `page.read`
//! rewrite + the row-level ACL + the field caveat) in `myelin_identity_service::knowledge_fragment`
//! — that crate is the engine and cannot be a dependency of a subsystem leaf crate (the §2.9 acyclic
//! DAG). The READ rewrites are deliberately the whole of the *rich* fragment.
//!
//! The Agent-Fabric's per-producer KN ToolDefs (publish / edit / draft / comment, AG-P19) need their
//! `required_caps` to name the KN **write** authority — and a names-only [`NamespaceFragment`] carrier
//! is exactly how Git supplies the same (`pull_request.merge` / `repo.push` come from
//! `myelin_git::rebac_fragment`, NOT invented in the Fabric). This module is the KN parallel: the
//! frozen **write-permission NAMES** on the `page` object type, so the Fabric sources its caps from
//! the canonical KN crate (one source of truth — a rename here is a compile/CDC break in the Fabric,
//! never a silent drift). Like Git's carrier it is names-only; the WRITE-side authz *rewrites* (who
//! may publish/edit — by construction a `page.read` precondition + the Knowledge endpoint's own write
//! authority) are the Knowledge platform's deliverable (KN-P04+), wired LIVE into the engine then.
//!
//! ## The frozen `page` write-permission vocabulary (§6.3 producer tools)
//! - **`publish`** — make a draft page visible (consequential: an approver set; gated by §6.3).
//! - **`edit`** — edit an existing (confidential) page's content (consequential; gated by §6.3 as
//!   `edit(confidential_page)`).
//! - **`draft`** — create/update a private draft (reversible; NOT gated by §6.3).
//! - **`comment`** — add a comment to a page (reversible; NOT gated by §6.3).
//!
//! These are the WRITE counterparts to the read-side `page.read` the rich fragment declares; the
//! object type (`page`) is the SAME canonical name both sides freeze (the CDC pins the agreement).

use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

/// The frozen Knowledge object-type names (§5; the SAME names the rich read-side fragment in
/// `myelin_identity_service::knowledge_fragment::object_types` declares). Public so the AG-P19 KN
/// producer ToolDefs + the 4.9 CDC reference the canonical strings, not stringly-typed literals.
pub mod object_types {
    /// A Knowledge **space** — the root container (a workspace / wiki root).
    pub const SPACE: &str = "space";
    /// A **page** in the page-tree — the object the producer-tool write permissions hang off
    /// (`page.publish` / `page.edit` / `page.draft` / `page.comment`).
    pub const PAGE: &str = "page";
    /// A **block** within a page (inherits the page's read; not independently writable at v1).
    pub const BLOCK: &str = "block";
    /// A **database row** — the row-level-ACL object (read-side; no producer write tool at M3).
    pub const DATABASE_ROW: &str = "database_row";
}

// ───────────────────────── the frozen page write-permission names (§6.3 producer tools) ───────────

/// **The `publish` permission** — make a draft page visible. The cap the gated `knowledge.publish`
/// producer ToolDef (AG-P19) requires (`page.publish`). Consequential (an approver set) → the §6.3
/// `requires_approval = yes` default.
pub const PUBLISH: &str = "publish";

/// **The `edit` permission** — edit an existing (confidential) page's content. The cap the gated
/// `knowledge.edit_confidential` producer ToolDef (AG-P19) requires (`page.edit`). §6.3 names this
/// `edit(confidential_page)`; consequential → `requires_approval = yes`.
pub const EDIT: &str = "edit";

/// **The `draft` permission** — create/update a private draft page. The cap the NON-gated
/// `knowledge.draft` producer ToolDef requires (`page.draft`). Reversible → §6.3 `requires_approval =
/// no`.
pub const DRAFT: &str = "draft";

/// **The `comment` permission** — add a comment to a page. The cap the NON-gated `knowledge.comment`
/// producer ToolDef requires (`page.comment`). Reversible → §6.3 `requires_approval = no`.
pub const COMMENT: &str = "comment";

/// **The `read` permission** — the read-side permission the rich fragment owns (§5). Re-exported here
/// (names-only) so a write tool can name the read precondition without depending on the engine crate.
pub const READ: &str = "read";

/// Build a [`NamespaceFragment`] (the frozen names-only ABI carrier) from `&str` slices — the same
/// small constructor shape `myelin_git::rebac_fragment` uses.
fn fragment(object_type: &str, relations: &[&str], permissions: &[&str]) -> NamespaceFragment {
    NamespaceFragment {
        object_type: ObjectType(object_type.to_string()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions: permissions.iter().map(|p| Permission(p.to_string())).collect(),
    }
}

/// **The `page` WRITE-side fragment carrier (4.9 — the producer-tool cap vocabulary).** The frozen
/// write permissions the KN producer ToolDefs (AG-P19) require: `publish` / `edit` (consequential,
/// §6.3-gated) + `draft` / `comment` (reversible, not gated). The `read` permission name is included
/// so the carrier names the full `page` vocabulary the Fabric references; the read-side *rewrite*
/// (`page.read = (parent_page->read ∪ …) − direct_block`) is the engine's rich fragment
/// (`myelin_identity_service::knowledge_fragment`), this carrier is names-only (the write *rewrites*
/// are the Knowledge endpoints' deliverable, KN-P04+).
///
/// Relations: `direct_writer` (the direct write grant a producer tool's cap resolves through),
/// `parent_page` / `parent_space` (the inheritance edges, the SAME names the read-side fragment
/// freezes), `watcher` (Notif read-fanout, C8). Permissions: `publish` / `edit` / `draft` /
/// `comment` / `read`.
pub fn page_write_fragment() -> NamespaceFragment {
    fragment(
        object_types::PAGE,
        &[
            "direct_writer",
            "parent_page",
            "parent_space",
            "watcher",
        ],
        &[PUBLISH, EDIT, DRAFT, COMMENT, READ],
    )
}

/// **The complete KN write-side names-only ReBAC carrier (4.9).** At M3 the producer-tool surface
/// hangs off the `page` object only (publish/edit/draft/comment all act on a page); the other three
/// object types (`space`/`block`/`database_row`) are read-side at M3. This is the SINGLE entry point
/// the AG-P19 KN ToolDefs source their caps from + the 4.9 CDC consumes.
pub fn knowledge_write_fragment() -> Vec<NamespaceFragment> {
    vec![page_write_fragment()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The four producer-tool write permissions are frozen on the `page` object (§6.3).** A rename
    /// of any cap name flips an assertion here — the names are the contract both the Fabric ToolDef
    /// caps and the eventual KN engine wiring agree on.
    #[test]
    fn the_page_write_permissions_are_frozen() {
        let page = page_write_fragment();
        assert_eq!(page.object_type.0, "page");
        for p in [PUBLISH, EDIT, DRAFT, COMMENT, READ] {
            assert!(
                page.permissions.iter().any(|perm| perm.0 == p),
                "the `page` write fragment declares the `{p}` permission (4.9 producer-tool cap)"
            );
        }
        // the canonical names are bare identifiers (the §6.3 / §5 vocabulary).
        assert_eq!(PUBLISH, "publish");
        assert_eq!(EDIT, "edit");
        assert_eq!(DRAFT, "draft");
        assert_eq!(COMMENT, "comment");
    }

    /// **The object-type names match the read-side rich fragment's canonical names.** `page` here is
    /// byte-identical to `knowledge_fragment::object_types::PAGE` (the engine crate) — the dual
    /// (rich read + names-only write) fragments agree on the object vocabulary (same as Git's dual
    /// `repo`/`pull_request` carriers).
    #[test]
    fn object_type_names_are_the_canonical_kn_vocabulary() {
        assert_eq!(object_types::SPACE, "space");
        assert_eq!(object_types::PAGE, "page");
        assert_eq!(object_types::BLOCK, "block");
        assert_eq!(object_types::DATABASE_ROW, "database_row");
    }

    /// **The carrier is names-only (no object id smuggled).** Every type/relation/permission is a
    /// bare identifier — the engine's `admit` would reject one that wasn't (mirrors the Git carrier's
    /// guard).
    #[test]
    fn no_kn_write_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in knowledge_write_fragment() {
            assert!(!mints(&f.object_type.0), "type `{}` is a bare identifier", f.object_type.0);
            for r in &f.relations {
                assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
            }
            for p in &f.permissions {
                assert!(!mints(&p.0), "permission `{}` is a bare identifier", p.0);
            }
        }
    }
}
