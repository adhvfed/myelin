use myelin_events::{CONSUMER_DEDUP_MIGRATION, OUTBOX_MIGRATION};
use myelin_substrate::{HotTables, Migration, MigrationPhase, Migrations};

pub const ISSUE_TABLE: &str = "issue";
pub const ISSUE_RELATION_TABLE: &str = "issue_relation";
pub const ISSUE_CHANGE_LOG_TABLE: &str = "issue_change_log";
pub const SCHEME_TABLE: &str = "scheme";
pub const SCHEME_ASSIGNMENT_TABLE: &str = "scheme_assignment";
pub const CYCLE_TABLE: &str = "cycle";
pub const CYCLE_MEMBERSHIP_TABLE: &str = "cycle_membership";
pub const MILESTONE_TABLE: &str = "milestone";
pub const PREFIX_COUNTER_TABLE: &str = "prefix_counter";
pub const CONSUMER_DEDUP_TABLE: &str = "consumer_dedup";
pub const OUTBOX_TABLE: &str = "outbox";
pub const ISSUE_AUTHZ_BINDING_TABLE: &str = "issue_authz_binding";
pub const ISSUE_AUTHZ_VISIBLE_TABLE: &str = "issue_authz_visible";
pub const ISSUE_VIEW_SUBJECT_TABLE: &str = "issue_view_subject";
pub const IMPORT_MAP_TABLE: &str = "import_map";
pub const ISSUE_CREATE_IDEMPOTENCY_TABLE: &str = "issue_create_idempotency";

pub const CREATE_ISSUE_AUTHZ_VISIBLE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS issue_authz_visible (
  tenant_id   text NOT NULL,
  region      text NOT NULL,
  projection  text NOT NULL CHECK (projection = 'issue:view'),
  subject     text NOT NULL,
  permission  text NOT NULL CHECK (permission = 'view'),
  object_type text NOT NULL CHECK (object_type = 'issue'),
  object_id   text NOT NULL,
  revision    bigint NOT NULL CHECK (revision > 0),
  PRIMARY KEY (tenant_id, region, projection, subject, permission, object_type, object_id),
  FOREIGN KEY (tenant_id, region, projection)
    REFERENCES authz_projection_state (tenant_id, region, projection)
);
CREATE INDEX IF NOT EXISTS issue_authz_visible_lookup
  ON issue_authz_visible
    (tenant_id, region, subject, permission, object_type, revision, object_id);"#;

pub const CREATE_ISSUE_VIEW_SUBJECT_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS issue_view_subject (
  tenant_id   text NOT NULL,
  region      text NOT NULL,
  projection  text NOT NULL CHECK (projection = 'issue:view'),
  subject     text NOT NULL,
  scope_kind  text NOT NULL CHECK (scope_kind IN ('project', 'confidential', 'confidential_grant')),
  scope_id    uuid NOT NULL,
  revision    bigint NOT NULL CHECK (revision > 0),
  PRIMARY KEY (tenant_id, region, projection, scope_kind, scope_id, subject),
  FOREIGN KEY (tenant_id, region, projection)
    REFERENCES authz_projection_state (tenant_id, region, projection)
);
CREATE INDEX IF NOT EXISTS issue_view_subject_lookup
  ON issue_view_subject
    (tenant_id, region, subject, revision, scope_kind, scope_id);"#;

const INVALIDATE_LEGACY_ISSUE_VIEW_DDL: &str = r#"
UPDATE authz_projection_state
   SET source_revision = source_revision + 1, status = 'pending', rebuilt_at = NULL
 WHERE projection = 'issue:view';"#;

pub const CREATE_ISSUE_AUTHZ_INVALIDATION_TRIGGERS_DDL: &str = r#"
CREATE TRIGGER issue_invalidate_issue_view
  AFTER INSERT OR UPDATE OR DELETE ON issue
  FOR EACH ROW EXECUTE FUNCTION myelin_invalidate_issue_view_projection();

CREATE TRIGGER issue_authz_binding_invalidate_issue_view
  AFTER INSERT OR UPDATE OR DELETE ON issue_authz_binding
  FOR EACH ROW EXECUTE FUNCTION myelin_invalidate_issue_view_projection();"#;

pub const NARROW_ISSUE_AUTHZ_INVALIDATION_TRIGGER_DDL: &str = r#"
CREATE OR REPLACE TRIGGER issue_invalidate_issue_view
  AFTER INSERT OR DELETE OR UPDATE OF project_id ON issue
  FOR EACH ROW EXECUTE FUNCTION myelin_invalidate_issue_view_projection();"#;

pub const ISSUE_BOARD_INDEX: &str = "issue_board";
pub const ISSUE_ROADMAP_INDEX: &str = "issue_roadmap";
pub const ISSUE_ASSIGNEE_INDEX: &str = "issue_assignee";
pub const ISSUE_PARENT_INDEX: &str = "issue_parent";
pub const ISSUE_CYCLE_INDEX: &str = "issue_cycle";
pub const ISSUE_PROPS_GIN_INDEX: &str = "issue_props_gin";
pub const ISSUE_RECENT_LIST_INDEX: &str = "issue_recent_list_idx";
pub const ISSUE_KEY_PREFIX_LIST_INDEX: &str = "issue_key_prefix_list_idx";
pub const CREATE_ISSUE_RECENT_LIST_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_recent_list_idx \
     ON issue (tenant_id, region, updated_at DESC, id DESC) \
     INCLUDE (state_category, key) WHERE deleted_at IS NULL AND NOT archived";
pub const CREATE_ISSUE_KEY_PREFIX_LIST_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_key_prefix_list_idx \
     ON issue (tenant_id, region, key text_pattern_ops, updated_at DESC, id DESC) \
     INCLUDE (state_category) WHERE deleted_at IS NULL AND NOT archived";

