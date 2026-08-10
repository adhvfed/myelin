use myelin_gdpr::ErasureMethod;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use crate::encryption::{ColumnCryptor, EncryptedColumn, SubjectId};
use crate::kms::{KeyClass, KmsEngine, PiiKeyRef, NONCE_LEN};
use crate::migration::{Migration, Migrations};
use crate::pg::PgError;

pub const AGENT_JOURNAL_SUBJECT_MIGRATION: &str = r#"
ALTER TABLE agent_model_step ADD COLUMN requested_by text;
ALTER TABLE agent_tool_effect ADD COLUMN requested_by text;
DROP TRIGGER IF EXISTS agent_model_step_guard_update ON agent_model_step;
DROP TRIGGER IF EXISTS agent_tool_effect_guard_update ON agent_tool_effect;

UPDATE agent_model_step AS journal
   SET requested_by = trace.requested_by
  FROM knowledge_agent_trace AS trace
 WHERE journal.requested_by IS NULL
   AND trace.tenant_id = journal.tenant_id
   AND trace.region = journal.region
   AND trace.run_id = journal.run_id;
UPDATE agent_tool_effect AS journal
   SET requested_by = trace.requested_by
  FROM knowledge_agent_trace AS trace
 WHERE journal.requested_by IS NULL
   AND trace.tenant_id = journal.tenant_id
   AND trace.region = journal.region
   AND trace.run_id = journal.run_id;

UPDATE agent_model_step AS journal
   SET requested_by = binding.owner_principal_id
  FROM agent_trigger_firing AS firing
  JOIN agent_trigger_binding AS binding
    ON binding.tenant_id = firing.tenant_id
   AND binding.region = firing.region
   AND binding.binding_id = firing.binding_id
 WHERE journal.requested_by IS NULL
   AND firing.tenant_id = journal.tenant_id
   AND firing.region = journal.region
   AND firing.run_id::text = journal.run_id;
UPDATE agent_tool_effect AS journal
   SET requested_by = binding.owner_principal_id
  FROM agent_trigger_firing AS firing
  JOIN agent_trigger_binding AS binding
    ON binding.tenant_id = firing.tenant_id
   AND binding.region = firing.region
   AND binding.binding_id = firing.binding_id
 WHERE journal.requested_by IS NULL
   AND firing.tenant_id = journal.tenant_id
   AND firing.region = journal.region
   AND firing.run_id::text = journal.run_id;

DO $myelin$
BEGIN
  IF EXISTS (SELECT 1 FROM agent_model_step WHERE requested_by IS NULL) THEN
    RAISE EXCEPTION 'cannot attribute every legacy agent_model_step to its requesting subject'
      USING HINT = 'Restore its run trace or trigger ownership before retrying this migration.';
  END IF;
  IF EXISTS (SELECT 1 FROM agent_tool_effect WHERE requested_by IS NULL) THEN
    RAISE EXCEPTION 'cannot attribute every legacy agent_tool_effect to its requesting subject'
      USING HINT = 'Restore its run trace or trigger ownership before retrying this migration.';
  END IF;
END
$myelin$;

ALTER TABLE agent_model_step
    ALTER COLUMN requested_by SET NOT NULL,
    ADD CONSTRAINT agent_model_step_requested_by_bound
      CHECK (length(requested_by) BETWEEN 1 AND 255);
ALTER TABLE agent_tool_effect
    ALTER COLUMN requested_by SET NOT NULL,
    ADD CONSTRAINT agent_tool_effect_requested_by_bound
      CHECK (length(requested_by) BETWEEN 1 AND 255);

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
     NEW.requested_by IS DISTINCT FROM OLD.requested_by OR
     NEW.started_at IS DISTINCT FROM OLD.started_at THEN
    RAISE EXCEPTION 'agent_model_step permits only its one-way completion transition';
  END IF;
  RETURN NEW;
