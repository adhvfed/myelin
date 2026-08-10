use crate::migration::{Migration, Migrations};

pub const AGENT_TRIGGER_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS agent_trigger_binding (
    tenant_id               text        NOT NULL,
    region                  text        NOT NULL,
    binding_id              uuid        NOT NULL,
    owner_principal_id      text        NOT NULL,
    run_as_agent_id         uuid        NOT NULL,
    client_nonce            text        NOT NULL,
    event_type              text        NOT NULL,
    matcher                 jsonb       NOT NULL,
    task                    text        NOT NULL,
    delegation_caveats      text[]      NOT NULL,
    max_firings             bigint      NOT NULL,
    firings_used            bigint      NOT NULL DEFAULT 0,
    max_causal_depth        integer     NOT NULL,
    require_no_personal_data boolean    NOT NULL,
    require_human_approval  boolean     NOT NULL,
    state                   text        NOT NULL DEFAULT 'active',
    created_at              timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, binding_id),
    UNIQUE (tenant_id, region, owner_principal_id, client_nonce),
    FOREIGN KEY (tenant_id, region, run_as_agent_id)
        REFERENCES identity_agent (tenant_id, region, agent_id),
    CHECK (length(owner_principal_id) BETWEEN 1 AND 255),
    CHECK (length(client_nonce) BETWEEN 1 AND 128),
    CHECK (length(event_type) BETWEEN 1 AND 255),
    CHECK (jsonb_typeof(matcher) = 'object'),
    CHECK (length(task) BETWEEN 1 AND 4096),
    CHECK (cardinality(delegation_caveats) <= 128),
    CHECK (max_firings BETWEEN 1 AND 1000000),
    CHECK (firings_used BETWEEN 0 AND max_firings),
    CHECK (max_causal_depth BETWEEN 0 AND 64),
    CHECK (state IN ('active', 'paused', 'disabled'))
);
CREATE INDEX IF NOT EXISTS agent_trigger_binding_active_event
    ON agent_trigger_binding (tenant_id, region, event_type, binding_id)
    WHERE state = 'active';

CREATE TABLE IF NOT EXISTS agent_trigger_firing (
    tenant_id      text        NOT NULL,
    region         text        NOT NULL,
    binding_id     uuid        NOT NULL,
    event_id       text        NOT NULL,
    event_type     text        NOT NULL,
    event_envelope jsonb       NOT NULL,
    state          text        NOT NULL,
    run_id         uuid,
    created_at     timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, binding_id, event_id),
    FOREIGN KEY (tenant_id, region, binding_id)
        REFERENCES agent_trigger_binding (tenant_id, region, binding_id),
    CHECK (length(event_id) BETWEEN 1 AND 255),
    CHECK (length(event_type) BETWEEN 1 AND 255),
    CHECK (jsonb_typeof(event_envelope) = 'object'),
    CHECK (state IN ('queued', 'awaiting_approval', 'claimed', 'started', 'terminal')),
    CHECK ((state IN ('started', 'terminal')) OR run_id IS NULL)
);
CREATE INDEX IF NOT EXISTS agent_trigger_firing_queue
    ON agent_trigger_firing (tenant_id, region, created_at, binding_id, event_id)
    WHERE state = 'queued';
"#;

pub const AGENT_TRIGGER_RLS_POLICY: &str = r#"
ALTER TABLE agent_trigger_binding ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_trigger_binding FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_trigger_binding;
CREATE POLICY myelin_tenant_isolation ON agent_trigger_binding
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));

ALTER TABLE agent_trigger_firing ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_trigger_firing FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_trigger_firing;
CREATE POLICY myelin_tenant_isolation ON agent_trigger_firing
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));
"#;

pub fn agent_trigger_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0090_agent_trigger", AGENT_TRIGGER_MIGRATION),
        Migration::plain("0091_agent_trigger_rls", AGENT_TRIGGER_RLS_POLICY),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governance_and_effectively_once_firing_are_structural() {
        assert!(AGENT_TRIGGER_MIGRATION.contains("owner_principal_id"));
        assert!(AGENT_TRIGGER_MIGRATION.contains("run_as_agent_id"));
        assert!(AGENT_TRIGGER_MIGRATION.contains("matcher                 jsonb"));
        assert!(AGENT_TRIGGER_MIGRATION
            .contains("PRIMARY KEY (tenant_id, region, binding_id, event_id)"));
        assert!(AGENT_TRIGGER_MIGRATION.contains("firings_used BETWEEN 0 AND max_firings"));
        assert_eq!(
            AGENT_TRIGGER_RLS_POLICY
                .matches("FORCE ROW LEVEL SECURITY")
                .count(),
            2
        );
    }
}
