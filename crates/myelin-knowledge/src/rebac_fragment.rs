//! # `rebac_fragment` — Knowledge's **read-side** ReBAC page-tree namespace fragment (contract 4.9,
//! KN-P15 → P-305, M3)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §5 (the page-tree ReBAC fragment — `page.read = (parent_page->read + parent_space->read +
//! direct_reader) - direct_block`; the `row_reader` userset pushed down via `db_row.id`; the
//! `view_field` `CaveatContext` off the hot path) + §5.1 (the row=tuples / field=caveat granularity
//! split, CR-D / Δ7) +
//! `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §1 (the frozen
//! per-subsystem fragments — Knowledge: page-tree inherit-with-overrides + row + field caveat).
//!
//! **Contract-index rows:** **4.9** (the Knowledge ReBAC namespace fragment — DECLARED here, compiled
//! by Identity), **4.3** (the `row_reader` `SetExpr` `list_objects` push-down shape — the conjoin
//! column the lowering JOINs, **lowered to SQL in KN-P16**), **4.2** (the field-level `CaveatContext`
//! caveat off the hot path — **consumed**).
//!
//! ## Why a READ-side names-only carrier lives HERE — and what it adds over the WRITE-side carrier
//! The crate already ships a **write-side** names-only carrier in [`myelin_content::rebac_fragment`]
//! (the `page.publish` / `edit` / `draft` / `comment` producer-tool cap vocabulary the Agent-Fabric
//! sources, AG-P19). THIS module is the **read-side** half of the dual-fragment pattern Git uses
//! ([`myelin_git::rebac_fragment`] read names + `myelin_identity_service::git_fragment` rich
//! rewrites): the frozen READ vocabulary — the four object types (`space` / `page` / `block` /
//! `database_row`), the read relations (`parent_page` / `parent_space` / `direct_reader` /
//! `direct_block` / `row_reader` / `watcher`), and the read permissions (`read` / `comment` / `edit`
//! / `view_field`).
//!
//! **The DAG discipline (why the names-only carrier, not the rich `FragmentDef`, lives in a leaf):**
//! `myelin-identity-service` is the ReBAC ENGINE and the §2.9 acyclic DAG forbids it depending on a
//! subsystem leaf crate. The rewrite STRUCTURE (`page.read = (… ∪ direct_reader) − direct_block`, the
//! exclusion that lets a sub-page NARROW inherited access) can only be expressed by the engine's rich
//! [`myelin_identity_service::knowledge_fragment::FragmentDef`] — Id therefore **OWNS the compiled
//! rewrites** there, declared from this same architecture §5 frozen vocabulary, and the CDC test
//! (`tests/cdc_4_9_knowledge_fragment.rs` in the engine crate + [`tests/cdc_4_9_knowledge_fragment`]
//! here) pins that the two sides agree on the relation/permission NAMES. A rename in either place is a
//! compile/CDC break, never a silent drift (EI-01 §7 — one source of truth).
//!
//! ## What this module adds that the names-only carrier alone cannot
//! Beyond declaring the names, this module ships the **page.read override formula** as a small,
//! testable evaluator ([`page_read_override`]) — the §5 set rule `(inherited ∪ direct_reader) −
//! direct_block` over a concrete reachable-set, so the **override-formula gate** (KN-P15's headline
//! drill) is provable from the Knowledge side WITHOUT the engine crate: `direct_block` removes a
//! narrowed sub-page from a viewer's reachable set; a `direct_reader` on a sub-page adds it. The
//! engine resolves the SAME formula over tuples (the CDC `cdc_4_9_direct_block_override_…` test);
//! this is the closed-form the engine's `Userset::Exclusion` rewrite implements.
//!
//! It also names the **row-level `SetExpr` push-down shape** ([`row_reader_set_expr`]) — the frozen
//! `InRelation { relation: row_reader, via_column: db_row.id }` the §5.1 group-grant lowers over. The
//! *lowering to SQL* (the JOIN against the per-tenant authz reverse index, closing the COUNT-leak) is
//! **KN-P16's deliverable (the named follow-on)**; this module pins the SHAPE the lowering consumes.
//!
//! ## Floors named
//! - **The `list_objects` `SetExpr` push-down → SQL is KN-P16 (P-306).** This module declares the
//!   `InRelation` shape ([`row_reader_set_expr`]); KN-P16 lowers it to the JOIN over `authz_visible`
//!   (the zero-leak-incl-COUNT gate, KN-D5).
//! - **The field-level PREDICATE CATALOGUE per database is co-designed with Identity's role-bundle
//!   catalogue (KQ-5, parallel work — NOT a floor of this prompt).** This module names the
//!   `view_field` permission + the `CaveatContext{object, field, attrs}` SHAPE the caveat rides
//!   ([`field_view_permission`] + [`myelin_identity::CaveatContext`]); which columns each database
//!   gates (the per-database predicate set) is the KQ-5 catalogue, designed alongside Id's role
//!   bundles, not frozen here.

