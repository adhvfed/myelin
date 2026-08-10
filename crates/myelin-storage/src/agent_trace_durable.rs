use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, ErasureMethod, LocateReport, Patch, PersonalDataHolder,
    PortableBundle, Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;

use crate::encryption::{ColumnCryptor, EncryptedColumn, SubjectId};
use crate::kms::{DekId, KekId, KeyClass, KmsEngine, PiiKeyRef, NONCE_LEN};
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

pub const AGENT_TRACE_ENCRYPTION_MIGRATION: &str = r#"
ALTER TABLE knowledge_agent_trace
    ALTER COLUMN answer DROP NOT NULL,
    ALTER COLUMN trace_body DROP NOT NULL,
    ADD COLUMN IF NOT EXISTS payload_key_ref text,
    ADD COLUMN IF NOT EXISTS payload_nonce bytea,
    ADD COLUMN IF NOT EXISTS payload_ciphertext bytea;
ALTER TABLE knowledge_agent_trace
    DROP CONSTRAINT IF EXISTS knowledge_agent_trace_payload_shape;
ALTER TABLE knowledge_agent_trace
    ADD CONSTRAINT knowledge_agent_trace_payload_shape CHECK (
        (answer IS NOT NULL AND trace_body IS NOT NULL
          AND payload_key_ref IS NULL AND payload_nonce IS NULL AND payload_ciphertext IS NULL)
        OR
        (answer IS NULL AND trace_body IS NULL
          AND length(payload_key_ref) BETWEEN 1 AND 1024
          AND octet_length(payload_nonce) = 12
          AND octet_length(payload_ciphertext) BETWEEN 1 AND 524288)
    );
"#;

