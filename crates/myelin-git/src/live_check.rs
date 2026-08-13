use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    RelName, RelationTuple, SubjectTree, TupleDelta, Zookie,
};
use myelin_substrate::{
    AuthzDecision, AuthzServed, Clock, FailStaticAuthz, FailStaticError, FailStaticSignals,
    FailStaticThreshold, Seconds, ServeError, SystemClock,
};
use myelin_tenancy::ArtifactRef;

pub mod perm {
    pub const PULL: &str = "pull";
    pub const PUSH: &str = "push";
    pub const PROTECTED_PUSH: &str = "protected_push";
    pub const MERGE: &str = "merge";
    pub const APPROVE_UNTRUSTED_CI: &str = "approve_untrusted_ci";
    pub const CODE_OWNER: &str = "code_owner";
}

pub fn strong_at(zookie: Zookie) -> Consistency {
    Consistency {
        at_least: zookie,
        mode: ConsistencyMode::Strong,
    }
}

pub fn bounded_stale_at(zookie: Zookie) -> Consistency {
    Consistency {
        at_least: zookie,
        mode: ConsistencyMode::BoundedStale,
    }
}

pub struct GitCheckGate<I: IdentityService, C: Clock = SystemClock> {
    id: I,
    failstatic: FailStaticAuthz<C>,
}

impl<I: IdentityService> GitCheckGate<I, SystemClock> {
    pub fn try_new(
        id: I,
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
    ) -> Result<Self, FailStaticError> {
        let failstatic = FailStaticAuthz::try_new(revocation_sla_secs, threshold)?;
        Ok(Self { id, failstatic })
    }
}

impl<I: IdentityService, C: Clock> GitCheckGate<I, C> {
    pub fn try_new_with_clock(
        id: I,
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
        clock: C,
    ) -> Result<Self, FailStaticError> {
        let failstatic =
            FailStaticAuthz::try_new_with_clock(revocation_sla_secs, threshold, clock)?;
        Ok(Self { id, failstatic })
    }

    pub fn id_ref(&self) -> &I {
        &self.id
    }

    pub fn clock(&self) -> &C {
        self.failstatic.clock()
    }

    pub fn static_max(&self) -> Seconds {
        self.failstatic.static_max()
    }

    pub fn signals(&self) -> FailStaticSignals {
        self.failstatic.signals()
    }

    pub fn check_failstatic(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let key = cache_key(subject, permission, object);
        self.failstatic.serve(key, at, subject_revoked, || {
            self.id
                .check(subject, permission, object, at, None)
                .map_err(|e| ServeError(format!("git→Id check hiccup: {e:?}")))
        })
    }

    pub fn check_failstatic_result(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        subject_revoked: bool,
        result: myelin_identity::Result<Decision>,
    ) -> AuthzDecision {
        let key = cache_key(subject, permission, object);
        self.failstatic.serve(key, at, subject_revoked, || {
            result
                .clone()
                .map_err(|error| ServeError(format!("git→Id check hiccup: {error:?}")))
        })
    }

    pub fn front_door_check(
        &self,
        subject: &Principal,
        action_permission: &Permission,
        repo: &ArtifactRef,
        zookie: Zookie,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let at = bounded_stale_at(zookie);
        self.check_failstatic(subject, action_permission, repo, &at, subject_revoked)
    }

    pub fn protected_push_check(
        &self,
        subject: &Principal,
        ref_object: &ArtifactRef,
        zookie: Zookie,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let at = bounded_stale_at(zookie);
        let permission = Permission(perm::PROTECTED_PUSH.to_string());
        self.check_failstatic(subject, &permission, ref_object, &at, subject_revoked)
    }

    pub fn merge_check(
        &self,
        actor: &Principal,
        pr_object: &ArtifactRef,
        zookie: Zookie,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let at = strong_at(zookie);
        let permission = Permission(perm::MERGE.to_string());
        self.check_failstatic(actor, &permission, pr_object, &at, subject_revoked)
    }

