//! # The CDC pair for contract 4.5 — `delegation(agent, trigger_actor) → EffectivePolicy`
//! (P-ID-17 / P-075)
//!
//! **Contract-index row 4.5** (`delegation`, the monotone-intersection algebra
//! `agent.policy ∩ delegation ∩ tenant.policy`, attenuation-never-amplification over the
//! macaroon/biscuit caveat chains; consumed by the Agent `EffectApi` + workflow activities). This is
//! the dedicated provider+consumer pair the P-ID-17 TESTS field names — the focused, in-CI evidence
//! that the two sides of the `delegation` seam cannot drift apart:
//!
//! - the **PROVIDER** ([`StoreBackedCheck::delegation_in`]) composes the three policy conjuncts as a
//!   monotone set intersection (with the "you cannot delegate authority you do not have" re-check)
//!   and returns the frozen [`EffectivePolicy`] carrying the composed grant set;
//! - the **CONSUMER is an `EffectApi`-side caller** — exactly the Agent-Fabric shape contract 4.5
//!   names ("consumed by the Agent EffectApi"): before it applies a proposed effect it asks
//!   `delegation(agent, trigger_actor)` for the effective policy and proceeds ONLY if the effect's
//!   required capability is INSIDE that effective set. An effect outside the intersection is refused
//!   (the agent is confined to `agent ∩ delegation ∩ tenant` — the structural floor that makes "an
//!   agent can do what no human role can" impossible).
//!
//! The provider's promise (the effective set is the monotone intersection, never a superset of any
//! conjunct) and the consumer's promise (it applies an effect iff its capability is in the effective
//! set) are pinned here so a change to either side fails this test in the same CI job. The M2 re-run
//! of ID-D5 against the LIVE `EffectApi` is P-ID-23; this pair is the M1 `delegation` CDC.

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

/// The PROVIDER: the store-backed `delegation` surface (the algebra needs no seeded tuples for the
/// pure three-set composition — the object-check conjunct is the separate four-conjunct path).
fn provider() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

/// The CONSUMER: an `EffectApi`-side caller. It asks the provider for the effective policy, then
/// applies the proposed effect ONLY if the effect's required capability is inside that set. Returns
/// `true` iff the effect would be applied (the agent is confined to the intersection).
fn effect_api_apply(effective: &EffectivePolicy, required_capability: &str) -> bool {
    // The Agent Fabric never re-implements the algebra (contract 4.5): it consumes the composed
    // EffectivePolicy and tests membership. An effect whose capability is outside the effective set
    // is refused (fail-closed — the agent cannot wield authority no conjunct granted).
    authority_of(effective).holds(required_capability)
}

/// **The 4.5 happy path: an effect inside the intersection is applied.** The agent ceiling, the
/// delegation, and the tenant guardrails all grant `#read`, and the delegator holds it → `#read` is
/// in the effective set → the `EffectApi` consumer applies the effect.
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
    // The only grant surviving every conjunct is repo:acme/web#read.
    assert_eq!(effective.caveats, vec!["repo:acme/web#read".to_string()]);
    assert!(
        effect_api_apply(&effective, "repo:acme/web#read"),
        "an effect whose capability is in the effective set is applied"
    );
}

/// **The 4.5 fail-closed path: an effect OUTSIDE the intersection is refused (the structural
/// floor).** `#write` is in the agent ceiling and the tenant policy, but NOT in the delegation chain
/// → it is not in the effective set → the `EffectApi` consumer refuses the effect. The agent cannot
/// exceed what was delegated.
#[test]
fn cdc_4_5_effect_outside_intersection_refused() {
    let svc = provider();
    let input = DelegationInput {
        agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
        delegation: auth(&["repo:acme/web#read"]), // the delegation withheld #write
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

/// **The 4.5 provider promise: the effective set is a SUBSET of every conjunct (monotone, never a
/// superset).** The proof the provider records witnesses the intersection — the property the
/// consumer relies on (it can trust the effective set never exceeds any conjunct). This pins the
/// provider/consumer contract: the consumer's membership test is only safe BECAUSE the provider
/// guarantees the monotone law.
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
    // effective = {a,b,c} ∩ ({b,c,d} ∩ {b,c,d}) ∩ {c,d,e} = {c}
    assert_eq!(effective.caveats, vec!["c".to_string()]);
    assert!(
        proof.holds(),
        "the provider witnesses the monotone intersection (effective ⊆ every conjunct)"
    );
    // The consumer can safely test membership: nothing outside the conjuncts is ever returned.
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
