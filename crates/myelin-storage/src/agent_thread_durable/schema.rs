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

pub const AGENT_THREAD_GIT_BINDING_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  ADD COLUMN IF NOT EXISTS repository_slug text,
  ADD COLUMN IF NOT EXISTS base_ref text,
  ADD COLUMN IF NOT EXISTS base_oid text,
  ADD COLUMN IF NOT EXISTS workspace_branch text;
ALTER TABLE agent_thread
  DROP CONSTRAINT IF EXISTS agent_thread_git_binding_complete;
ALTER TABLE agent_thread
  ADD CONSTRAINT agent_thread_git_binding_complete CHECK (
    (repository_slug IS NULL AND base_ref IS NULL AND base_oid IS NULL
      AND workspace_branch IS NULL)
    OR (octet_length(repository_slug) BETWEEN 1 AND 255
      AND octet_length(base_ref) BETWEEN 12 AND 1024
      AND base_oid ~ '^[0-9a-f]{40}$'
      AND octet_length(workspace_branch) BETWEEN 1 AND 1024)
  ) NOT VALID;
"#;

pub const AGENT_THREAD_GIT_BINDING_INVARIANT_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  VALIDATE CONSTRAINT agent_thread_git_binding_complete;
"#;

pub const AGENT_THREAD_GIT_BINDING_BACKFILL_MIGRATION: &str = r#"
UPDATE agent_thread
   SET repository_slug = NULL,
       base_ref = NULL,
       base_oid = NULL,
       workspace_branch = NULL
 WHERE num_nonnulls(repository_slug, base_ref, base_oid, workspace_branch) NOT IN (0, 4);
"#;

pub const AGENT_THREAD_SSH_CONNECTION_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread_ssh_grant
  ADD COLUMN IF NOT EXISTS connection_id text;
ALTER TABLE agent_thread_ssh_grant
  DROP CONSTRAINT IF EXISTS agent_thread_ssh_grant_connection_id_shape;
ALTER TABLE agent_thread_ssh_grant
  ADD CONSTRAINT agent_thread_ssh_grant_connection_id_shape CHECK (
    connection_id IS NULL
    OR (num_nonnulls(consumed_at, connection_id) = 2 AND length(connection_id) = 26)
  ) NOT VALID;
"#;

pub const AGENT_THREAD_SSH_CONNECTION_BACKFILL_MIGRATION: &str = r#"
UPDATE agent_thread_ssh_grant access
   SET connection_id = prior.connection_id
  FROM (
    SELECT tenant_id, region, grant_id, min(session_id) AS connection_id
      FROM agent_thread_workspace_session
     GROUP BY tenant_id, region, grant_id
  ) prior
 WHERE access.tenant_id = prior.tenant_id
   AND access.region = prior.region
   AND access.grant_id = prior.grant_id
   AND access.consumed_at IS NOT NULL
   AND access.connection_id IS NULL;
"#;

pub const AGENT_THREAD_SSH_CONNECTION_INVARIANT_MIGRATION: &str = r#"
ALTER TABLE agent_thread_ssh_grant
  DROP CONSTRAINT IF EXISTS agent_thread_ssh_grant_connection_id_shape;
ALTER TABLE agent_thread_ssh_grant
  DROP CONSTRAINT IF EXISTS agent_thread_ssh_grant_connection_claim_complete;
ALTER TABLE agent_thread_ssh_grant
  ADD CONSTRAINT agent_thread_ssh_grant_connection_claim_complete CHECK (
    num_nonnulls(consumed_at, connection_id) = 0
    OR (num_nonnulls(consumed_at, connection_id) = 2 AND length(connection_id) = 26)
  ) NOT VALID;
ALTER TABLE agent_thread_ssh_grant
  VALIDATE CONSTRAINT agent_thread_ssh_grant_connection_claim_complete;
"#;

pub const AGENT_THREAD_FILE_TRANSFER_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread_workspace_session
  DROP CONSTRAINT IF EXISTS agent_thread_workspace_session_mode;
