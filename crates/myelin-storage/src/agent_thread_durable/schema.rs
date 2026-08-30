use crate::{Migration, Migrations};

pub const AGENT_THREAD_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS agent_thread (
    tenant_id            text        NOT NULL,
    region               text        NOT NULL,
    thread_id            uuid        NOT NULL,
    owner_principal_id   text        NOT NULL,
    agent_id             uuid        NOT NULL,
    conversation_id      text        NOT NULL,
    workspace_id         uuid        NOT NULL,
    workspace_generation integer     NOT NULL DEFAULT 1,
    name                 text        NOT NULL,
    project_id           uuid,
    retention_days       smallint    NOT NULL,
    client_nonce         text        NOT NULL,
    state                text        NOT NULL DEFAULT 'provisioning'
      CHECK (state IN ('provisioning', 'ready', 'expiring', 'deleted', 'failed')),
    storage_locator      text,
    failure_reason       text,
    created_at           timestamptz NOT NULL,
    expires_at           timestamptz NOT NULL,
    updated_at           timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, thread_id),
    UNIQUE (tenant_id, region, owner_principal_id, client_nonce),
    UNIQUE (tenant_id, region, conversation_id),
    UNIQUE (tenant_id, region, workspace_id),
    FOREIGN KEY (tenant_id, region, agent_id)
      REFERENCES identity_agent (tenant_id, region, agent_id),
    CHECK (length(owner_principal_id) BETWEEN 1 AND 255),
    CHECK (length(conversation_id) = 26),
    CHECK (octet_length(name) BETWEEN 1 AND 80),
    CHECK (retention_days BETWEEN 1 AND 30),
    CHECK (length(client_nonce) BETWEEN 1 AND 128),
    CHECK (workspace_generation > 0),
    CHECK (expires_at > created_at),
    CHECK ((state = 'ready' AND storage_locator IS NOT NULL AND failure_reason IS NULL)
        OR (state <> 'ready')),
    CHECK (storage_locator IS NULL OR octet_length(storage_locator) BETWEEN 1 AND 1024),
    CHECK (failure_reason IS NULL OR octet_length(failure_reason) BETWEEN 1 AND 512)
);
CREATE UNIQUE INDEX IF NOT EXISTS agent_thread_owner_live_name
  ON agent_thread (tenant_id, region, owner_principal_id, lower(name))
  WHERE state IN ('provisioning', 'ready', 'expiring', 'failed');
CREATE INDEX IF NOT EXISTS agent_thread_owner_recent
  ON agent_thread (tenant_id, region, owner_principal_id, thread_id DESC);
CREATE INDEX IF NOT EXISTS agent_thread_expiry
  ON agent_thread (tenant_id, region, expires_at, thread_id)
  WHERE state IN ('provisioning', 'ready', 'failed');
"#;

pub const AGENT_THREAD_RLS_POLICY: &str = r#"
ALTER TABLE agent_thread ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_thread FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_thread;
CREATE POLICY myelin_tenant_isolation ON agent_thread
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const AGENT_THREAD_RUN_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS agent_thread_run (
    tenant_id            text        NOT NULL,
    region               text        NOT NULL,
    run_id               uuid        NOT NULL,
    thread_id            uuid        NOT NULL,
    conversation_id      text        NOT NULL,
    workspace_id         uuid        NOT NULL,
    workspace_generation integer     NOT NULL,
    bound_at             timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, run_id),
    FOREIGN KEY (tenant_id, region, run_id)
      REFERENCES external_agent_run (tenant_id, region, run_id),
    FOREIGN KEY (tenant_id, region, thread_id)
      REFERENCES agent_thread (tenant_id, region, thread_id),
    CHECK (length(conversation_id) = 26),
    CHECK (workspace_generation > 0)
);
CREATE INDEX IF NOT EXISTS agent_thread_run_thread_recent
  ON agent_thread_run (tenant_id, region, thread_id, run_id DESC);
