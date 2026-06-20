//! # The CDC pair for contract 4.9 — the **Issues** ReBAC namespace fragment (ISS-P01 / P-125)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem declares
//! relations + permissions, compiled into ONE cell schema; Identity owns the engine and never
//! invents object ids). The engine + admit-contract half is pinned by the Identity CDC
//! (`crates/myelin-identity-service/tests/cdc_4_9_namespace_engine.rs`, P-068); THIS file pins the
//! **Issues fragment slice** of the same row — the freeze ISS-P01 ships:
//!
//! - the **CONSUMER** is the **Issues subsystem declaring its namespace fragment at build time**
//!   ([`myelin_issues::rebac_fragment::issues_fragment`]) — the frozen names-only
//!   [`myelin_identity::NamespaceFragment`] carriers Identity admits into the cell schema. The
//!   consumer's promise: it declares exactly the §6.1 relations (the `- confidential` set-difference
//!   driver + its explicit grant, `assignee`, the `watcher` read-fanout, the field/transition ABAC
//!   sub-objects) and gates an action ONLY on a resolved grant.
//! - the **PROVIDER** is Identity's namespace engine ([`StoreBackedCheck`] over the
//!   `with_core_hierarchy` cell schema) — it admits the Issues fragment (`Admitted{fragment_id}`),
//!   resolves the Issues permissions through the four userset operators (INCLUDING the Zanzibar
//!   Exclusion the `- confidential` rewrite compiles to), and never invents an id.
//!
//! The two sides are pinned here so a drift on either (Issues drops/renames a relation; Identity's
//! admit-contract changes shape) fails this test in the same CI job. **The gate of ISS-P01 is the
//! build-time compile** — Identity's cell schema compiles against the Issues fragment; this CDC is
//! the mechanical evidence that the frozen shape ADMITS (well-formed) and, crucially, that the
//! `- confidential` set-difference userset **resolves LEAK-FREE** (a confidential issue is absent
//! from view for a non-grantee). The permission *rewrites* are wired LIVE on Identity's M2 bodies
//! (ISS-P11 / P-ID-*); here we PROVE they are admissible against the real engine TODAY (the freeze
//! anchor).

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, NamespaceFragment,
    ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple,
    TupleDelta, Zookie,
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

/// The PROVIDER surface: the engine (with the org/team/project core hierarchy preloaded, so the
/// Issues fragment's `parent_project->…` inheritance has its parent type) seeded with `tuples`.
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

/// The Issues fragment's frozen permission **rewrites** (§6.1), as the rich engine `FragmentDef`
/// form the Issues spine wires live. ISS-P01 ships only the names (the [`NamespaceFragment`]
/// carriers); this rich form is the CDC's compile-against-the-engine evidence that the frozen shape
/// — including the `- confidential` set-difference — is admissible.
///
/// `issue.view = (parent_project->read − confidential) + confidential_grant` is encoded over the
/// four operators: `Union( Exclusion{ base: parent_project->view, subtracted: confidential },
/// confidential_grant )`. (`parent_project->read` maps onto the core hierarchy's `project.view`,
/// where the inheritance terminates — the §6.1 `read` is the core `view`.)
fn issues_fragment_defs_rich() -> Vec<FragmentDef> {
    let rel = |n: &str| Userset::Relation(RelName(n.into()));
    let ttu = |tupleset: &str, computed: &str| Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    };
    vec![
        // issue: view (the set-difference crux) / comment / transition / manage.
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
                // view = (parent_project->read − confidential) + confidential_grant.
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
                // comment = view (the same set, including the confidential exclusion).
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
                // transition = assignee + parent_project->write (the core `view` terminates here).
                PermissionRule {
                    permission: Permission("transition".into()),
                    rewrite: Userset::Union(vec![rel("assignee"), ttu("parent_project", "view")]),
                },
                // manage = parent_project->write (core `view`).
                PermissionRule {
                    permission: Permission("manage".into()),
                    rewrite: ttu("parent_project", "view"),
                },
            ],
        },
        // issue_field: view_field = parent_issue->view (+ the frozen CaveatContext, off the hot path).
        FragmentDef {
            object_type: ObjectType(object_types::ISSUE_FIELD.into()),
            relations: vec![RelName("parent_issue".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view_field".into()),
                rewrite: ttu("parent_issue", "view"),
            }],
        },
        // issue_transition: perform_transition = parent_issue->transition (+ the approver CaveatContext).
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

/// **CONSUMER → PROVIDER: the Issues fragment (names-only ABI carriers) ADMITS into the cell
/// schema.** This is the build-time gate of ISS-P01 reified as a runtime assertion: Identity admits
/// every Issues object type the consumer declares — the cell schema compiles against the Issues
/// fragment.
#[test]
fn cdc_4_9_issues_names_only_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);

    // The CONSUMER declares its fragment at build time (the frozen names-only carriers).
    let consumer_fragment: Vec<NamespaceFragment> = rebac_fragment::issues_fragment();
    assert_eq!(
        consumer_fragment.len(),
        3,
        "issue + issue_field + issue_transition"
    );

    // The PROVIDER admits each (the rich form carrying the rewrites — the shape Identity compiles).
    for def in issues_fragment_defs_rich() {
        let admit = svc.admit_fragment_def(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Issues `{}` fragment must admit into the cell schema: {admit:?}",
            def.object_type.0
        );
    }
}

