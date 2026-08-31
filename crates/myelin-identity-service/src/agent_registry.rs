use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole as EventDataRole, EmitContext, EventDraft,
    EventId, EventType, IdMinter, Timestamp, UlidMinter, Visibility,
};
use myelin_identity::{
    DataRole, Principal, PrincipalKind, PrincipalStatus, RuntimeRef, IDENTITY_AGENT_CREATED,
};
use myelin_storage::{
    ensure_agent_policy_bundle_on_conn, DurableDelegationPolicyError,
    DurableDelegationPolicyRevisions, DurableDelegationPolicyVersions, EncryptedColumn, KeyClass,
    PgError, PiiKeyRef, SubjectKeyScope, SubstrateProvider, NONCE_LEN,
};
use myelin_tenancy::ArtifactRef;
use sqlx::Row;
use uuid::Uuid;

mod lifecycle;

pub use lifecycle::{AgentLifecycleAction, AgentLifecycleOutcome, AgentLifecycleRequest};

pub const EXTERNAL_MCP_RUNTIME: &str = "external:mcp";
pub const HOSTED_LUNA_RUNTIME: &str = "hosted:luna";
pub const MAX_AGENT_NAME_BYTES: usize = 80;
pub const MAX_AGENT_TOOLS: usize = 128;
pub const MAX_AGENT_WORKSPACE_GUIDANCE_BYTES: usize = 12 * 1024;
const MAX_AGENT_LIST_ROWS: u32 = 101;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentWorkspaceGuidance {
    pub content_digest: String,
    pub encrypted_content: EncryptedColumn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAgent {
    pub agent_id: Uuid,
    pub name: String,
    pub runtime_ref: String,
    pub tools: Vec<String>,
    pub grants: Vec<String>,
    pub workspace_guidance: Option<AgentWorkspaceGuidance>,
    pub tenant_policy_if_missing: Vec<String>,
    pub trigger_actor_policy_if_missing: Vec<String>,
    pub client_nonce: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRegistration {
    pub id: String,
    pub principal_id: String,
    pub name: String,
    pub runtime_ref: String,
    pub created_by: String,
    pub tools: Vec<String>,
    pub grants: Vec<String>,
    pub workspace_guidance_revision: u64,
    pub workspace_guidance: Option<AgentWorkspaceGuidance>,
    pub workspace_guidance_updated_at: Option<String>,
    pub status: PrincipalStatus,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentActivation {
    pub agent: AgentRegistration,
    pub created: bool,
    pub policy_versions: DurableDelegationPolicyVersions,
    pub policy_revisions: DurableDelegationPolicyRevisions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentWorkspaceGuidanceUpdate {
    pub agent_id: String,
    pub expected_revision: u64,
    pub guidance: Option<AgentWorkspaceGuidance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentWorkspaceGuidanceOutcome {
    pub agent: AgentRegistration,
    pub changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRegistryError {
    BadInput(String),
    NotFound,
    Conflict(String),
    Policy(String),
    Storage(String),
}

impl core::fmt::Display for AgentRegistryError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadInput(reason) => write!(formatter, "invalid agent: {reason}"),
            Self::NotFound => formatter.write_str("agent not found"),
            Self::Conflict(reason) => write!(formatter, "agent conflict: {reason}"),
            Self::Policy(reason) => write!(formatter, "agent policy refused activation: {reason}"),
            Self::Storage(reason) => write!(formatter, "agent registry storage failed: {reason}"),
        }
    }
}

impl std::error::Error for AgentRegistryError {}

#[derive(Clone)]
pub struct PgAgentRegistry {
    provider: SubstrateProvider,
    event_ids: Arc<dyn IdMinter>,
}

impl PgAgentRegistry {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self::with_minter(provider, Arc::new(UlidMinter::new()))
    }

    pub fn with_minter(provider: SubstrateProvider, event_ids: Arc<dyn IdMinter>) -> Self {
        Self {
            provider,
            event_ids,
        }
    }

    pub async fn create(
        &self,
        actor: &Principal,
        mut proposal: NewAgent,
    ) -> Result<AgentActivation, AgentRegistryError> {
        canonicalize(&mut proposal.tools);
        canonicalize(&mut proposal.grants);
        canonicalize(&mut proposal.tenant_policy_if_missing);
        canonicalize(&mut proposal.trigger_actor_policy_if_missing);
        validate_new_agent(actor, &proposal)?;
        self.require_local_region(actor)?;

        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        let actor_id = actor.principal_id.0.clone();
        let agent_id = proposal.agent_id;
        let principal_id = agent_principal_id(agent_id);
        let created_at = Utc::now();
        let event = agent_created_event(
            actor,
            agent_id,
            &proposal.runtime_ref,
            &proposal.tools,
            EventId(self.event_ids.mint().0),
            created_at,
        );
        let proposal_for_tx = proposal.clone();

        let outcome = self
            .provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    lock_agent_creation(
                        conn,
                        &tenant,
                        &region,
                        &actor_id,
                        &proposal_for_tx.client_nonce,
                        &proposal_for_tx.name,
                    )
                    .await?;

                    if let Some(existing) = agent_by_nonce(
                        conn,
                        &tenant,
                        &region,
                        &actor_id,
                        &proposal_for_tx.client_nonce,
                    )
                    .await?
                    {
                        if !same_intent(&existing, &proposal_for_tx) {
                            return Ok(CreateTx::NonceConflict);
                        }
                        return match ensure_policies(
                            conn,
                            &tenant,
                            &region,
                            &existing.principal_id,
                            &actor_id,
                            &proposal_for_tx,
                        )
                        .await?
                        {
                            Ok((versions, revisions)) => Ok(CreateTx::Existing {
                                agent: existing,
                                versions,
                                revisions,
                            }),
                            Err(error) => Ok(CreateTx::Policy(error)),
                        };
                    }

                    if agent_name_exists(conn, &tenant, &region, &proposal_for_tx.name).await? {
                        return Ok(CreateTx::NameConflict);
                    }

                    let (versions, revisions) = match ensure_policies(
                        conn,
                        &tenant,
                        &region,
                        &principal_id,
                        &actor_id,
                        &proposal_for_tx,
                    )
                    .await?
                    {
                        Ok(cursors) => cursors,
                        Err(error) => return Ok(CreateTx::Policy(error)),
                    };

                    let principal_kind = serde_json::to_string(&PrincipalKind::Agent {
                        runtime_ref: RuntimeRef(proposal_for_tx.runtime_ref.clone()),
                        on_behalf_of: Some(myelin_identity::PrincipalId(actor_id.clone())),
                    })
                    .expect("agent principal kind serializes");
                    sqlx::query(
                        "INSERT INTO principal (\
                           tenant_id, region, principal_id, kind, data_role, status\
                         ) VALUES ($1, $2, $3, $4, $5, $6)",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&principal_id)
                    .bind(principal_kind)
                    .bind(
                        serde_json::to_string(&DataRole::Controller).expect("data role serializes"),
                    )
                    .bind(
                        serde_json::to_string(&PrincipalStatus::Active)
                            .expect("principal status serializes"),
                    )
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("insert agent principal"))?;

                    let row = sqlx::query(
                        "INSERT INTO identity_agent (\
                           tenant_id, region, agent_id, name, runtime_ref, created_by, \
                           client_nonce, tools, grants, workspace_guidance_revision, \
                           workspace_guidance_digest, workspace_guidance_key_ref, \
                           workspace_guidance_nonce, workspace_guidance_ciphertext, \
                           workspace_guidance_updated_at, created_at\
                         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
                         RETURNING tenant_id, agent_id, name, runtime_ref, created_by, tools, grants, \
                           workspace_guidance_revision, workspace_guidance_digest, \
                           workspace_guidance_key_ref, workspace_guidance_nonce, \
                           workspace_guidance_ciphertext, workspace_guidance_updated_at, created_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(agent_id)
                    .bind(&proposal_for_tx.name)
                    .bind(&proposal_for_tx.runtime_ref)
                    .bind(&actor_id)
                    .bind(&proposal_for_tx.client_nonce)
                    .bind(&proposal_for_tx.tools)
                    .bind(&proposal_for_tx.grants)
                    .bind(i64::from(proposal_for_tx.workspace_guidance.is_some()))
                    .bind(
                        proposal_for_tx
                            .workspace_guidance
                            .as_ref()
                            .map(|guidance| guidance.content_digest.as_str()),
                    )
                    .bind(
                        proposal_for_tx
                            .workspace_guidance
                            .as_ref()
                            .map(|guidance| guidance.encrypted_content.key_ref.to_uri()),
                    )
                    .bind(
                        proposal_for_tx
                            .workspace_guidance
                            .as_ref()
                            .map(|guidance| guidance.encrypted_content.nonce.to_vec()),
                    )
                    .bind(
                        proposal_for_tx
                            .workspace_guidance
                            .as_ref()
                            .map(|guidance| guidance.encrypted_content.ciphertext.as_slice()),
                    )
                    .bind(proposal_for_tx.workspace_guidance.as_ref().map(|_| created_at))
                    .bind(created_at)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(query_error("insert agent registration"))?;
                    let agent = agent_from_row(&row, PrincipalStatus::Active)?;
                    myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
                        conn,
                        &event.aggregate.0,
                        &event,
                    )
                    .await?;
                    Ok(CreateTx::Created {
                        agent,
                        versions,
                        revisions,
                    })
                })
            })
            .await
            .map_err(|error| AgentRegistryError::Storage(error.to_string()))?;

        match outcome {
            CreateTx::Created {
                agent,
                versions,
                revisions,
            } => Ok(AgentActivation {
                agent,
                created: true,
                policy_versions: versions,
                policy_revisions: revisions,
            }),
            CreateTx::Existing {
                agent,
                versions,
                revisions,
            } => Ok(AgentActivation {
                agent,
                created: false,
                policy_versions: versions,
                policy_revisions: revisions,
            }),
            CreateTx::NonceConflict => Err(AgentRegistryError::Conflict(
                "that idempotency key was already used for a different agent".into(),
            )),
            CreateTx::NameConflict => Err(AgentRegistryError::Conflict(format!(
                "an agent named `{}` already exists in this organization",
                proposal.name
            ))),
            CreateTx::Policy(error) => Err(AgentRegistryError::Policy(error.to_string())),
        }
    }

    pub async fn get(
        &self,
        actor: &Principal,
        agent_id: &str,
    ) -> Result<AgentRegistration, AgentRegistryError> {
        self.require_local_region(actor)?;
        let agent_id = parse_agent_id(agent_id)?;
        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move { agent_by_id(conn, &tenant, &region, agent_id).await })
            })
            .await
            .map_err(|error| AgentRegistryError::Storage(error.to_string()))?
            .ok_or(AgentRegistryError::NotFound)
    }

    pub async fn list(
        &self,
        actor: &Principal,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AgentRegistration>, AgentRegistryError> {
        self.require_local_region(actor)?;
        if limit == 0 || limit > MAX_AGENT_LIST_ROWS {
            return Err(AgentRegistryError::BadInput(
                "agent store row limit must be between 1 and 101".into(),
            ));
        }
        let cursor = cursor.map(parse_agent_id).transpose()?;
        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "WITH cursor_position(created_at, agent_id) AS ( \
                           SELECT cursor_agent.created_at, cursor_agent.agent_id \
                             FROM identity_agent cursor_agent \
                             JOIN principal cursor_principal \
                               ON cursor_principal.tenant_id = cursor_agent.tenant_id \
                              AND cursor_principal.region = cursor_agent.region \
                              AND cursor_principal.principal_id = \
                                  'agent:' || cursor_agent.agent_id::text \
                            WHERE cursor_agent.tenant_id = $1 \
                              AND cursor_agent.region = $2 \
                              AND cursor_agent.agent_id = $3 \
                         ) \
                         SELECT a.tenant_id, a.agent_id, a.name, a.runtime_ref, a.created_by, a.tools, \
                                a.grants, a.workspace_guidance_revision, \
                                a.workspace_guidance_digest, a.workspace_guidance_key_ref, \
                                a.workspace_guidance_nonce, a.workspace_guidance_ciphertext, \
                                a.workspace_guidance_updated_at, a.created_at, p.kind, p.status \
                           FROM identity_agent a \
                           JOIN principal p ON p.tenant_id = a.tenant_id AND p.region = a.region \
                            AND p.principal_id = 'agent:' || a.agent_id::text \
                          WHERE a.tenant_id = $1 AND a.region = $2 \
                            AND ( \
                              $3::uuid IS NULL OR EXISTS ( \
                                SELECT 1 FROM cursor_position cursor \
                                 WHERE (a.created_at, a.agent_id) \
                                     < (cursor.created_at, cursor.agent_id) \
                              ) \
                            ) \
                          ORDER BY a.created_at DESC, a.agent_id DESC LIMIT $4",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(cursor)
                    .bind(i64::from(limit))
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(query_error("list agents"))?;
                    rows.iter().map(agent_from_joined_row).collect()
                })
            })
            .await
            .map_err(|error| AgentRegistryError::Storage(error.to_string()))
    }

    pub async fn update_workspace_guidance(
        &self,
        actor: &Principal,
        request: AgentWorkspaceGuidanceUpdate,
    ) -> Result<AgentWorkspaceGuidanceOutcome, AgentRegistryError> {
        if actor.kind != PrincipalKind::Human || actor.status != PrincipalStatus::Active {
            return Err(AgentRegistryError::BadInput(
                "workspace-guidance updates require an active Human owner".into(),
            ));
        }
        validate_workspace_guidance(actor, request.guidance.as_ref())?;
        self.require_local_region(actor)?;
        let agent_id = parse_agent_id(&request.agent_id)?;
        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        let owner = actor.principal_id.0.clone();
        let updated_at = Utc::now();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT a.tenant_id, a.agent_id, a.name, a.runtime_ref, a.created_by, \
                                a.tools, a.grants, a.workspace_guidance_revision, \
                                a.workspace_guidance_digest, a.workspace_guidance_key_ref, \
                                a.workspace_guidance_nonce, a.workspace_guidance_ciphertext, \
                                a.workspace_guidance_updated_at, a.created_at, p.kind, p.status \
                           FROM identity_agent a \
                           JOIN principal p ON p.tenant_id = a.tenant_id AND p.region = a.region \
                            AND p.principal_id = 'agent:' || a.agent_id::text \
                          WHERE a.tenant_id = $1 AND a.region = $2 AND a.agent_id = $3 \
                          FOR UPDATE OF a",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(agent_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("lock agent workspace guidance"))?;
                    let Some(row) = row else {
                        return Ok(None);
                    };
                    let current = agent_from_joined_row(&row)?;
                    if current.created_by != owner {
                        return Ok(None);
                    }
                    let requested_digest = request
                        .guidance
                        .as_ref()
                        .map(|guidance| guidance.content_digest.as_str());
                    let current_digest = current
                        .workspace_guidance
                        .as_ref()
                        .map(|guidance| guidance.content_digest.as_str());
                    if requested_digest == current_digest {
                        return Ok(Some(Ok(AgentWorkspaceGuidanceOutcome {
                            agent: current,
                            changed: false,
                        })));
                    }
                    if request.expected_revision != current.workspace_guidance_revision {
                        return Ok(Some(Err(current.workspace_guidance_revision)));
                    }
                    let next_revision = current
                        .workspace_guidance_revision
                        .checked_add(1)
                        .ok_or_else(|| PgError::Query("agent workspace-guidance revision overflowed".into()))?;
                    let next_revision = i64::try_from(next_revision).map_err(|_| {
                        PgError::Query("agent workspace-guidance revision overflowed".into())
                    })?;
                    let guidance = request.guidance.as_ref();
                    let row = sqlx::query(
                        "UPDATE identity_agent \
                            SET workspace_guidance_revision = $4, \
                                workspace_guidance_digest = $5, \
                                workspace_guidance_key_ref = $6, \
                                workspace_guidance_nonce = $7, \
                                workspace_guidance_ciphertext = $8, \
                                workspace_guidance_updated_at = $9 \
                          WHERE tenant_id = $1 AND region = $2 AND agent_id = $3 \
                          RETURNING tenant_id, agent_id, name, runtime_ref, created_by, tools, grants, \
                            workspace_guidance_revision, workspace_guidance_digest, \
                            workspace_guidance_key_ref, workspace_guidance_nonce, \
                            workspace_guidance_ciphertext, workspace_guidance_updated_at, created_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(agent_id)
                    .bind(next_revision)
                    .bind(guidance.map(|guidance| guidance.content_digest.as_str()))
                    .bind(guidance.map(|guidance| guidance.encrypted_content.key_ref.to_uri()))
                    .bind(guidance.map(|guidance| guidance.encrypted_content.nonce.to_vec()))
                    .bind(guidance.map(|guidance| guidance.encrypted_content.ciphertext.as_slice()))
                    .bind(updated_at)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(query_error("update agent workspace guidance"))?;
                    Ok(Some(Ok(AgentWorkspaceGuidanceOutcome {
                        agent: agent_from_row(&row, current.status)?,
                        changed: true,
                    })))
                })
            })
            .await
            .map_err(|error| AgentRegistryError::Storage(error.to_string()))?
            .ok_or(AgentRegistryError::NotFound)?
            .map_err(|actual_revision| {
                AgentRegistryError::Conflict(format!(
                    "agent workspace guidance changed at revision {actual_revision}; refresh before replacing it"
                ))
            })
    }

    fn require_local_region(&self, actor: &Principal) -> Result<(), AgentRegistryError> {
        if actor.region.0 != self.provider.config().region {
            return Err(AgentRegistryError::NotFound);
        }
        Ok(())
    }
}

