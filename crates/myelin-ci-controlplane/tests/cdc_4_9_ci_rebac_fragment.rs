use myelin_ci_controlplane::rebac_fragment::{
    self, ci_fragment, ci_project_fragment, environment_fragment, object_types, run_fragment,
    secret_fragment, ADMIN, ADMINISTER, APPROVE, APPROVER, DEPLOY, DEPLOYER, IS_UNTRUSTED_FORK,
    PARENT_CI_PROJECT, PARENT_REPO, READ, READER, ROLLBACK, SECRET_DIRECT_READER, TRIGGER, VIEW,
    WATCHER,
};
use myelin_identity::{NamespaceFragment, Permission, RelName};

fn rels(f: &NamespaceFragment) -> Vec<String> {
    f.relations.iter().map(|r| r.0.clone()).collect()
}
fn perms(f: &NamespaceFragment) -> Vec<String> {
    f.permissions.iter().map(|p| p.0.clone()).collect()
}

#[test]
fn cdc_4_9_ci_declares_the_four_object_types() {
    let frags = ci_fragment();
    let types: Vec<String> = frags.iter().map(|f| f.object_type.0.clone()).collect();
    assert_eq!(
        types,
        ["ci_project", "environment", "secret", "run"],
        "the four frozen CI object types, root-first (§5.2 / contract-index 4.9)"
    );
    assert_eq!(object_types::CI_PROJECT, "ci_project");
    assert_eq!(object_types::ENVIRONMENT, "environment");
    assert_eq!(object_types::SECRET, "secret");
    assert_eq!(object_types::RUN, "run");
}

#[test]
fn cdc_4_9_relation_and_permission_names_match_the_frozen_vocabulary() {
    assert_eq!(rels(&ci_project_fragment()), [PARENT_REPO, READER, ADMIN]);
    assert_eq!(perms(&ci_project_fragment()), [VIEW, ADMINISTER]);
    assert_eq!(
        rels(&environment_fragment()),
        [PARENT_CI_PROJECT, DEPLOYER, APPROVER]
    );
    assert_eq!(perms(&environment_fragment()), [DEPLOY, APPROVE, ROLLBACK]);
    assert_eq!(
        rels(&secret_fragment()),
        [PARENT_CI_PROJECT, SECRET_DIRECT_READER]
    );
    assert_eq!(perms(&secret_fragment()), [READ]);
    assert_eq!(
        rels(&run_fragment()),
        [PARENT_REPO, IS_UNTRUSTED_FORK, WATCHER]
    );
    assert_eq!(perms(&run_fragment()), [VIEW, TRIGGER, READ]);
}

#[test]
fn cdc_4_9_the_read_and_not_is_untrusted_fork_edge_classifies_a_fork_as_non_reader() {
    let run = run_fragment();
    assert!(
        run.relations.contains(&RelName(IS_UNTRUSTED_FORK.into())),
        "the `is_untrusted_fork` SUBTRACTED relation is declared on `run` (the fork-tier stamp, §5.2)"
    );
    assert!(
        run.permissions.contains(&Permission(READ.into())),
        "`run.read` (gated by − is_untrusted_fork) is declared on `run`"
    );
    assert_eq!(
        IS_UNTRUSTED_FORK, "is_untrusted_fork",
        "the FROZEN edge name (the `!is_untrusted_fork` exclusion driver) - drift here is a compile break"
    );
}

#[test]
fn cdc_4_9_secret_read_is_direct_narrow_not_inherited() {
    let secret = secret_fragment();
    assert!(
        secret
            .relations
            .contains(&RelName(SECRET_DIRECT_READER.into())),
        "`direct_reader` (the only secret-read path, CI-1) is declared"
    );
    assert_eq!(
        perms(&secret),
        [READ],
        "secret declares ONLY `read` - no project-inherited view permission (CI-1 non-inheritance)"
    );
    assert!(
        secret
            .relations
            .contains(&RelName(PARENT_CI_PROJECT.into())),
        "`parent_ci_project` is a lifecycle relation (NOT a read path)"
    );
}

#[test]
fn cdc_4_9_environment_declares_the_approve_list_subjects_target() {
    let env = environment_fragment();
    assert!(
        env.relations.contains(&RelName(APPROVER.into())),
        "`approver` (the HITL list_subjects target) is declared (§5.2)"
    );
    assert!(
        env.permissions.contains(&Permission(APPROVE.into())),
        "`approve` (the HITL approval permission) is declared (§5.2 / contract 4.4)"
    );
}

#[test]
fn cdc_4_9_watcher_is_on_the_watchable_run_type() {
    assert!(
        run_fragment().relations.contains(&RelName(WATCHER.into())),
        "the `run` watchable type declares `watcher` (Notif read-fanout, §5.2)"
    );
}

#[test]
fn cdc_4_9_no_ci_name_smuggles_an_object_id() {
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
    assert_eq!(rebac_fragment::ci_fragment().len(), 4);
}
