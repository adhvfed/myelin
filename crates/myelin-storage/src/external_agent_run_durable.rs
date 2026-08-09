use sqlx::types::chrono::{DateTime, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};

pub const EXTERNAL_AGENT_RUN_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS external_agent_run (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    run_id          uuid        NOT NULL,
    agent_id        uuid        NOT NULL,
    trigger_actor_id text       NOT NULL,
    trigger_credential_jti text NOT NULL,
    trigger_authority text[]    NOT NULL,
    client_nonce    text        NOT NULL,
    token_jti       text        NOT NULL,
    state           text        NOT NULL CHECK (state IN ('provisioning', 'ready', 'closed', 'terminal')),
    issued_at       timestamptz NOT NULL,
    expires_at      timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, run_id),
    UNIQUE (tenant_id, region, trigger_actor_id, client_nonce),
    FOREIGN KEY (tenant_id, region, agent_id)
        REFERENCES identity_agent (tenant_id, region, agent_id),
    CHECK (length(trigger_actor_id) BETWEEN 1 AND 255),
    CHECK (length(trigger_credential_jti) BETWEEN 1 AND 512),
    CHECK (cardinality(trigger_authority) BETWEEN 1 AND 512),
    CHECK (length(client_nonce) BETWEEN 1 AND 128),
    CHECK (length(token_jti) BETWEEN 1 AND 512),
    CHECK (expires_at > issued_at)
);
CREATE INDEX IF NOT EXISTS external_agent_run_agent_recent
    ON external_agent_run (tenant_id, region, agent_id, run_id DESC);
"#;

pub const EXTERNAL_AGENT_RUN_RLS_POLICY: &str = r#"
ALTER TABLE external_agent_run ENABLE ROW LEVEL SECURITY;
ALTER TABLE external_agent_run FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON external_agent_run;
CREATE POLICY myelin_tenant_isolation ON external_agent_run
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub fn external_agent_run_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0086_external_agent_run", EXTERNAL_AGENT_RUN_MIGRATION),
        Migration::plain("0087_external_agent_run_rls", EXTERNAL_AGENT_RUN_RLS_POLICY),
    ])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalAgentRunState {
    Provisioning,
    Ready,
    Closed,
    Terminal,
}

impl ExternalAgentRunState {
    pub fn token(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Ready => "ready",
            Self::Closed => "closed",
            Self::Terminal => "terminal",
        }
    }

    fn parse(value: &str) -> Result<Self, PgError> {
        match value {
            "provisioning" => Ok(Self::Provisioning),
            "ready" => Ok(Self::Ready),
            "closed" => Ok(Self::Closed),
            "terminal" => Ok(Self::Terminal),
            _ => Err(PgError::Query(
                "external agent run has an invalid durable state".into(),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableExternalAgentRun {
    pub run_id: String,
    pub agent_id: String,
    pub trigger_actor_id: String,
    pub trigger_credential_jti: String,
    pub trigger_authority: Vec<String>,
    pub client_nonce: String,
    pub token_jti: String,
    pub state: ExternalAgentRunState,
    pub issued_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedExternalAgentRun {
    pub run: DurableExternalAgentRun,
    pub created: bool,
}

#[derive(Clone)]
pub struct DurableExternalAgentRunBacking {
    provider: SubstrateProvider,
}

impl DurableExternalAgentRunBacking {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self { provider }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn claim(
        &self,
        tenant: &str,
        agent_id: Uuid,
        trigger_actor_id: &str,
        trigger_credential_jti: &str,
        trigger_authority: &[String],
        client_nonce: &str,
        proposed_run_id: Uuid,
        token_jti: &str,
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<ClaimedExternalAgentRun, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let trigger_actor_id = trigger_actor_id.to_string();
        let trigger_credential_jti = trigger_credential_jti.to_string();
        let trigger_authority = trigger_authority.to_vec();
        let client_nonce = client_nonce.to_string();
        let token_jti = token_jti.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let inserted = sqlx::query(
                        "INSERT INTO external_agent_run (\
                           tenant_id, region, run_id, agent_id, trigger_actor_id, \
                           trigger_credential_jti, trigger_authority, client_nonce, token_jti, \
                           state, issued_at, expires_at\
                         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'provisioning', $10, $11) \
                         ON CONFLICT (tenant_id, region, trigger_actor_id, client_nonce) \
                         DO NOTHING",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(proposed_run_id)
                    .bind(agent_id)
                    .bind(&trigger_actor_id)
                    .bind(&trigger_credential_jti)
                    .bind(&trigger_authority)
                    .bind(&client_nonce)
                    .bind(&token_jti)
                    .bind(issued_at)
                    .bind(expires_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("claim external agent run"))?
                    .rows_affected()
                        == 1;
                    let row = sqlx::query(
                        "SELECT run_id, agent_id, trigger_actor_id, trigger_credential_jti, \
                                trigger_authority, client_nonce, token_jti, state, issued_at, expires_at \
                           FROM external_agent_run \
                          WHERE tenant_id = $1 AND region = $2 \
                            AND trigger_actor_id = $3 AND client_nonce = $4",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&trigger_actor_id)
                    .bind(&client_nonce)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(query_error("load claimed external agent run"))?;
                    Ok(ClaimedExternalAgentRun {
                        run: run_from_row(&row)?,
                        created: inserted,
                    })
                })
            })
            .await
    }

    pub async fn mark_ready(
        &self,
        tenant: &str,
        run_id: Uuid,
    ) -> Result<DurableExternalAgentRun, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "UPDATE external_agent_run SET state = 'ready' \
                          WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                            AND state IN ('provisioning', 'ready') \
                         RETURNING run_id, agent_id, trigger_actor_id, trigger_credential_jti, \
                                   trigger_authority, client_nonce, token_jti, state, issued_at, \
                                   expires_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(run_id)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(query_error("mark external agent run ready"))?;
                    run_from_row(&row)
                })
            })
            .await
    }
}

