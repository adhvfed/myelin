use sqlx::Row;

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

pub fn agent_journal_privacy_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0105_agent_journal_subject",
        AGENT_JOURNAL_SUBJECT_MIGRATION,
    )])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentSubjectStatus {
    Active,
    Erased,
    Restricted,
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
}