enum CreateTx {
    Created {
        agent: AgentRegistration,
        versions: DurableDelegationPolicyVersions,
        revisions: DurableDelegationPolicyRevisions,
    },
    Existing {
        agent: AgentRegistration,
        versions: DurableDelegationPolicyVersions,
        revisions: DurableDelegationPolicyRevisions,
    },
    NonceConflict,
    NameConflict,
    Policy(DurableDelegationPolicyError),
}

async fn ensure_policies(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    agent_id: &str,
    actor_id: &str,
    proposal: &NewAgent,
) -> Result<
    Result<
        (
            DurableDelegationPolicyVersions,
            DurableDelegationPolicyRevisions,
        ),
        DurableDelegationPolicyError,
    >,
    PgError,
> {
    ensure_agent_policy_bundle_on_conn(
        conn,
        tenant,
        region,
        agent_id,
        actor_id,
        proposal.grants.clone(),
        proposal.tenant_policy_if_missing.clone(),
        proposal.trigger_actor_policy_if_missing.clone(),
    )
    .await
}

async fn lock_agent_creation(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    actor: &str,
    nonce: &str,
    name: &str,
) -> Result<(), PgError> {
    for identity in [
        format!("agent-nonce:{region}:{actor}:{nonce}"),
        format!("agent-name:{region}:{name}"),
    ] {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(\
                hashtextextended(length($2)::text || ':' || $2 || ':' || $1, 0))",
        )
        .bind(identity)
        .bind(tenant)
        .execute(&mut *conn)
        .await
        .map_err(query_error("lock agent creation identity"))?;
    }
    Ok(())
}