pub const CREATE_ISSUE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS issue (
  tenant_id              text        NOT NULL,
  region                 text        NOT NULL,
  id                     uuid        NOT NULL,
  key                    text        NOT NULL,
  prefix                 text        NOT NULL,
  type_id                uuid        NOT NULL,
  type_rank              smallint    NOT NULL,
  state                  text        NOT NULL,
  state_category         text        NOT NULL CHECK (state_category IN ('unstarted','started','completed','cancelled')),
  priority               smallint    NOT NULL DEFAULT 0,
  assignee               uuid,
  reporter               uuid        NOT NULL,
  parent_id              uuid,
  project_id             uuid        NOT NULL,
  cycle_id               uuid,
  rank                   text        NOT NULL,
  title                  text        NOT NULL,
  body_block             uuid,
  props                  jsonb       NOT NULL DEFAULT '{}',
  props_nodes            jsonb       NOT NULL DEFAULT '[]',
  created_at             timestamptz NOT NULL DEFAULT now(),
  updated_at             timestamptz NOT NULL DEFAULT now(),
  state_changed_at       timestamptz NOT NULL DEFAULT now(),
  archived               boolean     NOT NULL DEFAULT false,
  deleted_at             timestamptz,
  contains_personal_data boolean     NOT NULL DEFAULT false,
  data_role              text        NOT NULL DEFAULT 'tenant-content',
  restricted             boolean     NOT NULL DEFAULT false,
  pii_key_ref            text,
  version                bigint       NOT NULL,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, key)
)";

pub const CREATE_ISSUE_INDEXES_DDL: &[(&str, &str)] = &[
    (
        ISSUE_BOARD_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_board ON issue (tenant_id, project_id, state_category, rank) WHERE deleted_at IS NULL",
    ),
    (
        ISSUE_ROADMAP_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_roadmap ON issue (tenant_id, project_id, type_rank, rank) WHERE type_rank >= 2 AND deleted_at IS NULL",
    ),
    (
        ISSUE_ASSIGNEE_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_assignee ON issue (tenant_id, assignee, state_category) WHERE deleted_at IS NULL",
    ),
    (
        ISSUE_PARENT_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_parent ON issue (tenant_id, parent_id)",
    ),
    (
        ISSUE_CYCLE_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_cycle ON issue (tenant_id, cycle_id, state_category) WHERE cycle_id IS NOT NULL",
    ),
    (
        ISSUE_PROPS_GIN_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_props_gin ON issue USING gin (props jsonb_path_ops)",
    ),
];

pub const EXPAND_ISSUE_DURABLE_STORE_DDL: &str = "\
ALTER TABLE issue ADD COLUMN IF NOT EXISTS title_nonce bytea;
ALTER TABLE issue ADD COLUMN IF NOT EXISTS title_ciphertext bytea;
ALTER TABLE issue ADD COLUMN IF NOT EXISTS created_by_principal text;";

pub const EXPAND_NULLABLE_REPORTER_DDL: &str =
    "ALTER TABLE issue ALTER COLUMN reporter DROP NOT NULL;";

pub const EXPAND_ISSUE_RELATION_ACTOR_DDL: &str =
    "ALTER TABLE issue_relation ADD COLUMN IF NOT EXISTS created_by_principal text;";

pub const EXPAND_ISSUE_CREATOR_KIND_DDL: &str = r#"
ALTER TABLE issue
  ADD COLUMN IF NOT EXISTS created_by_kind text NOT NULL DEFAULT 'unknown'
  CHECK (created_by_kind IN ('human','agent','service','unknown'));
