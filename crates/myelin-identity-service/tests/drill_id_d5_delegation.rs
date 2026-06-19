//! # P-ID-17 (global P-075) GATE / DRILL — ID-D5, the adversarial-delegation intersection drill
//! (the dated green artifact)
//!
//! **Drill catalogue row ID-D5 (§4.2, F9):** *Adversarial delegation: an agent is confined to
//! `agent.policy ∩ delegation ∩ tenant.policy`, including via a delegator who lost the right.*
//! Survival signals: a **denial counter** (the count of effects that escaped the intersection — must
//! be **0**) and an **intersection proof** (the recorded witness that the effective set is the
//! intersection, never a superset of any conjunct). Run against the failure-injection harness's
//! telemetry-assertion library (the contract-1.8 survival-signal set), exactly as the cross-tenant
//! IDOR drill (`drill_id_d3`) and the disabled-user drill (`drill_id_d1`) do. `myelin-harness` is a
//! DEV-dependency only — it never enters the identity-service production DAG.
//!
//! **Quantified threshold (EI-01 §3 — prove it, never weaken):** **0 effects outside the
//! intersection**, and the **intersection proof emitted for each adversarial case** (the green
//! artifact is the recorded proof that the effective set is the intersection). A single escaping
//! effect (an agent wielding authority no conjunct granted) is the F9 failure and the drill aborts
//! LOUDLY (`expect_green` panics; the threshold is NEVER softened to pass).
//!
//! **The scenario.** An agent run is triggered on behalf of a delegating human. The drill drives an
//! ADVERSARIAL CORPUS of effects the agent might attempt — each naming a capability that is granted
//! by SOME but not ALL conjuncts (the classic over-grant attempts), plus the headline ID-D5 case: a
//! capability the delegating human ONCE held but has since LOST (revoked). For every attempted
//! effect, the agent is allowed to apply it ONLY if its capability is inside the composed effective
//! policy `agent ∩ delegation ∩ tenant` (with the "you cannot delegate authority you do not have"
//! re-check). The drill counts every effect that ESCAPED the intersection (must be 0) and records the
//! intersection proof for the delegation. A non-zero escape count means an agent did something no
//! conjunct authorised (the exact F9 failure) and the drill aborts.
//!
//! The M2 re-run of ID-D5 against the LIVE Agent-Fabric `EffectApi` is P-ID-23 (there the same
//! composed `EffectivePolicy` gates a real plan-then-apply effect); this drill proves the algebra in
//! isolation at M1.

use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_identity_service::{
    authority_of, Authority, DelegationInput, IntersectionProof, StoreBackedCheck, TupleStore,
};
use myelin_events::OutboxStore;
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

/// The provider surface (the algebra needs no seeded tuples for the three-set composition).
fn svc() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