END
$myelin$;

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
     NEW.requested_by IS DISTINCT FROM OLD.requested_by OR
     NEW.started_at IS DISTINCT FROM OLD.started_at THEN
    RAISE EXCEPTION 'agent_tool_effect permits only its one-way completion transition';
  END IF;
  RETURN NEW;
END
$myelin$;

CREATE TRIGGER agent_model_step_guard_update
BEFORE UPDATE ON agent_model_step
FOR EACH ROW EXECUTE FUNCTION myelin_guard_agent_model_step_update();
CREATE TRIGGER agent_tool_effect_guard_update
BEFORE UPDATE ON agent_tool_effect
FOR EACH ROW EXECUTE FUNCTION myelin_guard_agent_tool_effect_update();
"#;

pub const AGENT_JOURNAL_ENCRYPTION_MIGRATION: &str = r#"
DROP TRIGGER IF EXISTS agent_model_step_guard_update ON agent_model_step;
DROP TRIGGER IF EXISTS agent_tool_effect_guard_update ON agent_tool_effect;

ALTER TABLE agent_model_step
    ADD COLUMN response_key_ref text,
    ADD COLUMN response_nonce bytea,
    ADD COLUMN response_ciphertext bytea,
    DROP CONSTRAINT agent_model_step_check,
    DROP CONSTRAINT agent_model_step_state_check;
ALTER TABLE agent_tool_effect
    ADD COLUMN result_key_ref text,
    ADD COLUMN result_nonce bytea,
    ADD COLUMN result_ciphertext bytea,
    DROP CONSTRAINT agent_tool_effect_check,
    DROP CONSTRAINT agent_tool_effect_state_check;

UPDATE agent_model_step
   SET state = 'redacted', response = NULL
 WHERE state = 'completed';
UPDATE agent_tool_effect
   SET state = 'redacted', result_text = NULL
 WHERE state = 'completed';

ALTER TABLE agent_model_step
    ADD CONSTRAINT agent_model_step_encrypted_state
      CHECK (state IN ('started','completed','redacted')),
    ADD CONSTRAINT agent_model_step_encrypted_payload_shape CHECK (
      response IS NULL AND (
        (state = 'started' AND completed_at IS NULL
          AND response_key_ref IS NULL AND response_nonce IS NULL
          AND response_ciphertext IS NULL)
        OR
        (state = 'completed' AND completed_at IS NOT NULL
          AND response_key_ref IS NOT NULL
          AND response_nonce IS NOT NULL
          AND response_ciphertext IS NOT NULL
          AND length(response_key_ref) BETWEEN 1 AND 1024
          AND octet_length(response_nonce) = 12
          AND octet_length(response_ciphertext) BETWEEN 17 AND 4194320)
        OR
        (state = 'redacted' AND completed_at IS NOT NULL
          AND response_key_ref IS NULL AND response_nonce IS NULL
          AND response_ciphertext IS NULL)
      )
    );
ALTER TABLE agent_tool_effect
    ADD CONSTRAINT agent_tool_effect_encrypted_state
      CHECK (state IN ('started','completed','redacted')),
    ADD CONSTRAINT agent_tool_effect_encrypted_payload_shape CHECK (
      result_text IS NULL AND (
        (state = 'started' AND completed_at IS NULL
          AND result_key_ref IS NULL AND result_nonce IS NULL
          AND result_ciphertext IS NULL)
        OR
        (state = 'completed' AND completed_at IS NOT NULL
          AND result_key_ref IS NOT NULL
          AND result_nonce IS NOT NULL
          AND result_ciphertext IS NOT NULL
          AND length(result_key_ref) BETWEEN 1 AND 1024
          AND octet_length(result_nonce) = 12
          AND octet_length(result_ciphertext) BETWEEN 16 AND 262160)
        OR
        (state = 'redacted' AND completed_at IS NOT NULL
          AND result_key_ref IS NULL AND result_nonce IS NULL
          AND result_ciphertext IS NULL)
      )
    );

