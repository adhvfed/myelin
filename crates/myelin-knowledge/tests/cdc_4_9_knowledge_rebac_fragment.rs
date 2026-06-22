//! # The CDC pair for contract 4.9 (Knowledge half) — the **Knowledge ReBAC page-tree namespace
//! fragment** (KN-P15 → P-305, M3)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem DECLARES its
//! relations + permissions; Identity owns the engine + the admit-contract + the core hierarchy and
//! never invents object ids). The ENGINE half (Id's compiled rich rewrites — the page-tree-with-
//! overrides `Userset::Exclusion`, the row ACL, the field caveat) is pinned BEHAVIOURALLY by the
//! engine crate's `cdc_4_9_knowledge_fragment.rs` (P-249). THIS file is the **Knowledge subsystem
//! side**: the CONSUMER declares its read-side fragment vocabulary + proves the §5 page.read OVERRIDE
//! FORMULA (the headline drill) + the row-level `SetExpr` push-down SHAPE + the no-cross-db lint.
//!
//! ## The name-agreement anchor (why this side does not import the engine crate)
//! `myelin-knowledge` is a LEAF service consumer; the §2.9 acyclic DAG forbids it depending on the
//! ReBAC ENGINE (`myelin-identity-service`). The PROVIDER (the engine's compiled rewrites) cannot be
//! imported here, so the name-agreement is asserted against the architecture §5 frozen vocabulary
//! literals AND against the SAME crate's WRITE-side carrier ([`myelin_content::rebac_fragment`]) — one
//! source of truth across the two leaf carriers. The engine side's CDC asserts the SAME literals
//! against its rich `FragmentDef`; a rename on either side is a CDC break, never a silent drift
//! (EI-01 §7). The override FORMULA the engine resolves over tuples (`Userset::Exclusion`) is the
//! closed-form proven here directly.

use myelin_identity::SetExpr;
use myelin_knowledge::rebac_fragment::{
    self, block_read_fragment, database_row_read_fragment, field_view_permission,
    knowledge_read_fragment, object_types, page_read_fragment, page_read_override,
    row_reader_set_expr, space_read_fragment, DIRECT_BLOCK, DIRECT_READER, EDIT, PARENT_PAGE,
    PARENT_SPACE, READ, ROW_READER, VIEW_FIELD, WATCHER,
};

/// **CONSUMER → the Knowledge read fragment declares the four §5 object types in parent-before-child
/// order.** The vocabulary Identity admits into the cell schema (`space` → `page` → `block` →
/// `database_row`); each inheritance edge's parent type precedes its child.
#[test]
fn cdc_4_9_knowledge_declares_the_four_object_types() {
    let frags = knowledge_read_fragment();
    let types: Vec<String> = frags.iter().map(|f| f.object_type.0.clone()).collect();
    assert_eq!(
        types,
        ["space", "page", "block", "database_row"],
        "the four frozen Knowledge object types, parent-before-child"
    );
}

/// **CONSUMER ↔ the canonical §5 vocabulary: the relation/permission NAMES are frozen and AGREE with
/// the write-side carrier.** The object-type names are byte-identical to
/// [`myelin_content::rebac_fragment`] (the write-side `page` carrier); the read relations + read
/// permissions are the §5 literals the engine's rich fragment resolves over.
#[test]
fn cdc_4_9_read_names_agree_with_the_write_carrier_and_the_frozen_vocabulary() {
    // object types agree across the two leaf carriers (one source of truth).
    assert_eq!(
        object_types::PAGE,
        myelin_content::rebac_fragment::object_types::PAGE
    );
    assert_eq!(
        object_types::SPACE,
        myelin_content::rebac_fragment::object_types::SPACE
    );
    assert_eq!(
        object_types::BLOCK,
        myelin_content::rebac_fragment::object_types::BLOCK
    );
    assert_eq!(
        object_types::DATABASE_ROW,
        myelin_content::rebac_fragment::object_types::DATABASE_ROW
    );

    // the page declares the inheritance + override + grant relations and the read permissions (§5).
    let page = page_read_fragment();
    for r in [
        PARENT_PAGE,
        PARENT_SPACE,
        DIRECT_READER,
        DIRECT_BLOCK,
        WATCHER,
    ] {
        assert!(
            page.relations.iter().any(|rel| rel.0 == r),
            "page declares relation `{r}`"
        );
    }
    for p in [READ, "comment", EDIT] {
        assert!(
            page.permissions.iter().any(|perm| perm.0 == p),
            "page declares permission `{p}`"
        );
    }

    // the space + block + database_row vocabularies (§5).
    assert!(space_read_fragment()
        .permissions
        .iter()
        .any(|p| p.0 == READ));
    assert!(block_read_fragment()
        .relations
        .iter()
        .any(|r| r.0 == PARENT_PAGE));
    let row = database_row_read_fragment();
    assert!(
        row.relations.iter().any(|r| r.0 == ROW_READER),
        "database_row declares row_reader"
    );
    assert!(
        row.permissions.iter().any(|p| p.0 == VIEW_FIELD),
        "database_row declares view_field"
    );
}

