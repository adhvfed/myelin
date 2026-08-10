use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;
use serde_json::Value;
use sqlx::Row;

use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};

const MAX_PRINCIPAL_BYTES: usize = 255;
const MAX_RUN_ID_BYTES: usize = 512;
const MAX_ANSWER_BYTES: usize = 64 * 1024;
const MAX_TRACE_BODY_BYTES: usize = 256 * 1024;

pub const AGENT_TRACE_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_agent_trace (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    run_id          text        NOT NULL CHECK (length(run_id) BETWEEN 1 AND 512),
    artifact_ref    text        NOT NULL CHECK (length(artifact_ref) BETWEEN 1 AND 1024),
    agent_principal text        NOT NULL CHECK (length(agent_principal) BETWEEN 1 AND 255),
    requested_by    text        NOT NULL CHECK (length(requested_by) BETWEEN 1 AND 255),
    answer          text        NOT NULL CHECK (octet_length(answer) BETWEEN 1 AND 65536),
    trace_body      jsonb       NOT NULL CHECK (jsonb_typeof(trace_body) = 'object'),
    charged_micro   bigint      NOT NULL CHECK (charged_micro >= 0),
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, run_id),
    UNIQUE (tenant_id, region, artifact_ref),
    CHECK (octet_length(trace_body::text) <= 262144)
);
ALTER TABLE knowledge_agent_trace ENABLE ROW LEVEL SECURITY;
ALTER TABLE knowledge_agent_trace FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON knowledge_agent_trace;
CREATE POLICY myelin_tenant_isolation ON knowledge_agent_trace
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const AGENT_TRACE_ERASURE_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_agent_trace_erasure (
    tenant_id    text        NOT NULL,
    region       text        NOT NULL,
    run_id       text        NOT NULL CHECK (length(run_id) BETWEEN 1 AND 512),
    artifact_ref text        NOT NULL CHECK (length(artifact_ref) BETWEEN 1 AND 1024),
    erased_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, run_id)
);
ALTER TABLE knowledge_agent_trace_erasure ENABLE ROW LEVEL SECURITY;
ALTER TABLE knowledge_agent_trace_erasure FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON knowledge_agent_trace_erasure;
CREATE POLICY myelin_tenant_isolation ON knowledge_agent_trace_erasure
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub fn agent_trace_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0098_knowledge_agent_trace", AGENT_TRACE_MIGRATION),
        Migration::plain(
            "0099_knowledge_agent_trace_erasure",
            AGENT_TRACE_ERASURE_MIGRATION,
        ),
    ])
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTraceWrite {
    pub run_id: String,
    pub agent_principal: String,
    pub requested_by: String,
    pub answer: String,
    pub trace_body: Value,
    pub charged_micro: u64,
}

impl AgentTraceWrite {
    pub fn artifact_ref(&self, tenant: &TenantId) -> Result<ArtifactRef, AgentTraceError> {
        self.validate()?;
        let canonical = serde_json::to_vec(&self.trace_body)
            .map_err(|_| AgentTraceError::Invalid("trace body is not valid JSON"))?;
        let digest = blake3::hash(&canonical).to_hex();
        Ok(ArtifactRef(format!(
            "myelin://{}/knowledge/doc/blake3:{digest}",
            tenant.0
        )))
    }

