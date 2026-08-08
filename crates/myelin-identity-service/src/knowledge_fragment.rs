use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{CaveatContext, FieldId, Literal, ObjectType, Permission, RelName};
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeMap;

pub mod object_types {
    pub const SPACE: &str = "space";
    pub const PAGE: &str = "page";
    pub const BLOCK: &str = "block";
    pub const DATABASE_ROW: &str = "database_row";
}

pub const DIRECT_READER: &str = "direct_reader";
pub const DIRECT_EDITOR: &str = "direct_editor";

pub const DIRECT_BLOCK: &str = "direct_block";

pub const READ: &str = "read";
pub const EDIT: &str = "edit";

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

pub fn space_fragment() -> FragmentDef {
    frag(
        object_types::SPACE,
        &[DIRECT_READER, DIRECT_EDITOR, "member"],
        vec![
            perm(
                READ,
                Userset::Union(vec![rel(DIRECT_READER), rel(DIRECT_EDITOR), rel("member")]),
            ),
            perm(
                EDIT,
                Userset::Union(vec![rel(DIRECT_EDITOR), rel("member")]),
            ),
        ],
    )
    .watchable()
}

pub fn page_fragment() -> FragmentDef {
    frag(
        object_types::PAGE,
        &[
            "parent_page",
            "parent_space",
            DIRECT_READER,
            DIRECT_EDITOR,
            DIRECT_BLOCK,
        ],
        vec![
            perm(
                READ,
                Userset::Exclusion {
                    base: Box::new(Userset::Union(vec![
                        ttu("parent_page", READ),
                        ttu("parent_space", READ),
                        rel(DIRECT_READER),
                        rel(DIRECT_EDITOR),
                    ])),
                    subtracted: Box::new(rel(DIRECT_BLOCK)),
                },
            ),
            perm(
                EDIT,
                Userset::Union(vec![
                    ttu("parent_page", EDIT),
                    ttu("parent_space", EDIT),
                    rel(DIRECT_EDITOR),
                ]),
            ),
        ],
    )
    .watchable()
}

pub fn block_fragment() -> FragmentDef {
    frag(
        object_types::BLOCK,
        &["parent_page"],
        vec![
            perm(READ, ttu("parent_page", READ)),
            perm(EDIT, ttu("parent_page", EDIT)),
        ],
    )
}

pub fn database_row_fragment() -> FragmentDef {
    let row_read = || Userset::Union(vec![rel(DIRECT_READER), ttu("parent_page", READ)]);
    frag(
        object_types::DATABASE_ROW,
        &[DIRECT_READER, DIRECT_EDITOR, "parent_page"],
        vec![
            perm(READ, row_read()),
            perm(
                EDIT,
                Userset::Union(vec![rel(DIRECT_EDITOR), ttu("parent_page", EDIT)]),
            ),
            perm(VIEW_FIELD, row_read()),
        ],
    )
    .watchable()
}

pub fn knowledge_fragment() -> Vec<FragmentDef> {
    vec![
        space_fragment(),
        page_fragment(),
        block_fragment(),
        database_row_fragment(),
    ]
}

pub fn field_view_caveat(
    row: &str,
    field: &str,
    op: &str,
    lhs_var: &str,
    rhs: Literal,
    ctx: &[(&str, Literal)],
) -> CaveatContext {
    let mut attrs: BTreeMap<String, Literal> = BTreeMap::new();
    attrs.insert("__caveat_op".into(), Literal::Str(op.into()));
    attrs.insert("__caveat_lhs_var".into(), Literal::Str(lhs_var.into()));
    attrs.insert("__caveat_rhs".into(), rhs);
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
            eng.resolve_permission("page", EDIT).is_some(),
            "page.edit is a compiled permission"
        );
        assert!(
            eng.resolve_permission("database_row", READ).is_some(),
            "database_row.read is a compiled permission (the row-level ACL)"
        );
        assert!(
            eng.resolve_permission("database_row", EDIT).is_some(),
            "database_row.edit is a compiled permission"
        );
    }

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
                assert_eq!(
                    **subtracted,
                    rel(DIRECT_BLOCK),
                    "the override subtracts direct_block (the - direct_block §5 rewrite)"
                );
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

    #[test]
    fn field_view_caveat_hides_a_column_through_the_one_query_core() {
        use crate::check_engine::eval_caveat;
        use myelin_identity::Decision;

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

        let missing = field_view_caveat(
            "database_row:emp-1",
            "salary",
            "ge",
            "clearance",
            Literal::Int(3),
            &[],
        );
        assert_eq!(
            eval_caveat(&missing),
            Decision::Conditional,
            "a field caveat needing missing context is Conditional, never a silent allow (§8.6)"
        );
    }

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