/// **CONSUMER → the page.read OVERRIDE-FORMULA gate (§5, the KN-P15 headline drill).** `direct_block`
/// removes a narrowed sub-page from a viewer's reachable set; a `direct_reader` on a sub-page adds it;
/// the override is applied LAST so it wins over both inheritance and a direct grant. This is the
/// closed-form the engine's `Userset::Exclusion { base: (… ∪ direct_reader), subtracted: direct_block }`
/// resolves over tuples (the engine-side `cdc_4_9_direct_block_override_narrows_inherited_access`).
#[test]
fn cdc_4_9_page_read_override_formula_is_correct() {
    // direct_block REMOVES an inheriting reader (the sub-page narrows inherited access).
    let blocked = page_read_override(&["alice", "bob"], &[], &["alice"]);
    assert!(
        !blocked.contains("alice"),
        "the - direct_block override removes the inheriting alice"
    );
    assert!(
        blocked.contains("bob"),
        "an un-blocked inheriting reader stays"
    );

    // direct_reader ADDS a sub-page (the + direct_reader arm).
    let added = page_read_override(&["alice"], &["carol"], &[]);
    assert!(
        added.contains("carol"),
        "the + direct_reader arm adds carol"
    );

    // the exclusion is applied LAST → it overrides even a direct grant on the SAME page.
    let over = page_read_override(&[], &["mallory"], &["mallory"]);
    assert!(
        !over.contains("mallory"),
        "- direct_block overrides even a direct_reader grant (the exclusion wins)"
    );

    // inheritance-only (no override, no extra grant) is the inherited set verbatim (the block shape).
    let plain = page_read_override(&["alice", "bob"], &[], &[]);
    assert_eq!(
        plain.len(),
        2,
        "with no override, read is exactly the inherited set"
    );
}

/// **CONSUMER → the row-level `SetExpr` push-down SHAPE (§5.1, the KN-P16 lowering target).** The
/// `row_reader` group grant lowers as `InRelation { relation: row_reader, via_column: db_row.id }` —
/// a JOIN against the per-tenant authz reverse index, ONE query, no N+1. The SQL lowering (the
/// zero-leak-incl-COUNT KN-D5 gate) is **KN-P16's deliverable (P-306, the named follow-on)**; this
/// pins the shape it consumes.
#[test]
fn cdc_4_9_row_level_push_down_shape_is_inrelation_over_db_row_id() {
    match row_reader_set_expr() {
        SetExpr::InRelation {
            relation,
            via_column,
        } => {
            assert_eq!(relation.0, "row_reader");
            assert_eq!(via_column.table, "db_row");
            assert_eq!(
                via_column.column, "id",
                "the via_column the JOIN keys on (no N+1)"
            );
        }
        other => panic!("the row-level push-down must be an InRelation, got {other:?}"),
    }
    // the field-level gate rides the `view_field` permission (off the hot list_objects path, §5.1).
    assert_eq!(field_view_permission().0, "view_field");
}

/// **CONSUMER → no Knowledge fragment name smuggles an object id (Id never invents object ids).**
/// Every declared type/relation/permission is a bare identifier — the engine's admit would reject one
/// that wasn't.
#[test]
fn cdc_4_9_no_knowledge_name_smuggles_an_object_id() {
    let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
    for f in knowledge_read_fragment() {
        assert!(
            !mints(&f.object_type.0),
            "type `{}` is a bare identifier",
            f.object_type.0
        );
        for r in &f.relations {
            assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
        }
        for p in &f.permissions {
            assert!(!mints(&p.0), "permission `{}` is a bare identifier", p.0);
        }
    }
    let _ = rebac_fragment::object_types::PAGE; // module-path reachability
}

/// **The no-cross-db lint is GREEN over the Knowledge fragment module (the §5 design rule: Knowledge
/// DECLARES relations, it never reads another owner's DB).** The fragment is a pure declaration over
/// the `myelin_identity` ABI types — it imports NO sibling `::storage`/`::store`/`::db` data path. The
/// lint (the live workspace-scan rule) is empty on the module source; a deliberately cross-DB import
/// is RED (the fixture half).
#[test]
fn cdc_4_9_no_cross_db_lint_is_green_on_the_fragment() {
    let lint = myelin_lints::lints::no_cross_db();

    // GREEN: the actual fragment module imports only the frozen contract ABI — no sibling data path.
    let green = include_str!("../src/rebac_fragment.rs");
    assert!(
        lint.run(green).is_empty(),
        "the Knowledge ReBAC fragment declares relations over the contract ABI — no cross-DB reach"
    );

    // RED (the fixture half): a fragment that reached into another owner's store would be flagged.
    let red = "use myelin_identity::store::TupleStore;\n";
    assert!(
        !lint.run(red).is_empty(),
        "a fragment reaching into another owner's `::store` data path is RED (no-cross-db)"
    );
}
