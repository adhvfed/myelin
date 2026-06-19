//! # `delegation` — the monotone-intersection delegation algebra (P-ID-17 → P-075)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §6 (the delegation / on-behalf-of algebra — CONFIRMED, Phase 3 §6/§7, AG-2): an agent's
//! effective authority is the **monotone intersection**
//! `effective = agent.policy ∩ delegation ∩ tenant.policy`, computed as **attenuation never
//! amplification** over the macaroon/biscuit caveat chains. The **four conjuncts** (the agent's
//! ceiling; the delegating human's grant carried as the per-run token's caveat chain; the tenant
//! guardrails; the ordinary object `check` run as the agent principal) and the **"you cannot
//! delegate authority you do not have"** re-check at mint are all unchanged from Phase 3 §7. The
//! `delegation(agent, trigger_actor) → EffectivePolicy` contract returns the composed decision so
//! the Agent Fabric (M2) never re-implements the algebra. This is the security floor that makes
//! "an agent can do things no human role can" **structurally impossible** (EI-02 §2).
//!
//! **Contract-index:** row 4.5 `delegation(agent, trigger_actor) → EffectivePolicy`
//! (`agent.policy ∩ delegation ∩ tenant.policy`, monotone intersection, macaroon caveats) — OWNED
//! here. Row 4.2 `check` (the ordinary object-check conjunct, run AS the agent principal) —
//! CONSUMED.
//!
//! ## What this module ships (P-ID-17 — the algebra, NOT the token mint)
//! [`DelegationAlgebra::delegation`] composes the three policy conjuncts as a SET INTERSECTION over
//! the [`crate::machine_auth::Authority`] caveat sets (reusing the SAME monotone
//! [`crate::machine_auth::Authority::attenuate`] primitive `authenticate` uses — EI-01 §7, one
//! primitive, no bespoke intersection path) and returns the frozen
//! [`myelin_identity::EffectivePolicy`] carrier so the Agent Fabric's `EffectApi` (P-ID-23, M2)
//! consumes the composed answer rather than re-deriving it.
//!
//! The token-MINTING half (`mint_run_token`, which stamps the effective policy into a per-run
//! attenuated token) is **P-ID-18 (P-076)**; this prompt ships the ALGEBRA the mint applies.
//!
//! ## The two load-bearing properties (mutation-tested mandatory-core, per the prompt GATE)
//! - **The composition is MONOTONE — adding a conjunct never GROWS authority** (architecture §6,
//!   the macaroon/biscuit law). Each conjunct is intersected in, so the effective set is a subset of
//!   every conjunct: `effective ⊆ agent.policy`, `effective ⊆ delegation`, `effective ⊆
//!   tenant.policy`. A mutation that turned an intersection (`∩`) into a union (`∪`) — so a conjunct
//!   could ADD authority — MUST be caught (the [`mutation`] guard formalises this as an assertable
//!   invariant the drill re-checks).
//! - **You cannot delegate authority you do not have** (architecture §6/§7, the re-check at mint).
//!   The delegated grant the trigger actor carries is first **attenuated by the trigger actor's OWN
//!   held authority** — so a delegator delegating a grant they never held passes nothing through,
//!   and a delegator whose grant was REVOKED (their held authority shrank) shrinks the agent's
//!   effective authority accordingly (the ID-D5 adversarial case: a delegator who lost the right).
//!
//! ## Floors named
//! **None new** — the algebra is complete in M1. ID-D5 **re-runs in M2 against the live `EffectApi`**
//! (P-ID-23): there, the same composed [`EffectivePolicy`] gates a real plan-then-apply effect. The
//! fourth conjunct (the ordinary object `check` run as the agent principal) is wired here as the
//! optional [`DelegationAlgebra::delegation_with_check`] runtime gate over the SAME [`CheckEngine`]
//! the platform calls; the pure-algebra [`DelegationAlgebra::delegation`] composes the three policy
//! sets (the conjunct the Agent Fabric carries forward).