CREATE TRIGGER agent_model_step_guard_update
BEFORE UPDATE ON agent_model_step
FOR EACH ROW EXECUTE FUNCTION myelin_guard_agent_model_step_update();
CREATE TRIGGER agent_tool_effect_guard_update
BEFORE UPDATE ON agent_tool_effect
FOR EACH ROW EXECUTE FUNCTION myelin_guard_agent_tool_effect_update();
"#;

pub fn agent_journal_privacy_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "0105_agent_journal_subject",
            AGENT_JOURNAL_SUBJECT_MIGRATION,
        ),
        Migration::plain(
            "0106_agent_journal_encryption",
            AGENT_JOURNAL_ENCRYPTION_MIGRATION,
        ),
    ])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentSubjectStatus {
    Active,
    Erased,
    Restricted,
}

#[derive(Clone, Copy)]
pub(crate) enum JournalPayloadKind {
    ModelResponse,
    ToolResult,
}

impl JournalPayloadKind {
    fn token(self) -> &'static str {
        match self {
            Self::ModelResponse => "model-response",
            Self::ToolResult => "tool-result",
        }
    }
}

pub(crate) struct JournalPayloadContext<'a> {
    pub tenant: &'a str,
    pub region: &'a str,
    pub run_id: &'a str,
    pub position_key: &'a str,
    pub request_hash: &'a str,
    pub requested_by: &'a str,
    pub kind: JournalPayloadKind,
}

pub(crate) fn seal_journal_payload(
    kms: &KmsEngine,
    context: &JournalPayloadContext<'_>,
    plaintext: &[u8],
) -> Result<EncryptedColumn, PgError> {
    ColumnCryptor::new(kms, Region(context.region.to_string()))
        .encrypt_with_aad(
            &TenantId(context.tenant.to_string()),
            Some(&SubjectId::new(context.requested_by)),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            plaintext,
            &journal_payload_aad(context),
        )
        .map_err(|error| PgError::Query(format!("agent journal encryption failed: {error}")))
}

pub(crate) fn open_journal_payload(
    kms: &KmsEngine,
    context: &JournalPayloadContext<'_>,
    key_ref: &str,
    nonce: &[u8],
    ciphertext: Vec<u8>,
) -> Result<Vec<u8>, PgError> {
    let key_ref = PiiKeyRef::parse(key_ref).ok_or_else(|| {
        PgError::Query("agent journal has an invalid subject key reference".into())
    })?;
    if key_ref.tenant.as_str() != context.tenant
        || key_ref.class != KeyClass::Subject(context.requested_by.to_string())
    {
        return Err(PgError::Query(
            "agent journal key reference does not match its immutable subject".into(),
        ));
    }
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| PgError::Query("agent journal has an invalid encryption nonce".into()))?;
    ColumnCryptor::new(kms, Region(context.region.to_string()))
        .decrypt_with_aad(
            &EncryptedColumn {
                key_ref,
                nonce,
                ciphertext,
            },
            &journal_payload_aad(context),
        )
        .map_err(|error| PgError::Query(format!("agent journal decryption failed: {error}")))
}

fn journal_payload_aad(context: &JournalPayloadContext<'_>) -> Vec<u8> {
    let mut aad = Vec::new();
    for field in [
        "myelin.agent-journal.payload.v1",
        context.kind.token(),
        context.tenant,
        context.region,
        context.run_id,
        context.position_key,
        context.request_hash,
        context.requested_by,
    ] {
        aad.extend_from_slice(&(field.len() as u64).to_be_bytes());
        aad.extend_from_slice(field.as_bytes());
    }
    aad
}

pub(crate) async fn lock_agent_subject(
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
        .map_err(privacy_query)?;
    Ok(())
}