/// One adversarial case: a delegation scenario + the corpus of effects the agent attempts, each with
/// whether it SHOULD be inside the intersection (the ground truth the drill checks the algebra
/// against). An effect the algebra admits that ground-truth says is OUTSIDE the intersection is an
/// escape (the F9 failure).
struct AdversarialCase {
    name: &'static str,
    input: DelegationInput,
    /// (capability, expected_inside_intersection)
    effects: Vec<(&'static str, bool)>,
}

/// **ID-D5 — adversarial delegation: 0 effects escape `agent ∩ delegation ∩ tenant`, incl. via a
/// delegator who lost the right.**
#[test]
fn id_d5_adversarial_delegation_zero_escapes_with_intersection_proof() {
    let svc = svc();
    let agent = agent("p:agent");
    let delegator = human("p:human");

    let cases = vec![
        // Case A — the agent ceiling over-reaches: the agent's policy names #admin, but neither the
        // delegation nor the tenant granted it → #admin must be OUTSIDE the intersection.
        AdversarialCase {
            name: "agent-ceiling-overreach",
            input: DelegationInput {
                agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#admin"]),
                delegation: auth(&["repo:acme/web#read"]),
                tenant_policy: auth(&["repo:acme/web#read"]),
                trigger_actor_held: auth(&["repo:acme/web#read"]),
            },
            effects: vec![
                ("repo:acme/web#read", true),   // granted by every conjunct → inside
                ("repo:acme/web#admin", false), // only the agent ceiling → MUST be refused
            ],
        },
        // Case B — the tenant guardrail forbids: the agent + delegation both grant #write, but the
        // tenant policy does NOT → #write must be OUTSIDE the intersection (tenant guardrails win).
        AdversarialCase {
            name: "tenant-guardrail-forbids",
            input: DelegationInput {
                agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                delegation: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                tenant_policy: auth(&["repo:acme/web#read"]), // the tenant forbids #write
                trigger_actor_held: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
            },
            effects: vec![
                ("repo:acme/web#read", true),
                ("repo:acme/web#write", false), // tenant-forbidden → MUST be refused
            ],
        },
        // Case C — THE HEADLINE: a delegator who LOST the right. The agent ceiling, the delegation
        // chain, and the tenant policy ALL still name #write, but the delegator's HELD set lost it
        // (revoked) → the re-check drops #write → it MUST be OUTSIDE the intersection.
        AdversarialCase {
            name: "delegator-lost-the-right",
            input: DelegationInput {
                agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                delegation: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                tenant_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                trigger_actor_held: auth(&["repo:acme/web#read"]), // #write was revoked from the delegator
            },
            effects: vec![
                ("repo:acme/web#read", true),
                ("repo:acme/web#write", false), // delegator no longer holds it → MUST be refused
            ],
        },
        // Case D — the delegator delegates a grant they NEVER held (a forged delegation): every
        // conjunct names #deploy, but the delegator's held set never had it → refused.
        AdversarialCase {
            name: "delegate-a-grant-never-held",
            input: DelegationInput {
                agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#deploy"]),
                delegation: auth(&["repo:acme/web#read", "repo:acme/web#deploy"]),
                tenant_policy: auth(&["repo:acme/web#read", "repo:acme/web#deploy"]),
                trigger_actor_held: auth(&["repo:acme/web#read"]), // never held #deploy
            },
            effects: vec![
                ("repo:acme/web#read", true),
                ("repo:acme/web#deploy", false), // never held by the delegator → MUST be refused
            ],
        },
    ];

    // THE DRILL: for every case, compose the effective policy + record the intersection proof, then
    // drive the corpus of effects. Count every effect that ESCAPED the intersection (the algebra
    // admitted a capability ground-truth says is outside) — must be 0.
    let mut escape_count: i64 = 0;
    let mut proofs: Vec<(&'static str, IntersectionProof)> = Vec::new();

    for case in &cases {
        let (effective, proof) = svc.delegation_proved_in(&agent, &delegator, &case.input);

        // The intersection proof is the green artifact (EI-01 §3): the recorded witness that the
        // effective set is a SUBSET of every conjunct (never a superset). A proof that does NOT hold
        // is itself the F9 failure (the composition was not a true intersection).
        assert!(
            proof.holds(),
            "case {}: the intersection proof must witness effective ⊆ every conjunct",
            case.name
        );

        // Drive the adversarial corpus: the agent attempts each effect; it is admitted iff its
        // capability is inside the effective set (the EffectApi consumer shape, contract 4.5).
        for (capability, expected_inside) in &case.effects {
            let admitted = authority_of(&effective).holds(capability);
            if admitted != *expected_inside {
                if admitted && !*expected_inside {
                    // The exact F9 failure: an effect the agent should NOT be able to wield was
                    // admitted (it escaped the intersection).
                    escape_count += 1;
                } else {
                    // A capability ground-truth says is inside was refused — also a correctness
                    // failure (the algebra is too tight); abort loudly.
                    panic!(
                        "case {}: capability `{capability}` should be INSIDE the intersection but \
                         was refused (the algebra wrongly narrowed)",
                        case.name
                    );
                }
            }
        }
        proofs.push((case.name, proof));
    }

    // THE green artifacts, asserted through the harness telemetry-assertion library (loud on red):
    // (1) the denial counter: 0 effects escaped the intersection (CrossTenantCount = the generic
    //     zero-violation counter the Id drills use for "0 unauthorised escapes").
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::CrossTenantCount, escape_count);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(escape_count, 0, "0 effects outside the intersection (the ID-D5 F9 floor)");

    // (2) the intersection proof emitted for EACH adversarial case (the recorded green artifact).
    assert_eq!(
        proofs.len(),
        cases.len(),
        "an intersection proof was recorded for every adversarial case"
    );
    for (name, proof) in &proofs {
        assert!(proof.holds(), "case {name}: the recorded intersection proof holds");
    }

    // The dated green artifact (the intersection proof, printed so the gate's evidence is in the CI
    // log — observability is part of the pass, EI-01 §3).
    println!(
        "[P-075 DRILL GREEN 2026-06-19] ID-D5 adversarial delegation → \
         agent.policy ∩ delegation ∩ tenant.policy: tenant=acme agent=p:agent delegator=p:human \
         cases={} effects_attempted={} escapes={escape_count} (0 effects outside the intersection)",
        cases.len(),
        cases.iter().map(|c| c.effects.len()).sum::<usize>(),
    );
    for (name, proof) in &proofs {
        println!(
            "  intersection_proof[{name}]: agent={:?} delegated_after_recheck={:?} tenant={:?} \
             => effective={:?} subset_of_every_conjunct={}",
            proof.agent_policy,
            proof.delegated_after_recheck,
            proof.tenant_policy,
            proof.effective,
            proof.subset_of_every_conjunct,
        );
    }
}
