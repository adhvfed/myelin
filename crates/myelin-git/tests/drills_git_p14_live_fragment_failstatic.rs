use std::cell::Cell;

use myelin_events::{OutboxStore, Timestamp};
use myelin_git::live_check::{is_allow, perm, GitCheckGate};
use myelin_identity::{
    Consistency, Decision, IdentityService, ObjectId, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, SubjectTree, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::{AuthzServed, FailStaticThreshold, TestClock};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

struct BreakableId {
    inner: StoreBackedCheck,
    scope: TenantScope,
    broken: Cell<bool>,
}

impl BreakableId {
    fn new(inner: StoreBackedCheck, scope: TenantScope) -> Self {
        Self {
            inner,
            scope,
            broken: Cell::new(false),
        }
    }
    fn set_broken(&self, on: bool) {
        self.broken.set(on);
    }
}

impl IdentityService for BreakableId {
    fn authenticate(&self, c: &myelin_identity::Credential) -> myelin_identity::Result<Principal> {
        self.inner.authenticate(c)
    }
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&myelin_identity::CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        if self.broken.get() {
            return Err(myelin_identity::AuthzError::Unavailable(
                "forced Id break (drill)".into(),
            ));
        }
        self.inner.check(subject, permission, object, at, caveat)
    }
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &myelin_identity::ObjectType,
        at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::ListObjectsResult> {
        self.inner.list_objects(subject, permission, ty, at)
    }
    fn list_subjects(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> myelin_identity::Result<SubjectTree> {
        if self.broken.get() {
            return Err(myelin_identity::AuthzError::Unavailable(
                "forced Id break (drill)".into(),
            ));
        }
        Ok(self
            .inner
            .list_subjects_in(&self.scope, object, permission, at))
    }
    fn explain(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        self.inner.explain(subject, permission, object, at)
    }
    fn delegation(
        &self,
        agent: &Principal,
        trigger: &Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        self.inner.delegation(agent, trigger)
    }
    fn write_tuples(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        self.inner.write_tuples(deltas, precondition)
    }
    fn mint_run_token(
        &self,
        a: &PrincipalId,
        r: &myelin_identity::RunId,
        d: &myelin_identity::DelegationCaveats,
        t: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        self.inner.mint_run_token(a, r, d, t)
    }
    fn revoke(&self, t: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        self.inner.revoke(t)
    }
    fn resolve_pseudonym(&self, s: &PrincipalId, t: &TenantId) -> myelin_identity::Result<String> {
        self.inner.resolve_pseudonym(s, t)
    }
    fn erase(&self, s: &PrincipalId) -> myelin_identity::Result<()> {
        self.inner.erase(s)
    }
    fn admit_fragment(
        &self,
        f: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        self.inner.admit_fragment(f)
    }
}

fn scope(tenant: &str) -> TenantScope {
    let admin = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&admin, Region("fr-par".into()))
}

fn subject(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("fr-par".into());
    p
}

fn add(object: &str, relation: &str, subj: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subj.into()),
        caveat: None,
    })
}

fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}

const REVOCATION_SLA: u64 = 300;

fn engine_with_git_fragment(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    let admin = subject("p-admin", scope.tenant().as_str());
    store
        .write_tuples(
            scope,
            &admin,
            tuples,
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("seed tuples");
    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_git_fragment() {
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Git fragment admits into the live cell schema: {admit:?}"
        );
    }
    svc
}

#[test]
fn cdc_4_9_live_fragment_is_enforced_at_the_check() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(&s, &[add("repo:core", "admin", "p:alice")]);
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid staleness bound");

    let repo = ArtifactRef("repo:core".into());
    let pull = Permission(perm::PULL.into());

    let d = gate.front_door_check(
        &subject("p:alice", "acme"),
        &pull,
        &repo,
        Zookie(String::new()),
        false,
    );
    assert!(
        is_allow(&d),
        "a repo admin pulls through the LIVE fragment (0 unauthorized denied)"
    );

    let d = gate.front_door_check(
        &subject("p:bob", "acme"),
        &pull,
        &repo,
        Zookie(String::new()),
        false,
    );
    assert!(
        !is_allow(&d),
        "an outsider is denied (0 unauthorized action admitted)"
    );
}

#[test]
fn cdc_4_9_fork_endorsement_relation_is_enforced_live() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(&s, &[add("repo:core", "approve_untrusted_ci", "p:maint")]);
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid");
    let repo = ArtifactRef("repo:core".into());

    let d = gate.fork_endorsement_check(
        &subject("p:maint", "acme"),
        &repo,
        Zookie(String::new()),
        false,
    );
    assert!(
        is_allow(&d),
        "a maintainer with approve_untrusted_ci endorses (X-1, live)"
    );
    let d = gate.fork_endorsement_check(
        &subject("p:bob", "acme"),
        &repo,
        Zookie(String::new()),
        false,
    );
    assert!(
        !is_allow(&d),
        "an outsider cannot endorse (X-1, fail-closed)"
    );
}