ALTER TABLE agent_thread_workspace_session
  ADD CONSTRAINT agent_thread_workspace_session_mode
  CHECK (session_mode IN ('shell', 'command', 'file_transfer')) NOT VALID;
"#;

pub const AGENT_THREAD_FILE_TRANSFER_BACKFILL_MIGRATION: &str = r#"
ALTER TABLE agent_thread_workspace_session
  VALIDATE CONSTRAINT agent_thread_workspace_session_mode;
"#;

pub const AGENT_THREAD_FILE_TRANSFER_CONTRACT_MIGRATION: &str = r#"
ALTER TABLE agent_thread_workspace_session
  DROP CONSTRAINT IF EXISTS agent_thread_workspace_session_session_mode_check;
"#;

pub const AGENT_THREAD_ENVIRONMENT_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  ADD COLUMN IF NOT EXISTS environment_provider text,
  ADD COLUMN IF NOT EXISTS environment_config_path text,
  ADD COLUMN IF NOT EXISTS environment_config_digest text;
ALTER TABLE agent_thread
  DROP CONSTRAINT IF EXISTS agent_thread_environment_complete;
ALTER TABLE agent_thread
  ADD CONSTRAINT agent_thread_environment_complete CHECK (
    num_nonnulls(environment_provider, environment_config_path, environment_config_digest) = 0
    OR (num_nonnulls(repository_slug) = 1
      AND environment_provider = 'service_federation'
      AND environment_config_path = 'fed.yaml'
      AND environment_config_digest ~ '^blake3:[0-9a-f]{64}$')
  ) NOT VALID;
"#;

pub const AGENT_THREAD_ENVIRONMENT_BACKFILL_MIGRATION: &str = r#"
UPDATE agent_thread
   SET environment_provider = NULL,
       environment_config_path = NULL,
       environment_config_digest = NULL
 WHERE num_nonnulls(environment_provider, environment_config_path, environment_config_digest)
       NOT IN (0, 3);
"#;

pub const AGENT_THREAD_ENVIRONMENT_CONTRACT_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  VALIDATE CONSTRAINT agent_thread_environment_complete;
"#;

pub const AGENT_THREAD_REVIEW_READY_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  ADD COLUMN IF NOT EXISTS review_ready_at timestamptz;
"#;

pub const AGENT_THREAD_REVIEW_READY_SHAPE_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  DROP CONSTRAINT IF EXISTS agent_thread_review_ready_shape;
ALTER TABLE agent_thread
  ADD CONSTRAINT agent_thread_review_ready_shape CHECK (
    review_ready_at IS NULL
    OR (project_id IS NOT NULL
      AND publication_completed_at IS NOT NULL
      AND state IN ('ready', 'expiring', 'deleted')
      AND review_ready_at >= publication_completed_at)
  ) NOT VALID;
"#;

pub const AGENT_THREAD_REVIEW_READY_INVARIANT_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  VALIDATE CONSTRAINT agent_thread_review_ready_shape;
"#;

pub const AGENT_THREAD_WORKSPACE_GUIDANCE_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  ADD COLUMN IF NOT EXISTS workspace_guidance_digest text,
  ADD COLUMN IF NOT EXISTS workspace_guidance_key_ref text,
  ADD COLUMN IF NOT EXISTS workspace_guidance_nonce bytea,
  ADD COLUMN IF NOT EXISTS workspace_guidance_ciphertext bytea;
ALTER TABLE agent_thread
  DROP CONSTRAINT IF EXISTS agent_thread_workspace_guidance_complete;
