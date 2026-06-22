//! # `knowledge_fragment` — Id's compiled **Knowledge** ReBAC namespace fragment (contract 4.9,
//! P-ID-26 → P-249)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §5 (the **frozen Knowledge fragment**: `space`, `page`, `block`, `database_row`; **page-tree
//! inheritance WITH overrides** — `page.read = parent_page->read + direct_reader - direct_block` —
//! so a sub-page can NARROW inherited access; **row-level ACL** pushed down via `list_objects` (C1);
//! **field-level column hiding** as a `check`-time `CaveatContext` caveat (C3)), §7.3 (the Knowledge
//! **via_column** mapping: `db_row.id` — the conjoin column the `list_objects` `Filter` JOINs the
//! `database_row` reverse index against, one query, no N+1), §8.6 (the **field caveat on the full
//! `QueryAst` predicate core** — the off-hot-path field/transition ABAC rider, P-ID-22).
//!
//! **Contract-index rows:** **4.9** (the KN fragment — OWNED here), **4.3** (row-level ACL conjoin —
//! consumed via [`crate::list_objects`]), **4.2** (the field caveat — consumed via
//! [`crate::check_engine::eval_caveat_predicate`], the ONE `QueryAst` predicate core).
//!
//! This is the **SECOND of the five per-subsystem fragments** (P-ID-24/26/27/29/30) that promote the
//! M1 engine-only floor (P-068); the first (Git) is [`crate::git_fragment`]. Like the Git fragment it
//! is the canonical **rich** [`crate::namespace::FragmentDef`] declaration of the Knowledge authz
//! vocabulary, with the permission **rewrites** wired over the four Zanzibar userset operators so
//! `check`/`list_objects` resolve the Knowledge permissions through the SAME engine the core hierarchy
//! uses (one primitive — no bespoke KN check path, the §5 design rule). The Knowledge data model
//! (the page/database tables themselves) is the Knowledge-platform prompts'; this module ships only
//! the Id-side authz content.
//!
//! ## Why the rich fragment lives HERE (not in `myelin-knowledge`)
//! Same DAG discipline as [`crate::git_fragment`]: `myelin-identity-service` (the engine) does NOT
//! depend on a subsystem leaf crate (§2.9 acyclic DAG). The names-only ABI carrier
//! [`myelin_identity::NamespaceFragment`] cannot carry the rewrite STRUCTURE
//! (`page.read = parent_page->read ∪ direct_reader − direct_block`, the exclusion that makes a
//! sub-page narrow inherited access); only the engine's rich `FragmentDef` can. So **Id owns the
//! compiled rewrites** (this module), declared from the architecture §5 frozen vocabulary directly,
//! and the CDC test (`tests/cdc_4_9_knowledge_fragment.rs`) pins that the two sides agree on the
//! relation/permission NAMES.
//!
//! ## The compiled Knowledge fragment (§5)
//!
//! | object type     | relations                                                  | permissions (rewrite)                                                                |
//! |-----------------|------------------------------------------------------------|--------------------------------------------------------------------------------------|
//! | `space`         | `direct_reader` `member` `watcher`                         | `read = direct_reader ∪ member`                                                       |
//! | `page`          | `parent_page` `parent_space` `direct_reader` `direct_block` `watcher` | **`read = (parent_page->read ∪ parent_space->read ∪ direct_reader) − direct_block`** (the page-tree-with-overrides rewrite) |
//! | `block`         | `parent_page`                                              | `read = parent_page->read`                                                            |
//! | `database_row`  | `parent_page` `direct_reader` `watcher`                    | `read = direct_reader ∪ parent_page->read` (the row-level ACL `list_objects` pushes down via `db_row.id`) |
//!
//! - **page-tree-with-overrides** — `page.read` is a tuple-to-userset inheritance from the parent
//!   page (`parent_page->read`) UNIONed with a direct grant (`direct_reader`) and the root space
//!   (`parent_space->read`), all then **EXCLUDED by `direct_block`** (the §5 `- direct_block`
//!   override). The exclusion is what lets a sub-page NARROW inherited access by construction — a
//!   blocked subject disappears from the sub-page's `read` set even though they inherit the parent's,
//!   without a post-filter (the same exclusion crux as Issues' `- confidential`).
//! - **row-level ACL** — `database_row.read = direct_reader ∪ parent_page->read`; `list_objects`
//!   pushes this down via the `db_row.id` via_column (§7.3) — a db view's rows pre-filtered in ONE
//!   query, no N+1.
//! - **field-level column hiding** — NOT a permission here; it is the off-hot-path `check`-time
//!   `CaveatContext` caveat (C3, §8.6) over the full `QueryAst` predicate core
//!   ([`crate::check_engine::eval_caveat_predicate`]). `list_objects` returns the visible ROWS
//!   cheaply (the row ACL above); `check(subject, view_field, row, caveat)` then redacts individual
//!   COLUMNS on those already-filtered rows — a denied column is `Deny` (redacted) and is ABSENT from
//!   any count, never a silent allow ([`field_view_caveat`]).
//! - **watchability** — `space`/`page`/`database_row` declare the cross-cutting `watcher` relation
//!   (C8) so Notif's read-fanout `list_subjects(object, watcher)` is an ordinary Expand over S8.

