use myelin_events::{OutboxStore, Timestamp};
use myelin_git::rebac_fragment::{self, object_types};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, NamespaceFragment,
    ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    FragmentDef, NamespaceEngine, PermissionRule, StoreBackedCheck, TupleStore, Userset,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed tuples");
    StoreBackedCheck::new(store)
}

fn git_fragment_defs_rich() -> Vec<FragmentDef> {
    let rel = |n: &str| Userset::Relation(RelName(n.into()));
    let ttu = |tupleset: &str, computed: &str| Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    };
    vec![
        FragmentDef {
            object_type: ObjectType(object_types::REPO.into()),
            relations: vec![
                RelName("parent_project".into()),
                RelName("reader".into()),
                RelName("writer".into()),
                RelName("admin".into()),
                RelName("approve_untrusted_ci".into()),
                RelName("watcher".into()),
            ],
            permissions: vec![
                PermissionRule {
                    permission: Permission("pull".into()),
                    rewrite: Userset::Union(vec![
                        rel("reader"),
                        rel("writer"),
                        rel("admin"),
                        ttu("parent_project", "view"),
                    ]),
                },
                PermissionRule {
                    permission: Permission("push".into()),
                    rewrite: Userset::Union(vec![
                        rel("writer"),
                        rel("admin"),
                        ttu("parent_project", "view"),
                    ]),
                },
                PermissionRule {
                    permission: Permission("administer".into()),
                    rewrite: Userset::Union(vec![rel("admin"), ttu("parent_project", "view")]),
                },
                PermissionRule {
                    permission: Permission("protected_push".into()),
                    rewrite: rel("admin"),
                },
            ],
        },
        FragmentDef {
            object_type: ObjectType(object_types::REF.into()),
            relations: vec![
                RelName("parent_repo".into()),
                RelName("bypass".into()),
                RelName("code_owner".into()),
            ],
            permissions: vec![PermissionRule {
                permission: Permission("push_protected".into()),
                rewrite: Userset::Union(vec![rel("bypass"), ttu("parent_repo", "administer")]),
            }],
        },
        FragmentDef {
            object_type: ObjectType(object_types::PULL_REQUEST.into()),
            relations: vec![
                RelName("parent_repo".into()),
                RelName("author".into()),
                RelName("reviewer".into()),
                RelName("watcher".into()),
            ],
            permissions: vec![
                PermissionRule {
                    permission: Permission("view".into()),
                    rewrite: ttu("parent_repo", "pull"),
                },
                PermissionRule {
                    permission: Permission("review".into()),
                    rewrite: Userset::Union(vec![rel("reviewer"), ttu("parent_repo", "push")]),
                },
                PermissionRule {
                    permission: Permission("merge".into()),
                    rewrite: ttu("parent_repo", "protected_push"),
                },
            ],
        },
        FragmentDef {
            object_type: ObjectType(object_types::PR_COMMENT.into()),
            relations: vec![RelName("parent_pr".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: ttu("parent_pr", "view"),
            }],
        },
    ]
}

#[test]
fn cdc_4_9_git_names_only_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);

    let consumer_fragment: Vec<NamespaceFragment> = rebac_fragment::git_fragment();
    assert_eq!(
        consumer_fragment.len(),
        4,
        "repo + ref + pull_request + pr_comment"
    );

    for def in git_fragment_defs_rich() {
        let admit = svc.admit_fragment_def(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Git `{}` fragment must admit into the cell schema: {admit:?}",
            def.object_type.0
        );
    }
}

#[test]
fn cdc_4_9_git_rewrites_admit_and_resolve() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("repo:core", "admin", "p:alice"),
            add("repo:core", "parent_project", "project:web#view"),
            add("project:web", "reader", "p:carol"),
        ],
    );

    for def in git_fragment_defs_rich() {
        assert!(
            matches!(svc.admit_fragment_def(&def), FragmentAdmit::Admitted { .. }),
            "Git `{}` admits",
            def.object_type.0
        );
    }

    let repo = ArtifactRef("repo:core".into());
    let can_pull = |actor: &Principal| {
        matches!(
            svc.check(actor, &Permission("pull".into()), &repo, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_pull(&subject("p:alice")),
        "a direct repo admin can pull"
    );
    assert!(
        can_pull(&subject("p:carol")),
        "a project member inherits repo pull (parent_project->view)"
    );
    assert!(
        !can_pull(&subject("p:bob")),
        "an outsider cannot pull (fail-closed)"
    );

    let can_protected_push = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("protected_push".into()),
                &repo,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_protected_push(&subject("p:alice")),
        "admin → protected_push"
    );
    assert!(
        !can_protected_push(&subject("p:carol")),
        "a mere project reader does NOT get protected_push (it is admin-only, §5.2)"
    );
}

#[test]
fn cdc_4_9_approve_untrusted_ci_is_a_plain_relation_check() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[add("repo:core", "approve_untrusted_ci", "p:maintainer")],
    );
    for def in git_fragment_defs_rich() {
        let _ = svc.admit_fragment_def(&def);
    }
    let repo = ArtifactRef("repo:core".into());
    let endorse = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("approve_untrusted_ci".into()),
                &repo,
                &at_latest(),
                None,
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        endorse(&subject("p:maintainer")),
        "a maintainer endorses an untrusted fork run"
    );
    assert!(
        !endorse(&subject("p:bob")),
        "an outsider cannot endorse (X-1, fail-closed)"
    );
}

#[test]
fn cdc_4_9_git_fragment_shape_is_frozen() {
    let repo = rebac_fragment::repo_fragment();
    assert_eq!(repo.object_type, ObjectType("repo".into()));
    for r in [
        "parent_project",
        "reader",
        "writer",
        "admin",
        "approve_untrusted_ci",
        "watcher",
    ] {
        assert!(
            repo.relations.contains(&RelName(r.into())),
            "repo declares `{r}`"
        );
    }
    assert!(rebac_fragment::ref_fragment()
        .relations
        .contains(&RelName("code_owner".into())));
    assert!(rebac_fragment::pull_request_fragment()
        .relations
        .contains(&RelName("watcher".into())));
}

#[test]
fn cdc_4_9_git_fragment_is_internally_well_formed() {
    let mut eng = NamespaceEngine::new();
    for def in git_fragment_defs_rich() {
        let admit = eng.admit(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Git `{}` is internally well-formed (admits on a bare engine): {admit:?}",
            def.object_type.0
        );
    }
}
