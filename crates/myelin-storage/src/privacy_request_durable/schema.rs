use crate::{Migration, Migrations};

pub const PRIVACY_REQUEST_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS privacy_request (
    tenant_id           text        NOT NULL,
    region              text        NOT NULL,
    request_id          uuid        NOT NULL,
    owner_principal_id  text        NOT NULL,
    client_nonce        text        NOT NULL,
    kind                text        NOT NULL CHECK (kind = 'erasure'),
    scope               text        NOT NULL CHECK (scope = 'agent_data'),
    state               text        NOT NULL DEFAULT 'pending'
      CHECK (state IN ('pending', 'processing', 'completed')),
    attempt_count       integer     NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    lease_owner         text,
    lease_epoch         bigint      NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    lease_expires       timestamptz,
    failure_reason      text,
    certificate         jsonb,
    submitted_at        timestamptz NOT NULL,
    deadline_at         timestamptz NOT NULL,
    completed_at        timestamptz,
    PRIMARY KEY (tenant_id, region, request_id),
    UNIQUE (tenant_id, region, owner_principal_id, client_nonce),
    FOREIGN KEY (tenant_id, region, owner_principal_id)
      REFERENCES principal (tenant_id, region, principal_id),
    CHECK (length(owner_principal_id) BETWEEN 1 AND 255),
    CHECK (length(client_nonce) BETWEEN 1 AND 128),
    CHECK (lease_owner IS NULL OR length(lease_owner) BETWEEN 1 AND 255),
    CHECK (failure_reason IS NULL OR octet_length(failure_reason) BETWEEN 1 AND 1024),
    CHECK (deadline_at > submitted_at),
    CHECK (
      (state = 'pending' AND lease_owner IS NULL AND lease_expires IS NULL
                         AND certificate IS NULL AND completed_at IS NULL)
      OR
      (state = 'processing' AND lease_owner IS NOT NULL AND lease_expires IS NOT NULL
                            AND certificate IS NULL AND completed_at IS NULL)
      OR
      (state = 'completed' AND lease_owner IS NULL AND lease_expires IS NULL
                           AND failure_reason IS NULL AND certificate IS NOT NULL
                           AND completed_at IS NOT NULL)
    )
);
CREATE INDEX IF NOT EXISTS privacy_request_pending
  ON privacy_request (tenant_id, region, submitted_at, request_id)
  WHERE state = 'pending' OR state = 'processing';
CREATE INDEX IF NOT EXISTS privacy_request_owner_recent
  ON privacy_request (tenant_id, region, owner_principal_id, submitted_at DESC, request_id DESC);
ALTER TABLE privacy_request ENABLE ROW LEVEL SECURITY;
ALTER TABLE privacy_request FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON privacy_request;
CREATE POLICY myelin_tenant_isolation ON privacy_request
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub const PRIVACY_REQUEST_CHAT_MESSAGES_SCOPE_MIGRATION: &str = r#"
ALTER TABLE privacy_request
  DROP CONSTRAINT privacy_request_scope_check;
ALTER TABLE privacy_request
  ADD CONSTRAINT privacy_request_scope_check
  CHECK (scope IN ('agent_data', 'chat_messages'));
"#;

pub fn privacy_request_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0131_privacy_request",
        PRIVACY_REQUEST_MIGRATION,
    )])
}

pub fn privacy_request_chat_messages_scope_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0135_privacy_request_chat_messages_scope",
        PRIVACY_REQUEST_CHAT_MESSAGES_SCOPE_MIGRATION,
    )])
}

pub const PRIVACY_REQUEST_ISSUE_TITLES_SCOPE_MIGRATION: &str = r#"
ALTER TABLE privacy_request
  DROP CONSTRAINT privacy_request_scope_check;
ALTER TABLE privacy_request
  ADD CONSTRAINT privacy_request_scope_check
  CHECK (scope IN ('agent_data', 'chat_messages', 'issue_titles'));
"#;

pub fn privacy_request_issue_titles_scope_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0138_privacy_request_issue_titles_scope",
        PRIVACY_REQUEST_ISSUE_TITLES_SCOPE_MIGRATION,
    )])
}

pub const PRIVACY_REQUEST_GIT_PULL_REQUEST_TEXT_SCOPE_MIGRATION: &str = r#"
ALTER TABLE privacy_request
  DROP CONSTRAINT privacy_request_scope_check;
ALTER TABLE privacy_request
  ADD CONSTRAINT privacy_request_scope_check
  CHECK (scope IN ('agent_data', 'chat_messages', 'issue_titles', 'git_pull_request_text'));
"#;

pub fn privacy_request_git_pull_request_text_scope_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0140_privacy_request_git_pull_request_text_scope",
        PRIVACY_REQUEST_GIT_PULL_REQUEST_TEXT_SCOPE_MIGRATION,
    )])
}