pub(crate) async fn agent_subject_status(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    requested_by: &str,
) -> Result<AgentSubjectStatus, PgError> {
    let token = agent_subject_token(tenant, region, requested_by);
    lock_agent_subject(connection, tenant, region, &token).await?;
    let row = sqlx::query(
        "SELECT \
           EXISTS (SELECT 1 FROM knowledge_agent_trace_subject_erasure \
                    WHERE tenant_id = $1 AND region = $2 AND subject_token = $3) AS erased, \
           EXISTS (SELECT 1 FROM knowledge_agent_trace_subject_restriction \
                    WHERE tenant_id = $1 AND region = $2 AND subject_token = $3) AS restricted",
    )
    .bind(tenant)
    .bind(region)
    .bind(token)
    .fetch_one(connection)
    .await
    .map_err(privacy_query)?;
    if row.try_get::<bool, _>("erased").map_err(privacy_query)? {
        Ok(AgentSubjectStatus::Erased)
    } else if row
        .try_get::<bool, _>("restricted")
        .map_err(privacy_query)?
    {
        Ok(AgentSubjectStatus::Restricted)
    } else {
        Ok(AgentSubjectStatus::Active)
    }
}

pub(crate) fn agent_subject_token(tenant: &str, region: &str, requested_by: &str) -> String {
    let body =
        format!("myelin.agent_trace.subject.v1\u{1f}{tenant}\u{1f}{region}\u{1f}{requested_by}");
    blake3::hash(body.as_bytes()).to_hex().to_string()
}

fn privacy_query(error: sqlx::Error) -> PgError {
    PgError::Query(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_makes_subject_ownership_complete_and_immutable() {
        assert!(AGENT_JOURNAL_SUBJECT_MIGRATION.contains("trace.requested_by"));
        assert!(AGENT_JOURNAL_SUBJECT_MIGRATION.contains("binding.owner_principal_id"));
        assert!(AGENT_JOURNAL_SUBJECT_MIGRATION
            .contains("DROP TRIGGER IF EXISTS agent_model_step_guard_update"));
        assert!(AGENT_JOURNAL_SUBJECT_MIGRATION.contains("ALTER COLUMN requested_by SET NOT NULL"));
        assert_eq!(
            AGENT_JOURNAL_SUBJECT_MIGRATION
                .matches("NEW.requested_by IS DISTINCT FROM OLD.requested_by")
                .count(),
            2,
        );
    }

    #[test]
    fn subject_locator_is_stable_and_tenant_scoped() {
        let first = agent_subject_token("acme", "eu", "founder");
        assert_eq!(first, agent_subject_token("acme", "eu", "founder"));
        assert_ne!(first, agent_subject_token("other", "eu", "founder"));
        assert_ne!(first, agent_subject_token("acme", "eu", "someone-else"));
    }

    #[test]
    fn journal_aad_is_injective_across_record_identity() {
        let context = JournalPayloadContext {
            tenant: "acme",
            region: "eu",
            run_id: "run-1",
            position_key: "model-turn/0",
            request_hash: "hash",
            requested_by: "founder",
            kind: JournalPayloadKind::ModelResponse,
        };
        let first = journal_payload_aad(&context);
        let moved = journal_payload_aad(&JournalPayloadContext {
            run_id: "run-2",
            ..context
        });
        assert_ne!(first, moved);
    }

    #[test]
    fn plaintext_legacy_payloads_become_unreplayable_instead_of_surviving_the_upgrade() {
        assert!(
            AGENT_JOURNAL_ENCRYPTION_MIGRATION.contains("SET state = 'redacted', response = NULL")
        );
        assert!(AGENT_JOURNAL_ENCRYPTION_MIGRATION
            .contains("SET state = 'redacted', result_text = NULL"));
        assert!(AGENT_JOURNAL_ENCRYPTION_MIGRATION.contains("response IS NULL"));
        assert!(AGENT_JOURNAL_ENCRYPTION_MIGRATION.contains("result_text IS NULL"));
        assert!(AGENT_JOURNAL_ENCRYPTION_MIGRATION.contains("response_ciphertext"));
        assert!(AGENT_JOURNAL_ENCRYPTION_MIGRATION.contains("result_ciphertext"));
        assert!(AGENT_JOURNAL_ENCRYPTION_MIGRATION.contains("response_key_ref IS NOT NULL"));
        assert!(AGENT_JOURNAL_ENCRYPTION_MIGRATION.contains("result_key_ref IS NOT NULL"));
    }
}
