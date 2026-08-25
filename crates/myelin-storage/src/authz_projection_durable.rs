use crate::migration::{Migration, Migrations};

pub const AUTHZ_PROJECTION_STATE_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS authz_projection_state (
    tenant_id       text NOT NULL,
    region          text NOT NULL,
    projection      text NOT NULL,
    source_revision bigint NOT NULL CHECK (source_revision > 0),
    applied_revision bigint NOT NULL DEFAULT 0 CHECK (applied_revision >= 0),
    status          text NOT NULL CHECK (status IN ('pending', 'rebuilding', 'ready')),
    rebuilt_at      timestamptz,
    PRIMARY KEY (tenant_id, region, projection),
    CHECK (applied_revision <= source_revision),
    CHECK (status <> 'ready' OR applied_revision = source_revision)
);"#;

const AUTHZ_PROJECTION_STATE_RLS: &str = r#"
ALTER TABLE authz_projection_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE authz_projection_state FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON authz_projection_state;
CREATE POLICY myelin_tenant_isolation ON authz_projection_state
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));"#;

pub const AUTHZ_PROJECTION_INVALIDATOR_MIGRATION: &str = r#"
CREATE OR REPLACE FUNCTION myelin_invalidate_issue_view_projection()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
DECLARE
    scoped_tenant text;
    scoped_region text;
BEGIN
    scoped_tenant := COALESCE(NEW.tenant_id, OLD.tenant_id);
    scoped_region := COALESCE(NEW.region, OLD.region);
    INSERT INTO authz_projection_state
        (tenant_id, region, projection, source_revision, applied_revision, status, rebuilt_at)
    VALUES (scoped_tenant, scoped_region, 'issue:view', 1, 0, 'pending', NULL)
    ON CONFLICT (tenant_id, region, projection) DO UPDATE
       SET source_revision = authz_projection_state.source_revision + 1,
           status = 'pending',
           rebuilt_at = NULL;
    RETURN COALESCE(NEW, OLD);
END;
$$;

CREATE TRIGGER rebac_tuple_invalidate_issue_view
AFTER INSERT OR UPDATE OR DELETE ON rebac_tuple
FOR EACH ROW EXECUTE FUNCTION myelin_invalidate_issue_view_projection();"#;

/// Once per-run grants shared `rebac_tuple`, the original broad invalidator made issue lists
/// unavailable after every `run:*` insert. The trigger function is shared by the Issues tables, so
/// it scopes only ReBAC changes: issue rows always invalidate, while unrelated tuple families do
/// not.
pub const AUTHZ_PROJECTION_SCOPED_INVALIDATOR_MIGRATION: &str = r#"
CREATE OR REPLACE FUNCTION myelin_invalidate_issue_view_projection()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
DECLARE
    scoped_tenant text;
    scoped_region text;
BEGIN
    IF TG_TABLE_NAME = 'rebac_tuple' THEN
        IF split_part(COALESCE(NEW.object_id, ''), ':', 1)
               NOT IN ('issue', 'project', 'team', 'org')
           AND split_part(COALESCE(OLD.object_id, ''), ':', 1)
               NOT IN ('issue', 'project', 'team', 'org') THEN
            RETURN COALESCE(NEW, OLD);
        END IF;
    END IF;

    scoped_tenant := COALESCE(NEW.tenant_id, OLD.tenant_id);
    scoped_region := COALESCE(NEW.region, OLD.region);
    INSERT INTO authz_projection_state
        (tenant_id, region, projection, source_revision, applied_revision, status, rebuilt_at)
    VALUES (scoped_tenant, scoped_region, 'issue:view', 1, 0, 'pending', NULL)
    ON CONFLICT (tenant_id, region, projection) DO UPDATE
       SET source_revision = authz_projection_state.source_revision + 1,
           status = 'pending',
           rebuilt_at = NULL;
    RETURN COALESCE(NEW, OLD);
END;
$$;
"#;

pub const AUTHZ_PROJECTION_FORMAT_MIGRATION: &str = r#"
ALTER TABLE authz_projection_state
  ADD COLUMN IF NOT EXISTS format_version integer NOT NULL DEFAULT 0
    CHECK (format_version >= 0);

CREATE OR REPLACE FUNCTION myelin_invalidate_issue_view_projection()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
DECLARE
    scoped_tenant text;
    scoped_region text;
BEGIN
    IF TG_TABLE_NAME = 'rebac_tuple' THEN
        IF split_part(COALESCE(NEW.object_id, ''), ':', 1)
               NOT IN ('issue', 'project', 'team', 'org')
           AND split_part(COALESCE(OLD.object_id, ''), ':', 1)
               NOT IN ('issue', 'project', 'team', 'org') THEN
            RETURN COALESCE(NEW, OLD);
        END IF;
    END IF;

    scoped_tenant := COALESCE(NEW.tenant_id, OLD.tenant_id);
    scoped_region := COALESCE(NEW.region, OLD.region);
    INSERT INTO authz_projection_state
        (tenant_id, region, projection, source_revision, applied_revision, status,
         rebuilt_at, format_version)
    VALUES (scoped_tenant, scoped_region, 'issue:view', 1, 0, 'pending', NULL, 0)
    ON CONFLICT (tenant_id, region, projection) DO UPDATE
       SET source_revision = authz_projection_state.source_revision + 1,
           status = 'pending',
           rebuilt_at = NULL,
           format_version = 0;
    RETURN COALESCE(NEW, OLD);
END;
$$;

UPDATE authz_projection_state
   SET source_revision = source_revision + 1,
       status = 'pending',
       rebuilt_at = NULL,
       format_version = 0
 WHERE projection = 'issue:view';
"#;

pub fn authz_projection_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "0067_authz_projection_state",
            AUTHZ_PROJECTION_STATE_MIGRATION,
        ),
        Migration::plain(
            "0068_authz_projection_state_rls",
            AUTHZ_PROJECTION_STATE_RLS,
        ),
        Migration::plain(
            "0069_authz_projection_invalidator",
            AUTHZ_PROJECTION_INVALIDATOR_MIGRATION,
        ),
        Migration::plain(
            "0069c_authz_projection_scoped_invalidator",
            AUTHZ_PROJECTION_SCOPED_INVALIDATOR_MIGRATION,
        ),
    ])
}

pub fn authz_projection_format_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0136_authz_projection_format",
        AUTHZ_PROJECTION_FORMAT_MIGRATION,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_issue_projection_invalidator_names_only_its_dependency_graph() {
        for relevant in ["issue", "project", "team", "org"] {
            assert!(AUTHZ_PROJECTION_SCOPED_INVALIDATOR_MIGRATION.contains(relevant));
        }
        assert!(
            AUTHZ_PROJECTION_SCOPED_INVALIDATOR_MIGRATION.contains("TG_TABLE_NAME = 'rebac_tuple'")
        );
        assert!(!AUTHZ_PROJECTION_SCOPED_INVALIDATOR_MIGRATION.contains("'run'"));
    }

    #[test]
    fn migration_ids_follow_delegation_range_and_rows_are_effective_permissions() {
        let ids: Vec<_> = authz_projection_durable_migrations()
            .0
            .iter()
            .map(|migration| migration.id.to_string())
            .collect();
        assert_eq!(
            ids.first().map(String::as_str),
            Some("0067_authz_projection_state")
        );
        assert_eq!(
            ids.last().map(String::as_str),
            Some("0069c_authz_projection_scoped_invalidator")
        );
    }
}
