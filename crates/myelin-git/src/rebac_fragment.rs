use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

pub mod object_types {
    pub const REPO: &str = "repo";
    pub const REF: &str = "ref";
    pub const PULL_REQUEST: &str = "pull_request";
    pub const PR_COMMENT: &str = "pr_comment";
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

pub fn repo_fragment() -> NamespaceFragment {
    fragment(
        object_types::REPO,
        &[
            "parent_project",
            "reader",
            "writer",
            "admin",
            "approve_untrusted_ci",
            "watcher",
        ],
        &["pull", "push", "administer", "protected_push"],
    )
}

pub fn ref_fragment() -> NamespaceFragment {
    fragment(
        object_types::REF,
        &["parent_repo", "bypass", "code_owner"],
        &["push_protected"],
    )
}

pub fn pull_request_fragment() -> NamespaceFragment {
    fragment(
        object_types::PULL_REQUEST,
        &["parent_repo", "author", "reviewer", "watcher"],
        &["view", "review", "merge"],
    )
}

pub fn pr_comment_fragment() -> NamespaceFragment {
    fragment(object_types::PR_COMMENT, &["parent_pr"], &["view"])
}

pub fn git_fragment() -> Vec<NamespaceFragment> {
    vec![
        repo_fragment(),
        ref_fragment(),
        pull_request_fragment(),
        pr_comment_fragment(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_definition_declares_its_frozen_relations() {
        let repo = repo_fragment();
        let repo_rels: Vec<&str> = repo.relations.iter().map(|r| r.0.as_str()).collect();
        for expected in [
            "parent_project",
            "reader",
            "writer",
            "admin",
            "approve_untrusted_ci",
            "watcher",
        ] {
            assert!(
                repo_rels.contains(&expected),
                "repo must declare the `{expected}` relation (§5.2)"
            );
        }

        let r = ref_fragment();
        assert!(
            r.relations.contains(&RelName("code_owner".into())),
            "the CODEOWNERS-as-relations `code_owner` relation is on `ref`"
        );
        assert!(r.relations.contains(&RelName("bypass".into())));

        assert!(repo.relations.contains(&RelName("watcher".into())));
        assert!(
            pull_request_fragment()
                .relations
                .contains(&RelName("watcher".into())),
            "the `pull_request` watchable type declares `watcher` (Notif read-fanout)"
        );
    }

    #[test]
    fn approve_untrusted_ci_is_a_plain_repo_relation() {
        assert!(
            repo_fragment()
                .relations
                .contains(&RelName("approve_untrusted_ci".into())),
            "approve_untrusted_ci is an ordinary relation on repo (X-1)"
        );
    }

    #[test]
    fn the_four_git_object_types_are_frozen() {
        let frag = git_fragment();
        let types: Vec<&str> = frag.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, vec!["repo", "ref", "pull_request", "pr_comment"]);
        assert!(repo_fragment()
            .permissions
            .contains(&Permission("protected_push".into())));
        assert!(pull_request_fragment()
            .permissions
            .contains(&Permission("merge".into())));
    }

    #[test]
    fn no_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in git_fragment() {
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