async fn agent_by_nonce(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    actor: &str,
    nonce: &str,
) -> Result<Option<AgentRegistration>, PgError> {
    let row = sqlx::query(
        "SELECT a.tenant_id, a.agent_id, a.name, a.runtime_ref, a.created_by, a.tools, a.grants, \
                a.workspace_guidance_revision, a.workspace_guidance_digest, \
                a.workspace_guidance_key_ref, a.workspace_guidance_nonce, \
                a.workspace_guidance_ciphertext, a.workspace_guidance_updated_at, \
                a.created_at, p.kind, p.status \
           FROM identity_agent a \
           JOIN principal p ON p.tenant_id = a.tenant_id AND p.region = a.region \
            AND p.principal_id = 'agent:' || a.agent_id::text \
          WHERE a.tenant_id = $1 AND a.region = $2 AND a.created_by = $3 \
            AND a.client_nonce = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(actor)
    .bind(nonce)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("read agent idempotency record"))?;
    row.as_ref().map(agent_from_joined_row).transpose()
}

async fn agent_by_id(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    agent_id: Uuid,
) -> Result<Option<AgentRegistration>, PgError> {
    let row = sqlx::query(
        "SELECT a.tenant_id, a.agent_id, a.name, a.runtime_ref, a.created_by, a.tools, a.grants, \
                a.workspace_guidance_revision, a.workspace_guidance_digest, \
                a.workspace_guidance_key_ref, a.workspace_guidance_nonce, \
                a.workspace_guidance_ciphertext, a.workspace_guidance_updated_at, \
                a.created_at, p.kind, p.status \
           FROM identity_agent a \
           JOIN principal p ON p.tenant_id = a.tenant_id AND p.region = a.region \
            AND p.principal_id = 'agent:' || a.agent_id::text \
          WHERE a.tenant_id = $1 AND a.region = $2 AND a.agent_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(agent_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("read agent"))?;
    row.as_ref().map(agent_from_joined_row).transpose()
}

async fn agent_name_exists(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    name: &str,
) -> Result<bool, PgError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM identity_agent \
          WHERE tenant_id = $1 AND region = $2 AND name = $3)",
    )
    .bind(tenant)
    .bind(region)
    .bind(name)
    .fetch_one(&mut *conn)
    .await
    .map_err(query_error("check agent name"))
}