pub const AGENT_TRACE_SUBJECT_ERASURE_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_agent_trace_subject_erasure (
    tenant_id     text        NOT NULL,
    region        text        NOT NULL,
    subject_token text        NOT NULL CHECK (length(subject_token) = 64),
    erased_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, subject_token)
);
ALTER TABLE knowledge_agent_trace_subject_erasure ENABLE ROW LEVEL SECURITY;
ALTER TABLE knowledge_agent_trace_subject_erasure FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON knowledge_agent_trace_subject_erasure;
CREATE POLICY myelin_tenant_isolation ON knowledge_agent_trace_subject_erasure
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const AGENT_TRACE_SUBJECT_RESTRICTION_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_agent_trace_subject_restriction (
    tenant_id     text        NOT NULL,
    region        text        NOT NULL,
    subject_token text        NOT NULL CHECK (length(subject_token) = 64),
    restricted_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, subject_token)
);
ALTER TABLE knowledge_agent_trace_subject_restriction ENABLE ROW LEVEL SECURITY;
ALTER TABLE knowledge_agent_trace_subject_restriction FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON knowledge_agent_trace_subject_restriction;
CREATE POLICY myelin_tenant_isolation ON knowledge_agent_trace_subject_restriction
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
        Migration::plain(
            "0100_knowledge_agent_trace_encryption",
            AGENT_TRACE_ENCRYPTION_MIGRATION,
        ),
        Migration::plain(
            "0101_knowledge_agent_trace_subject_erasure",
            AGENT_TRACE_SUBJECT_ERASURE_MIGRATION,
        ),
        Migration::plain(
            "0102_knowledge_agent_trace_subject_restriction",
            AGENT_TRACE_SUBJECT_RESTRICTION_MIGRATION,
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

#[derive(Debug, Serialize, Deserialize)]
struct AgentTracePrivatePayload {
    answer: String,
    trace_body: Value,
}

struct SealedAgentTrace {
    key_ref: PiiKeyRef,
    nonce: [u8; NONCE_LEN],
    ciphertext: Vec<u8>,
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
pub struct AgentTraceSubjectEraseReceipt {
    pub traces_erased: u64,
    pub already_erased: bool,
    pub key_destroyed: bool,
    pub key_unrecoverable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentTraceError {
    Invalid(&'static str),
    Conflict,
    Erased,
    Restricted,
    Storage(String),
}

impl core::fmt::Display for AgentTraceError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Invalid(field) => write!(formatter, "invalid agent trace {field}"),
            Self::Conflict => formatter
                .write_str("agent run already has a different immutable trace; replay refused"),
            Self::Erased => formatter.write_str("agent run trace was erased; recreation refused"),
            Self::Restricted => {
                formatter.write_str("agent trace processing is restricted for this subject")
            }
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
    kms: Arc<KmsEngine>,
}

impl DurableAgentTraceStore {
    pub fn with_runtime(
        provider: SubstrateProvider,
        runtime: tokio::runtime::Handle,
        kms: Arc<KmsEngine>,
    ) -> Self {
        Self {
            provider,
            runtime,
            kms,
        }
    }

    fn drive<F: std::future::Future>(&self, future: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.runtime.block_on(future)),
            Err(_) => self.runtime.block_on(future),
        }
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
        let kms = self.kms.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT trace.run_id, trace.artifact_ref, trace.agent_principal, \
                                trace.requested_by, trace.answer, trace.trace_body, \
                                trace.payload_key_ref, trace.payload_nonce, \
                                trace.payload_ciphertext, trace.charged_micro, trace.created_at \
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
                    row.map(|row| agent_trace_result_from_row(row, &kms, &tenant, &region))
                        .transpose()
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

    pub async fn count_for_subject(
        &self,
        tenant: &str,
        requested_by: &str,
    ) -> Result<u64, ProviderError> {
        let tenant = tenant.to_string();
        let requested_by = requested_by.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let count = sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM knowledge_agent_trace \
                          WHERE tenant_id = $1 AND region = $2 AND requested_by = $3",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&requested_by)
                    .fetch_one(connection)
                    .await
                    .map_err(trace_query)?;
                    u64::try_from(count)
                        .map_err(|_| PgError::Query("agent trace count is negative".into()))
                })
            })
            .await
    }

    pub async fn erase_for_subject(
        &self,
        tenant: &str,
        requested_by: &str,
    ) -> Result<AgentTraceSubjectEraseReceipt, AgentTraceError> {
        let tenant_id = tenant.to_string();
        let subject = requested_by.to_string();
        let region = self.provider.config().region.clone();
        let subject_token = subject_token(&tenant_id, &region, &subject);
        let transaction_tenant = tenant_id.clone();
        let database_receipt = self
            .provider
            .with_tenant_tx(&transaction_tenant, move |connection| {
                Box::pin(async move {
                    erase_subject_on_connection(
                        connection,
                        &tenant_id,
                        &region,
                        &subject,
                        &subject_token,
                    )
                    .await
                })
            })
            .await
            .map_err(|error| AgentTraceError::Storage(error.to_string()))?;
        let tenant = TenantId(tenant.to_string());
        let class = KeyClass::Subject(requested_by.to_string());
        let key_destroyed = self
            .kms
            .destroy_dek(&DekId::new(tenant.clone(), class.clone()));
        let key_unrecoverable = self
            .kms
            .resolve_dek(
                &PiiKeyRef::new(tenant, 0, class),
                &Region(self.provider.config().region.clone()),
            )
            .is_err();
        Ok(AgentTraceSubjectEraseReceipt {
            traces_erased: database_receipt.traces_erased,
            already_erased: database_receipt.already_erased,
            key_destroyed,
            key_unrecoverable,
        })
    }

    pub async fn set_subject_restriction(
        &self,
        tenant: &str,
        requested_by: &str,
        restricted: bool,
    ) -> Result<bool, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let token = subject_token(&tenant, &region, requested_by);
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    lock_subject(connection, &tenant, &region, &token).await?;
                    let changed = if restricted {
                        sqlx::query(
                            "INSERT INTO knowledge_agent_trace_subject_restriction \
                               (tenant_id, region, subject_token) VALUES ($1, $2, $3) \
                             ON CONFLICT DO NOTHING",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&token)
                        .execute(&mut *connection)
                        .await
                        .map_err(trace_query)?
                        .rows_affected()
                            == 1
                    } else {
                        sqlx::query(
                            "DELETE FROM knowledge_agent_trace_subject_restriction \
                              WHERE tenant_id = $1 AND region = $2 AND subject_token = $3",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&token)
                        .execute(&mut *connection)
                        .await
                        .map_err(trace_query)?
                        .rows_affected()
                            == 1
                    };
                    Ok(changed)
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
        let kms = self.kms.clone();
        let transaction_tenant = tenant_id.clone();
        let future = provider.with_tenant_tx(&transaction_tenant, move |connection| {
            Box::pin(async move {
                write_on_connection(
                    connection,
                    &tenant_id,
                    &region,
                    &persisted_ref,
                    &trace,
                    &kms,
                )
                .await
            })
        });
        let result = self
            .drive(future)
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

