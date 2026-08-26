use crate::authenticate::{scheme as human_scheme, AuthTelemetry, IdorCounters};
use crate::principal_store::PrincipalStore;
use crate::revocation::{RevocationStore, RunTokenState};
use myelin_events::Timestamp;
use myelin_identity::{
    AuthzError, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RevokeTarget,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeSet;
use std::sync::Arc;

type NowFn = Arc<dyn Fn() -> Timestamp + Send + Sync>;

pub mod scheme {
    pub const SESSION: &str = "session";
    pub const PAT: &str = "pat";
    pub const CI: &str = "ci";
    pub const AGENT: &str = "agent";
    pub const DEPLOY_KEY: &str = "deploy_key";
    pub const PER_JOB: &str = "per_job";

    pub const MACHINE_SCHEMES: &[&str] = &[SESSION, PAT, CI, AGENT, DEPLOY_KEY, PER_JOB];

    pub fn is_machine(s: &str) -> bool {
        MACHINE_SCHEMES.contains(&s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineKind {
    Pat,
    Ci,
    Agent,
    DeployKey,
    PerJob,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialPurpose {
    HumanSession,
    OperatorBootstrap,
    AgentRun {
        run_id: String,
        delegation_snapshot: Option<i64>,
    },
    Pat,
    CiJob {
        run_id: String,
    },
    DeployKey,
    PerJob {
        run_id: String,
    },
}

impl CredentialPurpose {
    pub fn claim(&self) -> &'static str {
        match self {
            CredentialPurpose::HumanSession => "human_session",
            CredentialPurpose::OperatorBootstrap => "operator_bootstrap",
            CredentialPurpose::AgentRun { .. } => "agent_run",
            CredentialPurpose::Pat => "pat",
            CredentialPurpose::CiJob { .. } => "ci_job",
            CredentialPurpose::DeployKey => "deploy_key",
            CredentialPurpose::PerJob { .. } => "per_job",
        }
    }

    pub fn machine_kind(&self) -> MachineKind {
        match self {
            CredentialPurpose::HumanSession
            | CredentialPurpose::OperatorBootstrap
            | CredentialPurpose::AgentRun { .. } => MachineKind::Agent,
            CredentialPurpose::Pat => MachineKind::Pat,
            CredentialPurpose::CiJob { .. } => MachineKind::Ci,
            CredentialPurpose::DeployKey => MachineKind::DeployKey,
            CredentialPurpose::PerJob { .. } => MachineKind::PerJob,
        }
    }

    pub fn is_agent_run(&self) -> bool {
        matches!(self, CredentialPurpose::AgentRun { .. })
    }

    pub fn is_run_scoped(&self) -> bool {
        matches!(
            self,
            CredentialPurpose::AgentRun { .. }
                | CredentialPurpose::CiJob { .. }
                | CredentialPurpose::PerJob { .. }
        )
    }

    pub fn run_id(&self) -> Option<&str> {
        match self {
            CredentialPurpose::AgentRun { run_id, .. }
            | CredentialPurpose::CiJob { run_id }
            | CredentialPurpose::PerJob { run_id } => Some(run_id),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialAudience {
    Edge,
    Mcp,
}

impl CredentialAudience {
    pub fn claim(self) -> &'static str {
        match self {
            CredentialAudience::Edge => "edge",
            CredentialAudience::Mcp => "mcp",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpopState {
    Unbound,
    Verified,
}

impl MachineKind {
    pub fn from_scheme(s: &str) -> Option<MachineKind> {
        match s {
            scheme::SESSION => Some(MachineKind::Agent),
            scheme::PAT => Some(MachineKind::Pat),
            scheme::CI => Some(MachineKind::Ci),
            scheme::AGENT => Some(MachineKind::Agent),
            scheme::DEPLOY_KEY => Some(MachineKind::DeployKey),
            scheme::PER_JOB => Some(MachineKind::PerJob),
            _ => None,
        }
    }

    pub fn is_self_hosted_runner(self) -> bool {
        matches!(self, MachineKind::PerJob)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Authority {
    grants: BTreeSet<String>,
}

impl Authority {
    pub fn of<I, S>(grants: I) -> Authority
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Authority {
            grants: grants.into_iter().map(Into::into).collect(),
        }
    }

    pub fn grants(&self) -> impl Iterator<Item = &str> {
        self.grants.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.grants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    pub fn holds(&self, grant: &str) -> bool {
        self.grants.contains(grant)
    }

    pub fn attenuate(&self, requested: &Authority) -> Authority {
        Authority {
            grants: self
                .grants
                .intersection(&requested.grants)
                .cloned()
                .collect(),
        }
    }

    pub fn is_subset_of(&self, parent: &Authority) -> bool {
        self.grants.is_subset(&parent.grants)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityToken {
    pub tenant: TenantId,
    pub region: Region,
    pub kind: MachineKind,
    pub subject_key: String,
    pub authority: Authority,
    pub jti: String,
    pub dpop_bound: bool,
    pub purpose: CredentialPurpose,
    pub audience: CredentialAudience,
    pub exp_unix: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCapabilityContext {
    pub purpose: CredentialPurpose,
    pub audience: CredentialAudience,
    pub jti: String,
    pub effective_authority: Authority,
    pub expires_at_unix: i64,
    pub dpop: DpopState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialContext {
    Capability(VerifiedCapabilityContext),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestIdentity {
    pub principal: Principal,
    pub scope: TenantScope,
    pub credential: CredentialContext,
}

impl RequestIdentity {
    pub fn capability(&self) -> &VerifiedCapabilityContext {
        match &self.credential {
            CredentialContext::Capability(capability) => capability,
        }
    }
}

pub trait TokenVerifier: Send + Sync {
    fn verify(
        &self,
        credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<CapabilityToken>;

    fn verify_for_request(
        &self,
        credential: &myelin_identity::Credential,
        binding: &crate::capability_crypto::DpopBinding,
    ) -> myelin_identity::Result<CapabilityToken>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StructuralTokenVerifier;

impl StructuralTokenVerifier {
    pub fn new() -> StructuralTokenVerifier {
        StructuralTokenVerifier
    }
}

impl TokenVerifier for StructuralTokenVerifier {
    fn verify(
        &self,
        credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<CapabilityToken> {
        let kind = MachineKind::from_scheme(&credential.scheme).ok_or_else(|| {
            if human_scheme::is_human_sso(&credential.scheme) {
                AuthzError::BadRequest(format!(
                    "scheme `{}` is a v1 human/SSO surface (P-ID-06), not a capability-token / \
                     machine-identity surface (session/pat/ci/agent/deploy_key/per_job)",
                    credential.scheme
                ))
            } else {
                AuthzError::BadRequest(format!(
                    "scheme `{}` is not a capability-token / machine-identity surface \
                     (session/pat/ci/agent/deploy_key/per_job)",
                    credential.scheme
                ))
            }
        })?;

        let parts: Vec<&str> = credential.material.split('|').collect();
        if parts.len() != 10 {
            return Err(AuthzError::BadRequest(
                "malformed verified-token envelope (expected \
                 `<tenant>|<region>|<subject_key>|<jti>|<dpop:0|1>|<grants>|<purpose>|<aud>|<run_id>|<delegation_snapshot>`)"
                    .into(),
            ));
        }
        let (
            tenant,
            region,
            subject_key,
            jti,
            dpop,
            grants_csv,
            purpose,
            audience,
            run_id,
            delegation_snapshot,
        ) = (
            parts[0], parts[1], parts[2], parts[3], parts[4], parts[5], parts[6], parts[7],
            parts[8], parts[9],
        );
        if tenant.is_empty() || region.is_empty() || subject_key.is_empty() || jti.is_empty() {
            return Err(AuthzError::BadRequest(
                "malformed verified-token envelope: tenant/region/subject_key/jti must be non-empty"
                    .into(),
            ));
        }
        let dpop_bound = match dpop {
            "1" => true,
            "0" => false,
            other => {
                return Err(AuthzError::BadRequest(format!(
                    "malformed DPoP flag `{other}` (expected `0` or `1`)"
                )))
            }
        };
        let authority = if grants_csv.is_empty() {
            Authority::default()
        } else {
            Authority::of(grants_csv.split(',').map(str::to_string))
        };
        let purpose = match purpose {
            "human_session" => CredentialPurpose::HumanSession,
            "operator_bootstrap" => CredentialPurpose::OperatorBootstrap,
            "agent_run" if !run_id.is_empty() => CredentialPurpose::AgentRun {
                run_id: run_id.to_string(),
                delegation_snapshot: if delegation_snapshot.is_empty() {
                    None
                } else {
                    Some(delegation_snapshot.parse::<i64>().map_err(|_| {
                        AuthzError::BadRequest(
                            "signed delegation snapshot must be an integer".into(),
                        )
                    })?)
                },
            },
            "pat" => CredentialPurpose::Pat,
            "ci_job" if !run_id.is_empty() => CredentialPurpose::CiJob {
                run_id: run_id.to_string(),
            },
            "deploy_key" => CredentialPurpose::DeployKey,
            "per_job" if !run_id.is_empty() => CredentialPurpose::PerJob {
                run_id: run_id.to_string(),
            },
            "test_kind" => match kind {
                MachineKind::Pat => CredentialPurpose::Pat,
                MachineKind::Ci => CredentialPurpose::CiJob {
                    run_id: subject_key.to_string(),
                },
                MachineKind::Agent => CredentialPurpose::OperatorBootstrap,
                MachineKind::DeployKey => CredentialPurpose::DeployKey,
                MachineKind::PerJob => CredentialPurpose::PerJob {
                    run_id: subject_key.to_string(),
                },
            },
            other => {
                return Err(AuthzError::BadRequest(format!(
                    "unknown or incomplete signed credential purpose `{other}`"
                )))
            }
        };
        if purpose.machine_kind() != kind {
            return Err(AuthzError::FailClosed(format!(
                "credential scheme kind `{kind:?}` does not match signed purpose `{}`",
                purpose.claim()
            )));
        }
        if (credential.scheme == scheme::SESSION)
            != matches!(purpose, CredentialPurpose::HumanSession)
        {
            return Err(AuthzError::FailClosed(
                "the `session` scheme and signed `human_session` purpose must be used together"
                    .into(),
            ));
        }
        let audience = match audience {
            "edge" => CredentialAudience::Edge,
            "mcp" => CredentialAudience::Mcp,
            other => {
                return Err(AuthzError::BadRequest(format!(
                    "unknown signed credential audience `{other}`"
                )))
            }
        };

        Ok(CapabilityToken {
            tenant: TenantId(tenant.to_string()),
            region: Region(region.to_string()),
            kind,
            subject_key: subject_key.to_string(),
            authority,
            jti: jti.to_string(),
            dpop_bound,
            purpose,
            audience,
            exp_unix: i64::MAX,
        })
    }

    fn verify_for_request(
        &self,
        credential: &myelin_identity::Credential,
        _binding: &crate::capability_crypto::DpopBinding,
    ) -> myelin_identity::Result<CapabilityToken> {
        self.verify(credential)
    }
}

const REPO_GRANT_PREFIX: &str = "repo:";
const SELFHOSTED_GRANT_PREFIX: &str = "selfhosted:";

pub struct CapabilityAuthenticator {
    store: PrincipalStore,
    verifier: Arc<dyn TokenVerifier>,
    revocations: RevocationStore,
    now: NowFn,
    telemetry: Arc<AuthTelemetry>,
    idor: Arc<IdorCounters>,
}

impl CapabilityAuthenticator {
    #[cfg(test)]
    pub fn new(store: PrincipalStore) -> CapabilityAuthenticator {
        CapabilityAuthenticator::with_verifier(
            store,
            Arc::new(StructuralTokenVerifier::new()),
            RevocationStore::new(),
        )
    }

    pub fn with_verifier(
        store: PrincipalStore,
        verifier: Arc<dyn TokenVerifier>,
        revocations: RevocationStore,
    ) -> CapabilityAuthenticator {
        CapabilityAuthenticator {
            store,
            verifier,
            revocations,
            now: Arc::new(crate::clock::timestamp),
            telemetry: Arc::new(AuthTelemetry::new()),
            idor: Arc::new(IdorCounters::new()),
        }
    }

    pub fn with_clock(
        mut self,
        now: impl Fn() -> Timestamp + Send + Sync + 'static,
    ) -> CapabilityAuthenticator {
        self.now = Arc::new(now);
        self
    }

    pub fn telemetry(&self) -> &AuthTelemetry {
        &self.telemetry
    }

    pub fn idor_counters(&self) -> &IdorCounters {
        &self.idor
    }

    pub fn revocations(&self) -> &RevocationStore {
        &self.revocations
    }

    pub fn authenticate_identity(
        &self,
        credential: &myelin_identity::Credential,
        path_tenant: Option<&TenantId>,
    ) -> myelin_identity::Result<RequestIdentity> {
        self.authenticate_identity_with_binding(credential, path_tenant, None)
    }

    pub fn authenticate_identity_for_request(
        &self,
        credential: &myelin_identity::Credential,
        path_tenant: Option<&TenantId>,
        binding: &crate::capability_crypto::DpopBinding,
    ) -> myelin_identity::Result<RequestIdentity> {
        self.authenticate_identity_with_binding(credential, path_tenant, Some(binding))
    }

    fn authenticate_identity_with_binding(
        &self,
        credential: &myelin_identity::Credential,
        path_tenant: Option<&TenantId>,
        binding: Option<&crate::capability_crypto::DpopBinding>,
    ) -> myelin_identity::Result<RequestIdentity> {
        self.telemetry.observe();

        let token = match binding {
            Some(binding) => self.verifier.verify_for_request(credential, binding)?,
            None => self.verifier.verify(credential)?,
        };

        let scope = self.scope_for(&token);

        let now = (self.now)();
        let target = RevokeTarget::Jti(token.jti.clone());
        if token.purpose.is_run_scoped() {
            if let CredentialPurpose::AgentRun {
                delegation_snapshot,
                ..
            } = &token.purpose
            {
                if !matches!(delegation_snapshot, Some(snapshot) if *snapshot > 0) {
                    return Err(AuthzError::FailClosed(
                        "agent-run credential has no valid durable delegation-policy snapshot; \
                         caller-supplied legacy run mints are not authorization credentials"
                            .into(),
                    ));
                }
            }
            let state = self.revocations.run_token_state(&scope, &target, &now);
            if state != RunTokenState::LiveWithinRunLife {
                return Err(AuthzError::FailClosed(format!(
                    "run-scoped token `{}` is not live in durable S7 ({state:?}) - expired, \
                     torn-down, and unknown run credentials are refused",
                    token.jti
                )));
            }
        } else if self.revocations.is_revoked(&scope, &target, &now) {
            return Err(AuthzError::FailClosed(format!(
                "token `{}` is revoked (durable S7 revocation store) - fail-closed (the deny survives \
                 restart; tenant `{}`)",
                token.jti, token.tenant.0
            )));
        }

        if token.kind == MachineKind::Pat && !token.dpop_bound {
            return Err(AuthzError::BadRequest(
                "a long-lived PAT must be DPoP sender-constrained (RFC 9449, §4) - a bearer-only PAT \
                 is refused"
                    .into(),
            ));
        }

        let resolved = scope.resolve(path_tenant);
        debug_assert_eq!(
            resolved.tenant, token.tenant,
            "the effective tenant must be the verified token's (ID-3, C6)"
        );
        if resolved.path_derived {
            self.idor.count_path_derived();
        }
        if resolved.attempted_path_mismatch {
            self.idor.count_attempted_path_mismatch();
        }

        self.enforce_authority_ceiling(&token)?;

        let row = match &token.purpose {
            // Browser sessions and per-run agent capabilities are already signed, bounded
            // credentials whose subject is a durable principal id. Requiring a second credential
            // binding would turn either flow back into long-lived API-key provisioning.
            CredentialPurpose::HumanSession | CredentialPurpose::AgentRun { .. } => self
                .store
                .get_principal(&scope, &PrincipalId(token.subject_key.clone())),
            _ => self.store.resolve_credential(
                &scope,
                credential.scheme.as_str(),
                &token.subject_key,
            ),
        }
        .map_err(|e| {
            AuthzError::FailClosed(format!(
                "identity directory lookup failed for verified `{}` token - fail-closed: {e}",
                credential.scheme
            ))
        })?
        .ok_or_else(|| {
            AuthzError::FailClosed(format!(
                "no `{}` token record for the verified subject in tenant `{}` (unknown token - \
                     fail-closed, never a fabricated session)",
                credential.scheme, token.tenant.0
            ))
        })?;

        match row.status {
            PrincipalStatus::Active => {}
            PrincipalStatus::Suspended | PrincipalStatus::Disabled => {
                return Err(AuthzError::FailClosed(format!(
                    "machine principal `{}` is {:?} - authenticate fail-closes (it never resolves to \
                     an active session); full revocation is P-ID-14",
                    row.principal_id.0, row.status
                )));
            }
        }

        let principal = Principal::new(
            token.tenant.clone(),
            token.region.clone(),
            row.principal_id,
            row.kind,
            row.data_role,
            row.status,
        );
        Ok(RequestIdentity {
            principal,
            scope,
            credential: CredentialContext::Capability(VerifiedCapabilityContext {
                purpose: token.purpose,
                audience: token.audience,
                jti: token.jti,
                effective_authority: token.authority,
                expires_at_unix: token.exp_unix,
                dpop: if token.dpop_bound {
                    DpopState::Verified
                } else {
                    DpopState::Unbound
                },
            }),
        })
    }

    pub fn authenticate(
        &self,
        credential: &myelin_identity::Credential,
        path_tenant: Option<&TenantId>,
    ) -> myelin_identity::Result<Principal> {
        self.authenticate_identity(credential, path_tenant)
            .map(|identity| identity.principal)
    }

    pub fn authenticate_trait(
        &self,
        credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<Principal> {
        self.authenticate(credential, None)
    }

    fn enforce_authority_ceiling(&self, token: &CapabilityToken) -> myelin_identity::Result<()> {
        match token.kind {
            MachineKind::DeployKey => {
                for g in token.authority.grants() {
                    if !g.starts_with(REPO_GRANT_PREFIX) {
                        return Err(AuthzError::FailClosed(format!(
                            "deploy-key authority `{g}` exceeds the repo-scope ceiling (a deploy key \
                             is a repo-scoped Service principal, C6) - refused"
                        )));
                    }
                }
                Ok(())
            }
            MachineKind::PerJob => {
                let own = format!("{SELFHOSTED_GRANT_PREFIX}{}", token.tenant.0);
                for g in token.authority.grants() {
                    if !g.starts_with(SELFHOSTED_GRANT_PREFIX) {
                        return Err(AuthzError::FailClosed(format!(
                            "self-hosted-runner authority `{g}` is not a SelfHosted-scoped grant (C6) \
                             - refused"
                        )));
                    }
                    if g != own {
                        return Err(AuthzError::FailClosed(format!(
                            "self-hosted-runner authority `{g}` names a tenant other than its own \
                             (`{own}`) - a runner token cannot act cross-tenant (C6, no-global-pool) \
                             - refused"
                        )));
                    }
                }
                Ok(())
            }
            MachineKind::Pat | MachineKind::Ci | MachineKind::Agent => Ok(()),
        }
    }

    fn scope_for(&self, token: &CapabilityToken) -> TenantScope {
        let principal = Principal::stub(
            PrincipalId(format!("tok:{}", token.subject_key)),
            PrincipalKind::Service,
            token.tenant.clone(),
        );
        TenantScope::from_verified_token(&principal, token.region.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{iam_events::signals, Credential, DataRole};
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

    fn material(
        tenant: &str,
        region: &str,
        subject_key: &str,
        jti: &str,
        dpop: bool,
        grants: &[&str],
    ) -> String {
        format!(
            "{tenant}|{region}|{subject_key}|{jti}|{}|{}|test_kind|edge||",
            if dpop { "1" } else { "0" },
            grants.join(",")
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn seeded(
        scheme: &str,
        tenant: &str,
        region: &str,
        subject_key: &str,
        principal_id: &str,
        kind: PrincipalKind,
        status: PrincipalStatus,
    ) -> CapabilityAuthenticator {
        let st = store();
        let sc = scope(tenant, region);
        st.put_principal(
            &sc,
            PrincipalId(principal_id.into()),
            kind,
            DataRole::Controller,
            status,
            None,
        )
        .unwrap();
        st.link_credential(&sc, scheme, subject_key, &PrincipalId(principal_id.into()))
            .unwrap();
        let revocations = RevocationStore::new();
        if matches!(scheme, scheme::CI | scheme::PER_JOB) {
            for jti in ["jti-1", "jti-ci", "jti-live", "jti-r1", "jti-r2", "jti-x"] {
                revocations
                    .register_run_token_ttl(
                        &sc,
                        jti,
                        Timestamp("2020-01-01T00:00:00Z".into()),
                        Timestamp("2099-01-01T00:00:00Z".into()),
                    )
                    .expect("record seeded run lifetime");
            }
        }
        CapabilityAuthenticator::with_verifier(
            st,
            Arc::new(StructuralTokenVerifier::new()),
            revocations,
        )
    }

    fn cred(scheme: &str, material: String) -> Credential {
        Credential {
            scheme: scheme.into(),
            material,
        }
    }

    #[test]
    fn each_machine_scheme_resolves_to_its_principal() {
        let cases: &[(&str, bool, &str)] = &[
            (scheme::PAT, true, "repo:acme/web#write"),
            (scheme::CI, false, "ci:run"),
            (scheme::AGENT, false, "agent:run"),
            (scheme::DEPLOY_KEY, false, "repo:acme/web#push"),
            (scheme::PER_JOB, false, "selfhosted:acme"),
        ];
        for (s, dpop, grant) in cases {
            let auth = seeded(
                s,
                "acme",
                "eu-west",
                "subj-1",
                "svc:machine",
                PrincipalKind::Service,
                PrincipalStatus::Active,
            );
            let p = auth
                .authenticate(
                    &cred(
                        s,
                        material("acme", "eu-west", "subj-1", "jti-1", *dpop, &[grant]),
                    ),
                    None,
                )
                .unwrap_or_else(|e| panic!("scheme `{s}` should resolve: {e:?}"));
            assert_eq!(
                p.principal_id,
                PrincipalId("svc:machine".into()),
                "scheme {s}"
            );
            assert_eq!(
                p.tenant,
                TenantId("acme".into()),
                "scheme {s} tenant from token"
            );
            assert_eq!(p.region, Region("eu-west".into()), "scheme {s} region");
            assert_eq!(
                p.kind,
                PrincipalKind::Service,
                "scheme {s} machine → Service"
            );
        }
    }

    fn run_material(jti: &str) -> String {
        format!("acme|eu-west|run-subject|{jti}|0|repo.pull|agent_run|edge|run-1|42")
    }

    fn run_authenticator(
        revocations: RevocationStore,
        now: &'static str,
    ) -> CapabilityAuthenticator {
        scoped_run_authenticator(scheme::AGENT, "run-subject", revocations, now)
    }

    fn scoped_run_authenticator(
        credential_scheme: &str,
        subject_key: &str,
        revocations: RevocationStore,
        now: &'static str,
    ) -> CapabilityAuthenticator {
        let st = store();
        let sc = scope("acme", "eu-west");
        let principal_id = if credential_scheme == scheme::AGENT {
            subject_key
        } else {
            "svc:agent"
        };
        st.put_principal(
            &sc,
            PrincipalId(principal_id.into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .unwrap();
        if credential_scheme != scheme::AGENT {
            st.link_credential(
                &sc,
                credential_scheme,
                subject_key,
                &PrincipalId(principal_id.into()),
            )
            .unwrap();
        }
        CapabilityAuthenticator::with_verifier(
            st,
            Arc::new(StructuralTokenVerifier::new()),
            revocations,
        )
        .with_clock(move || Timestamp(now.into()))
    }

    #[test]
    fn agent_run_requires_live_durable_s7_state() {
        let sc = scope("acme", "eu-west");

        let live_s7 = RevocationStore::new();
        live_s7
            .register_run_token_ttl(
                &sc,
                "jti-live",
                Timestamp("2026-07-18T10:00:00Z".into()),
                Timestamp("2026-07-18T10:05:00Z".into()),
            )
            .expect("record live run lifetime");
        let identity = run_authenticator(live_s7, "2026-07-18T10:01:00Z")
            .authenticate_identity(&cred(scheme::AGENT, run_material("jti-live")), None)
            .expect("a known run token resolves its principal without a second API-key binding");
        assert_eq!(
            identity.principal.principal_id,
            PrincipalId("run-subject".into())
        );
        assert_eq!(identity.capability().effective_authority.len(), 1);

        let unknown = run_authenticator(RevocationStore::new(), "2026-07-18T10:01:00Z")
            .authenticate_identity(&cred(scheme::AGENT, run_material("jti-unknown")), None);
        assert!(matches!(unknown, Err(AuthzError::FailClosed(_))));

        let expired_s7 = RevocationStore::new();
        expired_s7
            .register_run_token_ttl(
                &sc,
                "jti-expired",
                Timestamp("2026-07-18T10:00:00Z".into()),
                Timestamp("2026-07-18T10:05:00Z".into()),
            )
            .expect("record expired run lifetime");
        let expired = run_authenticator(expired_s7, "2026-07-18T10:05:00Z")
            .authenticate_identity(&cred(scheme::AGENT, run_material("jti-expired")), None);
        assert!(matches!(expired, Err(AuthzError::FailClosed(_))));

        let torn_down_s7 = RevocationStore::new();
        torn_down_s7
            .register_run_token_ttl(
                &sc,
                "jti-torn-down",
                Timestamp("2026-07-18T10:00:00Z".into()),
                Timestamp("2026-07-18T10:05:00Z".into()),
            )
            .expect("record torn-down run lifetime");
        torn_down_s7
            .tear_down_run_token(
                &sc,
                "jti-torn-down",
                Timestamp("2026-07-18T10:01:00Z".into()),
            )
            .expect("record run teardown");
        let torn_down = run_authenticator(torn_down_s7, "2026-07-18T10:02:00Z")
            .authenticate_identity(&cred(scheme::AGENT, run_material("jti-torn-down")), None);
        assert!(matches!(torn_down, Err(AuthzError::FailClosed(_))));
    }

    #[test]
    fn agent_run_without_durable_snapshot_fails_closed() {
        let sc = scope("acme", "eu-west");
        let s7 = RevocationStore::new();
        s7.register_run_token_ttl(
            &sc,
            "jti-legacy",
            Timestamp("2026-07-18T10:00:00Z".into()),
            Timestamp("2026-07-18T10:05:00Z".into()),
        )
        .expect("record legacy run lifetime");
        let auth = run_authenticator(s7, "2026-07-18T10:01:00Z");
        let material = "acme|eu-west|run-subject|jti-legacy|0|repo.pull|agent_run|edge|run-1|";
        let result = auth.authenticate_identity(&cred(scheme::AGENT, material.into()), None);
        assert!(matches!(result, Err(AuthzError::FailClosed(_))));
    }

    #[test]
    fn ci_and_per_job_require_live_durable_s7_state() {
        for (credential_scheme, purpose, subject, grants) in [
            (scheme::CI, "ci_job", "ci-run", "ci.checks.report"),
            (scheme::PER_JOB, "per_job", "runner-job", "selfhosted:acme"),
        ] {
            let material = |jti: &str| {
                format!("acme|eu-west|{subject}|{jti}|0|{grants}|{purpose}|edge|run-7|")
            };
            let sc = scope("acme", "eu-west");

            let live_s7 = RevocationStore::new();
            live_s7
                .register_run_token_ttl(
                    &sc,
                    "jti-live-kind",
                    Timestamp("2026-07-18T10:00:00Z".into()),
                    Timestamp("2026-07-18T10:05:00Z".into()),
                )
                .expect("record live credential lifetime");
            scoped_run_authenticator(credential_scheme, subject, live_s7, "2026-07-18T10:01:00Z")
                .authenticate_identity(&cred(credential_scheme, material("jti-live-kind")), None)
                .expect("known live run-scoped credential");

            let unknown = scoped_run_authenticator(
                credential_scheme,
                subject,
                RevocationStore::new(),
                "2026-07-18T10:01:00Z",
            )
            .authenticate_identity(&cred(credential_scheme, material("jti-unknown-kind")), None);
            assert!(matches!(unknown, Err(AuthzError::FailClosed(_))));

            let expired_s7 = RevocationStore::new();
            expired_s7
                .register_run_token_ttl(
                    &sc,
                    "jti-expired-kind",
                    Timestamp("2026-07-18T10:00:00Z".into()),
                    Timestamp("2026-07-18T10:05:00Z".into()),
                )
                .expect("record expired credential lifetime");
            let expired = scoped_run_authenticator(
                credential_scheme,
                subject,
                expired_s7,
                "2026-07-18T10:05:00Z",
            )
            .authenticate_identity(&cred(credential_scheme, material("jti-expired-kind")), None);
            assert!(matches!(expired, Err(AuthzError::FailClosed(_))));

            let torn_s7 = RevocationStore::new();
            torn_s7
                .register_run_token_ttl(
                    &sc,
                    "jti-torn-kind",
                    Timestamp("2026-07-18T10:00:00Z".into()),
                    Timestamp("2026-07-18T10:05:00Z".into()),
                )
                .expect("record torn-down credential lifetime");
            torn_s7
                .tear_down_run_token(
                    &sc,
                    "jti-torn-kind",
                    Timestamp("2026-07-18T10:01:00Z".into()),
                )
                .expect("record credential teardown");
            let torn = scoped_run_authenticator(
                credential_scheme,
                subject,
                torn_s7,
                "2026-07-18T10:02:00Z",
            )
            .authenticate_identity(&cred(credential_scheme, material("jti-torn-kind")), None);
            assert!(matches!(torn, Err(AuthzError::FailClosed(_))));
        }
    }

    #[test]
    fn deploy_key_is_repo_scoped() {
        let auth = seeded(
            scheme::DEPLOY_KEY,
            "acme",
            "eu-west",
            "SHA256:dk",
            "svc:deploy",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        let p = auth
            .authenticate(
                &cred(
                    scheme::DEPLOY_KEY,
                    material(
                        "acme",
                        "eu-west",
                        "SHA256:dk",
                        "jti-dk",
                        false,
                        &["repo:acme/web#push"],
                    ),
                ),
                None,
            )
            .unwrap();
        assert_eq!(
            p.kind,
            PrincipalKind::Service,
            "a deploy key → repo-scoped Service principal"
        );

        let r = auth.authenticate(
            &cred(
                scheme::DEPLOY_KEY,
                material(
                    "acme",
                    "eu-west",
                    "SHA256:dk",
                    "jti-dk2",
                    false,
                    &["project:acme#admin"],
                ),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a deploy key may not exceed its one-repo ceiling (C6)"
        );
    }

    #[test]
    fn self_hosted_runner_cannot_act_cross_tenant() {
        let auth = seeded(
            scheme::PER_JOB,
            "acme",
            "eu-west",
            "run-1",
            "svc:runner",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        let p = auth
            .authenticate(
                &cred(
                    scheme::PER_JOB,
                    material(
                        "acme",
                        "eu-west",
                        "run-1",
                        "jti-r1",
                        false,
                        &["selfhosted:acme"],
                    ),
                ),
                None,
            )
            .unwrap();
        assert_eq!(
            p.tenant,
            TenantId("acme".into()),
            "the runner resolves into its own tenant"
        );
        assert_eq!(
            auth.idor_counters().path_derived_tenant_count(),
            0,
            "0 cross-tenant runner resolutions (the C6 mandatory-core)"
        );

        let r = auth.authenticate(
            &cred(
                scheme::PER_JOB,
                material(
                    "acme",
                    "eu-west",
                    "run-1",
                    "jti-r2",
                    false,
                    &["selfhosted:globex"],
                ),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a self-hosted-runner token cannot name another tenant's scope (C6, no-global-pool)"
        );
    }

    #[test]
    fn token_for_one_tenant_cannot_resolve_another_tenants_principal() {
        let st = store();
        let acme = scope("acme", "eu-west");
        st.put_principal(
            &acme,
            PrincipalId("svc:runner".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .unwrap();
        st.link_credential(
            &acme,
            scheme::PER_JOB,
            "run-1",
            &PrincipalId("svc:runner".into()),
        )
        .unwrap();
        let auth = CapabilityAuthenticator::new(st);

        let r = auth.authenticate(
            &cred(
                scheme::PER_JOB,
                material(
                    "globex",
                    "eu-west",
                    "run-1",
                    "jti-x",
                    false,
                    &["selfhosted:globex"],
                ),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a globex-verified token cannot resolve acme's principal (no cross-tenant resolve)"
        );
    }

    #[test]
    fn tenant_is_from_token_not_the_url_path() {
        let auth = seeded(
            scheme::CI,
            "acme",
            "eu-west",
            "run-7",
            "svc:ci",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        let p = auth
            .authenticate(
                &cred(
                    scheme::CI,
                    material("acme", "eu-west", "run-7", "jti-ci", false, &["ci:run"]),
                ),
                Some(&TenantId("globex".into())),
            )
            .unwrap();
        assert_eq!(
            p.tenant,
            TenantId("acme".into()),
            "the resolved tenant is the TOKEN's (acme), never the path's (globex)"
        );
        assert_eq!(
            auth.idor_counters().path_derived_tenant_count(),
            0,
            "path_derived_tenant_count == 0 (the IDOR floor - tenant never from the path)"
        );
        assert_eq!(
            auth.idor_counters().attempted_path_mismatch_count(),
            1,
            "the rejected IDOR attempt (path ≠ token) was counted (the guard held)"
        );
    }

    #[test]
    fn attenuated_pat_caveat_chain_narrows_authority() {
        let parent = Authority::of([
            "repo:acme/web#read",
            "repo:acme/web#write",
            "repo:acme/api#read",
        ]);
        let caveat = Authority::of([
            "repo:acme/web#read",
            "repo:acme/api#read",
            "repo:acme/web#admin",
        ]);
        let attenuated = parent.attenuate(&caveat);

        assert!(
            attenuated.is_subset_of(&parent),
            "attenuation is monotone: the child authority is no wider than the parent"
        );
        assert!(
            attenuated.len() < parent.len(),
            "the chain strictly narrowed (#write dropped)"
        );
        assert!(
            !attenuated.holds("repo:acme/web#admin"),
            "a grant the parent never held is NEVER minted by a caveat (monotone law)"
        );
        assert!(attenuated.holds("repo:acme/web#read"));
        assert!(attenuated.holds("repo:acme/api#read"));

        let step2 = attenuated.attenuate(&Authority::of(["repo:acme/web#read"]));
        assert!(
            step2.is_subset_of(&attenuated),
            "a second caveat narrows again (never widens)"
        );
        assert_eq!(step2.len(), 1, "the chain converged to one grant");
    }

    #[test]
    fn attenuation_is_never_amplifying() {
        let cases: &[(&[&str], &[&str])] = &[
            (&["a", "b"], &["a", "b", "c"]),
            (&["a", "b", "c"], &["b"]),
            (&[], &["a"]),
            (&["a"], &[]),
            (&["x", "y"], &["x", "y"]),
        ];
        for (parent_g, caveat_g) in cases {
            let parent = Authority::of(parent_g.iter().copied());
            let caveat = Authority::of(caveat_g.iter().copied());
            let child = parent.attenuate(&caveat);
            assert!(
                child.is_subset_of(&parent),
                "attenuate({parent_g:?}, {caveat_g:?}) = {:?} must be ⊆ the parent (never amplify)",
                child.grants().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn long_lived_pat_must_be_dpop_bound() {
        let auth = seeded(
            scheme::PAT,
            "acme",
            "eu-west",
            "pat-1",
            "p:alice",
            PrincipalKind::Human,
            PrincipalStatus::Active,
        );
        let r = auth.authenticate(
            &cred(
                scheme::PAT,
                material(
                    "acme",
                    "eu-west",
                    "pat-1",
                    "jti-p1",
                    false,
                    &["repo:acme/web#read"],
                ),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::BadRequest(_))),
            "a bearer-only PAT (no DPoP) is refused (§4)"
        );
        let p = auth
            .authenticate(
                &cred(
                    scheme::PAT,
                    material(
                        "acme",
                        "eu-west",
                        "pat-1",
                        "jti-p2",
                        true,
                        &["repo:acme/web#read"],
                    ),
                ),
                None,
            )
            .unwrap();
        assert_eq!(
            p.principal_id,
            PrincipalId("p:alice".into()),
            "a DPoP-bound PAT resolves"
        );
    }

    #[test]
    fn revoked_token_fails_closed() {
        let auth = seeded(
            scheme::CI,
            "acme",
            "eu-west",
            "run-9",
            "svc:ci",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        auth.authenticate(
            &cred(
                scheme::CI,
                material("acme", "eu-west", "run-9", "jti-live", false, &["ci:run"]),
            ),
            None,
        )
        .unwrap();
        let sc = scope("acme", "eu-west");
        auth.revocations()
            .tear_down_run_token(&sc, "jti-live", Timestamp("2026-06-26T00:00:00Z".into()))
            .expect("record credential teardown");
        let r = auth.authenticate(
            &cred(
                scheme::CI,
                material("acme", "eu-west", "run-9", "jti-live", false, &["ci:run"]),
            ),
            None,
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(_))),
            "a revoked token (durable S7 store) fails closed (never a live session)"
        );
        assert!(!auth.revocations().is_revoked(
            &scope("globex", "eu-west"),
            &RevokeTarget::Jti("jti-live".into()),
            &Timestamp("2026-06-26T00:00:01Z".into()),
        ));
    }

    #[test]
    fn human_sso_scheme_is_refused_here() {
        let auth = CapabilityAuthenticator::new(store());
        for s in human_scheme::HUMAN_SSO_SCHEMES {
            let r = auth.authenticate(
                &cred(s, material("acme", "eu-west", "x", "jti", false, &[])),
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::BadRequest(_))),
                "scheme `{s}` is P-ID-06's (human/SSO), refused by the machine-identity body"
            );
        }
    }

    #[test]
    fn malformed_token_envelope_is_refused() {
        let auth = CapabilityAuthenticator::new(store());
        let bad = [
            "",
            "acme|eu-west|s|jti|0",
            "acme|eu-west|s|jti|0|g|extra",
            "|eu-west|s|jti|0|",
            "acme|eu-west||jti|0|",
            "acme|eu-west|s||0|",
            "acme|eu-west|s|jti|2|",
        ];
        for m in bad {
            let r = auth.authenticate(&cred(scheme::CI, m.into()), None);
            assert!(
                matches!(r, Err(AuthzError::BadRequest(_))),
                "malformed token envelope `{m}` is refused"
            );
        }
        assert_eq!(
            auth.telemetry().decision_count(),
            bad.len() as u64,
            "every refused decision still emitted its observation"
        );
    }

    #[test]
    fn auth_decision_latency_emits_once_per_request() {
        let auth = seeded(
            scheme::AGENT,
            "acme",
            "eu-west",
            "run-x",
            "svc:agent",
            PrincipalKind::Service,
            PrincipalStatus::Active,
        );
        assert_eq!(auth.telemetry().decision_count(), 0);
        auth.authenticate(
            &cred(
                scheme::AGENT,
                material("acme", "eu-west", "run-x", "jti-a", false, &["agent:run"]),
            ),
            None,
        )
        .unwrap();
        assert_eq!(
            auth.telemetry().decision_count(),
            1,
            "success emits one observation"
        );
        let _ = auth.authenticate(
            &cred(
                scheme::AGENT,
                material("acme", "eu-west", "no-such", "jti-b", false, &["agent:run"]),
            ),
            None,
        );
        assert_eq!(
            auth.telemetry().decision_count(),
            2,
            "a failed decision also emits"
        );
        assert_eq!(AuthTelemetry::SIGNAL, signals::AUTH_DECISION_LATENCY);
    }

    #[test]
    fn authority_and_kind_predicates_are_exact() {
        let parent = Authority::of(["a", "b"]);
        assert!(
            Authority::of(["a"]).is_subset_of(&parent),
            "a strict subset is ⊆"
        );
        assert!(
            parent.is_subset_of(&parent),
            "an equal set is ⊆ (not strict)"
        );
        assert!(
            !Authority::of(["a", "c"]).is_subset_of(&parent),
            "a non-subset is NOT ⊆"
        );
        assert!(
            Authority::default().is_empty(),
            "the empty authority is empty"
        );
        assert!(!parent.is_empty(), "a non-empty authority is not empty");
        assert_eq!(parent.len(), 2);
        assert!(parent.holds("a") && !parent.holds("z"));

        assert!(MachineKind::PerJob.is_self_hosted_runner());
        for k in [
            MachineKind::Pat,
            MachineKind::Ci,
            MachineKind::Agent,
            MachineKind::DeployKey,
        ] {
            assert!(
                !k.is_self_hosted_runner(),
                "{k:?} is not a self-hosted runner"
            );
        }

        for s in scheme::MACHINE_SCHEMES {
            assert!(scheme::is_machine(s), "`{s}` is a machine scheme");
        }
        for s in human_scheme::HUMAN_SSO_SCHEMES {
            assert!(
                !scheme::is_machine(s),
                "`{s}` is NOT a machine scheme (it is human/SSO)"
            );
        }
        assert!(!scheme::is_machine("nonsense"));
    }

    #[test]
    fn disabled_machine_principal_fails_closed() {
        for status in [PrincipalStatus::Disabled, PrincipalStatus::Suspended] {
            let auth = seeded(
                scheme::DEPLOY_KEY,
                "acme",
                "eu-west",
                "SHA256:dk",
                "svc:deploy",
                PrincipalKind::Service,
                status,
            );
            let r = auth.authenticate(
                &cred(
                    scheme::DEPLOY_KEY,
                    material(
                        "acme",
                        "eu-west",
                        "SHA256:dk",
                        "jti",
                        false,
                        &["repo:acme/web#push"],
                    ),
                ),
                None,
            );
            assert!(
                matches!(r, Err(AuthzError::FailClosed(_))),
                "a {status:?} machine principal fails closed"
            );
        }
    }

    #[test]
    fn production_paseto_verifier_refuses_forged_token_never_mocks() {
        use crate::capability_crypto::{CellTokenAuthority, PasetoCapabilityVerifier};
        let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
        let auth = CapabilityAuthenticator::with_verifier(
            store(),
            Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
            RevocationStore::new(),
        );
        let forged = material(
            "acme",
            "eu-west",
            "run-1",
            "jti-forge",
            false,
            &["agent:run"],
        );
        let r = auth.authenticate(&cred(scheme::AGENT, forged), None);
        assert!(
            matches!(r, Err(AuthzError::BadRequest(_)) | Err(AuthzError::FailClosed(_))),
            "the production PASETO verifier must REFUSE a forged plaintext envelope (real crypto), \
             never resolve it through the mock StructuralTokenVerifier"
        );
    }
}
