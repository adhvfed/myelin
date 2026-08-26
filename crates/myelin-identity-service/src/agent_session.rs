use chrono::{DateTime, Duration, SecondsFormat, TimeZone, Utc};
use myelin_events::Timestamp;
use myelin_identity::{
    DataRole, DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind,
    PrincipalStatus, RunId, RunToken, RuntimeRef,
};
use myelin_storage::{
    ClaimedExternalAgentRun, DurableDelegationPolicyBacking, DurableExternalAgentRun,
    DurableExternalAgentRunBacking, ExternalAgentRunState, ProviderError, SubstrateProvider,
    TenantScope,
};
use myelin_tenancy::ArtifactRef;
use uuid::Uuid;

use crate::delegation::authority_of;
use crate::{
    run_token_jti, AgentRegistration, AgentRegistryError, Authority, DelegationPolicySource,
    MachineKind, MintError, PgAgentRegistry, StoreBackedCheck, EXTERNAL_MCP_RUNTIME,
};

pub const MAX_EXTERNAL_AGENT_RUN_TTL_SECS: u64 = 5 * 60;

#[derive(Clone)]
pub struct AgentSessionIssuer {
    agents: PgAgentRegistry,
    policies: DelegationPolicySource,
    runs: DurableExternalAgentRunBacking,
    identity: StoreBackedCheck,
    ttl_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSessionRequest {
    pub agent_id: String,
    pub client_nonce: String,
    pub trigger_credential_jti: String,
    pub trigger_expires_at_unix: i64,
    pub trigger_authority: Authority,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    pub run_id: String,
    pub agent_id: String,
    pub agent_principal_id: String,
    pub trigger_actor_id: String,
    pub selected_tools: Vec<String>,
    pub effective_grants: Vec<String>,
    pub issued_at: String,
    pub expires_at: String,
    pub state: ExternalAgentRunState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuedAgentSession {
    pub session: AgentSession,
    pub run_token: RunToken,
    pub created: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedAgentSession {
    pub run_id: String,
    pub agent_id: String,
    pub state: ExternalAgentRunState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedAgentSession {
    pub run_id: String,
    pub agent_id: String,
    pub token_jti: String,
}

struct MintedAgentRun {
    token: RunToken,
    effective_grants: Vec<String>,
}

#[derive(Clone, Copy)]
struct AgentRunLifetime {
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    not_after: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSessionError {
    BadInput(String),
    NotFound,
    RunNotFound,
    Conflict(String),
    Policy(String),
    Expired,
    Storage(String),
}

impl core::fmt::Display for AgentSessionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadInput(reason) => write!(formatter, "invalid agent session: {reason}"),
            Self::NotFound => formatter.write_str("agent not found"),
            Self::RunNotFound => formatter.write_str("agent run not found"),
            Self::Conflict(reason) => write!(formatter, "agent session conflict: {reason}"),
            Self::Policy(reason) => write!(formatter, "agent session policy refused: {reason}"),
            Self::Expired => formatter.write_str("agent session has expired"),
            Self::Storage(reason) => write!(formatter, "agent session storage failed: {reason}"),
        }
    }
}

impl std::error::Error for AgentSessionError {}

impl AgentSessionIssuer {
    pub fn new(
        provider: SubstrateProvider,
        identity: StoreBackedCheck,
        ttl_secs: u64,
    ) -> Result<Self, AgentSessionError> {
        if ttl_secs == 0 || ttl_secs > MAX_EXTERNAL_AGENT_RUN_TTL_SECS {
            return Err(AgentSessionError::BadInput(format!(
                "external agent run TTL must be between 1 and {MAX_EXTERNAL_AGENT_RUN_TTL_SECS} seconds"
            )));
        }
        Ok(Self {
            agents: PgAgentRegistry::new(provider.clone()),
            policies: DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(
                provider.clone(),
            )),
            runs: DurableExternalAgentRunBacking::new(provider),
            identity,
            ttl_secs,
        })
    }

    pub async fn start(
        &self,
        actor: &Principal,
        request: AgentSessionRequest,
    ) -> Result<IssuedAgentSession, AgentSessionError> {
        self.start_before(actor, request, None).await
    }

    pub async fn start_until(
        &self,
        actor: &Principal,
        request: AgentSessionRequest,
        not_after: DateTime<Utc>,
    ) -> Result<IssuedAgentSession, AgentSessionError> {
        self.start_before(actor, request, Some(not_after)).await
    }

    async fn start_before(
        &self,
        actor: &Principal,
        request: AgentSessionRequest,
        not_after: Option<DateTime<Utc>>,
    ) -> Result<IssuedAgentSession, AgentSessionError> {
        require_active_human(actor)?;
        let agent_id = canonical_uuid("agent id", &request.agent_id)?;
        validate_nonce(&request.client_nonce)?;
        validate_trigger_credential(&request)?;
        let now = truncate_to_seconds(request.now)?;
        let not_after = not_after.map(truncate_to_seconds).transpose()?;
        let expires_at = run_expiry(&request, now, self.ttl_secs, not_after)?;
        let lifetime = AgentRunLifetime {
            issued_at: now,
            expires_at,
            not_after,
        };

        let registration = self
            .agents
            .get(actor, &agent_id.to_string())
            .await
            .map_err(map_registry_error)?;
        if registration.created_by != actor.principal_id.0
            || registration.runtime_ref != EXTERNAL_MCP_RUNTIME
        {
            return Err(AgentSessionError::NotFound);
        }
        match registration.status {
            PrincipalStatus::Active => {}
            PrincipalStatus::Suspended => {
                return Err(AgentSessionError::Conflict(
                    "agent is suspended; resume it before starting new work".into(),
                ));
            }
            PrincipalStatus::Disabled => {
                return Err(AgentSessionError::Conflict(
                    "agent is retired and cannot start new work".into(),
                ));
            }
        }

        let claimed = self
            .claim(actor, &registration, agent_id, &request, lifetime)
            .await?;
        let minted = self
            .mint_claimed(actor, &registration, &claimed.run)
            .await?;
        let ready = self
            .runs
            .mark_ready(
                &actor.tenant.0,
                canonical_uuid("run id", &claimed.run.run_id)?,
            )
            .await
            .map_err(storage_error)?;

        Ok(IssuedAgentSession {
            session: AgentSession {
                run_id: ready.run_id,
                agent_id: registration.id,
                agent_principal_id: registration.principal_id,
                trigger_actor_id: actor.principal_id.0.clone(),
                selected_tools: registration.tools,
                effective_grants: minted.effective_grants,
                issued_at: ready.issued_at,
                expires_at: ready.expires_at,
                state: ready.state,
            },
            run_token: minted.token,
            created: claimed.created,
        })
    }

    pub async fn close(
        &self,
        actor: &Principal,
        run_id: &str,
        token_jti: &str,
    ) -> Result<ClosedAgentSession, AgentSessionError> {
        let agent_id = external_agent_id(actor)?;
        let run_id = canonical_uuid("run id", run_id)?;
        validate_token_jti(token_jti)?;
        let closed = self
            .runs
            .close(&actor.tenant.0, run_id, agent_id, token_jti)
            .await
            .map_err(storage_error)?
            .ok_or(AgentSessionError::RunNotFound)?;
        Ok(ClosedAgentSession {
            run_id: closed.run_id,
            agent_id: closed.agent_id,
            state: closed.state,
        })
    }

    /// Revoke a run whose credential was minted but could not be returned because a later
    /// orchestration step failed. This keeps a partial HTTP request from leaving usable authority
    /// behind even though the bearer never reached its intended caller.
    pub async fn revoke_unreturned(
        &self,
        actor: &Principal,
        issued: &IssuedAgentSession,
    ) -> Result<ClosedAgentSession, AgentSessionError> {
        require_active_human(actor)?;
        if issued.session.trigger_actor_id != actor.principal_id.0 {
            return Err(AgentSessionError::RunNotFound);
        }
        let run_id = canonical_uuid("run id", &issued.session.run_id)?;
        let agent_id = canonical_uuid("agent id", &issued.session.agent_id)?;
        let terminal = self
            .runs
            .terminate(&actor.tenant.0, run_id, agent_id, &issued.run_token.jti)
            .await
            .map_err(storage_error)?
            .ok_or(AgentSessionError::RunNotFound)?;
        Ok(ClosedAgentSession {
            run_id: terminal.run_id,
            agent_id: terminal.agent_id,
            state: terminal.state,
        })
    }

    pub async fn authorize(
        &self,
        actor: &Principal,
        run_id: &str,
        token_jti: &str,
        now: DateTime<Utc>,
    ) -> Result<AuthorizedAgentSession, AgentSessionError> {
        let agent_id = external_agent_id(actor)?;
        let run_id = canonical_uuid("run id", run_id)?;
        validate_token_jti(token_jti)?;
        let ready = self
            .runs
            .find_ready(&actor.tenant.0, run_id, agent_id, token_jti, now)
            .await
            .map_err(storage_error)?
            .ok_or(AgentSessionError::RunNotFound)?;
        Ok(AuthorizedAgentSession {
            run_id: ready.run_id,
            agent_id: ready.agent_id,
            token_jti: ready.token_jti,
        })
    }

    pub async fn terminate(
        &self,
        actor: &Principal,
        run_id: &str,
        token_jti: &str,
    ) -> Result<ClosedAgentSession, AgentSessionError> {
        let agent_id = external_agent_id(actor)?;
        let run_id = canonical_uuid("run id", run_id)?;
        validate_token_jti(token_jti)?;
        let terminal = self
            .runs
            .terminate(&actor.tenant.0, run_id, agent_id, token_jti)
            .await
            .map_err(storage_error)?
            .ok_or(AgentSessionError::RunNotFound)?;
        Ok(ClosedAgentSession {
            run_id: terminal.run_id,
            agent_id: terminal.agent_id,
            state: terminal.state,
        })
    }

    async fn claim(
        &self,
        actor: &Principal,
        registration: &AgentRegistration,
        agent_id: Uuid,
        request: &AgentSessionRequest,
        lifetime: AgentRunLifetime,
    ) -> Result<ClaimedExternalAgentRun, AgentSessionError> {
        let run_id = Uuid::new_v4();
        let agent_principal_id = PrincipalId(registration.principal_id.clone());
        let issued_at_stamp = timestamp(lifetime.issued_at);
        let proposed_jti = run_token_jti(
            &agent_principal_id,
            &RunId(run_id.to_string()),
            &issued_at_stamp,
        );
        let trigger_authority = request
            .trigger_authority
            .grants()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let claimed = self
            .runs
            .claim(
                &actor.tenant.0,
                agent_id,
                &actor.principal_id.0,
                &request.trigger_credential_jti,
                &trigger_authority,
                &request.client_nonce,
                run_id,
                &proposed_jti,
                lifetime.issued_at,
                lifetime.expires_at,
            )
            .await
            .map_err(storage_error)?;
        validate_claim(
            actor,
            agent_id,
            &request.trigger_authority,
            &claimed.run,
            lifetime.issued_at,
            lifetime.not_after,
        )?;
        Ok(claimed)
    }

    async fn mint_claimed(
        &self,
        actor: &Principal,
        registration: &AgentRegistration,
        claimed: &DurableExternalAgentRun,
    ) -> Result<MintedAgentRun, AgentSessionError> {
        let agent_principal_id = PrincipalId(registration.principal_id.clone());
        let run_id = RunId(claimed.run_id.clone());
        let agent = Principal::new(
            actor.tenant.clone(),
            actor.region.clone(),
            agent_principal_id.clone(),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef(registration.runtime_ref.clone()),
                on_behalf_of: Some(actor.principal_id.clone()),
            },
            DataRole::Controller,
            registration.status,
        );
        let ceiling = Authority::of(claimed.trigger_authority.iter().cloned());
        let scope = TenantScope::from_verified_token(actor, actor.region.clone());
        let resolved = self
            .policies
            .resolve_for_run(&scope, &agent, actor, &run_id)
            .await
            .map_err(|error| AgentSessionError::Policy(error.to_string()))?
            .attenuate(&ceiling);
        let caveats = DelegationCaveats(
            resolved
                .input()
                .delegation
                .grants()
                .map(str::to_string)
                .collect(),
        );
        let issued_at_datetime = parse_datetime("issued_at", &claimed.issued_at)?;
        let issued_at = timestamp(issued_at_datetime);
        let expires_at = parse_datetime("expires_at", &claimed.expires_at)?;
        let ttl_secs = expires_at
            .timestamp()
            .checked_sub(issued_at_datetime.timestamp())
            .and_then(|seconds| u64::try_from(seconds).ok())
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| AgentSessionError::Storage("durable run lifetime is invalid".into()))?;
        let run_token = self
            .identity
            .mint_run_token_from_resolved_policy_in(
                &scope,
                &agent_principal_id,
                &run_id,
                &agent,
                actor,
                &resolved,
                &caveats,
                MachineKind::Agent,
                &FailStaticBound {
                    static_max_secs: ttl_secs,
                },
                &issued_at,
            )
            .map_err(|error| match error {
                MintError::RevocationUnavailable | MintError::RunGrantUnavailable => {
                    AgentSessionError::Storage(error.to_string())
                }
                error => AgentSessionError::Policy(error.to_string()),
            })?;
        if run_token.jti != claimed.token_jti {
            self.identity
                .tear_down_run_token_in(&scope, &run_token)
                .map_err(|error| {
                    AgentSessionError::Storage(format!(
                        "mismatched token identity could not be revoked: {error}"
                    ))
                })?;
            return Err(AgentSessionError::Storage(
                "replayed agent session minted a different token identity".into(),
            ));
        }
        Ok(MintedAgentRun {
            token: run_token,
            effective_grants: authority_of(resolved.effective_policy())
                .grants()
                .map(str::to_string)
                .collect(),
        })
    }
}