ALTER TABLE agent_thread_run ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_thread_run FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_thread_run;
CREATE POLICY myelin_tenant_isolation ON agent_thread_run
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const AGENT_THREAD_SSH_GRANT_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS agent_thread_ssh_grant (
    tenant_id             text        NOT NULL,
    region                text        NOT NULL,
    grant_id              uuid        NOT NULL,
    route_username        text        NOT NULL,
    thread_id             uuid        NOT NULL,
    owner_principal_id    text        NOT NULL,
    workspace_id          uuid        NOT NULL,
    workspace_generation  integer     NOT NULL,
    public_key_fingerprint text       NOT NULL,
    client_nonce          text        NOT NULL,
    issued_at             timestamptz NOT NULL,
    expires_at            timestamptz NOT NULL,
    revoked_at            timestamptz,
    PRIMARY KEY (tenant_id, region, grant_id),
    UNIQUE (tenant_id, region, owner_principal_id, client_nonce),
    UNIQUE (route_username),
    FOREIGN KEY (tenant_id, region, thread_id)
      REFERENCES agent_thread (tenant_id, region, thread_id),
    CHECK (length(route_username) BETWEEN 16 AND 384),
    CHECK (length(owner_principal_id) BETWEEN 1 AND 255),
    CHECK (workspace_generation > 0),
    CHECK (length(public_key_fingerprint) = 50),
    CHECK (length(client_nonce) BETWEEN 1 AND 128),
    CHECK (expires_at > issued_at),
    CHECK (expires_at <= issued_at + interval '5 minutes'),
    CHECK (revoked_at IS NULL OR revoked_at >= issued_at)
);
CREATE INDEX IF NOT EXISTS agent_thread_ssh_grant_expiry
  ON agent_thread_ssh_grant (tenant_id, region, expires_at, grant_id)
  WHERE revoked_at IS NULL;
ALTER TABLE agent_thread_ssh_grant ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_thread_ssh_grant FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_thread_ssh_grant;
CREATE POLICY myelin_tenant_isolation ON agent_thread_ssh_grant
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const AGENT_THREAD_WORKSPACE_SESSION_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS agent_thread_workspace_session (
    tenant_id            text        NOT NULL,
    region               text        NOT NULL,
    session_id           text        NOT NULL,
    grant_id             uuid        NOT NULL,
    thread_id            uuid        NOT NULL,
    owner_principal_id   text        NOT NULL,
    workspace_id         uuid        NOT NULL,
    workspace_generation integer     NOT NULL,
    access_method        text        NOT NULL CHECK (access_method = 'ssh'),
    session_mode         text        NOT NULL CHECK (session_mode IN ('shell', 'command')),
    terminal             boolean     NOT NULL,
    started_at           timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, session_id),
    FOREIGN KEY (tenant_id, region, thread_id)
      REFERENCES agent_thread (tenant_id, region, thread_id),
    CHECK (length(session_id) = 26),
    CHECK (length(owner_principal_id) BETWEEN 1 AND 255),
    CHECK (workspace_generation > 0)
);
CREATE INDEX IF NOT EXISTS agent_thread_workspace_session_recent
  ON agent_thread_workspace_session
    (tenant_id, region, owner_principal_id, thread_id, session_id DESC);
ALTER TABLE agent_thread_workspace_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_thread_workspace_session FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_thread_workspace_session;
CREATE POLICY myelin_tenant_isolation ON agent_thread_workspace_session
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const AGENT_THREAD_SSH_SINGLE_USE_MIGRATION: &str = r#"
ALTER TABLE agent_thread_ssh_grant
  ADD COLUMN IF NOT EXISTS consumed_at timestamptz;
UPDATE agent_thread_ssh_grant access
   SET consumed_at = prior.first_started_at
  FROM (
    SELECT tenant_id, region, grant_id, min(started_at) AS first_started_at
      FROM agent_thread_workspace_session
     GROUP BY tenant_id, region, grant_id
  ) prior
 WHERE access.tenant_id = prior.tenant_id
   AND access.region = prior.region
   AND access.grant_id = prior.grant_id
   AND access.consumed_at IS NULL;
ALTER TABLE agent_thread_ssh_grant
  DROP CONSTRAINT IF EXISTS agent_thread_ssh_grant_consumed_after_issue;
ALTER TABLE agent_thread_ssh_grant
  ADD CONSTRAINT agent_thread_ssh_grant_consumed_after_issue
  CHECK (consumed_at IS NULL OR consumed_at >= issued_at) NOT VALID;
ALTER TABLE agent_thread_ssh_grant
  VALIDATE CONSTRAINT agent_thread_ssh_grant_consumed_after_issue;
