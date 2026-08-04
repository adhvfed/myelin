use crate::check_engine::CheckEngine;
use crate::machine_auth::Authority;
use myelin_identity::{Consistency, Decision, EffectivePolicy, Permission, Principal, RelName};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;

pub const EFFECTIVE_GRANT_CARRIER: &str = "grant";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationInput {
    pub agent_policy: Authority,
    pub delegation: Authority,
    pub tenant_policy: Authority,
    pub trigger_actor_held: Authority,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntersectionProof {
    pub agent_policy: Vec<String>,
    pub delegated_after_recheck: Vec<String>,
    pub tenant_policy: Vec<String>,
    pub effective: Vec<String>,
    pub subset_of_every_conjunct: bool,
}

impl IntersectionProof {
    pub fn holds(&self) -> bool {
        self.subset_of_every_conjunct
    }
}

#[derive(Clone)]
pub struct DelegationAlgebra {
    engine: Option<CheckEngine>,
}

impl Default for DelegationAlgebra {
    fn default() -> Self {
        DelegationAlgebra::new()
    }
}

impl DelegationAlgebra {
    pub fn new() -> DelegationAlgebra {
        DelegationAlgebra { engine: None }
    }

    pub fn with_check(engine: CheckEngine) -> DelegationAlgebra {
        DelegationAlgebra {
            engine: Some(engine),
        }
    }

    pub fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
        input: &DelegationInput,
    ) -> EffectivePolicy {
        let (effective, _proof) = self.compose(input);
        effective_policy_of(&effective)
    }

    pub fn delegation_proved(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
        input: &DelegationInput,
    ) -> (EffectivePolicy, IntersectionProof) {
        let (effective, proof) = self.compose(input);
        (effective_policy_of(&effective), proof)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn delegation_with_check(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &DelegationInput,
        scope: &TenantScope,
        required_grant: &str,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
    ) -> Decision {
        let (effective, _proof) = self.compose(input);
        if !effective.holds(required_grant) {
            return Decision::Deny;
        }
        let _ = trigger_actor;

        let engine = match &self.engine {
            Some(e) => e,
            None => return Decision::Deny,
        };
        let relation = RelName(permission.0.clone());
        engine.check(scope, agent, &relation, object, at, None)
    }

    fn compose(&self, input: &DelegationInput) -> (Authority, IntersectionProof) {
        let delegated = input.delegation.attenuate(&input.trigger_actor_held);

        let effective = input
            .agent_policy
            .attenuate(&delegated)
            .attenuate(&input.tenant_policy);

        let subset_of_every_conjunct = effective.is_subset_of(&input.agent_policy)
            && effective.is_subset_of(&delegated)
            && effective.is_subset_of(&input.tenant_policy);

        let proof = IntersectionProof {
            agent_policy: input.agent_policy.grants().map(str::to_string).collect(),
            delegated_after_recheck: delegated.grants().map(str::to_string).collect(),
            tenant_policy: input.tenant_policy.grants().map(str::to_string).collect(),
            effective: effective.grants().map(str::to_string).collect(),
            subset_of_every_conjunct,
        };
        (effective, proof)
    }
}

pub fn effective_policy_of(authority: &Authority) -> EffectivePolicy {
    EffectivePolicy {
        caveats: authority.grants().map(str::to_string).collect(),
    }
}

pub fn authority_of(policy: &EffectivePolicy) -> Authority {
    Authority::of(policy.caveats.iter().cloned())
}

pub mod mutation {
    use super::*;

    pub fn composition_is_intersection(
        agent_policy: &Authority,
        delegation: &Authority,
        tenant_policy: &Authority,
        trigger_actor_held: &Authority,
    ) -> bool {
        let algebra = DelegationAlgebra::new();
        let input = DelegationInput {
            agent_policy: agent_policy.clone(),
            delegation: delegation.clone(),
            tenant_policy: tenant_policy.clone(),
            trigger_actor_held: trigger_actor_held.clone(),
        };
        let (_effective, proof) = algebra.compose(&input);
        proof.holds()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TupleStore;
    use myelin_events::{OutboxStore, Timestamp};
    use myelin_identity::{
        ConsistencyMode, ObjectId, PrincipalId, PrincipalKind, RelationTuple, RuntimeRef,
        TupleDelta, Zookie,
    };
    use myelin_tenancy::{Region, TenantId};

    fn agent_principal(id: &str) -> Principal {
        let mut p = Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt-1".into()),
                on_behalf_of: Some(PrincipalId("p:human".into())),
            },
            TenantId("acme".into()),
        );
        p.region = Region("eu-west".into());
        p
    }

    fn human(id: &str) -> Principal {
        let mut p = Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        p.region = Region("eu-west".into());
        p
    }

    fn auth(grants: &[&str]) -> Authority {
        Authority::of(grants.iter().copied())
    }

    fn input(agent: &[&str], deleg: &[&str], tenant: &[&str], held: &[&str]) -> DelegationInput {
        DelegationInput {
            agent_policy: auth(agent),
            delegation: auth(deleg),
            tenant_policy: auth(tenant),
            trigger_actor_held: auth(held),
        }
    }

    #[test]
    fn intersection_is_monotone_never_grows() {
        let algebra = DelegationAlgebra::new();
        let inp = input(
            &["a", "b", "c", "d"],
            &["a", "b", "c"],
            &["b", "c", "d"],
            &["a", "b", "c"],
        );
        let (eff, proof) =
            algebra.delegation_proved(&agent_principal("p:agent"), &human("p:human"), &inp);
        assert_eq!(eff.caveats, vec!["b".to_string(), "c".to_string()]);
        assert!(
            proof.holds(),
            "the effective set is a subset of every conjunct (monotone)"
        );

        let tighter = input(
            &["a", "b", "c", "d"],
            &["a", "b", "c"],
            &["b"],
            &["a", "b", "c"],
        );
        let eff2 = algebra.delegation(&agent_principal("p:agent"), &human("p:human"), &tighter);
        assert_eq!(
            eff2.caveats,
            vec!["b".to_string()],
            "a tighter conjunct narrows, never grows"
        );
        assert!(
            authority_of(&eff2).is_subset_of(&authority_of(&eff)),
            "adding/tightening a conjunct yields a SUBSET (monotone - never amplifies)"
        );
    }

    #[test]
    fn authority_attenuates_through_the_caveat_chain() {
        let algebra = DelegationAlgebra::new();
        let inp = input(
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/api#read"],
            &[
                "repo:acme/web#read",
                "repo:acme/web#write",
                "repo:acme/api#read",
            ],
            &["repo:acme/web#read", "repo:acme/api#read"],
        );
        let eff = algebra.delegation(&agent_principal("p:agent"), &human("p:human"), &inp);
        assert_eq!(eff.caveats, vec!["repo:acme/web#read".to_string()]);
        assert!(
            authority_of(&eff).is_subset_of(&auth(&["repo:acme/web#read", "repo:acme/web#write"])),
            "the effective set never exceeds the agent ceiling (a chain link)"
        );
    }

    #[test]
    fn revoking_delegators_grant_shrinks_agent_authority() {
        let algebra = DelegationAlgebra::new();
        let agent = agent_principal("p:agent");
        let delegator = human("p:human");

        let before = input(
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
        );
        let eff_before = algebra.delegation(&agent, &delegator, &before);
        assert!(
            authority_of(&eff_before).holds("repo:acme/web#write"),
            "while the delegator holds #write, the agent's effective set includes it"
        );

        let after = input(
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read"],
        );
        let eff_after = algebra.delegation(&agent, &delegator, &after);
        assert!(
            !authority_of(&eff_after).holds("repo:acme/web#write"),
            "once the delegator loses #write, the agent's effective authority shrinks (ID-D5)"
        );
        assert!(
            authority_of(&eff_after).is_subset_of(&authority_of(&eff_before)),
            "the revocation only ever shrinks the effective set (monotone)"
        );
    }

    #[test]
    fn cannot_delegate_authority_you_never_held() {
        let algebra = DelegationAlgebra::new();
        let inp = input(
            &["repo:acme/web#admin"],
            &["repo:acme/web#admin"],
            &["repo:acme/web#admin"],
            &["repo:acme/web#read"],
        );
        let (eff, proof) =
            algebra.delegation_proved(&agent_principal("p:agent"), &human("p:human"), &inp);
        assert!(
            eff.caveats.is_empty(),
            "a grant the delegator never held is never delegated"
        );
        assert!(proof.holds());
        assert!(
            proof.delegated_after_recheck.is_empty(),
            "the re-check dropped the un-held grant before composition"
        );
    }

    #[test]
    fn mutation_floor_composition_is_intersection_not_union() {
        type ConjunctCase<'a> = (&'a [&'a str], &'a [&'a str], &'a [&'a str], &'a [&'a str]);
        let cases: &[ConjunctCase] = &[
            (&["a", "b"], &["a", "b", "c"], &["a", "b"], &["a", "b", "c"]),
            (&["a", "b", "c"], &["b"], &["a", "b", "c"], &["b"]),
            (&[], &["a"], &["a"], &["a"]),
            (&["a"], &[], &["a"], &["a"]),
            (&["a"], &["a"], &[], &["a"]),
            (&["a"], &["a"], &["a"], &[]),
            (&["x", "y"], &["x", "y"], &["x", "y"], &["x", "y"]),
        ];
        for (agent, deleg, tenant, held) in cases {
            assert!(
                mutation::composition_is_intersection(
                    &auth(agent),
                    &auth(deleg),
                    &auth(tenant),
                    &auth(held),
                ),
                "compose({agent:?},{deleg:?},{tenant:?},held={held:?}) must be ⊆ every conjunct \
                 (intersection, never union)"
            );
        }
        let algebra = DelegationAlgebra::new();
        let inp = input(&["a", "b"], &["b", "c"], &["b", "d"], &["b", "c"]);
        let eff = algebra.delegation(&agent_principal("p:agent"), &human("p:human"), &inp);
        assert_eq!(
            eff.caveats,
            vec!["b".to_string()],
            "the effective set is the intersection {{b}}"
        );
    }

    #[test]
    fn effective_policy_carrier_round_trips() {
        let a = auth(&["c:3", "a:1", "b:2"]);
        let policy = effective_policy_of(&a);
        assert_eq!(
            policy.caveats,
            vec!["a:1".to_string(), "b:2".to_string(), "c:3".to_string()]
        );
        assert_eq!(
            authority_of(&policy),
            a,
            "the carrier round-trips to the same authority"
        );
        assert_eq!(EFFECTIVE_GRANT_CARRIER, "grant");
    }

    #[test]
    fn fourth_conjunct_object_check_run_as_agent() {
        let store = TupleStore::new(OutboxStore::new());
        let scope = {
            let admin = human("p:admin");
            TenantScope::from_verified_token(&admin, Region("eu-west".into()))
        };
        store
            .write_tuples(
                &scope,
                &human("p:admin"),
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId("repo:core".into()),
                    relation: RelName("write".into()),
                    subject: PrincipalId("p:agent".into()),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-06-19T00:00:00Z".into()),
            )
            .expect("seed the agent's object grant");

        let algebra = DelegationAlgebra::with_check(CheckEngine::new(store));
        let agent = agent_principal("p:agent");
        let delegator = human("p:human");
        let at = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        let obj = ArtifactRef("myelin://acme/git/repo/repo:core".into());
        let inp = input(
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
        );

        let d = algebra.delegation_with_check(
            &agent,
            &delegator,
            &inp,
            &scope,
            "repo:acme/web#write",
            &Permission("write".into()),
            &obj,
            &at,
        );
        assert_eq!(
            d,
            Decision::Allow,
            "grant in the intersection + object check pass ⇒ Allow"
        );

        let d_cap = algebra.delegation_with_check(
            &agent,
            &delegator,
            &inp,
            &scope,
            "repo:acme/web#admin",
            &Permission("write".into()),
            &obj,
            &at,
        );
        assert_eq!(
            d_cap,
            Decision::Deny,
            "a grant outside the intersection is refused"
        );

        let d_obj = algebra.delegation_with_check(
            &agent,
            &delegator,
            &inp,
            &scope,
            "repo:acme/web#write",
            &Permission("delete".into()),
            &obj,
            &at,
        );
        assert_eq!(
            d_obj,
            Decision::Deny,
            "a failed object check refuses (fail-closed)"
        );
    }

    #[test]
    fn four_conjunct_check_without_engine_fails_closed() {
        let algebra = DelegationAlgebra::new();
        let scope = TenantScope::from_verified_token(&human("p:admin"), Region("eu-west".into()));
        let at = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        let inp = input(&["g"], &["g"], &["g"], &["g"]);
        let d = algebra.delegation_with_check(
            &agent_principal("p:agent"),
            &human("p:human"),
            &inp,
            &scope,
            "g",
            &Permission("write".into()),
            &ArtifactRef("myelin://acme/git/repo/repo:core".into()),
            &at,
        );
        assert_eq!(
            d,
            Decision::Deny,
            "no object-check engine ⇒ fail-closed Deny"
        );
    }
}