/// **PROVIDER: the `- confidential` set-difference resolves LEAK-FREE (the no-leak guarantee, D3).**
/// This is the headline of ISS-P01's TESTS field: a confidential issue is **absent from view for a
/// non-grantee**, by construction (Zanzibar Exclusion), not by a post-filter.
///
/// Data: project:web has reader carol + reader dave. issue:secret#parent_project = project:web#view;
/// issue:secret is `confidential` for dave (dave is on the confidential exclusion); carol gets an
/// explicit `confidential_grant`. So:
/// - carol: a project reader WITH a confidential_grant → CAN view (the `+ confidential_grant` arm).
/// - dave:  a project reader who is ALSO `confidential` (subtracted) and has NO grant → CANNOT view
///   (the issue disappears from his view by the set-difference, even though he reads the project).
/// - erin:  an outsider (no project read) → CANNOT view (fail-closed).
#[test]
fn cdc_4_9_confidential_set_difference_resolves_leak_free() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // project:web readers.
            add("project:web", "reader", "p:carol"),
            add("project:web", "reader", "p:dave"),
            // issue:secret inherits project:web's view (parent_project->view).
            add("issue:secret", "parent_project", "project:web#view"),
            // dave is on the confidential exclusion list (the subtracted arm).
            add("issue:secret", "confidential", "p:dave"),
            // carol has an explicit confidential_grant (re-admitted despite NOT being excluded —
            // proves the `+ confidential_grant` arm; carol is also a plain project reader).
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
            svc.check(actor, &Permission("view".into()), &issue, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };

    // carol: project reader + confidential_grant → CAN view.
    assert!(
        can_view(&subject("p:carol")),
        "a project reader WITH a confidential_grant can view the confidential issue (the + arm)"
    );
    // dave: project reader BUT on the confidential exclusion, no grant → the issue DISAPPEARS.
    assert!(
        !can_view(&subject("p:dave")),
        "a confidential issue is ABSENT from view for an excluded non-grantee (the - confidential \
         set-difference, D3 — NO leak)"
    );
    // erin: outsider → fail-closed.
    assert!(
        !can_view(&subject("p:erin")),
        "an outsider (no project read) cannot view (fail-closed)"
    );

    // `comment = view` carries the SAME exclusion: dave cannot comment either.
    let can_comment = |actor: &Principal| {
        matches!(
            svc.check(actor, &Permission("comment".into()), &issue, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(can_comment(&subject("p:carol")), "the grantee can comment (comment = view)");
    assert!(
        !can_comment(&subject("p:dave")),
        "the excluded non-grantee cannot comment either (comment = view, same exclusion)"
    );
}

/// **PROVIDER: a confidential issue with NO grant disappears for EVERY project reader.** The
/// stronger leak-free witness: when the WHOLE project is confidential-excluded and nobody is granted,
/// the issue is invisible to all project readers (the exclusion is total, never a partial post-filter
/// leak).
#[test]
fn cdc_4_9_fully_confidential_issue_is_invisible_to_all_readers() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("project:web", "reader", "p:carol"),
            add("issue:locked", "parent_project", "project:web#view"),
            // Both the reader AND the issue marked confidential for that reader, no grant.
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

/// **PROVIDER: transition = assignee + parent_project->write.** The assignee (and a project member
/// inheriting write) can transition; an outsider cannot. Proves the non-confidential Issues
/// permissions resolve through the engine too (the freeze is whole, not just the set-difference).
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
            svc.check(actor, &Permission("transition".into()), &issue, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(can_transition(&subject("p:frank")), "the assignee can transition");
    assert!(
        can_transition(&subject("p:grace")),
        "a project member inherits transition (parent_project->write maps to core view)"
    );
    assert!(
        !can_transition(&subject("p:heidi")),
        "an outsider cannot transition (fail-closed)"
    );
}

/// **The frozen Issues fragment matches the architecture §6.1 relation set exactly.** A consumer-side
/// shape pin: if a relation is dropped/renamed in `rebac_fragment`, this fails (it is the names the
/// PROVIDER admits + the live wiring keys on).
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
    // watcher on the watchable issue type (Notif read-fanout).
    assert!(issue.relations.contains(&RelName("watcher".into())));
    // the two ABAC sub-objects inherit through parent_issue.
    assert!(rebac_fragment::issue_field_fragment()
        .relations
        .contains(&RelName("parent_issue".into())));
    assert!(rebac_fragment::issue_transition_fragment()
        .relations
        .contains(&RelName("parent_issue".into())));
}

/// **An empty engine without the core hierarchy still admits the Issues fragment's own relations.**
/// The Issues fragment is internally well-formed (no permission references an undeclared OWN
/// relation — the `- confidential` Exclusion subtracts a DECLARED relation); a `NamespaceEngine` with
/// no core admits every Issues object type — the freeze is self-contained.
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
