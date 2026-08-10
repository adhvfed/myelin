use crate::delegation::{authority_of, DelegationAlgebra, DelegationInput, IntersectionProof};
use crate::delegation_policy::ResolvedDelegationPolicy;
use crate::machine_auth::{scheme, CapabilityToken, CredentialPurpose, MachineKind, TokenVerifier};
use crate::revocation::{RevocationStore, RunTokenState};
use crate::tuple_store::TupleStore;
use myelin_events::Timestamp;
use myelin_identity::{
    Credential, DelegationCaveats, FailStaticBound, ObjectId, Precondition, Principal, PrincipalId,
    RelName, RelationTuple, RevokeTarget, RunId, RunToken, TupleDelta,
};
use myelin_storage::TenantScope;
use std::sync::Arc;

pub const SELFHOSTED_GRANT_PREFIX: &str = "selfhosted:";

pub const RUN_GRANT_RELATION: &str = "run_bound";

#[derive(Clone)]
pub struct RunTokenAuthorizer {
    verifier: Arc<dyn TokenVerifier>,
    revocations: RevocationStore,
    now: Arc<dyn Fn() -> Timestamp + Send + Sync>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiJobAuthorizationError {
    EmptyExpectedIdentifier,
    CredentialVerificationRefused,
    WrongMachineKind { actual: MachineKind },
    WrongCredentialPurpose,
    JobIdentifierMismatch,
    CarrierJtiMismatch,
    TenantMismatch,
    RegionMismatch,
    SubjectMismatch,
    NotLive { state: RunTokenState },
    MissingCapability { capability: String },
}

impl std::fmt::Display for CiJobAuthorizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyExpectedIdentifier => {
                write!(f, "expected CI job/run identifier must be non-empty")
            }
            Self::CredentialVerificationRefused => write!(
                f,
                "CI job credential signature, expiry, or caveat verification refused"
            ),
            Self::WrongMachineKind { actual } => {
                write!(f, "CI launch requires machine kind `Ci`, got `{actual:?}`")
            }
            Self::WrongCredentialPurpose => {
                write!(f, "CI launch requires signed credential purpose `ci_job`")
            }
            Self::JobIdentifierMismatch => write!(
                f,
                "signed CI job/run identifier does not match the expected launch identifier"
            ),
            Self::CarrierJtiMismatch => {
                write!(f, "run-token carrier JTI does not match the signed JTI")
            }
            Self::TenantMismatch => {
                write!(f, "signed CI token tenant does not match the launch tenant")
            }
            Self::RegionMismatch => {
                write!(f, "signed CI token region does not match the launch region")
            }
            Self::SubjectMismatch => {
                write!(f, "signed CI token subject does not match the expected principal")
            }
            Self::NotLive { state } => {
                write!(f, "CI job token is not live in durable S7 at launch ({state:?})")
            }
            Self::MissingCapability { capability } => write!(
                f,
                "CI launch requires capability `{capability}` outside the signed attenuated authority"
            ),
        }
    }
}

impl std::error::Error for CiJobAuthorizationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BoundaryCheckError {
    CarrierJtiMismatch,
    TenantMismatch,
    RegionMismatch,
    SubjectMismatch,
    NotLive(RunTokenState),
    MissingCapability(String),
}

impl RunTokenAuthorizer {
    pub fn new(verifier: Arc<dyn TokenVerifier>, revocations: RevocationStore) -> Self {
        Self {
            verifier,
            revocations,
            now: Arc::new(system_now_timestamp),
        }
    }

    pub fn with_clock(mut self, now: impl Fn() -> Timestamp + Send + Sync + 'static) -> Self {
        self.now = Arc::new(now);
        self
    }

    pub fn authorize(
        &self,
        scope: &TenantScope,
        expected_principal: &PrincipalId,
        run_token: &RunToken,
        required_caps: &[String],
    ) -> Result<CapabilityToken, String> {
        let verified = self
            .verifier
            .verify(&Credential {
                scheme: scheme::AGENT.into(),
                material: run_token.token.clone(),
            })
            .map_err(|e| format!("run-token signature/caveat verification refused: {e:?}"))?;

        if verified.kind != MachineKind::Agent {
            return Err("presented token is not an agent run token".into());
        }
        match &verified.purpose {
            CredentialPurpose::AgentRun {
                delegation_snapshot: Some(snapshot),
                ..
            } if *snapshot > 0 => {}
            CredentialPurpose::AgentRun { .. } => {
                return Err(
                    "agent run token is not bound to a positive durable delegation snapshot".into(),
                )
            }
            _ => return Err("presented token purpose is not `agent_run`".into()),
        }
        self.check_boundary(
            scope,
            expected_principal,
            run_token,
            required_caps,
            &verified,
        )
        .map_err(|error| match error {
            BoundaryCheckError::CarrierJtiMismatch => {
                "run-token carrier jti does not match the signed jti".into()
            }
            BoundaryCheckError::TenantMismatch | BoundaryCheckError::RegionMismatch => {
                "run-token signed scope does not match the mutation boundary scope".into()
            }
            BoundaryCheckError::SubjectMismatch => {
                "run-token signed subject does not match the acting principal".into()
            }
            BoundaryCheckError::NotLive(_) => {
                "run token is unknown, torn down, or expired at the mutation boundary".into()
            }
            BoundaryCheckError::MissingCapability(capability) => format!(
                "tool requires capability `{capability}` outside the signed attenuated run-token authority"
            ),
        })?;
        Ok(verified)
    }

    pub fn authorize_ci_job(
        &self,
        scope: &TenantScope,
        expected_principal: &PrincipalId,
        expected_job_run_id: &str,
        run_token: &RunToken,
        required_caps: &[String],
    ) -> Result<CapabilityToken, CiJobAuthorizationError> {
        if expected_job_run_id.is_empty() {
            return Err(CiJobAuthorizationError::EmptyExpectedIdentifier);
        }
        let verified = self
            .verifier
            .verify(&Credential {
                scheme: scheme::CI.into(),
                material: run_token.token.clone(),
            })
            .map_err(|_| CiJobAuthorizationError::CredentialVerificationRefused)?;
        if verified.kind != MachineKind::Ci {
            return Err(CiJobAuthorizationError::WrongMachineKind {
                actual: verified.kind,
            });
        }
        match &verified.purpose {
            CredentialPurpose::CiJob { run_id }
                if !run_id.is_empty() && run_id == expected_job_run_id => {}
            CredentialPurpose::CiJob { .. } => {
                return Err(CiJobAuthorizationError::JobIdentifierMismatch)
            }
            _ => return Err(CiJobAuthorizationError::WrongCredentialPurpose),
        }
        self.check_boundary(
            scope,
            expected_principal,
            run_token,
            required_caps,
            &verified,
        )
        .map_err(|error| match error {
            BoundaryCheckError::CarrierJtiMismatch => CiJobAuthorizationError::CarrierJtiMismatch,
            BoundaryCheckError::TenantMismatch => CiJobAuthorizationError::TenantMismatch,
            BoundaryCheckError::RegionMismatch => CiJobAuthorizationError::RegionMismatch,
            BoundaryCheckError::SubjectMismatch => CiJobAuthorizationError::SubjectMismatch,
            BoundaryCheckError::NotLive(state) => CiJobAuthorizationError::NotLive { state },
            BoundaryCheckError::MissingCapability(capability) => {
                CiJobAuthorizationError::MissingCapability { capability }
            }
        })?;
        Ok(verified)
    }

