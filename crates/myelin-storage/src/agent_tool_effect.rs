use std::sync::Arc;

use myelin_tenancy::TenantId;
use sqlx::Row;

use crate::agent_journal_privacy::{
    agent_subject_status, open_journal_payload, seal_journal_payload, AgentSubjectStatus,
    JournalPayloadContext, JournalPayloadKind,
};
use crate::kms::KmsEngine;
use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};

const MAX_RUN_ID_BYTES: usize = 512;
const MAX_EFFECT_KEY_BYTES: usize = 1024;
const MAX_PRINCIPAL_BYTES: usize = 255;
const REQUEST_HASH_BYTES: usize = 64;
const MAX_RESULT_BYTES: usize = 256 * 1024;

pub const AGENT_TOOL_EFFECT_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS agent_tool_effect (
    tenant_id    text        NOT NULL,
    region       text        NOT NULL,
    run_id       text        NOT NULL CHECK (length(run_id) BETWEEN 1 AND 512),
    effect_key   text        NOT NULL CHECK (length(effect_key) BETWEEN 1 AND 1024),
    request_hash text        NOT NULL CHECK (length(request_hash) = 64),
    state        text        NOT NULL CHECK (state IN ('started','completed')),
    result_text  text,
    started_at   timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    PRIMARY KEY (tenant_id, region, run_id, effect_key),
    CHECK ((state = 'started' AND result_text IS NULL AND completed_at IS NULL) OR
           (state = 'completed' AND result_text IS NOT NULL AND completed_at IS NOT NULL)),
    CHECK (result_text IS NULL OR octet_length(result_text) <= 262144)
);
ALTER TABLE agent_tool_effect ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_tool_effect FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_tool_effect;
CREATE POLICY myelin_tenant_isolation ON agent_tool_effect
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const AGENT_TOOL_EFFECT_GUARD_MIGRATION: &str = r#"
CREATE OR REPLACE FUNCTION myelin_guard_agent_tool_effect_update()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  IF OLD.state <> 'started' OR NEW.state <> 'completed' OR
     NEW.tenant_id IS DISTINCT FROM OLD.tenant_id OR
     NEW.region IS DISTINCT FROM OLD.region OR
     NEW.run_id IS DISTINCT FROM OLD.run_id OR
     NEW.effect_key IS DISTINCT FROM OLD.effect_key OR
     NEW.request_hash IS DISTINCT FROM OLD.request_hash OR
     NEW.started_at IS DISTINCT FROM OLD.started_at THEN
    RAISE EXCEPTION 'agent_tool_effect permits only its one-way completion transition';
  END IF;
  RETURN NEW;
END
$myelin$;
DROP TRIGGER IF EXISTS agent_tool_effect_guard_update ON agent_tool_effect;
CREATE TRIGGER agent_tool_effect_guard_update
BEFORE UPDATE ON agent_tool_effect
FOR EACH ROW EXECUTE FUNCTION myelin_guard_agent_tool_effect_update();
"#;

pub fn agent_tool_effect_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0104_agent_tool_effect", AGENT_TOOL_EFFECT_MIGRATION),
        Migration::plain(
            "0104_agent_tool_effect_guard",
            AGENT_TOOL_EFFECT_GUARD_MIGRATION,
        ),
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEffectBegin {
    Execute,
    Completed(String),
    Unreplayable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEffectCompletion {
    Applied,
    Replayed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEffectError {
    InvalidInput(&'static str),
    Conflict,
    Missing,
    Erased,
    Restricted,
    Unreplayable,
    Storage(String),
}

impl core::fmt::Display for ToolEffectError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidInput(detail) => write!(formatter, "invalid tool effect: {detail}"),
            Self::Conflict => formatter
                .write_str("tool effect identity was already used for different immutable content"),
            Self::Missing => formatter.write_str("tool effect completed before it was started"),
            Self::Erased => formatter.write_str("the requesting subject was erased"),
            Self::Restricted => {
                formatter.write_str("tool processing is restricted for the requesting subject")
            }
            Self::Unreplayable => formatter.write_str(
                "the legacy tool result was privacy-redacted; repeating its effect is refused",
            ),
            Self::Storage(detail) => write!(formatter, "tool effect journal unavailable: {detail}"),
        }
    }
}

impl std::error::Error for ToolEffectError {}

#[derive(Clone)]
pub struct AgentToolEffectStore {
    provider: SubstrateProvider,
    runtime: tokio::runtime::Handle,
    kms: Arc<KmsEngine>,
}

impl AgentToolEffectStore {
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

