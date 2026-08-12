use myelin_identity::SetExpr;
use myelin_knowledge::rebac_fragment::{
    self, block_read_fragment, database_row_read_fragment, field_view_permission,
    knowledge_read_fragment, object_types, page_read_fragment, page_read_override,
    row_reader_set_expr, space_read_fragment, DIRECT_BLOCK, DIRECT_READER, EDIT, PARENT_PAGE,
    PARENT_SPACE, READ, ROW_READER, VIEW_FIELD, WATCHER,
};

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

#[test]
fn cdc_4_9_read_names_agree_with_the_write_carrier_and_the_frozen_vocabulary() {
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

#[test]
fn cdc_4_9_page_read_override_formula_is_correct() {
    let blocked = page_read_override(&["alice", "bob"], &[], &["alice"]);
    assert!(
        !blocked.contains("alice"),
        "the - direct_block override removes the inheriting alice"
    );
    assert!(
        blocked.contains("bob"),
        "an un-blocked inheriting reader stays"
    );

    let added = page_read_override(&["alice"], &["carol"], &[]);
    assert!(
        added.contains("carol"),
        "the + direct_reader arm adds carol"
    );

    let over = page_read_override(&[], &["mallory"], &["mallory"]);
    assert!(
        !over.contains("mallory"),
        "- direct_block overrides even a direct_reader grant (the exclusion wins)"
    );

    let plain = page_read_override(&["alice", "bob"], &[], &[]);
    assert_eq!(
        plain.len(),
        2,
        "with no override, read is exactly the inherited set"
    );
}

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
    assert_eq!(field_view_permission().0, "view_field");
}

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
    let _ = rebac_fragment::object_types::PAGE;
}
