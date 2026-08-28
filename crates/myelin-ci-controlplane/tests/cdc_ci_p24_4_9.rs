use myelin_ci_controlplane::secret_broker::{
    SecretBroker, SecretCapability, SecretOutcome, WithholdReason, SECRET_READ_PERMISSION,
};
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

struct RecordingId {
    last: RefCell<Option<(String, String)>>,
}
impl IdentityService for RecordingId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn check(
        &self,
        _s: &Principal,
        p: &Permission,
        o: &ArtifactRef,
        _at: &Consistency,
        _cav: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        *self.last.borrow_mut() = Some((o.0.clone(), p.0.clone()));
        if o.0 == "myelin://acme/ci/secret/granted" {
            Ok(Decision::Allow)
        } else {
            Ok(Decision::Deny)
        }
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _pre: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("cdc stub"))
    }
}

fn at() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::Strong,
    }
}

struct ResolvingCap;
impl SecretCapability for ResolvingCap {
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
            .then(|| zeroize::Zeroizing::new(format!("material:{handle}")))
    }
}

fn secret_object_of(r: &SecretRef) -> ArtifactRef {
    ArtifactRef(r.handle.clone())
}

fn subject(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    )
}

#[test]
fn cdc_4_9_broker_gates_secret_read_and_fork_short_circuits() {
    let cap = ResolvingCap;
    let id = RecordingId {
        last: RefCell::new(None),
    };
    let broker = SecretBroker::new(&cap, &id);

    let refs = vec![
        SecretRef {
            name: "GRANTED".into(),
            handle: "myelin://acme/ci/secret/granted".into(),
        },
        SecretRef {
            name: "DENIED".into(),
            handle: "myelin://acme/ci/secret/denied".into(),
        },
    ];

    let trusted = broker
        .resolve(
            TrustTier::Trusted,
            &subject("u:member"),
            secret_object_of,
            &refs,
            &at(),
        )
        .expect("resolves");
    assert_eq!(
        trusted.secret_count(),
        1,
        "only the DIRECT-granted name resolves (4.9 DIRECT NARROW)"
    );
    let (_obj, perm) = id.last.borrow().clone().expect("a check was made");
    assert_eq!(perm, SECRET_READ_PERMISSION);
    assert_eq!(
        SECRET_READ_PERMISSION, "read",
        "the FROZEN §5.2 secret.read permission (4.9)"
    );

    let fork = broker
        .resolve(
            TrustTier::UntrustedFork,
            &subject("u:fork"),
            secret_object_of,
            &refs,
            &at(),
        )
        .expect("resolves");
    assert_eq!(
        fork.secret_count(),
        0,
        "the !is_untrusted_fork arm: 0 fork secret reads (4.9)"
    );
    assert!(fork.outcomes.iter().all(|o| matches!(
        o,
        SecretOutcome::Withheld {
            reason: WithholdReason::UntrustedFork,
            ..
        }
    )));
}