    pub fn fork_endorsement_check(
        &self,
        subject: &Principal,
        repo_object: &ArtifactRef,
        zookie: Zookie,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let at = strong_at(zookie);
        let permission = Permission(perm::APPROVE_UNTRUSTED_CI.to_string());
        self.check_failstatic(subject, &permission, repo_object, &at, subject_revoked)
    }

    pub fn code_owners(
        &self,
        ref_object: &ObjectId,
        zookie: Zookie,
    ) -> myelin_identity::Result<SubjectTree> {
        let at = strong_at(zookie);
        let permission = Permission(perm::CODE_OWNER.to_string());
        self.id.list_subjects(ref_object, &permission, &at)
    }

    pub fn grant_relation(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        self.id.write_tuples(deltas, precondition)
    }
}

pub fn add_tuple(
    object: &str,
    relation: &str,
    subject: &myelin_identity::PrincipalId,
) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.to_string()),
        relation: RelName(relation.to_string()),
        subject: subject.clone(),
        caveat: None,
    })
}

fn cache_key(subject: &Principal, permission: &Permission, object: &ArtifactRef) -> String {
    myelin_substrate::encode_authz_key(&[
        subject.tenant.as_str(),
        subject.region.as_str(),
        &subject.principal_id.0,
        &permission.0,
        &object.0,
    ])
}

pub fn is_allow(d: &AuthzDecision) -> bool {
    matches!(d.decision, Decision::Allow)
}