use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{CaveatContext, FieldId, Literal, ObjectType, Permission, RelName};
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeMap;

/// The four frozen Knowledge object-type names (§5; mirrors `myelin_knowledge`'s names-only carrier).
/// Public so the CDC test + a live-wiring caller reference the SAME canonical strings.
pub mod object_types {
    /// A Knowledge space — the root container (a workspace / wiki root).
    pub const SPACE: &str = "space";
    /// A page in the page-tree (the inherit-with-overrides node).
    pub const PAGE: &str = "page";
    /// A block within a page (inherits the page's read).
    pub const BLOCK: &str = "block";
    /// A database row — the row-level-ACL object the `list_objects` push-down keys on (`db_row.id`).
    pub const DATABASE_ROW: &str = "database_row";
}

/// **The `direct_reader` relation** — a direct read grant on a `space`/`page`/`database_row` (the
/// `+ direct_reader` arm of the page-tree rewrite; the row-level ACL grant). Exposed so the CDC + a
/// live caller reference the canonical name, not a stringly-typed literal.
pub const DIRECT_READER: &str = "direct_reader";

/// **The `direct_block` relation (the page-tree OVERRIDE, §5)** — a subject blocked on a `page`. It
/// is the `- direct_block` exclusion arm of `page.read`: a blocked subject is removed from the page's
/// read set EVEN IF they inherit it from the parent page/space. This is the "a sub-page can narrow
/// inherited access" lever (the override-exclusion rewrite, a mutation-tested core path).
pub const DIRECT_BLOCK: &str = "direct_block";

/// **The `read` permission name** — the resolved-visibility permission `list_objects(subject, read,
/// database_row)` pushes down and `check(subject, read, page)` resolves through the page-tree rewrite.
pub const READ: &str = "read";

/// **The field-view caveat's `FieldId` namespace marker.** The Knowledge field-caveat is `check`'d as
/// `check(subject, view_field, row, CaveatContext{field, attrs})`; the `view_field` permission is the
/// field-level read gate, resolved off the hot path through the ONE `QueryAst` predicate core (§8.6).
pub const VIEW_FIELD: &str = "view_field";

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

/// **The `space` fragment** (§5 `definition space`) — the root container. `read = direct_reader ∪
/// member` (a space reader or member can read the space; pages inherit it via `parent_space->read`).
/// Watchable (C8) so Notif can fan out over a space.
pub fn space_fragment() -> FragmentDef {
    frag(
        object_types::SPACE,
        &[DIRECT_READER, "member"],
        vec![perm(
            READ,
            Userset::Union(vec![rel(DIRECT_READER), rel("member")]),
        )],
    )
    .watchable()
}

/// **The `page` fragment** (§5 `definition page`) — the page-tree-with-overrides node, the headline
/// rewrite.
///
/// **`read = (parent_page->read ∪ parent_space->read ∪ direct_reader) − direct_block`** — the §5
/// `page.read = parent_page->read + direct_reader - direct_block` rewrite, extended with the
/// `parent_space->read` inheritance arm (a top-level page with no parent page inherits from its space
/// instead — the same union, one more arm). The **`- direct_block` exclusion** is the OVERRIDE: a
/// blocked subject is removed from this page's read set even when they inherit it from the parent —
/// the "a sub-page narrows inherited access" lever, by construction, never a post-filter. Watchable
/// (C8). A page with neither a `parent_page` nor a `parent_space` tuple is read only by a
/// `direct_reader` (the root degenerate case — the union's other arms resolve over no parent tuple).
pub fn page_fragment() -> FragmentDef {
    frag(
        object_types::PAGE,
        &["parent_page", "parent_space", DIRECT_READER, DIRECT_BLOCK],
        vec![perm(
            READ,
            // (parent_page->read ∪ parent_space->read ∪ direct_reader) − direct_block
            Userset::Exclusion {
                base: Box::new(Userset::Union(vec![
                    ttu("parent_page", READ),
                    ttu("parent_space", READ),
                    rel(DIRECT_READER),
                ])),
                subtracted: Box::new(rel(DIRECT_BLOCK)),
            },
        )],
    )
    .watchable()
}

