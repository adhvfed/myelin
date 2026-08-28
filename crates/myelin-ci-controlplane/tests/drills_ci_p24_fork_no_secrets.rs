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

struct AlwaysResolves;
impl SecretCapability for AlwaysResolves {
    fn resolve_handle(
        &self,
        tenant: &TenantId,
        object: &ArtifactRef,
        _binding_name: &str,
        handle: &str,
    ) -> Option<zeroize::Zeroizing<String>> {
        let expected_prefix = format!("myelin://{}/ci/secret/", tenant.0);
        let id = handle.strip_prefix(&expected_prefix)?;
        (!id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            && object.0 == handle)
            .then(|| zeroize::Zeroizing::new(format!("PROD-SECRET-MATERIAL:{handle}")))
    }
}

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

#[test]
fn ci_d7_fork_gets_no_secrets() {
    let cap = AlwaysResolves;
    let id = MisconfiguredGrantAll {
        reads: RefCell::new(0),
    };
    let broker = SecretBroker::new(&cap, &id);

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

    let fork_res = broker
        .resolve(
            TrustTier::UntrustedFork,
            &fork_subject,
            secret_object_of,
            &protected_secrets,
            &at,
        )
        .expect("resolution does not error (a fork resolution is a clean 0, not a panic)");

    let fork_secret_reads = fork_res.secret_count();
    assert_eq!(fork_secret_reads, 0, "CI-D7 gate: 0 fork secret reads");
    assert!(fork_res.is_empty());
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
    let fork_authz_reads = *id.reads.borrow();
    assert_eq!(
        fork_authz_reads, 0,
        "the fork short-circuited BEFORE any authz check - a misconfigured grant cannot leak"
    );

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