pub fn agent_run_ref(tenant: &str, run_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/agent/run/{run_id}"))
}

fn run_expiry(
    request: &AgentSessionRequest,
    issued_at: DateTime<Utc>,
    configured_ttl_secs: u64,
    not_after: Option<DateTime<Utc>>,
) -> Result<DateTime<Utc>, AgentSessionError> {
    let remaining_secs = request
        .trigger_expires_at_unix
        .checked_sub(issued_at.timestamp())
        .ok_or_else(|| {
            AgentSessionError::BadInput("trigger credential expiry overflowed".into())
        })?;
    let remaining_secs = u64::try_from(remaining_secs)
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or(AgentSessionError::Expired)?;
    let ttl_secs = configured_ttl_secs.min(remaining_secs);
    let configured_expiry = issued_at
        .checked_add_signed(Duration::seconds(ttl_secs as i64))
        .ok_or_else(|| AgentSessionError::BadInput("agent session expiry overflowed".into()))?;
    let expires_at = not_after
        .map(|bound| configured_expiry.min(bound))
        .unwrap_or(configured_expiry);
    if expires_at <= issued_at {
        return Err(AgentSessionError::Expired);
    }
    Ok(expires_at)
}

fn validate_claim(
    actor: &Principal,
    requested_agent_id: Uuid,
    current_authority: &Authority,
    run: &DurableExternalAgentRun,
    now: DateTime<Utc>,
    not_after: Option<DateTime<Utc>>,
) -> Result<(), AgentSessionError> {
    if run.agent_id != requested_agent_id.to_string()
        || run.trigger_actor_id != actor.principal_id.0
    {
        return Err(AgentSessionError::Conflict(
            "idempotency key was already used for a different agent run".into(),
        ));
    }
    if !run
        .trigger_authority
        .iter()
        .all(|grant| current_authority.holds(grant))
    {
        return Err(AgentSessionError::Policy(
            "the current human session no longer holds the authority used to start this run".into(),
        ));
    }
    if matches!(
        run.state,
        ExternalAgentRunState::Closed | ExternalAgentRunState::Terminal
    ) {
        return Err(AgentSessionError::Conflict(
            "idempotency key belongs to a finished agent run".into(),
        ));
    }
    let durable_expires_at = parse_datetime("expires_at", &run.expires_at)?;
    if not_after.is_some_and(|bound| durable_expires_at > bound) {
        return Err(AgentSessionError::Conflict(
            "idempotency key belongs to an agent run outside the requested lifetime".into(),
        ));
    }
    if now >= durable_expires_at {
        return Err(AgentSessionError::Expired);
    }
    Ok(())
}

