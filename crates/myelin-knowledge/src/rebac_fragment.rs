use myelin_identity::{ColRef, NamespaceFragment, ObjectType, Permission, RelName, SetExpr};
use std::collections::BTreeSet;

pub mod object_types {
    pub const SPACE: &str = "space";
    pub const PAGE: &str = "page";
    pub const BLOCK: &str = "block";
    pub const DATABASE_ROW: &str = "database_row";
}

pub const PARENT_PAGE: &str = "parent_page";
pub const PARENT_SPACE: &str = "parent_space";
pub const PARENT_DB: &str = "parent_db";
pub const DIRECT_READER: &str = "direct_reader";
pub const DIRECT_BLOCK: &str = "direct_block";
pub const ROW_READER: &str = "row_reader";
pub const WATCHER: &str = "watcher";

pub const READ: &str = "read";
pub const COMMENT: &str = "comment";
pub const EDIT: &str = "edit";
pub const VIEW_FIELD: &str = "view_field";

pub const DB_ROW_TABLE: &str = "db_row";
pub const DB_ROW_ID_COLUMN: &str = "id";

fn fragment(object_type: &str, relations: &[&str], permissions: &[&str]) -> NamespaceFragment {
    NamespaceFragment {
        object_type: ObjectType(object_type.to_string()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions: permissions
            .iter()
            .map(|p| Permission(p.to_string()))
            .collect(),
    }
}

pub fn space_read_fragment() -> NamespaceFragment {
    fragment(
        object_types::SPACE,
        &[DIRECT_READER, "member", WATCHER],
        &[READ],
    )
}

pub fn page_read_fragment() -> NamespaceFragment {
    fragment(
        object_types::PAGE,
        &[
            PARENT_PAGE,
            PARENT_SPACE,
            DIRECT_READER,
            DIRECT_BLOCK,
            WATCHER,
        ],
        &[READ, COMMENT, EDIT],
    )
}

pub fn block_read_fragment() -> NamespaceFragment {
    fragment(object_types::BLOCK, &[PARENT_PAGE], &[READ])
}

pub fn database_row_read_fragment() -> NamespaceFragment {
    fragment(
        object_types::DATABASE_ROW,
        &[PARENT_DB, DIRECT_READER, ROW_READER, WATCHER],
        &[READ, VIEW_FIELD],
    )
}

pub fn knowledge_read_fragment() -> Vec<NamespaceFragment> {
    vec![
        space_read_fragment(),
        page_read_fragment(),
        block_read_fragment(),
        database_row_read_fragment(),
    ]
}

pub fn page_read_override<S: AsRef<str>>(
    inherited: &[S],
    direct_reader: &[S],
    direct_block: &[S],
) -> BTreeSet<String> {
    let mut readers: BTreeSet<String> = BTreeSet::new();
    for s in inherited {
        readers.insert(s.as_ref().to_string());
    }
    for s in direct_reader {
        readers.insert(s.as_ref().to_string());
    }
    for s in direct_block {
        readers.remove(s.as_ref());
    }
    readers
}

pub fn row_reader_set_expr() -> SetExpr {
    SetExpr::InRelation {
        relation: RelName(ROW_READER.to_string()),
        via_column: ColRef {
            table: DB_ROW_TABLE.to_string(),
            column: DB_ROW_ID_COLUMN.to_string(),
        },
    }
}

pub fn field_view_permission() -> Permission {
    Permission(VIEW_FIELD.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_vocabulary_is_frozen() {
        let frags = knowledge_read_fragment();
        let types: Vec<&str> = frags.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(
            types,
            ["space", "page", "block", "database_row"],
            "parent-before-child order"
        );

        let page = page_read_fragment();
        for p in [READ, COMMENT, EDIT] {
            assert!(
                page.permissions.iter().any(|perm| perm.0 == p),
                "the `page` read fragment declares `{p}`"
            );
        }
        for r in [
            PARENT_PAGE,
            PARENT_SPACE,
            DIRECT_READER,
            DIRECT_BLOCK,
            WATCHER,
        ] {
            assert!(
                page.relations.iter().any(|rel| rel.0 == r),
                "page declares `{r}`"
            );
        }
        let row = database_row_read_fragment();
        assert!(
            row.relations.iter().any(|r| r.0 == ROW_READER),
            "database_row declares row_reader"
        );
        assert!(
            row.permissions.iter().any(|p| p.0 == VIEW_FIELD),
            "database_row declares view_field (the field gate)"
        );
        assert_eq!(READ, "read");
        assert_eq!(DIRECT_BLOCK, "direct_block");
        assert_eq!(ROW_READER, "row_reader");
        assert_eq!(VIEW_FIELD, "view_field");
    }

    #[test]
    fn object_type_names_are_the_canonical_kn_vocabulary() {
        assert_eq!(object_types::SPACE, "space");
        assert_eq!(object_types::PAGE, "page");
        assert_eq!(object_types::BLOCK, "block");
        assert_eq!(object_types::DATABASE_ROW, "database_row");
        assert_eq!(
            object_types::PAGE,
            myelin_content::rebac_fragment::object_types::PAGE
        );
        assert_eq!(
            object_types::SPACE,
            myelin_content::rebac_fragment::object_types::SPACE
        );
    }

    #[test]
    fn direct_block_override_removes_an_inheriting_reader() {
        let resolved = page_read_override(&["alice", "bob"], &[], &["alice"]);
        assert!(
            !resolved.contains("alice"),
            "the - direct_block override removes the inheriting alice"
        );
        assert!(
            resolved.contains("bob"),
            "an un-blocked inheriting reader stays"
        );
    }

    #[test]
    fn direct_reader_adds_a_sub_page() {
        let resolved = page_read_override(&["alice"], &["carol"], &[]);
        assert!(
            resolved.contains("carol"),
            "the + direct_reader arm adds carol"
        );
        assert!(resolved.contains("alice"), "the inheriting alice stays");
    }

    #[test]
    fn direct_block_overrides_even_a_direct_grant() {
        let resolved = page_read_override(&[], &["mallory"], &["mallory"]);
        assert!(
            !resolved.contains("mallory"),
            "the - direct_block exclusion is applied last → it overrides even a direct_reader grant"
        );
        assert!(resolved.is_empty(), "no one reads the page");
    }

    #[test]
    fn inheritance_only_is_the_inherited_set() {
        let resolved = page_read_override(&["alice", "bob"], &[], &[]);
        let expect: BTreeSet<String> = ["alice", "bob"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            resolved, expect,
            "with no override, read is the inherited set"
        );
    }

    #[test]
    fn row_reader_push_down_is_inrelation_over_db_row_id() {
        match row_reader_set_expr() {
            SetExpr::InRelation {
                relation,
                via_column,
            } => {
                assert_eq!(
                    relation.0, "row_reader",
                    "the row-level group grant relation"
                );
                assert_eq!(via_column.table, "db_row", "the conjoin table");
                assert_eq!(
                    via_column.column, "id",
                    "the via_column the JOIN keys on (no N+1)"
                );
            }
            other => panic!("the row push-down must be an InRelation, got {other:?}"),
        }
    }

    #[test]
    fn field_view_permission_is_named() {
        assert_eq!(field_view_permission().0, "view_field");
    }

    #[test]
    fn no_read_name_smuggles_an_object_id() {
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
    }
}