impl PersonalDataHolder for DurableAgentTraceStore {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        ensure_subject_tenant(subject, &tenant)?;
        let subject_id = &subject.principal.principal_id.0;
        let count = self
            .drive(self.count_for_subject(&tenant.0, subject_id))
            .map_err(|error| DsrError(error.to_string()))?;
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                "agent_fabric_trace",
                subject_id,
                &tenant.0,
                &format!("located:{count}:subject-keyed-agent-traces"),
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        ensure_subject_tenant(subject, &tenant)?;
        let subject_id = &subject.principal.principal_id.0;
        let count = self
            .drive(self.count_for_subject(&tenant.0, subject_id))
            .map_err(|error| DsrError(error.to_string()))?;
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                "agent_fabric_trace",
                subject_id,
                &tenant.0,
                &format!("exported:{count}:content-addressed-agent-traces"),
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                "agent_fabric_trace",
                &subject.principal.principal_id.0,
                &subject.principal.tenant.0,
                "immutable trace: rectify by writing a corrected content-addressed source",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let subject_id = &subject.principal.principal_id.0;
        let tenant = &subject.principal.tenant;
        let changed = self
            .drive(self.set_subject_restriction(&tenant.0, subject_id, on))
            .map_err(|error| DsrError(error.to_string()))?;
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                "agent_fabric_trace",
                subject_id,
                &tenant.0,
                &format!("trace-processing-restricted:{on};changed:{changed}"),
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        match scope {
            EraseScope::Subject { subject, tenant } => {
                ensure_subject_tenant(&subject, &tenant)?;
                let subject_id = subject.principal.principal_id.0;
                let erased = self
                    .drive(self.erase_for_subject(&tenant.0, &subject_id))
                    .map_err(|error| DsrError(error.to_string()))?;
                if !erased.key_unrecoverable {
                    return Err(DsrError(
                        "agent trace subject key still resolves after erasure".into(),
                    ));
                }
                Ok(EraseReceipt {
                    receipt: Receipt::content_addressed(
                        "erase",
                        "agent_fabric_trace",
                        &subject_id,
                        &tenant.0,
                        &format!(
                            "crypto-shredded:{}:traces;subject-marker:durable",
                            erased.traces_erased
                        ),
                        Some(0),
                        0,
                    ),
                })
            }
            EraseScope::Tenant(tenant) => {
                let region = Region(self.provider.config().region.clone());
                self.kms.destroy_kek(&KekId::new(tenant.clone(), region));
                Ok(EraseReceipt {
                    receipt: Receipt::content_addressed(
                        "erase",
                        "agent_fabric_trace",
                        "",
                        &tenant.0,
                        "tenant KEK destroyed; agent traces are unrecoverable with tenant offboarding",
                        Some(0),
                        0,
                    ),
                })
            }
        }
    }
}

fn ensure_subject_tenant(subject: &SubjectRef, tenant: &TenantId) -> DsrResult<()> {
    if &subject.principal.tenant != tenant {
        return Err(DsrError(
            "agent trace DSR tenant does not match the data subject".into(),
        ));
    }
    Ok(())
}

