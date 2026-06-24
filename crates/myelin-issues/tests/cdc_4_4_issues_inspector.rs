//! # The CDC pair for contract 4.4 (CONSUMED by Issues) — the S15 permission inspector reads
//! `list_subjects`/`explain` (0 private recompute; the inspector's answer EQUALS Identity's `explain`).
//!
//! **ISS-P29 / P-396 — the governance admin views (S13–S18).** Contract-index row **4.4**
//! (`list_subjects(object, permission, zookie?) → SubjectTree` + `explain(...) → RewriteTrace`) is
//! **CONSUMED** here by the S15 **permission inspector** (`crate::governance::PermissionInspector`). This
//! is the focused, in-CI evidence that the inspector seam cannot drift from Identity's Expand:
//!
//! - the **PROVIDER** is the REAL Identity Expand engine ([`StoreBackedCheck::list_subjects_in`] /
//!   [`StoreBackedCheck::explain_in`] over S3 + the live S8 reverse index) — the SAME engine the
//!   identity-service's own `cdc_4_4_list_subjects.rs` proves;
//! - the **CONSUMER** is the Issues S15 permission inspector. It reads the Expand THROUGH the
//!   [`PermissionResolver`] port (the consumer seam) and renders **EXACTLY** the provider's
//!   `SubjectTree`/`RewriteTrace` — **0 private recompute**. There is NO second ReBAC evaluator in Issues
//!   (EI-01 §7).
//!
//! **The inspector-equals-explain gate (the DoD green artifact):** the inspector's `who_can` membership
//! EQUALS the provider's `SubjectTree.members` byte-for-byte, and the inspector's `why` trace EQUALS the
//! provider's `RewriteTrace` byte-for-byte. A change to either side fails this test in the same CI job.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, ObjectType, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_identity_service::{
    namespace::{FragmentDef, PermissionRule, Userset},
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
};
use myelin_issues::governance::{PermissionInspector, PermissionResolver};
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

/// The PROVIDER: the REAL store-backed `list_subjects`/`explain` over S3 + a live S8 reverse index (fed
/// off the bus from the seeded grants), with an `issue` fragment admitted carrying `approve = approver ∪
/// lead` — the governance-inspector case ("who can approve this gated transition, and why").
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
            Timestamp("2026-06-24T00:00:00Z".into()),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env);
    }

    let svc = StoreBackedCheck::with_index(store, index);
    let _ = svc.admit_fragment_def(&FragmentDef {
        object_type: ObjectType("issue".into()),
        relations: vec![RelName("approver".into()), RelName("lead".into())],
        permissions: vec![PermissionRule {
            permission: Permission("approve".into()),
            rewrite: Userset::Union(vec![
                Userset::Relation(RelName("approver".into())),
                Userset::Relation(RelName("lead".into())),
            ]),
        }],
    });
    svc
}

/// **The consumer seam (the inspector's port over the REAL Identity Expand).** Adapts the
/// `StoreBackedCheck` engine to the [`PermissionResolver`] port the Issues inspector reads through —
/// carrying the verified `(tenant, region)` scope the ABI method cannot (a tenant-less expand is a leak).
/// This is the PRODUCTION shape (the inspector reads the gateway-fronted Identity RPC); here it is wired to
/// the REAL engine to prove the inspector cannot drift from `explain`.
struct IdentityExpandResolver {
    svc: StoreBackedCheck,
    scope: TenantScope,
}

