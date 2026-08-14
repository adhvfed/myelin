use std::sync::Arc;

use myelin_tenancy::TenantId;
use serde_json::Value;
use sqlx::Row;

use crate::agent_journal_privacy::{
    agent_subject_status, open_journal_payload, seal_journal_payload, AgentSubjectLocator,
    AgentSubjectStatus, JournalPayloadContext, JournalPayloadKind,
};
use crate::kms::KmsEngine;
use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};

const MAX_ID_BYTES: usize = 512;
const MAX_PRINCIPAL_BYTES: usize = 255;
const REQUEST_HASH_BYTES: usize = 64;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub const AGENT_MODEL_STEP_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS agent_model_step (
    tenant_id    text        NOT NULL,
    region       text        NOT NULL,
    run_id       text        NOT NULL CHECK (length(run_id) BETWEEN 1 AND 512),
    step_key     text        NOT NULL CHECK (length(step_key) BETWEEN 1 AND 512),
    request_hash text        NOT NULL CHECK (length(request_hash) = 64),
    state        text        NOT NULL CHECK (state IN ('started','completed')),
    response     jsonb,
    started_at   timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    PRIMARY KEY (tenant_id, region, run_id, step_key),
    CHECK ((state = 'started' AND response IS NULL AND completed_at IS NULL) OR
           (state = 'completed' AND response IS NOT NULL AND completed_at IS NOT NULL)),
    CHECK (response IS NULL OR octet_length(response::text) <= 4194304)
);
ALTER TABLE agent_model_step ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_model_step FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_model_step;
CREATE POLICY myelin_tenant_isolation ON agent_model_step
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));";

pub const AGENT_MODEL_STEP_GUARD_MIGRATION: &str = "\
CREATE OR REPLACE FUNCTION myelin_guard_agent_model_step_update()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  IF OLD.state <> 'started' OR NEW.state <> 'completed' OR
     NEW.tenant_id IS DISTINCT FROM OLD.tenant_id OR
     NEW.region IS DISTINCT FROM OLD.region OR
     NEW.run_id IS DISTINCT FROM OLD.run_id OR
     NEW.step_key IS DISTINCT FROM OLD.step_key OR
     NEW.request_hash IS DISTINCT FROM OLD.request_hash OR
     NEW.started_at IS DISTINCT FROM OLD.started_at THEN
    RAISE EXCEPTION 'agent_model_step permits only its one-way completion transition';
  END IF;
  RETURN NEW;
END
$myelin$;
DROP TRIGGER IF EXISTS agent_model_step_guard_update ON agent_model_step;
CREATE TRIGGER agent_model_step_guard_update
BEFORE UPDATE ON agent_model_step
FOR EACH ROW EXECUTE FUNCTION myelin_guard_agent_model_step_update();";

pub fn agent_model_step_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0096_agent_model_step", AGENT_MODEL_STEP_MIGRATION),
        Migration::plain(
            "0097_agent_model_step_guard",
            AGENT_MODEL_STEP_GUARD_MIGRATION,
        ),
    ])
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModelStepBegin {
    Started,
    Completed(Value),
    InDoubt,
    Unreplayable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelStepCompletion {
    Applied,
    Replayed,
}

enum StoredModelStepBegin {
    Started,
    Completed(StoredModelResponse),
    InDoubt,
    Unreplayable,
}

enum StoredModelStepCompletion {
    Applied,
    Replayed(StoredModelResponse),
}

struct StoredModelResponse {
    key_ref: String,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelStepError {
    InvalidInput(&'static str),
    Conflict,
    Missing,
    Erased,
    Restricted,
    Storage(String),
}

impl core::fmt::Display for ModelStepError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidInput(detail) => write!(formatter, "invalid model step: {detail}"),
            Self::Conflict => write!(
                formatter,
                "model step identity was already used for different immutable content"
            ),
            Self::Missing => write!(formatter, "model step was completed before it was started"),
            Self::Erased => formatter.write_str("the requesting subject was erased"),
            Self::Restricted => {
                formatter.write_str("model processing is restricted for the requesting subject")
            }
            Self::Storage(detail) => write!(formatter, "model step journal unavailable: {detail}"),
        }
    }
}