    pub fn begin(
        &self,
        tenant: &TenantId,
        run_id: &str,
        effect_key: &str,
        request_hash: &str,
        requested_by: &str,
    ) -> Result<ToolEffectBegin, ToolEffectError> {
        validate_identity(run_id, effect_key, request_hash, requested_by)?;
        let region = self.provider.config().region.clone();
        let tenant_id = tenant.0.clone();
        let run_id = run_id.to_string();
        let effect_key = effect_key.to_string();
        let request_hash = request_hash.to_string();
        let requested_by = requested_by.to_string();
        let kms = self.kms.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                begin_on_connection(
                    connection,
                    &tenant_id,
                    &region,
                    &run_id,
                    &effect_key,
                    &request_hash,
                    &requested_by,
                    &kms,
                )
                .await
            })
        }))?
    }

    pub fn complete(
        &self,
        tenant: &TenantId,
        run_id: &str,
        effect_key: &str,
        request_hash: &str,
        requested_by: &str,
        result: &str,
    ) -> Result<ToolEffectCompletion, ToolEffectError> {
        validate_identity(run_id, effect_key, request_hash, requested_by)?;
        if result.len() > MAX_RESULT_BYTES {
            return Err(ToolEffectError::InvalidInput(
                "result exceeds its 256 KiB bound",
            ));
        }
        let region = self.provider.config().region.clone();
        let tenant_id = tenant.0.clone();
        let run_id = run_id.to_string();
        let effect_key = effect_key.to_string();
        let request_hash = request_hash.to_string();
        let requested_by = requested_by.to_string();
        let result = result.to_string();
        let kms = self.kms.clone();
        self.block(self.provider.with_tenant_tx(&tenant.0, move |connection| {
            Box::pin(async move {
                complete_on_connection(
                    connection,
                    &tenant_id,
                    &region,
                    &run_id,
                    &effect_key,
                    &request_hash,
                    &requested_by,
                    &result,
                    &kms,
                )
                .await
            })
        }))?
    }

    fn block<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, ProviderError>>,
    ) -> Result<T, ToolEffectError> {
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.runtime.block_on(future)),
            Err(_) => self.runtime.block_on(future),
        }
        .map_err(|error| ToolEffectError::Storage(error.to_string()))
    }
}

