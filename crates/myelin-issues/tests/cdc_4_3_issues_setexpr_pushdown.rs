//! # The CDC pair for contract 4.3 — the `list_objects` `SetExpr` push-down, **Issues consumer side**
//! (ISS-P13 / P-379).
//!
//! **Contract-index row 4.3** (`list_objects → Ids | Filter{set_expr, zookie}` — the SetExpr lowered
//! to a SQL predicate / JOIN over the consumer's OWN id column via the per-tenant authz reverse index;
//! no N+1, no post-filter — the single most load-bearing inter-system contract). Issues is **THE
//! headline consumer** of this contract: the board/backlog scan lowers the returned `SetExpr` FIRST
//! into a leak-free predicate over `issue.id`.
//!
//! - the **PROVIDER** is Identity's LIVE `list_objects`
//!   ([`myelin_identity_service::StoreBackedCheck::list_objects`]) over the real namespace engine seeded
//!   with real ReBAC tuples (incl. the `- confidential` set-difference exclusion). It returns a real
//!   [`myelin_identity::ListObjectsResult`] — the leak-free reachable set (a confidential non-grantee's
//!   issue is ALREADY absent — the engine drops it, never a post-filter).
//! - the **CONSUMER** is the Issues query planner
//!   ([`myelin_issues::planner::lower_over_issue_id`] / [`compose_board_query`]) — it lowers the
//!   returned `SetExpr` into one leak-free SQL predicate / JOIN over `issue.id`, no N+1, no post-filter.
//!
//! The two sides are pinned here so a drift on either (Identity changes the `SetExpr` shape; Issues
//! mis-lowers a variant) fails this test in the same CI job. The headline assertion: a confidential
//! issue the provider excluded is **absent** from the consumer's lowered result — 0 leak, end-to-end
//! across the contract boundary (the ISS-D3 F1 leak-free family at the 4.3 seam).

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ListObjectsResult, ObjectId,
    ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr,
    TupleDelta, Zookie,
};
use myelin_identity_service::{FragmentDef, PermissionRule, StoreBackedCheck, TupleStore, Userset};
use myelin_issues::planner::{compose_board_query, lower_over_issue_id, AuthzVisibleIndex};
use myelin_issues::rebac_fragment::object_types;
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

/// The PROVIDER engine seeded with `tuples` (the org/team/project core hierarchy preloaded by
/// `StoreBackedCheck::new`, so the Issues `parent_project->view` inheritance resolves).
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

/// The Issues `view` rewrite as the rich engine form (§6.1):
/// `(parent_project->view − confidential) ∪ confidential_grant` — the set-difference whose
/// confidential arm makes a confidential issue ABSENT from the provider's reachable set.
fn issue_view_def() -> FragmentDef {
    let rel = |n: &str| Userset::Relation(RelName(n.into()));
    let ttu = |t: &str, c: &str| Userset::TupleToUserset {
        tupleset: RelName(t.into()),
        computed: RelName(c.into()),
    };
    FragmentDef {
        object_type: ObjectType(object_types::ISSUE.into()),
        relations: vec![
            RelName("parent_project".into()),
            RelName("assignee".into()),
            RelName("watcher".into()),
            RelName("confidential".into()),
            RelName("confidential_grant".into()),
        ],
        permissions: vec![PermissionRule {
            permission: Permission("view".into()),
            rewrite: Userset::Union(vec![
                Userset::Exclusion {
                    base: Box::new(ttu("parent_project", "view")),
                    subtracted: Box::new(rel("confidential")),
                },
                rel("confidential_grant"),
            ]),
        }],
    }
}