use crate::check_engine::CheckEngine;
use crate::machine_auth::Authority;
use myelin_identity::{
    Consistency, Decision, EffectivePolicy, Permission, Principal, RelName,
};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;

/// The grant-string prefix the `EffectivePolicy` caveat carrier uses to round-trip an
/// [`Authority`] grant set through the frozen `Vec<String>` carrier. The frozen
/// [`myelin_identity::EffectivePolicy`] carries `caveats: Vec<String>`; the effective grant set is
/// projected into that list verbatim (sorted, deduplicated — the [`Authority`] set order). No
/// prefix is added; the carrier IS the sorted grant list. (The constant documents the convention.)
pub const EFFECTIVE_GRANT_CARRIER: &str = "grant";

/// **The four conjuncts of the delegation algebra (architecture §6/§7).** Named so the composition
/// reads as the architecture states it (`agent.policy ∩ delegation ∩ tenant.policy`, + the object
/// check run as the agent). The first three are policy SETS intersected; the fourth is the runtime
/// object `check` ([`DelegationAlgebra::delegation_with_check`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationInput {
    /// **Conjunct 1 — the agent's ceiling.** The maximum authority the agent principal may EVER
    /// wield, independent of who triggered it (the agent's own policy). The effective set can never
    /// exceed this.
    pub agent_policy: Authority,
    /// **Conjunct 2 — the delegation.** The delegating human's grant, carried as the per-run token's
    /// caveat chain (what the trigger actor is delegating to this run). Re-checked against conjunct 4
    /// below.
    pub delegation: Authority,
    /// **Conjunct 3 — the tenant guardrails.** The tenant-wide policy ceiling (what ANY principal in
    /// the tenant may do — the tenant's own guardrails). The effective set can never exceed this.
    pub tenant_policy: Authority,
    /// **The mint-time re-check input — the trigger actor's OWN held authority.** "You cannot
    /// delegate authority you do not have" (architecture §6/§7): the `delegation` (conjunct 2) is
    /// first intersected with this, so a delegator can only pass through grants they themselves
    /// hold. When the delegator's grant is REVOKED (this set shrinks), the agent's effective
    /// authority shrinks with it (the ID-D5 adversarial case).
    pub trigger_actor_held: Authority,
}

/// **The recorded proof that the effective set is the monotone intersection (the ID-D5 green
/// artifact, EI-01 §3 "prove it").** Every [`DelegationAlgebra::delegation`] call can emit this —
/// the four conjunct sets, the composed effective set, and the verified post-conditions (the
/// effective set is a subset of every conjunct; it never exceeds any conjunct). The drill records it
/// per adversarial case as the dated green artifact (the intersection proof the prompt GATE names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntersectionProof {
    /// Conjunct 1 — the agent ceiling (sorted grant list).
    pub agent_policy: Vec<String>,
    /// Conjunct 2 — the delegation, AFTER the "cannot delegate what you don't have" re-check
    /// (`delegation ∩ trigger_actor_held`).
    pub delegated_after_recheck: Vec<String>,
    /// Conjunct 3 — the tenant guardrails (sorted grant list).
    pub tenant_policy: Vec<String>,
    /// The composed effective set (`agent ∩ delegated_after_recheck ∩ tenant`).
    pub effective: Vec<String>,
    /// The verified post-condition: the effective set is a subset of EVERY conjunct (the monotone
    /// law — adding a conjunct never grew authority). `true` is the only passing value; a `false`
    /// here is the loud red the drill aborts on (never a silent allow).
    pub subset_of_every_conjunct: bool,
}

impl IntersectionProof {
    /// Does this proof witness the monotone intersection (every post-condition held)?
    pub fn holds(&self) -> bool {
        self.subset_of_every_conjunct
    }
}