    fn check_boundary(
        &self,
        scope: &TenantScope,
        expected_principal: &PrincipalId,
        run_token: &RunToken,
        required_caps: &[String],
        verified: &CapabilityToken,
    ) -> Result<(), BoundaryCheckError> {
        if verified.jti != run_token.jti {
            return Err(BoundaryCheckError::CarrierJtiMismatch);
        }
        if verified.tenant != *scope.tenant() {
            return Err(BoundaryCheckError::TenantMismatch);
        }
        if verified.region != *scope.region() {
            return Err(BoundaryCheckError::RegionMismatch);
        }
        if verified.subject_key != expected_principal.0 {
            return Err(BoundaryCheckError::SubjectMismatch);
        }
        let state = self.revocations.run_token_state(
            scope,
            &RevokeTarget::Jti(verified.jti.clone()),
            &(self.now)(),
        );
        if state != RunTokenState::LiveWithinRunLife {
            return Err(BoundaryCheckError::NotLive(state));
        }
        for capability in required_caps {
            if !verified.authority.holds(capability) {
                return Err(BoundaryCheckError::MissingCapability(capability.clone()));
            }
        }
        Ok(())
    }
}

fn system_now_timestamp() -> Timestamp {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let dt = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default();
    Timestamp(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MintError {
    SelfHostedScopeViolation(String),
    NonPositiveTtl,
    InvalidDelegationSnapshot(i64),
    UnsupportedRunKind(MachineKind),
    ResolvedPolicyBindingMismatch,
    InvalidMintAttempt,
}

impl core::fmt::Display for MintError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MintError::SelfHostedScopeViolation(g) => write!(
                f,
                "self-hosted-runner run token authority `{g}` names a scope outside its own \
                 tenant's SelfHosted jobs - a runner token cannot act cross-tenant (C6, \
                 no-global-pool) - refused"
            ),
            MintError::NonPositiveTtl => write!(
                f,
                "a per-run token TTL must be positive (life == run life) - a zero-TTL token is \
                 refused (it could be mistaken for never-expiring)"
            ),
            MintError::InvalidDelegationSnapshot(snapshot) => write!(
                f,
                "durable delegation snapshot `{snapshot}` is not a positive storage cursor - refused"
            ),
            MintError::UnsupportedRunKind(kind) => write!(
                f,
                "machine kind `{kind:?}` is not a run-scoped credential kind - refused"
            ),
            MintError::ResolvedPolicyBindingMismatch => f.write_str(
                "resolved delegation policy does not match the requested run/principal/scope binding - refused",
            ),
            MintError::InvalidMintAttempt => f.write_str(
                "run-token mint attempt must be a non-empty bounded printable token - refused",
            ),
        }
    }
}

impl std::error::Error for MintError {}

pub trait TokenSigner: Send + Sync {
    fn sign(&self, request: &TokenSignRequest) -> String;
}

#[derive(Clone)]
pub struct TokenSignRequest {
    scope: TenantScope,
    subject: PrincipalId,
    jti: String,
    purpose: CredentialPurpose,
    expires_at: Timestamp,
    grants: Vec<String>,
}

impl TokenSignRequest {
    pub fn new(
        scope: &TenantScope,
        subject: PrincipalId,
        jti: impl Into<String>,
        purpose: CredentialPurpose,
        expires_at: Timestamp,
        grants: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            scope: scope.clone(),
            subject,
            jti: jti.into(),
            purpose,
            expires_at,
            grants: grants.into_iter().map(Into::into).collect(),
        }
    }

    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    pub fn subject(&self) -> &PrincipalId {
        &self.subject
    }

    pub fn jti(&self) -> &str {
        &self.jti
    }

    pub fn purpose(&self) -> &CredentialPurpose {
        &self.purpose
    }

    pub fn expires_at(&self) -> &Timestamp {
        &self.expires_at
    }

    pub fn grants(&self) -> &[String] {
        &self.grants
    }
}