fn agent_from_joined_row(row: &sqlx::postgres::PgRow) -> Result<AgentRegistration, PgError> {
    let kind = serde_json::from_str::<PrincipalKind>(
        &row.try_get::<String, _>("kind")
            .map_err(decode_error("agent principal kind"))?,
    )
    .map_err(|error| PgError::Query(format!("decode agent principal kind: {error}")))?;
    let status = serde_json::from_str::<PrincipalStatus>(
        &row.try_get::<String, _>("status")
            .map_err(decode_error("agent principal status"))?,
    )
    .map_err(|error| PgError::Query(format!("decode agent principal status: {error}")))?;
    let agent = agent_from_row(row, status)?;
    let expected_kind = PrincipalKind::Agent {
        runtime_ref: RuntimeRef(agent.runtime_ref.clone()),
        on_behalf_of: Some(myelin_identity::PrincipalId(agent.created_by.clone())),
    };
    if kind != expected_kind {
        return Err(PgError::Query(
            "agent registry/principal binding failed its integrity check".into(),
        ));
    }
    Ok(agent)
}

fn agent_from_row(
    row: &sqlx::postgres::PgRow,
    status: PrincipalStatus,
) -> Result<AgentRegistration, PgError> {
    let id = row
        .try_get::<Uuid, _>("agent_id")
        .map_err(decode_error("agent id"))?;
    let tenant = row
        .try_get::<String, _>("tenant_id")
        .map_err(decode_error("agent tenant"))?;
    let created_by = row
        .try_get::<String, _>("created_by")
        .map_err(decode_error("agent creator"))?;
    let workspace_guidance_revision = row
        .try_get::<i64, _>("workspace_guidance_revision")
        .map_err(decode_error("agent workspace-guidance revision"))?;
    let workspace_guidance_revision = u64::try_from(workspace_guidance_revision)
        .map_err(|_| PgError::Query("agent workspace-guidance revision is invalid".into()))?;
    let workspace_guidance = decode_workspace_guidance(row, &tenant, &created_by)?;
    let workspace_guidance_updated_at = row
        .try_get::<Option<DateTime<Utc>>, _>("workspace_guidance_updated_at")
        .map_err(decode_error("agent workspace-guidance update time"))?
        .map(|updated_at| updated_at.to_rfc3339_opts(SecondsFormat::Micros, true));
    if (workspace_guidance_revision == 0 && workspace_guidance_updated_at.is_some())
        || (workspace_guidance_revision > 0 && workspace_guidance_updated_at.is_none())
    {
        return Err(PgError::Query(
            "agent workspace-guidance revision has an invalid timestamp".into(),
        ));
    }
    Ok(AgentRegistration {
        id: id.to_string(),
        principal_id: agent_principal_id(id),
        name: row.try_get("name").map_err(decode_error("agent name"))?,
        runtime_ref: row
            .try_get("runtime_ref")
            .map_err(decode_error("agent runtime"))?,
        created_by,
        tools: row.try_get("tools").map_err(decode_error("agent tools"))?,
        grants: row
            .try_get("grants")
            .map_err(decode_error("agent grants"))?,
        workspace_guidance_revision,
        workspace_guidance,
        workspace_guidance_updated_at,
        status,
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map_err(decode_error("agent creation time"))?
            .to_rfc3339_opts(SecondsFormat::Micros, true),
    })
}