/// **The monotone-intersection delegation algebra (contract 4.5; architecture §6/§7).** Composes
/// `agent.policy ∩ delegation ∩ tenant.policy` over the [`Authority`] caveat sets, with the
/// "you cannot delegate authority you do not have" re-check, and returns the frozen
/// [`EffectivePolicy`] so the Agent Fabric (P-ID-23, M2) never re-implements the algebra.
///
/// Holds an optional [`CheckEngine`] for the fourth conjunct (the ordinary object `check` run as the
/// agent principal) — [`DelegationAlgebra::delegation_with_check`]. The pure three-set composition
/// ([`DelegationAlgebra::delegation`]) needs no store and is the conjunct the Agent Fabric carries
/// forward into its plan-then-apply pipeline.
#[derive(Clone)]
pub struct DelegationAlgebra {
    /// The depth-bounded `check` engine for the optional object-check conjunct (run AS the agent).
    /// `None` for the pure-algebra surface (the three-set composition needs no tuple store).
    engine: Option<CheckEngine>,
}

impl Default for DelegationAlgebra {
    fn default() -> Self {
        DelegationAlgebra::new()
    }
}

impl DelegationAlgebra {
    /// The pure-algebra surface — composes the three policy sets with no object-check conjunct (the
    /// answer the Agent Fabric carries forward; the object check is run per-effect at apply time).
    pub fn new() -> DelegationAlgebra {
        DelegationAlgebra { engine: None }
    }

    /// The algebra WITH the object-check conjunct wired over the SAME [`CheckEngine`] the platform's
    /// `check` (P-ID-09) uses — so the fourth conjunct (the ordinary object check, run as the agent
    /// principal) composes through one primitive, never a bespoke per-delegation check path.
    pub fn with_check(engine: CheckEngine) -> DelegationAlgebra {
        DelegationAlgebra {
            engine: Some(engine),
        }
    }