impl core::fmt::Debug for TokenSignRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TokenSignRequest")
            .field("tenant", self.scope.tenant())
            .field("region", self.scope.region())
            .field("subject", &"<redacted>")
            .field("jti", &"<redacted>")
            .field("purpose", &self.purpose.claim())
            .field("expires_at", &self.expires_at)
            .field("grant_count", &self.grants.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StructuralTokenSigner;

impl StructuralTokenSigner {
    pub fn new() -> StructuralTokenSigner {
        StructuralTokenSigner
    }
}

impl TokenSigner for StructuralTokenSigner {
    fn sign(&self, request: &TokenSignRequest) -> String {
        let tenant = &request.scope().tenant().0;
        let region = &request.scope().region().0;
        let subject_key = &request.subject().0;
        let jti = request.jti();
        let purpose = request.purpose();
        let audience = match purpose {
            crate::machine_auth::CredentialPurpose::AgentRun { .. } => "mcp",
            _ => "edge",
        };
        format!(
            "{tenant}|{region}|{subject_key}|{jti}|0|{}|{}|{audience}|{}|{}",
            request.grants().join(","),
            purpose.claim(),
            purpose.run_id().unwrap_or_default(),
            match purpose {
                crate::machine_auth::CredentialPurpose::AgentRun {
                    delegation_snapshot,
                    ..
                } => delegation_snapshot.map_or_else(String::new, |snapshot| snapshot.to_string()),
                _ => String::new(),
            },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationProof {
    pub jti: String,
    pub revoked_on_teardown: bool,
    pub auto_expires_within_run_life: bool,
    pub token_revocation_lag_secs: i64,
}

impl RevocationProof {
    pub fn holds(&self) -> bool {
        self.revoked_on_teardown && self.auto_expires_within_run_life
    }
}

#[derive(Clone)]
pub struct RunTokenMinter {
    algebra: DelegationAlgebra,
    revocations: RevocationStore,
    signer: std::sync::Arc<dyn TokenSigner>,
    tuples: Option<TupleStore>,
}

impl RunTokenMinter {
    pub fn with_signer_and_tuples(
        revocations: RevocationStore,
        tuples: Option<TupleStore>,
        signer: std::sync::Arc<dyn TokenSigner>,
    ) -> RunTokenMinter {
        RunTokenMinter {
            algebra: DelegationAlgebra::new(),
            revocations,
            signer,
            tuples,
        }
    }

    #[cfg(test)]
    pub fn new(revocations: RevocationStore) -> RunTokenMinter {
        RunTokenMinter::with_signer_and_tuples(
            revocations,
            None,
            std::sync::Arc::new(StructuralTokenSigner::new()),
        )
    }

    #[cfg(test)]
    pub fn with_tuple_store(revocations: RevocationStore, tuples: TupleStore) -> RunTokenMinter {
        RunTokenMinter::with_signer_and_tuples(
            revocations,
            Some(tuples),
            std::sync::Arc::new(StructuralTokenSigner::new()),
        )
    }

    pub fn with_signer(mut self, signer: std::sync::Arc<dyn TokenSigner>) -> RunTokenMinter {
        self.signer = signer;
        self
    }

    pub fn revocations(&self) -> &RevocationStore {
        &self.revocations
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mint_run_token(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &DelegationInput,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
    ) -> Result<RunToken, MintError> {
        let (token, _proof) = self.mint_proved(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            input,
            delegation_caveats,
            kind,
            ttl,
            now,
        )?;
        Ok(token)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mint_proved(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &DelegationInput,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
    ) -> Result<(RunToken, IntersectionProof), MintError> {
        self.mint_proved_with_snapshot(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            input,
            delegation_caveats,
            kind,
            ttl,
            now,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mint_from_resolved_policy(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        resolved: &ResolvedDelegationPolicy,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
    ) -> Result<RunToken, MintError> {
        self.mint_from_resolved_policy_at_attempt(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            resolved,
            delegation_caveats,
            kind,
            ttl,
            now,
            None,
        )
    }

    /// Mints a credential for one deterministic workflow activity attempt.
    ///
    /// The attempt token changes credential identity, never authority. It prevents a credential
    /// torn down after a park or failure from being reissued to a later activity attempt.
    #[allow(clippy::too_many_arguments)]
    pub fn mint_from_resolved_policy_for_attempt(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        resolved: &ResolvedDelegationPolicy,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
        mint_attempt: &str,
    ) -> Result<RunToken, MintError> {
        if mint_attempt.is_empty()
            || mint_attempt.len() > 512
            || !mint_attempt.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(MintError::InvalidMintAttempt);
        }
        self.mint_from_resolved_policy_at_attempt(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            resolved,
            delegation_caveats,
            kind,
            ttl,
            now,
            Some(mint_attempt),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn mint_from_resolved_policy_at_attempt(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        resolved: &ResolvedDelegationPolicy,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
        mint_attempt: Option<&str>,
    ) -> Result<RunToken, MintError> {
        let snapshot = resolved.cursor.snapshot;
        if snapshot <= 0 {
            return Err(MintError::InvalidDelegationSnapshot(snapshot));
        }
        if &resolved.run_id != run_id
            || &resolved.agent_id != agent_id
            || &agent.principal_id != agent_id
            || resolved.trigger_actor_id != trigger_actor.principal_id
            || scope.tenant() != &agent.tenant
            || scope.region() != &agent.region
            || scope.tenant() != &trigger_actor.tenant
            || scope.region() != &trigger_actor.region
            || self
                .algebra
                .delegation(agent, trigger_actor, &resolved.input)
                != resolved.effective_policy
        {
            return Err(MintError::ResolvedPolicyBindingMismatch);
        }
        let (token, _) = self.mint_proved_with_snapshot(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            &resolved.input,
            delegation_caveats,
            kind,
            ttl,
            now,
            Some(snapshot),
            mint_attempt,
        )?;
        Ok(token)
    }

    #[allow(clippy::too_many_arguments)]
    fn mint_proved_with_snapshot(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &DelegationInput,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now: &Timestamp,
        delegation_snapshot: Option<i64>,
        mint_attempt: Option<&str>,
    ) -> Result<(RunToken, IntersectionProof), MintError> {
        if ttl.static_max_secs == 0 {
            return Err(MintError::NonPositiveTtl);
        }
        let _ = delegation_caveats;

        let (effective_policy, proof) = self.algebra.delegation_proved(agent, trigger_actor, input);
        let effective = authority_of(&effective_policy);

        if kind.is_self_hosted_runner() {
            let own = format!("{SELFHOSTED_GRANT_PREFIX}{}", scope.tenant().0);
            for g in effective.grants() {
                if !g.starts_with(SELFHOSTED_GRANT_PREFIX) || g != own {
                    return Err(MintError::SelfHostedScopeViolation(g.to_string()));
                }
            }
        }

        let jti = mint_attempt.map_or_else(
            || run_token_jti(agent_id, run_id, now),
            |attempt| run_token_attempt_jti(agent_id, run_id, now, attempt),
        );

        let expires_at = expires_at_of(now, ttl);

        let purpose = match kind {
            MachineKind::Agent => CredentialPurpose::AgentRun {
                run_id: run_id.0.clone(),
                delegation_snapshot,
            },
            MachineKind::Ci => CredentialPurpose::CiJob {
                run_id: run_id.0.clone(),
            },
            MachineKind::PerJob => CredentialPurpose::PerJob {
                run_id: run_id.0.clone(),
            },
            MachineKind::Pat | MachineKind::DeployKey => {
                return Err(MintError::UnsupportedRunKind(kind))
            }
        };

        let request = TokenSignRequest::new(
            scope,
            agent_id.clone(),
            &jti,
            purpose,
            expires_at.clone(),
            effective.grants(),
        );
        let material = self.signer.sign(&request);

        self.revocations
            .register_run_token_ttl(scope, &jti, now.clone(), expires_at.clone());

        if let Some(tuples) = &self.tuples {
            let delta = TupleDelta::Add(RelationTuple {
                object: ObjectId(format!("run:{}", run_id.0)),
                relation: RelName(RUN_GRANT_RELATION.into()),
                subject: agent_id.clone(),
                caveat: None,
            });
            let _ = tuples.write_tuples(
                scope,
                agent,
                &[delta],
                None::<&Precondition>,
                Some(expires_at),
                now.clone(),
            );
        }

        Ok((
            RunToken {
                token: material,
                jti,
            },
            proof,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn re_mint_on_resume(
        &self,
        scope: &TenantScope,
        agent_id: &PrincipalId,
        run_id: &RunId,
        agent: &Principal,
        trigger_actor: &Principal,
        input_as_of_resume: &DelegationInput,
        delegation_caveats: &DelegationCaveats,
        kind: MachineKind,
        ttl: &FailStaticBound,
        now_resume: &Timestamp,
    ) -> Result<RunToken, MintError> {
        self.mint_run_token(
            scope,
            agent_id,
            run_id,
            agent,
            trigger_actor,
            input_as_of_resume,
            delegation_caveats,
            kind,
            ttl,
            now_resume,
        )
    }

    pub fn teardown(&self, scope: &TenantScope, token: &RunToken, now: &Timestamp) {
        self.revocations
            .tear_down_run_token(scope, &token.jti, now.clone());
    }

    pub fn is_live(&self, scope: &TenantScope, token: &RunToken, now: &Timestamp) -> bool {
        match self.revocation_state(scope, token, now) {
            RunTokenState::LiveWithinRunLife => true,
            RunTokenState::Expired | RunTokenState::TornDown | RunTokenState::Unknown => false,
        }
    }

    pub fn revocation_state(
        &self,
        scope: &TenantScope,
        token: &RunToken,
        now: &Timestamp,
    ) -> RunTokenState {
        let target = RevokeTarget::Jti(token.jti.clone());
        self.revocations.run_token_state(scope, &target, now)
    }
}

pub fn run_token_jti(agent_id: &PrincipalId, run_id: &RunId, mint_instant: &Timestamp) -> String {
    format!("runtok:{}:{}:{}", agent_id.0, run_id.0, mint_instant.0)
}

pub fn run_token_attempt_jti(
    agent_id: &PrincipalId,
    run_id: &RunId,
    mint_instant: &Timestamp,
    mint_attempt: &str,
) -> String {
    format!(
        "runtok:{}:{}:{}:attempt:{}",
        agent_id.0,
        run_id.0,
        mint_instant.0,
        blake3::hash(mint_attempt.as_bytes()).to_hex()
    )
}

pub fn expires_at_of(now: &Timestamp, ttl: &FailStaticBound) -> Timestamp {
    Timestamp(add_secs_rfc3339(&now.0, ttl.static_max_secs))
}

fn add_secs_rfc3339(instant: &str, secs: u64) -> String {
    let parsed = parse_rfc3339(instant);
    match parsed {
        Some((y, mo, d, h, mi, s)) => {
            let total = day_seconds(h, mi, s) + secs;
            let extra_days = total / 86_400;
            let rem = total % 86_400;
            let (nh, nmi, ns) = (
                (rem / 3_600) as u32,
                ((rem % 3_600) / 60) as u32,
                (rem % 60) as u32,
            );
            let (ny, nmo, nd) = add_days(y, mo, d, extra_days);
            format!("{ny:04}-{nmo:02}-{nd:02}T{nh:02}:{nmi:02}:{ns:02}Z")
        }
        None => format!("{instant}#+{secs}s"),
    }
}

fn day_seconds(h: u32, mi: u32, s: u32) -> u64 {
    h as u64 * 3_600 + mi as u64 * 60 + s as u64
}

fn parse_rfc3339(s: &str) -> Option<(i64, u32, u32, u32, u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    let h: u32 = s.get(11..13)?.parse().ok()?;
    let mi: u32 = s.get(14..16)?.parse().ok()?;
    let sec: u32 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    Some((y, mo, d, h, mi, sec))
}

fn add_days(mut y: i64, mut mo: u32, mut d: u32, extra_days: u64) -> (i64, u32, u32) {
    let mut remaining = extra_days;
    while remaining > 0 {
        let dim = days_in_month(y, mo);
        if d < dim {
            d += 1;
        } else {
            d = 1;
            if mo == 12 {
                mo = 1;
                y += 1;
            } else {
                mo += 1;
            }
        }
        remaining -= 1;
    }
    (y, mo, d)
}

fn days_in_month(y: i64, mo: u32) -> u32 {
    match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine_auth::{Authority, StructuralTokenVerifier};
    use myelin_events::OutboxStore;
    use myelin_identity::{PrincipalKind, RuntimeRef};
    use myelin_tenancy::{Region, TenantId};

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn scope_in(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    fn agent(id: &str, tenant: &str) -> Principal {
        let mut p = Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt-1".into()),
                on_behalf_of: Some(PrincipalId("p:human".into())),
            },
            TenantId(tenant.into()),
        );
        p.region = Region("eu-west".into());
        p
    }

    fn human(id: &str, tenant: &str) -> Principal {
        let mut p = Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        p.region = Region("eu-west".into());
        p
    }

    fn auth(grants: &[&str]) -> Authority {
        Authority::of(grants.iter().copied())
    }

    #[test]
    fn token_sign_request_debug_redacts_bearer_identifiers_and_authority() {
        let request = TokenSignRequest::new(
            &scope("acme"),
            PrincipalId("svc:secret-agent".into()),
            "runtok:secret-jti",
            CredentialPurpose::AgentRun {
                run_id: "run:secret".into(),
                delegation_snapshot: Some(42),
            },
            Timestamp("2030-01-01T00:00:00Z".into()),
            ["repo:secret:admin"],
        );
        let debug = format!("{request:?}");
        assert!(!debug.contains("svc:secret-agent"));
        assert!(!debug.contains("runtok:secret-jti"));
        assert!(!debug.contains("run:secret"));
        assert!(!debug.contains("repo:secret:admin"));
        assert!(debug.contains("agent_run"));
        assert!(debug.contains("grant_count: 1"));
    }

    fn input(agent: &[&str], deleg: &[&str], tenant: &[&str], held: &[&str]) -> DelegationInput {
        DelegationInput {
            agent_policy: auth(agent),
            delegation: auth(deleg),
            tenant_policy: auth(tenant),
            trigger_actor_held: auth(held),
        }
    }

    fn ts(s: &str) -> Timestamp {
        Timestamp(s.into())
    }

    #[test]
    fn workflow_attempts_have_stable_but_distinct_run_token_identities() {
        let agent = PrincipalId("agent:reviewer".into());
        let run = RunId("run-7".into());
        let now = ts("2026-08-10T09:37:00Z");
        let first = run_token_attempt_jti(&agent, &run, &now, "run-7/agent.run:1/act/1");

        assert_eq!(
            first,
            run_token_attempt_jti(&agent, &run, &now, "run-7/agent.run:1/act/1")
        );
        assert_ne!(
            first,
            run_token_attempt_jti(&agent, &run, &now, "run-7/agent.run:3/act/1")
        );
        assert_ne!(
            first,
            run_token_attempt_jti(&agent, &run, &now, "run-7/agent.run:1/act/2")
        );
    }

    fn ttl(secs: u64) -> FailStaticBound {
        FailStaticBound {
            static_max_secs: secs,
        }
    }

    fn caveats(g: &[&str]) -> DelegationCaveats {
        DelegationCaveats(g.iter().map(|s| s.to_string()).collect())
    }

    fn resolved_policy(
        run_id: &str,
        agent: &Principal,
        trigger_actor: &Principal,
        input: DelegationInput,
        snapshot: i64,
    ) -> ResolvedDelegationPolicy {
        let effective_policy = DelegationAlgebra::new().delegation(agent, trigger_actor, &input);
        ResolvedDelegationPolicy {
            run_id: RunId(run_id.into()),
            agent_id: agent.principal_id.clone(),
            trigger_actor_id: trigger_actor.principal_id.clone(),
            input,
            effective_policy,
            cursor: crate::delegation_policy::DelegationRunPolicyCursor {
                snapshot,
                versions: myelin_storage::DurableDelegationPolicyVersions {
                    agent: 1,
                    delegation: 1,
                    tenant: 1,
                    trigger_actor: 1,
                },
                revisions: myelin_storage::DurableDelegationPolicyRevisions {
                    agent: 1,
                    delegation: 1,
                    tenant: 1,
                    trigger_actor: 1,
                },
            },
        }
    }

    fn mint_ci_token(
        s7: &RevocationStore,
        scope: &TenantScope,
        subject: &str,
        job_run_id: &str,
        grants: &[&str],
        lifetime_secs: u64,
    ) -> RunToken {
        RunTokenMinter::new(s7.clone())
            .mint_run_token(
                scope,
                &PrincipalId(subject.into()),
                &RunId(job_run_id.into()),
                &agent(subject, &scope.tenant().0),
                &human("p:human", &scope.tenant().0),
                &input(grants, grants, grants, grants),
                &caveats(grants),
                MachineKind::Ci,
                &ttl(lifetime_secs),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint CI job token")
    }

    #[derive(Clone)]
    struct FixedCiSchemeVerifier(CapabilityToken);

    impl TokenVerifier for FixedCiSchemeVerifier {
        fn verify(&self, credential: &Credential) -> myelin_identity::Result<CapabilityToken> {
            assert_eq!(
                credential.scheme,
                scheme::CI,
                "CI boundary pins the verifier scheme"
            );
            Ok(self.0.clone())
        }

        fn verify_for_request(
            &self,
            credential: &Credential,
            _binding: &crate::capability_crypto::DpopBinding,
        ) -> myelin_identity::Result<CapabilityToken> {
            self.verify(credential)
        }
    }

    fn fixed_capability(kind: MachineKind, purpose: CredentialPurpose) -> CapabilityToken {
        CapabilityToken {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            kind,
            subject_key: "svc:ci".into(),
            authority: auth(&["job.launch", "artifact.write"]),
            jti: "fixed-jti".into(),
            dpop_bound: false,
            purpose,
            audience: crate::machine_auth::CredentialAudience::Edge,
            exp_unix: i64::MAX,
        }
    }

    #[test]
    fn final_boundary_rechecks_signed_attenuated_authority_and_liveness() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7.clone());
        let acme = scope("acme");
        let minted_at = ts("2026-06-19T00:00:00Z");
        let agent = agent("p:agent", "acme");
        let trigger = human("p:human", "acme");
        let policy = resolved_policy(
            "run-authority",
            &agent,
            &trigger,
            input(
                &["repo.push", "pull_request.merge"],
                &["repo.push"],
                &["repo.push", "pull_request.merge"],
                &["repo.push", "pull_request.merge"],
            ),
            7,
        );
        let token = minter
            .mint_from_resolved_policy(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-authority".into()),
                &agent,
                &trigger,
                &policy,
                &caveats(&["repo.push"]),
                MachineKind::Agent,
                &ttl(300),
                &minted_at,
            )
            .expect("mint attenuated run token");
        let authorizer =
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7.clone())
                .with_clock(|| ts("2026-06-19T00:01:00Z"));

        authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &token,
                &["repo.push".into()],
            )
            .expect("the one capability surviving delegation is admitted");
        let denied = authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &token,
                &["pull_request.merge".into()],
            )
            .expect_err("attenuated-away capability must be denied");
        assert!(denied.contains("outside the signed attenuated"));

        minter.teardown(&acme, &token, &ts("2026-06-19T00:02:00Z"));
        let denied = authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &token,
                &["repo.push".into()],
            )
            .expect_err("a torn-down token must fail again at the mutation boundary");
        assert!(denied.contains("torn down"));
    }

    #[test]
    fn final_boundary_refuses_mixed_carrier_identity_and_scope() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7.clone());
        let acme = scope("acme");
        let agent = agent("p:agent", "acme");
        let trigger = human("p:human", "acme");
        let policy = resolved_policy(
            "run-binding",
            &agent,
            &trigger,
            input(
                &["repo.push"],
                &["repo.push"],
                &["repo.push"],
                &["repo.push"],
            ),
            9,
        );
        let token = minter
            .mint_from_resolved_policy(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-binding".into()),
                &agent,
                &trigger,
                &policy,
                &caveats(&["repo.push"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        let authorizer = RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7)
            .with_clock(|| ts("2026-06-19T00:01:00Z"));

        let mut mixed = token.clone();
        mixed.jti = "attacker-selected-jti".into();
        assert!(authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &mixed,
                &["repo.push".into()],
            )
            .expect_err("carrier/signed jti mismatch")
            .contains("does not match"));
        assert!(authorizer
            .authorize(
                &acme,
                &PrincipalId("p:other".into()),
                &token,
                &["repo.push".into()],
            )
            .expect_err("subject mismatch")
            .contains("subject"));
        assert!(authorizer
            .authorize(
                &scope("globex"),
                &PrincipalId("p:agent".into()),
                &token,
                &["repo.push".into()],
            )
            .expect_err("scope mismatch")
            .contains("scope"));
    }

    #[test]
    fn final_boundary_refuses_snapshotless_agent_run_even_when_capability_matches() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7.clone());
        let acme = scope("acme");
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("legacy-raw-policy-run".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &input(
                    &["repo.push"],
                    &["repo.push"],
                    &["repo.push"],
                    &["repo.push"],
                ),
                &caveats(&["repo.push"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("legacy raw-policy mint remains available outside production routing");
        let authorizer = RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7)
            .with_clock(|| ts("2026-06-19T00:01:00Z"));
        let denied = authorizer
            .authorize(
                &acme,
                &PrincipalId("p:agent".into()),
                &token,
                &["repo.push".into()],
            )
            .expect_err("snapshot-less AgentRun must never reach a mutation adapter");
        assert!(denied.contains("durable delegation snapshot"));
    }

    #[test]
    fn ci_job_final_boundary_binds_scheme_kind_purpose_identity_scope_and_authority() {
        let s7 = RevocationStore::new();
        let acme = scope("acme");
        let token = mint_ci_token(
            &s7,
            &acme,
            "svc:ci",
            "job:run-17:build",
            &["job.launch", "artifact.write"],
            300,
        );
        let authorizer =
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7.clone())
                .with_clock(|| ts("2026-06-19T00:01:00Z"));

        let authorized = authorizer
            .authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &token,
                &["job.launch".into(), "artifact.write".into()],
            )
            .expect("the exact live CI job token is admitted immediately before launch");
        assert_eq!(authorized.kind, MachineKind::Ci);
        assert_eq!(
            authorized.purpose,
            CredentialPurpose::CiJob {
                run_id: "job:run-17:build".into()
            }
        );

        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::EmptyExpectedIdentifier)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-17:test",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::JobIdentifierMismatch)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &scope("globex"),
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::TenantMismatch)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &scope_in("acme", "eu-north"),
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::RegionMismatch)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:other".into()),
                "job:run-17:build",
                &token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::SubjectMismatch)
        );
        let mut mixed_carrier = token.clone();
        mixed_carrier.jti = "attacker-selected-jti".into();
        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &mixed_carrier,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::CarrierJtiMismatch)
        );
        assert_eq!(
            authorizer.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-17:build",
                &token,
                &["secret.read".into()],
            ),
            Err(CiJobAuthorizationError::MissingCapability {
                capability: "secret.read".into()
            })
        );
    }

    #[test]
    fn ci_job_final_boundary_refuses_wrong_credentials_and_every_non_live_s7_state() {
        let acme = scope("acme");
        let live_s7 = RevocationStore::new();
        let ci_token = mint_ci_token(
            &live_s7,
            &acme,
            "svc:ci",
            "job:run-18:test",
            &["job.launch"],
            300,
        );

        let agent_token = RunTokenMinter::new(live_s7.clone())
            .mint_run_token(
                &acme,
                &PrincipalId("svc:ci".into()),
                &RunId("job:run-18:test".into()),
                &agent("svc:ci", "acme"),
                &human("p:human", "acme"),
                &input(
                    &["job.launch"],
                    &["job.launch"],
                    &["job.launch"],
                    &["job.launch"],
                ),
                &caveats(&["job.launch"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:01Z"),
            )
            .unwrap();
        let structural =
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), live_s7.clone())
                .with_clock(|| ts("2026-06-19T00:01:00Z"));
        assert_eq!(
            structural.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-18:test",
                &agent_token,
                &["job.launch".into()],
            ),
            Err(CiJobAuthorizationError::CredentialVerificationRefused),
            "an Agent credential cannot be reinterpreted under the required `ci` scheme"
        );
        let per_job_token = RunTokenMinter::new(live_s7.clone())
            .mint_run_token(
                &acme,
                &PrincipalId("svc:ci".into()),
                &RunId("job:run-18:test".into()),
                &agent("svc:ci", "acme"),
                &human("p:human", "acme"),
                &input(
                    &["selfhosted:acme"],
                    &["selfhosted:acme"],
                    &["selfhosted:acme"],
                    &["selfhosted:acme"],
                ),
                &caveats(&["selfhosted:acme"]),
                MachineKind::PerJob,
                &ttl(300),
                &ts("2026-06-19T00:00:02Z"),
            )
            .unwrap();
        assert_eq!(
            structural.authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-18:test",
                &per_job_token,
                &["selfhosted:acme".into()],
            ),
            Err(CiJobAuthorizationError::CredentialVerificationRefused),
            "a PerJob credential cannot be reinterpreted under the required `ci` scheme"
        );
        let malformed = RunToken {
            token: "not-a-verified-token".into(),
            jti: "public-jti".into(),
        };
        let verification_error = structural
            .authorize_ci_job(
                &acme,
                &PrincipalId("svc:ci".into()),
                "job:run-18:test",
                &malformed,
                &["job.launch".into()],
            )
            .unwrap_err();
        assert_eq!(
            verification_error,
            CiJobAuthorizationError::CredentialVerificationRefused
        );
        assert!(!verification_error.to_string().contains(&malformed.token));

        for (kind, purpose, expected) in [
            (
                MachineKind::Agent,
                CredentialPurpose::CiJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongMachineKind {
                    actual: MachineKind::Agent,
                },
            ),
            (
                MachineKind::PerJob,
                CredentialPurpose::CiJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongMachineKind {
                    actual: MachineKind::PerJob,
                },
            ),
            (
                MachineKind::Pat,
                CredentialPurpose::CiJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongMachineKind {
                    actual: MachineKind::Pat,
                },
            ),
            (
                MachineKind::DeployKey,
                CredentialPurpose::CiJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongMachineKind {
                    actual: MachineKind::DeployKey,
                },
            ),
            (
                MachineKind::Ci,
                CredentialPurpose::PerJob {
                    run_id: "job:run-18:test".into(),
                },
                CiJobAuthorizationError::WrongCredentialPurpose,
            ),
            (
                MachineKind::Ci,
                CredentialPurpose::AgentRun {
                    run_id: "job:run-18:test".into(),
                    delegation_snapshot: Some(1),
                },
                CiJobAuthorizationError::WrongCredentialPurpose,
            ),
        ] {
            let fixed = fixed_capability(kind, purpose);
            let carrier = RunToken {
                token: "opaque".into(),
                jti: fixed.jti.clone(),
            };
            let authorizer = RunTokenAuthorizer::new(
                Arc::new(FixedCiSchemeVerifier(fixed)),
                RevocationStore::new(),
            );
            assert_eq!(
                authorizer.authorize_ci_job(
                    &acme,
                    &PrincipalId("svc:ci".into()),
                    "job:run-18:test",
                    &carrier,
                    &["job.launch".into()],
                ),
                Err(expected)
            );
        }

        let authorize_with = |s7: RevocationStore, now: &'static str| {
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), s7)
                .with_clock(move || ts(now))
                .authorize_ci_job(
                    &acme,
                    &PrincipalId("svc:ci".into()),
                    "job:run-18:test",
                    &ci_token,
                    &["job.launch".into()],
                )
        };
        assert_eq!(
            authorize_with(RevocationStore::new(), "2026-06-19T00:01:00Z"),
            Err(CiJobAuthorizationError::NotLive {
                state: RunTokenState::Unknown
            })
        );
        assert_eq!(
            authorize_with(live_s7.clone(), "2026-06-19T00:06:00Z"),
            Err(CiJobAuthorizationError::NotLive {
                state: RunTokenState::Expired
            })
        );

        let torn_down_s7 = RevocationStore::new();
        let torn_down = mint_ci_token(
            &torn_down_s7,
            &acme,
            "svc:ci",
            "job:run-19:test",
            &["job.launch"],
            300,
        );
        torn_down_s7.tear_down_run_token(&acme, &torn_down.jti, ts("2026-06-19T00:01:00Z"));
        assert_eq!(
            RunTokenAuthorizer::new(Arc::new(StructuralTokenVerifier::new()), torn_down_s7,)
                .with_clock(|| ts("2026-06-19T00:01:01Z"))
                .authorize_ci_job(
                    &acme,
                    &PrincipalId("svc:ci".into()),
                    "job:run-19:test",
                    &torn_down,
                    &["job.launch".into()],
                ),
            Err(CiJobAuthorizationError::NotLive {
                state: RunTokenState::TornDown
            })
        );

        let revoked_s7 = RevocationStore::new();
        revoked_s7.revoke(
            &acme,
            &RevokeTarget::Jti(ci_token.jti.clone()),
            ts("2026-06-19T00:00:30Z"),
        );
        assert_eq!(
            authorize_with(revoked_s7, "2026-06-19T00:01:00Z"),
            Err(CiJobAuthorizationError::NotLive {
                state: RunTokenState::TornDown
            })
        );
    }

    #[test]
    fn mint_applies_the_intersection_cannot_delegate_what_you_lack() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let inp = input(
            &["repo:acme/web#admin", "repo:acme/web#read"],
            &["repo:acme/web#admin", "repo:acme/web#read"],
            &["repo:acme/web#admin", "repo:acme/web#read"],
            &["repo:acme/web#read"],
        );
        let (token, proof) = minter
            .mint_proved(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-1".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &inp,
                &caveats(&["repo:acme/web#admin", "repo:acme/web#read"]),
                MachineKind::Agent,
                &ttl(60),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint succeeds");
        assert!(
            token.token.contains("repo:acme/web#read"),
            "the held grant is minted"
        );
        assert!(
            !token.token.contains("admin"),
            "a grant the delegator never held is NEVER minted into the token (the mint re-check)"
        );
        assert!(
            proof.holds(),
            "the minted authority is ⊆ every conjunct (monotone)"
        );
        assert_eq!(proof.effective, vec!["repo:acme/web#read".to_string()]);
    }

    #[test]
    fn self_hosted_runner_token_cannot_act_cross_tenant() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");

        let ok = input(
            &["selfhosted:acme"],
            &["selfhosted:acme"],
            &["selfhosted:acme"],
            &["selfhosted:acme"],
        );
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("svc:runner".into()),
                &RunId("run-1".into()),
                &agent("svc:runner", "acme"),
                &human("p:human", "acme"),
                &ok,
                &caveats(&["selfhosted:acme"]),
                MachineKind::PerJob,
                &ttl(60),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("own-tenant SelfHosted run token mints");
        assert!(token.token.contains("selfhosted:acme"));

        let cross = input(
            &["selfhosted:globex"],
            &["selfhosted:globex"],
            &["selfhosted:globex"],
            &["selfhosted:globex"],
        );
        let r = minter.mint_run_token(
            &acme,
            &PrincipalId("svc:runner".into()),
            &RunId("run-2".into()),
            &agent("svc:runner", "acme"),
            &human("p:human", "acme"),
            &cross,
            &caveats(&["selfhosted:globex"]),
            MachineKind::PerJob,
            &ttl(60),
            &ts("2026-06-19T00:00:00Z"),
        );
        assert_eq!(
            r,
            Err(MintError::SelfHostedScopeViolation(
                "selfhosted:globex".into()
            )),
            "a self-hosted run token naming another tenant's scope is refused (C6, no-global-pool)"
        );
    }

    #[test]
    fn re_mint_on_resume_is_fresh_and_recomputes_the_intersection() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let agent_id = PrincipalId("p:agent".into());
        let run = RunId("run-1".into());

        let dispatch = input(
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
        );
        let t0 = minter
            .mint_run_token(
                &acme,
                &agent_id,
                &run,
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &dispatch,
                &caveats(&["repo:acme/web#read", "repo:acme/web#write"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("dispatch mint");
        assert!(
            t0.token.contains("repo:acme/web#write"),
            "dispatch token carries #write"
        );

        let resume = input(
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read", "repo:acme/web#write"],
            &["repo:acme/web#read"],
        );
        let t1 = minter
            .re_mint_on_resume(
                &acme,
                &agent_id,
                &run,
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &resume,
                &caveats(&["repo:acme/web#read", "repo:acme/web#write"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-22T09:00:00Z"),
            )
            .expect("re-mint on resume");
        assert_ne!(
            t1.jti, t0.jti,
            "the re-mint is a FRESH token (distinct jti)"
        );
        assert!(
            t1.token.contains("repo:acme/web#read"),
            "the re-minted token keeps #read"
        );
        assert!(
            !t1.token.contains("#write"),
            "the re-minted token is NARROWER (the delegator lost #write - recomputed as-of-resume)"
        );
    }

    #[test]
    fn per_run_token_auto_expires_at_run_life() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-1".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &input(&["g"], &["g"], &["g"], &["g"]),
                &caveats(&["g"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        assert!(
            minter.is_live(&acme, &token, &ts("2026-06-19T00:02:00Z")),
            "the token is live within its run-life window"
        );
        assert!(
            !minter.is_live(&acme, &token, &ts("2026-06-19T00:06:00Z")),
            "the token auto-expires at run-life even if teardown is skipped (revoke-on-crash)"
        );
        assert_eq!(
            minter.revocation_state(&acme, &token, &ts("2026-06-19T00:06:00Z")),
            RunTokenState::Expired,
            "past run-life the token's state is Expired (the auto-expire)"
        );
    }

    #[test]
    fn killed_run_token_is_revoked_and_auto_expires() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-1".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &input(&["g"], &["g"], &["g"], &["g"]),
                &caveats(&["g"]),
                MachineKind::Agent,
                &ttl(300),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        assert!(minter.is_live(&acme, &token, &ts("2026-06-19T00:01:00Z")));
        minter.teardown(&acme, &token, &ts("2026-06-19T00:01:30Z"));
        assert!(
            !minter.is_live(&acme, &token, &ts("2026-06-19T00:01:31Z")),
            "after teardown the token is dead immediately (token-revocation-lag = 0)"
        );
        assert_eq!(
            minter.revocation_state(&acme, &token, &ts("2026-06-19T00:01:31Z")),
            RunTokenState::TornDown
        );
        assert!(!minter.is_live(&acme, &token, &ts("2026-06-19T00:06:00Z")));
    }

    #[test]
    fn zero_ttl_mint_is_refused() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let r = minter.mint_run_token(
            &acme,
            &PrincipalId("p:agent".into()),
            &RunId("run-1".into()),
            &agent("p:agent", "acme"),
            &human("p:human", "acme"),
            &input(&["g"], &["g"], &["g"], &["g"]),
            &caveats(&["g"]),
            MachineKind::Agent,
            &ttl(0),
            &ts("2026-06-19T00:00:00Z"),
        );
        assert_eq!(r, Err(MintError::NonPositiveTtl));
    }

    #[test]
    fn mint_writes_the_auto_expiring_run_grant_tuple() {
        let s7 = RevocationStore::new();
        let tuples = TupleStore::new(OutboxStore::new());
        let minter = RunTokenMinter::with_tuple_store(s7, tuples.clone());
        let acme = scope("acme");
        minter
            .mint_run_token(
                &acme,
                &PrincipalId("p:agent".into()),
                &RunId("run-77".into()),
                &agent("p:agent", "acme"),
                &human("p:human", "acme"),
                &input(&["g"], &["g"], &["g"], &["g"]),
                &caveats(&["g"]),
                MachineKind::Agent,
                &ttl(120),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        let stored = tuples.tuples_in(&acme);
        let grant = stored
            .iter()
            .find(|t| t.tuple.object.0 == "run:run-77")
            .expect("the auto-expiring per-run grant tuple was written");
        assert_eq!(grant.tuple.relation.0, RUN_GRANT_RELATION);
        assert_eq!(grant.tuple.subject.0, "p:agent");
        assert_eq!(
            grant.expires_at,
            Some(ts("2026-06-19T00:02:00Z")),
            "the per-run grant tuple's expires_at == run life (now + 120s)"
        );
    }

    #[test]
    fn expires_at_is_always_strictly_after_now() {
        assert_eq!(
            expires_at_of(&ts("2026-06-19T00:00:00Z"), &ttl(30)).0,
            "2026-06-19T00:00:30Z"
        );
        assert_eq!(
            expires_at_of(&ts("2026-06-19T00:00:45Z"), &ttl(30)).0,
            "2026-06-19T00:01:15Z"
        );
        assert_eq!(
            expires_at_of(&ts("2026-06-19T00:59:45Z"), &ttl(30)).0,
            "2026-06-19T01:00:15Z"
        );
        assert_eq!(
            expires_at_of(&ts("2026-06-30T23:59:50Z"), &ttl(20)).0,
            "2026-07-01T00:00:10Z"
        );
        for (now, secs) in [
            ("2026-06-19T00:00:00Z", 1u64),
            ("2026-12-31T23:59:59Z", 1),
            ("not-a-real-instant", 60),
        ] {
            let exp = expires_at_of(&ts(now), &ttl(secs));
            assert!(
                exp.0.as_str() > now,
                "expires_at ({}) must be strictly after now ({now})",
                exp.0
            );
        }
    }

    #[test]
    fn authoritative_mint_binds_the_resolved_run_and_snapshot() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let agent = agent("svc:agent", "acme");
        let trigger = human("p:human", "acme");
        let resolved = resolved_policy(
            "run-9",
            &agent,
            &trigger,
            input(
                &["repo.pull"],
                &["repo.pull"],
                &["repo.pull"],
                &["repo.pull"],
            ),
            42,
        );

        let token = minter
            .mint_from_resolved_policy(
                &acme,
                &agent.principal_id,
                &RunId("run-9".into()),
                &agent,
                &trigger,
                &resolved,
                &caveats(&["repo.pull"]),
                MachineKind::Agent,
                &ttl(60),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("authoritative mint");

        let parts: Vec<&str> = token.token.split('|').collect();
        assert_eq!(parts[8], "run-9");
        assert_eq!(parts[9], "42", "the durable snapshot is signed");
    }

    #[test]
    fn authoritative_mint_refuses_a_mismatched_resolved_binding() {
        let minter = RunTokenMinter::new(RevocationStore::new());
        let acme = scope("acme");
        let agent = agent("svc:agent", "acme");
        let trigger = human("p:human", "acme");
        let resolved = resolved_policy(
            "run-9",
            &agent,
            &trigger,
            input(
                &["repo.pull"],
                &["repo.pull"],
                &["repo.pull"],
                &["repo.pull"],
            ),
            42,
        );

        let result = minter.mint_from_resolved_policy(
            &acme,
            &agent.principal_id,
            &RunId("run-other".into()),
            &agent,
            &trigger,
            &resolved,
            &caveats(&["repo.pull"]),
            MachineKind::Agent,
            &ttl(60),
            &ts("2026-06-19T00:00:00Z"),
        );

        assert_eq!(result, Err(MintError::ResolvedPolicyBindingMismatch));
    }

    #[test]
    fn minted_token_uses_the_authenticate_envelope_shape() {
        let s7 = RevocationStore::new();
        let minter = RunTokenMinter::new(s7);
        let acme = scope("acme");
        let token = minter
            .mint_run_token(
                &acme,
                &PrincipalId("svc:agent".into()),
                &RunId("run-9".into()),
                &agent("svc:agent", "acme"),
                &human("p:human", "acme"),
                &input(
                    &["agent:run"],
                    &["agent:run"],
                    &["agent:run"],
                    &["agent:run"],
                ),
                &caveats(&["agent:run"]),
                MachineKind::Agent,
                &ttl(60),
                &ts("2026-06-19T00:00:00Z"),
            )
            .expect("mint");
        let parts: Vec<&str> = token.token.split('|').collect();
        assert_eq!(parts.len(), 10, "the envelope has all signed-fact fields");
        assert_eq!(parts[0], "acme", "tenant from the verified scope");
        assert_eq!(parts[1], "eu-west", "region from the verified scope");
        assert_eq!(parts[2], "svc:agent", "subject_key is the agent id");
        assert_eq!(
            parts[3], token.jti,
            "the envelope jti matches the RunToken jti"
        );
        assert_eq!(
            parts[4], "0",
            "a per-run token is dpop=0 (TTL-constrained, not DPoP-bound)"
        );
        assert_eq!(
            parts[5], "agent:run",
            "the grants are the attenuated effective authority"
        );
        assert_eq!(parts[6], "agent_run");
        assert_eq!(parts[7], "edge");
        assert_eq!(parts[8], "run-9");
        assert_eq!(
            parts[9], "",
            "caller-supplied legacy mint has no durable policy snapshot"
        );
    }
}
