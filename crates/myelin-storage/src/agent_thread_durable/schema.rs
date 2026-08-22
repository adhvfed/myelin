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

pub fn agent_thread_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0124_agent_thread", AGENT_THREAD_MIGRATION),
        Migration::plain("0125_agent_thread_rls", AGENT_THREAD_RLS_POLICY),
        Migration::plain("0126_agent_thread_run", AGENT_THREAD_RUN_MIGRATION),
    ])
}