fn require_active_human(actor: &Principal) -> Result<(), AgentSessionError> {
    if actor.status != PrincipalStatus::Active || !matches!(actor.kind, PrincipalKind::Human) {
        return Err(AgentSessionError::NotFound);
    }
    Ok(())
}

fn external_agent_id(actor: &Principal) -> Result<Uuid, AgentSessionError> {
    if actor.status != PrincipalStatus::Active
        || !matches!(
            &actor.kind,
            PrincipalKind::Agent { runtime_ref, .. } if runtime_ref.0 == EXTERNAL_MCP_RUNTIME
        )
    {
        return Err(AgentSessionError::NotFound);
    }
    actor
        .principal_id
        .0
        .strip_prefix("agent:")
        .and_then(|id| canonical_uuid("agent principal id", id).ok())
        .ok_or(AgentSessionError::NotFound)
}

fn validate_trigger_credential(request: &AgentSessionRequest) -> Result<(), AgentSessionError> {
    validate_token_jti(&request.trigger_credential_jti)?;
    if request.trigger_authority.is_empty() {
        return Err(AgentSessionError::Policy(
            "trigger credential has no delegable authority".into(),
        ));
    }
    if request.trigger_authority.grants().count() > 512 {
        return Err(AgentSessionError::BadInput(
            "trigger credential authority exceeds 512 grants".into(),
        ));
    }
    Ok(())
}