/// **The `block` fragment** (§5 `definition block`). `read = parent_page->read` — a block is read
/// exactly when its page is (inheritance only, no direct grant, no override; the block is content
/// inside the page's ACL).
pub fn block_fragment() -> FragmentDef {
    frag(
        object_types::BLOCK,
        &["parent_page"],
        vec![perm(READ, ttu("parent_page", READ))],
    )
}

/// **The `database_row` fragment** (§5 `definition database_row`) — the **row-level ACL** object.
///
/// `read = direct_reader ∪ parent_page->read`. The `list_objects(subject, read, database_row)`
/// push-down conjoins this over the consumer's `db_row.id` via_column (§7.3) — a db view's rows
/// pre-filtered in ONE query (no N+1, no post-filter): an un-readable row is ABSENT from the result
/// INCLUDING the COUNT (the confidential-row-disappears-by-construction crux). Watchable (C8).
/// **Field-level** column hiding on a returned row is NOT here — it is the off-hot-path `check`-time
/// caveat ([`field_view_caveat`], §8.6).
pub fn database_row_fragment() -> FragmentDef {
    // The row-level read rewrite (`direct_reader ∪ parent_page->read`), shared by `read` (the
    // `list_objects` row ACL) AND `view_field` (the row-read precondition the field caveat rides on).
    let row_read = || Userset::Union(vec![rel(DIRECT_READER), ttu("parent_page", READ)]);
    frag(
        object_types::DATABASE_ROW,
        &[DIRECT_READER, "parent_page"],
        vec![
            perm(READ, row_read()),
            // `view_field` is the field-level read GATE (§8.6): it resolves to the SAME row-read
            // grant (you may attempt to view a column only on a row you can read), and the off-hot-
            // path `CaveatContext` caveat then redacts the individual COLUMN on top. Modelling it as a
            // compiled permission (NOT a bespoke relation grant) means the field caveat is evaluated
            // on an ALREADY-readable row — exactly the §8.6 "on the already-filtered rows" semantics —
            // so a viewer who cannot read the row is denied the field by the grant, and a viewer who
            // can read it has the column gated solely by the caveat (Allow/Deny/Conditional).
            perm(VIEW_FIELD, row_read()),
        ],
    )
    .watchable()
}

/// **The complete compiled Knowledge ReBAC namespace fragment (contract 4.9)** — the four rich
/// [`FragmentDef`]s Identity admits into the one cell schema, in parent-before-child order (`space` →
/// `page` → `block` → `database_row`) so each inheritance edge's parent type is already in the schema
/// when its child admits. This is the SINGLE entry point [`crate::StoreBackedCheck::admit_knowledge_fragment`]
/// and the CDC test consume.
pub fn knowledge_fragment() -> Vec<FragmentDef> {
    vec![
        space_fragment(),
        page_fragment(),
        block_fragment(),
        database_row_fragment(),
    ]
}

