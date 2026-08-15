use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, NamespaceFragment,
    ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    FragmentDef, NamespaceEngine, PermissionRule, StoreBackedCheck, TupleStore, Userset,
};
use myelin_issues::rebac_fragment::{self, object_types};
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
    if !tuples.is_empty() {
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
    }
    StoreBackedCheck::new(store)
}

fn issues_fragment_defs_rich() -> Vec<FragmentDef> {
    let rel = |n: &str| Userset::Relation(RelName(n.into()));
    let ttu = |tupleset: &str, computed: &str| Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    };
    vec![
        FragmentDef {
            object_type: ObjectType(object_types::ISSUE.into()),
            relations: vec![
                RelName("parent_project".into()),
                RelName("assignee".into()),
                RelName("watcher".into()),
                RelName("confidential".into()),
                RelName("confidential_grant".into()),
            ],
            permissions: vec![
                PermissionRule {
                    permission: Permission("view".into()),
                    rewrite: Userset::Union(vec![
                        Userset::Exclusion {
                            base: Box::new(ttu("parent_project", "view")),
                            subtracted: Box::new(rel("confidential")),
                        },
                        rel("confidential_grant"),
                    ]),
                },
                PermissionRule {
                    permission: Permission("comment".into()),
                    rewrite: Userset::Union(vec![
                        Userset::Exclusion {
                            base: Box::new(ttu("parent_project", "view")),
                            subtracted: Box::new(rel("confidential")),
                        },
                        rel("confidential_grant"),
                    ]),
                },
                PermissionRule {
                    permission: Permission("transition".into()),
                    rewrite: Userset::Union(vec![rel("assignee"), ttu("parent_project", "view")]),
                },
                PermissionRule {
                    permission: Permission("manage".into()),
                    rewrite: ttu("parent_project", "view"),
                },
            ],
        },
        FragmentDef {
            object_type: ObjectType(object_types::ISSUE_FIELD.into()),
            relations: vec![RelName("parent_issue".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view_field".into()),
                rewrite: ttu("parent_issue", "view"),
            }],
        },
        FragmentDef {
            object_type: ObjectType(object_types::ISSUE_TRANSITION.into()),
            relations: vec![RelName("parent_issue".into())],
            permissions: vec![PermissionRule {
                permission: Permission("perform_transition".into()),
                rewrite: ttu("parent_issue", "transition"),
            }],
        },
    ]
}

#[test]
fn cdc_4_9_issues_names_only_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);

    let consumer_fragment: Vec<NamespaceFragment> = rebac_fragment::issues_fragment();
    assert_eq!(
        consumer_fragment.len(),
        3,
        "issue + issue_field + issue_transition"
    );

    for def in issues_fragment_defs_rich() {
        let admit = svc.admit_fragment_def(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Issues `{}` fragment must admit into the cell schema: {admit:?}",
            def.object_type.0
        );
    }
}

#[test]
fn cdc_4_9_confidential_set_difference_resolves_leak_free() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("project:web", "reader", "p:carol"),
            add("project:web", "reader", "p:dave"),
            add("issue:secret", "parent_project", "project:web#view"),
            add("issue:secret", "confidential", "p:dave"),
            add("issue:secret", "confidential_grant", "p:carol"),
        ],
    );

    for def in issues_fragment_defs_rich() {
        assert!(
            matches!(svc.admit_fragment_def(&def), FragmentAdmit::Admitted { .. }),
            "Issues `{}` admits",
            def.object_type.0
        );
    }

    let issue = ArtifactRef("issue:secret".into());
    let can_view = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("view".into()),
                &issue,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };

    assert!(
        can_view(&subject("p:carol")),
        "a project reader WITH a confidential_grant can view the confidential issue (the + arm)"
    );
    assert!(
        !can_view(&subject("p:dave")),
        "a confidential issue is ABSENT from view for an excluded non-grantee (the - confidential \
         set-difference, D3 - NO leak)"
    );
    assert!(
        !can_view(&subject("p:erin")),
        "an outsider (no project read) cannot view (fail-closed)"
    );

    let can_comment = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("comment".into()),
                &issue,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_comment(&subject("p:carol")),
        "the grantee can comment (comment = view)"
    );
    assert!(
        !can_comment(&subject("p:dave")),
        "the excluded non-grantee cannot comment either (comment = view, same exclusion)"
    );
}

#[test]
fn cdc_4_9_fully_confidential_issue_is_invisible_to_all_readers() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("project:web", "reader", "p:carol"),
            add("issue:locked", "parent_project", "project:web#view"),
            add("issue:locked", "confidential", "p:carol"),
        ],
    );
    for def in issues_fragment_defs_rich() {
        let _ = svc.admit_fragment_def(&def);
    }
    let issue = ArtifactRef("issue:locked".into());
    assert!(
        !matches!(
            svc.check(&subject("p:carol"), &Permission("view".into()), &issue, &at_latest(), None),
            Ok(Decision::Allow)
        ),
        "a confidential issue with no grant is invisible even to the project reader who is excluded"
    );
}

#[test]
fn cdc_4_9_transition_resolves_through_assignee_and_inheritance() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("issue:eng-1", "parent_project", "project:web#view"),
            add("issue:eng-1", "assignee", "p:frank"),
            add("project:web", "reader", "p:grace"),
        ],
    );
    for def in issues_fragment_defs_rich() {
        assert!(matches!(
            svc.admit_fragment_def(&def),
            FragmentAdmit::Admitted { .. }
        ));
    }
    let issue = ArtifactRef("issue:eng-1".into());
    let can_transition = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("transition".into()),
                &issue,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_transition(&subject("p:frank")),
        "the assignee can transition"
    );
    assert!(
        can_transition(&subject("p:grace")),
        "a project member inherits transition (parent_project->write maps to core view)"
    );
    assert!(
        !can_transition(&subject("p:heidi")),
        "an outsider cannot transition (fail-closed)"
    );
}

#[test]
fn cdc_4_9_issues_fragment_shape_is_frozen() {
    let issue = rebac_fragment::issue_fragment();
    assert_eq!(issue.object_type, ObjectType("issue".into()));
    for r in [
        "parent_project",
        "assignee",
        "watcher",
        "confidential",
        "confidential_grant",
    ] {
        assert!(
            issue.relations.contains(&RelName(r.into())),
            "issue declares `{r}`"
        );
    }
    assert!(issue.relations.contains(&RelName("watcher".into())));
    assert!(rebac_fragment::issue_field_fragment()
        .relations
        .contains(&RelName("parent_issue".into())));
    assert!(rebac_fragment::issue_transition_fragment()
        .relations
        .contains(&RelName("parent_issue".into())));
}

#[test]
fn cdc_4_9_issues_fragment_is_internally_well_formed() {
    let mut eng = NamespaceEngine::new();
    for def in issues_fragment_defs_rich() {
        let admit = eng.admit(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Issues `{}` is internally well-formed (admits on a bare engine): {admit:?}",
            def.object_type.0
        );
    }
}
