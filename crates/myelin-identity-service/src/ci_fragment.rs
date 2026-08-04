use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{ObjectType, Permission, RelName};

pub mod object_types {
    pub const CI_PROJECT: &str = "ci_project";
    pub const ENVIRONMENT: &str = "environment";
    pub const SECRET: &str = "secret";
    pub const RUN: &str = "run";
}

pub const SECRET_DIRECT_READER: &str = "direct_reader";

pub const IS_UNTRUSTED_FORK: &str = "is_untrusted_fork";

pub const READ: &str = "read";

pub const VIEW: &str = "view";

pub const TRIGGER: &str = "trigger";

pub const DEPLOY: &str = "deploy";

pub const ADMINISTER: &str = "administer";

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

pub fn ci_project_fragment() -> FragmentDef {
    frag(
        object_types::CI_PROJECT,
        &["parent_repo", "reader", "admin"],
        vec![
            perm(
                VIEW,
                Userset::Union(vec![
                    rel("reader"),
                    rel("admin"),
                    ttu("parent_repo", "pull"),
                ]),
            ),
            perm(ADMINISTER, rel("admin")),
        ],
    )
}

pub fn environment_fragment() -> FragmentDef {
    frag(
        object_types::ENVIRONMENT,
        &["parent_ci_project", "deployer"],
        vec![perm(
            DEPLOY,
            Userset::Union(vec![rel("deployer"), ttu("parent_ci_project", ADMINISTER)]),
        )],
    )
}

pub fn secret_fragment() -> FragmentDef {
    frag(
        object_types::SECRET,
        &["parent_ci_project", SECRET_DIRECT_READER],
        vec![
            perm(READ, rel(SECRET_DIRECT_READER)),
        ],
    )
}

pub fn run_fragment() -> FragmentDef {
    frag(
        object_types::RUN,
        &["parent_repo", IS_UNTRUSTED_FORK],
        vec![
            perm(VIEW, ttu("parent_repo", "pull")),
            perm(TRIGGER, ttu("parent_repo", "push")),
            perm(
                READ,
                Userset::Exclusion {
                    base: Box::new(ttu("parent_repo", "pull")),
                    subtracted: Box::new(rel(IS_UNTRUSTED_FORK)),
                },
            ),
        ],
    )
}

pub fn ci_fragment() -> Vec<FragmentDef> {
    vec![
        ci_project_fragment(),
        environment_fragment(),
        secret_fragment(),
        run_fragment(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::NamespaceEngine;
    use myelin_identity::FragmentAdmit;

    #[test]
    fn ci_fragment_admits_into_the_cell_schema() {
        let mut eng = NamespaceEngine::with_core_hierarchy();
        for def in ci_fragment() {
            let admit = eng.admit(&def);
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the CI `{}` fragment admits into the cell schema: {admit:?}",
                def.object_type.0
            );
        }
        for ty in ["ci_project", "environment", "secret", "run"] {
            assert!(
                eng.object_types().contains(&ty.to_string()),
                "`{ty}` is admitted"
            );
        }
        assert!(
            eng.resolve_permission("secret", READ).is_some(),
            "secret.read is a compiled permission"
        );
        assert!(
            eng.resolve_permission("run", READ).is_some(),
            "run.read is a compiled permission"
        );
        assert!(
            eng.resolve_permission("run", VIEW).is_some(),
            "run.view is a compiled permission"
        );
    }

    #[test]
    fn secret_read_is_a_bare_direct_relation_not_inherited() {
        let secret = secret_fragment();
        let read = secret
            .permissions
            .iter()
            .find(|p| p.permission.0 == READ)
            .expect("secret declares read");
        assert_eq!(
            read.rewrite,
            rel(SECRET_DIRECT_READER),
            "secret.read = direct_reader (DIRECT NARROW, NOT inherited - CI-1, §1)"
        );
        assert!(
            !matches!(read.rewrite, Userset::TupleToUserset { .. }),
            "secret.read does NOT inherit via tuple-to-userset"
        );
        assert!(
            !rewrite_mentions_tupleset(&read.rewrite, "parent_ci_project"),
            "secret.read never reaches parent_ci_project (no project-read inheritance, CI-1)"
        );
    }

    #[test]
    fn run_read_is_view_minus_is_untrusted_fork() {
        let run = run_fragment();
        let read = run
            .permissions
            .iter()
            .find(|p| p.permission.0 == READ)
            .expect("run declares read");
        match &read.rewrite {
            Userset::Exclusion { base, subtracted } => {
                assert_eq!(
                    **subtracted,
                    rel(IS_UNTRUSTED_FORK),
                    "the ABAC edge subtracts is_untrusted_fork (the !is_untrusted_fork edge, C7)"
                );
                assert_eq!(
                    **base,
                    ttu("parent_repo", "pull"),
                    "the read base is run.view = parent_repo->pull (§5)"
                );
            }
            other => panic!("run.read must be an Exclusion (− is_untrusted_fork), got {other:?}"),
        }
    }

    #[test]
    fn run_view_and_trigger_inherit_the_repo() {
        let run = run_fragment();
        let view = run
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .expect("run declares view");
        assert_eq!(
            view.rewrite,
            ttu("parent_repo", "pull"),
            "run.view = parent_repo->pull (§5)"
        );
        let trigger = run
            .permissions
            .iter()
            .find(|p| p.permission.0 == TRIGGER)
            .expect("run declares trigger");
        assert_eq!(
            trigger.rewrite,
            ttu("parent_repo", "push"),
            "run.trigger = parent_repo->push (§5)"
        );
    }

    #[test]
    fn no_ci_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in ci_fragment() {
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

    fn rewrite_mentions_tupleset(rw: &Userset, tupleset: &str) -> bool {
        match rw {
            Userset::Relation(_) => false,
            Userset::Union(arms) | Userset::Intersect(arms) => {
                arms.iter().any(|a| rewrite_mentions_tupleset(a, tupleset))
            }
            Userset::Exclusion { base, subtracted } => {
                rewrite_mentions_tupleset(base, tupleset)
                    || rewrite_mentions_tupleset(subtracted, tupleset)
            }
            Userset::TupleToUserset { tupleset: t, .. } => t.0 == tupleset,
        }
    }
}
