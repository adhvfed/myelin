use myelin_events::OutboxStore;
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_identity_service::{
    authority_of, Authority, DelegationInput, IntersectionProof, StoreBackedCheck, TupleStore,
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

fn svc() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

struct AdversarialCase {
    name: &'static str,
    input: DelegationInput,
    effects: Vec<(&'static str, bool)>,
}

#[test]
fn id_d5_adversarial_delegation_zero_escapes_with_intersection_proof() {
    let svc = svc();
    let agent = agent("p:agent");
    let delegator = human("p:human");

    let cases = vec![
        AdversarialCase {
            name: "agent-ceiling-overreach",
            input: DelegationInput {
                agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#admin"]),
                delegation: auth(&["repo:acme/web#read"]),
                tenant_policy: auth(&["repo:acme/web#read"]),
                trigger_actor_held: auth(&["repo:acme/web#read"]),
            },
            effects: vec![
                ("repo:acme/web#read", true),
                ("repo:acme/web#admin", false),
            ],
        },
        AdversarialCase {
            name: "tenant-guardrail-forbids",
            input: DelegationInput {
                agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                delegation: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                tenant_policy: auth(&["repo:acme/web#read"]),
                trigger_actor_held: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
            },
            effects: vec![
                ("repo:acme/web#read", true),
                ("repo:acme/web#write", false),
            ],
        },
        AdversarialCase {
            name: "delegator-lost-the-right",
            input: DelegationInput {
                agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                delegation: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                tenant_policy: auth(&["repo:acme/web#read", "repo:acme/web#write"]),
                trigger_actor_held: auth(&["repo:acme/web#read"]),
            },
            effects: vec![
                ("repo:acme/web#read", true),
                ("repo:acme/web#write", false),
            ],
        },
        AdversarialCase {
            name: "delegate-a-grant-never-held",
            input: DelegationInput {
                agent_policy: auth(&["repo:acme/web#read", "repo:acme/web#deploy"]),
                delegation: auth(&["repo:acme/web#read", "repo:acme/web#deploy"]),
                tenant_policy: auth(&["repo:acme/web#read", "repo:acme/web#deploy"]),
                trigger_actor_held: auth(&["repo:acme/web#read"]),
            },
            effects: vec![
                ("repo:acme/web#read", true),
                ("repo:acme/web#deploy", false),
            ],
        },
    ];

    let mut escape_count: i64 = 0;
    let mut proofs: Vec<(&'static str, IntersectionProof)> = Vec::new();

    for case in &cases {
        let (effective, proof) = svc.delegation_proved_in(&agent, &delegator, &case.input);

        assert!(
            proof.holds(),
            "case {}: the intersection proof must witness effective ⊆ every conjunct",
            case.name
        );

        for (capability, expected_inside) in &case.effects {
            let admitted = authority_of(&effective).holds(capability);
            if admitted != *expected_inside {
                if admitted && !*expected_inside {
                    escape_count += 1;
                } else {
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

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::CrossTenantCount, escape_count);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        escape_count, 0,
        "0 effects outside the intersection (the ID-D5 F9 floor)"
    );

    assert_eq!(
        proofs.len(),
        cases.len(),
        "an intersection proof was recorded for every adversarial case"
    );
    for (name, proof) in &proofs {
        assert!(
            proof.holds(),
            "case {name}: the recorded intersection proof holds"
        );
    }

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
