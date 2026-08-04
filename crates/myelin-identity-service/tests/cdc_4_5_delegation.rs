use myelin_events::OutboxStore;
use myelin_identity::{EffectivePolicy, Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_identity_service::{
    authority_of, Authority, DelegationInput, StoreBackedCheck, TupleStore,
};
use myelin_tenancy::{Region, TenantId};

fn agent(id: &str) -> Principal {
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

fn provider() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

fn effect_api_apply(effective: &EffectivePolicy, required_capability: &str) -> bool {
    authority_of(effective).holds(required_capability)
}

#[test]
fn cdc_4_5_effect_inside_intersection_applies() {
    let svc = provider();
    let input = DelegationInput {
        agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
        delegation: auth(&["repo:acme/web#read", "repo:acme/api#read"]),
        tenant_policy: auth(&[
            "repo:acme/web#read",
            "repo:acme/api#read",
            "repo:acme/web#write",
        ]),
        trigger_actor_held: auth(&["repo:acme/web#read", "repo:acme/api#read"]),
    };
    let effective = svc.delegation_in(&agent("p:agent"), &human("p:human"), &input);
    assert_eq!(effective.caveats, vec!["repo:acme/web#read".to_string()]);
    assert!(
        effect_api_apply(&effective, "repo:acme/web#read"),
        "an effect whose capability is in the effective set is applied"
    );
}

#[test]
fn cdc_4_5_effect_outside_intersection_refused() {
    let svc = provider();
    let input = DelegationInput {
        agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
        delegation: auth(&["repo:acme/web#read"]),
        tenant_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
        trigger_actor_held: auth(&["repo:acme/web#read"]),
    };
    let effective = svc.delegation_in(&agent("p:agent"), &human("p:human"), &input);
    assert!(
        !effect_api_apply(&effective, "repo:acme/web#write"),
        "an effect outside the intersection is refused (the agent is confined to agent ∩ delegation ∩ tenant)"
    );
    assert!(
        effect_api_apply(&effective, "repo:acme/web#read"),
        "the delegated grant is still applied"
    );
}

#[test]
fn cdc_4_5_provider_effective_set_is_subset_of_every_conjunct() {
    let svc = provider();
    let input = DelegationInput {
        agent_policy: auth(&["a", "b", "c"]),
        delegation: auth(&["b", "c", "d"]),
        tenant_policy: auth(&["c", "d", "e"]),
        trigger_actor_held: auth(&["b", "c", "d"]),
    };
    let (effective, proof) = svc.delegation_proved_in(&agent("p:agent"), &human("p:human"), &input);
    assert_eq!(effective.caveats, vec!["c".to_string()]);
    assert!(
        proof.holds(),
        "the provider witnesses the monotone intersection (effective ⊆ every conjunct)"
    );
    assert!(effect_api_apply(&effective, "c"));
    assert!(
        !effect_api_apply(&effective, "a"),
        "a (only in the agent ceiling) is NOT effective"
    );
    assert!(
        !effect_api_apply(&effective, "e"),
        "e (only in the tenant policy) is NOT effective"
    );
}