WITH historic_kind AS (
  SELECT b.tenant_id, b.region, b.issue_id,
         CASE
           WHEN o.envelope #> '{actor,kind}' = '"Human"'::jsonb THEN 'human'
           WHEN (o.envelope #> '{actor,kind}') ? 'Agent' THEN 'agent'
           WHEN o.envelope #> '{actor,kind}' = '"Service"'::jsonb THEN 'service'
           ELSE 'unknown'
         END AS actor_kind
  FROM issue_authz_binding b
  LEFT JOIN outbox o ON o.event_id = b.created_event_id
)
UPDATE issue AS i
   SET created_by_kind = historic_kind.actor_kind
  FROM historic_kind
 WHERE i.tenant_id = historic_kind.tenant_id
   AND i.region = historic_kind.region
   AND i.id = historic_kind.issue_id
   AND i.created_by_kind = 'unknown';
"#;

pub const EXPAND_ISSUE_RELATION_CREATOR_KIND_DDL: &str = r#"
ALTER TABLE issue_relation
  ADD COLUMN IF NOT EXISTS created_by_kind text NOT NULL DEFAULT 'unknown'
  CHECK (created_by_kind IN ('human','agent','service','unknown'));
WITH historic_kind AS (
  SELECT envelope #>> '{tenant}' AS tenant_id,
         envelope #>> '{region}' AS region,
         envelope #>> '{payload,relation_id}' AS relation_id,
         CASE
           WHEN envelope #> '{actor,kind}' = '"Human"'::jsonb THEN 'human'
           WHEN (envelope #> '{actor,kind}') ? 'Agent' THEN 'agent'
           WHEN envelope #> '{actor,kind}' = '"Service"'::jsonb THEN 'service'
           ELSE 'unknown'
         END AS actor_kind
  FROM outbox
  WHERE envelope #>> '{type_}' = 'issue.relation.created'
)
UPDATE issue_relation AS relation
   SET created_by_kind = historic_kind.actor_kind
  FROM historic_kind
 WHERE relation.tenant_id = historic_kind.tenant_id
   AND relation.region = historic_kind.region
   AND relation.relation_id::text = historic_kind.relation_id
   AND relation.created_by_kind = 'unknown';
"#;

pub const CREATE_ISSUE_AUTHZ_BINDING_DDL: &str = "\
CREATE TABLE IF NOT EXISTS issue_authz_binding (
  tenant_id        text        NOT NULL,
  region           text        NOT NULL,
  issue_id         uuid        NOT NULL,
  project_id       uuid        NOT NULL,
  issue_object     text        NOT NULL,
  project_userset  text        NOT NULL,
  relation         text        NOT NULL CHECK (relation = 'parent_project'),
  request_event_id text        NOT NULL,
  created_event_id text        NOT NULL,
  state            text        NOT NULL CHECK (state IN ('pending','active')),
  zookie           text,
  attempts         int         NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  last_error       text,
  created_at       timestamptz NOT NULL DEFAULT now(),
  activated_at     timestamptz,
  PRIMARY KEY (tenant_id, issue_id),
  UNIQUE (request_event_id),
  UNIQUE (created_event_id),
  FOREIGN KEY (tenant_id, issue_id) REFERENCES issue (tenant_id, id),
  CHECK ((state = 'pending' AND zookie IS NULL AND activated_at IS NULL) OR
         (state = 'active' AND zookie IS NOT NULL AND activated_at IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS issue_authz_binding_pending
  ON issue_authz_binding (tenant_id, state, created_at, issue_id);";

pub const EXPAND_ISSUE_AUTHZ_CREATED_EVENT_DDL: &str = "\
CREATE INDEX IF NOT EXISTS issue_authz_binding_pending_region
  ON issue_authz_binding (tenant_id, region, state, created_at, issue_id);";

pub const CREATE_ISSUE_RELATION_DDL: &str = "\
CREATE TABLE IF NOT EXISTS issue_relation (
  tenant_id   text        NOT NULL,
  region      text        NOT NULL,
  relation_id uuid        NOT NULL,
  src_issue   uuid        NOT NULL,
  dst_ref     text        NOT NULL,
  rel         text        NOT NULL CHECK (rel IN ('parent','blocks','blocked_by','closes','depends_on','relates')),
  created_by  uuid        NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, relation_id),
  UNIQUE (tenant_id, src_issue, dst_ref, rel),
  FOREIGN KEY (tenant_id, src_issue) REFERENCES issue(tenant_id, id) ON DELETE CASCADE
)";

pub const CREATE_ISSUE_RELATION_INDEXES_DDL: &[(&str, &str)] = &[
    (
        "issue_rel_src",
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_rel_src ON issue_relation (tenant_id, src_issue, rel)",
    ),
    (
        "issue_rel_dst",
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS issue_rel_dst ON issue_relation (tenant_id, dst_ref, rel)",
    ),
];

pub const CREATE_ISSUE_CHANGE_LOG_DDL: &str = "\
CREATE TABLE IF NOT EXISTS issue_change_log (
  tenant_id    text        NOT NULL,
  region       text        NOT NULL,
  issue_id     uuid        NOT NULL,
  seq          bigint      NOT NULL,
  actor        uuid        NOT NULL,
  actor_kind   text        NOT NULL CHECK (actor_kind IN ('human','agent','service')),
  on_behalf_of uuid,
  change       jsonb       NOT NULL,
  pii_key_ref  text,
  at           timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, issue_id, seq)
)";

pub const CREATE_SCHEME_DDL: &str = "\
CREATE TABLE IF NOT EXISTS scheme (
  tenant_id text   NOT NULL,
  region    text   NOT NULL,
  scheme_id uuid   NOT NULL,
  kind      text   NOT NULL CHECK (kind IN ('workflow','field','permission','sla','type')),
  name      text   NOT NULL,
  body      jsonb  NOT NULL,
  version   bigint NOT NULL,
  PRIMARY KEY (tenant_id, scheme_id)
)";

pub const CREATE_SCHEME_ASSIGNMENT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS scheme_assignment (
  tenant_id  text NOT NULL,
  region     text NOT NULL,
  scheme_id  uuid NOT NULL,
  kind       text NOT NULL CHECK (kind IN ('workflow','field','permission','sla','type')),
  type_id    uuid,
  project_id uuid,
  team_id    uuid,
  type_key    uuid GENERATED ALWAYS AS
                (COALESCE(type_id, '00000000-0000-0000-0000-000000000000'::uuid)) STORED,
  project_key uuid GENERATED ALWAYS AS
                (COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid)) STORED,
  team_key    uuid GENERATED ALWAYS AS
                (COALESCE(team_id, '00000000-0000-0000-0000-000000000000'::uuid)) STORED,
  PRIMARY KEY (tenant_id, kind, type_key, project_key, team_key)
)";

pub const CREATE_CYCLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS cycle (
  tenant_id  text        NOT NULL,
  region     text        NOT NULL,
  cycle_id   uuid        NOT NULL,
  project_id uuid        NOT NULL,
  name       text        NOT NULL,
  starts_at  timestamptz NOT NULL,
  ends_at    timestamptz NOT NULL,
  capacity   numeric,
  state      text        NOT NULL CHECK (state IN ('planned','active','completed')),
  PRIMARY KEY (tenant_id, cycle_id)
)";

pub const CREATE_CYCLE_MEMBERSHIP_DDL: &str = "\
CREATE TABLE IF NOT EXISTS cycle_membership (
  tenant_id         text        NOT NULL,
  region            text        NOT NULL,
  cycle_id          uuid        NOT NULL,
  issue_id          uuid        NOT NULL,
  added_at          timestamptz NOT NULL DEFAULT now(),
  carried_over_from uuid,
  PRIMARY KEY (tenant_id, cycle_id, issue_id)
)";

pub const CREATE_MILESTONE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS milestone (
  tenant_id    text        NOT NULL,
  region       text        NOT NULL,
  milestone_id uuid        NOT NULL,
  project_id   uuid        NOT NULL,
  name         text        NOT NULL,
  target_date  date,
  released_at  timestamptz,
  PRIMARY KEY (tenant_id, milestone_id)
)";

pub const CREATE_PREFIX_COUNTER_DDL: &str = "\
CREATE TABLE IF NOT EXISTS prefix_counter (
  tenant_id  text   NOT NULL,
  region     text   NOT NULL,
  prefix     text   NOT NULL,
  high_water bigint NOT NULL,
  block_size int    NOT NULL DEFAULT 50,
  PRIMARY KEY (tenant_id, prefix)
)";