fn decode_workspace_guidance(
    row: &sqlx::postgres::PgRow,
    tenant: &str,
    owner: &str,
) -> Result<Option<AgentWorkspaceGuidance>, PgError> {
    match (
        row.try_get::<Option<String>, _>("workspace_guidance_digest")
            .map_err(decode_error("agent workspace-guidance digest"))?,
        row.try_get::<Option<String>, _>("workspace_guidance_key_ref")
            .map_err(decode_error("agent workspace-guidance key"))?,
        row.try_get::<Option<Vec<u8>>, _>("workspace_guidance_nonce")
            .map_err(decode_error("agent workspace-guidance nonce"))?,
        row.try_get::<Option<Vec<u8>>, _>("workspace_guidance_ciphertext")
            .map_err(decode_error("agent workspace-guidance ciphertext"))?,
    ) {
        (None, None, None, None) => Ok(None),
        (Some(content_digest), Some(key_ref), Some(nonce), Some(ciphertext)) => {
            let key_ref = PiiKeyRef::parse(&key_ref).ok_or_else(|| {
                PgError::Query("agent workspace-guidance key reference is invalid".into())
            })?;
            let nonce: [u8; NONCE_LEN] = nonce
                .try_into()
                .map_err(|_| PgError::Query("agent workspace-guidance nonce is invalid".into()))?;
            if key_ref.tenant.as_str() != tenant
                || key_ref.class
                    != (KeyClass::ScopedSubject {
                        scope: SubjectKeyScope::AgentData,
                        subject: owner.to_string(),
                    })
            {
                return Err(PgError::Query(
                    "agent workspace-guidance key does not match its owner".into(),
                ));
            }
            Ok(Some(AgentWorkspaceGuidance {
                content_digest,
                encrypted_content: EncryptedColumn {
                    key_ref,
                    nonce,
                    ciphertext,
                },
            }))
        }
        _ => Err(PgError::Query(
            "agent has incomplete durable workspace guidance".into(),
        )),
    }
}

