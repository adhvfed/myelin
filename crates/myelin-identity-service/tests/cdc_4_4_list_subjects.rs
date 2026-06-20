//! # The CDC pair for contract 4.4 — `list_subjects(object, permission, zookie?) → SubjectTree` +
//! `explain(...) → RewriteTrace` (P-ID-13 / P-071)
//!
//! **Contract-index row 4.4** (`list_subjects` / `explain` — the Zanzibar Expand served by S8 at
//! 50k-member density, C8). This is the dedicated provider+consumer pair the P-ID-13 TESTS field
//! names — the focused, in-CI evidence that the two sides of the `list_subjects`/`explain` seam
//! cannot drift apart:
//!
//! - the **PROVIDER** ([`StoreBackedCheck::list_subjects_in`] / [`StoreBackedCheck::explain_in`] over
//!   S3 + the live S8 reverse index) returns the **flattened** [`SubjectTree`] `{object, relation,
//!   members, zookie}` (the concrete principal subjects holding the permission, served by S8 at
//!   density) and the [`RewriteTrace`] `{steps}` (why a subject's access resolved);
//! - the **CONSUMER** is an **admin inspector / HITL approver-set** — exactly the two consumers row
//!   4.4 names ("admin inspector, HITL approver set, Notif read-fanout"). The admin inspector renders
//!   the membership of an object's permission (the `SubjectTree.members`); the HITL approver-set
//!   takes the SAME `SubjectTree` to decide who may approve a gated transition (the approver set IS
//!   the subjects who hold the approve permission). It NEVER sees a non-member, and the `explain`
//!   trace it shows ends in `ALLOW`/`DENY` (never empty, never a silent allow).
//!
//! The provider's promise (the flattened set is exactly the subjects who hold the permission, served
//! by S8 — never a superset, never a per-member scan) and the consumer's promise (it renders exactly
//! the expanded set / the trace, never inventing a member) are pinned here so a change to either side
//! fails this test in the same CI job.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, ObjectType, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_identity_service::{
    namespace::{FragmentDef, PermissionRule, Userset},
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore, WATCHER_RELATION,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn admin(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(&admin(tenant), Region("eu-west".into()))
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn grant(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

/// The PROVIDER: the store-backed `list_subjects`/`explain` surface over S3 + a live S8 reverse index
/// (fed off the bus from the seeded grants), with an `issue` fragment admitted carrying an `approve`
/// permission (`approve = approver ∪ lead`) — the HITL approver-set case.
fn provider(scope: &TenantScope, grants: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    store
        .write_tuples(
            scope,
            &admin(&scope.tenant().0),
            grants,
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env);
    }

    let svc = StoreBackedCheck::with_index(store, index);
    // An `issue` fragment: `approve = approver ∪ lead` (the HITL approver-set permission). The issue is
    // also WATCHABLE (P-ID-23, C8) so the Notif read-fanout consumer (`list_subjects(issue, watcher)`)
    // is exercised on the SAME provider — the third consumer row 4.4 names.
    let _ = svc.admit_fragment_def(
        &FragmentDef {
            object_type: ObjectType("issue".into()),
            relations: vec![RelName("approver".into()), RelName("lead".into())],
            permissions: vec![PermissionRule {
                permission: Permission("approve".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("approver".into())),
                    Userset::Relation(RelName("lead".into())),
                ]),
            }],
        }
        .watchable(),
    );
    svc
}

/// The CONSUMER (admin inspector): renders the membership of an object's permission — exactly the
/// `SubjectTree.members`, sorted, never inventing a member. This is the "who can see/do X on this
/// object" panel row 4.4 names.
fn admin_inspector_renders(tree: &SubjectTree) -> Vec<String> {
    let mut out: Vec<String> = tree.members.iter().map(|m| m.0.clone()).collect();
    out.sort();
    out
}

/// The CONSUMER (HITL approver-set): the approver set IS the subjects who hold the `approve`
/// permission — the SAME `SubjectTree`. Returns whether a candidate approver is in the set (the gate
/// the HITL card enforces: only an approver may approve). Leak-free — a non-approver is never in.
fn hitl_approver_set_admits(tree: &SubjectTree, candidate: &str) -> bool {
    tree.members.iter().any(|m| m.0 == candidate)
}

/// The CONSUMER (admin inspector): renders the `explain` trace verbatim for the inspector panel. The
/// trace must be non-empty and end in an ALLOW/DENY verdict (the consumer shows the WHY).
fn inspector_renders_trace(trace: &RewriteTrace) -> Vec<String> {
    trace.steps.clone()
}

/// **The 4.4 Expand: the provider flattens an object's permission membership + the admin inspector
/// renders exactly it.** `issue:PROJ-1`'s `approve = approver ∪ lead`: two approvers + one lead;
/// `list_subjects(issue:PROJ-1, approve)` flattens all three — the inspector renders exactly those.
#[test]
fn cdc_4_4_list_subjects_flattens_membership_inspector_renders_it() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", "approver", "p:alice"),
            grant("issue:PROJ-1", "approver", "p:bob"),
            grant("issue:PROJ-1", "lead", "p:carol"),
            // a different issue's approvers must not leak in.
            grant("issue:PROJ-2", "approver", "p:dave"),
        ],
    );
    let tree = svc.list_subjects_in(
        &s,
        &ObjectId("issue:PROJ-1".into()),
        &Permission("approve".into()),
        &at_latest(),
    );
    let rendered = admin_inspector_renders(&tree);
    assert_eq!(
        rendered,
        vec!["p:alice".to_string(), "p:bob".into(), "p:carol".into()],
        "the inspector renders exactly the approve membership (leak-free — PROJ-2's approver absent)"
    );
}