use myelin_identity::{ColRef, NamespaceFragment, ObjectType, Permission, RelName, SetExpr};
use std::collections::BTreeSet;

/// The four frozen Knowledge object-type names (§5) — the SAME canonical strings the engine's rich
/// read-side fragment (`myelin_identity_service::knowledge_fragment::object_types`) and the write-side
/// carrier ([`myelin_content::rebac_fragment::object_types`]) declare. Public so the 4.9 CDC + a
/// live-wiring caller reference the canonical strings, never a stringly-typed literal.
pub mod object_types {
    /// A Knowledge **space** — the root container (a workspace / wiki root); a space maps to a
    /// `project` object in the wider namespace (§4.3). Pages inherit it via `parent_space->read`.
    pub const SPACE: &str = "space";
    /// A **page** in the page-tree — the inherit-with-overrides node (`page.read` is the headline
    /// rewrite; the `- direct_block` exclusion narrows a sub-page).
    pub const PAGE: &str = "page";
    /// A **block** within a page — read exactly when its page is (inheritance only, no direct grant).
    pub const BLOCK: &str = "block";
    /// A **database row** — the row-level-ACL object the `list_objects` push-down keys on (`db_row.id`).
    pub const DATABASE_ROW: &str = "database_row";
}

// ───────────────────────── the frozen read-side relation names (§5) ─────────────────────────────

/// **`parent_page`** — the sub-page nesting edge (the `page_parent` typed table, §4.3). The
/// tuple-to-userset rewrite `parent_page->read` is the whole tree's inheritance in one rule.
pub const PARENT_PAGE: &str = "parent_page";
/// **`parent_space`** — a top-level page's inheritance edge to its space (`parent_space->read`).
pub const PARENT_SPACE: &str = "parent_space";
/// **`parent_db`** — a `database_row`'s edge to its owning page/database (the row inherits page read).
pub const PARENT_DB: &str = "parent_db";
/// **`direct_reader`** — an explicit read grant on a `space` / `page` / `database_row` (the
/// `+ direct_reader` arm of the page-tree rewrite; the row-level direct grant).
pub const DIRECT_READER: &str = "direct_reader";
/// **`direct_block`** — the page-tree OVERRIDE (§5): a subject blocked on a `page` is REMOVED from the
/// page's read set EVEN IF they inherit it from the parent. The `- direct_block` exclusion arm; the
/// "a sub-page narrows inherited access" lever (Zanzibar's exclusion userset).
pub const DIRECT_BLOCK: &str = "direct_block";
/// **`row_reader`** — the §5.1 row-level GROUP grant (`row_reader: team#member`) on a database: a
/// single grant covers thousands of rows via tuple-to-userset rewrite, pushed down via `db_row.id`.
pub const ROW_READER: &str = "row_reader";
/// **`watcher`** — the cross-cutting Notif read-fanout relation (C8) on `space` / `page` /
/// `database_row` so `list_subjects(object, watcher)` is an ordinary Expand.
pub const WATCHER: &str = "watcher";