impl std::error::Error for ModelStepError {}

#[derive(Clone)]
pub struct AgentModelStepStore {
    provider: SubstrateProvider,
    runtime: tokio::runtime::Handle,
    kms: Arc<KmsEngine>,
}

impl AgentModelStepStore {
    pub fn new(provider: SubstrateProvider, kms: Arc<KmsEngine>) -> Self {
        Self::with_runtime(provider, tokio::runtime::Handle::current(), kms)
    }

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

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    fn block<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, ProviderError>>,
    ) -> Result<T, ModelStepError> {
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.runtime.block_on(future)),
            Err(_) => self.runtime.block_on(future),
        }
        .map_err(|error| ModelStepError::Storage(error.to_string()))
    }

    pub fn begin(
        &self,
        tenant: &TenantId,
        run_id: &str,
        step_key: &str,
        request_hash: &str,
        requested_by: &str,
    ) -> Result<ModelStepBegin, ModelStepError> {
        validate_identity(run_id, step_key, request_hash, requested_by)?;
        let context = JournalPayloadContext::new(
            &tenant.0,
            &self.region(),
            run_id,
            step_key,
            request_hash,
            requested_by,
            JournalPayloadKind::ModelResponse,
        );
        let locator = context.subject_locator(&self.kms);
        let transaction_context = context.clone();
        let stored = self.block(self.provider.with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                begin_on_connection(connection, &transaction_context, &locator).await
            })
        }))??;
        match stored {
            StoredModelStepBegin::Started => Ok(ModelStepBegin::Started),
            StoredModelStepBegin::InDoubt => Ok(ModelStepBegin::InDoubt),
            StoredModelStepBegin::Unreplayable => Ok(ModelStepBegin::Unreplayable),
            StoredModelStepBegin::Completed(payload) => {
                open_model_response(payload, &self.kms, &context).map(ModelStepBegin::Completed)
            }
        }
    }

    pub fn complete(
        &self,
        tenant: &TenantId,
        run_id: &str,
        step_key: &str,
        request_hash: &str,
        requested_by: &str,
        response: &Value,
    ) -> Result<ModelStepCompletion, ModelStepError> {
        validate_identity(run_id, step_key, request_hash, requested_by)?;
        let encoded = serde_json::to_vec(response)
            .map_err(|_| ModelStepError::InvalidInput("response is not valid JSON"))?;
        if encoded.len() > MAX_RESPONSE_BYTES {
            return Err(ModelStepError::InvalidInput("response exceeds four MiB"));
        }
        let response = response.clone();
        let context = JournalPayloadContext::new(
            &tenant.0,
            &self.region(),
            run_id,
            step_key,
            request_hash,
            requested_by,
            JournalPayloadKind::ModelResponse,
        );
        let locator = context.subject_locator(&self.kms);
        let transaction_context = context.clone();
        let kms = self.kms.clone();
        let stored = self.block(self.provider.with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                complete_on_connection(connection, &transaction_context, &encoded, &locator, &kms)
                    .await
            })
        }))??;
        match stored {
            StoredModelStepCompletion::Applied => Ok(ModelStepCompletion::Applied),
            StoredModelStepCompletion::Replayed(payload) => {
                if open_model_response(payload, &self.kms, &context)? == response {
                    Ok(ModelStepCompletion::Replayed)
                } else {
                    Err(ModelStepError::Conflict)
                }
            }
        }
    }
}

fn validate_identity(
    run_id: &str,
    step_key: &str,
    request_hash: &str,
    requested_by: &str,
) -> Result<(), ModelStepError> {
    if run_id.is_empty() || run_id.len() > MAX_ID_BYTES {
        return Err(ModelStepError::InvalidInput(
            "run id must contain 1 to 512 bytes",
        ));
    }
    if step_key.is_empty() || step_key.len() > MAX_ID_BYTES {
        return Err(ModelStepError::InvalidInput(
            "step key must contain 1 to 512 bytes",
        ));
    }
    if request_hash.len() != REQUEST_HASH_BYTES
        || !request_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ModelStepError::InvalidInput(
            "request hash must be 64 hexadecimal bytes",
        ));
    }
    if requested_by.is_empty() || requested_by.len() > MAX_PRINCIPAL_BYTES {
        return Err(ModelStepError::InvalidInput(
            "requesting principal must contain 1 to 255 bytes",
        ));
    }
    Ok(())
}

