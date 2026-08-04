use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

pub mod object_types {
    pub const ISSUE: &str = "issue";
    pub const ISSUE_FIELD: &str = "issue_field";
    pub const ISSUE_TRANSITION: &str = "issue_transition";
}

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

pub fn issue_fragment() -> NamespaceFragment {
    fragment(
        object_types::ISSUE,
        &[
            "parent_project",
            "assignee",
            "watcher",
            "confidential",
            "confidential_grant",
        ],
        &["view", "comment", "transition", "manage"],
    )
}

pub fn issue_field_fragment() -> NamespaceFragment {
    fragment(
        object_types::ISSUE_FIELD,
        &["parent_issue"],
        &["view_field"],
    )
}

pub fn issue_transition_fragment() -> NamespaceFragment {
    fragment(
        object_types::ISSUE_TRANSITION,
        &["parent_issue"],
        &["perform_transition"],
    )
}

pub fn issues_fragment() -> Vec<NamespaceFragment> {
    vec![
        issue_fragment(),
        issue_field_fragment(),
        issue_transition_fragment(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_definition_declares_its_frozen_relations() {
        let issue = issue_fragment();
        let issue_rels: Vec<&str> = issue.relations.iter().map(|r| r.0.as_str()).collect();
        for expected in [
            "parent_project",
            "assignee",
            "watcher",
            "confidential",
            "confidential_grant",
        ] {
            assert!(
                issue_rels.contains(&expected),
                "issue must declare the `{expected}` relation (§6.1)"
            );
        }

        assert!(issue_field_fragment()
            .relations
            .contains(&RelName("parent_issue".into())));
        assert!(issue_transition_fragment()
            .relations
            .contains(&RelName("parent_issue".into())));
    }

    #[test]
    fn the_confidential_set_difference_relations_are_declared() {
        let issue = issue_fragment();
        assert!(
            issue.relations.contains(&RelName("confidential".into())),
            "`confidential` (the SUBTRACTED arm of `view`) must be declared (§6.1)"
        );
        assert!(
            issue
                .relations
                .contains(&RelName("confidential_grant".into())),
            "`confidential_grant` (the explicit re-admit arm) must be declared (§6.1)"
        );
    }

    #[test]
    fn watcher_is_declared_on_the_watchable_issue_type() {
        assert!(
            issue_fragment()
                .relations
                .contains(&RelName("watcher".into())),
            "the `issue` watchable type declares `watcher` (Notif read-fanout)"
        );
    }

    #[test]
    fn the_three_issues_object_types_are_frozen() {
        let frag = issues_fragment();
        let types: Vec<&str> = frag.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, vec!["issue", "issue_field", "issue_transition"]);
        for p in ["view", "comment", "transition", "manage"] {
            assert!(
                issue_fragment().permissions.contains(&Permission(p.into())),
                "issue declares the `{p}` permission (§6.1)"
            );
        }
        assert!(issue_field_fragment()
            .permissions
            .contains(&Permission("view_field".into())));
        assert!(issue_transition_fragment()
            .permissions
            .contains(&Permission("perform_transition".into())));
    }

    #[test]
    fn no_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in issues_fragment() {
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