ALTER TABLE agent_thread
  ADD CONSTRAINT agent_thread_workspace_guidance_complete CHECK (
    num_nonnulls(workspace_guidance_digest, workspace_guidance_key_ref,
      workspace_guidance_nonce, workspace_guidance_ciphertext) = 0
    OR (num_nonnulls(workspace_guidance_digest, workspace_guidance_key_ref,
          workspace_guidance_nonce, workspace_guidance_ciphertext) = 4
      AND workspace_guidance_digest ~ '^blake3:[0-9a-f]{64}$'
      AND octet_length(workspace_guidance_key_ref) BETWEEN 1 AND 1024
      AND octet_length(workspace_guidance_nonce) = 12
      AND octet_length(workspace_guidance_ciphertext) BETWEEN 17 AND 12304)
  ) NOT VALID;
"#;

pub const AGENT_THREAD_WORKSPACE_GUIDANCE_INVARIANT_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  VALIDATE CONSTRAINT agent_thread_workspace_guidance_complete;
"#;

pub const AGENT_THREAD_WORKSPACE_GUIDANCE_BACKFILL_MIGRATION: &str = r#"
UPDATE agent_thread
   SET workspace_guidance_digest = NULL,
       workspace_guidance_key_ref = NULL,
       workspace_guidance_nonce = NULL,
       workspace_guidance_ciphertext = NULL
 WHERE num_nonnulls(workspace_guidance_digest, workspace_guidance_key_ref,
       workspace_guidance_nonce, workspace_guidance_ciphertext) NOT IN (0, 4);
"#;

pub const AGENT_THREAD_WORKSPACE_GUIDANCE_SOURCE_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  ADD COLUMN IF NOT EXISTS workspace_guidance_request_digest text;
ALTER TABLE agent_thread
  DROP CONSTRAINT IF EXISTS agent_thread_workspace_guidance_request_shape;
ALTER TABLE agent_thread
  ADD CONSTRAINT agent_thread_workspace_guidance_request_shape CHECK (
    workspace_guidance_request_digest IS NULL
    OR (workspace_guidance_digest IS NOT NULL
      AND workspace_guidance_request_digest ~ '^blake3:[0-9a-f]{64}$')
  ) NOT VALID;
"#;

pub const AGENT_THREAD_WORKSPACE_GUIDANCE_SOURCE_BACKFILL_MIGRATION: &str = r#"
UPDATE agent_thread
   SET workspace_guidance_request_digest = workspace_guidance_digest
 WHERE workspace_guidance_digest IS NOT NULL
   AND workspace_guidance_request_digest IS NULL;
"#;

pub const AGENT_THREAD_WORKSPACE_GUIDANCE_SOURCE_INVARIANT_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  VALIDATE CONSTRAINT agent_thread_workspace_guidance_request_shape;
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

pub const AGENT_THREAD_PUBLICATION_EXPAND_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  ADD COLUMN IF NOT EXISTS publication_completed_at timestamptz;
"#;

pub const AGENT_THREAD_PUBLICATION_BACKFILL_MIGRATION: &str = r#"
DO $migration$
BEGIN
  IF to_regclass('public.chat_conversation') IS NOT NULL THEN
    UPDATE agent_thread thread
       SET publication_completed_at = thread.updated_at
      FROM chat_conversation conversation
     WHERE thread.tenant_id = conversation.tenant_id
       AND thread.region = conversation.region
       AND thread.conversation_id = conversation.conversation_id
       AND thread.state = 'ready'
       AND thread.publication_completed_at IS NULL
       AND conversation.acl_zookie IS NOT NULL;
  END IF;
END
$migration$;
"#;

pub const AGENT_THREAD_PUBLICATION_RECOVERY_INDEX: &str = "agent_thread_provisioning_recovery";

pub const AGENT_THREAD_PUBLICATION_RECOVERY_INDEX_MIGRATION: &str = r#"
CREATE INDEX CONCURRENTLY IF NOT EXISTS agent_thread_provisioning_recovery
  ON agent_thread (tenant_id, region, updated_at, thread_id)
  WHERE state IN ('provisioning', 'failed')
     OR (state = 'ready' AND publication_completed_at IS NULL);
"#;

