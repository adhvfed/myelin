//! # CI-D7 drill — fork-gets-no-secrets (CI-P24 / P-367).
//!
//! The whole-system drill (drill catalogue row CI-D7): an **adversarial `UntrustedFork` run** attempts
//! to read protected secrets through the in-boundary broker → the **`read & !is_untrusted_fork` ABAC
//! edge holds STRUCTURALLY** → **0 secret reads by a fork-tier run**. The quantified gate:
//! `0 fork secret reads`.
//!
//! This is the canonical "fork exfiltrates prod secrets" CVE class (the poisoned-pipeline attack,
//! EI-02 §1). The failure-injection harness MAXIMISES the adversary's advantage:
//! - the fork references EVERY protected secret name (a full-spectrum exfil attempt);
//! - the authz layer is MISCONFIGURED to grant the fork's subject read on ALL of them (a
//!   defence-in-depth test: even a broken grant must not leak — the STRUCTURAL fork short-circuit is
//!   the boundary, not the authz check);
//! - the broker is also asked to mint an OIDC cloud credential (the registry-exfil vector).
//!
//! Every one is REFUSED: the fork resolves to 0 secrets, makes 0 authz reads, and gets no cloud
//! credential. Emits the dated green artifact on pass.

use myelin_ci_controlplane::secret_broker::{SecretBroker, SecretCapability, WithholdReason};
use myelin_ci_sandbox::{SecretRef, TrustTier};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition,
    Principal, PrincipalId, PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId,
    RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;

/// A capability that WOULD resolve every secret (the adversary's advantage maximised).
struct AlwaysResolves;
impl SecretCapability for AlwaysResolves {
    fn resolve_handle(
        &self,
        tenant: &TenantId,
        object: &ArtifactRef,
        handle: &str,
    ) -> Option<String> {
        let expected_prefix = format!("myelin://{}/ci/secret/", tenant.0);
        let id = handle.strip_prefix(&expected_prefix)?;
        (!id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            && object.0 == handle)
            .then(|| format!("PROD-SECRET-MATERIAL:{handle}"))
    }
}

/// An authz layer MISCONFIGURED to ALLOW the fork's subject `read` on every secret — and that records
/// every `check` it received, so the drill can prove the fork made ZERO authz reads (the structural
/// short-circuit fired BEFORE the broken grant was ever consulted).
struct MisconfiguredGrantAll {
    reads: RefCell<usize>,
}
impl IdentityService for MisconfiguredGrantAll {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        _cav: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        *self.reads.borrow_mut() += 1;
        // The MISCONFIGURATION: grant everything. The structural defence must survive this.
        Ok(Decision::Allow)
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _pre: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("drill stub"))
    }
}

fn secret_object_of(r: &SecretRef) -> ArtifactRef {
    ArtifactRef(r.handle.clone())
}