fn run_from_row(row: &sqlx::postgres::PgRow) -> Result<DurableExternalAgentRun, PgError> {
    let issued_at = row
        .try_get::<DateTime<Utc>, _>("issued_at")
        .map_err(row_error("issued_at"))?;
    let expires_at = row
        .try_get::<DateTime<Utc>, _>("expires_at")
        .map_err(row_error("expires_at"))?;
    Ok(DurableExternalAgentRun {
        run_id: row
            .try_get::<Uuid, _>("run_id")
            .map_err(row_error("run_id"))?
            .to_string(),
        agent_id: row
            .try_get::<Uuid, _>("agent_id")
            .map_err(row_error("agent_id"))?
            .to_string(),
        trigger_actor_id: row
            .try_get("trigger_actor_id")
            .map_err(row_error("trigger_actor_id"))?,
        trigger_credential_jti: row
            .try_get("trigger_credential_jti")
            .map_err(row_error("trigger_credential_jti"))?,
        trigger_authority: row
            .try_get("trigger_authority")
            .map_err(row_error("trigger_authority"))?,
        client_nonce: row
            .try_get("client_nonce")
            .map_err(row_error("client_nonce"))?,
        token_jti: row.try_get("token_jti").map_err(row_error("token_jti"))?,
        state: ExternalAgentRunState::parse(
            &row.try_get::<String, _>("state")
                .map_err(row_error("state"))?,
        )?,
        issued_at: issued_at.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    })
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn row_error(column: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode external agent run `{column}`: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_enforces_tenant_scope_idempotency_and_expiry() {
        assert!(EXTERNAL_AGENT_RUN_MIGRATION
            .contains("UNIQUE (tenant_id, region, trigger_actor_id, client_nonce)"));
        assert!(EXTERNAL_AGENT_RUN_MIGRATION.contains("FOREIGN KEY (tenant_id, region, agent_id)"));
        assert!(EXTERNAL_AGENT_RUN_MIGRATION.contains("CHECK (expires_at > issued_at)"));
        assert!(EXTERNAL_AGENT_RUN_MIGRATION.contains("trigger_authority text[]"));
        assert!(EXTERNAL_AGENT_RUN_RLS_POLICY.contains("FORCE ROW LEVEL SECURITY"));
    }

    #[test]
    fn durable_states_have_one_canonical_token() {
        for (state, token) in [
            (ExternalAgentRunState::Provisioning, "provisioning"),
            (ExternalAgentRunState::Ready, "ready"),
            (ExternalAgentRunState::Closed, "closed"),
            (ExternalAgentRunState::Terminal, "terminal"),
        ] {
            assert_eq!(state.token(), token);
            assert_eq!(ExternalAgentRunState::parse(token).unwrap(), state);
        }
        assert!(ExternalAgentRunState::parse("running-ish").is_err());
    }
}