async fn begin_on_connection(
    connection: &mut sqlx::PgConnection,
    context: &JournalPayloadContext,
    locator: &AgentSubjectLocator,
) -> Result<Result<StoredModelStepBegin, ModelStepError>, PgError> {
    match agent_subject_status(connection, &context.tenant, &context.region, locator).await? {
        AgentSubjectStatus::Active => {}
        AgentSubjectStatus::Erasing | AgentSubjectStatus::Erased => {
            return Ok(Err(ModelStepError::Erased))
        }
        AgentSubjectStatus::Restricted => return Ok(Err(ModelStepError::Restricted)),
    }
    let inserted = sqlx::query(
        "INSERT INTO agent_model_step
           (tenant_id, region, run_id, step_key, request_hash, requested_by, state)
         VALUES ($1, $2, $3, $4, $5, $6, 'started') ON CONFLICT DO NOTHING",
    )
    .bind(&context.tenant)
    .bind(&context.region)
    .bind(&context.run_id)
    .bind(&context.position_key)
    .bind(&context.request_hash)
    .bind(&context.requested_by)
    .execute(&mut *connection)
    .await
    .map_err(store_query)?;
    if inserted.rows_affected() == 1 {
        return Ok(Ok(StoredModelStepBegin::Started));
    }

    let row = sqlx::query(
        "SELECT request_hash, requested_by, state, response_key_ref, response_nonce, \
                response_ciphertext FROM agent_model_step
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND step_key = $4",
    )
    .bind(&context.tenant)
    .bind(&context.region)
    .bind(&context.run_id)
    .bind(&context.position_key)
    .fetch_one(&mut *connection)
    .await
    .map_err(store_query)?;
    let stored_hash: String = row.try_get("request_hash").map_err(store_query)?;
    let stored_subject: String = row.try_get("requested_by").map_err(store_query)?;
    if stored_hash != context.request_hash || stored_subject != context.requested_by {
        return Ok(Err(ModelStepError::Conflict));
    }
    match row
        .try_get::<String, _>("state")
        .map_err(store_query)?
        .as_str()
    {
        "started" => Ok(Ok(StoredModelStepBegin::InDoubt)),
        "redacted" => Ok(Ok(StoredModelStepBegin::Unreplayable)),
        "completed" => stored_model_response_from_row(&row)
            .map(StoredModelStepBegin::Completed)
            .map(Ok),
        _ => Err(PgError::Query("model step has an invalid state".into())),
    }
}