fn validate_identity(
    run_id: &str,
    effect_key: &str,
    request_hash: &str,
    requested_by: &str,
) -> Result<(), ToolEffectError> {
    if run_id.is_empty() || run_id.len() > MAX_RUN_ID_BYTES {
        return Err(ToolEffectError::InvalidInput(
            "run id must contain 1 to 512 bytes",
        ));
    }
    if effect_key.is_empty() || effect_key.len() > MAX_EFFECT_KEY_BYTES {
        return Err(ToolEffectError::InvalidInput(
            "effect key must contain 1 to 1024 bytes",
        ));
    }
    if request_hash.len() != REQUEST_HASH_BYTES
        || !request_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ToolEffectError::InvalidInput(
            "request hash must be 64 hexadecimal bytes",
        ));
    }
    if requested_by.is_empty() || requested_by.len() > MAX_PRINCIPAL_BYTES {
        return Err(ToolEffectError::InvalidInput(
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
    effect_key: &str,
    request_hash: &str,
    requested_by: &str,
    kms: &KmsEngine,
) -> Result<Result<ToolEffectBegin, ToolEffectError>, PgError> {
    match agent_subject_status(connection, tenant, region, requested_by).await? {
        AgentSubjectStatus::Active => {}
        AgentSubjectStatus::Erased => return Ok(Err(ToolEffectError::Erased)),
        AgentSubjectStatus::Restricted => return Ok(Err(ToolEffectError::Restricted)),
    }
    let inserted = sqlx::query(
        "INSERT INTO agent_tool_effect
           (tenant_id, region, run_id, effect_key, request_hash, requested_by, state)
         VALUES ($1, $2, $3, $4, $5, $6, 'started') ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(effect_key)
    .bind(request_hash)
    .bind(requested_by)
    .execute(&mut *connection)
    .await
    .map_err(store_query)?;
    if inserted.rows_affected() == 1 {
        return Ok(Ok(ToolEffectBegin::Execute));
    }

    let row = sqlx::query(
        "SELECT request_hash, requested_by, state, result_key_ref, result_nonce, \
                result_ciphertext FROM agent_tool_effect
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND effect_key = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(effect_key)
    .fetch_one(&mut *connection)
    .await
    .map_err(store_query)?;
    if row
        .try_get::<String, _>("request_hash")
        .map_err(store_query)?
        != request_hash
        || row
            .try_get::<String, _>("requested_by")
            .map_err(store_query)?
            != requested_by
    {
        return Ok(Err(ToolEffectError::Conflict));
    }
    match row
        .try_get::<String, _>("state")
        .map_err(store_query)?
        .as_str()
    {
        "started" => Ok(Ok(ToolEffectBegin::Execute)),
        "redacted" => Ok(Ok(ToolEffectBegin::Unreplayable)),
        "completed" => tool_result_from_row(
            &row,
            kms,
            &JournalPayloadContext {
                tenant,
                region,
                run_id,
                position_key: effect_key,
                request_hash,
                requested_by,
                kind: JournalPayloadKind::ToolResult,
            },
        )
        .map(ToolEffectBegin::Completed)
        .map(Ok),
        _ => Err(PgError::Query("tool effect has an invalid state".into())),
    }
}

async fn complete_on_connection(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    run_id: &str,
    effect_key: &str,
    request_hash: &str,
    requested_by: &str,
    result: &str,
    kms: &KmsEngine,
) -> Result<Result<ToolEffectCompletion, ToolEffectError>, PgError> {
    match agent_subject_status(connection, tenant, region, requested_by).await? {
        AgentSubjectStatus::Active => {}
        AgentSubjectStatus::Erased => return Ok(Err(ToolEffectError::Erased)),
        AgentSubjectStatus::Restricted => return Ok(Err(ToolEffectError::Restricted)),
    }
    let row = sqlx::query(
        "SELECT request_hash, requested_by, state, result_key_ref, result_nonce, \
                result_ciphertext FROM agent_tool_effect
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND effect_key = $4 FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(effect_key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(store_query)?;
    let Some(row) = row else {
        return Ok(Err(ToolEffectError::Missing));
    };
    if row
        .try_get::<String, _>("request_hash")
        .map_err(store_query)?
        != request_hash
        || row
            .try_get::<String, _>("requested_by")
            .map_err(store_query)?
            != requested_by
    {
        return Ok(Err(ToolEffectError::Conflict));
    }
    match row
        .try_get::<String, _>("state")
        .map_err(store_query)?
        .as_str()
    {
        "completed" => {
            let stored = tool_result_from_row(
                &row,
                kms,
                &JournalPayloadContext {
                    tenant,
                    region,
                    run_id,
                    position_key: effect_key,
                    request_hash,
                    requested_by,
                    kind: JournalPayloadKind::ToolResult,
                },
            )?;
            Ok(Ok(ToolEffectCompletion::Replayed(stored)))
        }
        "started" => {
            let sealed = seal_journal_payload(
                kms,
                &JournalPayloadContext {
                    tenant,
                    region,
                    run_id,
                    position_key: effect_key,
                    request_hash,
                    requested_by,
                    kind: JournalPayloadKind::ToolResult,
                },
                result.as_bytes(),
            )?;
            sqlx::query(
                "UPDATE agent_tool_effect
                    SET state = 'completed', result_text = NULL, result_key_ref = $5, \
                        result_nonce = $6, result_ciphertext = $7, completed_at = now()
                  WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND effect_key = $4",
            )
            .bind(tenant)
            .bind(region)
            .bind(run_id)
            .bind(effect_key)
            .bind(sealed.key_ref.to_uri())
            .bind(sealed.nonce.as_slice())
            .bind(sealed.ciphertext)
            .execute(connection)
            .await
            .map_err(store_query)?;
            Ok(Ok(ToolEffectCompletion::Applied))
        }
        "redacted" => Ok(Err(ToolEffectError::Unreplayable)),
        _ => Err(PgError::Query("tool effect has an invalid state".into())),
    }
}

fn tool_result_from_row(
    row: &sqlx::postgres::PgRow,
    kms: &KmsEngine,
    context: &JournalPayloadContext<'_>,
) -> Result<String, PgError> {
    let key_ref = row
        .try_get::<Option<String>, _>("result_key_ref")
        .map_err(store_query)?
        .ok_or_else(|| PgError::Query("completed tool effect has no encryption key".into()))?;
    let nonce = row
        .try_get::<Option<Vec<u8>>, _>("result_nonce")
        .map_err(store_query)?
        .ok_or_else(|| PgError::Query("completed tool effect has no encryption nonce".into()))?;
    let ciphertext = row
        .try_get::<Option<Vec<u8>>, _>("result_ciphertext")
        .map_err(store_query)?
        .ok_or_else(|| PgError::Query("completed tool effect has no ciphertext".into()))?;
    String::from_utf8(open_journal_payload(
        kms, context, &key_ref, &nonce, ciphertext,
    )?)
    .map_err(|_| PgError::Query("decrypted tool result is not UTF-8".into()))
}

fn store_query(error: sqlx::Error) -> PgError {
    PgError::Query(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_encodes_a_retryable_intent_and_immutable_result() {
        assert!(AGENT_TOOL_EFFECT_MIGRATION
            .contains("PRIMARY KEY (tenant_id, region, run_id, effect_key)"));
        assert!(AGENT_TOOL_EFFECT_MIGRATION.contains("octet_length(result_text) <= 262144"));
        assert!(AGENT_TOOL_EFFECT_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(AGENT_TOOL_EFFECT_GUARD_MIGRATION
            .contains("OLD.state <> 'started' OR NEW.state <> 'completed'"));
    }

    #[test]
    fn malformed_effect_identity_is_rejected_before_storage() {
        assert!(
            validate_identity("run", "model-turn/0/tool/0", &"a".repeat(64), "founder").is_ok()
        );
        assert!(validate_identity("", "effect", &"a".repeat(64), "founder").is_err());
        assert!(validate_identity("run", "", &"a".repeat(64), "founder").is_err());
        assert!(validate_identity("run", "effect", "not-a-hash", "founder").is_err());
        assert!(validate_identity("run", "effect", &"a".repeat(64), "").is_err());
    }
}