fn same_intent(agent: &AgentRegistration, proposal: &NewAgent) -> bool {
    agent.name == proposal.name
        && agent.runtime_ref == proposal.runtime_ref
        && agent.tools == proposal.tools
        && agent.grants == proposal.grants
        && agent
            .workspace_guidance
            .as_ref()
            .map(|guidance| guidance.content_digest.as_str())
            == proposal
                .workspace_guidance
                .as_ref()
                .map(|guidance| guidance.content_digest.as_str())
}

pub fn validate_new_agent(
    actor: &Principal,
    proposal: &NewAgent,
) -> Result<(), AgentRegistryError> {
    if actor.kind != PrincipalKind::Human || actor.status != PrincipalStatus::Active {
        return Err(AgentRegistryError::BadInput(
            "activation requires an active Human delegator".into(),
        ));
    }
    if proposal.agent_id.get_version_num() != 4 {
        return Err(AgentRegistryError::BadInput(
            "agent id must be a UUID v4".into(),
        ));
    }
    if proposal.name.is_empty()
        || proposal.name.len() > MAX_AGENT_NAME_BYTES
        || proposal.name.trim() != proposal.name
        || proposal.name.chars().any(char::is_control)
    {
        return Err(AgentRegistryError::BadInput(format!(
            "name must contain 1..={MAX_AGENT_NAME_BYTES} bytes, without surrounding whitespace or control characters"
        )));
    }
    if proposal.runtime_ref.is_empty()
        || proposal.runtime_ref.len() > 255
        || !proposal
            .runtime_ref
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
    {
        return Err(AgentRegistryError::BadInput(
            "runtime reference must contain 1..=255 ASCII-graphic bytes".into(),
        ));
    }
    if proposal.tools.is_empty() || proposal.tools.len() > MAX_AGENT_TOOLS {
        return Err(AgentRegistryError::BadInput(format!(
            "activation must select 1..={MAX_AGENT_TOOLS} tools"
        )));
    }
    if proposal.grants.is_empty() || proposal.grants.len() > 512 {
        return Err(AgentRegistryError::BadInput(
            "activation must carry 1..=512 effective grants".into(),
        ));
    }
    for (label, values, max) in [
        ("tool", &proposal.tools, 255usize),
        ("grant", &proposal.grants, 1_024usize),
        (
            "tenant policy grant",
            &proposal.tenant_policy_if_missing,
            1_024usize,
        ),
        (
            "trigger actor policy grant",
            &proposal.trigger_actor_policy_if_missing,
            1_024usize,
        ),
    ] {
        if values.iter().any(|value| {
            value.is_empty() || value.len() > max || value.chars().any(char::is_whitespace)
        }) {
            return Err(AgentRegistryError::BadInput(format!(
                "{label} values must be bounded, non-empty, and contain no whitespace"
            )));
        }
    }
    if !contains_every(&proposal.tenant_policy_if_missing, &proposal.grants)
        || !contains_every(&proposal.trigger_actor_policy_if_missing, &proposal.grants)
    {
        return Err(AgentRegistryError::BadInput(
            "new tenant and trigger-actor policy ceilings must contain every agent grant".into(),
        ));
    }
    if proposal.client_nonce.is_empty()
        || proposal.client_nonce.len() > 128
        || !proposal
            .client_nonce
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
    {
        return Err(AgentRegistryError::BadInput(
            "client nonce must be 1..=128 ASCII-graphic bytes".into(),
        ));
    }
    validate_workspace_guidance(actor, proposal.workspace_guidance.as_ref())?;
    Ok(())
}

