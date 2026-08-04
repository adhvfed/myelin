use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

pub mod object_types {
    pub const CI_PROJECT: &str = "ci_project";
    pub const ENVIRONMENT: &str = "environment";
    pub const SECRET: &str = "secret";
    pub const RUN: &str = "run";
}

pub const PARENT_REPO: &str = "parent_repo";
pub const READER: &str = "reader";
pub const ADMIN: &str = "admin";
pub const PARENT_CI_PROJECT: &str = "parent_ci_project";
pub const DEPLOYER: &str = "deployer";
pub const APPROVER: &str = "approver";
pub const SECRET_DIRECT_READER: &str = "direct_reader";
pub const IS_UNTRUSTED_FORK: &str = "is_untrusted_fork";
pub const WATCHER: &str = "watcher";

pub const VIEW: &str = "view";
pub const ADMINISTER: &str = "administer";
pub const DEPLOY: &str = "deploy";
pub const APPROVE: &str = "approve";
pub const ROLLBACK: &str = "rollback";
pub const TRIGGER: &str = "trigger";
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

pub fn ci_project_fragment() -> NamespaceFragment {
    fragment(
        object_types::CI_PROJECT,
        &[PARENT_REPO, READER, ADMIN],
        &[VIEW, ADMINISTER],
    )
}

pub fn environment_fragment() -> NamespaceFragment {
    fragment(
        object_types::ENVIRONMENT,
        &[PARENT_CI_PROJECT, DEPLOYER, APPROVER],
        &[DEPLOY, APPROVE, ROLLBACK],
    )
}

pub fn secret_fragment() -> NamespaceFragment {
    fragment(
        object_types::SECRET,
        &[PARENT_CI_PROJECT, SECRET_DIRECT_READER],
        &[READ],
    )
}

pub fn run_fragment() -> NamespaceFragment {
    fragment(
        object_types::RUN,
        &[PARENT_REPO, IS_UNTRUSTED_FORK, WATCHER],
        &[VIEW, TRIGGER, READ],
    )
}

pub fn ci_fragment() -> Vec<NamespaceFragment> {
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

    #[test]
    fn the_four_ci_object_types_are_frozen() {
        let frag = ci_fragment();
        let types: Vec<&str> = frag.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, vec!["ci_project", "environment", "secret", "run"]);
    }

    #[test]
    fn each_definition_declares_its_frozen_relations() {
        let rels = |f: &NamespaceFragment| -> Vec<String> {
            f.relations.iter().map(|r| r.0.clone()).collect()
        };
        assert_eq!(
            rels(&ci_project_fragment()),
            vec![PARENT_REPO, READER, ADMIN]
        );
        assert_eq!(
            rels(&environment_fragment()),
            vec![PARENT_CI_PROJECT, DEPLOYER, APPROVER]
        );
        assert_eq!(
            rels(&secret_fragment()),
            vec![PARENT_CI_PROJECT, SECRET_DIRECT_READER]
        );
        assert_eq!(
            rels(&run_fragment()),
            vec![PARENT_REPO, IS_UNTRUSTED_FORK, WATCHER]
        );
    }

    #[test]
    fn the_is_untrusted_fork_edge_relations_are_declared_on_run() {
        let run = run_fragment();
        assert!(
            run.relations.contains(&RelName(IS_UNTRUSTED_FORK.into())),
            "`is_untrusted_fork` (the SUBTRACTED arm of `run.read`) must be declared (§5.2)"
        );
        assert!(
            run.permissions.contains(&Permission(READ.into())),
            "`run.read` (gated by the !is_untrusted_fork edge) must be declared (§5.2)"
        );
    }

    #[test]
    fn secret_declares_direct_reader_and_only_read() {
        let secret = secret_fragment();
        assert!(
            secret
                .relations
                .contains(&RelName(SECRET_DIRECT_READER.into())),
            "`direct_reader` (the only secret-read path, CI-1) must be declared"
        );
        assert_eq!(
            secret.permissions,
            vec![Permission(READ.into())],
            "secret declares ONLY `read` (the DIRECT NARROW gate) - no project-inherited view perm"
        );
    }

    #[test]
    fn environment_declares_the_approve_list_subjects_target() {
        let env = environment_fragment();
        assert!(
            env.relations.contains(&RelName(APPROVER.into())),
            "`approver` (the HITL list_subjects target) must be declared (§5.2)"
        );
        assert!(
            env.permissions.contains(&Permission(APPROVE.into())),
            "`approve` (the HITL approval permission) must be declared (§5.2 / 4.4)"
        );
    }

    #[test]
    fn watcher_is_declared_on_the_watchable_run_type() {
        assert!(
            run_fragment().relations.contains(&RelName(WATCHER.into())),
            "the `run` watchable type declares `watcher` (Notif read-fanout, §5.2)"
        );
    }

    #[test]
    fn no_ci_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in ci_fragment() {
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