pub const AGENT_THREAD_RENEWAL_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS agent_thread_renewal (
    tenant_id            text        NOT NULL,
    region               text        NOT NULL,
    renewal_id           uuid        NOT NULL,
    thread_id            uuid        NOT NULL,
    owner_principal_id   text        NOT NULL,
    agent_id             uuid        NOT NULL,
    conversation_id      text        NOT NULL,
    project_id           uuid,
    review_ready         boolean     NOT NULL,
    retention_days       smallint    NOT NULL,
    client_nonce         text        NOT NULL,
    previous_expires_at  timestamptz NOT NULL,
    expires_at           timestamptz NOT NULL,
    completed_at         timestamptz,
    created_at           timestamptz NOT NULL,
    updated_at           timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, renewal_id),
    UNIQUE (tenant_id, region, owner_principal_id, client_nonce),
    FOREIGN KEY (tenant_id, region, thread_id)
      REFERENCES agent_thread (tenant_id, region, thread_id),
    CHECK (length(owner_principal_id) BETWEEN 1 AND 255),
    CHECK (length(conversation_id) = 26),
    CHECK (retention_days BETWEEN 1 AND 30),
    CHECK (length(client_nonce) BETWEEN 1 AND 128),
    CHECK (expires_at > previous_expires_at),
    CHECK (updated_at >= created_at),
    CHECK (completed_at IS NULL OR completed_at >= created_at)
);
CREATE INDEX IF NOT EXISTS agent_thread_renewal_pending
  ON agent_thread_renewal (tenant_id, region, updated_at, renewal_id)
  WHERE completed_at IS NULL;
ALTER TABLE agent_thread_renewal ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_thread_renewal FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_thread_renewal;
CREATE POLICY myelin_tenant_isolation ON agent_thread_renewal
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const AGENT_THREAD_PUBLICATION_INVARIANT_MIGRATION: &str = r#"
ALTER TABLE agent_thread
  DROP CONSTRAINT IF EXISTS agent_thread_publication_after_creation;
ALTER TABLE agent_thread
  ADD CONSTRAINT agent_thread_publication_after_creation
  CHECK (publication_completed_at IS NULL
    OR (state IN ('ready', 'expiring', 'deleted')
      AND publication_completed_at >= created_at)) NOT VALID;
ALTER TABLE agent_thread
  VALIDATE CONSTRAINT agent_thread_publication_after_creation;
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

pub fn agent_thread_publication_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0147_agent_thread_publication_expand",
            AGENT_THREAD_PUBLICATION_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread",
        ),
        Migration::phased(
            "0148_agent_thread_publication_backfill",
            AGENT_THREAD_PUBLICATION_BACKFILL_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread",
        ),
        Migration::phased(
            "0149_agent_thread_publication_invariant",
            AGENT_THREAD_PUBLICATION_INVARIANT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread",
        ),
        Migration::phased(
            "0150_agent_thread_publication_recovery_index",
            AGENT_THREAD_PUBLICATION_RECOVERY_INDEX_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread",
        ),
    ])
}

pub fn agent_thread_renewal_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0175_agent_thread_renewal",
        AGENT_THREAD_RENEWAL_MIGRATION,
    )])
}

pub fn agent_thread_git_binding_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0151_agent_thread_git_binding_expand",
            AGENT_THREAD_GIT_BINDING_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread",
        ),
        Migration::phased(
            "0152_agent_thread_git_binding_backfill",
            AGENT_THREAD_GIT_BINDING_BACKFILL_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread",
        ),
        Migration::phased(
            "0153_agent_thread_git_binding_invariant",
            AGENT_THREAD_GIT_BINDING_INVARIANT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread",
        ),
    ])
}

pub fn agent_thread_ssh_connection_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0154_agent_thread_ssh_connection_expand",
            AGENT_THREAD_SSH_CONNECTION_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread_ssh_grant",
        ),
        Migration::phased(
            "0155_agent_thread_ssh_connection_backfill",
            AGENT_THREAD_SSH_CONNECTION_BACKFILL_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread_ssh_grant",
        ),
        Migration::phased(
            "0156_agent_thread_ssh_connection_invariant",
            AGENT_THREAD_SSH_CONNECTION_INVARIANT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread_ssh_grant",
        ),
    ])
}