// ───────────────────────── the frozen read-side permission names (§5) ───────────────────────────

/// **`read`** — the resolved-visibility permission `list_objects(subject, read, …)` pushes down and
/// `check(subject, read, page)` resolves through the page-tree rewrite.
pub const READ: &str = "read";
/// **`comment`** — `read - direct_block` (§5): a commenter must be able to read and not be blocked.
pub const COMMENT: &str = "comment";
/// **`edit`** — `direct_editor + parent_page->edit + parent_space->member - direct_block` (§5).
pub const EDIT: &str = "edit";
/// **`view_field`** — the field-level read GATE (§5.1 / OQ-E): resolves to the row-read grant and is
/// then gated by the off-hot-path `CaveatContext{field}` at `check`-time (the column-hiding caveat).
pub const VIEW_FIELD: &str = "view_field";

/// The Knowledge **`via_column`** for the row-level `list_objects` push-down (§5.1 / §7.3): the
/// `database_row.id` column the `InRelation` `SetExpr` JOINs the per-tenant authz reverse index
/// against — one query, no N+1. KN-P16 lowers [`row_reader_set_expr`] over this column.
pub const DB_ROW_TABLE: &str = "db_row";
/// The id column on [`DB_ROW_TABLE`] the conjoin keys on.
pub const DB_ROW_ID_COLUMN: &str = "id";

// ───────────────────────── the names-only fragment carriers (4.9) ────────────────────────────────