pub const CREATE_IMPORT_MAP_DDL: &str = "\
CREATE TABLE IF NOT EXISTS import_map (
  tenant_id   text NOT NULL,
  region      text NOT NULL,
  import_job  uuid NOT NULL,
  source      text NOT NULL CHECK (source IN ('jira','linear','github','csv','canonical')),
  source_id   text NOT NULL CHECK (octet_length(source_id) BETWEEN 1 AND 512),
  myelin_kind text NOT NULL CHECK (myelin_kind IN ('issue','cycle','milestone','relation','user')),
  myelin_id   uuid,
  status      text NOT NULL CHECK (status IN ('pending','created','wired','lossy','dropped')),
  loss_note   text,
  PRIMARY KEY (tenant_id, import_job, source, source_id),
  CHECK (status NOT IN ('lossy','dropped') OR loss_note IS NOT NULL)
)";

pub const EXPAND_IMPORT_MAP_REQUEST_HASH_DDL: &str = "\
ALTER TABLE import_map
  ADD COLUMN IF NOT EXISTS request_hash text
  CHECK (request_hash IS NULL OR request_hash ~ '^blake3:[0-9a-f]{64}$')";

pub const EXPAND_IMPORT_MAP_IDENTITY_INDEX_DDL: &str = "\
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS import_map_identity_v2
  ON import_map (tenant_id, region, import_job, source, source_id, myelin_kind)";

pub const CONTRACT_IMPORT_MAP_IDENTITY_DDL: &str = "\
ALTER TABLE import_map DROP CONSTRAINT import_map_pkey;
ALTER TABLE import_map ADD CONSTRAINT import_map_pkey_v2
  PRIMARY KEY USING INDEX import_map_identity_v2";

pub const CREATE_ISSUE_CREATE_IDEMPOTENCY_DDL: &str = "\
CREATE TABLE IF NOT EXISTS issue_create_idempotency (
  tenant_id     text        NOT NULL,
  region        text        NOT NULL,
  storage_nonce text        NOT NULL CHECK (storage_nonce ~ '^blake3:[0-9a-f]{64}$'),
  request_hash  text        NOT NULL CHECK (request_hash ~ '^blake3:[0-9a-f]{64}$'),
  issue_id      uuid,
  status        text        NOT NULL CHECK (status IN ('pending','created')),
  created_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, region, storage_nonce),
  CHECK ((status = 'pending' AND issue_id IS NULL) OR
         (status = 'created' AND issue_id IS NOT NULL))
)";

pub const CREATE_CONSUMER_DEDUP_DDL: &str = CONSUMER_DEDUP_MIGRATION;

pub fn make_tenant_scoped_ddl(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

fn create_statements() -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("iss_0001_issue", ISSUE_TABLE, CREATE_ISSUE_DDL.to_string()),
        (
            "iss_0002_issue_relation",
            ISSUE_RELATION_TABLE,
            CREATE_ISSUE_RELATION_DDL.to_string(),
        ),
        (
            "iss_0003_issue_change_log",
            ISSUE_CHANGE_LOG_TABLE,
            CREATE_ISSUE_CHANGE_LOG_DDL.to_string(),
        ),
        (
            "iss_0004_scheme",
            SCHEME_TABLE,
            CREATE_SCHEME_DDL.to_string(),
        ),
        (
            "iss_0005_scheme_assignment",
            SCHEME_ASSIGNMENT_TABLE,
            CREATE_SCHEME_ASSIGNMENT_DDL.to_string(),
        ),
        ("iss_0006_cycle", CYCLE_TABLE, CREATE_CYCLE_DDL.to_string()),
        (
            "iss_0007_cycle_membership",
            CYCLE_MEMBERSHIP_TABLE,
            CREATE_CYCLE_MEMBERSHIP_DDL.to_string(),
        ),
        (
            "iss_0008_milestone",
            MILESTONE_TABLE,
            CREATE_MILESTONE_DDL.to_string(),
        ),
        (
            "iss_0009_prefix_counter",
            PREFIX_COUNTER_TABLE,
            CREATE_PREFIX_COUNTER_DDL.to_string(),
        ),
        (
            "iss_0010_consumer_dedup",
            CONSUMER_DEDUP_TABLE,
            CREATE_CONSUMER_DEDUP_DDL.to_string(),
        ),
        (
            "iss_0011_outbox",
            OUTBOX_TABLE,
            OUTBOX_MIGRATION.to_string(),
        ),
    ]
}