fn validate_workspace_guidance(
    actor: &Principal,
    guidance: Option<&AgentWorkspaceGuidance>,
) -> Result<(), AgentRegistryError> {
    let Some(guidance) = guidance else {
        return Ok(());
    };
    let digest = guidance
        .content_digest
        .strip_prefix("blake3:")
        .unwrap_or_default();
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || guidance.encrypted_content.key_ref.tenant != actor.tenant
        || guidance.encrypted_content.key_ref.class
            != (KeyClass::ScopedSubject {
                scope: SubjectKeyScope::AgentData,
                subject: actor.principal_id.0.clone(),
            })
        || guidance.encrypted_content.ciphertext.len() < 17
        || guidance.encrypted_content.ciphertext.len() > MAX_AGENT_WORKSPACE_GUIDANCE_BYTES + 16
    {
        return Err(AgentRegistryError::BadInput(
            "workspace guidance is not a canonical owner-encrypted receipt".into(),
        ));
    }
    Ok(())
}

fn canonicalize(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn contains_every(haystack: &[String], needles: &[String]) -> bool {
    let haystack = haystack.iter().map(String::as_str).collect::<BTreeSet<_>>();
    needles
        .iter()
        .all(|needle| haystack.contains(needle.as_str()))
}

fn agent_principal_id(agent_id: Uuid) -> String {
    format!("agent:{agent_id}")
}

pub fn agent_ref(tenant: &str, agent_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/identity/agent/{agent_id}"))
}

fn parse_agent_id(value: &str) -> Result<Uuid, AgentRegistryError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| AgentRegistryError::BadInput("agent id must be a canonical UUID".into()))?;
    if parsed.to_string() != value {
        return Err(AgentRegistryError::BadInput(
            "agent id must be a canonical UUID".into(),
        ));
    }
    Ok(parsed)
}