pub fn agent_thread_file_transfer_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0157_agent_thread_file_transfer_expand",
            AGENT_THREAD_FILE_TRANSFER_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread_workspace_session",
        ),
        Migration::phased(
            "0158_agent_thread_file_transfer_backfill",
            AGENT_THREAD_FILE_TRANSFER_BACKFILL_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread_workspace_session",
        ),
        Migration::phased(
            "0159_agent_thread_file_transfer_contract",
            AGENT_THREAD_FILE_TRANSFER_CONTRACT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread_workspace_session",
        ),
    ])
}

pub fn agent_thread_environment_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0160_agent_thread_environment_expand",
            AGENT_THREAD_ENVIRONMENT_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread",
        ),
        Migration::phased(
            "0161_agent_thread_environment_backfill",
            AGENT_THREAD_ENVIRONMENT_BACKFILL_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread",
        ),
        Migration::phased(
            "0162_agent_thread_environment_contract",
            AGENT_THREAD_ENVIRONMENT_CONTRACT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread",
        ),
    ])
}

pub fn agent_thread_review_ready_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0163_agent_thread_review_ready_expand",
            AGENT_THREAD_REVIEW_READY_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread",
        ),
        Migration::phased(
            "0164_agent_thread_review_ready_shape",
            AGENT_THREAD_REVIEW_READY_SHAPE_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread",
        ),
        Migration::phased(
            "0165_agent_thread_review_ready_invariant",
            AGENT_THREAD_REVIEW_READY_INVARIANT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread",
        ),
    ])
}

pub fn agent_thread_workspace_guidance_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0166_agent_thread_workspace_guidance_expand",
            AGENT_THREAD_WORKSPACE_GUIDANCE_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread",
        ),
        Migration::phased(
            "0167_agent_thread_workspace_guidance_backfill",
            AGENT_THREAD_WORKSPACE_GUIDANCE_BACKFILL_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread",
        ),
        Migration::phased(
            "0168_agent_thread_workspace_guidance_invariant",
            AGENT_THREAD_WORKSPACE_GUIDANCE_INVARIANT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread",
        ),
    ])
}