pub fn issues_migrations() -> Migrations {
    let mut migrations = Vec::new();
    for (id, table, create) in create_statements() {
        let mut ddl = create;
        if !ddl.trim_end().ends_with(';') {
            ddl.push(';');
        }
        if table != OUTBOX_TABLE && table != CONSUMER_DEDUP_TABLE {
            ddl.push('\n');
            ddl.push_str(&make_tenant_scoped_ddl(table));
            ddl.push(';');
        }
        migrations.push(Migration::plain_on(id, ddl, table));
    }
    for &(name, ddl) in CREATE_ISSUE_INDEXES_DDL {
        let id = format!("iss_0012_{name}");
        migrations.push(Migration::plain_on(id, ddl, ISSUE_TABLE));
    }
    for &(name, ddl) in CREATE_ISSUE_RELATION_INDEXES_DDL {
        let id = format!("iss_0013_{name}");
        migrations.push(Migration::plain_on(id, ddl, ISSUE_RELATION_TABLE));
    }
    migrations.push(Migration::plain_on(
        "iss_0014_issue_durable_store_expand",
        EXPAND_ISSUE_DURABLE_STORE_DDL,
        ISSUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        "iss_0015_nullable_reporter_expand",
        EXPAND_NULLABLE_REPORTER_DDL,
        ISSUE_TABLE,
    ));
    let authz_binding_ddl = format!(
        "{}\n{};",
        CREATE_ISSUE_AUTHZ_BINDING_DDL,
        make_tenant_scoped_ddl(ISSUE_AUTHZ_BINDING_TABLE)
    );
    migrations.push(Migration::plain_on(
        "iss_0016_issue_authz_binding",
        authz_binding_ddl,
        ISSUE_AUTHZ_BINDING_TABLE,
    ));
    migrations.push(Migration::plain_on(
        "iss_0017_issue_authz_created_event",
        EXPAND_ISSUE_AUTHZ_CREATED_EVENT_DDL,
        ISSUE_AUTHZ_BINDING_TABLE,
    ));
    let issue_authz_visible_ddl = format!(
        "{}\n{};",
        CREATE_ISSUE_AUTHZ_VISIBLE_DDL,
        make_tenant_scoped_ddl(ISSUE_AUTHZ_VISIBLE_TABLE)
    );
    migrations.push(Migration::plain_on(
        "iss_0018_issue_authz_visible",
        issue_authz_visible_ddl,
        ISSUE_AUTHZ_VISIBLE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        "iss_0019_issue_authz_invalidation_triggers",
        CREATE_ISSUE_AUTHZ_INVALIDATION_TRIGGERS_DDL,
        ISSUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        "iss_0020_issue_recent_list_idx",
        CREATE_ISSUE_RECENT_LIST_INDEX_DDL,
        ISSUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        "iss_0021_issue_key_prefix_list_idx",
        CREATE_ISSUE_KEY_PREFIX_LIST_INDEX_DDL,
        ISSUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        "iss_0022_narrow_issue_authz_invalidation",
        NARROW_ISSUE_AUTHZ_INVALIDATION_TRIGGER_DDL,
        ISSUE_TABLE,
    ));
    let import_map_ddl = format!(
        "{};\n{};",
        CREATE_IMPORT_MAP_DDL,
        make_tenant_scoped_ddl(IMPORT_MAP_TABLE)
    );
    migrations.push(Migration::plain_on(
        "iss_0023_import_map",
        import_map_ddl,
        IMPORT_MAP_TABLE,
    ));
    let create_idempotency_ddl = format!(
        "{};\n{};",
        CREATE_ISSUE_CREATE_IDEMPOTENCY_DDL,
        make_tenant_scoped_ddl(ISSUE_CREATE_IDEMPOTENCY_TABLE)
    );
    migrations.push(Migration::plain_on(
        "iss_0024_issue_create_idempotency",
        create_idempotency_ddl,
        ISSUE_CREATE_IDEMPOTENCY_TABLE,
    ));
    migrations.push(Migration::phased(
        "iss_0025_issue_relation_actor",
        EXPAND_ISSUE_RELATION_ACTOR_DDL,
        MigrationPhase::Expand,
        ISSUE_RELATION_TABLE,
    ));
    migrations.push(Migration::phased(
        "iss_0026_import_request_hash",
        EXPAND_IMPORT_MAP_REQUEST_HASH_DDL,
        MigrationPhase::Expand,
        IMPORT_MAP_TABLE,
    ));
    migrations.push(Migration::phased(
        "iss_0027_import_identity_index",
        EXPAND_IMPORT_MAP_IDENTITY_INDEX_DDL,
        MigrationPhase::Expand,
        IMPORT_MAP_TABLE,
    ));
    migrations.push(Migration::phased(
        "iss_0028_import_identity_contract",
        CONTRACT_IMPORT_MAP_IDENTITY_DDL,
        MigrationPhase::Contract,
        IMPORT_MAP_TABLE,
    ));
    migrations.push(Migration::phased(
        "iss_0029_issue_creator_kind",
        EXPAND_ISSUE_CREATOR_KIND_DDL,
        MigrationPhase::Expand,
        ISSUE_TABLE,
    ));
    migrations.push(Migration::phased(
        "iss_0030_issue_relation_creator_kind",
        EXPAND_ISSUE_RELATION_CREATOR_KIND_DDL,
        MigrationPhase::Expand,
        ISSUE_RELATION_TABLE,
    ));
    let issue_view_subject_ddl = format!(
        "{}\n{};\n{}",
        CREATE_ISSUE_VIEW_SUBJECT_DDL,
        make_tenant_scoped_ddl(ISSUE_VIEW_SUBJECT_TABLE),
        INVALIDATE_LEGACY_ISSUE_VIEW_DDL,
    );
    migrations.push(Migration::plain_on(
        "iss_0031_factored_issue_view",
        issue_view_subject_ddl,
        ISSUE_VIEW_SUBJECT_TABLE,
    ));
    Migrations::of(migrations)
}