fn agent_created_event(
    actor: &Principal,
    agent_id: Uuid,
    runtime_ref: &str,
    tools: &[String],
    event_id: EventId,
    now: DateTime<Utc>,
) -> myelin_events::EventEnvelope {
    let timestamp = Timestamp(now.to_rfc3339_opts(SecondsFormat::Micros, true));
    let id = agent_id.to_string();
    derive_envelope(
        EventDraft {
            type_: EventType(IDENTITY_AGENT_CREATED.into()),
            subject: agent_ref(&actor.tenant.0, &id),
            aggregate: AggregateKey(format!("identity:agent:{id}")),
            payload: serde_json::json!({
                "agent_id": id,
                "principal_id": agent_principal_id(agent_id),
                "runtime_ref": runtime_ref,
                "tools": tools,
                "delegator_principal_id": actor.principal_id.0,
            }),
            data_role: EventDataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: actor.tenant.clone(),
            region: actor.region.clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        },
        None,
    )
}

fn query_error(context: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{context}: {error}"))
}

fn decode_error(context: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode {context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::PrincipalId;
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

    fn proposal() -> NewAgent {
        NewAgent {
            agent_id: Uuid::new_v4(),
            name: "Review companion".into(),
            runtime_ref: EXTERNAL_MCP_RUNTIME.into(),
            tools: vec!["git.open_pr".into()],
            grants: vec!["agent.tools.read".into(), "repo.push".into()],
            workspace_guidance: None,
            tenant_policy_if_missing: vec!["agent.tools.read".into(), "repo.push".into()],
            trigger_actor_policy_if_missing: vec!["agent.tools.read".into(), "repo.push".into()],
            client_nonce: "retry-agent-1".into(),
        }
    }

    #[test]
    fn activation_intent_is_bounded_human_readable_and_human_delegated() {
        assert!(validate_new_agent(&human(), &proposal()).is_ok());
        let mut invalid = proposal();
        invalid.name = " padded".into();
        assert!(validate_new_agent(&human(), &invalid).is_err());
        let mut invalid = proposal();
        invalid.tools.clear();
        assert!(validate_new_agent(&human(), &invalid).is_err());
        let mut inactive = human();
        inactive.status = PrincipalStatus::Suspended;
        assert!(validate_new_agent(&inactive, &proposal()).is_err());
    }

    #[test]
    fn agent_refs_and_principal_ids_are_distinct_canonical_addresses() {
        let id = Uuid::parse_str("018f0d25-7b55-7d8b-9f1e-a851f9b534c1").unwrap();
        assert_eq!(
            agent_principal_id(id),
            "agent:018f0d25-7b55-7d8b-9f1e-a851f9b534c1"
        );
        assert_eq!(
            agent_ref("acme", &id.to_string()).0,
            "myelin://acme/identity/agent/018f0d25-7b55-7d8b-9f1e-a851f9b534c1"
        );
    }
}