/// **The 4.4 HITL approver-set: the SAME SubjectTree is the approver set the HITL card gates on.** An
/// approver/lead is admitted; a non-approver is denied — the provider's flattened set IS the gate.
#[test]
fn cdc_4_4_hitl_approver_set_admits_only_members() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", "approver", "p:alice"),
            grant("issue:PROJ-1", "lead", "p:carol"),
        ],
    );
    let tree = svc.list_subjects_in(
        &s,
        &ObjectId("issue:PROJ-1".into()),
        &Permission("approve".into()),
        &at_latest(),
    );
    assert!(
        hitl_approver_set_admits(&tree, "p:alice"),
        "an approver may approve (in the approver set)"
    );
    assert!(
        hitl_approver_set_admits(&tree, "p:carol"),
        "a lead may approve (the ∪ lead arm)"
    );
    assert!(
        !hitl_approver_set_admits(&tree, "p:mallory"),
        "a non-approver may NOT approve (leak-free — never in the set)"
    );
}

/// **The 4.4 explain: the provider returns a non-empty, correct RewriteTrace + the inspector renders
/// it.** explain for an approver ends in ALLOW; for a non-approver ends in DENY — never empty, never a
/// silent allow (the mandatory-core branch).
#[test]
fn cdc_4_4_explain_trace_is_non_empty_and_correct() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("issue:PROJ-1", "approver", "p:alice")]);

    let allow_trace = svc.explain_in(
        &s,
        &PrincipalId("p:alice".into()),
        &Permission("approve".into()),
        &ObjectId("issue:PROJ-1".into()),
        &at_latest(),
    );
    let rendered = inspector_renders_trace(&allow_trace);
    assert!(!rendered.is_empty(), "the inspector renders a non-empty trace");
    assert!(
        rendered.last().unwrap().starts_with("ALLOW"),
        "an approver's trace ends in ALLOW: {rendered:?}"
    );

    let deny_trace = svc.explain_in(
        &s,
        &PrincipalId("p:mallory".into()),
        &Permission("approve".into()),
        &ObjectId("issue:PROJ-1".into()),
        &at_latest(),
    );
    assert!(
        deny_trace.steps.last().unwrap().starts_with("DENY"),
        "a non-approver's trace ends in DENY (never a silent allow): {:?}",
        deny_trace.steps
    );
}

/// **The 4.4 Notif read-fanout consumer (P-ID-23, C8): `list_subjects(object, watcher)` flattens the
/// watcher set the Notif fanout delivers to.** The third consumer row 4.4 names ("Notif read-fanout").
/// `issue:PROJ-1` is watched by alice + bob (not the approver carol who does not watch);
/// `list_watchers_in` returns EXACTLY the watchers — the Notif fanout delivers to them and no one else
/// (a non-watcher never gets the notification, so the humanised-tombstone path has no title to leak).
#[test]
fn cdc_4_4_notif_read_fanout_flattens_the_watcher_set() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", WATCHER_RELATION, "p:alice"),
            grant("issue:PROJ-1", WATCHER_RELATION, "p:bob"),
            // carol is an approver but does NOT watch — she must not be in the fanout set.
            grant("issue:PROJ-1", "approver", "p:carol"),
            // a different issue's watcher must not leak into PROJ-1's fanout.
            grant("issue:PROJ-2", WATCHER_RELATION, "p:dave"),
        ],
    );
    let tree = svc.list_watchers_in(&s, &ObjectId("issue:PROJ-1".into()), &at_latest());
    assert_eq!(
        tree.relation,
        RelName(WATCHER_RELATION.into()),
        "the fanout expands the watcher relation"
    );
    let watchers = admin_inspector_renders(&tree);
    assert_eq!(
        watchers,
        vec!["p:alice".to_string(), "p:bob".into()],
        "the Notif fanout delivers to exactly the watchers (carol does not watch; PROJ-2's dave absent)"
    );
}

/// **The 4.4 seam is cross-tenant-safe (ID-D3).** A `list_subjects` under `globex` sees NONE of
/// `acme`'s issue approvers — the provider reads only the verified scope's S8 partition, so the
/// consumer can never render a cross-tenant member.
#[test]
fn cdc_4_4_no_cross_tenant_membership() {
    let acme = scope("acme");
    let svc = provider(&acme, &[grant("issue:PROJ-1", "approver", "p:alice")]);
    let globex = scope("globex");
    let tree = svc.list_subjects_in(
        &globex,
        &ObjectId("issue:PROJ-1".into()),
        &Permission("approve".into()),
        &at_latest(),
    );
    assert!(
        admin_inspector_renders(&tree).is_empty(),
        "0 cross-tenant approvers — a globex inspector sees none of acme's membership"
    );
}