"#;

pub const AGENT_THREAD_RECENT_LIST_INDEX_MIGRATION: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS agent_thread_owner_created_recent
  ON agent_thread (
    tenant_id, region, owner_principal_id,
    created_at DESC, conversation_id DESC, thread_id DESC
  )
  WHERE state <> 'deleted';
"#;

pub const AGENT_THREAD_SESSION_LAUNCH_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread_workspace_session
  ADD COLUMN IF NOT EXISTS launched_at timestamptz;
"#;

pub const AGENT_THREAD_SESSION_LAUNCH_BACKFILL_MIGRATION: &str = r#"
UPDATE agent_thread_workspace_session
   SET launched_at = started_at
 WHERE launched_at IS NULL;
"#;

pub const AGENT_THREAD_SESSION_LAUNCH_INVARIANT_MIGRATION: &str = r#"
ALTER TABLE agent_thread_workspace_session
  DROP CONSTRAINT IF EXISTS agent_thread_workspace_session_launch_after_reservation;
ALTER TABLE agent_thread_workspace_session
  ADD CONSTRAINT agent_thread_workspace_session_launch_after_reservation
  CHECK (launched_at IS NULL OR launched_at >= started_at) NOT VALID;
ALTER TABLE agent_thread_workspace_session
  VALIDATE CONSTRAINT agent_thread_workspace_session_launch_after_reservation;
"#;

pub const AGENT_THREAD_SESSION_LAUNCH_INDEX_MIGRATION: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS agent_thread_workspace_session_launched_recent
  ON agent_thread_workspace_session
    (tenant_id, region, owner_principal_id, thread_id, launched_at DESC, session_id DESC)
  WHERE launched_at IS NOT NULL;
"#;

pub fn agent_thread_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0124_agent_thread", AGENT_THREAD_MIGRATION),
        Migration::plain("0125_agent_thread_rls", AGENT_THREAD_RLS_POLICY),
        Migration::plain("0126_agent_thread_run", AGENT_THREAD_RUN_MIGRATION),
        Migration::plain(
            "0127_agent_thread_ssh_grant",
            AGENT_THREAD_SSH_GRANT_MIGRATION,
        ),
        Migration::plain(
            "0130_agent_thread_workspace_session",
            AGENT_THREAD_WORKSPACE_SESSION_MIGRATION,
        ),
    ])
}

pub fn agent_thread_ssh_single_use_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0141_agent_thread_ssh_single_use",
        AGENT_THREAD_SSH_SINGLE_USE_MIGRATION,
    )])
}

pub fn agent_thread_recent_list_migrations() -> Migrations {
    Migrations::of([Migration::phased(
        "0142_agent_thread_recent_list_index",
        AGENT_THREAD_RECENT_LIST_INDEX_MIGRATION,
        crate::MigrationPhase::Expand,
        "agent_thread",
    )])
}

pub fn agent_thread_session_launch_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0143_agent_thread_session_launch_expand",
            AGENT_THREAD_SESSION_LAUNCH_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread_workspace_session",
        ),
        Migration::phased(
            "0144_agent_thread_session_launch_backfill",
            AGENT_THREAD_SESSION_LAUNCH_BACKFILL_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread_workspace_session",
        ),
        Migration::phased(
            "0145_agent_thread_session_launch_invariant",
            AGENT_THREAD_SESSION_LAUNCH_INVARIANT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread_workspace_session",
        ),
        Migration::phased(
            "0146_agent_thread_session_launch_index",
            AGENT_THREAD_SESSION_LAUNCH_INDEX_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread_workspace_session",
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HotTables, MigrationPhase, OnlineMigrationRunner};

    #[test]
    fn session_publication_uses_an_online_expand_backfill_contract_cycle() {
        let migrations = agent_thread_session_launch_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(
                &migrations,
                &HotTables::declare(["agent_thread_workspace_session"]),
            )
            .expect("the live workspace history migration is online-safe");
        assert_eq!(migrations.0.len(), 4);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
        assert_eq!(migrations.0[3].phase, MigrationPhase::Expand);
        assert!(AGENT_THREAD_SESSION_LAUNCH_INDEX_MIGRATION.contains("CONCURRENTLY"));
    }
}
