use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

pub mod object_types {
    pub const SPACE: &str = "space";
    pub const PAGE: &str = "page";
    pub const BLOCK: &str = "block";
    pub const DATABASE_ROW: &str = "database_row";
}

pub const PUBLISH: &str = "publish";

pub const EDIT: &str = "edit";

pub const DRAFT: &str = "draft";

pub const COMMENT: &str = "comment";

pub const READ: &str = "read";

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

pub fn page_write_fragment() -> NamespaceFragment {
    fragment(
        object_types::PAGE,
        &["direct_writer", "parent_page", "parent_space", "watcher"],
        &[PUBLISH, EDIT, DRAFT, COMMENT, READ],
    )
}

pub fn knowledge_write_fragment() -> Vec<NamespaceFragment> {
    vec![page_write_fragment()]
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(PUBLISH, "publish");
        assert_eq!(EDIT, "edit");
        assert_eq!(DRAFT, "draft");
        assert_eq!(COMMENT, "comment");
    }

    #[test]
    fn object_type_names_are_the_canonical_kn_vocabulary() {
        assert_eq!(object_types::SPACE, "space");
        assert_eq!(object_types::PAGE, "page");
        assert_eq!(object_types::BLOCK, "block");
        assert_eq!(object_types::DATABASE_ROW, "database_row");
    }

    #[test]
    fn no_kn_write_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in knowledge_write_fragment() {
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