    fn validate(&self) -> Result<(), AgentTraceError> {
        bounded("run id", &self.run_id, MAX_RUN_ID_BYTES)?;
        bounded(
            "agent principal",
            &self.agent_principal,
            MAX_PRINCIPAL_BYTES,
        )?;
        bounded(
            "requesting principal",
            &self.requested_by,
            MAX_PRINCIPAL_BYTES,
        )?;
        bounded("agent answer", &self.answer, MAX_ANSWER_BYTES)?;
        let body = serde_json::to_vec(&self.trace_body)
            .map_err(|_| AgentTraceError::Invalid("trace body is not valid JSON"))?;
        if !self.trace_body.is_object() || body.len() > MAX_TRACE_BODY_BYTES {
            return Err(AgentTraceError::Invalid(
                "trace body must be a JSON object of at most 256 KiB",
            ));
        }
        let matches_envelope = self.trace_body.get("schema").and_then(Value::as_str)
            == Some("myelin.agent_trace.v1")
            && self.trace_body.get("run_id").and_then(Value::as_str) == Some(self.run_id.as_str())
            && self.trace_body.get("actor").and_then(Value::as_str)
                == Some(self.agent_principal.as_str())
            && self.trace_body.get("requested_by").and_then(Value::as_str)
                == Some(self.requested_by.as_str())
            && self.trace_body.get("answer").and_then(Value::as_str) == Some(self.answer.as_str())
            && self.trace_body.get("charged_micro").and_then(Value::as_u64)
                == Some(self.charged_micro)
            && self
                .trace_body
                .get("blocks")
                .and_then(Value::as_array)
                .is_some_and(|blocks| !blocks.is_empty());
        if !matches_envelope {
            return Err(AgentTraceError::Invalid(
                "body does not match its immutable run envelope",
            ));
        }
        Ok(())
    }
}

fn bounded(label: &'static str, value: &str, maximum: usize) -> Result<(), AgentTraceError> {
    if value.is_empty() || value.len() > maximum {
        return Err(AgentTraceError::Invalid(label));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTraceReceipt {
    pub artifact_ref: ArtifactRef,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTraceResult {
    pub run_id: String,
    pub artifact_ref: ArtifactRef,
    pub agent_principal: String,
    pub answer: String,
    pub charged_micro: u64,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTraceEraseReceipt {
    pub artifact_ref: ArtifactRef,
    pub already_erased: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EraseAgentTraceOutcome {
    Erased(AgentTraceEraseReceipt),
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentTraceError {
    Invalid(&'static str),
    Conflict,
    Erased,
    Storage(String),
}

impl core::fmt::Display for AgentTraceError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Invalid(field) => write!(formatter, "invalid agent trace {field}"),
            Self::Conflict => formatter
                .write_str("agent run already has a different immutable trace; replay refused"),
            Self::Erased => formatter.write_str("agent run trace was erased; recreation refused"),
            Self::Storage(error) => write!(formatter, "agent trace storage failed: {error}"),
        }
    }
}

impl std::error::Error for AgentTraceError {}

pub trait AgentTraceWriter: Send + Sync {
    fn write(
        &self,
        tenant: &TenantId,
        trace: AgentTraceWrite,
    ) -> Result<AgentTraceReceipt, AgentTraceError>;
}

#[derive(Default)]
pub struct InMemoryAgentTraceStore {
    traces: Mutex<BTreeMap<(String, String), AgentTraceWrite>>,
}

impl InMemoryAgentTraceStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AgentTraceWriter for InMemoryAgentTraceStore {
    fn write(
        &self,
        tenant: &TenantId,
        trace: AgentTraceWrite,
    ) -> Result<AgentTraceReceipt, AgentTraceError> {
        let artifact_ref = trace.artifact_ref(tenant)?;
        let key = (tenant.0.clone(), trace.run_id.clone());
        let mut traces = self.traces.lock().expect("agent trace store lock");
        match traces.get(&key) {
            Some(existing) if existing == &trace => Ok(AgentTraceReceipt {
                artifact_ref,
                replayed: true,
            }),
            Some(_) => Err(AgentTraceError::Conflict),
            None => {
                traces.insert(key, trace);
                Ok(AgentTraceReceipt {
                    artifact_ref,
                    replayed: false,
                })
            }
        }
    }
}

#[derive(Clone)]
pub struct DurableAgentTraceStore {
    provider: SubstrateProvider,
    runtime: tokio::runtime::Handle,
}

impl DurableAgentTraceStore {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self::with_runtime(provider, tokio::runtime::Handle::current())
    }

    pub fn with_runtime(provider: SubstrateProvider, runtime: tokio::runtime::Handle) -> Self {
        Self { provider, runtime }
    }

    pub async fn fetch_for_owner(
        &self,
        tenant: &str,
        owner_principal: &str,
        binding_id: sqlx::types::Uuid,
        run_id: &str,
    ) -> Result<Option<AgentTraceResult>, ProviderError> {
        let tenant = tenant.to_string();
        let owner_principal = owner_principal.to_string();
        let run_id = run_id.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT trace.run_id, trace.artifact_ref, trace.agent_principal, \
                                trace.answer, trace.charged_micro, trace.created_at \
                           FROM knowledge_agent_trace trace \
                           JOIN agent_trigger_firing firing \
                             ON firing.tenant_id = trace.tenant_id \
                            AND firing.region = trace.region \
                            AND firing.run_id::text = trace.run_id \
                           JOIN agent_trigger_binding binding \
                             ON binding.tenant_id = firing.tenant_id \
                            AND binding.region = firing.region \
                            AND binding.binding_id = firing.binding_id \
                          WHERE trace.tenant_id = $1 AND trace.region = $2 \
                            AND trace.run_id = $3 AND binding.binding_id = $4 \
                            AND binding.owner_principal_id = $5",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&run_id)
                    .bind(binding_id)
                    .bind(&owner_principal)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(trace_query)?;
                    row.map(agent_trace_result_from_row).transpose()
                })
            })
            .await
    }

    pub async fn erase_for_owner(
        &self,
        tenant: &str,
        owner_principal: &str,
        binding_id: sqlx::types::Uuid,
        run_id: &str,
    ) -> Result<EraseAgentTraceOutcome, ProviderError> {
        let tenant = tenant.to_string();
        let owner_principal = owner_principal.to_string();
        let run_id = run_id.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    erase_for_owner_on_connection(
                        connection,
                        &tenant,
                        &region,
                        &owner_principal,
                        binding_id,
                        &run_id,
                    )
                    .await
                })
            })
            .await
    }

    fn write_blocking(
        &self,
        tenant: &TenantId,
        trace: AgentTraceWrite,
    ) -> Result<AgentTraceReceipt, AgentTraceError> {
        let tenant_id = tenant.0.clone();
        let region = self.provider.config().region.clone();
        let artifact_ref = trace.artifact_ref(tenant)?;
        let persisted_ref = artifact_ref.clone();
        let provider = self.provider.clone();
        let transaction_tenant = tenant_id.clone();
        let future = provider.with_tenant_tx(&transaction_tenant, move |connection| {
            Box::pin(async move {
                write_on_connection(connection, &tenant_id, &region, &persisted_ref, &trace).await
            })
        });
        let result = match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.runtime.block_on(future)),
            Err(_) => self.runtime.block_on(future),
        }
        .map_err(|error| AgentTraceError::Storage(error.to_string()))??;
        Ok(AgentTraceReceipt {
            artifact_ref,
            replayed: result,
        })
    }
}