fn validate_token_jti(value: &str) -> Result<(), AgentSessionError> {
    if value.is_empty() || value.len() > 512 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AgentSessionError::BadInput(
            "credential identity is malformed".into(),
        ));
    }
    Ok(())
}

fn validate_nonce(value: &str) -> Result<(), AgentSessionError> {
    if value.is_empty() || value.len() > 128 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AgentSessionError::BadInput(
            "idempotency nonce must be 1..128 ASCII-graphic bytes".into(),
        ));
    }
    Ok(())
}

fn canonical_uuid(label: &str, value: &str) -> Result<Uuid, AgentSessionError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| AgentSessionError::BadInput(format!("{label} must be a canonical UUID")))?;
    if parsed.to_string() != value {
        return Err(AgentSessionError::BadInput(format!(
            "{label} must be a canonical UUID"
        )));
    }
    Ok(parsed)
}

fn truncate_to_seconds(value: DateTime<Utc>) -> Result<DateTime<Utc>, AgentSessionError> {
    Utc.timestamp_opt(value.timestamp(), 0)
        .single()
        .ok_or_else(|| AgentSessionError::BadInput("agent session time is invalid".into()))
}

fn timestamp(value: DateTime<Utc>) -> Timestamp {
    Timestamp(value.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn parse_datetime(label: &str, value: &str) -> Result<DateTime<Utc>, AgentSessionError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AgentSessionError::Storage(format!("durable {label} is malformed")))
}