pub fn agent_thread_workspace_guidance_source_migrations() -> Migrations {
    Migrations::of([
        Migration::phased(
            "0172_agent_thread_workspace_guidance_source_expand",
            AGENT_THREAD_WORKSPACE_GUIDANCE_SOURCE_EXPAND_MIGRATION,
            crate::MigrationPhase::Expand,
            "agent_thread",
        ),
        Migration::phased(
            "0173_agent_thread_workspace_guidance_source_backfill",
            AGENT_THREAD_WORKSPACE_GUIDANCE_SOURCE_BACKFILL_MIGRATION,
            crate::MigrationPhase::Backfill,
            "agent_thread",
        ),
        Migration::phased(
            "0174_agent_thread_workspace_guidance_source_invariant",
            AGENT_THREAD_WORKSPACE_GUIDANCE_SOURCE_INVARIANT_MIGRATION,
            crate::MigrationPhase::Contract,
            "agent_thread",
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HotTables, MigrationPhase, OnlineMigrationRunner};

    #[test]
    fn ssh_connection_claim_is_an_online_expand_backfill_contract_cycle() {
        let migrations = agent_thread_ssh_connection_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &HotTables::declare(["agent_thread_ssh_grant"]))
            .expect("the live SSH grant migration is online-safe");
        assert_eq!(migrations.0.len(), 3);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
    }

    #[test]
    fn file_transfer_mode_is_an_online_expand_contract_cycle() {
        let migrations = agent_thread_file_transfer_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(
                &migrations,
                &HotTables::declare(["agent_thread_workspace_session"]),
            )
            .expect("the live workspace history migration is online-safe");
        assert_eq!(migrations.0.len(), 3);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
    }

    #[test]
    fn workspace_guidance_is_an_online_encrypted_optional_receipt() {
        let migrations = agent_thread_workspace_guidance_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &HotTables::declare(["agent_thread"]))
            .expect("the live workspace-guidance migration is online-safe");
        assert_eq!(migrations.0.len(), 3);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
        assert!(AGENT_THREAD_WORKSPACE_GUIDANCE_EXPAND_MIGRATION.contains("bytea"));
        assert!(AGENT_THREAD_WORKSPACE_GUIDANCE_EXPAND_MIGRATION.contains("NOT VALID"));
    }

    #[test]
    fn workspace_guidance_source_is_an_online_retry_stability_receipt() {
        let migrations = agent_thread_workspace_guidance_source_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &HotTables::declare(["agent_thread"]))
            .expect("the live workspace-guidance source migration is online-safe");

        assert_eq!(migrations.0.len(), 3);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
        assert!(AGENT_THREAD_WORKSPACE_GUIDANCE_SOURCE_EXPAND_MIGRATION.contains("NOT VALID"));
        assert!(AGENT_THREAD_WORKSPACE_GUIDANCE_SOURCE_BACKFILL_MIGRATION
            .contains("workspace_guidance_request_digest = workspace_guidance_digest"));
    }

    #[test]
    fn environment_receipt_is_a_forward_only_online_schema_change() {
        let migrations = agent_thread_environment_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &HotTables::declare(["agent_thread"]))
            .expect("the live agent thread migration is online-safe");
        assert_eq!(migrations.0.len(), 3);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
    }

    #[test]
    fn review_ready_is_a_forward_only_online_schema_change() {
        let migrations = agent_thread_review_ready_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &HotTables::declare(["agent_thread"]))
            .expect("the live review-ready migration is online-safe");
        assert_eq!(migrations.0.len(), 3);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
        assert!(AGENT_THREAD_REVIEW_READY_SHAPE_MIGRATION.contains("NOT VALID"));
    }

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

    #[test]
    fn thread_publication_is_backfilled_only_from_visible_chat_and_indexed_online() {
        let migrations = agent_thread_publication_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &HotTables::declare(["agent_thread"]))
            .expect("the live agent thread publication migration is online-safe");
        assert_eq!(migrations.0.len(), 4);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
        assert_eq!(migrations.0[3].phase, MigrationPhase::Expand);
        assert!(AGENT_THREAD_PUBLICATION_BACKFILL_MIGRATION.contains("to_regclass"));
        assert!(AGENT_THREAD_PUBLICATION_BACKFILL_MIGRATION.contains("acl_zookie IS NOT NULL"));
        assert!(AGENT_THREAD_PUBLICATION_RECOVERY_INDEX_MIGRATION.contains("CONCURRENTLY"));
        assert!(AGENT_THREAD_PUBLICATION_RECOVERY_INDEX_MIGRATION
            .contains(AGENT_THREAD_PUBLICATION_RECOVERY_INDEX));
    }

    #[test]
    fn git_binding_is_a_forward_only_online_schema_change() {
        let migrations = agent_thread_git_binding_migrations();
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &HotTables::declare(["agent_thread"]))
            .expect("the live agent thread Git binding migration is online-safe");
        assert_eq!(migrations.0.len(), 3);
        assert_eq!(migrations.0[0].phase, MigrationPhase::Expand);
        assert_eq!(migrations.0[1].phase, MigrationPhase::Backfill);
        assert_eq!(migrations.0[2].phase, MigrationPhase::Contract);
        assert!(AGENT_THREAD_GIT_BINDING_EXPAND_MIGRATION.contains("NOT VALID"));
        assert!(AGENT_THREAD_GIT_BINDING_BACKFILL_MIGRATION.contains("num_nonnulls"));
        assert!(AGENT_THREAD_GIT_BINDING_INVARIANT_MIGRATION.contains("VALIDATE CONSTRAINT"));
    }
}