impl AgentTraceWriter for DurableAgentTraceStore {
    fn write(
        &self,
        tenant: &TenantId,
        trace: AgentTraceWrite,
    ) -> Result<AgentTraceReceipt, AgentTraceError> {
        self.write_blocking(tenant, trace)
    }
}

async fn write_on_connection(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    artifact_ref: &ArtifactRef,
    trace: &AgentTraceWrite,
) -> Result<Result<bool, AgentTraceError>, PgError> {
    lock_trace(connection, tenant, region, &trace.run_id).await?;
    let erased = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM knowledge_agent_trace_erasure \
          WHERE tenant_id = $1 AND region = $2 AND run_id = $3)",
    )
    .bind(tenant)
    .bind(region)
    .bind(&trace.run_id)
    .fetch_one(&mut *connection)
    .await
    .map_err(trace_query)?;
    if erased {
        return Ok(Err(AgentTraceError::Erased));
    }
    let charged_micro = i64::try_from(trace.charged_micro)
        .map_err(|_| PgError::Query("agent trace charge exceeds i64".into()))?;
    let inserted = sqlx::query(
        "INSERT INTO knowledge_agent_trace \
           (tenant_id, region, run_id, artifact_ref, agent_principal, requested_by, \
            answer, trace_body, charged_micro) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(region)
    .bind(&trace.run_id)
    .bind(&artifact_ref.0)
    .bind(&trace.agent_principal)
    .bind(&trace.requested_by)
    .bind(&trace.answer)
    .bind(&trace.trace_body)
    .bind(charged_micro)
    .execute(&mut *connection)
    .await
    .map_err(trace_query)?;
    if inserted.rows_affected() == 1 {
        return Ok(Ok(false));
    }

    let existing = sqlx::query(
        "SELECT artifact_ref, agent_principal, requested_by, answer, trace_body, charged_micro \
           FROM knowledge_agent_trace \
          WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(&trace.run_id)
    .fetch_optional(connection)
    .await
    .map_err(trace_query)?;
    let Some(existing) = existing else {
        return Ok(Err(AgentTraceError::Conflict));
    };
    let same = existing
        .try_get::<String, _>("artifact_ref")
        .map_err(trace_query)?
        == artifact_ref.0
        && existing
            .try_get::<String, _>("agent_principal")
            .map_err(trace_query)?
            == trace.agent_principal
        && existing
            .try_get::<String, _>("requested_by")
            .map_err(trace_query)?
            == trace.requested_by
        && existing
            .try_get::<String, _>("answer")
            .map_err(trace_query)?
            == trace.answer
        && existing
            .try_get::<Value, _>("trace_body")
            .map_err(trace_query)?
            == trace.trace_body
        && existing
            .try_get::<i64, _>("charged_micro")
            .map_err(trace_query)?
            == charged_micro;
    Ok(if same {
        Ok(true)
    } else {
        Err(AgentTraceError::Conflict)
    })
}

async fn erase_for_owner_on_connection(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    owner_principal: &str,
    binding_id: sqlx::types::Uuid,
    run_id: &str,
) -> Result<EraseAgentTraceOutcome, PgError> {
    let owns_run = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (\
             SELECT 1 FROM agent_trigger_firing firing \
             JOIN agent_trigger_binding binding \
               ON binding.tenant_id = firing.tenant_id \
              AND binding.region = firing.region \
              AND binding.binding_id = firing.binding_id \
             WHERE firing.tenant_id = $1 AND firing.region = $2 \
               AND firing.binding_id = $3 AND firing.run_id::text = $4 \
               AND binding.owner_principal_id = $5\
         )",
    )
    .bind(tenant)
    .bind(region)
    .bind(binding_id)
    .bind(run_id)
    .bind(owner_principal)
    .fetch_one(&mut *connection)
    .await
    .map_err(trace_query)?;
    if !owns_run {
        return Ok(EraseAgentTraceOutcome::NotFound);
    }

    lock_trace(connection, tenant, region, run_id).await?;
    if let Some(artifact_ref) = sqlx::query_scalar::<_, String>(
        "SELECT artifact_ref FROM knowledge_agent_trace_erasure \
          WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(trace_query)?
    {
        return Ok(EraseAgentTraceOutcome::Erased(AgentTraceEraseReceipt {
            artifact_ref: ArtifactRef(artifact_ref),
            already_erased: true,
        }));
    }

    let Some(artifact_ref) = sqlx::query_scalar::<_, String>(
        "SELECT artifact_ref FROM knowledge_agent_trace \
          WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(trace_query)?
    else {
        return Ok(EraseAgentTraceOutcome::NotFound);
    };

    sqlx::query(
        "INSERT INTO knowledge_agent_trace_erasure \
           (tenant_id, region, run_id, artifact_ref) VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(&artifact_ref)
    .execute(&mut *connection)
    .await
    .map_err(trace_query)?;
    let deleted = sqlx::query(
        "DELETE FROM knowledge_agent_trace \
          WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .execute(&mut *connection)
    .await
    .map_err(trace_query)?;
    if deleted.rows_affected() != 1 {
        return Err(PgError::Query(
            "agent trace disappeared during serialized erasure".into(),
        ));
    }
    Ok(EraseAgentTraceOutcome::Erased(AgentTraceEraseReceipt {
        artifact_ref: ArtifactRef(artifact_ref),
        already_erased: false,
    }))
}

async fn lock_trace(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    run_id: &str,
) -> Result<(), PgError> {
    let lock_key = format!("agent-trace\u{1f}{tenant}\u{1f}{region}\u{1f}{run_id}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(connection)
        .await
        .map_err(trace_query)?;
    Ok(())
}

fn agent_trace_result_from_row(row: sqlx::postgres::PgRow) -> Result<AgentTraceResult, PgError> {
    let charged = row
        .try_get::<i64, _>("charged_micro")
        .map_err(trace_query)?;
    Ok(AgentTraceResult {
        run_id: row.try_get("run_id").map_err(trace_query)?,
        artifact_ref: ArtifactRef(row.try_get("artifact_ref").map_err(trace_query)?),
        agent_principal: row.try_get("agent_principal").map_err(trace_query)?,
        answer: row.try_get("answer").map_err(trace_query)?,
        charged_micro: u64::try_from(charged)
            .map_err(|_| PgError::Query("agent trace has a negative charge".into()))?,
        created_at: row
            .try_get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>, _>("created_at")
            .map_err(trace_query)?
            .to_rfc3339(),
    })
}

fn trace_query(error: sqlx::Error) -> PgError {
    PgError::Query(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(answer: &str) -> AgentTraceWrite {
        AgentTraceWrite {
            run_id: "33333333-3333-4333-8333-333333333333".into(),
            agent_principal: "agent:22222222-2222-4222-8222-222222222222".into(),
            requested_by: "human:founder".into(),
            answer: answer.into(),
            trace_body: serde_json::json!({
                "schema": "myelin.agent_trace.v1",
                "run_id": "33333333-3333-4333-8333-333333333333",
                "actor": "agent:22222222-2222-4222-8222-222222222222",
                "requested_by": "human:founder",
                "answer": answer,
                "charged_micro": 42,
                "blocks": [{
                    "type": "paragraph",
                    "inline": {"spans": [{"Text": {"text": answer, "marks": [], "link": null}}], "nodes": []}
                }]
            }),
            charged_micro: 42,
        }
    }

    #[test]
    fn the_trace_is_a_canonical_content_addressed_knowledge_document() {
        let tenant = TenantId("acme".into());
        let first = trace("One useful conclusion.")
            .artifact_ref(&tenant)
            .unwrap();
        let replay = trace("One useful conclusion.")
            .artifact_ref(&tenant)
            .unwrap();
        let changed = trace("A different conclusion.")
            .artifact_ref(&tenant)
            .unwrap();

        assert_eq!(first, replay);
        assert_ne!(first, changed);
        myelin_refs::parse_scoped(&first.0).expect("the trace crosses the ArtifactRef boundary");
        assert!(first.0.starts_with("myelin://acme/knowledge/doc/blake3:"));
    }

    #[test]
    fn the_in_memory_writer_replays_only_the_same_immutable_trace() {
        let tenant = TenantId("acme".into());
        let store = InMemoryAgentTraceStore::new();
        assert!(!store.write(&tenant, trace("Done.")).unwrap().replayed);
        assert!(store.write(&tenant, trace("Done.")).unwrap().replayed);
        assert_eq!(
            store.write(&tenant, trace("Rewritten.")).unwrap_err(),
            AgentTraceError::Conflict
        );
    }

    #[test]
    fn the_durable_holder_is_rls_scoped_bounded_and_immutable_by_primary_key() {
        assert!(AGENT_TRACE_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(AGENT_TRACE_MIGRATION.contains("PRIMARY KEY (tenant_id, region, run_id)"));
        assert!(AGENT_TRACE_MIGRATION.contains("UNIQUE (tenant_id, region, artifact_ref)"));
        assert!(AGENT_TRACE_MIGRATION.contains("octet_length(answer) BETWEEN 1 AND 65536"));
        assert!(AGENT_TRACE_MIGRATION.contains("octet_length(trace_body::text) <= 262144"));
        assert!(AGENT_TRACE_ERASURE_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(AGENT_TRACE_ERASURE_MIGRATION.contains("PRIMARY KEY (tenant_id, region, run_id)"));
    }
}