pub fn issues_hot_tables() -> HotTables {
    HotTables::declare([
        ISSUE_TABLE,
        ISSUE_RELATION_TABLE,
        ISSUE_CHANGE_LOG_TABLE,
        IMPORT_MAP_TABLE,
        ISSUE_CREATE_IDEMPOTENCY_TABLE,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_eleven_spine_tables_are_present_fk_ordered() {
        let migrations = issues_migrations();
        let create_ids: std::collections::BTreeSet<&str> =
            create_statements().iter().map(|(id, _, _)| *id).collect();
        let tables: Vec<&str> = migrations
            .0
            .iter()
            .filter(|m| create_ids.contains(m.id.as_ref()))
            .map(|m| m.table.as_deref().expect("create migrations name a table"))
            .collect();
        assert_eq!(
            tables,
            vec![
                ISSUE_TABLE,
                ISSUE_RELATION_TABLE,
                ISSUE_CHANGE_LOG_TABLE,
                SCHEME_TABLE,
                SCHEME_ASSIGNMENT_TABLE,
                CYCLE_TABLE,
                CYCLE_MEMBERSHIP_TABLE,
                MILESTONE_TABLE,
                PREFIX_COUNTER_TABLE,
                CONSUMER_DEDUP_TABLE,
                OUTBOX_TABLE,
            ],
            "all 11 spine tables, FK-dependency ordered (issue before issue_relation)"
        );
        let issue_pos = tables.iter().position(|t| *t == ISSUE_TABLE).unwrap();
        let rel_pos = tables
            .iter()
            .position(|t| *t == ISSUE_RELATION_TABLE)
            .unwrap();
        assert!(
            issue_pos < rel_pos,
            "issue is created before issue_relation (the FK target before the FK source)"
        );
    }

    #[test]
    fn every_domain_table_is_tenant_region_first_and_rls_scoped() {
        for (_id, table, ddl) in create_statements() {
            if table == OUTBOX_TABLE || table == CONSUMER_DEDUP_TABLE {
                continue;
            }
            let tenant_pos = ddl.find("tenant_id").expect("tenant_id column");
            let region_pos = ddl.find("region").expect("region column");
            assert!(
                tenant_pos < region_pos,
                "tenant_id is the FIRST column (before region) on `{table}`: {ddl}"
            );
            assert!(
                ddl.contains("PRIMARY KEY (tenant_id"),
                "the primary key is tenant-first on `{table}`: {ddl}"
            );
        }
        let create_ids: std::collections::BTreeSet<&str> =
            create_statements().iter().map(|(id, _, _)| *id).collect();
        for m in issues_migrations()
            .0
            .iter()
            .filter(|m| create_ids.contains(m.id.as_ref()))
        {
            if matches!(
                m.table.as_deref(),
                Some(table) if table == OUTBOX_TABLE || table == CONSUMER_DEDUP_TABLE
            ) {
                assert!(
                    !m.ddl.contains("myelin_make_tenant_scoped"),
                    "cell-wide bus plumbing is not given an Issues-local RLS policy"
                );
            } else {
                assert!(
                    m.ddl.contains("myelin_make_tenant_scoped"),
                    "domain migration {} installs the platform RLS policy (0 un-scoped tables)",
                    m.id
                );
            }
        }
        let binding = issues_migrations()
            .0
            .into_iter()
            .find(|m| m.id == "iss_0016_issue_authz_binding")
            .expect("authorization binding migration");
        assert!(CREATE_ISSUE_AUTHZ_BINDING_DDL
            .starts_with("CREATE TABLE IF NOT EXISTS issue_authz_binding (\n  tenant_id"));
        assert!(CREATE_ISSUE_AUTHZ_BINDING_DDL.contains("\n  region"));
        assert!(binding.ddl.contains("myelin_make_tenant_scoped"));
        for invariant in [
            "request_event_id text",
            "created_event_id text",
            "UNIQUE (request_event_id)",
            "UNIQUE (created_event_id)",
            "state IN ('pending','active')",
            "FOREIGN KEY (tenant_id, issue_id)",
        ] {
            assert!(
                binding.ddl.contains(invariant),
                "authorization binding pins `{invariant}`"
            );
        }
        let expand = issues_migrations()
            .0
            .into_iter()
            .find(|m| m.id == "iss_0017_issue_authz_created_event")
            .expect("authorization created-event expansion");
        for invariant in [
            "issue_authz_binding_pending_region",
            "tenant_id, region, state, created_at, issue_id",
        ] {
            assert!(
                expand.ddl.contains(invariant),
                "expansion pins `{invariant}`"
            );
        }
    }

    #[test]
    fn the_migration_set_is_forward_only() {
        let migrations = issues_migrations();
        for m in &migrations.0 {
            assert!(
                !myelin_substrate::is_destructive(m.ddl.as_ref()),
                "migration {} is forward-only (no DROP): {}",
                m.id,
                m.ddl
            );
        }
    }

    #[test]
    fn effective_projection_migrations_require_the_storage_invalidator_first() {
        let durable_migrations = myelin_storage::all_durable_migrations();
        let durable_ids: Vec<_> = durable_migrations
            .0
            .iter()
            .map(|migration| migration.id.as_ref())
            .collect();
        let invalidator = durable_ids
            .iter()
            .position(|id| *id == "0069_authz_projection_invalidator")
            .expect("the storage aggregate carries the projection invalidator");
        let first_later_storage_migration = durable_ids
            .iter()
            .position(|id| *id == "0070_auth_replay")
            .expect("later append-only storage migrations remain in the same aggregate");
        assert!(
            invalidator < first_later_storage_migration,
            "the projection invalidator must retain its append-only migration position"
        );
        let issues = issues_migrations();
        assert!(issues
            .0
            .iter()
            .any(|migration| migration.id == "iss_0018_issue_authz_visible"));
        assert_eq!(
            issues.0.last().map(|migration| migration.id.as_ref()),
            Some("iss_0031_factored_issue_view")
        );
        for invariant in [
            "CREATE INDEX CONCURRENTLY",
            "tenant_id, region, key text_pattern_ops, updated_at DESC, id DESC",
            "WHERE deleted_at IS NULL AND NOT archived",
        ] {
            assert!(
                CREATE_ISSUE_KEY_PREFIX_LIST_INDEX_DDL.contains(invariant),
                "key-prefix index pins `{invariant}`"
            );
        }
        assert!(CREATE_ISSUE_RECENT_LIST_INDEX_DDL
            .contains("WHERE deleted_at IS NULL AND NOT archived"));
        assert!(CREATE_ISSUE_AUTHZ_INVALIDATION_TRIGGERS_DDL
            .contains("EXECUTE FUNCTION myelin_invalidate_issue_view_projection()"));
        assert!(!CREATE_ISSUE_AUTHZ_INVALIDATION_TRIGGERS_DDL.contains("to_regprocedure"));
        assert!(NARROW_ISSUE_AUTHZ_INVALIDATION_TRIGGER_DDL
            .contains("AFTER INSERT OR DELETE OR UPDATE OF project_id ON issue"));
        assert!(!NARROW_ISSUE_AUTHZ_INVALIDATION_TRIGGER_DDL.contains("state"));
        assert!(CREATE_ISSUE_AUTHZ_VISIBLE_DDL.contains("permission  text NOT NULL"));
        assert!(!CREATE_ISSUE_AUTHZ_VISIBLE_DDL.contains("relation  text NOT NULL"));
    }

    #[test]
    fn the_import_map_is_durable_scoped_and_state_constrained() {
        let migration = issues_migrations()
            .0
            .into_iter()
            .find(|migration| migration.id == "iss_0023_import_map")
            .expect("the import map migration");
        assert_eq!(migration.table.as_deref(), Some(IMPORT_MAP_TABLE));
        for invariant in [
            "PRIMARY KEY (tenant_id, import_job, source, source_id)",
            "source IN ('jira','linear','github','csv','canonical')",
            "status IN ('pending','created','wired','lossy','dropped')",
            "status NOT IN ('lossy','dropped') OR loss_note IS NOT NULL",
            "myelin_make_tenant_scoped('import_map')",
        ] {
            assert!(
                migration.ddl.contains(invariant),
                "the import map pins `{invariant}`"
            );
        }
        assert!(issues_hot_tables().is_hot(IMPORT_MAP_TABLE));
        let request_hash_expansion = issues_migrations()
            .0
            .into_iter()
            .find(|migration| migration.id == "iss_0026_import_request_hash")
            .expect("the import request fingerprint expansion");
        assert_eq!(request_hash_expansion.phase, MigrationPhase::Expand);
        assert!(request_hash_expansion
            .ddl
            .contains("ADD COLUMN IF NOT EXISTS request_hash"));
        assert!(request_hash_expansion.ddl.contains("request_hash IS NULL"));

        let identity_index = issues_migrations()
            .0
            .into_iter()
            .find(|migration| migration.id == "iss_0027_import_identity_index")
            .expect("the region-and-kind import identity index");
        assert_eq!(identity_index.phase, MigrationPhase::Expand);
        assert!(identity_index.ddl.contains("UNIQUE INDEX CONCURRENTLY"));
        assert!(identity_index
            .ddl
            .contains("tenant_id, region, import_job, source, source_id, myelin_kind"));

        let identity_contract = issues_migrations()
            .0
            .into_iter()
            .find(|migration| migration.id == "iss_0028_import_identity_contract")
            .expect("the region-and-kind import identity contract");
        assert_eq!(identity_contract.phase, MigrationPhase::Contract);
        assert!(identity_contract
            .ddl
            .contains("DROP CONSTRAINT import_map_pkey"));
        assert!(identity_contract
            .ddl
            .contains("PRIMARY KEY USING INDEX import_map_identity_v2"));
    }

    #[test]
    fn interactive_create_idempotency_is_durable_scoped_and_state_constrained() {
        let migration = issues_migrations()
            .0
            .into_iter()
            .find(|migration| migration.id == "iss_0024_issue_create_idempotency")
            .expect("the interactive create idempotency migration");
        assert_eq!(
            migration.table.as_deref(),
            Some(ISSUE_CREATE_IDEMPOTENCY_TABLE)
        );
        for invariant in [
            "PRIMARY KEY (tenant_id, region, storage_nonce)",
            "status IN ('pending','created')",
            "status = 'pending' AND issue_id IS NULL",
            "status = 'created' AND issue_id IS NOT NULL",
            "myelin_make_tenant_scoped('issue_create_idempotency')",
        ] {
            assert!(
                migration.ddl.contains(invariant),
                "the interactive create ledger pins `{invariant}`"
            );
        }
        assert!(issues_hot_tables().is_hot(ISSUE_CREATE_IDEMPOTENCY_TABLE));
        assert!(
            !migration.ddl.contains("REFERENCES issue"),
            "the privacy-safe retry receipt survives issue deletion so a retry cannot recreate work"
        );
    }

    #[test]
    fn the_runner_admits_the_whole_set_idempotently() {
        use myelin_substrate::MigrationRunner;
        let migrations = issues_migrations();
        let expected_migrations = migrations.0.len();
        let hot = issues_hot_tables();
        let mut runner = MigrationRunner::new();
        runner
            .run(&migrations, &hot)
            .expect("the full Issue-Tracker spine applies forward-only");
        assert_eq!(
            runner.applied().len(),
            expected_migrations,
            "the runner applied every table/index/expand migration"
        );
        assert_eq!(
            runner.applied()[0],
            "iss_0001_issue",
            "issue is applied first (FK order)"
        );

        let mut runner2 = MigrationRunner::new();
        runner2
            .run(&migrations, &hot)
            .expect("the spine re-applies idempotently");
        assert_eq!(
            runner2.applied().len(),
            expected_migrations,
            "the re-apply admits every migration again"
        );
    }

    #[test]
    fn a_destructive_rollback_is_refused() {
        use myelin_substrate::MigrationRunner;
        let bad = Migrations::of([Migration::plain("iss_9999_drop", "DROP TABLE issue")]);
        let mut runner = MigrationRunner::new();
        let e = runner
            .run(&bad, &issues_hot_tables())
            .expect_err("a DROP must be refused");
        assert!(
            e.0.contains("forward-only"),
            "the refusal names forward-only: {}",
            e.0
        );
    }

    #[test]
    fn the_three_hot_tables_are_declared() {
        let hot = issues_hot_tables();
        for t in [ISSUE_TABLE, ISSUE_RELATION_TABLE, ISSUE_CHANGE_LOG_TABLE] {
            assert!(hot.is_hot(t), "`{t}` is declared hot (arch 01 §8.1)");
        }
        assert!(
            !hot.is_hot(SCHEME_TABLE),
            "scheme is NOT a hot table (config write rate)"
        );
        assert!(
            !hot.is_hot(MILESTONE_TABLE),
            "milestone is NOT a hot table (low write rate)"
        );
    }

    #[test]
    fn the_issue_lifecycle_and_gdpr_columns_are_present() {
        for col in [
            "deleted_at",
            "contains_personal_data",
            "data_role",
            "restricted",
            "pii_key_ref",
            "version",
            "props",
            "props_nodes",
        ] {
            assert!(
                CREATE_ISSUE_DDL.contains(col),
                "the issue lifecycle/GDPR/tail column `{col}` is present (recon §X-7 / §2)"
            );
        }
        assert!(
            CREATE_ISSUE_DDL.contains("assignee               uuid")
                && CREATE_ISSUE_DDL.contains("reporter               uuid"),
            "assignee/reporter are pseudonymous principal ids (erasure-safe, 4.8)"
        );
    }

    #[test]
    fn the_frozen_vocabularies_are_check_constraints() {
        let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squash(CREATE_ISSUE_DDL).contains(
                "state_category text NOT NULL CHECK (state_category IN ('unstarted','started','completed','cancelled'))"
            ),
            "issue.state_category is the frozen FOUR invariant categories (sketch 02)"
        );
        assert!(
            squash(CREATE_ISSUE_RELATION_DDL).contains(
                "rel text NOT NULL CHECK (rel IN ('parent','blocks','blocked_by','closes','depends_on','relates'))"
            ),
            "issue_relation.rel is the frozen six-relation vocabulary (§4)"
        );
        assert!(
            squash(CREATE_SCHEME_DDL).contains(
                "kind text NOT NULL CHECK (kind IN ('workflow','field','permission','sla','type'))"
            ),
            "scheme.kind is the frozen five-kind vocabulary (§3)"
        );
    }

    #[test]
    fn issue_relation_is_the_forward_edge_fk_anchored_on_src() {
        assert!(
            CREATE_ISSUE_RELATION_DDL
                .contains("FOREIGN KEY (tenant_id, src_issue) REFERENCES issue(tenant_id, id) ON DELETE CASCADE"),
            "the FK constrains only the src_issue end (the dst_ref may be cross-subsystem)"
        );
        assert!(
            CREATE_ISSUE_RELATION_DDL.contains("UNIQUE (tenant_id, src_issue, dst_ref, rel)"),
            "the forward edge is unique per (src_issue, dst_ref, rel) - no dual-write"
        );
        assert!(
            CREATE_ISSUE_RELATION_DDL.contains("dst_ref     text"),
            "dst_ref is an ArtifactRef text (may be cross-subsystem, §4)"
        );
        let actor_expansion = issues_migrations()
            .0
            .into_iter()
            .find(|migration| migration.id == "iss_0025_issue_relation_actor")
            .expect("issue relation actor expansion");
        assert_eq!(actor_expansion.ddl, EXPAND_ISSUE_RELATION_ACTOR_DDL);
        assert!(
            actor_expansion.ddl.contains("created_by_principal text"),
            "human, agent, and service principal ids retain their canonical attribution"
        );
        assert_eq!(actor_expansion.phase, MigrationPhase::Expand);
    }

    #[test]
    fn creator_kind_is_durable_and_legacy_backfill_reads_typed_event_provenance() {
        let migrations = issues_migrations();
        for (id, table) in [
            ("iss_0029_issue_creator_kind", ISSUE_TABLE),
            ("iss_0030_issue_relation_creator_kind", ISSUE_RELATION_TABLE),
        ] {
            let migration = migrations
                .0
                .iter()
                .find(|migration| migration.id == id)
                .expect("creator-kind migration");
            assert_eq!(migration.table.as_deref(), Some(table));
            assert_eq!(migration.phase, MigrationPhase::Expand);
            assert!(migration.ddl.contains("created_by_kind"));
            assert!(migration.ddl.contains("'{actor,kind}'"));
            assert!(!migration.ddl.contains("created_by_principal LIKE"));
        }
    }

    #[test]
    fn the_six_issue_indexes_ride_the_migration_concurrently() {
        let migrations = issues_migrations();
        for (name, ddl) in CREATE_ISSUE_INDEXES_DDL {
            let matching: Vec<_> = migrations
                .0
                .iter()
                .filter(|m| m.table.as_deref() == Some(ISSUE_TABLE) && m.ddl.as_ref() == *ddl)
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "index `{name}` has exactly one standalone migration"
            );
            assert_eq!(
                matching[0].ddl.matches(';').count(),
                0,
                "one SQL statement per concurrent-index query"
            );
        }
        for (name, ddl) in CREATE_ISSUE_INDEXES_DDL {
            assert!(
                ddl.contains("CONCURRENTLY"),
                "issue index `{name}` is built CONCURRENTLY (hot-table expand discipline)"
            );
        }
        let board = CREATE_ISSUE_INDEXES_DDL
            .iter()
            .find(|(n, _)| *n == ISSUE_BOARD_INDEX)
            .map(|(_, d)| *d)
            .unwrap();
        assert!(
            board.contains("(tenant_id, project_id, state_category, rank)")
                && board.contains("WHERE deleted_at IS NULL"),
            "issue_board is the tenant-first board scan over (project, category, rank), live-only"
        );
    }

    #[test]
    fn the_outbox_is_the_frozen_platform_shape_co_located() {
        let outbox = issues_migrations()
            .0
            .into_iter()
            .find(|m| m.table.as_deref() == Some(OUTBOX_TABLE))
            .unwrap();
        assert!(
            outbox.ddl.contains("CREATE TABLE IF NOT EXISTS outbox"),
            "the outbox is the frozen platform 2.3 table"
        );
        assert!(
            outbox.ddl.contains("UNIQUE (aggregate, seq)"),
            "the outbox carries the (aggregate, seq) co-commit ordering key"
        );
        assert!(
            outbox.ddl.starts_with(OUTBOX_MIGRATION),
            "the outbox migration is the frozen myelin_events::OUTBOX_MIGRATION, verbatim"
        );
    }

    #[test]
    fn the_consumer_dedup_ledger_is_the_platform_2_5_shape() {
        assert_eq!(CREATE_CONSUMER_DEDUP_DDL, CONSUMER_DEDUP_MIGRATION);
        for col in ["consumer", "event_id", "recorded_at"] {
            assert!(
                CREATE_CONSUMER_DEDUP_DDL.contains(col),
                "the 2.5 column `{col}` is declared"
            );
        }
        assert!(
            CREATE_CONSUMER_DEDUP_DDL.contains("PRIMARY KEY (consumer, event_id)"),
            "the exactly-once dedup key is (consumer, event_id) - the platform consumer template"
        );
    }
}