impl PermissionResolver for IdentityExpandResolver {
    fn list_subjects(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> SubjectTree {
        self.svc
            .list_subjects_in(&self.scope, object, permission, at)
    }

    fn explain(
        &self,
        subject: &PrincipalId,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> RewriteTrace {
        self.svc
            .explain_in(&self.scope, subject, permission, object, at)
    }
}

/// **The inspector's `who_can` membership EQUALS Identity's `list_subjects` (0 private recompute).**
/// `issue:PROJ-1`'s `approve = approver ∪ lead`: two approvers + one lead. The provider flattens all three;
/// the inspector renders EXACTLY that set — a different issue's approver never leaks in.
#[test]
fn cdc_4_4_inspector_membership_equals_list_subjects() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", "approver", "p:alice"),
            grant("issue:PROJ-1", "approver", "p:bob"),
            grant("issue:PROJ-1", "lead", "p:carol"),
            grant("issue:PROJ-2", "approver", "p:dave"),
        ],
    );
    let object = ObjectId("issue:PROJ-1".into());
    let perm = Permission("approve".into());
    let at = at_latest();

    // The PROVIDER's answer (the REAL Expand).
    let tree = svc.list_subjects_in(&s, &object, &perm, &at);

    // The CONSUMER (the inspector) reads the SAME engine through the port.
    let inspector = PermissionInspector::new(IdentityExpandResolver {
        svc,
        scope: s.clone(),
    });
    let answer = inspector.who_can(&object, &perm, &at);

    // The inspector's members EQUAL the provider's SubjectTree.members — byte-for-byte (0 private
    // recompute; the inspector invents/drops no member). PROJ-2's approver is ABSENT (leak-free).
    assert_eq!(
        answer.members, tree.members,
        "the inspector's membership must EQUAL Identity's list_subjects (0 private recompute)"
    );
    assert_eq!(
        answer.members,
        vec![
            PrincipalId("p:alice".into()),
            PrincipalId("p:bob".into()),
            PrincipalId("p:carol".into()),
        ],
        "exactly the approve membership (PROJ-2's approver absent — leak-free)"
    );
    assert_eq!(answer.object, object);
    assert_eq!(answer.permission, perm);
}

/// **The inspector's `why` trace EQUALS Identity's `explain` (0 private recompute).** The inspector renders
/// Identity's `RewriteTrace` VERBATIM — it authors no second explanation. The trace is non-empty and ends
/// in an ALLOW/DENY verdict (never a silent allow).
#[test]
fn cdc_4_4_inspector_why_equals_explain() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", "approver", "p:alice"),
            grant("issue:PROJ-1", "lead", "p:carol"),
        ],
    );
    let object = ObjectId("issue:PROJ-1".into());
    let perm = Permission("approve".into());
    let at = at_latest();

    // A GRANTED subject (alice is an approver) and a DENIED subject (mallory is neither).
    let alice = PrincipalId("p:alice".into());
    let mallory = PrincipalId("p:mallory".into());

    // The PROVIDER's traces (the REAL Expand).
    let provider_allow = svc.explain_in(&s, &alice, &perm, &object, &at);
    let provider_deny = svc.explain_in(&s, &mallory, &perm, &object, &at);

    let inspector = PermissionInspector::new(IdentityExpandResolver {
        svc,
        scope: s.clone(),
    });

    // The inspector's `why` trace EQUALS the provider's `explain` — byte-for-byte.
    let inspector_allow = inspector.why(&alice, &perm, &object, &at);
    let inspector_deny = inspector.why(&mallory, &perm, &object, &at);

    assert_eq!(
        inspector_allow.steps, provider_allow.steps,
        "the inspector's 'why' trace must EQUAL Identity's explain (0 private recompute)"
    );
    assert_eq!(
        inspector_deny.steps, provider_deny.steps,
        "the inspector's 'why' trace must EQUAL Identity's explain for a denied subject too"
    );

    // The verdicts are correct + non-empty (never a silent allow).
    assert!(
        !inspector_allow.steps.is_empty()
            && inspector_allow.steps.last().unwrap().starts_with("ALLOW"),
        "a granted subject's trace ends in ALLOW"
    );
    assert!(
        !inspector_deny.steps.is_empty()
            && inspector_deny.steps.last().unwrap().starts_with("DENY"),
        "a denied subject's trace ends in DENY (never empty, never a silent allow)"
    );
}
