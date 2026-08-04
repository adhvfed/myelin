use crate::principal_store::PrincipalStore;
use myelin_identity::{
    iam_events::signals, AuthzError, CaveatContext, Consistency, Credential, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition,
    Principal, PrincipalId, PrincipalStatus, RevokeTarget, RewriteTrace, RunId, RunToken,
    SubjectTree, TupleDelta, Zookie,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub mod scheme {
    pub const OIDC: &str = "oidc";
    pub const SAML: &str = "saml";
    pub const SCIM: &str = "scim";
    pub const PASSKEY: &str = "passkey";
    pub const SSH: &str = "ssh";

    pub const HUMAN_SSO_SCHEMES: &[&str] = &[OIDC, SAML, SCIM, PASSKEY, SSH];

    pub fn is_human_sso(s: &str) -> bool {
        HUMAN_SSO_SCHEMES.contains(&s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAssertion {
    pub tenant: TenantId,
    pub region: Region,
    pub scheme: String,
    pub subject_key: String,
    pub expires_at_unix: Option<i64>,
}

pub trait CredentialVerifier: Send + Sync {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RefuseUnsupportedVerifier;

impl RefuseUnsupportedVerifier {
    pub fn new() -> RefuseUnsupportedVerifier {
        RefuseUnsupportedVerifier
    }
}

impl CredentialVerifier for RefuseUnsupportedVerifier {
    fn verify(&self, _credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        Err(AuthzError::NotYetImplemented(
            "credential scheme has no production-wired cryptographic verifier yet (refuse-not-mock, \
             MR-012) - a real verifier (OIDC JWKS / SAML XML-DSig / WebAuthn attestation / SSH \
             challenge) must be config-wired via SchemeDispatchVerifier::route; SCIM is a \
             provisioning seam, not an auth credential. The mock StructuralVerifier is a #[cfg(test)] \
             double and is NEVER the production fallback.",
        ))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StructuralVerifier;

impl StructuralVerifier {
    pub fn new() -> StructuralVerifier {
        StructuralVerifier
    }
}

impl CredentialVerifier for StructuralVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        if !scheme::is_human_sso(&credential.scheme) {
            return Err(AuthzError::BadRequest(format!(
                "scheme `{}` is not a v1 human/SSO surface (oidc/saml/scim/passkey/ssh); the \
                 capability-token + machine-identity surfaces are P-ID-07",
                credential.scheme
            )));
        }
        let parts: Vec<&str> = credential.material.split('|').collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
            return Err(AuthzError::BadRequest(
                "malformed verified-assertion envelope (expected `<tenant>|<region>|<subject_key>`, \
                 all non-empty)"
                    .into(),
            ));
        }
        Ok(VerifiedAssertion {
            tenant: TenantId(parts[0].to_string()),
            region: Region(parts[1].to_string()),
            scheme: credential.scheme.clone(),
            subject_key: parts[2].to_string(),
            expires_at_unix: None,
        })
    }
}

#[derive(Debug, Default)]
pub struct AuthTelemetry {
    count: AtomicU64,
}

impl AuthTelemetry {
    pub fn new() -> AuthTelemetry {
        AuthTelemetry {
            count: AtomicU64::new(0),
        }
    }

    pub const SIGNAL: &'static str = signals::AUTH_DECISION_LATENCY;

    pub(crate) fn observe(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decision_count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
pub struct IdorCounters {
    path_derived_tenant_count: AtomicU64,
    attempted_path_mismatch_count: AtomicU64,
}

impl IdorCounters {
    pub fn new() -> IdorCounters {
        IdorCounters::default()
    }

    pub fn path_derived_tenant_count(&self) -> u64 {
        self.path_derived_tenant_count.load(Ordering::Relaxed)
    }

    pub fn attempted_path_mismatch_count(&self) -> u64 {
        self.attempted_path_mismatch_count.load(Ordering::Relaxed)
    }

    pub(crate) fn count_path_derived(&self) {
        self.path_derived_tenant_count
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn count_attempted_path_mismatch(&self) {
        self.attempted_path_mismatch_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub struct HumanSsoAuthenticator {
    store: PrincipalStore,
    verifier: Arc<dyn CredentialVerifier>,
    telemetry: Arc<AuthTelemetry>,
    idor: Arc<IdorCounters>,
}

impl HumanSsoAuthenticator {
    #[cfg(test)]
    pub fn new(store: PrincipalStore) -> HumanSsoAuthenticator {
        HumanSsoAuthenticator::with_verifier(store, Arc::new(StructuralVerifier::new()))
    }

    pub fn production(store: PrincipalStore) -> HumanSsoAuthenticator {
        HumanSsoAuthenticator::production_with_oidc_replay(store, None)
    }

    pub fn production_with_oidc(
        store: PrincipalStore,
        oidc: (crate::oidc::OidcConfig, crate::oidc::JwkSet),
        replay: crate::oidc::ReplayGuard,
    ) -> HumanSsoAuthenticator {
        let (config, jwks) = oidc;
        HumanSsoAuthenticator::production_with_oidc_replay(store, Some((config, jwks, replay)))
    }

    pub fn production_with_oidc_refresh(
        store: PrincipalStore,
        oidc: (crate::oidc::OidcConfig, crate::oidc::JwkSet),
        replay: crate::oidc::ReplayGuard,
        refresh: impl Fn() -> Result<crate::oidc::JwkSet, AuthzError> + Send + Sync + 'static,
    ) -> HumanSsoAuthenticator {
        use crate::oidc::OidcVerifier;

        let (config, jwks) = oidc;
        let verifier = OidcVerifier::new(config, jwks)
            .with_replay_guard(replay)
            .with_jwks_refresh(refresh);
        HumanSsoAuthenticator::production_with_oidc_verifier(store, verifier)
    }

    fn production_with_oidc_verifier(
        store: PrincipalStore,
        verifier: crate::oidc::OidcVerifier,
    ) -> HumanSsoAuthenticator {
        use crate::oidc::SchemeDispatchVerifier;

        let dispatch = SchemeDispatchVerifier::new(Arc::new(RefuseUnsupportedVerifier::new()))
            .route(scheme::OIDC, Arc::new(verifier));
        HumanSsoAuthenticator::with_verifier(store, Arc::new(dispatch))
    }

    fn production_with_oidc_replay(
        store: PrincipalStore,
        oidc: Option<(
            crate::oidc::OidcConfig,
            crate::oidc::JwkSet,
            crate::oidc::ReplayGuard,
        )>,
    ) -> HumanSsoAuthenticator {
        use crate::oidc::{OidcVerifier, SchemeDispatchVerifier};
        let mut dispatch = SchemeDispatchVerifier::new(Arc::new(RefuseUnsupportedVerifier::new()));
        if let Some((config, jwks, replay)) = oidc {
            let verifier = OidcVerifier::new(config, jwks).with_replay_guard(replay);
            dispatch = dispatch.route(scheme::OIDC, Arc::new(verifier));
        }
        HumanSsoAuthenticator::with_verifier(store, Arc::new(dispatch))
    }

    pub fn with_verifier(
        store: PrincipalStore,
        verifier: Arc<dyn CredentialVerifier>,
    ) -> HumanSsoAuthenticator {
        HumanSsoAuthenticator {
            store,
            verifier,
            telemetry: Arc::new(AuthTelemetry::new()),
            idor: Arc::new(IdorCounters::new()),
        }
    }

    pub fn telemetry(&self) -> &AuthTelemetry {
        &self.telemetry
    }

    pub fn idor_counters(&self) -> &IdorCounters {
        &self.idor
    }

    pub fn authenticate(
        &self,
        credential: &Credential,
        path_tenant: Option<&TenantId>,
    ) -> myelin_identity::Result<Principal> {
        self.authenticate_with_assertion(credential, path_tenant)
            .map(|(principal, _)| principal)
    }

    pub fn authenticate_with_assertion(
        &self,
        credential: &Credential,
        path_tenant: Option<&TenantId>,
    ) -> myelin_identity::Result<(Principal, VerifiedAssertion)> {
        self.telemetry.observe();

        let assertion = self.verifier.verify(credential)?;

        let scope = self.scope_for(&assertion);
        let resolved = scope.resolve(path_tenant);
        debug_assert_eq!(
            resolved.tenant, assertion.tenant,
            "the effective tenant must be the verified credential's (ID-3)"
        );
        if resolved.path_derived {
            self.idor
                .path_derived_tenant_count
                .fetch_add(1, Ordering::Relaxed);
        }
        if resolved.attempted_path_mismatch {
            self.idor
                .attempted_path_mismatch_count
                .fetch_add(1, Ordering::Relaxed);
        }

        let row = self
            .store
            .try_resolve_credential(&scope, &assertion.scheme, &assertion.subject_key)
            .map_err(|e| {
                AuthzError::FailClosed(format!(
                    "identity directory lookup failed for verified `{}` credential - fail-closed: {e}",
                    assertion.scheme
                ))
            })?
            .ok_or_else(|| {
                AuthzError::FailClosed(format!(
                    "no `{}` principal mapped for the verified subject in tenant `{}` (unknown \
                     credential - fail-closed, never a fabricated session)",
                    assertion.scheme, assertion.tenant.0
                ))
            })?;

        match row.status {
            PrincipalStatus::Active => {}
            PrincipalStatus::Suspended | PrincipalStatus::Disabled => {
                return Err(AuthzError::FailClosed(format!(
                    "principal `{}` is {:?} (SCIM-deprovisioned / suspended) - authenticate \
                     fail-closes (it never resolves to an active session); full revocation is \
                     P-ID-14",
                    row.principal_id.0, row.status
                )));
            }
        }

        let principal = Principal::new(
            assertion.tenant.clone(),
            assertion.region.clone(),
            row.principal_id,
            row.kind,
            row.data_role,
            row.status,
        );
        Ok((principal, assertion))
    }

    pub fn authenticate_trait(
        &self,
        credential: &Credential,
    ) -> myelin_identity::Result<Principal> {
        self.authenticate(credential, None)
    }

    fn scope_for(&self, assertion: &VerifiedAssertion) -> TenantScope {
        let token = Principal::stub(
            myelin_identity::PrincipalId(format!("cred:{}", assertion.subject_key)),
            myelin_identity::PrincipalKind::Human,
            assertion.tenant.clone(),
        );
        TenantScope::from_verified_token(&token, assertion.region.clone())
    }
}

impl IdentityService for HumanSsoAuthenticator {
    fn authenticate(&self, credential: &Credential) -> myelin_identity::Result<Principal> {
        self.authenticate_trait(credential)
    }

    fn check(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        Ok(Decision::Deny)
    }

    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented(
            "list_objects → P-ID-11/12 (M1)",
        ))
    }

    fn list_subjects(
        &self,
        _object: &ObjectId,
        _permission: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<SubjectTree> {
        Err(AuthzError::NotYetImplemented(
            "list_subjects → P-ID-13 (M1)",
        ))
    }

    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("explain → P-ID-13 (M1)"))
    }

    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("delegation → P-ID-17 (M1)"))
    }

    fn write_tuples(
        &self,
        _deltas: &[TupleDelta],
        _precondition: Option<&Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        Err(AuthzError::NotYetImplemented("write_tuples → P-ID-08 (M1)"))
    }

    fn mint_run_token(
        &self,
        _agent_id: &PrincipalId,
        _run_id: &RunId,
        _delegation_caveats: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> myelin_identity::Result<RunToken> {
        Err(AuthzError::NotYetImplemented(
            "mint_run_token → P-ID-18 (M1)",
        ))
    }

    fn revoke(&self, _target: &RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("revoke → P-ID-14 (M1)"))
    }

    fn resolve_pseudonym(
        &self,
        _subject: &PrincipalId,
        _tenant: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented(
            "resolve_pseudonym → P-ID-19 (M1)",
        ))
    }

    fn erase(&self, _subject: &PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("erase → P-ID-20 (M1)"))
    }

    fn admit_fragment(
        &self,
        _fragment: &NamespaceFragment,
    ) -> myelin_identity::Result<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented(
            "admit_fragment → P-ID-10 (M1)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalKind};
    use myelin_storage::KmsEngine;

    fn store() -> PrincipalStore {
        PrincipalStore::new(Arc::new(KmsEngine::new()))
    }

    fn scope(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    fn material(tenant: &str, region: &str, subject_key: &str) -> String {
        format!("{tenant}|{region}|{subject_key}")
    }

    fn seeded(
        scheme: &str,
        tenant: &str,
        region: &str,
        subject_key: &str,
        principal_id: &str,
        kind: PrincipalKind,
        status: PrincipalStatus,
    ) -> HumanSsoAuthenticator {
        let st = store();
        let sc = scope(tenant, region);
        st.put_principal(
            &sc,
            PrincipalId(principal_id.into()),
            kind,
            DataRole::Processor,
            status,
            None,
        )
        .unwrap();
        st.link_credential(&sc, scheme, subject_key, &PrincipalId(principal_id.into()))
            .unwrap();
        HumanSsoAuthenticator::new(st)
    }

    #[test]
    fn each_v1_human_sso_scheme_resolves_to_its_principal() {
        for s in scheme::HUMAN_SSO_SCHEMES {
            let auth = seeded(
                s,
                "acme",
                "eu-west",
                "subj-1",
                "p:alice",
                PrincipalKind::Human,
                PrincipalStatus::Active,
            );
            let p = auth
                .authenticate(
                    &Credential {
                        scheme: (*s).into(),
                        material: material("acme", "eu-west", "subj-1"),
                    },
                    None,
                )
                .unwrap_or_else(|e| panic!("scheme `{s}` should resolve: {e:?}"));
            assert_eq!(p.principal_id, PrincipalId("p:alice".into()), "scheme {s}");
            assert_eq!(
                p.tenant,
                TenantId("acme".into()),
                "scheme {s} tenant from credential"
            );
            assert_eq!(p.region, Region("eu-west".into()), "scheme {s} region");
            assert_eq!(p.kind, PrincipalKind::Human, "scheme {s} polymorphic kind");
        }
    }

    #[test]
    fn ssh_can_resolve_a_service_kind_principal() {
        let auth = seeded(
            scheme::SSH,
            "acme",
            "eu-west",
            "SHA256:deadbeef",
            "svc:deploy",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        let p = auth
            .authenticate(
                &Credential {
                    scheme: scheme::SSH.into(),
                    material: material("acme", "eu-west", "SHA256:deadbeef"),
                },
                None,
            )
            .unwrap();
        assert_eq!(p.kind, PrincipalKind::Service);
    }

    #[test]
    fn tenant_is_from_credential_not_the_url_path() {
        let auth = seeded(
            scheme::OIDC,
            "acme",
            "eu-west",
            "subj-1",
            "p:alice",
            PrincipalKind::Human,
            PrincipalStatus::Active,
        );
        let p = auth
            .authenticate(
                &Credential {
                    scheme: scheme::OIDC.into(),
                    material: material("acme", "eu-west", "subj-1"),
                },
                Some(&TenantId("globex".into())),
            )
            .unwrap();
        assert_eq!(
            p.tenant,
            TenantId("acme".into()),
            "the resolved tenant is the CREDENTIAL's (acme), never the path's (globex)"
        );
        assert_eq!(
            auth.idor_counters().path_derived_tenant_count(),
            0,
            "path_derived_tenant_count == 0 (the IDOR floor - tenant never from the path)"
        );
        assert_eq!(
            auth.idor_counters().attempted_path_mismatch_count(),
            1,
            "the rejected IDOR attempt (path ≠ credential) was counted (the guard held)"
        );
    }

    #[test]
    fn credential_for_one_tenant_cannot_resolve_another_tenants_principal() {
        let st = store();
        let acme = scope("acme", "eu-west");
        st.put_principal(
            &acme,
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            DataRole::Processor,
            PrincipalStatus::Active,
            None,
        )
        .unwrap();
        st.link_credential(
            &acme,
            scheme::OIDC,
            "subj-1",
            &PrincipalId("p:alice".into()),
        )
        .unwrap();
        let auth = HumanSsoAuthenticator::new(st);

        let r = auth.authenticate(
            &Credential {
                scheme: scheme::OIDC.into(),
                material: material("globex", "eu-west", "subj-1"),
            },
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a globex-verified credential cannot resolve acme's principal (no cross-tenant resolve)"
        );
    }

    #[test]
    fn disabled_principal_fails_closed() {
        for status in [PrincipalStatus::Disabled, PrincipalStatus::Suspended] {
            let auth = seeded(
                scheme::SCIM,
                "acme",
                "eu-west",
                "ext-7",
                "p:bob",
                PrincipalKind::Human,
                status,
            );
            let r = auth.authenticate(
                &Credential {
                    scheme: scheme::SCIM.into(),
                    material: material("acme", "eu-west", "ext-7"),
                },
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::FailClosed(_))),
                "a {status:?} principal fails closed (never an active session)"
            );
        }
    }

    #[test]
    fn auth_decision_latency_emits_once_per_request_on_every_path() {
        let auth = seeded(
            scheme::PASSKEY,
            "acme",
            "eu-west",
            "cred-id-9",
            "p:carol",
            PrincipalKind::Human,
            PrincipalStatus::Active,
        );
        assert_eq!(auth.telemetry().decision_count(), 0);
        auth.authenticate(
            &Credential {
                scheme: scheme::PASSKEY.into(),
                material: material("acme", "eu-west", "cred-id-9"),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            auth.telemetry().decision_count(),
            1,
            "success emits one observation"
        );
        let _ = auth.authenticate(
            &Credential {
                scheme: scheme::PASSKEY.into(),
                material: material("acme", "eu-west", "no-such-subject"),
            },
            None,
        );
        assert_eq!(
            auth.telemetry().decision_count(),
            2,
            "a failed decision ALSO emits one observation (signal on every path)"
        );
        assert_eq!(AuthTelemetry::SIGNAL, "auth_decision_latency");
        assert_eq!(AuthTelemetry::SIGNAL, signals::AUTH_DECISION_LATENCY);
    }

    #[test]
    fn capability_token_scheme_is_refused_here() {
        let auth = HumanSsoAuthenticator::new(store());
        for s in ["pat", "ci", "agent", "deploy_key"] {
            let r = auth.authenticate(
                &Credential {
                    scheme: s.into(),
                    material: material("acme", "eu-west", "x"),
                },
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::BadRequest(_))),
                "scheme `{s}` is P-ID-07's (machine identity), refused by the human/SSO body"
            );
        }
    }

    #[test]
    fn malformed_assertion_is_refused_loudly() {
        let auth = HumanSsoAuthenticator::new(store());
        for bad in ["", "acme", "acme|eu-west", "acme||subj", "|eu-west|subj"] {
            let r = auth.authenticate(
                &Credential {
                    scheme: scheme::OIDC.into(),
                    material: bad.into(),
                },
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::BadRequest(_))),
                "malformed envelope `{bad}` is refused"
            );
        }
        assert_eq!(
            auth.telemetry().decision_count(),
            5,
            "every refused decision still emitted its observation"
        );
    }

    #[test]
    fn unknown_credential_fails_closed() {
        let auth = HumanSsoAuthenticator::new(store());
        let r = auth.authenticate(
            &Credential {
                scheme: scheme::SAML.into(),
                material: material("acme", "eu-west", "ghost"),
            },
            None,
        );
        assert!(matches!(r, Err(AuthzError::FailClosed(_))));
    }

    #[test]
    fn production_default_refuses_forged_credential_never_mocks() {
        let auth = HumanSsoAuthenticator::production(store());
        for s in scheme::HUMAN_SSO_SCHEMES {
            let r = auth.authenticate(
                &Credential {
                    scheme: (*s).into(),
                    material: material("acme", "eu-west", "subj-1"),
                },
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::NotYetImplemented(_))),
                "the production default must REFUSE scheme `{s}` (refuse-not-mock), not resolve a \
                 forged plaintext envelope through the mock StructuralVerifier"
            );
        }
        let scim = auth.authenticate(
            &Credential {
                scheme: scheme::SCIM.into(),
                material: material("acme", "eu-west", "ext-7"),
            },
            None,
        );
        assert!(matches!(scim, Err(AuthzError::NotYetImplemented(_))));
    }
}