pub fn is_degraded(d: &AuthzDecision) -> bool {
    matches!(d.served, AuthzServed::Static)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{
        AuthzError, CaveatContext, Credential, DataRole, ListObjectsResult, ObjectType,
        PrincipalId, PrincipalKind, PrincipalStatus, Result as IdResult, RewriteTrace,
        SubjectTree as IdSubjectTree, TupleDelta as IdTupleDelta,
    };
    use myelin_substrate::TestClock;
    use myelin_tenancy::{Region, TenantId};
    use std::cell::Cell;
    use std::collections::HashMap;

    struct StubId {
        allow: HashMap<String, Decision>,
        hiccup: Cell<bool>,
        code_owners: HashMap<String, Vec<PrincipalId>>,
        granted: std::cell::RefCell<Vec<String>>,
        zookie_seq: Cell<u64>,
    }

    impl StubId {
        fn new() -> Self {
            Self {
                allow: HashMap::new(),
                hiccup: Cell::new(false),
                code_owners: HashMap::new(),
                granted: std::cell::RefCell::new(Vec::new()),
                zookie_seq: Cell::new(0),
            }
        }
        fn allowing(mut self, perm: &str, object: &str) -> Self {
            self.allow
                .insert(format!("{perm}@{object}"), Decision::Allow);
            self
        }
        fn with_code_owners(mut self, object: &str, owners: &[&str]) -> Self {
            self.code_owners.insert(
                object.to_string(),
                owners.iter().map(|o| PrincipalId(o.to_string())).collect(),
            );
            self
        }
        fn set_hiccup(&self, on: bool) {
            self.hiccup.set(on);
        }
    }

    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _cav: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            if self.hiccup.get() {
                return Err(AuthzError::Unavailable("forced Id break (drill)".into()));
            }
            Ok(self
                .allow
                .get(&format!("{}@{}", permission.0, object.0))
                .copied()
                .unwrap_or(Decision::Deny))
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            object: &ObjectId,
            permission: &Permission,
            _at: &Consistency,
        ) -> IdResult<IdSubjectTree> {
            if self.hiccup.get() {
                return Err(AuthzError::Unavailable("forced Id break (drill)".into()));
            }
            assert_eq!(
                permission.0,
                perm::CODE_OWNER,
                "code_owners lists the code_owner relation"
            );
            let members = self.code_owners.get(&object.0).cloned().unwrap_or_default();
            Ok(IdSubjectTree {
                object: object.clone(),
                relation: RelName(permission.0.clone()),
                members,
                zookie: Zookie("zk-co".into()),
            })
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _a: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(
            &self,
            _a: &Principal,
            _t: &Principal,
        ) -> IdResult<myelin_identity::EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            deltas: &[IdTupleDelta],
            _p: Option<&myelin_identity::Precondition>,
        ) -> IdResult<Zookie> {
            for d in deltas {
                if let IdTupleDelta::Add(t) = d {
                    self.granted
                        .borrow_mut()
                        .push(format!("{}@{}@{}", t.relation.0, t.object.0, t.subject.0));
                }
            }
            let n = self.zookie_seq.get() + 1;
            self.zookie_seq.set(n);
            Ok(Zookie(format!("zk-{n}")))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &myelin_identity::RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<myelin_identity::RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(
            &self,
            _f: &myelin_identity::NamespaceFragment,
        ) -> IdResult<myelin_identity::FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
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

    const REVOCATION_SLA: Seconds = 300;

    fn subject(id: &str) -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("fr-par".into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn repo_ref(repo: &str) -> ArtifactRef {
        ArtifactRef(format!("repo:{repo}"))
    }

    fn gate(id: StubId, clock: TestClock) -> GitCheckGate<StubId, TestClock> {
        GitCheckGate::try_new_with_clock(id, REVOCATION_SLA, &threshold(), clock)
            .expect("valid staleness bound")
    }

    #[test]
    fn live_fragment_pull_grant_is_enforced_outsider_denied() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());

        let d = g.front_door_check(
            &subject("p:alice"),
            &pull,
            &repo,
            Zookie(String::new()),
            false,
        );
        assert!(
            is_allow(&d),
            "a granted reader pulls (live fragment enforced)"
        );
        assert_eq!(d.served, AuthzServed::Fresh);

        let other = repo_ref("secret");
        let d = g.front_door_check(
            &subject("p:bob"),
            &pull,
            &other,
            Zookie(String::new()),
            false,
        );
        assert!(
            !is_allow(&d),
            "an outsider is denied (0 unauthorized admitted)"
        );
    }

    #[test]
    fn protected_push_is_the_tighter_gate() {
        let ref_obj = ArtifactRef("ref:core::refs/heads/main".into());
        let id = StubId::new().allowing(perm::PROTECTED_PUSH, &ref_obj.0);
        let g = gate(id, TestClock::at(1_000));

        let d = g.protected_push_check(&subject("p:admin"), &ref_obj, Zookie(String::new()), false);
        assert!(is_allow(&d), "an admin pushes the protected ref");

        let other_ref = ArtifactRef("ref:core::refs/heads/release".into());
        let d = g.protected_push_check(
            &subject("p:writer"),
            &other_ref,
            Zookie(String::new()),
            false,
        );
        assert!(
            !is_allow(&d),
            "a mere writer cannot push a different protected ref (fail-closed)"
        );
    }

    #[test]
    fn fork_endorsement_is_a_plain_relation_check_with_read_your_writes() {
        let repo = repo_ref("core");
        let id = StubId::new();
        let g = gate(id, TestClock::at(1_000));

        let d = g.fork_endorsement_check(&subject("p:maint"), &repo, Zookie(String::new()), false);
        assert!(
            !is_allow(&d),
            "no endorsement relation yet → denied (X-1, fail-closed)"
        );

        let delta = add_tuple(
            &repo.0,
            perm::APPROVE_UNTRUSTED_CI,
            &PrincipalId("p:maint".into()),
        );
        let zk = g.grant_relation(&[delta], None).expect("grant");
        assert_eq!(
            zk,
            Zookie("zk-1".into()),
            "write_tuples returns a fresh read-your-writes fence"
        );

        assert_eq!(g.id_ref().granted.borrow().len(), 1);

        let _ = zk;
    }

    #[test]
    fn code_owners_resolves_via_list_subjects() {
        let ref_id = ObjectId("ref:core::/src/payments/**".into());
        let id = StubId::new().with_code_owners(&ref_id.0, &["p:alice", "team:payments"]);
        let g = gate(id, TestClock::at(1_000));

        let tree = g
            .code_owners(&ref_id, Zookie(String::new()))
            .expect("resolved");
        let owners: Vec<&str> = tree.members.iter().map(|p| p.0.as_str()).collect();
        assert!(owners.contains(&"p:alice"), "alice is a required reviewer");
        assert!(
            owners.contains(&"team:payments"),
            "the payments team is a required reviewer"
        );
    }

    #[test]
    fn forced_id_break_degrades_not_cascades_on_bounded_stale() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());
        let alice = subject("p:alice");

        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
        assert!(is_allow(&d) && d.served == AuthzServed::Fresh);

        g.id_ref().set_hiccup(true);

        g.clock().advance(31);
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
        assert!(
            d.is_degraded(),
            "the BoundedStale read served STATIC during the Id hiccup"
        );
        assert!(
            is_allow(&d),
            "the degraded answer is the cached ALLOW (already-authorised survives)"
        );

        g.id_ref().set_hiccup(false);
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
        assert_eq!(
            d.served,
            AuthzServed::Fresh,
            "recovered → fresh again (the degrade was bounded)"
        );

        let s = g.signals();
        assert!(
            s.stale >= 1,
            "a degrade was observed (fresh/stale/closed ratio signal)"
        );
        assert!(
            s.last_staleness_secs <= g.static_max(),
            "staleness ≤ static_max (≤ revocation SLA)"
        );
    }

    #[test]
    fn a_batched_identity_answer_keeps_the_same_bounded_static_fallback() {
        let g = gate(StubId::new(), TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());
        let alice = subject("p:alice");
        let at = bounded_stale_at(Zookie(String::new()));

        let fresh =
            g.check_failstatic_result(&alice, &pull, &repo, &at, false, Ok(Decision::Allow));
        assert!(is_allow(&fresh));
        assert_eq!(fresh.served, AuthzServed::Fresh);

        g.clock().advance(31);
        let degraded = g.check_failstatic_result(
            &alice,
            &pull,
            &repo,
            &at,
            false,
            Err(AuthzError::Unavailable("batch snapshot unavailable".into())),
        );
        assert!(
            is_allow(&degraded),
            "a cached grant survives the same bounded outage"
        );
        assert_eq!(degraded.served, AuthzServed::Static);
    }

    #[test]
    fn just_revoked_subject_is_denied_through_the_stale_cache() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());
        let alice = subject("p:alice");

        assert!(is_allow(&g.front_door_check(
            &alice,
            &pull,
            &repo,
            Zookie(String::new()),
            false
        )));

        g.id_ref().set_hiccup(true);
        g.clock().advance(31);
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), true);
        assert_eq!(
            d.served,
            AuthzServed::Revoked,
            "a revoked subject is denied through the cache"
        );
        assert!(
            !is_allow(&d),
            "the cached ALLOW does NOT override the revoke (0 stale escalation)"
        );
    }

    #[test]
    fn strong_merge_read_bypasses_the_cache_and_fails_closed_on_a_hiccup() {
        let pr = ArtifactRef("pr:core:42".into());
        let id = StubId::new().allowing(perm::MERGE, &pr.0);
        let g = gate(id, TestClock::at(1_000));
        let actor = subject("p:admin");

        let d = g.merge_check(&actor, &pr, Zookie("zk-1".into()), false);
        assert!(is_allow(&d), "a healthy merge read allows");
        assert_eq!(
            d.served,
            AuthzServed::SourceBypass,
            "a Strong read bypasses the cache"
        );

        g.id_ref().set_hiccup(true);
        let d = g.merge_check(&actor, &pr, Zookie("zk-2".into()), false);
        assert!(
            !is_allow(&d),
            "a Strong read fails CLOSED on a hiccup (never stale)"
        );
        assert_eq!(d.served, AuthzServed::BypassClosed);
    }

    #[test]
    fn past_static_max_a_sustained_break_fails_closed_never_open() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());
        let alice = subject("p:alice");

        assert!(is_allow(&g.front_door_check(
            &alice,
            &pull,
            &repo,
            Zookie(String::new()),
            false
        )));
        g.id_ref().set_hiccup(true);

        g.clock().advance(301);
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
        assert!(
            !is_allow(&d),
            "past static_max → fail CLOSED (deny is correct again)"
        );
        assert_eq!(
            d.served,
            AuthzServed::Closed,
            "Closed, never an open fall-through"
        );
    }

    #[test]
    fn a_static_max_over_the_revocation_sla_does_not_construct() {
        let bad = FailStaticThreshold {
            status: "OPEN - LEGAL".into(),
            owner: "DPO / Legal".into(),
            static_max_secs: None,
            static_max_default_secs: 400,
            agent_token_ttl_secs: 60,
            constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
        };
        match GitCheckGate::try_new(StubId::new(), REVOCATION_SLA, &bad) {
            Err(FailStaticError::ExceedsRevocationSla { .. }) => {}
            Err(other) => panic!("wrong rejection (expected ExceedsRevocationSla): {other:?}"),
            Ok(_) => panic!("a static_max > revocation SLA must NOT construct (4.11)"),
        }
    }

    #[test]
    fn cache_key_partitions_by_verified_principal_no_cross_actor_leak() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());

        assert!(is_allow(&g.front_door_check(
            &subject("p:alice"),
            &pull,
            &repo,
            Zookie(String::new()),
            false
        )));

        g.id_ref().set_hiccup(true);
        g.clock().advance(31);
        let d = g.front_door_check(
            &subject("p:bob"),
            &pull,
            &repo,
            Zookie(String::new()),
            false,
        );
        assert!(
            !is_allow(&d),
            "bob does NOT inherit alice's cached grant (no cross-actor leak)"
        );
        assert_eq!(
            d.served,
            AuthzServed::Closed,
            "bob has no bucket → Closed, never alice's Static"
        );
    }

    #[test]
    fn cache_key_is_distinct_per_question() {
        let alice = subject("p:alice");
        let bob = subject("p:bob");
        let pull = Permission(perm::PULL.into());
        let push = Permission(perm::PUSH.into());
        let core = repo_ref("core");
        let secret = repo_ref("secret");

        let k = |s: &Principal, p: &Permission, o: &ArtifactRef| cache_key(s, p, o);
        assert_ne!(
            k(&alice, &pull, &core),
            k(&bob, &pull, &core),
            "subject differs"
        );
        assert_ne!(
            k(&alice, &pull, &core),
            k(&alice, &push, &core),
            "permission differs"
        );
        assert_ne!(
            k(&alice, &pull, &core),
            k(&alice, &pull, &secret),
            "object differs"
        );
        assert_eq!(
            k(&alice, &pull, &core),
            k(&alice, &pull, &core),
            "same question, same bucket"
        );
    }

    #[test]
    fn cache_key_is_injective_against_delimiter_injection() {
        let pull = Permission(perm::PULL.into());

        let alice_evil = subject("alice::pull@repo:secret");
        let obj_pub = ArtifactRef("repo:pub".into());

        let alice_plain = subject("alice");
        let obj_crafted = ArtifactRef("repo:secret::pull@repo:pub".into());

        assert_ne!(
            cache_key(&alice_evil, &pull, &obj_pub),
            cache_key(&alice_plain, &pull, &obj_crafted),
            "a delimiter-embedding principal id must NOT collide with a different (subject, object) \
             question - the R2.3b injective encoding keeps them distinct"
        );

        assert_eq!(
            cache_key(&alice_evil, &pull, &obj_pub),
            cache_key(&alice_evil, &pull, &obj_pub),
            "the same question is still a stable single key"
        );
    }

    #[test]
    fn classifiers_report_exactly_their_rung() {
        let fresh_allow = AuthzDecision {
            decision: Decision::Allow,
            served: AuthzServed::Fresh,
        };
        let static_allow = AuthzDecision {
            decision: Decision::Allow,
            served: AuthzServed::Static,
        };
        let closed_deny = AuthzDecision {
            decision: Decision::Deny,
            served: AuthzServed::Closed,
        };

        assert!(is_allow(&fresh_allow) && !is_degraded(&fresh_allow));
        assert!(is_allow(&static_allow) && is_degraded(&static_allow));
        assert!(!is_allow(&closed_deny) && !is_degraded(&closed_deny));
    }
}