#[test]
fn chained_grant_read_your_writes_break_degrade_revoked_denied() {
    let s = scope("acme");
    let store = TupleStore::new(OutboxStore::new());
    let admin = subject("p-admin", "acme");

    let zookie = store
        .write_tuples(
            &s,
            &admin,
            &[add("repo:core", "admin", "p:alice")],
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("grant alice admin");
    assert!(
        !zookie.0.is_empty(),
        "the grant returns a read-your-writes zookie fence"
    );

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_git_fragment() {
        assert!(matches!(
            admit,
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid");

    let repo = ArtifactRef("repo:core".into());
    let pull = Permission(perm::PULL.into());
    let alice = subject("p:alice", "acme");

    let d = gate.front_door_check(&alice, &pull, &repo, zookie.clone(), false);
    assert!(
        is_allow(&d),
        "read-your-writes: the just-granted admin pulls immediately (4.10)"
    );
    assert_eq!(
        d.served,
        AuthzServed::Fresh,
        "served fresh from the live engine"
    );

    gate.id_ref().set_broken(true);

    gate.clock().advance(31);
    let d = gate.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
    assert!(
        d.is_degraded(),
        "the BoundedStale read DEGRADES (served STATIC) during the Id break"
    );
    assert!(
        is_allow(&d),
        "the degraded answer is the cached ALLOW - already-authorised survives"
    );

    let sig = gate.signals();
    assert!(
        sig.stale >= 1,
        "the degrade is observable (fresh/stale/closed ratio signal)"
    );
    assert!(
        sig.last_staleness_secs <= gate.static_max(),
        "staleness age ≤ static_max ≤ revocation SLA (a degrade never outlives the bound)"
    );

    let d = gate.front_door_check(
        &alice,
        &pull,
        &repo,
        Zookie(String::new()),
         true,
    );
    assert_eq!(
        d.served,
        AuthzServed::Revoked,
        "a revoked subject is denied through the cache"
    );
    assert!(
        !is_allow(&d),
        "the cached ALLOW does NOT override the revoke (0 stale escalation)"
    );

    gate.id_ref().set_broken(false);
    let d = gate.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
    assert_eq!(
        d.served,
        AuthzServed::Fresh,
        "recovered → fresh again (the degrade was bounded)"
    );
}

#[test]
fn cdc_4_11_strong_merge_read_bypasses_cache_fails_closed_on_break() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(
        &s,
        &[
            add("repo:core", "admin", "p:alice"),
            add(
                "pull_request:core:42",
                "parent_repo",
                "repo:core#protected_push",
            ),
        ],
    );
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid");
    let pr = ArtifactRef("pull_request:core:42".into());
    let alice = subject("p:alice", "acme");

    let d = gate.merge_check(&alice, &pr, Zookie(String::new()), false);
    assert_eq!(
        d.served,
        AuthzServed::SourceBypass,
        "a Strong merge read bypasses the cache"
    );
    assert!(
        is_allow(&d),
        "alice (admin → protected_push → merge) may merge"
    );

    gate.id_ref().set_broken(true);
    let d = gate.merge_check(&alice, &pr, Zookie(String::new()), false);
    assert_eq!(
        d.served,
        AuthzServed::BypassClosed,
        "a Strong read fails CLOSED on a break"
    );
    assert!(
        !is_allow(&d),
        "a security-sensitive merge read never serves stale (4.10)"
    );
}

#[test]
fn cdc_4_11_static_max_over_revocation_sla_does_not_construct() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(&s, &[]);
    let bad = FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 400,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    let built = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &bad,
        TestClock::at(0),
    );
    assert!(
        matches!(
            built,
            Err(myelin_substrate::FailStaticError::ExceedsRevocationSla { .. })
        ),
        "a static_max > revocation SLA must NOT construct (4.11, the §8.2 bound is structural)"
    );
}

#[test]
fn cdc_4_11_past_static_max_sustained_break_fails_closed() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(&s, &[add("repo:core", "admin", "p:alice")]);
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid");
    let repo = ArtifactRef("repo:core".into());
    let pull = Permission(perm::PULL.into());
    let alice = subject("p:alice", "acme");

    assert!(is_allow(&gate.front_door_check(
        &alice,
        &pull,
        &repo,
        Zookie(String::new()),
        false
    )));
    gate.id_ref().set_broken(true);
    gate.clock().advance(301);
    let d = gate.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
    assert_eq!(
        d.served,
        AuthzServed::Closed,
        "past static_max → Closed (deny is correct), never open"
    );
    assert!(!is_allow(&d));
}