/// **CI-D7: 0 fork secret reads.** An adversarial fork references every protected secret; the authz
/// layer is misconfigured to grant them all; the broker STILL resolves ZERO and makes ZERO authz reads
/// (the `!is_untrusted_fork` arm by construction). A trusted control run with the SAME spec resolves
/// them (the defence is asymmetric — a wall against forks, not against members).
#[test]
fn ci_d7_fork_gets_no_secrets() {
    let cap = AlwaysResolves;
    let id = MisconfiguredGrantAll {
        reads: RefCell::new(0),
    };
    let broker = SecretBroker::new(&cap, &id);

    // The fork's full-spectrum exfil spec: every protected secret a prod deploy uses.
    let protected_secrets = vec![
        SecretRef {
            name: "PROD_DB_PASSWORD".into(),
            handle: "myelin://acme/ci/secret/db".into(),
        },
        SecretRef {
            name: "REGISTRY_PUSH_TOKEN".into(),
            handle: "myelin://acme/ci/secret/registry".into(),
        },
        SecretRef {
            name: "CLOUD_DEPLOY_KEY".into(),
            handle: "myelin://acme/ci/secret/cloud".into(),
        },
        SecretRef {
            name: "SIGNING_PRIVATE_KEY".into(),
            handle: "myelin://acme/ci/secret/signing".into(),
        },
    ];

    let fork_subject = Principal::stub(
        PrincipalId("u:attacker-fork".into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    );
    let at = Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::Strong,
    };

    // ---- the adversarial fork-tier resolution ----------------------------------------------------
    let fork_res = broker
        .resolve(
            TrustTier::UntrustedFork,
            &fork_subject,
            secret_object_of,
            &protected_secrets,
            &at,
        )
        .expect("resolution does not error (a fork resolution is a clean 0, not a panic)");

    // THE QUANTIFIED GATE: 0 secret reads by a fork-tier run.
    let fork_secret_reads = fork_res.secret_count();
    assert_eq!(fork_secret_reads, 0, "CI-D7 gate: 0 fork secret reads");
    assert!(fork_res.is_empty());
    // Every referenced name is withheld with the STRUCTURAL fork reason.
    for o in &fork_res.outcomes {
        assert!(o.resolved().is_none());
        assert!(matches!(
            o,
            myelin_ci_controlplane::secret_broker::SecretOutcome::Withheld {
                reason: WithholdReason::UntrustedFork,
                ..
            }
        ));
    }
    // THE STRUCTURAL PROOF: the fork made ZERO authz reads — the misconfigured grant was never even
    // consulted (the short-circuit fired first). This is why a broken grant cannot leak to a fork.
    let fork_authz_reads = *id.reads.borrow();
    assert_eq!(
        fork_authz_reads, 0,
        "the fork short-circuited BEFORE any authz check — a misconfigured grant cannot leak"
    );

    // The fork is ALSO refused an OIDC cloud credential (the registry-exfil vector).
    let mut oidc_mint_attempts = 0;
    let cred = broker.mint_oidc(
        TrustTier::UntrustedFork,
        "registry.fr-par",
        900,
        |aud, ttl| {
            oidc_mint_attempts += 1;
            Some(format!("oidc:{aud}:{ttl}"))
        },
    );
    assert!(
        cred.is_none(),
        "a fork gets NO audience-scoped cloud credential"
    );
    assert_eq!(
        oidc_mint_attempts, 0,
        "the fork never even reached the OIDC mint"
    );

    // ---- the asymmetry control: a TRUSTED member run with the SAME spec DOES resolve --------------
    // (proves the broker is a wall against forks, not a wall against everything — the secrets ARE
    // resolvable for a legitimately-granted member run.)
    let trusted_subject = Principal::stub(
        PrincipalId("u:trusted-member".into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    );
    let trusted_res = broker
        .resolve(
            TrustTier::Trusted,
            &trusted_subject,
            secret_object_of,
            &protected_secrets,
            &at,
        )
        .expect("a trusted resolution does not error");
    assert!(
        trusted_res.all_resolved(),
        "a TRUSTED run with the grant resolves the secrets (the defence is asymmetric)"
    );
    assert_eq!(trusted_res.secret_count(), 4);

    // ---- the dated green artifact (the prompt's "CI-D7 emits its dated green artifact") -----------
    println!(
        "[CI-D7 GREEN 2026-06-23] fork-gets-no-secrets: {} protected secrets referenced by an \
         adversarial fork; {} fork secret reads (gate: 0); {} fork authz reads (the structural \
         short-circuit fired BEFORE the misconfigured grant); fork OIDC credential refused. \
         A trusted control run resolved all {}. The `read & !is_untrusted_fork` ABAC edge held \
         STRUCTURALLY.",
        protected_secrets.len(),
        fork_secret_reads,
        fork_authz_reads,
        trusted_res.secret_count(),
    );
}

/// **The HITL companion gate (the prompt's second quantified gate): a double-click approval is ONE
/// approval; a declined effect is WITHHELD (returns Denied, never mutates).** Proven over the FROZEN
/// per-effect `idem_key` in `deployment::DeployGate`.
#[test]
fn ci_p24_hitl_double_click_is_one_apply_decline_withholds() {
    use myelin_ci_controlplane::deployment::{DeployGate, DeployGateOutcome, DECLINE_TOKEN};
    use myelin_flow::ApprovalDecision;
    use std::collections::HashMap;

    let mut applied: HashMap<String, String> = HashMap::new();
    let mut applies = 0;

    // A double-click on "approve" → ONE apply (the per-effect idem_key dedup).
    for _ in 0..2 {
        let o = DeployGate::gate_deploy(
            "deploy-card-prod",
            0,
            1,
            ApprovalDecision::Approve,
            &mut applied,
            || {
                applies += 1;
                "dep-prod-1".to_string()
            },
        );
        assert_eq!(o, DeployGateOutcome::Approved("dep-prod-1".into()));
    }
    assert_eq!(
        applies, 1,
        "a double-click is ONE approval (per-effect idem_key, OQ-F)"
    );

    // A declined deploy → WITHHELD, 0 mutation (AG-8).
    let mut decline_applies = 0;
    let declined = DeployGate::gate_deploy(
        "deploy-card-staging",
        0,
        1,
        ApprovalDecision::Decline,
        &mut applied,
        || {
            decline_applies += 1;
            "never".to_string()
        },
    );
    assert_eq!(declined, DeployGateOutcome::Withheld(DECLINE_TOKEN.into()));
    assert_eq!(
        decline_applies, 0,
        "a declined deploy makes 0 mutation (Denied, AG-8)"
    );

    println!(
        "[CI-P24 HITL GREEN 2026-06-23] per-effect idem_key: double-click=1 apply ({applies}); \
         declined deploy withheld with 0 mutation ({decline_applies} applies). The protected-env \
         gate withholds until approved."
    );
}