async fn complete_on_connection(
    connection: &mut sqlx::PgConnection,
    context: &JournalPayloadContext,
    encoded_response: &[u8],
    locator: &AgentSubjectLocator,
    kms: &KmsEngine,
) -> Result<Result<StoredModelStepCompletion, ModelStepError>, PgError> {
    match agent_subject_status(connection, &context.tenant, &context.region, locator).await? {
        AgentSubjectStatus::Active => {}
        AgentSubjectStatus::Erasing | AgentSubjectStatus::Erased => {
            return Ok(Err(ModelStepError::Erased))
        }
        AgentSubjectStatus::Restricted => return Ok(Err(ModelStepError::Restricted)),
    }
    let row = sqlx::query(
        "SELECT request_hash, requested_by, state, response_key_ref, response_nonce, \
                response_ciphertext FROM agent_model_step
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND step_key = $4 FOR UPDATE",
    )
    .bind(&context.tenant)
    .bind(&context.region)
    .bind(&context.run_id)
    .bind(&context.position_key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(store_query)?;
    let Some(row) = row else {
        return Ok(Err(ModelStepError::Missing));
    };
    let stored_hash: String = row.try_get("request_hash").map_err(store_query)?;
    let stored_subject: String = row.try_get("requested_by").map_err(store_query)?;
    if stored_hash != context.request_hash || stored_subject != context.requested_by {
        return Ok(Err(ModelStepError::Conflict));
    }
    let state: String = row.try_get("state").map_err(store_query)?;
    if state == "completed" {
        return stored_model_response_from_row(&row)
            .map(StoredModelStepCompletion::Replayed)
            .map(Ok);
    }
    if state != "started" {
        return Ok(Err(ModelStepError::Conflict));
    }
    let sealed = seal_journal_payload(kms, context, encoded_response)?;

    sqlx::query(
        "UPDATE agent_model_step \
            SET state = 'completed', response = NULL, response_key_ref = $5, \
                response_nonce = $6, response_ciphertext = $7, completed_at = now()
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND step_key = $4",
    )
    .bind(&context.tenant)
    .bind(&context.region)
    .bind(&context.run_id)
    .bind(&context.position_key)
    .bind(sealed.key_ref.to_uri())
    .bind(sealed.nonce.as_slice())
    .bind(&sealed.ciphertext)
    .execute(connection)
    .await
    .map_err(store_query)?;
    Ok(Ok(StoredModelStepCompletion::Applied))
}

fn stored_model_response_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredModelResponse, PgError> {
    let key_ref = row
        .try_get::<Option<String>, _>("response_key_ref")
        .map_err(store_query)?
        .ok_or_else(|| PgError::Query("completed model step has no encryption key".into()))?;
    let nonce = row
        .try_get::<Option<Vec<u8>>, _>("response_nonce")
        .map_err(store_query)?
        .ok_or_else(|| PgError::Query("completed model step has no encryption nonce".into()))?;
    let ciphertext = row
        .try_get::<Option<Vec<u8>>, _>("response_ciphertext")
        .map_err(store_query)?
        .ok_or_else(|| PgError::Query("completed model step has no ciphertext".into()))?;
    Ok(StoredModelResponse {
        key_ref,
        nonce,
        ciphertext,
    })
}

fn open_model_response(
    payload: StoredModelResponse,
    kms: &KmsEngine,
    context: &JournalPayloadContext,
) -> Result<Value, ModelStepError> {
    let plaintext = open_journal_payload(
        kms,
        context,
        &payload.key_ref,
        &payload.nonce,
        payload.ciphertext,
    )
    .map_err(|error| ModelStepError::Storage(error.to_string()))?;
    serde_json::from_slice(&plaintext)
        .map_err(|_| ModelStepError::Storage("decrypted model response is invalid JSON".into()))
}

fn store_query(error: sqlx::Error) -> PgError {
    PgError::Query(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_encodes_the_at_most_once_state_machine() {
        let ddl = AGENT_MODEL_STEP_MIGRATION;
        assert!(ddl.contains("PRIMARY KEY (tenant_id, region, run_id, step_key)"));
        assert!(ddl.contains("state IN ('started','completed')"));
        assert!(ddl.contains("state = 'started' AND response IS NULL"));
        assert!(ddl.contains("state = 'completed' AND response IS NOT NULL"));
        assert!(ddl.contains("FORCE ROW LEVEL SECURITY"));
        assert!(ddl.contains("octet_length(response::text) <= 4194304"));
        let guard = AGENT_MODEL_STEP_GUARD_MIGRATION;
        assert!(guard.contains("OLD.state <> 'started' OR NEW.state <> 'completed'"));
        assert!(guard.contains("BEFORE UPDATE ON agent_model_step"));
    }

    #[test]
    fn malformed_operation_identity_is_rejected_before_storage() {
        assert!(validate_identity("run", "step", &"a".repeat(64), "founder").is_ok());
        assert_eq!(
            validate_identity("run", "step", "not-a-hash", "founder"),
            Err(ModelStepError::InvalidInput(
                "request hash must be 64 hexadecimal bytes"
            )),
        );
        assert!(validate_identity("", "step", &"a".repeat(64), "founder").is_err());
        assert!(validate_identity("run", "", &"a".repeat(64), "founder").is_err());
        assert!(validate_identity("run", "step", &"a".repeat(64), "").is_err());
    }
}
