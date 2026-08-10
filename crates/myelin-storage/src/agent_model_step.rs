use myelin_tenancy::TenantId;
use serde_json::Value;
use sqlx::Row;

use crate::agent_journal_privacy::{agent_subject_status, AgentSubjectStatus};
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelStepCompletion {
    Applied,
    Replayed,
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
}

impl AgentModelStepStore {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self::with_runtime(provider, tokio::runtime::Handle::current())
    }

    pub fn with_runtime(provider: SubstrateProvider, runtime: tokio::runtime::Handle) -> Self {
        Self { provider, runtime }
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
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_id = run_id.to_string();
        let step_key = step_key.to_string();
        let request_hash = request_hash.to_string();
        let requested_by = requested_by.to_string();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                begin_on_connection(
                    connection,
                    &tenant_s,
                    &region,
                    &run_id,
                    &step_key,
                    &request_hash,
                    &requested_by,
                )
                .await
            })
        }))?
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
        let region = self.region();
        let tenant_s = tenant.0.clone();
        let run_id = run_id.to_string();
        let step_key = step_key.to_string();
        let request_hash = request_hash.to_string();
        let requested_by = requested_by.to_string();
        let response = response.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                complete_on_connection(
                    connection,
                    &tenant_s,
                    &region,
                    &run_id,
                    &step_key,
                    &request_hash,
                    &requested_by,
                    &response,
                )
                .await
            })
        }))?
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
    tenant: &str,
    region: &str,
    run_id: &str,
    step_key: &str,
    request_hash: &str,
    requested_by: &str,
) -> Result<Result<ModelStepBegin, ModelStepError>, PgError> {
    match agent_subject_status(connection, tenant, region, requested_by).await? {
        AgentSubjectStatus::Active => {}
        AgentSubjectStatus::Erased => return Ok(Err(ModelStepError::Erased)),
        AgentSubjectStatus::Restricted => return Ok(Err(ModelStepError::Restricted)),
    }
    let inserted = sqlx::query(
        "INSERT INTO agent_model_step
           (tenant_id, region, run_id, step_key, request_hash, requested_by, state)
         VALUES ($1, $2, $3, $4, $5, $6, 'started') ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(step_key)
    .bind(request_hash)
    .bind(requested_by)
    .execute(&mut *connection)
    .await
    .map_err(store_query)?;
    if inserted.rows_affected() == 1 {
        return Ok(Ok(ModelStepBegin::Started));
    }

    let row = sqlx::query(
        "SELECT request_hash, requested_by, state, response FROM agent_model_step
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND step_key = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(step_key)
    .fetch_one(&mut *connection)
    .await
    .map_err(store_query)?;
    let stored_hash: String = row.try_get("request_hash").map_err(store_query)?;
    let stored_subject: String = row.try_get("requested_by").map_err(store_query)?;
    if stored_hash != request_hash || stored_subject != requested_by {
        return Ok(Err(ModelStepError::Conflict));
    }
    match row
        .try_get::<String, _>("state")
        .map_err(store_query)?
        .as_str()
    {
        "started" => Ok(Ok(ModelStepBegin::InDoubt)),
        "completed" => {
            let response = row
                .try_get::<Option<Value>, _>("response")
                .map_err(store_query)?
                .ok_or_else(|| PgError::Query("completed model step has no response".into()))?;
            Ok(Ok(ModelStepBegin::Completed(response)))
        }
        _ => Err(PgError::Query("model step has an invalid state".into())),
    }
}

async fn complete_on_connection(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    run_id: &str,
    step_key: &str,
    request_hash: &str,
    requested_by: &str,
    response: &Value,
) -> Result<Result<ModelStepCompletion, ModelStepError>, PgError> {
    match agent_subject_status(connection, tenant, region, requested_by).await? {
        AgentSubjectStatus::Active => {}
        AgentSubjectStatus::Erased => return Ok(Err(ModelStepError::Erased)),
        AgentSubjectStatus::Restricted => return Ok(Err(ModelStepError::Restricted)),
    }
    let row = sqlx::query(
        "SELECT request_hash, requested_by, state, response FROM agent_model_step
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND step_key = $4 FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(step_key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(store_query)?;
    let Some(row) = row else {
        return Ok(Err(ModelStepError::Missing));
    };
    let stored_hash: String = row.try_get("request_hash").map_err(store_query)?;
    let stored_subject: String = row.try_get("requested_by").map_err(store_query)?;
    if stored_hash != request_hash || stored_subject != requested_by {
        return Ok(Err(ModelStepError::Conflict));
    }
    let state: String = row.try_get("state").map_err(store_query)?;
    if state == "completed" {
        let stored = row
            .try_get::<Option<Value>, _>("response")
            .map_err(store_query)?
            .ok_or_else(|| PgError::Query("completed model step has no response".into()))?;
        return Ok(if stored == *response {
            Ok(ModelStepCompletion::Replayed)
        } else {
            Err(ModelStepError::Conflict)
        });
    }
    if state != "started" {
        return Err(PgError::Query("model step has an invalid state".into()));
    }

    sqlx::query(
        "UPDATE agent_model_step SET state = 'completed', response = $5, completed_at = now()
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND step_key = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(step_key)
    .bind(response)
    .execute(connection)
    .await
    .map_err(store_query)?;
    Ok(Ok(ModelStepCompletion::Applied))
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