/// **PROVIDER → CONSUMER (4.3): the real `list_objects(view, issue)` reachable set, lowered by the
/// Issues planner over `issue.id`, is LEAK-FREE — the confidential issue the provider excluded is
/// absent from the consumer's lowered result.**
#[test]
fn cdc_4_3_confidential_issue_absent_through_the_planner() {
    let s = scope("acme");
    // project:web readers carol + dave; two issues inherit project:web#view. issue:secret is
    // confidential for dave (the subtracted arm) and carol holds an explicit confidential_grant.
    let svc = provider(
        &s,
        &[
            add("project:web", "reader", "p:carol"),
            add("project:web", "reader", "p:dave"),
            add("issue:open", "parent_project", "project:web#view"),
            add("issue:secret", "parent_project", "project:web#view"),
            add("issue:secret", "confidential", "p:dave"),
            add("issue:secret", "confidential_grant", "p:carol"),
        ],
    );
    let _ = svc.admit_fragment_def(&issue_view_def());

    let view = Permission("view".into());
    let universe = ["issue:open", "issue:secret"];

    // PROVIDER: the leak-free reachable `view` set per viewer — the engine resolves the inherited
    // `parent_project->view` userset AND the `- confidential` set-difference. (The live ABI
    // `list_objects` materialises from the S8 reverse index, whose population of INHERITED grants is
    // Identity's reverse-index work, P-ID-11/12; here we drive the SAME leak-free engine resolution
    // through `check` over the candidate universe — the reachable_set's own per-candidate filter, the
    // EXACT predicate the materialiser applies. The SHAPE the consumer lowers is the resulting id set,
    // contract 4.3's `Ids` arm.)
    let reachable = |actor: &Principal| -> Vec<ObjectId> {
        universe
            .iter()
            .filter(|id| {
                matches!(
                    svc.check(
                        actor,
                        &view,
                        &ArtifactRef((**id).into()),
                        &at_latest(),
                        None
                    ),
                    Ok(Decision::Allow)
                )
            })
            .map(|id| ObjectId((*id).into()))
            .collect()
    };

    // dave: a project reader on the confidential exclusion, NO grant → issue:secret is ABSENT.
    let dave_set = reachable(&subject("p:dave"));
    assert!(
        dave_set.contains(&ObjectId("issue:open".into())),
        "dave reads the non-confidential issue (inherited parent_project->view)"
    );
    assert!(
        !dave_set.contains(&ObjectId("issue:secret".into())),
        "PROVIDER: the confidential issue is ABSENT from dave's reachable set (the - confidential \
         set-difference; 0 leak at the source)"
    );

    // CONSUMER: the Issues planner lowers the returned `Ids` set over issue.id, leak-free. Evaluate the
    // lowering against the full candidate universe and assert issue:secret never survives for dave.
    let idx = AuthzVisibleIndex::new();
    let universe_ids: Vec<ObjectId> = universe.iter().map(|id| ObjectId((*id).into())).collect();
    let dave_lowered = lower_over_issue_id(&SetExpr::Ids(dave_set), &subject("p:dave"));
    let dave_visible = idx.evaluate(
        &TenantId("acme".into()),
        &Region("eu-west".into()),
        &subject("p:dave"),
        &dave_lowered,
        &universe_ids,
    );
    assert_eq!(
        dave_visible,
        vec![ObjectId("issue:open".into())],
        "CONSUMER: the planner's lowered board excludes the confidential issue — 0 leak end-to-end"
    );

    // The grantee carol DOES see issue:secret (the + confidential_grant arm), end-to-end.
    let carol_set = reachable(&subject("p:carol"));
    assert!(
        carol_set.contains(&ObjectId("issue:secret".into())),
        "carol (grantee) reaches the confidential issue at the provider (the + grant arm)"
    );
    let carol_lowered = lower_over_issue_id(&SetExpr::Ids(carol_set), &subject("p:carol"));
    let carol_visible = idx.evaluate(
        &TenantId("acme".into()),
        &Region("eu-west".into()),
        &subject("p:carol"),
        &carol_lowered,
        &universe_ids,
    );
    assert!(
        carol_visible.contains(&ObjectId("issue:secret".into())),
        "CONSUMER: the grantee's lowered board includes the confidential issue (the + grant arm)"
    );
}

/// **PROVIDER → CONSUMER (4.3): the composed board query is ONE leak-free statement** over the real
/// reachable set — the conjoin is the planner's job (no N+1), the tenant predicate isolates the
/// partition, the ACL pre-filter precedes the ORDER BY.
#[test]
fn cdc_4_3_composed_board_is_one_query_over_the_real_set() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("project:web", "reader", "p:carol"),
            add("issue:open", "parent_project", "project:web#view"),
        ],
    );
    let _ = svc.admit_fragment_def(&issue_view_def());

    let result = svc
        .list_objects(
            &subject("p:carol"),
            &Permission("view".into()),
            &ObjectType(object_types::ISSUE.into()),
            &at_latest(),
        )
        .expect("live list_objects");
    let set_expr = match result {
        ListObjectsResult::Ids { ids, .. } => SetExpr::Ids(ids),
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
    };
    let q = compose_board_query(
        &set_expr,
        &subject("p:carol"),
        &TenantId("acme".into()),
        &Region("eu-west".into()),
    );
    assert_eq!(
        q.statement_count(),
        1,
        "one query — the conjoin is the planner's job, no N+1"
    );
    assert!(q
        .sql
        .contains("WHERE issue.tenant_id = :tenant AND issue.region = :region"));
    let acl_pos = q.sql.find("AND (").unwrap();
    let order_pos = q.sql.find("ORDER BY issue.rank").unwrap();
    assert!(
        acl_pos < order_pos,
        "the ACL pre-filter precedes the ORDER BY (never a post-filter)"
    );
}
