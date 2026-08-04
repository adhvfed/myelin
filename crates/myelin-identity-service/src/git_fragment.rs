use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{ObjectId, ObjectType, Permission, PrincipalId, RelName, RelationTuple};

pub mod object_types {
    pub const REPO: &str = "repo";
    pub const REF: &str = "ref";
    pub const PULL_REQUEST: &str = "pull_request";
    pub const PR_COMMENT: &str = "pr_comment";
}

pub const APPROVE_UNTRUSTED_CI: &str = "approve_untrusted_ci";

pub const CODE_OWNER: &str = "code_owner";

pub const PROTECTED_PUSH: &str = "protected_push";

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

pub fn repo_fragment() -> FragmentDef {
    frag(
        object_types::REPO,
        &[
            "parent_project",
            "reader",
            "writer",
            "admin",
            APPROVE_UNTRUSTED_CI,
            "watcher",
        ],
        vec![
            perm(
                "pull",
                Userset::Union(vec![
                    rel("reader"),
                    rel("writer"),
                    rel("admin"),
                    ttu("parent_project", "view"),
                ]),
            ),
            perm(
                "push",
                Userset::Union(vec![
                    rel("writer"),
                    rel("admin"),
                    ttu("parent_project", "view"),
                ]),
            ),
            perm(
                "administer",
                Userset::Union(vec![rel("admin"), ttu("parent_project", "view")]),
            ),
            perm(PROTECTED_PUSH, rel("admin")),
        ],
    )
}

pub fn ref_fragment() -> FragmentDef {
    frag(
        object_types::REF,
        &["parent_repo", "bypass", CODE_OWNER],
        vec![perm(
            "push_protected",
            Userset::Union(vec![rel("bypass"), ttu("parent_repo", "administer")]),
        )],
    )
}

pub fn pull_request_fragment() -> FragmentDef {
    frag(
        object_types::PULL_REQUEST,
        &["parent_repo", "author", "reviewer", "watcher"],
        vec![
            perm("view", ttu("parent_repo", "pull")),
            perm(
                "review",
                Userset::Union(vec![rel("reviewer"), ttu("parent_repo", "push")]),
            ),
            perm("merge", ttu("parent_repo", PROTECTED_PUSH)),
        ],
    )
}

pub fn pr_comment_fragment() -> FragmentDef {
    frag(
        object_types::PR_COMMENT,
        &["parent_pr"],
        vec![perm("view", ttu("parent_pr", "view"))],
    )
}

pub fn git_fragment() -> Vec<FragmentDef> {
    vec![
        repo_fragment(),
        ref_fragment(),
        pull_request_fragment(),
        pr_comment_fragment(),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeownersRule {
    pub path_glob: String,
    pub owners: Vec<PrincipalId>,
}

pub fn compile_codeowners(repo: &str, rules: &[CodeownersRule]) -> Vec<RelationTuple> {
    let mut tuples = Vec::new();
    for rule in rules {
        let ref_id = format!("{}:{}::{}", object_types::REF, repo, rule.path_glob);
        for owner in &rule.owners {
            tuples.push(RelationTuple {
                object: ObjectId(ref_id.clone()),
                relation: RelName(CODE_OWNER.to_string()),
                subject: owner.clone(),
                caveat: None,
            });
        }
    }
    tuples
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::NamespaceEngine;
    use myelin_identity::FragmentAdmit;

    #[test]
    fn git_fragment_admits_into_the_cell_schema() {
        let mut eng = NamespaceEngine::with_core_hierarchy();
        for def in git_fragment() {
            let admit = eng.admit(&def);
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the Git `{}` fragment admits into the cell schema: {admit:?}",
                def.object_type.0
            );
        }
        for ty in ["repo", "ref", "pull_request", "pr_comment"] {
            assert!(
                eng.object_types().contains(&ty.to_string()),
                "`{ty}` is admitted"
            );
        }
        assert!(eng.resolve_permission("repo", PROTECTED_PUSH).is_some());
        assert!(eng.resolve_permission("pull_request", "merge").is_some());
    }

    #[test]
    fn pull_request_merge_resolves_via_protected_push() {
        let merge = pull_request_fragment()
            .permissions
            .into_iter()
            .find(|p| p.permission.0 == "merge")
            .expect("pull_request declares merge");
        assert_eq!(
            merge.rewrite,
            ttu("parent_repo", PROTECTED_PUSH),
            "merge = parent_repo->protected_push (§5, frozen)"
        );
    }

    #[test]
    fn codeowners_glob_compiles_to_reviewer_tuples() {
        let rules = vec![CodeownersRule {
            path_glob: "/src/payments/**".into(),
            owners: vec![
                PrincipalId("p:alice".into()),
                PrincipalId("team:payments".into()),
            ],
        }];
        let tuples = compile_codeowners("repo:core", &rules);
        assert_eq!(
            tuples.len(),
            2,
            "two owners → two reviewer-requirement tuples"
        );
        for t in &tuples {
            assert_eq!(
                t.relation,
                RelName(CODE_OWNER.into()),
                "each is a code_owner tuple"
            );
            assert_eq!(
                t.object,
                ObjectId("ref:repo:core::/src/payments/**".into()),
                "the ref id encodes the repo + the path-glob scope (glob recoverable)"
            );
        }
        let subjects: Vec<&str> = tuples.iter().map(|t| t.subject.0.as_str()).collect();
        assert!(subjects.contains(&"p:alice"));
        assert!(subjects.contains(&"team:payments"));
    }

    #[test]
    fn approve_untrusted_ci_is_a_plain_repo_relation() {
        let repo = repo_fragment();
        assert!(
            repo.relations
                .contains(&RelName(APPROVE_UNTRUSTED_CI.into())),
            "approve_untrusted_ci is a declared repo relation (X-1)"
        );
        assert!(
            !repo.permissions.iter().any(|p| p.permission.0 == APPROVE_UNTRUSTED_CI),
            "approve_untrusted_ci is a relation, not a permission - a plain check, not bespoke logic"
        );
    }

    #[test]
    fn no_git_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in git_fragment() {
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