    /// **`delegation(agent, trigger_actor) → EffectivePolicy` (contract 4.5) — the three-set monotone
    /// intersection.** Returns the frozen [`EffectivePolicy`] whose `caveats` carry the composed
    /// effective grant set (sorted, deduplicated). The composition:
    ///
    /// 1. **The mint-time re-check** — `delegated = delegation ∩ trigger_actor_held` ("you cannot
    ///    delegate authority you do not have"). A grant the trigger actor never held is dropped here.
    /// 2. **The monotone intersection** — `effective = agent.policy ∩ delegated ∩ tenant.policy`,
    ///    each step the SAME [`Authority::attenuate`] (set intersection) `authenticate` uses. Each
    ///    conjunct can only narrow; none can amplify.
    ///
    /// The `agent`/`trigger_actor` [`Principal`]s are threaded for the signature the contract froze
    /// (4.5) and so a caller cannot pass mismatched policy/principal pairs by accident; the policy
    /// sets themselves come from `input` (the caveat chains the credentials carry — resolved by
    /// `authenticate`, P-ID-07). The pure ABI [`myelin_identity::IdentityService::delegation`] slot
    /// delegates to this with the policies looked up for its two principals.
    pub fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
        input: &DelegationInput,
    ) -> EffectivePolicy {
        let (effective, _proof) = self.compose(input);
        effective_policy_of(&effective)
    }

    /// **The algebra WITH the recorded [`IntersectionProof`] (the ID-D5 green artifact).** Identical
    /// composition to [`DelegationAlgebra::delegation`], additionally returning the proof the drill
    /// records (the four conjunct sets + the composed effective set + the verified monotone
    /// post-condition). EI-01 §3: the property does not exist until a test forces it AND observability
    /// records it — the proof IS that recorded observation.
    pub fn delegation_proved(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
        input: &DelegationInput,
    ) -> (EffectivePolicy, IntersectionProof) {
        let (effective, proof) = self.compose(input);
        (effective_policy_of(&effective), proof)
    }

    /// **The four-conjunct decision: the three-set policy intersection AND the ordinary object
    /// `check` run AS the agent principal (architecture §6, conjunct 4).** Returns `Allow` ONLY when
    /// BOTH the composed effective policy holds `required_grant` AND the agent principal passes the
    /// object `check` for `permission` on `object` (fail-closed: a missing engine, a `Deny`, or a
    /// `Conditional` all refuse). This is the conjunct the Agent Fabric's `EffectApi` runs at apply
    /// time (P-ID-23, M2); wired here so ID-D5 can exercise it end-to-end over the real check.
    ///
    /// - `required_grant` is the capability the proposed effect needs (e.g. `"repo:acme/web#write"`).
    ///   A grant outside the effective intersection denies (the agent cannot exceed the composed
    ///   policy) — this is the structural floor: an agent confined to `agent ∩ delegation ∩ tenant`.
    /// - The object `check` is the ordinary fail-closed Zanzibar evaluation run as the AGENT (not the
    ///   delegator) — so the agent must ALSO hold the object-level relation, not just the capability.
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
        // (a) The capability conjunct: the proposed effect's grant must be inside the composed
        //     effective policy (agent ∩ delegation ∩ tenant, after the re-check). A grant outside the
        //     intersection is refused — the agent cannot wield authority no conjunct granted.
        let (effective, _proof) = self.compose(input);
        if !effective.holds(required_grant) {
            return Decision::Deny;
        }
        let _ = trigger_actor; // the trigger actor's grant already folded into `delegation` (re-check).

        // (b) The fourth conjunct: the ordinary object check, run AS THE AGENT principal. Without a
        //     wired engine the object conjunct cannot be evaluated → fail-closed Deny (never an open
        //     fall-through on a missing dependency).
        let engine = match &self.engine {
            Some(e) => e,
            None => return Decision::Deny,
        };
        // The permission is checked against the object as a relation through the SAME depth-bounded
        // userset rewrite the platform's `check` uses (one primitive). A non-Allow is fail-closed.
        let relation = RelName(permission.0.clone());
        engine.check(scope, agent, &relation, object, at, None)
    }

    /// The shared composition core (the one place the `∩` lives — EI-01 §7, one primitive). Computes
    /// the re-checked delegation and the monotone three-set intersection, and assembles the
    /// [`IntersectionProof`] (the post-condition verified, never assumed).
    fn compose(&self, input: &DelegationInput) -> (Authority, IntersectionProof) {
        // (1) The mint-time re-check: "you cannot delegate authority you do not have". The delegation
        //     is intersected with the trigger actor's OWN held authority — a grant the delegator
        //     never held (or lost) passes nothing. SAME monotone `attenuate` (set intersection).
        let delegated = input.delegation.attenuate(&input.trigger_actor_held);

        // (2) The monotone intersection: agent.policy ∩ delegated ∩ tenant.policy. Each `attenuate`
        //     is a set intersection — a conjunct can only NARROW, never amplify. Order is irrelevant
        //     (intersection is commutative + associative); we fold agent → delegated → tenant.
        let effective = input
            .agent_policy
            .attenuate(&delegated)
            .attenuate(&input.tenant_policy);

        // (3) The verified post-condition (the monotone law made assertable): the effective set is a
        //     SUBSET of every conjunct. This is computed, not assumed — if a future mutation turned a
        //     `∩` into a `∪`, the effective set would exceed some conjunct and this flips to false.
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

/// Project an [`Authority`] grant set into the frozen [`EffectivePolicy`] carrier (`caveats:
/// Vec<String>`). The grant set is already sorted + deduplicated (it is a `BTreeSet`), so the carrier
/// is the deterministic sorted grant list — the SAME bytes for the same effective set (so a CDC
/// consumer comparing two `EffectivePolicy`s by value is stable).
pub fn effective_policy_of(authority: &Authority) -> EffectivePolicy {
    EffectivePolicy {
        caveats: authority.grants().map(str::to_string).collect(),
    }
}

/// Read an [`Authority`] back out of an [`EffectivePolicy`] carrier (the inverse of
/// [`effective_policy_of`]) — so a consumer (the Agent Fabric `EffectApi`) can re-test membership of
/// the composed grant set without re-deriving the algebra.
pub fn authority_of(policy: &EffectivePolicy) -> Authority {
    Authority::of(policy.caveats.iter().cloned())
}

/// **The mutation-floor guard (the prompt's mutation floor made an assertable invariant).** The
/// delegation algebra's intersection is mandatory-core: a mutation that turned `∩` into `∪` MUST be
/// caught. This module exposes the property as a pure function over arbitrary conjunct sets so the
/// unit test + the drill both assert it on the SAME code path: for ANY conjunct sets, the composed
/// effective set is a SUBSET of every conjunct (an intersection), never a superset (a union would
/// fail this for some input).
pub mod mutation {
    use super::*;

    /// For arbitrary conjunct sets, the composed effective set is a subset of every conjunct (the
    /// monotone-intersection invariant). Returns `true` iff the composition is a true intersection.
    /// A `∩→∪` mutation would make some input return `false` (a union can exceed a conjunct).
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

    /// **The intersection is monotone: adding a conjunct never grows authority (architecture §6).**
    /// Start from the agent ceiling; each conjunct intersected in only narrows. The effective set is
    /// a subset of EVERY conjunct.
    #[test]
    fn intersection_is_monotone_never_grows() {
        let algebra = DelegationAlgebra::new();
        let inp = input(
            &["a", "b", "c", "d"], // agent ceiling
            &["a", "b", "c"],      // delegation
            &["b", "c", "d"],      // tenant guardrails
            &["a", "b", "c"],      // trigger actor holds what they delegate
        );
        let (eff, proof) = algebra.delegation_proved(&agent_principal("p:agent"), &human("p:human"), &inp);
        // effective = {a,b,c,d} ∩ ({a,b,c} ∩ {a,b,c}) ∩ {b,c,d} = {b,c}
        assert_eq!(eff.caveats, vec!["b".to_string(), "c".to_string()]);
        assert!(proof.holds(), "the effective set is a subset of every conjunct (monotone)");

        // Adding a TIGHTER tenant conjunct only narrows further (never grows).
        let tighter = input(&["a", "b", "c", "d"], &["a", "b", "c"], &["b"], &["a", "b", "c"]);
        let eff2 = algebra.delegation(&agent_principal("p:agent"), &human("p:human"), &tighter);
        assert_eq!(eff2.caveats, vec!["b".to_string()], "a tighter conjunct narrows, never grows");
        assert!(
            authority_of(&eff2).is_subset_of(&authority_of(&eff)),
            "adding/tightening a conjunct yields a SUBSET (monotone — never amplifies)"
        );
    }

    /// **A token's authority attenuates correctly through a caveat chain (architecture §4/§6).** The
    /// delegation conjunct is itself an attenuated chain; composing it through the algebra keeps the
    /// monotone law — the effective set never exceeds any link.
    #[test]
    fn authority_attenuates_through_the_caveat_chain() {
        let algebra = DelegationAlgebra::new();
        // The delegator holds a broad authority; the delegation caveat narrows it; the agent ceiling
        // narrows again; the tenant guardrail narrows again. Each link is an intersection.
        let inp = input(
            &["repo:acme/web#read", "repo:acme/web#write"], // agent ceiling
            &["repo:acme/web#read", "repo:acme/api#read"],  // delegated chain link
            &["repo:acme/web#read", "repo:acme/web#write", "repo:acme/api#read"], // tenant
            &["repo:acme/web#read", "repo:acme/api#read"],  // delegator holds the chain
        );
        let eff = algebra.delegation(&agent_principal("p:agent"), &human("p:human"), &inp);
        // The ONLY grant surviving every link is repo:acme/web#read.
        assert_eq!(eff.caveats, vec!["repo:acme/web#read".to_string()]);
        assert!(
            authority_of(&eff).is_subset_of(&auth(&["repo:acme/web#read", "repo:acme/web#write"])),
            "the effective set never exceeds the agent ceiling (a chain link)"
        );
    }

    /// **Revoking the delegator's grant shrinks the agent's effective authority (the ID-D5 core,
    /// architecture §6 — "you cannot delegate authority you do not have").** With the delegator
    /// holding `#write`, the agent's effective set includes `#write`; once the delegator's `#write`
    /// is revoked (their held set shrinks), the agent's effective set shrinks too — even though the
    /// delegation chain and the agent ceiling still NAME `#write`.
    #[test]
    fn revoking_delegators_grant_shrinks_agent_authority() {
        let algebra = DelegationAlgebra::new();
        let agent = agent_principal("p:agent");
        let delegator = human("p:human");

        // Before revocation: the delegator HOLDS both grants → both flow through.
        let before = input(
            &["repo:acme/web#read", "repo:acme/web#write"], // agent ceiling names both
            &["repo:acme/web#read", "repo:acme/web#write"], // delegation names both
            &["repo:acme/web#read", "repo:acme/web#write"], // tenant allows both
            &["repo:acme/web#read", "repo:acme/web#write"], // delegator HOLDS both
        );
        let eff_before = algebra.delegation(&agent, &delegator, &before);
        assert!(
            authority_of(&eff_before).holds("repo:acme/web#write"),
            "while the delegator holds #write, the agent's effective set includes it"
        );

        // After revocation: the delegator's HELD authority lost #write (revoked). The delegation
        // chain and the agent ceiling STILL name #write, but the re-check drops it.
        let after = input(
            &["repo:acme/web#read", "repo:acme/web#write"], // agent ceiling unchanged
            &["repo:acme/web#read", "repo:acme/web#write"], // delegation chain unchanged
            &["repo:acme/web#read", "repo:acme/web#write"], // tenant unchanged
            &["repo:acme/web#read"],                        // delegator HELD set shrank (#write revoked)
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

    /// **You cannot delegate authority you do not have: a delegator delegating a grant they never
    /// held passes nothing (architecture §6/§7, the re-check at mint).** The delegation NAMES a
    /// grant, but the delegator's held set does not contain it → it is dropped before composition.
    #[test]
    fn cannot_delegate_authority_you_never_held() {
        let algebra = DelegationAlgebra::new();
        let inp = input(
            &["repo:acme/web#admin"], // agent ceiling names admin
            &["repo:acme/web#admin"], // the delegation TRIES to grant admin
            &["repo:acme/web#admin"], // tenant allows admin
            &["repo:acme/web#read"],  // but the delegator never HELD admin
        );
        let (eff, proof) = algebra.delegation_proved(&agent_principal("p:agent"), &human("p:human"), &inp);
        assert!(eff.caveats.is_empty(), "a grant the delegator never held is never delegated");
        assert!(proof.holds());
        assert!(
            proof.delegated_after_recheck.is_empty(),
            "the re-check dropped the un-held grant before composition"
        );
    }

    /// **The mutation-floor guard: the composition is an INTERSECTION (not a union) for arbitrary
    /// conjunct sets (the prompt's mutation floor).** This is the property a `∩→∪` mutation breaks:
    /// for some input a union would let the effective set EXCEED a conjunct. The guard asserts the
    /// subset-of-every-conjunct invariant across a matrix of cases.
    #[test]
    fn mutation_floor_composition_is_intersection_not_union() {
        // (agent_policy, delegation, tenant_policy, trigger_actor_held) grant lists.
        type ConjunctCase<'a> = (&'a [&'a str], &'a [&'a str], &'a [&'a str], &'a [&'a str]);
        let cases: &[ConjunctCase] = &[
            (&["a", "b"], &["a", "b", "c"], &["a", "b"], &["a", "b", "c"]),
            (&["a", "b", "c"], &["b"], &["a", "b", "c"], &["b"]),
            (&[], &["a"], &["a"], &["a"]),       // empty agent ceiling ⇒ empty effective
            (&["a"], &[], &["a"], &["a"]),       // empty delegation ⇒ empty effective
            (&["a"], &["a"], &[], &["a"]),       // empty tenant guardrail ⇒ empty effective
            (&["a"], &["a"], &["a"], &[]),       // delegator holds nothing ⇒ empty effective
            (&["x", "y"], &["x", "y"], &["x", "y"], &["x", "y"]), // all equal ⇒ {x,y}
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
        // And a sanity case proving the effective set is exactly the intersection, not a union.
        let algebra = DelegationAlgebra::new();
        let inp = input(&["a", "b"], &["b", "c"], &["b", "d"], &["b", "c"]);
        let eff = algebra.delegation(&agent_principal("p:agent"), &human("p:human"), &inp);
        assert_eq!(eff.caveats, vec!["b".to_string()], "the effective set is the intersection {{b}}");
    }

    /// **The frozen `EffectivePolicy` carrier round-trips (the contract-4.5 wire shape).** The
    /// effective grant set projects into `EffectivePolicy{caveats}` deterministically (sorted), and
    /// reads back as the same authority.
    #[test]
    fn effective_policy_carrier_round_trips() {
        let a = auth(&["c:3", "a:1", "b:2"]);
        let policy = effective_policy_of(&a);
        // The carrier is the sorted grant list (BTreeSet order) — deterministic bytes.
        assert_eq!(policy.caveats, vec!["a:1".to_string(), "b:2".to_string(), "c:3".to_string()]);
        assert_eq!(authority_of(&policy), a, "the carrier round-trips to the same authority");
        assert_eq!(EFFECTIVE_GRANT_CARRIER, "grant");
    }

    /// **The fourth conjunct: the object `check` run AS the agent (architecture §6, conjunct 4).**
    /// `delegation_with_check` returns `Allow` ONLY when the effective policy holds the grant AND the
    /// agent passes the object check; a grant outside the intersection, OR a failed object check,
    /// both deny (fail-closed).
    #[test]
    fn fourth_conjunct_object_check_run_as_agent() {
        // Seed a tuple store: the agent has the `write` relation on repo:core.
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
        // The effective policy holds the capability grant.
        let inp = input(
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
        );

        // Grant inside the intersection AND the agent passes the object check → Allow.
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
        assert_eq!(d, Decision::Allow, "grant in the intersection + object check pass ⇒ Allow");

        // A grant OUTSIDE the effective intersection → Deny (the agent cannot exceed the composed policy).
        let d_cap = algebra.delegation_with_check(
            &agent,
            &delegator,
            &inp,
            &scope,
            "repo:acme/web#admin", // not in the effective set
            &Permission("write".into()),
            &obj,
            &at,
        );
        assert_eq!(d_cap, Decision::Deny, "a grant outside the intersection is refused");

        // The capability holds, but the object check fails (the agent has no `delete` relation) → Deny.
        let d_obj = algebra.delegation_with_check(
            &agent,
            &delegator,
            &inp,
            &scope,
            "repo:acme/web#write",
            &Permission("delete".into()), // the agent has no `delete` tuple on repo:core
            &obj,
            &at,
        );
        assert_eq!(d_obj, Decision::Deny, "a failed object check refuses (fail-closed)");
    }

    /// **Without a wired engine, the four-conjunct check fails closed.** The pure-algebra surface has
    /// no object-check engine; asking it for the four-conjunct decision denies (never an open
    /// fall-through on a missing dependency).
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
        assert_eq!(d, Decision::Deny, "no object-check engine ⇒ fail-closed Deny");
    }
}