/// **Build the field-level column-hiding caveat (§8.6, C3) for a `database_row` field — a
/// `CaveatContext` over the ONE `QueryAst` predicate core.**
///
/// Field-level column hiding is NOT a namespace permission (that is row visibility, the
/// `list_objects` push-down). It is an off-hot-path ABAC rider evaluated at `check`-time on an
/// ALREADY-FILTERED, already-fetched row: `check(subject, view_field, row, CaveatContext)` then
/// redacts the individual COLUMN. This helper builds the `CaveatContext` the Knowledge subsystem
/// passes to `check` for a column gated by a predicate over a runtime attribute — e.g. "the `salary`
/// column is visible iff the viewer's `clearance` attribute ≥ the row's `min_clearance`", a genuinely
/// NON-LITERAL predicate whose operands are resolved from `attrs` at eval time.
///
/// The produced `CaveatContext` carries the field id + the `attrs` the predicate reads; the predicate
/// itself is lowered through the SAME `__caveat_*`/`_var` encoding the M1 bridge
/// ([`crate::check_engine::eval_caveat`]) routes through the ONE `myelin_query` interpreter (no second
/// predicate language, EI-01 §7). A column whose predicate is VIOLATED is `Deny` (redacted, absent
/// from any count); a column whose predicate references context the caller did not supply is
/// `Conditional` (the caller supplies it) — **never a silent `Allow`** (§8.6).
///
/// `row` is the `database_row` object id the field belongs to; `field` is the column name; `op` is a
/// comparison operator name (`eq`/`ne`/`lt`/`le`/`gt`/`ge`); `lhs_var` is a context variable the
/// predicate reads (the non-literal operand — e.g. `clearance`); `rhs` is the literal threshold the
/// variable is compared against (e.g. the row's `min_clearance`). `ctx` supplies the runtime values
/// for `lhs_var` (and any other variables) the caller fetched on the row.
pub fn field_view_caveat(
    row: &str,
    field: &str,
    op: &str,
    lhs_var: &str,
    rhs: Literal,
    ctx: &[(&str, Literal)],
) -> CaveatContext {
    let mut attrs: BTreeMap<String, Literal> = BTreeMap::new();
    // The predicate, in the frozen self-describing `__caveat_*` encoding the ONE-core bridge lowers
    // to a `myelin_query::Predicate` and routes through the single interpreter (§8.6; no second
    // predicate language). The lhs is a CONTEXT VARIABLE (the non-literal field-caveat shape Knowledge
    // needs); the rhs is the literal threshold.
    attrs.insert("__caveat_op".into(), Literal::Str(op.into()));
    attrs.insert("__caveat_lhs_var".into(), Literal::Str(lhs_var.into()));
    attrs.insert("__caveat_rhs".into(), rhs);
    // The runtime context the predicate's variable(s) read (the values the caller fetched on the
    // already-filtered row). An unbound variable surfaces as Conditional, never a silent allow.
    for (k, v) in ctx {
        attrs.insert((*k).to_string(), v.clone());
    }
    CaveatContext {
        object: ArtifactRef(row.to_string()),
        field: Some(FieldId(field.to_string())),
        transition: None,
        attrs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::NamespaceEngine;
    use myelin_identity::FragmentAdmit;

    /// **The compiled Knowledge fragment admits into the cell schema (the engine-only-floor
    /// progression).** Every Knowledge object type admits on top of the core org/team/project
    /// hierarchy; the four types enter the compiled vocabulary; `page.read` + `database_row.read`
    /// resolve as compiled permissions.
    #[test]
    fn knowledge_fragment_admits_into_the_cell_schema() {
        let mut eng = NamespaceEngine::with_core_hierarchy();
        for def in knowledge_fragment() {
            let admit = eng.admit(&def);
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the Knowledge `{}` fragment admits into the cell schema: {admit:?}",
                def.object_type.0
            );
        }
        for ty in ["space", "page", "block", "database_row"] {
            assert!(
                eng.object_types().contains(&ty.to_string()),
                "`{ty}` is admitted"
            );
        }
        assert!(
            eng.resolve_permission("page", READ).is_some(),
            "page.read is a compiled permission"
        );
        assert!(
            eng.resolve_permission("database_row", READ).is_some(),
            "database_row.read is a compiled permission (the row-level ACL)"
        );
    }

    /// **`page.read` is the page-tree-with-overrides rewrite (§5): an EXCLUSION of `direct_block` over
    /// the inheritance union.** The override-exclusion is the mutation-tested core: the rewrite MUST be
    /// `(… ∪ direct_reader) − direct_block`, not a bare union (a mutation dropping the exclusion is
    /// caught here — and behaviourally in the CDC).
    #[test]
    fn page_read_is_inheritance_union_minus_direct_block() {
        let page = page_fragment();
        let read = page
            .permissions
            .iter()
            .find(|p| p.permission.0 == READ)
            .expect("page declares read");
        match &read.rewrite {
            Userset::Exclusion { base, subtracted } => {
                // The subtracted arm is exactly `direct_block` (the override).
                assert_eq!(
                    **subtracted,
                    rel(DIRECT_BLOCK),
                    "the override subtracts direct_block (the - direct_block §5 rewrite)"
                );
                // The base is the inheritance ∪ direct grant union.
                match &**base {
                    Userset::Union(arms) => {
                        assert!(
                            arms.contains(&ttu("parent_page", READ)),
                            "inherits parent_page->read"
                        );
                        assert!(
                            arms.contains(&rel(DIRECT_READER)),
                            "unions the direct_reader grant"
                        );
                    }
                    other => panic!("page.read base must be the inheritance union, got {other:?}"),
                }
            }
            other => panic!("page.read must be an Exclusion (- direct_block), got {other:?}"),
        }
    }

    /// **`database_row.read` is the row-level ACL union (`direct_reader ∪ parent_page->read`).** The
    /// row a `list_objects` push-down keys on via `db_row.id` (§7.3).
    #[test]
    fn database_row_read_is_direct_reader_union_parent_page() {
        let row = database_row_fragment();
        let read = row
            .permissions
            .iter()
            .find(|p| p.permission.0 == READ)
            .expect("database_row declares read");
        assert_eq!(
            read.rewrite,
            Userset::Union(vec![rel(DIRECT_READER), ttu("parent_page", READ)]),
            "database_row.read = direct_reader ∪ parent_page->read (the row-level ACL)"
        );
    }

    /// **`space`/`page`/`database_row` are WATCHABLE (C8): each declares the `watcher` relation** so
    /// Notif's read-fanout `list_subjects(object, watcher)` is an ordinary Expand. `block` is NOT
    /// independently watchable (it inherits its page's ACL; a watcher fans out at page granularity).
    #[test]
    fn watchable_knowledge_types_declare_the_watcher_relation() {
        assert!(space_fragment().is_watchable(), "space is watchable");
        assert!(page_fragment().is_watchable(), "page is watchable");
        assert!(
            database_row_fragment().is_watchable(),
            "database_row is watchable"
        );
        assert!(
            !block_fragment().is_watchable(),
            "block is not independently watchable"
        );
    }

    /// **The field-view caveat is a NON-LITERAL predicate over the ONE `QueryAst` core, and a violated
    /// column is `Deny` (redacted) — absent from any count, never a silent allow (§8.6, C3).** Build a
    /// `salary`-column caveat "visible iff clearance ≥ 3"; a viewer with clearance 4 sees it (Allow), a
    /// viewer with clearance 1 has it redacted (Deny), and a viewer whose clearance is NOT supplied is
    /// Conditional (the caller supplies it) — the mandatory-core no-silent-allow branch.
    #[test]
    fn field_view_caveat_hides_a_column_through_the_one_query_core() {
        use crate::check_engine::eval_caveat;
        use myelin_identity::Decision;

        // The cleared viewer: clearance 4 ≥ threshold 3 → the salary column is visible (Allow).
        let cleared = field_view_caveat(
            "database_row:emp-1",
            "salary",
            "ge",
            "clearance",
            Literal::Int(3),
            &[("clearance", Literal::Int(4))],
        );
        assert_eq!(
            eval_caveat(&cleared),
            Decision::Allow,
            "cleared viewer sees the salary column"
        );

        // The under-cleared viewer: clearance 1 < threshold 3 → the column is REDACTED (Deny) — it is
        // absent from the viewer's projection AND any count (no count leak).
        let blocked = field_view_caveat(
            "database_row:emp-1",
            "salary",
            "ge",
            "clearance",
            Literal::Int(3),
            &[("clearance", Literal::Int(1))],
        );
        assert_eq!(
            eval_caveat(&blocked),
            Decision::Deny,
            "under-cleared viewer's salary column is redacted"
        );

        // The viewer whose clearance is NOT supplied → Conditional (the caller supplies it) — NEVER a
        // silent Allow (the mutation-tested mandatory-core branch).
        let missing = field_view_caveat(
            "database_row:emp-1",
            "salary",
            "ge",
            "clearance",
            Literal::Int(3),
            &[], // no clearance attr supplied
        );
        assert_eq!(
            eval_caveat(&missing),
            Decision::Conditional,
            "a field caveat needing missing context is Conditional, never a silent allow (§8.6)"
        );
    }

    /// **No Knowledge fragment name smuggles an object id (Id never invents object ids).** Every
    /// type/relation/permission name is a bare identifier — the engine's `mints_object_id` admit check
    /// would reject one that wasn't.
    #[test]
    fn no_knowledge_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in knowledge_fragment() {
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