fn map_registry_error(error: AgentRegistryError) -> AgentSessionError {
    match error {
        AgentRegistryError::BadInput(reason) => AgentSessionError::BadInput(reason),
        AgentRegistryError::NotFound => AgentSessionError::NotFound,
        AgentRegistryError::Conflict(reason) => AgentSessionError::Conflict(reason),
        AgentRegistryError::Policy(reason) => AgentSessionError::Policy(reason),
        AgentRegistryError::Storage(reason) => AgentSessionError::Storage(reason),
    }
}

fn storage_error(error: ProviderError) -> AgentSessionError {
    AgentSessionError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalKind, RuntimeRef};
    use myelin_tenancy::{Region, TenantId};

    fn human() -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("human:ada".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    #[test]
    fn replay_never_widens_the_original_trigger_authority() {
        let run = DurableExternalAgentRun {
            run_id: "11111111-1111-1111-1111-111111111111".into(),
            agent_id: "22222222-2222-2222-2222-222222222222".into(),
            trigger_actor_id: "human:ada".into(),
            trigger_credential_jti: "session-original".into(),
            trigger_authority: vec!["repo.pull".into(), "run.view".into()],
            client_nonce: "retry-safe".into(),
            token_jti: "redacted".into(),
            state: ExternalAgentRunState::Ready,
            issued_at: "2026-08-09T12:00:00+00:00".into(),
            expires_at: "2026-08-09T12:05:00+00:00".into(),
        };
        let now = DateTime::parse_from_rfc3339("2026-08-09T12:01:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(validate_claim(
            &human(),
            Uuid::parse_str(&run.agent_id).unwrap(),
            &Authority::of(["repo.pull"]),
            &run,
            now,
            None,
        )
        .is_err());
        assert!(validate_claim(
            &human(),
            Uuid::parse_str(&run.agent_id).unwrap(),
            &Authority::of(["repo.pull", "run.view", "repo.push"]),
            &run,
            now,
            None,
        )
        .is_ok());
    }

    #[test]
    fn a_thread_boundary_caps_the_run_without_extending_the_trigger_session() {
        let now = DateTime::parse_from_rfc3339("2026-08-09T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let request = AgentSessionRequest {
            agent_id: "22222222-2222-2222-2222-222222222222".into(),
            client_nonce: "thread-run".into(),
            trigger_credential_jti: "browser-session".into(),
            trigger_expires_at_unix: (now + Duration::minutes(10)).timestamp(),
            trigger_authority: Authority::of(["agent.run"]),
            now,
        };

        assert_eq!(
            run_expiry(&request, now, 300, Some(now + Duration::seconds(45))).unwrap(),
            now + Duration::seconds(45)
        );
        assert!(matches!(
            run_expiry(&request, now, 300, Some(now)),
            Err(AgentSessionError::Expired)
        ));
    }

    #[test]
    fn only_live_humans_can_start_external_runs() {
        let mut agent = human();
        agent.kind = PrincipalKind::Agent {
            runtime_ref: RuntimeRef("external:mcp".into()),
            on_behalf_of: Some(PrincipalId("human:ada".into())),
        };
        assert!(matches!(
            require_active_human(&agent),
            Err(AgentSessionError::NotFound)
        ));
        assert_eq!(
            agent_run_ref("acme", "run-1").0,
            "myelin://acme/agent/run/run-1"
        );
    }

    #[test]
    fn only_a_canonical_live_external_agent_can_close_its_run() {
        let id = "22222222-2222-2222-2222-222222222222";
        let mut agent = human();
        agent.principal_id = PrincipalId(format!("agent:{id}"));
        agent.kind = PrincipalKind::Agent {
            runtime_ref: RuntimeRef(EXTERNAL_MCP_RUNTIME.into()),
            on_behalf_of: Some(PrincipalId("human:ada".into())),
        };
        assert_eq!(external_agent_id(&agent).unwrap().to_string(), id);

        agent.principal_id = PrincipalId("agent:not-a-uuid".into());
        assert!(matches!(
            external_agent_id(&agent),
            Err(AgentSessionError::NotFound)
        ));
    }
}