async fn write_on_connection(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    artifact_ref: &ArtifactRef,
    trace: &AgentTraceWrite,
    kms: &KmsEngine,
) -> Result<Result<bool, AgentTraceError>, PgError> {
    let token = subject_token(tenant, region, &trace.requested_by);
    lock_subject(connection, tenant, region, &token).await?;
    let subject_erased = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM knowledge_agent_trace_subject_erasure \
          WHERE tenant_id = $1 AND region = $2 AND subject_token = $3)",
    )
    .bind(tenant)
    .bind(region)
    .bind(&token)
    .fetch_one(&mut *connection)
    .await
    .map_err(trace_query)?;
    if subject_erased {
        return Ok(Err(AgentTraceError::Erased));
    }
    let subject_restricted = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM knowledge_agent_trace_subject_restriction \
          WHERE tenant_id = $1 AND region = $2 AND subject_token = $3)",
    )
    .bind(tenant)
    .bind(region)
    .bind(&token)
    .fetch_one(&mut *connection)
    .await
    .map_err(trace_query)?;
    if subject_restricted {
        return Ok(Err(AgentTraceError::Restricted));
    }
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
    let sealed = match seal_trace(
        kms,
        &TenantId(tenant.to_string()),
        region,
        artifact_ref,
        trace,
    ) {
        Ok(sealed) => sealed,
        Err(error) => return Ok(Err(error)),
    };
    let charged_micro = i64::try_from(trace.charged_micro)
        .map_err(|_| PgError::Query("agent trace charge exceeds i64".into()))?;
    let inserted = sqlx::query(
        "INSERT INTO knowledge_agent_trace \
           (tenant_id, region, run_id, artifact_ref, agent_principal, requested_by, \
            answer, trace_body, charged_micro, payload_key_ref, payload_nonce, \
            payload_ciphertext) \
         VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL, $7, $8, $9, $10) \
         ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(region)
    .bind(&trace.run_id)
    .bind(&artifact_ref.0)
    .bind(&trace.agent_principal)
    .bind(&trace.requested_by)
    .bind(charged_micro)
    .bind(sealed.key_ref.to_uri())
    .bind(sealed.nonce.as_slice())
    .bind(&sealed.ciphertext)
    .execute(&mut *connection)
    .await
    .map_err(trace_query)?;
    if inserted.rows_affected() == 1 {
        return Ok(Ok(false));
    }

    let existing = sqlx::query(
        "SELECT artifact_ref, agent_principal, requested_by, charged_micro \
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
            .try_get::<i64, _>("charged_micro")
            .map_err(trace_query)?
            == charged_micro;
    Ok(if same {
        Ok(true)
    } else {
        Err(AgentTraceError::Conflict)
    })
}

struct DatabaseSubjectEraseReceipt {
    traces_erased: u64,
    already_erased: bool,
}

async fn erase_subject_on_connection(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    requested_by: &str,
    subject_token: &str,
) -> Result<DatabaseSubjectEraseReceipt, PgError> {
    lock_subject(connection, tenant, region, subject_token).await?;
    let already_erased = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM knowledge_agent_trace_subject_erasure \
          WHERE tenant_id = $1 AND region = $2 AND subject_token = $3)",
    )
    .bind(tenant)
    .bind(region)
    .bind(subject_token)
    .fetch_one(&mut *connection)
    .await
    .map_err(trace_query)?;
    sqlx::query(
        "INSERT INTO knowledge_agent_trace_subject_erasure \
           (tenant_id, region, subject_token) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(region)
    .bind(subject_token)
    .execute(&mut *connection)
    .await
    .map_err(trace_query)?;
    sqlx::query(
        "INSERT INTO knowledge_agent_trace_erasure \
           (tenant_id, region, run_id, artifact_ref) \
         SELECT tenant_id, region, run_id, artifact_ref FROM knowledge_agent_trace \
          WHERE tenant_id = $1 AND region = $2 AND requested_by = $3 \
         ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(region)
    .bind(requested_by)
    .execute(&mut *connection)
    .await
    .map_err(trace_query)?;
    let deleted = sqlx::query(
        "DELETE FROM knowledge_agent_trace \
          WHERE tenant_id = $1 AND region = $2 AND requested_by = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(requested_by)
    .execute(&mut *connection)
    .await
    .map_err(trace_query)?;
    Ok(DatabaseSubjectEraseReceipt {
        traces_erased: deleted.rows_affected(),
        already_erased,
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

async fn lock_subject(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    subject_token: &str,
) -> Result<(), PgError> {
    let lock_key = format!("agent-trace-subject\u{1f}{tenant}\u{1f}{region}\u{1f}{subject_token}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(connection)
        .await
        .map_err(trace_query)?;
    Ok(())
}

fn subject_token(tenant: &str, region: &str, requested_by: &str) -> String {
    let body =
        format!("myelin.agent_trace.subject.v1\u{1f}{tenant}\u{1f}{region}\u{1f}{requested_by}");
    blake3::hash(body.as_bytes()).to_hex().to_string()
}

fn seal_trace(
    kms: &KmsEngine,
    tenant: &TenantId,
    region: &str,
    artifact_ref: &ArtifactRef,
    trace: &AgentTraceWrite,
) -> Result<SealedAgentTrace, AgentTraceError> {
    let payload = serde_json::to_vec(&AgentTracePrivatePayload {
        answer: trace.answer.clone(),
        trace_body: trace.trace_body.clone(),
    })
    .map_err(|error| AgentTraceError::Storage(error.to_string()))?;
    let aad = trace_aad(&tenant.0, region, &trace.run_id, &artifact_ref.0);
    let encrypted = ColumnCryptor::new(kms, Region(region.to_string()))
        .encrypt_with_aad(
            tenant,
            Some(&SubjectId::new(&trace.requested_by)),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            &payload,
            &aad,
        )
        .map_err(|error| AgentTraceError::Storage(error.to_string()))?;
    Ok(SealedAgentTrace {
        key_ref: encrypted.key_ref,
        nonce: encrypted.nonce,
        ciphertext: encrypted.ciphertext,
    })
}

fn agent_trace_result_from_row(
    row: sqlx::postgres::PgRow,
    kms: &KmsEngine,
    tenant: &str,
    region: &str,
) -> Result<AgentTraceResult, PgError> {
    let run_id = row.try_get::<String, _>("run_id").map_err(trace_query)?;
    let artifact_ref = ArtifactRef(row.try_get("artifact_ref").map_err(trace_query)?);
    let agent_principal = row
        .try_get::<String, _>("agent_principal")
        .map_err(trace_query)?;
    let requested_by = row
        .try_get::<String, _>("requested_by")
        .map_err(trace_query)?;
    let charged = row
        .try_get::<i64, _>("charged_micro")
        .map_err(trace_query)?;
    let charged_micro = u64::try_from(charged)
        .map_err(|_| PgError::Query("agent trace has a negative charge".into()))?;
    let answer = match (
        row.try_get::<Option<String>, _>("answer")
            .map_err(trace_query)?,
        row.try_get::<Option<Value>, _>("trace_body")
            .map_err(trace_query)?,
    ) {
        (Some(answer), Some(_)) => answer,
        (None, None) => {
            let key_ref = row
                .try_get::<Option<String>, _>("payload_key_ref")
                .map_err(trace_query)?
                .and_then(|value| PiiKeyRef::parse(&value))
                .ok_or_else(|| PgError::Query("agent trace has an invalid key reference".into()))?;
            if key_ref.tenant.as_str() != tenant
                || key_ref.class != KeyClass::Subject(requested_by.clone())
            {
                return Err(PgError::Query(
                    "agent trace encryption scope does not match its attribution".into(),
                ));
            }
            let nonce = row
                .try_get::<Option<Vec<u8>>, _>("payload_nonce")
                .map_err(trace_query)?
                .ok_or_else(|| PgError::Query("agent trace has no encryption nonce".into()))?;
            let nonce: [u8; NONCE_LEN] = nonce
                .try_into()
                .map_err(|_| PgError::Query("agent trace has an invalid nonce".into()))?;
            let ciphertext = row
                .try_get::<Option<Vec<u8>>, _>("payload_ciphertext")
                .map_err(trace_query)?
                .ok_or_else(|| PgError::Query("agent trace has no ciphertext".into()))?;
            let aad = trace_aad(tenant, region, &run_id, &artifact_ref.0);
            let plaintext = ColumnCryptor::new(kms, Region(region.to_string()))
                .decrypt_with_aad(
                    &EncryptedColumn {
                        key_ref,
                        nonce,
                        ciphertext,
                    },
                    &aad,
                )
                .map_err(|error| PgError::Query(format!("agent trace decrypt failed: {error}")))?;
            let payload: AgentTracePrivatePayload =
                serde_json::from_slice(&plaintext).map_err(|error| {
                    PgError::Query(format!("agent trace payload is invalid: {error}"))
                })?;
            let candidate = AgentTraceWrite {
                run_id: run_id.clone(),
                agent_principal: agent_principal.clone(),
                requested_by,
                answer: payload.answer.clone(),
                trace_body: payload.trace_body,
                charged_micro,
            };
            candidate
                .validate()
                .map_err(|error| PgError::Query(error.to_string()))?;
            if candidate
                .artifact_ref(&TenantId(tenant.to_string()))
                .map_err(|error| PgError::Query(error.to_string()))?
                != artifact_ref
            {
                return Err(PgError::Query(
                    "agent trace payload does not match its content address".into(),
                ));
            }
            payload.answer
        }
        _ => {
            return Err(PgError::Query(
                "agent trace has a mixed plaintext/encrypted payload".into(),
            ))
        }
    };
    Ok(AgentTraceResult {
        run_id,
        artifact_ref,
        agent_principal,
        answer,
        charged_micro,
        created_at: row
            .try_get::<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>, _>("created_at")
            .map_err(trace_query)?
            .to_rfc3339(),
    })
}

fn trace_aad(tenant: &str, region: &str, run_id: &str, artifact_ref: &str) -> Vec<u8> {
    format!("myelin.agent_trace.v1\u{1f}{tenant}\u{1f}{region}\u{1f}{run_id}\u{1f}{artifact_ref}")
        .into_bytes()
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
        assert!(AGENT_TRACE_ENCRYPTION_MIGRATION.contains("payload_ciphertext bytea"));
        assert!(AGENT_TRACE_ENCRYPTION_MIGRATION.contains("answer IS NULL AND trace_body IS NULL"));
        assert!(AGENT_TRACE_SUBJECT_ERASURE_MIGRATION.contains("subject_token text"));
        assert!(AGENT_TRACE_SUBJECT_ERASURE_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(AGENT_TRACE_SUBJECT_RESTRICTION_MIGRATION.contains("restricted_at"));
        assert!(AGENT_TRACE_SUBJECT_RESTRICTION_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
    }
}