fn fragment(object_type: &str, relations: &[&str], permissions: &[&str]) -> NamespaceFragment {
    NamespaceFragment {
        object_type: ObjectType(object_type.to_string()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions: permissions.iter().map(|p| Permission(p.to_string())).collect(),
    }
}

/// **The `space` read fragment** (§5 `definition space`) — the root container. Relations:
/// `direct_reader` / `member` / `watcher`; permission: `read`. Watchable (C8).
pub fn space_read_fragment() -> NamespaceFragment {
    fragment(
        object_types::SPACE,
        &[DIRECT_READER, "member", WATCHER],
        &[READ],
    )
}

/// **The `page` read fragment** (§5 `definition page`) — the inherit-with-overrides node, the headline
/// of the Knowledge fragment. Relations: `parent_page` / `parent_space` / `direct_reader` /
/// `direct_block` / `watcher`; permissions: `read` / `comment` / `edit`. The rewrite STRUCTURE
/// (`read = (parent_page->read ∪ parent_space->read ∪ direct_reader) − direct_block`) is the engine's
/// rich fragment; this carrier is names-only — but the override SET RULE is provable from this crate
/// via [`page_read_override`].
pub fn page_read_fragment() -> NamespaceFragment {
    fragment(
        object_types::PAGE,
        &[PARENT_PAGE, PARENT_SPACE, DIRECT_READER, DIRECT_BLOCK, WATCHER],
        &[READ, COMMENT, EDIT],
    )
}

/// **The `block` read fragment** (§5 `definition block`) — `read = parent_page->read` (inheritance
/// only; a block is read exactly when its page is). Relations: `parent_page`; permission: `read`.
pub fn block_read_fragment() -> NamespaceFragment {
    fragment(object_types::BLOCK, &[PARENT_PAGE], &[READ])
}

/// **The `database_row` read fragment** (§5 `definition database_row` / §5.1) — the row-level-ACL
/// object. Relations: `parent_db` / `direct_reader` / `row_reader` / `watcher`; permissions: `read`
/// (`= parent_db->read + row_reader`, pushed down via `db_row.id`) + `view_field` (the field-level
/// gate the off-hot-path `CaveatContext` rides on). Watchable (C8).
pub fn database_row_read_fragment() -> NamespaceFragment {
    fragment(
        object_types::DATABASE_ROW,
        &[PARENT_DB, DIRECT_READER, ROW_READER, WATCHER],
        &[READ, VIEW_FIELD],
    )
}

/// **The complete Knowledge READ-side names-only ReBAC fragment (contract 4.9)** — the four object
/// types Knowledge declares for the cell schema, in parent-before-child order (`space` → `page` →
/// `block` → `database_row`). The SINGLE entry point the 4.9 CDC consumes to assert the Knowledge
/// side agrees with Id's compiled rich fragment on the relation/permission NAMES. The rewrites are the
/// engine's ([`myelin_identity_service::knowledge_fragment`]); this is the declared vocabulary.
pub fn knowledge_read_fragment() -> Vec<NamespaceFragment> {
    vec![
        space_read_fragment(),
        page_read_fragment(),
        block_read_fragment(),
        database_row_read_fragment(),
    ]
}

// ───────────────────────── the page.read OVERRIDE FORMULA (the KN-P15 gate) ──────────────────────

/// **The page.read override formula (§5) — the closed-form set rule `(inherited ∪ direct_reader) −
/// direct_block`, evaluated over a concrete reachable-set.**
///
/// This is the testable nucleus of KN-P15's headline drill: it computes a page's resolved reader set
/// from the three inputs of the §5 rewrite —
/// - `inherited`: the subjects who read this page by INHERITANCE (the `parent_page->read ∪
///   parent_space->read` tuple-to-userset arms, already resolved over the parent),
/// - `direct_reader`: the subjects with an explicit `direct_reader` grant on THIS page (the
///   `+ direct_reader` arm), and
/// - `direct_block`: the subjects blocked on THIS page (the `- direct_block` OVERRIDE arm) —
///
/// and returns `(inherited ∪ direct_reader) − direct_block`. The `direct_block` exclusion is applied
/// LAST so it overrides BOTH inheritance and a direct grant: a blocked subject disappears from the
/// page's read set even if they inherit it from the parent (the "a sub-page narrows inherited access"
/// lever) AND even if granted directly here — exactly the engine's `Userset::Exclusion { base, …}`
/// semantics, by construction, never a post-filter. A `direct_reader` on a (non-blocked) sub-page
/// ADDS the subject (the `+ direct_reader` arm). This is mutation-tested core (the override is the
/// leak-critical path): dropping the exclusion, or applying it before the union, is caught here.
pub fn page_read_override<S: AsRef<str>>(
    inherited: &[S],
    direct_reader: &[S],
    direct_block: &[S],
) -> BTreeSet<String> {
    let mut readers: BTreeSet<String> = BTreeSet::new();
    // base = inherited ∪ direct_reader (the page-tree inheritance UNIONed with the direct grant).
    for s in inherited {
        readers.insert(s.as_ref().to_string());
    }
    for s in direct_reader {
        readers.insert(s.as_ref().to_string());
    }
    // − direct_block (the OVERRIDE) — applied LAST so it removes a blocked subject from the resolved
    // set even when inherited OR granted directly. This is the §5 exclusion, by construction.
    for s in direct_block {
        readers.remove(s.as_ref());
    }
    readers
}

// ───────────────────────── the row-level SetExpr push-down SHAPE (KN-P16 lowers) ─────────────────

/// **The row-level `SetExpr` push-down shape (§5.1 / contract 4.3) — `InRelation { relation:
/// row_reader, via_column: db_row.id }`.** The frozen shape a `list_objects(subject, read,
/// database_row)` returns as its `Filter`; the §5.1 `row_reader: team#member` GROUP grant lowers over
/// the `db_row.id` `via_column` — a JOIN against the per-tenant authz reverse index, ONE query, no
/// N+1, no post-filter, so an un-readable row is ABSENT from the result INCLUDING the COUNT.
///
/// **Floor:** the *lowering to SQL* (the JOIN against `authz_visible`, the zero-leak-incl-COUNT KN-D5
/// gate) is **KN-P16's deliverable (P-306, the named follow-on)**. This pins the SHAPE that lowering
/// consumes — a rename of `row_reader` or a wrong `via_column` is a compile break here.
pub fn row_reader_set_expr() -> SetExpr {
    SetExpr::InRelation {
        relation: RelName(ROW_READER.to_string()),
        via_column: ColRef {
            table: DB_ROW_TABLE.to_string(),
            column: DB_ROW_ID_COLUMN.to_string(),
        },
    }
}

/// **The field-level `view_field` permission name (§5.1 / OQ-E).** Field-level column hiding is NOT a
/// row permission; it is the off-hot-path `check`-time `CaveatContext{object, field, attrs}` caveat
/// over the ONE `myelin_query` predicate core, evaluated on the ALREADY-filtered, already-fetched rows
/// — never on the hot `list_objects` path (it would defeat the conjoin). Knowledge names the
/// permission here; the caveat predicate is built/resolved by Identity
/// (`myelin_identity_service::knowledge_fragment::field_view_caveat`). The per-database PREDICATE
/// CATALOGUE (which columns each database gates) is co-designed with Id's role-bundle catalogue
/// (KQ-5, parallel work — not frozen here).
pub fn field_view_permission() -> Permission {
    Permission(VIEW_FIELD.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The four Knowledge object types + their read permissions are frozen (§5).** A rename of any
    /// object type or read permission flips an assertion here — the names are the contract the engine's
    /// rich fragment and the 4.9 CDC agree on.
    #[test]
    fn the_read_vocabulary_is_frozen() {
        let frags = knowledge_read_fragment();
        let types: Vec<&str> = frags.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, ["space", "page", "block", "database_row"], "parent-before-child order");

        let page = page_read_fragment();
        for p in [READ, COMMENT, EDIT] {
            assert!(
                page.permissions.iter().any(|perm| perm.0 == p),
                "the `page` read fragment declares `{p}`"
            );
        }
        // the page declares the override + inheritance relations.
        for r in [PARENT_PAGE, PARENT_SPACE, DIRECT_READER, DIRECT_BLOCK, WATCHER] {
            assert!(page.relations.iter().any(|rel| rel.0 == r), "page declares `{r}`");
        }
        // the row declares row_reader + view_field (the row ACL + the field gate).
        let row = database_row_read_fragment();
        assert!(row.relations.iter().any(|r| r.0 == ROW_READER), "database_row declares row_reader");
        assert!(
            row.permissions.iter().any(|p| p.0 == VIEW_FIELD),
            "database_row declares view_field (the field gate)"
        );
        // the bare canonical names.
        assert_eq!(READ, "read");
        assert_eq!(DIRECT_BLOCK, "direct_block");
        assert_eq!(ROW_READER, "row_reader");
        assert_eq!(VIEW_FIELD, "view_field");
    }

    /// **The object-type + relation + permission names match the engine's rich read-side fragment.**
    /// `space`/`page`/`block`/`database_row` here are byte-identical to
    /// `myelin_identity_service::knowledge_fragment::object_types` — the dual (rich read in the engine +
    /// names-only read here) fragments agree on the vocabulary (the CDC pins the behaviour).
    #[test]
    fn object_type_names_are_the_canonical_kn_vocabulary() {
        assert_eq!(object_types::SPACE, "space");
        assert_eq!(object_types::PAGE, "page");
        assert_eq!(object_types::BLOCK, "block");
        assert_eq!(object_types::DATABASE_ROW, "database_row");
        // and they match the write-side carrier in myelin-content (one source of truth).
        assert_eq!(object_types::PAGE, myelin_content::rebac_fragment::object_types::PAGE);
        assert_eq!(object_types::SPACE, myelin_content::rebac_fragment::object_types::SPACE);
    }

    /// **The page.read override formula: `direct_block` REMOVES a narrowed sub-page from a viewer's
    /// reachable set (§5).** alice inherits read from the parent; the sub-page's `- direct_block`
    /// override removes her — by construction, not a post-filter. bob inherits and is not blocked, so
    /// he stays.
    #[test]
    fn direct_block_override_removes_an_inheriting_reader() {
        let resolved = page_read_override(
            &["alice", "bob"], // inherited from the parent page/space
            &[],               // no direct grant on the sub-page
            &["alice"],        // the sub-page BLOCKS alice (the override)
        );
        assert!(!resolved.contains("alice"), "the - direct_block override removes the inheriting alice");
        assert!(resolved.contains("bob"), "an un-blocked inheriting reader stays");
    }

    /// **The page.read override formula: a `direct_reader` ADDS a sub-page (the `+ direct_reader`
    /// arm).** carol does not inherit, but a direct grant on the sub-page adds her.
    #[test]
    fn direct_reader_adds_a_sub_page() {
        let resolved = page_read_override(
            &["alice"],  // only alice inherits
            &["carol"],  // carol is granted directly on the sub-page
            &[],
        );
        assert!(resolved.contains("carol"), "the + direct_reader arm adds carol");
        assert!(resolved.contains("alice"), "the inheriting alice stays");
    }

    /// **The exclusion is applied LAST — `direct_block` overrides EVEN a direct grant on the same
    /// page.** A subject both directly granted AND blocked is BLOCKED (the override wins; the mutation
    /// that applies the exclusion before the union, or drops it, is caught here).
    #[test]
    fn direct_block_overrides_even_a_direct_grant() {
        let resolved = page_read_override(
            &[],            // no inheritance
            &["mallory"],   // directly granted...
            &["mallory"],   // ...AND blocked → the override wins
        );
        assert!(
            !resolved.contains("mallory"),
            "the - direct_block exclusion is applied last → it overrides even a direct_reader grant"
        );
        assert!(resolved.is_empty(), "no one reads the page");
    }

    /// **The inheritance-only degenerate (a block / a page with neither block nor extra grant) is
    /// exactly the inherited set.** With no direct grant and no block, the resolved set is the
    /// inheritance union verbatim — the `block.read = parent_page->read` shape.
    #[test]
    fn inheritance_only_is_the_inherited_set() {
        let resolved = page_read_override(&["alice", "bob"], &[], &[]);
        let expect: BTreeSet<String> = ["alice", "bob"].iter().map(|s| s.to_string()).collect();
        assert_eq!(resolved, expect, "with no override, read is the inherited set");
    }

    /// **The row-level `SetExpr` push-down shape is the frozen `InRelation { row_reader, db_row.id }`
    /// (§5.1, the KN-P16 lowering target).** The relation is `row_reader`; the `via_column` is
    /// `db_row.id`. A wrong column or relation is a compile/assert break here — the shape KN-P16's
    /// SQL lowering consumes is pinned.
    #[test]
    fn row_reader_push_down_is_inrelation_over_db_row_id() {
        match row_reader_set_expr() {
            SetExpr::InRelation { relation, via_column } => {
                assert_eq!(relation.0, "row_reader", "the row-level group grant relation");
                assert_eq!(via_column.table, "db_row", "the conjoin table");
                assert_eq!(via_column.column, "id", "the via_column the JOIN keys on (no N+1)");
            }
            other => panic!("the row push-down must be an InRelation, got {other:?}"),
        }
    }

    /// **The field-level gate is the `view_field` permission (the off-hot-path caveat rides it).** The
    /// row-level push-down returns visible ROWS; `view_field` + the `CaveatContext` then redacts a
    /// COLUMN — never on the hot `list_objects` path.
    #[test]
    fn field_view_permission_is_named() {
        assert_eq!(field_view_permission().0, "view_field");
    }

    /// **No read-side fragment name smuggles an object id (Id never invents object ids).** Every
    /// type/relation/permission is a bare identifier — the engine's admit would reject one that wasn't
    /// (mirrors the write-side carrier's guard).
    #[test]
    fn no_read_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in knowledge_read_fragment() {
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
