use myelin_substrate::{HotTables, Migration, Migrations};

pub const CI_RUN_TABLE: &str = "ci_run";
pub const CI_DRIVE_MANIFEST_TABLE: &str = "ci_drive_manifest";
pub const CI_JOB_TABLE: &str = "ci_job";
pub const CHECK_ATTEMPT_TABLE: &str = "check_attempt";
pub const CI_RUN_CHECK_ATTEMPT_TABLE: &str = "ci_run_check_attempt";
pub const JOB_QUEUE_TABLE: &str = "job_queue";
pub const FAIR_DEFICIT_TABLE: &str = "fair_deficit";
pub const RUNNER_TABLE: &str = "runner";
pub const LOG_SEGMENT_TABLE: &str = "log_segment";
pub const LOG_ANCHOR_TABLE: &str = "log_anchor";
pub const ARTIFACT_TABLE: &str = "artifact";
pub const CACHE_ENTRY_TABLE: &str = "cache_entry";
pub const ENVIRONMENT_TABLE: &str = "environment";
pub const DEPLOYMENT_TABLE: &str = "deployment";
pub const SECRET_BINDING_TABLE: &str = "secret_binding";
pub const CI_SECRET_TABLE: &str = "ci_secret";
pub const CI_SECRET_MIGRATION_ID: &str = "ci_0023_ci_secret";
pub const CI_SECRET_ADMIN_SCOPE_MIGRATION_ID: &str = "ci_0024a_secret_admin_scope";
pub const CI_SECRET_ADMIN_UNIQUE_MIGRATION_ID: &str = "ci_0024b_secret_admin_unique";
pub const CI_SECRET_BINDING_INTEGRITY_MIGRATION_ID: &str = "ci_0024c_secret_binding_integrity";
pub const CI_SECRET_TOMBSTONE_MIGRATION_ID: &str = "ci_0024d_secret_version_tombstone";
pub const CI_SECRET_TOMBSTONE_TABLE: &str = "ci_secret_tombstone";
pub const CI_SECRET_VERSION_HIGH_WATER_MIGRATION_ID: &str = "ci_0024e_secret_version_high_water";
pub const CI_SECRET_VERSION_HIGH_WATER_TABLE: &str = "ci_secret_version_high_water";
pub const CI_COST_EVENT_TABLE: &str = "ci_cost_event";
pub const CI_JOB_ACCOUNTING_TABLE: &str = "ci_job_accounting";
pub const CI_JOB_PARENT_ATTEMPT_TABLE: &str = "ci_job_parent_attempt";
pub const CI_JOB_PRELAUNCH_USAGE_TABLE: &str = "ci_job_prelaunch_usage";
pub const CI_JOB_CREDENTIAL_GENERATION_TABLE: &str = "ci_job_credential_generation";
pub const CI_JOB_SPEC_TABLE: &str = "ci_job_spec";

pub const JQ_CLAIMABLE_INDEX: &str = "jq_claimable";
pub const JQ_SERIALIZE_INDEX: &str = "jq_serialize";
pub const JQ_IDEM_INDEX: &str = "jq_idem";
pub const CI_JOB_RUN_LEDGER_INDEX: &str = "ci_job_run_ledger";
pub const CI_RUN_QUEUED_REGION_INDEX: &str = "ci_run_queued_region";
pub const CI_WORKFLOW_ACTIVE_REGION_INDEX: &str = "ci_workflow_active_region";
pub const CI_RUN_SURFACE_REPO_CREATED_INDEX: &str = "ci_run_surface_repo_created";
pub const CI_RUN_SURFACE_INDEX_READINESS: myelin_storage::IndexReadinessSpec<'static> =
    myelin_storage::IndexReadinessSpec::new(
        CI_RUN_SURFACE_REPO_CREATED_INDEX,
        CI_RUN_TABLE,
        "r",
        "i",
        "btree",
        &[
            "tenant_id",
            "region",
            "repo_ref",
            "created_at DESC",
            "run_id DESC",
        ],
        Some("(repo_ref IS NOT NULL)"),
    );
pub const CI_JOB_RUN_LEDGER_INDEX_MIGRATION_ID: &str = "ci_0002a_ci_job_run_ledger";
pub const CI_JOB_RUN_LEDGER_VALIDATION_MIGRATION_ID: &str = "ci_0002b_validate_ci_job_run_ledger";
pub const CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID: &str = "ci_0001b_ci_run_causal_provenance";
pub const CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID: &str = "ci_0001c_ci_run_concurrency_group";
pub const CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID: &str = "ci_0001d_ci_run_pr_head_generation";
pub const CI_RUN_SOURCE_REF_MIGRATION_ID: &str = "ci_0001e_ci_run_source_ref";
pub const CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID: &str = "ci_0025_ci_run_source_ref_constraint";
pub const CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID: &str =
    "ci_0025a_ci_run_source_ref_constraint_validate";
pub const CI_RUN_BRANCH_SCOPE_EXPAND_MIGRATION_ID: &str = "ci_0025b_ci_run_branch_scope_expand";
pub const CI_RUN_BRANCH_SCOPE_VALIDATE_MIGRATION_ID: &str = "ci_0025c_ci_run_branch_scope_validate";
pub const CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID: &str = "ci_0025d_ci_run_branch_scope_contract";
pub const CI_JOB_SPEC_STAGE_MIGRATION_ID: &str = "ci_0015a_ci_job_spec_stage";
pub const CI_JOB_ACCOUNTING_SKIPPED_MIGRATION_ID: &str = "ci_0017a_ci_job_accounting_skipped";
pub const CI_JOB_ACCOUNTING_DISPOSITION_V4_MIGRATION_ID: &str =
    "ci_0017b_ci_job_accounting_disposition_v4";
pub const CI_JOB_ACCOUNTING_DISPOSITION_V4_VERDICT_MIGRATION_ID: &str =
    "ci_0017c_ci_job_accounting_disposition_v4_verdict";
pub const CI_JOB_ACCOUNTING_DISPOSITION_V4_SECRET_RESOLUTION_MIGRATION_ID: &str =
    "ci_0017d_ci_job_accounting_disposition_v4_secret_resolution";
pub const CI_JOB_QUEUE_COMPLETION_MIGRATION_ID: &str = "ci_0004a_job_queue_completion";
pub const CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID: &str = "ci_0004b_job_queue_claim_authority";
pub const CI_JOB_QUEUE_CLAIM_TIME_MIGRATION_ID: &str = "ci_0004c_job_queue_claim_time";
pub const CI_SCHEDULER_LEASE_EPOCH_GRANT_MIGRATION_ID: &str =
    "ci_0016a_scheduler_lease_epoch_grant";
pub const CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID: &str =
    "ci_0016b_scheduler_claim_nonce_grant";
pub const CI_SCHEDULER_CLAIM_TIME_GRANT_MIGRATION_ID: &str = "ci_0016c_scheduler_claim_time_grant";
pub const CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID: &str = "ci_0018_ci_run_queued_region";
pub const CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID: &str = "ci_0018a_scheduler_ci_run_discovery";
pub const CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID: &str = "ci_0018b_ci_workflow_active_region";
pub const CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID: &str =
    "ci_0018c_scheduler_ci_workflow_discovery";
pub const CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID: &str =
    "ci_0018d_ci_run_surface_repo_created";
pub const CI_JOB_QUEUE_RETRY_ATTEMPTS_MIGRATION_ID: &str = "ci_0018e_job_queue_retry_attempts";
pub const CI_RUN_ACTIVE_WORKFLOW_INDEX_MIGRATION_ID: &str = "ci_0018f_ci_run_active_workflow";
pub const CI_SCHEDULER_CI_RUN_WORKFLOW_ID_GRANT_MIGRATION_ID: &str =
    "ci_0018g_scheduler_ci_run_workflow_id_grant";
pub const CI_SCHEDULER_CI_JOB_REAP_RESET_GRANT_MIGRATION_ID: &str =
    "ci_0018h_scheduler_ci_job_reap_reset_grant";
pub const CI_JOB_PARENT_ATTEMPT_MIGRATION_ID: &str = "ci_0019_ci_job_parent_attempt";
pub const CI_JOB_PRELAUNCH_USAGE_MIGRATION_ID: &str = "ci_0020_ci_job_prelaunch_usage";
pub const CI_JOB_PRELAUNCH_USAGE_REAPER_INDEX_MIGRATION_ID: &str =
    "ci_0020a_ci_job_prelaunch_usage_reaper";
pub const CI_SCHEDULER_PRELAUNCH_USAGE_REAP_GRANT_MIGRATION_ID: &str =
    "ci_0020b_scheduler_prelaunch_usage_reap_grant";
pub const CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_MIGRATION_ID: &str =
    "ci_0020c_ci_job_prelaunch_usage_seal_deadline";
pub const CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_MIGRATION_ID: &str =
    "ci_0020d_ci_job_prelaunch_usage_seal_deadline_reaper";
pub const CI_JOB_QUEUE_CLAIM_WINDOW_MIGRATION_ID: &str = "ci_0020e_job_queue_claim_window";
pub const CI_JOB_QUEUE_CLAIM_WINDOW_VALIDATE_MIGRATION_ID: &str =
    "ci_0020f_job_queue_claim_window_validate";
pub const CI_SCHEDULER_WORKFLOW_VERSION_GRANT_MIGRATION_ID: &str =
    "ci_0020g_scheduler_workflow_version_grant";
pub const CI_PIPELINE_VERSION_BACKLOG_PROBE_MIGRATION_ID: &str =
    "ci_0020h_ci_pipeline_version_backlog_probe";
pub const CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID: &str =
    "ci_0020i_ci_pipeline_cutover_fence_row";
pub const CI_JOB_CREDENTIAL_GENERATION_MIGRATION_ID: &str = "ci_0021_ci_job_credential_generation";
pub const CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_MIGRATION_ID: &str =
    "ci_0022_job_queue_reservation_write_version";
pub const CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_VALIDATE_MIGRATION_ID: &str =
    "ci_0022a_job_queue_reservation_write_version_validate";
pub const CI_JOB_QUEUE_ACTIVATION_READINESS_INDEX_MIGRATION_ID: &str =
    "ci_0022b_job_queue_activation_readiness_index";
pub const CI_V2_ACTIVATION_READINESS_PROBE_MIGRATION_ID: &str =
    "ci_0022c_ci_v2_activation_readiness_probe";
pub const CI_PIPELINE_V3_CUTOVER_FENCE_ROW_MIGRATION_ID: &str =
    "ci_0022d_ci_pipeline_v3_cutover_fence_row";
pub const CI_PIPELINE_V4_CUTOVER_FENCE_ROW_MIGRATION_ID: &str =
    "ci_0026_ci_pipeline_v4_cutover_fence_row";
pub const CI_PIPELINE_V5_CUTOVER_FENCE_ROW_MIGRATION_ID: &str =
    "ci_0027_ci_pipeline_v5_cutover_fence_row";
pub const CI_PIPELINE_V6_CUTOVER_FENCE_ROW_MIGRATION_ID: &str =
    "ci_0028_ci_pipeline_v6_cutover_fence_row";

pub const CREATE_CI_RUN_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_run (
  tenant_id           text NOT NULL,
  region              text NOT NULL,
  run_id              uuid NOT NULL,
  project_id          uuid NOT NULL,
  repo_ref            text,
  commit_oid          text,
  pipeline_id         uuid NOT NULL,
  wf_run_id           uuid NOT NULL,
  cause_event_id      text,
  definition_snapshot text NOT NULL,
  trigger_kind        text NOT NULL CHECK (trigger_kind IN ('push','pull_request','issue_transition','manual','agent','schedule')),
  triggered_by        text,
  trust_tier          text NOT NULL CHECK (trust_tier IN ('trusted','untrusted_fork','self_hosted')),
  state               text NOT NULL CHECK (state IN ('queued','running','succeeded','failed','cancelled','timed_out','reaped')),
  cost_settled        boolean NOT NULL DEFAULT false,
  correlation_id      text NOT NULL,
  created_at          timestamptz NOT NULL DEFAULT now(),
  finished_at         timestamptz,
  PRIMARY KEY (tenant_id, run_id)
)";

pub const ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL: &str = "ALTER TABLE ci_run \
ADD COLUMN IF NOT EXISTS cause_depth bigint NOT NULL DEFAULT 0 \
CHECK (cause_depth BETWEEN 0 AND 4294967295), \
ADD COLUMN IF NOT EXISTS caused_by text";

pub const ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL: &str = "ALTER TABLE ci_run \
ADD COLUMN concurrency_group text \
CHECK (concurrency_group IS NULL OR (\
trigger_kind = 'pull_request' \
AND concurrency_group ~ '^pr:[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*:[1-9][0-9]*$' \
AND octet_length(concurrency_group) BETWEEN 4 AND 512 \
AND concurrency_group !~ '[[:cntrl:]]'))";

pub const ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL: &str = "ALTER TABLE ci_run \
ADD COLUMN pr_head_generation bigint \
CHECK (pr_head_generation IS NULL OR (\
trigger_kind = 'pull_request' AND pr_head_generation > 0))";

pub const ALTER_CI_RUN_ADD_SOURCE_REF_DDL: &str = "ALTER TABLE ci_run \
ADD COLUMN IF NOT EXISTS source_ref text \
CHECK (source_ref IS NULL OR (\
trigger_kind = 'push' \
AND source_ref ~ '^refs/heads/[A-Za-z0-9][A-Za-z0-9._/-]*$' \
AND octet_length(source_ref) BETWEEN 12 AND 1024 \
AND source_ref !~ '(^|/)\\.\\.?(/|$)' \
AND source_ref !~ '[[:cntrl:] ~^:?*\\[]'))";

pub const REPAIR_CI_RUN_SOURCE_REF_CONSTRAINT_DDL: &str = r#"DO $myelin$
DECLARE
  expected_definition constant text :=
    'CHECK (((source_ref IS NULL) OR ((trigger_kind = ''push''::text) AND (source_ref ~ ''^refs/heads/[A-Za-z0-9][A-Za-z0-9._/-]*$''::text) AND ((octet_length(source_ref) >= 12) AND (octet_length(source_ref) <= 1024)) AND (source_ref !~ ''(^|/)\.\.?(/|$)''::text) AND (source_ref !~ ''[[:cntrl:] ~^:?*\[]''::text))))';
  existing_definition text;
  equivalent_constraint name;
BEGIN
  SELECT pg_catalog.pg_get_constraintdef(constraint_catalog.oid)
    INTO existing_definition
    FROM pg_catalog.pg_constraint AS constraint_catalog
   WHERE constraint_catalog.conrelid = 'ci_run'::regclass
     AND constraint_catalog.conname = 'ci_run_source_ref_shape';

  IF existing_definition IS NOT NULL THEN
    IF existing_definition IS DISTINCT FROM expected_definition
       AND existing_definition IS DISTINCT FROM expected_definition || ' NOT VALID' THEN
      RAISE EXCEPTION
        'ci_run_source_ref_shape already exists with a DIVERGENT definition: % (expected: %)',
        existing_definition, expected_definition;
    END IF;
  ELSE
    SELECT constraint_catalog.conname
      INTO equivalent_constraint
      FROM pg_catalog.pg_constraint AS constraint_catalog
     WHERE constraint_catalog.conrelid = 'ci_run'::regclass
       AND constraint_catalog.contype = 'c'
       AND pg_catalog.pg_get_constraintdef(constraint_catalog.oid) IN (
         expected_definition,
         expected_definition || ' NOT VALID'
       )
     ORDER BY constraint_catalog.convalidated DESC, constraint_catalog.oid
     LIMIT 1;

    IF equivalent_constraint IS NOT NULL THEN
      EXECUTE format(
        'ALTER TABLE ci_run RENAME CONSTRAINT %I TO ci_run_source_ref_shape',
        equivalent_constraint
      );
    ELSE
      ALTER TABLE ci_run
        ADD CONSTRAINT ci_run_source_ref_shape
        CHECK (source_ref IS NULL OR (
          trigger_kind = 'push'
          AND source_ref ~ '^refs/heads/[A-Za-z0-9][A-Za-z0-9._/-]*$'
          AND octet_length(source_ref) BETWEEN 12 AND 1024
          AND source_ref !~ '(^|/)\.\.?(/|$)'
          AND source_ref !~ '[[:cntrl:] ~^:?*\[]'
        )) NOT VALID;
    END IF;
  END IF;
END
$myelin$"#;

pub const VALIDATE_CI_RUN_SOURCE_REF_CONSTRAINT_DDL: &str =
    "ALTER TABLE ci_run VALIDATE CONSTRAINT ci_run_source_ref_shape";

pub const EXPAND_CI_RUN_BRANCH_SCOPE_DDL: &str = "ALTER TABLE ci_run \
ADD CONSTRAINT ci_run_source_ref_shape_v2 \
CHECK (source_ref IS NULL OR (\
trigger_kind IN ('push', 'pull_request') \
AND source_ref ~ '^refs/heads/[A-Za-z0-9][A-Za-z0-9._/-]*$' \
AND octet_length(source_ref) BETWEEN 12 AND 1024 \
AND source_ref !~ '(^|/)\\.\\.?(/|$)' \
AND source_ref !~ '[[:cntrl:] ~^:?*\\[]')) NOT VALID";

pub const VALIDATE_CI_RUN_BRANCH_SCOPE_DDL: &str =
    "ALTER TABLE ci_run VALIDATE CONSTRAINT ci_run_source_ref_shape_v2";

pub const CONTRACT_CI_RUN_BRANCH_SCOPE_DDL: &str = "ALTER TABLE ci_run \
DROP CONSTRAINT ci_run_source_ref_shape; \
ALTER TABLE ci_run \
RENAME CONSTRAINT ci_run_source_ref_shape_v2 TO ci_run_source_ref_shape";

pub const CREATE_CI_DRIVE_MANIFEST_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_drive_manifest (
  tenant_id          text NOT NULL,
  region             text NOT NULL,
  wf_run_id          uuid NOT NULL,
  ci_run_id          uuid NOT NULL,
  schema_version     integer NOT NULL CHECK (schema_version = 1),
  source_snapshot_ref text NOT NULL,
  manifest_digest    text NOT NULL CHECK (manifest_digest ~ '^blake3:[0-9a-f]{64}$'),
  manifest_bytes     bytea NOT NULL,
  created_at         timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, wf_run_id),
  UNIQUE (tenant_id, ci_run_id),
  FOREIGN KEY (tenant_id, ci_run_id) REFERENCES ci_run(tenant_id, run_id)
);
REVOKE UPDATE, DELETE ON ci_drive_manifest FROM myelin_app;
CREATE OR REPLACE FUNCTION myelin_reject_ci_drive_manifest_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  RAISE EXCEPTION 'ci_drive_manifest is immutable';
END
$myelin$;
CREATE TRIGGER ci_drive_manifest_reject_mutation
BEFORE UPDATE OR DELETE ON ci_drive_manifest
FOR EACH ROW EXECUTE FUNCTION myelin_reject_ci_drive_manifest_mutation()";

pub const CREATE_CI_JOB_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_job (
  tenant_id      text NOT NULL,
  region         text NOT NULL,
  job_id         uuid NOT NULL,
  run_id         uuid NOT NULL,
  stage          text NOT NULL,
  name           text NOT NULL,
  needs          uuid[] NOT NULL DEFAULT '{}',
  matrix_key     jsonb,
  spec_ref       text NOT NULL,
  state          text NOT NULL CHECK (state IN ('queued','leased','running','succeeded','failed','cancelled','reaped')),
  attempt        integer NOT NULL DEFAULT 1,
  result_summary jsonb,
  PRIMARY KEY (tenant_id, job_id),
  FOREIGN KEY (tenant_id, run_id) REFERENCES ci_run(tenant_id, run_id)
)";

pub const CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_job_run_ledger ON ci_job (tenant_id, region, run_id)";

pub const VALIDATE_CI_JOB_RUN_LEDGER_INDEX_DDL: &str = "\
DO $myelin$
BEGIN
  IF NOT EXISTS (
    SELECT 1
      FROM pg_catalog.pg_index AS index_state
      JOIN pg_catalog.pg_class AS index_relation
        ON index_relation.oid = index_state.indexrelid
      JOIN pg_catalog.pg_class AS table_relation
        ON table_relation.oid = index_state.indrelid
      JOIN pg_catalog.pg_namespace AS relation_namespace
        ON relation_namespace.oid = table_relation.relnamespace
     WHERE relation_namespace.nspname = current_schema()
       AND table_relation.relname = 'ci_job'
       AND table_relation.relkind = 'r'
       AND index_relation.relnamespace = relation_namespace.oid
       AND index_relation.relname = 'ci_job_run_ledger'
       AND index_relation.relkind = 'i'
       AND index_state.indisvalid
       AND index_state.indisready
  ) THEN
    RAISE EXCEPTION 'ci_job_run_ledger on %.ci_job is missing, invalid, or not ready; verify the index exists, then repair it with REINDEX INDEX CONCURRENTLY %.ci_job_run_ledger before restarting', current_schema(), current_schema();
  END IF;
END
$myelin$";

pub const CREATE_CHECK_ATTEMPT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS check_attempt (
  tenant_id    text NOT NULL,
  region       text NOT NULL,
  repo_ref     text NOT NULL,
  commit_oid   text NOT NULL,
  context      text NOT NULL,
  next_attempt integer NOT NULL DEFAULT 1,
  current_run  uuid,
  PRIMARY KEY (tenant_id, repo_ref, commit_oid, context)
)";

pub const CREATE_CI_RUN_CHECK_ATTEMPT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_run_check_attempt (
  tenant_id    text NOT NULL,
  region       text NOT NULL,
  run_id       uuid NOT NULL,
  repo_ref     text NOT NULL,
  commit_oid   text NOT NULL,
  context      text NOT NULL,
  run_attempt  integer NOT NULL CHECK (run_attempt > 0),
  PRIMARY KEY (tenant_id, run_id, context),
  UNIQUE (tenant_id, repo_ref, commit_oid, context, run_attempt),
  FOREIGN KEY (tenant_id, run_id) REFERENCES ci_run(tenant_id, run_id) ON DELETE CASCADE
);
REVOKE UPDATE, DELETE ON ci_run_check_attempt FROM myelin_app";

pub const CREATE_JOB_QUEUE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS job_queue (
  tenant_id         text NOT NULL,
  region            text NOT NULL,
  job_id            uuid NOT NULL,
  run_id            uuid NOT NULL,
  lane              text NOT NULL CHECK (lane IN ('interactive','batch','deploy')),
  labels            text[] NOT NULL DEFAULT '{}',
  trust_tier        text NOT NULL CHECK (trust_tier IN ('trusted','untrusted_fork','self_hosted')),
  concurrency_group text,
  fair_key          text NOT NULL,
  idem_token        text NOT NULL,
  enqueued_at       timestamptz NOT NULL DEFAULT now(),
  lease_owner       text,
  lease_expires     timestamptz,
  state             text NOT NULL CHECK (state IN ('queued','leased','running','terminal')),
  PRIMARY KEY (tenant_id, job_id)
)";

pub const CREATE_JOB_QUEUE_INDEXES_DDL: &[(&str, &str)] = &[
    (
        JQ_CLAIMABLE_INDEX,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS jq_claimable ON job_queue (region, lane, enqueued_at) WHERE state = 'queued'",
    ),
    (
        JQ_SERIALIZE_INDEX,
        "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS jq_serialize ON job_queue (tenant_id, concurrency_group) WHERE state = 'running' AND concurrency_group LIKE 'deploy:%'",
    ),
    (
        JQ_IDEM_INDEX,
        "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS jq_idem ON job_queue (tenant_id, idem_token)",
    ),
];

pub const CREATE_FAIR_DEFICIT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS fair_deficit (
  tenant_id   text NOT NULL,
  region      text NOT NULL,
  fair_key    text NOT NULL,
  deficit     bigint NOT NULL DEFAULT 0,
  last_served timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, region, fair_key)
)";

pub const CREATE_RUNNER_DDL: &str = "\
CREATE TABLE IF NOT EXISTS runner (
  tenant_id      text NOT NULL,
  region         text NOT NULL,
  runner_id      uuid NOT NULL,
  pool           text NOT NULL,
  labels         text[] NOT NULL DEFAULT '{}',
  ownership      text NOT NULL CHECK (ownership IN ('hosted','self_hosted')),
  trust_tier     text NOT NULL CHECK (trust_tier IN ('trusted','untrusted_fork','self_hosted')),
  attestation    jsonb,
  attest_state   text NOT NULL CHECK (attest_state IN ('pending','attested','failed')),
  health         text NOT NULL CHECK (health IN ('healthy','degraded','offline')),
  capacity       jsonb NOT NULL,
  last_heartbeat timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, runner_id)
)";

pub const CREATE_LOG_SEGMENT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS log_segment (
  tenant_id   text NOT NULL,
  region      text NOT NULL,
  run_id      uuid NOT NULL,
  job_id      uuid NOT NULL,
  segment_seq integer NOT NULL,
  blob_ref    text,
  byte_start  bigint NOT NULL,
  byte_end    bigint NOT NULL,
  pii_key_ref text NOT NULL,
  PRIMARY KEY (tenant_id, run_id, job_id, segment_seq)
)";

pub const CREATE_LOG_ANCHOR_DDL: &str = "\
CREATE TABLE IF NOT EXISTS log_anchor (
  tenant_id  text NOT NULL,
  region     text NOT NULL,
  run_id     uuid NOT NULL,
  job_id     uuid NOT NULL,
  step_id    text NOT NULL,
  byte_start bigint NOT NULL,
  byte_end   bigint,
  status     text NOT NULL CHECK (status IN ('running','passed','failed','skipped')),
  PRIMARY KEY (tenant_id, run_id, job_id, step_id)
)";

pub const CREATE_ARTIFACT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS artifact (
  tenant_id    text NOT NULL,
  region       text NOT NULL,
  artifact_id  uuid NOT NULL,
  run_id       uuid NOT NULL,
  name         text NOT NULL,
  blob_ref     text NOT NULL,
  size_bytes   bigint NOT NULL,
  provenance   jsonb,
  pii_key_ref  text NOT NULL,
  retain_until timestamptz NOT NULL,
  PRIMARY KEY (tenant_id, artifact_id)
)";

pub const CREATE_CACHE_ENTRY_DDL: &str = "\
CREATE TABLE IF NOT EXISTS cache_entry (
  tenant_id text NOT NULL,
  region    text NOT NULL,
  run_id    uuid NOT NULL,
  cache_key text NOT NULL,
  scope     text NOT NULL,
  blob_ref  text NOT NULL,
  last_used timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, scope, cache_key)
)";

pub const CREATE_ENVIRONMENT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS environment (
  tenant_id  text NOT NULL,
  region     text NOT NULL,
  env_id     uuid NOT NULL,
  project_id uuid NOT NULL,
  name       text NOT NULL,
  protected  boolean NOT NULL DEFAULT false,
  PRIMARY KEY (tenant_id, env_id)
)";

pub const CREATE_DEPLOYMENT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS deployment (
  tenant_id   text NOT NULL,
  region      text NOT NULL,
  dep_id      uuid NOT NULL,
  env_id      uuid NOT NULL,
  run_id      uuid NOT NULL,
  version     text NOT NULL,
  state       text NOT NULL CHECK (state IN ('awaiting_approval','deploying','deployed','failed','rolled_back')),
  approved_by text,
  PRIMARY KEY (tenant_id, dep_id)
)";

pub const CREATE_SECRET_BINDING_DDL: &str = "\
CREATE TABLE IF NOT EXISTS secret_binding (
  tenant_id  text NOT NULL,
  region     text NOT NULL,
  project_id uuid NOT NULL,
  name       text NOT NULL,
  scope      text NOT NULL,
  value_ref  text NOT NULL,
  PRIMARY KEY (tenant_id, project_id, name, scope)
)";

pub const CREATE_CI_SECRET_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_secret (
  tenant_id  text        NOT NULL,
  region     text        NOT NULL,
  secret_id  text        NOT NULL CHECK (secret_id ~ '^[A-Za-z0-9_-]{1,128}$'),
  name       text        NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 128),
  pii_key_ref text       NOT NULL,
  nonce      bytea       NOT NULL CHECK (octet_length(nonce) = 12),
  ciphertext bytea       NOT NULL CHECK (octet_length(ciphertext) >= 16),
  version    bigint      NOT NULL DEFAULT 1 CHECK (version > 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, secret_id)
)";

pub const ALTER_CI_SECRET_ADD_ADMIN_SCOPE_DDL: &str =
    "ALTER TABLE ci_secret ADD COLUMN IF NOT EXISTS project_id uuid";
pub const CREATE_CI_SECRET_ADMIN_UNIQUE_INDEX_DDL: &str = "\
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS ux_ci_secret_tenant_project_name
  ON ci_secret (tenant_id, project_id, name) WHERE project_id IS NOT NULL";

pub const ALTER_SECRET_BINDING_ADD_INTEGRITY_DDL: &str = "\
ALTER TABLE secret_binding ADD COLUMN IF NOT EXISTS secret_id text;
UPDATE secret_binding AS binding
   SET secret_id = secret.secret_id
  FROM ci_secret AS secret
 WHERE binding.secret_id IS NULL
   AND binding.tenant_id = secret.tenant_id
   AND binding.region = secret.region
   AND binding.value_ref = 'myelin://' || secret.tenant_id || '/ci/secret/' || secret.secret_id;
DELETE FROM secret_binding WHERE secret_id IS NULL;
ALTER TABLE secret_binding ALTER COLUMN secret_id SET NOT NULL;
ALTER TABLE secret_binding
  ADD CONSTRAINT fk_secret_binding_ci_secret
  FOREIGN KEY (tenant_id, secret_id)
  REFERENCES ci_secret (tenant_id, secret_id)
  ON DELETE CASCADE";

pub const CREATE_CI_SECRET_TOMBSTONE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_secret_tombstone (
  tenant_id  text   NOT NULL,
  region     text   NOT NULL,
  project_id uuid   NOT NULL,
  secret_id  text   NOT NULL CHECK (secret_id ~ '^[A-Za-z0-9_-]{1,128}$'),
  max_version bigint NOT NULL CHECK (max_version > 0),
  deleted_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, project_id, secret_id)
)";

pub const CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_secret_version_high_water (
  tenant_id   text        NOT NULL,
  region      text        NOT NULL,
  secret_id   text        NOT NULL CHECK (secret_id ~ '^[A-Za-z0-9_-]{1,128}$'),
  max_version bigint      NOT NULL CHECK (max_version > 0),
  updated_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, secret_id)
);
INSERT INTO ci_secret_version_high_water (tenant_id, region, secret_id, max_version)
SELECT tenant_id,
       (array_agg(region ORDER BY version DESC, region))[1],
       secret_id,
       max(version)
  FROM (
    SELECT tenant_id, region, secret_id, version FROM ci_secret
    UNION ALL
    SELECT tenant_id, region, secret_id, max_version AS version FROM ci_secret_tombstone
  ) AS history
 GROUP BY tenant_id, secret_id
ON CONFLICT (tenant_id, secret_id) DO UPDATE SET
  region = EXCLUDED.region,
  max_version = GREATEST(ci_secret_version_high_water.max_version, EXCLUDED.max_version),
  updated_at = now()";

pub const CREATE_CI_COST_EVENT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_cost_event (
  tenant_id             text NOT NULL,
  region                text NOT NULL,
  cost_id               uuid NOT NULL,
  run_id                uuid NOT NULL,
  job_id                uuid NOT NULL,
  meter                 text NOT NULL CHECK (meter IN ('cpu_seconds','mem_gb_seconds','gpu_seconds','storage_gb_hours','egress_gb')),
  amount                bigint NOT NULL,
  wholesale_minor_units bigint NOT NULL,
  markup_minor_units    bigint NOT NULL,
  kind                  text NOT NULL CHECK (kind IN ('ci','agent')),
  PRIMARY KEY (tenant_id, cost_id)
)";

pub const CREATE_CI_JOB_SPEC_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_job_spec (
  tenant_id  text  NOT NULL,
  region     text  NOT NULL,
  job_id     uuid  NOT NULL,
  run_id     uuid  NOT NULL,
  idem_token text  NOT NULL,
  spec       jsonb NOT NULL,
  PRIMARY KEY (tenant_id, job_id)
)";

pub const CREATE_CI_JOB_ACCOUNTING_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_job_accounting (
  tenant_id             text NOT NULL,
  region                text NOT NULL,
  job_id                uuid NOT NULL,
  wf_run_id             uuid NOT NULL,
  ci_run_id             uuid NOT NULL,
  reserve_handle        text NOT NULL,
  passed                boolean NOT NULL,
  timed_out             boolean NOT NULL,
  cpu_seconds           bigint NOT NULL CHECK (cpu_seconds >= 0),
  mem_byte_seconds      bigint NOT NULL CHECK (mem_byte_seconds >= 0),
  pricing_revision      text NOT NULL,
  billed_minor_units    bigint NOT NULL CHECK (billed_minor_units >= 0),
  refunded_minor_units  bigint NOT NULL CHECK (refunded_minor_units >= 0),
  completion_receipt    text NOT NULL CHECK (completion_receipt ~ '^v3:[0-9a-f]{64}$'),
  accounted_at          timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, job_id),
  UNIQUE (tenant_id, completion_receipt),
  FOREIGN KEY (tenant_id, ci_run_id) REFERENCES ci_run(tenant_id, run_id),
  CHECK (NOT (passed AND timed_out))
);
REVOKE UPDATE, DELETE ON ci_job_accounting FROM myelin_app;
CREATE OR REPLACE FUNCTION myelin_reject_ci_job_accounting_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  RAISE EXCEPTION 'ci_job_accounting is immutable';
END
$myelin$;
CREATE TRIGGER ci_job_accounting_reject_mutation
BEFORE UPDATE OR DELETE ON ci_job_accounting
FOR EACH ROW EXECUTE FUNCTION myelin_reject_ci_job_accounting_mutation()";

pub const ALTER_CI_JOB_SPEC_ADD_STAGE_DDL: &str =
    "ALTER TABLE ci_job_spec ADD COLUMN IF NOT EXISTS stage text";

pub const ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL: &str = "ALTER TABLE ci_job_accounting \
ADD COLUMN IF NOT EXISTS skipped boolean NOT NULL DEFAULT false, \
ADD CONSTRAINT ci_job_accounting_skipped_verdict \
CHECK (NOT skipped OR (NOT passed AND NOT timed_out))";

pub const ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_DDL: &str = "\
ALTER TABLE ci_job_accounting
  ADD COLUMN IF NOT EXISTS terminal_disposition text,
  ADD COLUMN IF NOT EXISTS completion_receipt_v4 text,
  ADD CONSTRAINT ci_job_accounting_terminal_disposition_v4
    CHECK (
      (terminal_disposition IS NULL AND completion_receipt_v4 IS NULL)
      OR (
        terminal_disposition IN (
          'workload_passed',
          'workload_failed',
          'workload_timed_out',
          'checkout_transport_failed',
          'checkout_transport_timed_out',
          'checkout_materialization_failed',
          'checkout_materialization_timed_out',
          'preparation_attempts_exhausted',
          'skipped_before_start',
          'cancelled_during_preparation',
          'cancelled_after_workload_launch'
        )
        AND completion_receipt_v4 ~ '^v4:[0-9a-f]{64}$'
      )
    ),
  ADD CONSTRAINT ci_job_accounting_completion_receipt_v4_unique
    UNIQUE (tenant_id, completion_receipt_v4)";

pub const ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_VERDICT_DDL: &str = "\
ALTER TABLE ci_job_accounting
  ADD CONSTRAINT ci_job_accounting_terminal_disposition_v4_verdict
    CHECK (
      terminal_disposition IS NULL
      OR CASE terminal_disposition
        WHEN 'workload_passed' THEN passed AND NOT timed_out AND NOT skipped
        WHEN 'workload_timed_out' THEN NOT passed AND timed_out AND NOT skipped
        WHEN 'checkout_transport_timed_out' THEN NOT passed AND timed_out AND NOT skipped
        WHEN 'checkout_materialization_timed_out' THEN NOT passed AND timed_out AND NOT skipped
        WHEN 'skipped_before_start' THEN NOT passed AND NOT timed_out AND skipped
        WHEN 'cancelled_during_preparation' THEN NOT passed AND NOT timed_out AND skipped
        ELSE NOT passed AND NOT timed_out AND NOT skipped
      END
    )";

pub const ALTER_CI_JOB_ACCOUNTING_DISPOSITION_V4_SECRET_RESOLUTION_DDL: &str = "\
ALTER TABLE ci_job_accounting
  DROP CONSTRAINT ci_job_accounting_terminal_disposition_v4,
  ADD CONSTRAINT ci_job_accounting_terminal_disposition_v4
    CHECK (
      (terminal_disposition IS NULL AND completion_receipt_v4 IS NULL)
      OR (
        terminal_disposition IN (
          'workload_passed',
          'workload_failed',
          'workload_timed_out',
          'secret_resolution_failed',
          'secret_resolution_timed_out',
          'checkout_transport_failed',
          'checkout_transport_timed_out',
          'checkout_materialization_failed',
          'checkout_materialization_timed_out',
          'preparation_attempts_exhausted',
          'skipped_before_start',
          'cancelled_during_preparation',
          'cancelled_after_workload_launch'
        )
        AND completion_receipt_v4 ~ '^v4:[0-9a-f]{64}$'
      )
    ),
  DROP CONSTRAINT ci_job_accounting_terminal_disposition_v4_verdict,
  ADD CONSTRAINT ci_job_accounting_terminal_disposition_v4_verdict
    CHECK (
      terminal_disposition IS NULL
      OR CASE terminal_disposition
        WHEN 'workload_passed' THEN passed AND NOT timed_out AND NOT skipped
        WHEN 'workload_timed_out' THEN NOT passed AND timed_out AND NOT skipped
        WHEN 'secret_resolution_timed_out' THEN NOT passed AND timed_out AND NOT skipped
        WHEN 'checkout_transport_timed_out' THEN NOT passed AND timed_out AND NOT skipped
        WHEN 'checkout_materialization_timed_out' THEN NOT passed AND timed_out AND NOT skipped
        WHEN 'skipped_before_start' THEN NOT passed AND NOT timed_out AND skipped
        WHEN 'cancelled_during_preparation' THEN NOT passed AND NOT timed_out AND skipped
        ELSE NOT passed AND NOT timed_out AND NOT skipped
      END
    )";

pub const CREATE_CI_JOB_PARENT_ATTEMPT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_job_parent_attempt (
  tenant_id                    text NOT NULL,
  region                       text NOT NULL,
  job_id                       uuid NOT NULL,
  wf_run_id                    uuid NOT NULL,
  ci_run_id                    uuid NOT NULL,
  reserve_handle               text NOT NULL,
  lease_owner                  text NOT NULL,
  lease_epoch                  bigint NOT NULL CHECK (lease_epoch > 0),
  claim_nonce                  uuid NOT NULL,
  claim_started_at_epoch_secs  bigint NOT NULL,
  claim_expires_at_epoch_secs  bigint NOT NULL
    CHECK (claim_expires_at_epoch_secs > claim_started_at_epoch_secs),
  budget_revision              smallint NOT NULL,
  max_parent_attempts          bigint NOT NULL CHECK (max_parent_attempts BETWEEN 1 AND 4294967295),
  begun_at                     timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, region, job_id, lease_epoch, claim_nonce),
  UNIQUE (tenant_id, region, job_id, lease_epoch),
  UNIQUE (tenant_id, region, job_id, claim_nonce),
  FOREIGN KEY (tenant_id, ci_run_id) REFERENCES ci_run(tenant_id, run_id)
);
REVOKE UPDATE, DELETE ON ci_job_parent_attempt FROM myelin_app;
CREATE OR REPLACE FUNCTION myelin_reject_ci_job_parent_attempt_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  RAISE EXCEPTION 'ci_job_parent_attempt is immutable';
END
$myelin$;
CREATE TRIGGER ci_job_parent_attempt_reject_mutation
BEFORE UPDATE OR DELETE ON ci_job_parent_attempt
FOR EACH ROW EXECUTE FUNCTION myelin_reject_ci_job_parent_attempt_mutation()";

pub const CREATE_CI_JOB_PRELAUNCH_USAGE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_job_prelaunch_usage (
  tenant_id                 text NOT NULL,
  region                    text NOT NULL,
  job_id                    uuid NOT NULL,
  lease_epoch               bigint NOT NULL,
  claim_nonce               uuid NOT NULL,
  phase                     text NOT NULL CHECK (phase IN ('checkout_transport','checkout_materialization')),
  status                    text NOT NULL CHECK (status IN ('started','measured','sealed_ceiling')),
  ceiling_cpu_seconds       numeric(20,0) NOT NULL
    CHECK (ceiling_cpu_seconds BETWEEN 0 AND 18446744073709551615),
  ceiling_mem_byte_seconds  numeric(20,0) NOT NULL
    CHECK (ceiling_mem_byte_seconds BETWEEN 0 AND 18446744073709551615),
  exact_cpu_seconds         numeric(20,0)
    CHECK (exact_cpu_seconds BETWEEN 0 AND 18446744073709551615),
  exact_mem_byte_seconds    numeric(20,0)
    CHECK (exact_mem_byte_seconds BETWEEN 0 AND 18446744073709551615),
  started_at                timestamptz NOT NULL DEFAULT now(),
  resolved_at               timestamptz,
  PRIMARY KEY (tenant_id, region, job_id, lease_epoch, claim_nonce, phase),
  CHECK (
    (status = 'started' AND exact_cpu_seconds IS NULL AND exact_mem_byte_seconds IS NULL AND resolved_at IS NULL)
    OR (status = 'measured' AND exact_cpu_seconds IS NOT NULL AND exact_mem_byte_seconds IS NOT NULL
        AND resolved_at IS NOT NULL AND resolved_at >= started_at)
    OR (status = 'sealed_ceiling' AND exact_cpu_seconds IS NULL AND exact_mem_byte_seconds IS NULL
        AND resolved_at IS NOT NULL AND resolved_at >= started_at)
  ),
  FOREIGN KEY (tenant_id, region, job_id, lease_epoch, claim_nonce)
    REFERENCES ci_job_parent_attempt (tenant_id, region, job_id, lease_epoch, claim_nonce)
);
REVOKE DELETE ON ci_job_prelaunch_usage FROM myelin_app;
CREATE OR REPLACE FUNCTION myelin_guard_ci_job_prelaunch_usage_transition()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  IF OLD.status <> 'started' THEN
    RAISE EXCEPTION 'ci_job_prelaunch_usage phase % is already resolved (%)', OLD.phase, OLD.status;
  END IF;
  IF NEW.status = 'started' THEN
    RAISE EXCEPTION 'ci_job_prelaunch_usage phase % cannot revert to started', OLD.phase;
  END IF;
  IF NEW.tenant_id <> OLD.tenant_id OR NEW.region <> OLD.region OR NEW.job_id <> OLD.job_id
     OR NEW.lease_epoch <> OLD.lease_epoch OR NEW.claim_nonce <> OLD.claim_nonce OR NEW.phase <> OLD.phase
     OR NEW.ceiling_cpu_seconds <> OLD.ceiling_cpu_seconds
     OR NEW.ceiling_mem_byte_seconds <> OLD.ceiling_mem_byte_seconds
     OR NEW.started_at <> OLD.started_at THEN
    RAISE EXCEPTION 'ci_job_prelaunch_usage identity and ceiling are immutable';
  END IF;
  RETURN NEW;
END
$myelin$;
CREATE TRIGGER ci_job_prelaunch_usage_guard_transition
BEFORE UPDATE ON ci_job_prelaunch_usage
FOR EACH ROW EXECUTE FUNCTION myelin_guard_ci_job_prelaunch_usage_transition()";

pub const CREATE_CI_JOB_CREDENTIAL_GENERATION_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_job_credential_generation (
  tenant_id                    text NOT NULL,
  region                       text NOT NULL,
  job_id                       uuid NOT NULL,
  wf_run_id                    uuid NOT NULL,
  ci_run_id                    uuid NOT NULL,
  token_authority_handle       text NOT NULL,
  idem_token                   text NOT NULL,
  lease_owner                  text NOT NULL,
  lease_epoch                  bigint NOT NULL CHECK (lease_epoch > 0),
  claim_nonce                  uuid NOT NULL,
  claim_started_at_epoch_secs  bigint NOT NULL CHECK (claim_started_at_epoch_secs > 0),
  claim_expires_at_epoch_secs  bigint NOT NULL
    CHECK (claim_expires_at_epoch_secs > claim_started_at_epoch_secs),
  binding_version              smallint NOT NULL CHECK (binding_version = 1),
  purpose                      text NOT NULL CHECK (purpose IN ('checkout_advertise','checkout_fetch','checkout_materialization','workload')),
  phase_ordinal                smallint NOT NULL CHECK (phase_ordinal BETWEEN 1 AND 4),
  issued_at_epoch_secs         bigint NOT NULL CHECK (issued_at_epoch_secs > 0),
  expires_at_epoch_secs        bigint NOT NULL,
  generation_id                text NOT NULL,
  jti                          text NOT NULL,
  minted_at                    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, region, job_id, lease_epoch, claim_nonce, purpose),
  UNIQUE (tenant_id, region, generation_id),
  UNIQUE (tenant_id, region, jti),
  CHECK (
    CASE purpose
      WHEN 'checkout_advertise' THEN phase_ordinal = 1
      WHEN 'checkout_fetch' THEN phase_ordinal = 2
      WHEN 'checkout_materialization' THEN phase_ordinal = 3
      WHEN 'workload' THEN phase_ordinal = 4
      ELSE false
    END
  ),
  CHECK (expires_at_epoch_secs > issued_at_epoch_secs),
  CHECK (expires_at_epoch_secs <= claim_expires_at_epoch_secs),
  CHECK (issued_at_epoch_secs >= claim_started_at_epoch_secs),
  FOREIGN KEY (tenant_id, ci_run_id) REFERENCES ci_run(tenant_id, run_id)
);
REVOKE UPDATE, DELETE ON ci_job_credential_generation FROM myelin_app;
CREATE OR REPLACE FUNCTION myelin_reject_ci_job_credential_generation_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  RAISE EXCEPTION 'ci_job_credential_generation is immutable';
END
$myelin$;
CREATE TRIGGER ci_job_credential_generation_reject_mutation
BEFORE UPDATE OR DELETE ON ci_job_credential_generation
FOR EACH ROW EXECUTE FUNCTION myelin_reject_ci_job_credential_generation_mutation()";

pub const CREATE_CI_JOB_PRELAUNCH_USAGE_REAPER_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_job_prelaunch_usage_reaper \
ON ci_job_prelaunch_usage (region, started_at) WHERE status = 'started'";

pub const ALTER_CI_JOB_PRELAUNCH_USAGE_ADD_SEAL_DEADLINE_DDL: &str = "\
ALTER TABLE ci_job_prelaunch_usage
  ADD COLUMN IF NOT EXISTS seal_after timestamptz;
ALTER TABLE ci_job_prelaunch_usage
  ADD CONSTRAINT ci_job_prelaunch_usage_seal_after_order
  CHECK (seal_after IS NULL OR seal_after >= started_at) NOT VALID;
ALTER TABLE ci_job_prelaunch_usage
  VALIDATE CONSTRAINT ci_job_prelaunch_usage_seal_after_order;
CREATE OR REPLACE FUNCTION myelin_guard_ci_job_prelaunch_usage_transition()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  IF OLD.status <> 'started' THEN
    RAISE EXCEPTION 'ci_job_prelaunch_usage phase % is already resolved (%)', OLD.phase, OLD.status;
  END IF;
  IF NEW.status = 'started' THEN
    RAISE EXCEPTION 'ci_job_prelaunch_usage phase % cannot revert to started', OLD.phase;
  END IF;
  IF NEW.tenant_id <> OLD.tenant_id OR NEW.region <> OLD.region OR NEW.job_id <> OLD.job_id
     OR NEW.lease_epoch <> OLD.lease_epoch OR NEW.claim_nonce <> OLD.claim_nonce OR NEW.phase <> OLD.phase
     OR NEW.ceiling_cpu_seconds <> OLD.ceiling_cpu_seconds
     OR NEW.ceiling_mem_byte_seconds <> OLD.ceiling_mem_byte_seconds
     OR NEW.started_at <> OLD.started_at
     OR NEW.seal_after IS DISTINCT FROM OLD.seal_after THEN
    RAISE EXCEPTION 'ci_job_prelaunch_usage identity, ceiling, and deadline are immutable';
  END IF;
  RETURN NEW;
END
$myelin$";

pub const CREATE_CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_job_prelaunch_usage_seal_deadline_reaper \
ON ci_job_prelaunch_usage (region, seal_after) WHERE status = 'started' AND seal_after IS NOT NULL";

pub const GRANT_SCHEDULER_CI_JOB_PRELAUNCH_USAGE_REAP_DDL: &str = "\
CREATE POLICY myelin_ci_scheduler_parent_attempt_access ON ci_job_parent_attempt
  AS PERMISSIVE FOR SELECT TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
CREATE POLICY myelin_ci_scheduler_parent_attempt_guard ON ci_job_parent_attempt
  AS RESTRICTIVE FOR SELECT TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
CREATE POLICY myelin_ci_scheduler_prelaunch_usage_access ON ci_job_prelaunch_usage
  AS PERMISSIVE FOR ALL TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  )
  WITH CHECK (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
CREATE POLICY myelin_ci_scheduler_prelaunch_usage_guard ON ci_job_prelaunch_usage
  AS RESTRICTIVE FOR ALL TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  )
  WITH CHECK (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
GRANT SELECT ON ci_job_parent_attempt TO myelin_ci_region_scheduler;
GRANT SELECT ON ci_job_prelaunch_usage TO myelin_ci_region_scheduler;
GRANT UPDATE (status, resolved_at) ON ci_job_prelaunch_usage TO myelin_ci_region_scheduler";

pub const ALTER_JOB_QUEUE_ADD_COMPLETION_DDL: &str = "ALTER TABLE job_queue \
ADD COLUMN IF NOT EXISTS lease_epoch bigint NOT NULL DEFAULT 0, \
ADD COLUMN IF NOT EXISTS completion_receipt text";

pub const ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL: &str = "ALTER TABLE job_queue \
ADD COLUMN IF NOT EXISTS claim_nonce uuid, \
ADD COLUMN IF NOT EXISTS stage text";

pub const ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL: &str = "ALTER TABLE job_queue \
ADD COLUMN IF NOT EXISTS claim_started_at timestamptz, \
ADD COLUMN IF NOT EXISTS claim_expires_at timestamptz";

pub const ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL: &str = "\
ALTER TABLE job_queue ADD COLUMN IF NOT EXISTS claim_window_secs bigint;
DO $myelin$
DECLARE
  expected_definition constant text :=
    'CHECK (((claim_window_secs >= 1) AND (claim_window_secs <= 88800))) NOT VALID';
  existing_definition text;
BEGIN
  ALTER TABLE job_queue
    ADD CONSTRAINT job_queue_claim_window_range
    CHECK (claim_window_secs BETWEEN 1 AND 88800) NOT VALID;
EXCEPTION WHEN duplicate_object THEN
  SELECT pg_catalog.pg_get_constraintdef(constraint_catalog.oid)
    INTO existing_definition
    FROM pg_catalog.pg_constraint AS constraint_catalog
   WHERE constraint_catalog.conrelid = 'job_queue'::regclass
     AND constraint_catalog.conname = 'job_queue_claim_window_range';
  IF existing_definition IS DISTINCT FROM expected_definition
     AND existing_definition IS DISTINCT FROM replace(expected_definition, ' NOT VALID', '') THEN
    RAISE EXCEPTION
      'job_queue_claim_window_range already exists with a DIVERGENT definition: % (expected: %)',
      existing_definition, expected_definition;
  END IF;
END
$myelin$";

pub const VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL: &str = "\
ALTER TABLE job_queue VALIDATE CONSTRAINT job_queue_claim_window_range";

pub const GRANT_SCHEDULER_WORKFLOW_VERSION_DDL: &str =
    "GRANT SELECT (wf_version) ON workflow_run TO myelin_ci_region_scheduler";

pub const CREATE_CI_PIPELINE_VERSION_BACKLOG_PROBE_DDL: &str = "\
-- ATOMICITY: this whole script is ONE transaction, but deliberately the IMPLICIT one PostgreSQL
-- opens around a multi-statement simple query — NOT an explicit `BEGIN`/`COMMIT`. Both are equally
-- atomic (a failure rolls the complete script back), but an explicit `BEGIN` leaves the SESSION in
-- an aborted transaction block when a statement raises, and `PgMigrator` returns that connection to
-- the pool: the next user of it fails with `25P02 current transaction is aborted`, including the
-- migrator's own `pg_advisory_unlock`. The implicit form ends the transaction on error, so a
-- refusal here is loud and local instead of poisoning the pool.

-- (1) PROVISIONING PREFLIGHT. The migration VERIFIES; it never creates or alters a cluster role,
-- even if `current_user` happens to hold CREATEROLE. BYPASSRLS provisioning is an operator-
-- controlled action, and migration behaviour must not vary with accidental excess privilege.
DO $myelin$
DECLARE
  remediation constant text :=
    'run scripts/pg-init/01-ci-definition-fence.sql as the database provisioning administrator, '
    'passing migration_role=<DATABASE_MIGRATION_URL role>, then retry boot';
  role_ok boolean;
  schema_owner text;
  edge record;
  extra text;
BEGIN
  SELECT rolcanlogin = false AND rolsuper = false AND rolbypassrls = true
         AND rolcreatedb = false AND rolcreaterole = false AND rolreplication = false
         AND rolinherit = false
    INTO role_ok
    FROM pg_catalog.pg_roles
   WHERE rolname = 'myelin_ci_definition_fence';
  IF role_ok IS NULL THEN
    RAISE EXCEPTION 'the myelin_ci_definition_fence role is absent: %', remediation;
  END IF;
  IF NOT role_ok THEN
    RAISE EXCEPTION
      'the myelin_ci_definition_fence role does not carry the exact provisioned attributes '
      '(NOLOGIN NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT): %',
      remediation;
  END IF;

  SELECT pg_catalog.pg_get_userbyid(n.nspowner) INTO schema_owner
    FROM pg_catalog.pg_namespace n WHERE n.nspname = 'myelin_ci_security';
  IF schema_owner IS NULL THEN
    RAISE EXCEPTION 'the myelin_ci_security schema is absent: %', remediation;
  END IF;
  IF schema_owner <> 'myelin_ci_definition_fence' THEN
    RAISE EXCEPTION
      'the myelin_ci_security schema is owned by % rather than myelin_ci_definition_fence: %',
      schema_owner, remediation;
  END IF;

  SELECT a.admin_option, a.inherit_option, a.set_option INTO edge
    FROM pg_catalog.pg_auth_members a
   WHERE a.roleid = 'myelin_ci_definition_fence'::regrole::oid
     AND a.member = current_user::regrole::oid;
  IF edge IS NULL THEN
    RAISE EXCEPTION
      'the migration role % has no direct membership in myelin_ci_definition_fence, so it cannot '
      'create the probe function as its final owner: %', current_user, remediation;
  END IF;
  IF NOT edge.set_option OR edge.inherit_option OR edge.admin_option THEN
    RAISE EXCEPTION
      'the migration role % membership in myelin_ci_definition_fence must be '
      '(set_option=true, inherit_option=false, admin_option=false), found (%, %, %): %',
      current_user, edge.set_option, edge.inherit_option, edge.admin_option, remediation;
  END IF;

  -- BOTH DIRECTIONS. Verifying only the current migrator's own edge would accept privilege drift
  -- since provisioning: another role holding SET TRUE could adopt the same BYPASSRLS authority, and
  -- the fence role being a member of anything else would silently widen its own reach.
  SELECT string_agg(m.rolname, ', ' ORDER BY m.rolname) INTO extra
    FROM pg_catalog.pg_auth_members a
    JOIN pg_catalog.pg_roles m ON m.oid = a.member
   WHERE a.roleid = 'myelin_ci_definition_fence'::regrole::oid
     AND a.member <> current_user::regrole::oid;
  IF extra IS NOT NULL THEN
    RAISE EXCEPTION
      'role(s) % may also adopt myelin_ci_definition_fence; exactly one migration role may hold '
      'that authority: %', extra, remediation;
  END IF;
  SELECT string_agg(g.rolname, ', ' ORDER BY g.rolname) INTO extra
    FROM pg_catalog.pg_auth_members a
    JOIN pg_catalog.pg_roles g ON g.oid = a.roleid
   WHERE a.member = 'myelin_ci_definition_fence'::regrole::oid;
  IF extra IS NOT NULL THEN
    RAISE EXCEPTION
      'myelin_ci_definition_fence is a member of %, which widens its reach beyond the one '
      'question it exists to answer: %', extra, remediation;
  END IF;
END
$myelin$;

-- (2) TABLE ACCESS, established only now that `workflow_run` exists. Table-level REVOKE does not
-- remove separately granted COLUMN ACLs, so every live column is revoked explicitly before the
-- three-column grant is (re)issued. This is what makes an adopted role's stale column grants go.
REVOKE ALL PRIVILEGES ON TABLE public.workflow_run FROM myelin_ci_definition_fence;
DO $myelin$
DECLARE
  column_name text;
BEGIN
  FOR column_name IN
    SELECT a.attname
      FROM pg_catalog.pg_attribute a
     WHERE a.attrelid = 'public.workflow_run'::regclass
       AND a.attnum > 0
       AND NOT a.attisdropped
  LOOP
    EXECUTE format(
      'REVOKE ALL (%I) ON TABLE public.workflow_run FROM myelin_ci_definition_fence', column_name);
  END LOOP;
END
$myelin$;
GRANT USAGE ON SCHEMA public TO myelin_ci_definition_fence;
GRANT SELECT (wf_type, wf_version, state) ON public.workflow_run TO myelin_ci_definition_fence;

-- (3) BECOME THE FINAL OWNER. The function is BORN fence-owned, so no ownership transfer is ever
-- needed — and the silent-adoption hazard of replacing a function that keeps a foreign owner
-- cannot arise. Works in both postures: a dogfood superuser may set the role regardless, and a
-- production non-superuser migrator uses its explicit `SET TRUE` membership.
SET LOCAL ROLE myelin_ci_definition_fence;

-- (4) EXACT ADOPT-OR-CREATE. Never CREATE OR REPLACE: an existing function is accepted only when
-- every catalog field agrees exactly, and ANY divergence raises rather than overwriting something
-- another operator or an older build put there.
DO $myelin$
DECLARE
  probe constant text :=
    'myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer)';
  expected_body constant text :=
    'SELECT EXISTS (SELECT 1 FROM public.workflow_run '
    'WHERE wf_type = ''ci.pipeline'' AND wf_version = $1 '
    'AND state IN (''running'', ''waiting''))';
  existing oid := pg_catalog.to_regprocedure(probe);
  found record;
BEGIN
  IF existing IS NULL THEN
    EXECUTE format(
      'CREATE FUNCTION %s RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER '
      'SET search_path = pg_catalog SET row_security = off AS $body$%s$body$',
      probe, expected_body);
  ELSE
    SELECT p.proowner, p.prolang, p.prokind, p.prorettype, p.proargtypes::text,
           p.provolatile, p.prosecdef, p.proconfig, btrim(p.prosrc) AS body
      INTO found
      FROM pg_catalog.pg_proc p
     WHERE p.oid = existing;
    IF found.proowner <> 'myelin_ci_definition_fence'::regrole::oid
       OR found.prolang <> (SELECT oid FROM pg_catalog.pg_language WHERE lanname = 'sql')
       OR found.prokind <> 'f'
       OR found.prorettype <> 'boolean'::regtype::oid
       OR found.proargtypes <> 'int4'::regtype::oid::text
       OR found.provolatile <> 's'
       OR found.prosecdef <> true
       OR found.proconfig IS DISTINCT FROM
          ARRAY['search_path=pg_catalog', 'row_security=off']
       OR found.body <> expected_body THEN
      RAISE EXCEPTION
        'a function already exists at % but diverges from the expected definition-fence probe '
        '(owner/language/kind/return/args/volatility/security/config/body). Refusing to overwrite '
        'it. Inspect it, then remove or reconcile it deliberately.', probe;
    END IF;
  END IF;
END
$myelin$;

-- (5) FUNCTION AND SCHEMA ACLs, normalized. Every non-owner grant is stripped first so an adopted
-- object cannot retain an execute grant from another purpose.
DO $myelin$
DECLARE
  probe constant text :=
    'myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer)';
  grantee text;
BEGIN
  FOR grantee IN
    SELECT pg_catalog.pg_get_userbyid(acl.grantee)
      FROM pg_catalog.pg_proc p
      CROSS JOIN LATERAL pg_catalog.aclexplode(p.proacl) AS acl
     WHERE p.oid = pg_catalog.to_regprocedure(probe)
       AND acl.grantee <> 'myelin_ci_definition_fence'::regrole::oid
       AND acl.grantee <> 0
  LOOP
    EXECUTE format('REVOKE ALL ON FUNCTION %s FROM %I', probe, grantee);
  END LOOP;
END
$myelin$;
REVOKE ALL ON FUNCTION myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer)
  FROM PUBLIC;
-- Every non-owner grant on the security schema itself is stripped too. Granting `myelin_app` USAGE
-- without removing anyone else's USAGE/CREATE would let privilege drift since provisioning survive
-- a migration that claims to verify exact provisioning.
DO $myelin$
DECLARE
  grantee text;
BEGIN
  FOR grantee IN
    SELECT pg_catalog.pg_get_userbyid(acl.grantee)
      FROM pg_catalog.pg_namespace n
      CROSS JOIN LATERAL pg_catalog.aclexplode(n.nspacl) AS acl
     WHERE n.nspname = 'myelin_ci_security'
       AND acl.grantee <> 'myelin_ci_definition_fence'::regrole::oid
       AND acl.grantee <> 0
       AND acl.grantee <> 'myelin_app'::regrole::oid
  LOOP
    EXECUTE format('REVOKE ALL ON SCHEMA myelin_ci_security FROM %I', grantee);
  END LOOP;
END
$myelin$;
REVOKE ALL ON SCHEMA myelin_ci_security FROM PUBLIC;
REVOKE CREATE ON SCHEMA myelin_ci_security FROM myelin_app;
GRANT USAGE ON SCHEMA myelin_ci_security TO myelin_app;
GRANT EXECUTE ON FUNCTION
  myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer) TO myelin_app;

RESET ROLE;";

pub const SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL: &str = "\
INSERT INTO wf_definition (wf_type, version, code_hash, status)
VALUES ('ci.pipeline', 2, 'sentinel:ci-pipeline-v2-never-deployed-on-this-database', 'retired')
ON CONFLICT (wf_type, version) DO NOTHING";

pub const SEED_CI_PIPELINE_V3_CUTOVER_FENCE_ROW_DDL: &str = "\
INSERT INTO wf_definition (wf_type, version, code_hash, status)
VALUES (
  'ci.pipeline',
  3,
  'sentinel:ci-pipeline-v3-never-deployed-on-this-database',
  'retired'
)
ON CONFLICT (wf_type, version) DO NOTHING";

pub const SEED_CI_PIPELINE_V4_CUTOVER_FENCE_ROW_DDL: &str = "\
INSERT INTO wf_definition (wf_type, version, code_hash, status)
VALUES (
  'ci.pipeline',
  4,
  'sentinel:ci-pipeline-v4-never-deployed-on-this-database',
  'retired'
)
ON CONFLICT (wf_type, version) DO NOTHING";

pub const SEED_CI_PIPELINE_V5_CUTOVER_FENCE_ROW_DDL: &str = "\
INSERT INTO wf_definition (wf_type, version, code_hash, status)
VALUES (
  'ci.pipeline',
  5,
  'sentinel:ci-pipeline-v5-never-deployed-on-this-database',
  'retired'
)
ON CONFLICT (wf_type, version) DO NOTHING";

pub const SEED_CI_PIPELINE_V6_CUTOVER_FENCE_ROW_DDL: &str = "\
INSERT INTO wf_definition (wf_type, version, code_hash, status)
VALUES (
  'ci.pipeline',
  6,
  'sentinel:ci-pipeline-v6-never-deployed-on-this-database',
  'retired'
)
ON CONFLICT (wf_type, version) DO NOTHING";

pub const ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL: &str = "\
ALTER TABLE job_queue ADD COLUMN IF NOT EXISTS reservation_write_version smallint;
DO $myelin$
DECLARE
  expected_definition constant text :=
    'CHECK ((reservation_write_version = 2)) NOT VALID';
  existing_definition text;
BEGIN
  ALTER TABLE job_queue
    ADD CONSTRAINT job_queue_reservation_write_version_marker
    CHECK (reservation_write_version = 2) NOT VALID;
EXCEPTION WHEN duplicate_object THEN
  SELECT pg_catalog.pg_get_constraintdef(constraint_catalog.oid)
    INTO existing_definition
    FROM pg_catalog.pg_constraint AS constraint_catalog
   WHERE constraint_catalog.conrelid = 'job_queue'::regclass
     AND constraint_catalog.conname = 'job_queue_reservation_write_version_marker';
  IF existing_definition IS DISTINCT FROM expected_definition
     AND existing_definition IS DISTINCT FROM replace(expected_definition, ' NOT VALID', '') THEN
    RAISE EXCEPTION
      'job_queue_reservation_write_version_marker already exists with a DIVERGENT definition: % (expected: %)',
      existing_definition, expected_definition;
  END IF;
END
$myelin$";

pub const VALIDATE_JOB_QUEUE_RESERVATION_WRITE_VERSION_DDL: &str = "\
ALTER TABLE job_queue VALIDATE CONSTRAINT job_queue_reservation_write_version_marker";

pub const CREATE_JOB_QUEUE_ACTIVATION_READINESS_INDEX_DDL: &str = "\
CREATE INDEX CONCURRENTLY IF NOT EXISTS job_queue_activation_readiness \
ON job_queue (region) \
WHERE state <> 'terminal' AND (claim_window_secs IS NULL OR reservation_write_version IS DISTINCT FROM 2)";

pub const CREATE_CI_V2_ACTIVATION_READINESS_PROBE_DDL: &str = "\
-- ATOMICITY: this whole script is ONE transaction, the IMPLICIT one PostgreSQL opens around a
-- multi-statement simple query — NOT an explicit `BEGIN`/`COMMIT`, which would leave the pooled
-- migration connection in an aborted transaction block on refusal (the exact `ci_0020h` reasoning).

-- (1) PROVISIONING PREFLIGHT. The migration VERIFIES; it never creates or alters a cluster role.
-- BYPASSRLS provisioning is an operator-controlled action, and migration behaviour must not vary
-- with accidental excess privilege.
DO $myelin$
DECLARE
  remediation constant text :=
    'run scripts/pg-init/01-ci-definition-fence.sql as the database provisioning administrator, '
    'passing migration_role=<DATABASE_MIGRATION_URL role>, then retry boot';
  role_ok boolean;
  schema_owner text;
  edge record;
  extra text;
BEGIN
  SELECT rolcanlogin = false AND rolsuper = false AND rolbypassrls = true
         AND rolcreatedb = false AND rolcreaterole = false AND rolreplication = false
         AND rolinherit = false
    INTO role_ok
    FROM pg_catalog.pg_roles
   WHERE rolname = 'myelin_ci_definition_fence';
  IF role_ok IS NULL THEN
    RAISE EXCEPTION 'the myelin_ci_definition_fence role is absent: %', remediation;
  END IF;
  IF NOT role_ok THEN
    RAISE EXCEPTION
      'the myelin_ci_definition_fence role does not carry the exact provisioned attributes '
      '(NOLOGIN NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT): %',
      remediation;
  END IF;

  SELECT pg_catalog.pg_get_userbyid(n.nspowner) INTO schema_owner
    FROM pg_catalog.pg_namespace n WHERE n.nspname = 'myelin_ci_security';
  IF schema_owner IS NULL THEN
    RAISE EXCEPTION 'the myelin_ci_security schema is absent: %', remediation;
  END IF;
  IF schema_owner <> 'myelin_ci_definition_fence' THEN
    RAISE EXCEPTION
      'the myelin_ci_security schema is owned by % rather than myelin_ci_definition_fence: %',
      schema_owner, remediation;
  END IF;

  SELECT a.admin_option, a.inherit_option, a.set_option INTO edge
    FROM pg_catalog.pg_auth_members a
   WHERE a.roleid = 'myelin_ci_definition_fence'::regrole::oid
     AND a.member = current_user::regrole::oid;
  IF edge IS NULL THEN
    RAISE EXCEPTION
      'the migration role % has no direct membership in myelin_ci_definition_fence, so it cannot '
      'create the probe function as its final owner: %', current_user, remediation;
  END IF;
  IF NOT edge.set_option OR edge.inherit_option OR edge.admin_option THEN
    RAISE EXCEPTION
      'the migration role % membership in myelin_ci_definition_fence must be '
      '(set_option=true, inherit_option=false, admin_option=false), found (%, %, %): %',
      current_user, edge.set_option, edge.inherit_option, edge.admin_option, remediation;
  END IF;

  -- BOTH DIRECTIONS, exactly as ci_0020h: another role holding SET TRUE could adopt the same
  -- BYPASSRLS authority, and the fence role being a member of anything else would silently widen it.
  SELECT string_agg(m.rolname, ', ' ORDER BY m.rolname) INTO extra
    FROM pg_catalog.pg_auth_members a
    JOIN pg_catalog.pg_roles m ON m.oid = a.member
   WHERE a.roleid = 'myelin_ci_definition_fence'::regrole::oid
     AND a.member <> current_user::regrole::oid;
  IF extra IS NOT NULL THEN
    RAISE EXCEPTION
      'role(s) % may also adopt myelin_ci_definition_fence; exactly one migration role may hold '
      'that authority: %', extra, remediation;
  END IF;
  SELECT string_agg(g.rolname, ', ' ORDER BY g.rolname) INTO extra
    FROM pg_catalog.pg_auth_members a
    JOIN pg_catalog.pg_roles g ON g.oid = a.roleid
   WHERE a.member = 'myelin_ci_definition_fence'::regrole::oid;
  IF extra IS NOT NULL THEN
    RAISE EXCEPTION
      'myelin_ci_definition_fence is a member of %, which widens its reach beyond the one '
      'question it exists to answer: %', extra, remediation;
  END IF;
END
$myelin$;

-- (2) TABLE ACCESS, established only now that `job_queue` exists. Table-level REVOKE does not remove
-- separately granted COLUMN ACLs, so every live column is revoked explicitly before the four-column
-- grant is (re)issued — this is what makes an adopted role's stale column grants go.
REVOKE ALL PRIVILEGES ON TABLE public.job_queue FROM myelin_ci_definition_fence;
DO $myelin$
DECLARE
  column_name text;
BEGIN
  FOR column_name IN
    SELECT a.attname
      FROM pg_catalog.pg_attribute a
     WHERE a.attrelid = 'public.job_queue'::regclass
       AND a.attnum > 0
       AND NOT a.attisdropped
  LOOP
    EXECUTE format(
      'REVOKE ALL (%I) ON TABLE public.job_queue FROM myelin_ci_definition_fence', column_name);
  END LOOP;
END
$myelin$;
GRANT USAGE ON SCHEMA public TO myelin_ci_definition_fence;
GRANT SELECT (region, state, claim_window_secs, reservation_write_version)
  ON public.job_queue TO myelin_ci_definition_fence;

-- (3) BECOME THE FINAL OWNER. The function is BORN fence-owned, so no ownership transfer is ever
-- needed and the silent-adoption hazard of a foreign-owned replace cannot arise.
SET LOCAL ROLE myelin_ci_definition_fence;

-- (4) EXACT ADOPT-OR-CREATE. Never CREATE OR REPLACE: an existing function is accepted only when
-- every catalog field agrees exactly, and ANY divergence raises rather than overwriting.
DO $myelin$
DECLARE
  probe constant text :=
    'myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count()';
  expected_body constant text :=
    'SELECT count(*) FROM public.job_queue '
    'WHERE state <> ''terminal'' '
    'AND (claim_window_secs IS NULL OR reservation_write_version IS DISTINCT FROM 2)';
  existing oid := pg_catalog.to_regprocedure(probe);
  found record;
BEGIN
  IF existing IS NULL THEN
    EXECUTE format(
      'CREATE FUNCTION %s RETURNS bigint LANGUAGE sql STABLE SECURITY DEFINER '
      'SET search_path = pg_catalog SET row_security = off AS $body$%s$body$',
      probe, expected_body);
  ELSE
    SELECT p.proowner, p.prolang, p.prokind, p.prorettype, p.proargtypes::text,
           p.provolatile, p.prosecdef, p.proconfig, btrim(p.prosrc) AS body
      INTO found
      FROM pg_catalog.pg_proc p
     WHERE p.oid = existing;
    IF found.proowner <> 'myelin_ci_definition_fence'::regrole::oid
       OR found.prolang <> (SELECT oid FROM pg_catalog.pg_language WHERE lanname = 'sql')
       OR found.prokind <> 'f'
       OR found.prorettype <> 'bigint'::regtype::oid
       OR found.proargtypes <> ''
       OR found.provolatile <> 's'
       OR found.prosecdef <> true
       OR found.proconfig IS DISTINCT FROM
          ARRAY['search_path=pg_catalog', 'row_security=off']
       OR found.body <> expected_body THEN
      RAISE EXCEPTION
        'a function already exists at % but diverges from the expected activation-readiness probe '
        '(owner/language/kind/return/args/volatility/security/config/body). Refusing to overwrite '
        'it. Inspect it, then remove or reconcile it deliberately.', probe;
    END IF;
  END IF;
END
$myelin$;

-- (5) FUNCTION AND SCHEMA ACLs, normalized. Every non-owner grant is stripped first so an adopted
-- object cannot retain an execute grant from another purpose.
DO $myelin$
DECLARE
  probe constant text :=
    'myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count()';
  grantee text;
BEGIN
  FOR grantee IN
    SELECT pg_catalog.pg_get_userbyid(acl.grantee)
      FROM pg_catalog.pg_proc p
      CROSS JOIN LATERAL pg_catalog.aclexplode(p.proacl) AS acl
     WHERE p.oid = pg_catalog.to_regprocedure(probe)
       AND acl.grantee <> 'myelin_ci_definition_fence'::regrole::oid
       AND acl.grantee <> 0
  LOOP
    EXECUTE format('REVOKE ALL ON FUNCTION %s FROM %I', probe, grantee);
  END LOOP;
END
$myelin$;
REVOKE ALL ON FUNCTION myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count()
  FROM PUBLIC;
-- The security schema's USAGE grant to myelin_app was established by ci_0020h and is left intact;
-- this migration only adds the EXECUTE on its own function.
GRANT EXECUTE ON FUNCTION
  myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count() TO myelin_app;

RESET ROLE;";

pub const ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL: &str = "ALTER TABLE job_queue \
ADD COLUMN IF NOT EXISTS retry_attempts jsonb NOT NULL DEFAULT '{}'::jsonb";

pub const GRANT_SCHEDULER_LEASE_EPOCH_DDL: &str =
    "GRANT UPDATE (lease_epoch) ON job_queue TO myelin_ci_region_scheduler";

pub const GRANT_SCHEDULER_CLAIM_NONCE_DDL: &str =
    "GRANT UPDATE (claim_nonce) ON job_queue TO myelin_ci_region_scheduler";

pub const GRANT_SCHEDULER_CLAIM_TIME_DDL: &str = "GRANT UPDATE \
(claim_started_at, claim_expires_at) ON job_queue TO myelin_ci_region_scheduler";

fn create_statements() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "ci_0001_ci_run",
            CI_RUN_TABLE,
            CREATE_CI_RUN_DDL.to_string(),
        ),
        (
            "ci_0001a_ci_drive_manifest",
            CI_DRIVE_MANIFEST_TABLE,
            CREATE_CI_DRIVE_MANIFEST_DDL.to_string(),
        ),
        (
            "ci_0002_ci_job",
            CI_JOB_TABLE,
            CREATE_CI_JOB_DDL.to_string(),
        ),
        (
            "ci_0003_check_attempt",
            CHECK_ATTEMPT_TABLE,
            CREATE_CHECK_ATTEMPT_DDL.to_string(),
        ),
        (
            "ci_0003a_ci_run_check_attempt",
            CI_RUN_CHECK_ATTEMPT_TABLE,
            CREATE_CI_RUN_CHECK_ATTEMPT_DDL.to_string(),
        ),
        (
            "ci_0004_job_queue",
            JOB_QUEUE_TABLE,
            CREATE_JOB_QUEUE_DDL.to_string(),
        ),
        (
            "ci_0005_fair_deficit",
            FAIR_DEFICIT_TABLE,
            CREATE_FAIR_DEFICIT_DDL.to_string(),
        ),
        (
            "ci_0006_runner",
            RUNNER_TABLE,
            CREATE_RUNNER_DDL.to_string(),
        ),
        (
            "ci_0007_log_segment",
            LOG_SEGMENT_TABLE,
            CREATE_LOG_SEGMENT_DDL.to_string(),
        ),
        (
            "ci_0008_log_anchor",
            LOG_ANCHOR_TABLE,
            CREATE_LOG_ANCHOR_DDL.to_string(),
        ),
        (
            "ci_0009_artifact",
            ARTIFACT_TABLE,
            CREATE_ARTIFACT_DDL.to_string(),
        ),
        (
            "ci_0010_cache_entry",
            CACHE_ENTRY_TABLE,
            CREATE_CACHE_ENTRY_DDL.to_string(),
        ),
        (
            "ci_0011_environment",
            ENVIRONMENT_TABLE,
            CREATE_ENVIRONMENT_DDL.to_string(),
        ),
        (
            "ci_0012_deployment",
            DEPLOYMENT_TABLE,
            CREATE_DEPLOYMENT_DDL.to_string(),
        ),
        (
            "ci_0013_secret_binding",
            SECRET_BINDING_TABLE,
            CREATE_SECRET_BINDING_DDL.to_string(),
        ),
        (
            "ci_0014_ci_cost_event",
            CI_COST_EVENT_TABLE,
            CREATE_CI_COST_EVENT_DDL.to_string(),
        ),
        (
            "ci_0015_ci_job_spec",
            CI_JOB_SPEC_TABLE,
            CREATE_CI_JOB_SPEC_DDL.to_string(),
        ),
        (
            "ci_0017_ci_job_accounting",
            CI_JOB_ACCOUNTING_TABLE,
            CREATE_CI_JOB_ACCOUNTING_DDL.to_string(),
        ),
        (
            CI_JOB_PARENT_ATTEMPT_MIGRATION_ID,
            CI_JOB_PARENT_ATTEMPT_TABLE,
            CREATE_CI_JOB_PARENT_ATTEMPT_DDL.to_string(),
        ),
        (
            CI_JOB_PRELAUNCH_USAGE_MIGRATION_ID,
            CI_JOB_PRELAUNCH_USAGE_TABLE,
            CREATE_CI_JOB_PRELAUNCH_USAGE_DDL.to_string(),
        ),
        (
            CI_JOB_CREDENTIAL_GENERATION_MIGRATION_ID,
            CI_JOB_CREDENTIAL_GENERATION_TABLE,
            CREATE_CI_JOB_CREDENTIAL_GENERATION_DDL.to_string(),
        ),
        (
            CI_SECRET_MIGRATION_ID,
            CI_SECRET_TABLE,
            CREATE_CI_SECRET_DDL.to_string(),
        ),
    ]
}

pub const CI_JOB_QUEUE_INDEX_MIGRATIONS: &[(&str, &str)] = &[
    ("ci_0004a_jq_claimable", JQ_CLAIMABLE_INDEX),
    ("ci_0004b_jq_serialize", JQ_SERIALIZE_INDEX),
    ("ci_0004c_jq_idem", JQ_IDEM_INDEX),
];

pub const CI_REGION_SCHEDULER_RLS_MIGRATION_ID: &str = "ci_0016_region_scheduler_rls";

pub const CREATE_CI_REGION_SCHEDULER_RLS_DDL: &str = "\
CREATE POLICY myelin_ci_scheduler_job_queue_access ON job_queue
  AS PERMISSIVE FOR ALL TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  )
  WITH CHECK (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
CREATE POLICY myelin_ci_scheduler_job_queue_guard ON job_queue
  AS RESTRICTIVE FOR ALL TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  )
  WITH CHECK (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
CREATE POLICY myelin_ci_scheduler_fair_deficit_access ON fair_deficit
  AS PERMISSIVE FOR SELECT TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
CREATE POLICY myelin_ci_scheduler_fair_deficit_guard ON fair_deficit
  AS RESTRICTIVE FOR SELECT TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
GRANT SELECT ON job_queue TO myelin_ci_region_scheduler;
GRANT UPDATE (state, lease_owner, lease_expires) ON job_queue TO myelin_ci_region_scheduler;
GRANT SELECT ON fair_deficit TO myelin_ci_region_scheduler";

pub const GRANT_SCHEDULER_CI_RUN_DISCOVERY_DDL: &str = "\
CREATE POLICY myelin_ci_scheduler_ci_run_discovery_access ON ci_run
  AS PERMISSIVE FOR SELECT TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
CREATE POLICY myelin_ci_scheduler_ci_run_discovery_guard ON ci_run
  AS RESTRICTIVE FOR SELECT TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
GRANT SELECT (tenant_id, region, state, created_at, run_id) ON ci_run
  TO myelin_ci_region_scheduler";

pub const GRANT_SCHEDULER_CI_RUN_WORKFLOW_ID_DDL: &str =
    "GRANT SELECT (wf_run_id) ON ci_run TO myelin_ci_region_scheduler";

pub const GRANT_SCHEDULER_CI_JOB_REAP_RESET_DDL: &str = "\
GRANT SELECT (tenant_id, job_id, state) ON ci_job TO myelin_ci_region_scheduler;
GRANT UPDATE (state) ON ci_job TO myelin_ci_region_scheduler";

pub const CREATE_CI_RUN_ACTIVE_WORKFLOW_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
ci_run_active_workflow ON ci_run (tenant_id, region, wf_run_id) \
WHERE state = 'running'";

pub const CREATE_CI_RUN_QUEUED_REGION_INDEX_DDL: &str = "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
ci_run_queued_region ON ci_run (region, created_at, run_id) INCLUDE (tenant_id) \
WHERE state = 'queued'";

pub const CREATE_CI_WORKFLOW_ACTIVE_REGION_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
ci_workflow_active_region ON workflow_run (region, created_at, tenant_id, run_id) \
INCLUDE (partition) \
WHERE wf_type = 'ci.pipeline' AND state IN ('running', 'waiting')";

pub const CREATE_CI_RUN_SURFACE_REPO_CREATED_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
ci_run_surface_repo_created ON ci_run \
(tenant_id, region, repo_ref, created_at DESC, run_id DESC) \
WHERE repo_ref IS NOT NULL";

pub const GRANT_SCHEDULER_CI_WORKFLOW_DISCOVERY_DDL: &str = "\
CREATE POLICY myelin_ci_scheduler_workflow_discovery_access ON workflow_run
  AS PERMISSIVE FOR SELECT TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
CREATE POLICY myelin_ci_scheduler_workflow_discovery_guard ON workflow_run
  AS RESTRICTIVE FOR SELECT TO myelin_ci_region_scheduler
  USING (
    current_setting('myelin.tenant_id', true) = ''
    AND region = public.myelin_ci_scheduler_region()
    AND region = current_setting('myelin.region', true)
  );
GRANT SELECT (tenant_id, region, run_id, wf_type, state, partition, created_at)
  ON workflow_run TO myelin_ci_region_scheduler";

pub const CI_DURABLE_WRITER_IDS: &[&str] = &[
    "ci_0001_ci_run",
    "ci_0003_check_attempt",
    "ci_0003a_ci_run_check_attempt",
    "ci_0014_ci_cost_event",
];

fn assemble_ci_migration(id: &'static str, table: &'static str, create: String) -> Migration {
    let mut ddl = create;
    if !ddl.trim_end().ends_with(';') {
        ddl.push(';');
    }
    ddl.push('\n');
    ddl.push_str(&make_tenant_scoped_ddl(table));
    ddl.push(';');
    let ddl: &'static str = Box::leak(ddl.into_boxed_str());
    Migration::plain_on(id, ddl, table)
}

pub fn make_tenant_scoped_ddl(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

pub fn ci_controlplane_migrations() -> Migrations {
    let mut migrations = Vec::new();
    for (id, table, create) in create_statements() {
        if table == CI_SECRET_TABLE {
            continue;
        }
        migrations.push(assemble_ci_migration(id, table, create));
        if table == CI_RUN_TABLE {
            migrations.push(Migration::plain_on(
                CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID,
                ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL,
                CI_RUN_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID,
                ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL,
                CI_RUN_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID,
                ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL,
                CI_RUN_TABLE,
            ));
        }
        if table == CI_JOB_TABLE {
            migrations.push(Migration::plain_on(
                CI_JOB_RUN_LEDGER_INDEX_MIGRATION_ID,
                CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL,
                CI_JOB_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_JOB_RUN_LEDGER_VALIDATION_MIGRATION_ID,
                VALIDATE_CI_JOB_RUN_LEDGER_INDEX_DDL,
                CI_JOB_TABLE,
            ));
        }
        if table == JOB_QUEUE_TABLE {
            for ((index_id, expected_name), (actual_name, ddl)) in CI_JOB_QUEUE_INDEX_MIGRATIONS
                .iter()
                .zip(CREATE_JOB_QUEUE_INDEXES_DDL.iter())
            {
                debug_assert_eq!(expected_name, actual_name);
                migrations.push(Migration::plain_on(index_id, ddl, JOB_QUEUE_TABLE));
            }
            migrations.push(Migration::plain_on(
                CI_JOB_QUEUE_COMPLETION_MIGRATION_ID,
                ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
                JOB_QUEUE_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID,
                ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL,
                JOB_QUEUE_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_JOB_QUEUE_CLAIM_TIME_MIGRATION_ID,
                ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
                JOB_QUEUE_TABLE,
            ));
        }
        if table == CI_JOB_SPEC_TABLE {
            migrations.push(Migration::plain_on(
                CI_JOB_SPEC_STAGE_MIGRATION_ID,
                ALTER_CI_JOB_SPEC_ADD_STAGE_DDL,
                CI_JOB_SPEC_TABLE,
            ));
        }
        if table == CI_JOB_ACCOUNTING_TABLE {
            migrations.push(Migration::plain_on(
                CI_JOB_ACCOUNTING_SKIPPED_MIGRATION_ID,
                ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL,
                CI_JOB_ACCOUNTING_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_JOB_ACCOUNTING_DISPOSITION_V4_MIGRATION_ID,
                ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_DDL,
                CI_JOB_ACCOUNTING_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_JOB_ACCOUNTING_DISPOSITION_V4_VERDICT_MIGRATION_ID,
                ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_VERDICT_DDL,
                CI_JOB_ACCOUNTING_TABLE,
            ));
        }
        if table == CI_JOB_PRELAUNCH_USAGE_TABLE {
            migrations.push(Migration::plain_on(
                CI_JOB_PRELAUNCH_USAGE_REAPER_INDEX_MIGRATION_ID,
                CREATE_CI_JOB_PRELAUNCH_USAGE_REAPER_INDEX_DDL,
                CI_JOB_PRELAUNCH_USAGE_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_SCHEDULER_PRELAUNCH_USAGE_REAP_GRANT_MIGRATION_ID,
                GRANT_SCHEDULER_CI_JOB_PRELAUNCH_USAGE_REAP_DDL,
                CI_JOB_PRELAUNCH_USAGE_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_MIGRATION_ID,
                ALTER_CI_JOB_PRELAUNCH_USAGE_ADD_SEAL_DEADLINE_DDL,
                CI_JOB_PRELAUNCH_USAGE_TABLE,
            ));
            migrations.push(Migration::plain_on(
                CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_MIGRATION_ID,
                CREATE_CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_DDL,
                CI_JOB_PRELAUNCH_USAGE_TABLE,
            ));
        }
    }
    migrations.push(Migration::plain_on(
        CI_REGION_SCHEDULER_RLS_MIGRATION_ID,
        CREATE_CI_REGION_SCHEDULER_RLS_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SCHEDULER_LEASE_EPOCH_GRANT_MIGRATION_ID,
        GRANT_SCHEDULER_LEASE_EPOCH_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID,
        GRANT_SCHEDULER_CLAIM_NONCE_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SCHEDULER_CLAIM_TIME_GRANT_MIGRATION_ID,
        GRANT_SCHEDULER_CLAIM_TIME_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID,
        CREATE_CI_RUN_QUEUED_REGION_INDEX_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID,
        GRANT_SCHEDULER_CI_RUN_DISCOVERY_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID,
        CREATE_CI_WORKFLOW_ACTIVE_REGION_INDEX_DDL,
        "workflow_run",
    ));
    migrations.push(Migration::plain_on(
        CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID,
        GRANT_SCHEDULER_CI_WORKFLOW_DISCOVERY_DDL,
        "workflow_run",
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID,
        CREATE_CI_RUN_SURFACE_REPO_CREATED_INDEX_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_JOB_QUEUE_RETRY_ATTEMPTS_MIGRATION_ID,
        ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_ACTIVE_WORKFLOW_INDEX_MIGRATION_ID,
        CREATE_CI_RUN_ACTIVE_WORKFLOW_INDEX_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SCHEDULER_CI_RUN_WORKFLOW_ID_GRANT_MIGRATION_ID,
        GRANT_SCHEDULER_CI_RUN_WORKFLOW_ID_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SCHEDULER_CI_JOB_REAP_RESET_GRANT_MIGRATION_ID,
        GRANT_SCHEDULER_CI_JOB_REAP_RESET_DDL,
        CI_JOB_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_JOB_QUEUE_CLAIM_WINDOW_MIGRATION_ID,
        ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_JOB_QUEUE_CLAIM_WINDOW_VALIDATE_MIGRATION_ID,
        VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SCHEDULER_WORKFLOW_VERSION_GRANT_MIGRATION_ID,
        GRANT_SCHEDULER_WORKFLOW_VERSION_DDL,
        "workflow_run",
    ));
    migrations.push(Migration::plain(
        CI_PIPELINE_VERSION_BACKLOG_PROBE_MIGRATION_ID,
        CREATE_CI_PIPELINE_VERSION_BACKLOG_PROBE_DDL,
    ));
    migrations.push(Migration::plain(
        CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID,
        SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL,
    ));
    migrations.push(Migration::plain_on(
        CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_MIGRATION_ID,
        ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_VALIDATE_MIGRATION_ID,
        VALIDATE_JOB_QUEUE_RESERVATION_WRITE_VERSION_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_JOB_QUEUE_ACTIVATION_READINESS_INDEX_MIGRATION_ID,
        CREATE_JOB_QUEUE_ACTIVATION_READINESS_INDEX_DDL,
        JOB_QUEUE_TABLE,
    ));
    migrations.push(Migration::plain(
        CI_V2_ACTIVATION_READINESS_PROBE_MIGRATION_ID,
        CREATE_CI_V2_ACTIVATION_READINESS_PROBE_DDL,
    ));
    migrations.push(Migration::plain(
        CI_PIPELINE_V3_CUTOVER_FENCE_ROW_MIGRATION_ID,
        SEED_CI_PIPELINE_V3_CUTOVER_FENCE_ROW_DDL,
    ));
    migrations.push(assemble_ci_migration(
        CI_SECRET_MIGRATION_ID,
        CI_SECRET_TABLE,
        CREATE_CI_SECRET_DDL.to_owned(),
    ));
    migrations.push(Migration::plain_on(
        CI_SECRET_ADMIN_SCOPE_MIGRATION_ID,
        ALTER_CI_SECRET_ADD_ADMIN_SCOPE_DDL,
        CI_SECRET_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SECRET_ADMIN_UNIQUE_MIGRATION_ID,
        CREATE_CI_SECRET_ADMIN_UNIQUE_INDEX_DDL,
        CI_SECRET_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_SECRET_BINDING_INTEGRITY_MIGRATION_ID,
        ALTER_SECRET_BINDING_ADD_INTEGRITY_DDL,
        SECRET_BINDING_TABLE,
    ));
    migrations.push(assemble_ci_migration(
        CI_SECRET_TOMBSTONE_MIGRATION_ID,
        CI_SECRET_TOMBSTONE_TABLE,
        CREATE_CI_SECRET_TOMBSTONE_DDL.to_owned(),
    ));
    migrations.push(assemble_ci_migration(
        CI_SECRET_VERSION_HIGH_WATER_MIGRATION_ID,
        CI_SECRET_VERSION_HIGH_WATER_TABLE,
        CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL.to_owned(),
    ));
    migrations.push(Migration::plain_on(
        CI_JOB_ACCOUNTING_DISPOSITION_V4_SECRET_RESOLUTION_MIGRATION_ID,
        ALTER_CI_JOB_ACCOUNTING_DISPOSITION_V4_SECRET_RESOLUTION_DDL,
        CI_JOB_ACCOUNTING_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_SOURCE_REF_MIGRATION_ID,
        ALTER_CI_RUN_ADD_SOURCE_REF_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID,
        REPAIR_CI_RUN_SOURCE_REF_CONSTRAINT_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID,
        VALIDATE_CI_RUN_SOURCE_REF_CONSTRAINT_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_BRANCH_SCOPE_EXPAND_MIGRATION_ID,
        EXPAND_CI_RUN_BRANCH_SCOPE_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_BRANCH_SCOPE_VALIDATE_MIGRATION_ID,
        VALIDATE_CI_RUN_BRANCH_SCOPE_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID,
        CONTRACT_CI_RUN_BRANCH_SCOPE_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain(
        CI_PIPELINE_V4_CUTOVER_FENCE_ROW_MIGRATION_ID,
        SEED_CI_PIPELINE_V4_CUTOVER_FENCE_ROW_DDL,
    ));
    migrations.push(Migration::plain(
        CI_PIPELINE_V5_CUTOVER_FENCE_ROW_MIGRATION_ID,
        SEED_CI_PIPELINE_V5_CUTOVER_FENCE_ROW_DDL,
    ));
    migrations.push(Migration::plain(
        CI_PIPELINE_V6_CUTOVER_FENCE_ROW_MIGRATION_ID,
        SEED_CI_PIPELINE_V6_CUTOVER_FENCE_ROW_DDL,
    ));
    Migrations::of(migrations)
}

pub fn ci_durable_migrations() -> Migrations {
    let mut migrations = create_statements()
        .into_iter()
        .filter(|(id, _table, _create)| CI_DURABLE_WRITER_IDS.contains(id))
        .map(|(id, table, create)| assemble_ci_migration(id, table, create))
        .collect::<Vec<_>>();
    migrations.insert(
        1,
        Migration::plain_on(
            CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID,
            ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL,
            CI_RUN_TABLE,
        ),
    );
    migrations.insert(
        2,
        Migration::plain_on(
            CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID,
            ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL,
            CI_RUN_TABLE,
        ),
    );
    migrations.insert(
        3,
        Migration::plain_on(
            CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID,
            ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL,
            CI_RUN_TABLE,
        ),
    );
    migrations.push(Migration::plain_on(
        CI_RUN_SOURCE_REF_MIGRATION_ID,
        ALTER_CI_RUN_ADD_SOURCE_REF_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID,
        REPAIR_CI_RUN_SOURCE_REF_CONSTRAINT_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID,
        VALIDATE_CI_RUN_SOURCE_REF_CONSTRAINT_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_BRANCH_SCOPE_EXPAND_MIGRATION_ID,
        EXPAND_CI_RUN_BRANCH_SCOPE_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_BRANCH_SCOPE_VALIDATE_MIGRATION_ID,
        VALIDATE_CI_RUN_BRANCH_SCOPE_DDL,
        CI_RUN_TABLE,
    ));
    migrations.push(Migration::plain_on(
        CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID,
        CONTRACT_CI_RUN_BRANCH_SCOPE_DDL,
        CI_RUN_TABLE,
    ));
    Migrations::of(migrations)
}

pub fn ci_durable_hot_tables() -> HotTables {
    HotTables::declare([
        CI_COST_EVENT_TABLE,
        CHECK_ATTEMPT_TABLE,
        CI_RUN_CHECK_ATTEMPT_TABLE,
    ])
}

pub fn ci_controlplane_hot_tables() -> HotTables {
    HotTables::declare([
        JOB_QUEUE_TABLE,
        LOG_SEGMENT_TABLE,
        CI_COST_EVENT_TABLE,
        CHECK_ATTEMPT_TABLE,
        CI_RUN_CHECK_ATTEMPT_TABLE,
        CI_JOB_PARENT_ATTEMPT_TABLE,
        CI_JOB_PRELAUNCH_USAGE_TABLE,
        CI_JOB_CREDENTIAL_GENERATION_TABLE,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_disposition_migrations_are_checksum_frozen() {
        use myelin_storage::pg_migrator::ddl_checksum;
        assert_eq!(
            ddl_checksum(ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_DDL),
            "blake3:1ebb53e026947d09199e3162092c81af778740841d941f56483ef397b413e01c",
            "ci_0017b DDL is checksum-frozen (already applied + immutable); widen the disposition enum \
             via a forward migration, never by editing this const"
        );
        assert_eq!(
            ddl_checksum(ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_VERDICT_DDL),
            "blake3:13ef0a21602ade14a087aa433f8de6f9432cb1079f9cd2c468fe216d9074ab68",
            "ci_0017c verdict DDL is checksum-frozen (already applied + immutable)"
        );
    }

    #[test]
    fn all_twenty_three_controlplane_tables_are_present_fk_ordered() {
        let migrations = ci_controlplane_migrations();
        let tables: Vec<&str> = migrations
            .0
            .iter()
            .filter(|m| m.ddl.contains("CREATE TABLE"))
            .map(|m| m.table.unwrap())
            .collect();
        assert_eq!(
            tables,
            vec![
                CI_RUN_TABLE,
                CI_DRIVE_MANIFEST_TABLE,
                CI_JOB_TABLE,
                CHECK_ATTEMPT_TABLE,
                CI_RUN_CHECK_ATTEMPT_TABLE,
                JOB_QUEUE_TABLE,
                FAIR_DEFICIT_TABLE,
                RUNNER_TABLE,
                LOG_SEGMENT_TABLE,
                LOG_ANCHOR_TABLE,
                ARTIFACT_TABLE,
                CACHE_ENTRY_TABLE,
                ENVIRONMENT_TABLE,
                DEPLOYMENT_TABLE,
                SECRET_BINDING_TABLE,
                CI_COST_EVENT_TABLE,
                CI_JOB_SPEC_TABLE,
                CI_JOB_ACCOUNTING_TABLE,
                CI_JOB_PARENT_ATTEMPT_TABLE,
                CI_JOB_PRELAUNCH_USAGE_TABLE,
                CI_JOB_CREDENTIAL_GENERATION_TABLE,
                CI_SECRET_TABLE,
                CI_SECRET_TOMBSTONE_TABLE,
                CI_SECRET_VERSION_HIGH_WATER_TABLE,
            ],
            "all 24 control-plane tables, FK-dependency ordered (ci_run before its dependants)"
        );
        let run_pos = tables.iter().position(|t| *t == CI_RUN_TABLE).unwrap();
        let manifest_pos = tables
            .iter()
            .position(|t| *t == CI_DRIVE_MANIFEST_TABLE)
            .unwrap();
        let job_pos = tables.iter().position(|t| *t == CI_JOB_TABLE).unwrap();
        assert!(
            run_pos < manifest_pos && run_pos < job_pos,
            "ci_run is created before both FK dependants"
        );
    }

    #[test]
    fn ci_secret_schema_has_a_ciphertext_at_rest_floor() {
        assert!(CREATE_CI_SECRET_DDL.contains("nonce      bytea"));
        assert!(CREATE_CI_SECRET_DDL.contains("ciphertext bytea"));
        assert!(CREATE_CI_SECRET_DDL.contains("pii_key_ref text"));
        assert!(CREATE_CI_SECRET_DDL.contains("PRIMARY KEY (tenant_id, secret_id)"));
        for forbidden in [" plaintext", " material", " secret_value", " value text"] {
            assert!(
                !CREATE_CI_SECRET_DDL.contains(forbidden),
                "ci_secret must have no plaintext-at-rest column: {forbidden}"
            );
        }
        assert_eq!(
            ALTER_CI_SECRET_ADD_ADMIN_SCOPE_DDL,
            "ALTER TABLE ci_secret ADD COLUMN IF NOT EXISTS project_id uuid"
        );
        assert!(
            CREATE_CI_SECRET_ADMIN_UNIQUE_INDEX_DDL.starts_with("CREATE UNIQUE INDEX CONCURRENTLY")
        );
        assert!(CREATE_CI_SECRET_ADMIN_UNIQUE_INDEX_DDL
            .contains("(tenant_id, project_id, name) WHERE project_id IS NOT NULL"));
    }

    #[test]
    fn secret_binding_integrity_is_forward_only_fk_cascade() {
        assert!(
            !CREATE_SECRET_BINDING_DDL.contains("secret_id"),
            "the applied ci_0013 migration remains checksum-immutable"
        );
        assert!(
            !CREATE_CI_SECRET_DDL.contains("secret_binding"),
            "the applied ci_0023 migration remains checksum-immutable"
        );
        assert!(
            ALTER_SECRET_BINDING_ADD_INTEGRITY_DDL.contains("FOREIGN KEY (tenant_id, secret_id)")
        );
        assert!(ALTER_SECRET_BINDING_ADD_INTEGRITY_DDL
            .contains("REFERENCES ci_secret (tenant_id, secret_id)"));
        assert!(ALTER_SECRET_BINDING_ADD_INTEGRITY_DDL.contains("ON DELETE CASCADE"));
        assert!(ALTER_SECRET_BINDING_ADD_INTEGRITY_DDL
            .contains("DELETE FROM secret_binding WHERE secret_id IS NULL"));

        let migrations = ci_controlplane_migrations();
        let integrity = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_SECRET_BINDING_INTEGRITY_MIGRATION_ID)
            .expect("the binding-integrity migration is registered");
        assert_eq!(integrity.ddl, ALTER_SECRET_BINDING_ADD_INTEGRITY_DDL);
        let tombstone = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_SECRET_TOMBSTONE_MIGRATION_ID)
            .expect("the version-tombstone migration is registered");
        assert!(tombstone.ddl.contains("myelin_make_tenant_scoped"));
    }

    #[test]
    fn secret_version_high_water_migration_is_forward_only_and_backfills_history() {
        assert!(CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL
            .starts_with("CREATE TABLE IF NOT EXISTS ci_secret_version_high_water"));
        assert!(
            CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL.contains("PRIMARY KEY (tenant_id, secret_id)")
        );
        assert!(CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL
            .contains("SELECT tenant_id, region, secret_id, version FROM ci_secret"));
        assert!(CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL.contains(
            "SELECT tenant_id, region, secret_id, max_version AS version FROM ci_secret_tombstone"
        ));
        assert!(CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL.contains("max(version)"));
        assert!(!myelin_substrate::is_destructive(
            CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL
        ));

        let migrations = ci_controlplane_migrations();
        let tombstone_pos = migrations
            .0
            .iter()
            .position(|migration| migration.id == CI_SECRET_TOMBSTONE_MIGRATION_ID)
            .expect("managed tombstone migration");
        let high_water_pos = migrations
            .0
            .iter()
            .position(|migration| migration.id == CI_SECRET_VERSION_HIGH_WATER_MIGRATION_ID)
            .expect("universal high-water migration");
        assert_eq!(high_water_pos, tombstone_pos + 1);
        assert!(migrations.0[high_water_pos]
            .ddl
            .contains("myelin_make_tenant_scoped('ci_secret_version_high_water')"));
    }

    #[test]
    fn every_table_is_tenant_region_first() {
        for (_id, _table, ddl) in create_statements() {
            let tenant_pos = ddl.find("tenant_id").expect("tenant_id column");
            let region_pos = ddl.find("region").expect("region column");
            assert!(
                tenant_pos < region_pos,
                "tenant_id is the FIRST column (before region): {ddl}"
            );
            assert!(
                ddl.contains("PRIMARY KEY (tenant_id"),
                "the primary key is tenant-first: {ddl}"
            );
        }
    }

    #[test]
    fn drive_manifest_is_digest_checked_and_structurally_insert_only() {
        let ddl = CREATE_CI_DRIVE_MANIFEST_DDL;
        for required in [
            "CHECK (schema_version = 1)",
            "CHECK (manifest_digest ~ '^blake3:[0-9a-f]{64}$')",
            "UNIQUE (tenant_id, ci_run_id)",
            "REFERENCES ci_run(tenant_id, run_id)",
            "REVOKE UPDATE, DELETE ON ci_drive_manifest FROM myelin_app",
            "BEFORE UPDATE OR DELETE ON ci_drive_manifest",
            "RAISE EXCEPTION 'ci_drive_manifest is immutable'",
        ] {
            assert!(ddl.contains(required), "manifest DDL pins `{required}`");
        }
        assert!(
            !ddl.contains("secret_value") && !ddl.contains("token_jti"),
            "the immutable replay authority never stores secret values or minted token JTIs"
        );
    }

    #[test]
    fn job_accounting_is_complete_unique_and_structurally_insert_only() {
        let ddl = CREATE_CI_JOB_ACCOUNTING_DDL;
        for required in [
            "cpu_seconds",
            "mem_byte_seconds",
            "pricing_revision",
            "billed_minor_units",
            "refunded_minor_units",
            "UNIQUE (tenant_id, completion_receipt)",
            "REFERENCES ci_run(tenant_id, run_id)",
            "REVOKE UPDATE, DELETE ON ci_job_accounting FROM myelin_app",
            "BEFORE UPDATE OR DELETE ON ci_job_accounting",
            "RAISE EXCEPTION 'ci_job_accounting is immutable'",
        ] {
            assert!(ddl.contains(required), "accounting DDL pins `{required}`");
        }
        for required in [
            "skipped boolean NOT NULL DEFAULT false",
            "NOT skipped OR (NOT passed AND NOT timed_out)",
        ] {
            assert!(
                ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL.contains(required),
                "skipped-accounting ALTER pins `{required}`"
            );
        }
        for required in [
            "terminal_disposition text",
            "completion_receipt_v4 text",
            "completion_receipt_v4 ~ '^v4:[0-9a-f]{64}$'",
            "terminal_disposition IS NULL AND completion_receipt_v4 IS NULL",
            "cancelled_during_preparation",
            "UNIQUE (tenant_id, completion_receipt_v4)",
        ] {
            assert!(
                ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_DDL.contains(required),
                "v4 accounting ALTER pins `{required}`"
            );
        }
        for required in [
            "ci_job_accounting_terminal_disposition_v4_verdict",
            "terminal_disposition IS NULL",
            "WHEN 'workload_passed' THEN passed AND NOT timed_out AND NOT skipped",
            "WHEN 'cancelled_during_preparation' THEN NOT passed AND NOT timed_out AND skipped",
        ] {
            assert!(
                ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_VERDICT_DDL.contains(required),
                "v4 accounting verdict ALTER pins `{required}`"
            );
        }
    }

    #[test]
    fn parent_attempt_is_unique_fk_anchored_and_structurally_insert_only() {
        let ddl = CREATE_CI_JOB_PARENT_ATTEMPT_DDL;
        for required in [
            "PRIMARY KEY (tenant_id, region, job_id, lease_epoch, claim_nonce)",
            "UNIQUE (tenant_id, region, job_id, lease_epoch)",
            "UNIQUE (tenant_id, region, job_id, claim_nonce)",
            "max_parent_attempts          bigint NOT NULL CHECK (max_parent_attempts BETWEEN 1 AND 4294967295)",
            "REFERENCES ci_run(tenant_id, run_id)",
            "REVOKE UPDATE, DELETE ON ci_job_parent_attempt FROM myelin_app",
            "BEFORE UPDATE OR DELETE ON ci_job_parent_attempt",
            "RAISE EXCEPTION 'ci_job_parent_attempt is immutable'",
        ] {
            assert!(ddl.contains(required), "parent-attempt DDL pins `{required}`");
        }
    }

    #[test]
    fn prelaunch_usage_is_phase_restricted_fk_anchored_and_delete_free() {
        let ddl = CREATE_CI_JOB_PRELAUNCH_USAGE_DDL;
        for required in [
            "phase                     text NOT NULL CHECK (phase IN ('checkout_transport','checkout_materialization'))",
            "status                    text NOT NULL CHECK (status IN ('started','measured','sealed_ceiling'))",
            "PRIMARY KEY (tenant_id, region, job_id, lease_epoch, claim_nonce, phase)",
            "REFERENCES ci_job_parent_attempt (tenant_id, region, job_id, lease_epoch, claim_nonce)",
            "REVOKE DELETE ON ci_job_prelaunch_usage FROM myelin_app",
            "BEFORE UPDATE ON ci_job_prelaunch_usage",
        ] {
            assert!(ddl.contains(required), "prelaunch-usage DDL pins `{required}`");
        }
        assert!(
            !ddl.contains("exact_cpu_seconds <= ceiling_cpu_seconds")
                && !ddl.contains("exact_mem_byte_seconds <= ceiling_mem_byte_seconds"),
            "an honest over-ceiling measurement must never be rejected by a database constraint"
        );
    }

    #[test]
    fn credential_generation_is_purpose_unique_append_only_and_status_free() {
        let ddl = CREATE_CI_JOB_CREDENTIAL_GENERATION_DDL;
        for required in [
            "PRIMARY KEY (tenant_id, region, job_id, lease_epoch, claim_nonce, purpose)",
            "UNIQUE (tenant_id, region, generation_id)",
            "UNIQUE (tenant_id, region, jti)",
            "purpose                      text NOT NULL CHECK (purpose IN ('checkout_advertise','checkout_fetch','checkout_materialization','workload'))",
            "WHEN 'checkout_advertise' THEN phase_ordinal = 1",
            "WHEN 'checkout_fetch' THEN phase_ordinal = 2",
            "WHEN 'checkout_materialization' THEN phase_ordinal = 3",
            "WHEN 'workload' THEN phase_ordinal = 4",
            "CHECK (expires_at_epoch_secs > issued_at_epoch_secs)",
            "CHECK (expires_at_epoch_secs <= claim_expires_at_epoch_secs)",
            "CHECK (issued_at_epoch_secs >= claim_started_at_epoch_secs)",
            "binding_version              smallint NOT NULL CHECK (binding_version = 1)",
            "REFERENCES ci_run(tenant_id, run_id)",
            "REVOKE UPDATE, DELETE ON ci_job_credential_generation FROM myelin_app",
            "BEFORE UPDATE OR DELETE ON ci_job_credential_generation",
            "RAISE EXCEPTION 'ci_job_credential_generation is immutable'",
        ] {
            assert!(
                ddl.contains(required),
                "credential-generation DDL pins `{required}`"
            );
        }
        assert!(
            !ddl.contains("status"),
            "there is deliberately NO status column: current = the highest phase_ordinal"
        );
        assert!(
            !ddl.contains("REFERENCES ci_job_parent_attempt"),
            "advertise is minted in the resolver, BEFORE begin_parent_attempt can create the parent"
        );
        assert!(
            !ddl.contains("bearer") && !ddl.contains("token_material"),
            "the credential log stores the expected JTI and generation id, never bearer material"
        );
    }

    #[test]
    fn credential_generation_carries_no_scheduler_grant_migration() {
        for migration in &ci_controlplane_migrations().0 {
            if migration.ddl.contains("myelin_ci_region_scheduler") {
                assert!(
                    !migration.ddl.contains("ci_job_credential_generation"),
                    "migration {} grants the scheduler role access to the credential log",
                    migration.id
                );
            }
        }
    }

    #[test]
    fn prelaunch_seal_deadline_is_an_immutable_online_expand_with_a_partial_index() {
        let ddl = ALTER_CI_JOB_PRELAUNCH_USAGE_ADD_SEAL_DEADLINE_DDL;
        for required in [
            "ADD COLUMN IF NOT EXISTS seal_after timestamptz",
            "CHECK (seal_after IS NULL OR seal_after >= started_at) NOT VALID",
            "VALIDATE CONSTRAINT ci_job_prelaunch_usage_seal_after_order",
            "NEW.seal_after IS DISTINCT FROM OLD.seal_after",
            "identity, ceiling, and deadline are immutable",
        ] {
            assert!(
                ddl.contains(required),
                "seal-deadline DDL pins `{required}`"
            );
        }
        assert!(
            !ddl.contains("seal_after timestamptz NOT NULL"),
            "the hot-table expand remains nullable until a later bounded backfill/contract"
        );
        assert_eq!(
            CREATE_CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_DDL,
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_job_prelaunch_usage_seal_deadline_reaper \
ON ci_job_prelaunch_usage (region, seal_after) WHERE status = 'started' AND seal_after IS NOT NULL"
        );
    }

    #[test]
    fn the_backlog_probe_is_born_fence_owned_and_never_overwrites_a_divergent_function() {
        let ddl = CREATE_CI_PIPELINE_VERSION_BACKLOG_PROBE_DDL;
        for required in [
            "SET LOCAL ROLE myelin_ci_definition_fence;",
            "RESET ROLE;",
            "myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs",
            "SECURITY DEFINER",
            "SET search_path = pg_catalog",
            "SET row_security = off",
            "run scripts/pg-init/01-ci-definition-fence.sql as the database provisioning administrator",
            "passing migration_role=<DATABASE_MIGRATION_URL role>, then retry boot",
            "to_regprocedure",
            "diverges from the expected definition-fence probe",
            "REVOKE ALL PRIVILEGES ON TABLE public.workflow_run FROM myelin_ci_definition_fence",
            "GRANT SELECT (wf_type, wf_version, state) ON public.workflow_run",
            "FROM public.workflow_run",
            "state IN (''running'', ''waiting'')",
        ] {
            assert!(ddl.contains(required), "the backlog probe pins `{required}`");
        }
        assert!(
            !ddl.contains("BEGIN;") && !ddl.contains("COMMIT;"),
            "atomicity comes from the implicit multi-statement transaction; an explicit BEGIN would \
             leave the pooled migration connection in an aborted transaction block on refusal"
        );
        assert!(
            !ddl.contains("ALTER FUNCTION"),
            "the function is BORN fence-owned; ALTER FUNCTION ... OWNER TO would reintroduce the \
             silent-adoption hazard this shape exists to remove"
        );
        assert!(
            !ddl.contains("CREATE OR REPLACE FUNCTION"),
            "a blind replace would overwrite a divergent function instead of raising"
        );
        for forbidden in [
            "CREATE ROLE",
            "ALTER ROLE",
            "GRANT myelin_ci_definition_fence TO",
        ] {
            assert!(
                !ddl.contains(forbidden),
                "a migration must never provision cluster authority (`{forbidden}`) - it verifies \
                 and names the operator script"
            );
        }
        assert!(
            ddl.contains("TO myelin_app") && !ddl.contains("TO myelin_ci_region_scheduler"),
            "only the runtime role that registers wf_definition may execute the fence's probe"
        );
    }

    #[test]
    fn the_activation_readiness_probe_is_born_fence_owned_and_column_scoped_to_job_queue() {
        let ddl = CREATE_CI_V2_ACTIVATION_READINESS_PROBE_DDL;
        for required in [
            "SET LOCAL ROLE myelin_ci_definition_fence;",
            "RESET ROLE;",
            "myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count",
            "RETURNS bigint",
            "SECURITY DEFINER",
            "SET search_path = pg_catalog",
            "SET row_security = off",
            "run scripts/pg-init/01-ci-definition-fence.sql as the database provisioning administrator",
            "passing migration_role=<DATABASE_MIGRATION_URL role>, then retry boot",
            "to_regprocedure",
            "diverges from the expected activation-readiness probe",
            "REVOKE ALL PRIVILEGES ON TABLE public.job_queue FROM myelin_ci_definition_fence",
            "GRANT SELECT (region, state, claim_window_secs, reservation_write_version)",
            "ON public.job_queue TO myelin_ci_definition_fence",
            "SELECT count(*) FROM public.job_queue",
            "state <> ''terminal''",
            "claim_window_secs IS NULL OR reservation_write_version IS DISTINCT FROM 2",
        ] {
            assert!(
                ddl.contains(required),
                "the activation-readiness probe pins `{required}`"
            );
        }
        assert!(
            !ddl.contains("BEGIN;") && !ddl.contains("COMMIT;"),
            "atomicity comes from the implicit multi-statement transaction, as in ci_0020h"
        );
        assert!(
            !ddl.contains("ALTER FUNCTION") && !ddl.contains("CREATE OR REPLACE FUNCTION"),
            "the function is BORN fence-owned and never blind-replaced"
        );
        for forbidden in [
            "CREATE ROLE",
            "ALTER ROLE",
            "GRANT myelin_ci_definition_fence TO",
        ] {
            assert!(
                !ddl.contains(forbidden),
                "a migration must never provision cluster authority (`{forbidden}`)"
            );
        }
        assert!(
            ddl.contains("TO myelin_app") && !ddl.contains("myelin_ci_region_scheduler"),
            "only myelin_app may execute the readiness probe; the regional scheduler gets no \
             cross-region authority"
        );
        for forbidden_column in [
            "tenant_id",
            "idem_token",
            "lease_owner",
            "job_id",
            "meter_to",
        ] {
            assert!(
                !ddl.contains(&format!(
                    "GRANT SELECT (region, state, claim_window_secs, reservation_write_version, {forbidden_column}"
                )),
                "the readiness probe grant must never widen to `{forbidden_column}`"
            );
        }
    }

    #[test]
    fn reservation_write_version_expand_is_online_and_marks_exactly_two() {
        let expand = ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL;
        assert!(expand.contains("ADD COLUMN IF NOT EXISTS reservation_write_version smallint"));
        assert!(expand.contains("ADD CONSTRAINT job_queue_reservation_write_version_marker"));
        assert!(expand.contains("CHECK (reservation_write_version = 2) NOT VALID"));
        assert!(
            !expand.contains("reservation_write_version smallint NOT NULL"),
            "legacy/V3 writers leave the marker NULL; the hot-table expand stays nullable"
        );
        assert!(
            !myelin_substrate::is_blocking_alter(expand)
                && !myelin_substrate::is_blocking_alter(
                    VALIDATE_JOB_QUEUE_RESERVATION_WRITE_VERSION_DDL
                ),
            "neither half of the reservation-marker expand may take a blocking lock on the hot queue"
        );
        assert!(expand.contains("pg_get_constraintdef"));
        assert!(expand.contains("DIVERGENT definition"));
        assert!(expand.contains("CHECK ((reservation_write_version = 2)) NOT VALID"));
        assert_eq!(
            VALIDATE_JOB_QUEUE_RESERVATION_WRITE_VERSION_DDL,
            "ALTER TABLE job_queue VALIDATE CONSTRAINT job_queue_reservation_write_version_marker"
        );
    }

    #[test]
    fn the_activation_readiness_index_is_concurrent_and_covers_unsafe_rows() {
        let index = CREATE_JOB_QUEUE_ACTIVATION_READINESS_INDEX_DDL;
        assert!(index
            .starts_with("CREATE INDEX CONCURRENTLY IF NOT EXISTS job_queue_activation_readiness"));
        assert_eq!(
            index.matches(';').count(),
            0,
            "a concurrent index must be one top-level command"
        );
        for required in [
            "ON job_queue (region)",
            "WHERE state <> 'terminal'",
            "claim_window_secs IS NULL OR reservation_write_version IS DISTINCT FROM 2",
        ] {
            assert!(
                index.contains(required),
                "the readiness index pins `{required}`"
            );
        }
    }

    #[test]
    fn the_definition_fence_provisioning_script_is_the_named_operator_remediation() {
        let script = include_str!("../../../scripts/pg-init/01-ci-definition-fence.sql");
        for required in [
            "CREATE ROLE myelin_ci_definition_fence",
            "NOLOGIN NOSUPERUSER BYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT",
            "CREATE SCHEMA IF NOT EXISTS myelin_ci_security AUTHORIZATION myelin_ci_definition_fence",
            "WITH ADMIN FALSE, INHERIT FALSE, SET TRUE",
            "REVOKE ALL ON SCHEMA myelin_ci_security FROM PUBLIC",
        ] {
            assert!(script.contains(required), "the fence provisioning pins `{required}`");
        }
        assert!(
            !script.contains("GRANT SELECT") && !script.contains("ON TABLE public.workflow_run TO"),
            "table access belongs to ci_0020h, which runs only once workflow_run exists"
        );
        assert!(
            CREATE_CI_PIPELINE_VERSION_BACKLOG_PROBE_DDL
                .contains("scripts/pg-init/01-ci-definition-fence.sql"),
            "the migration's refusal must name this exact script"
        );
        let conventions = include_str!("../../../scripts/pg-init/00-rls-conventions.sql");
        assert!(
            !conventions.contains("myelin_ci_definition_fence"),
            "the fence provisioning moved to its own operator-runnable file"
        );
    }

    #[test]
    fn the_cutover_fence_row_seed_is_additive_and_never_admissible() {
        for (version, ddl) in [
            (2, SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL),
            (3, SEED_CI_PIPELINE_V3_CUTOVER_FENCE_ROW_DDL),
            (4, SEED_CI_PIPELINE_V4_CUTOVER_FENCE_ROW_DDL),
            (5, SEED_CI_PIPELINE_V5_CUTOVER_FENCE_ROW_DDL),
            (6, SEED_CI_PIPELINE_V6_CUTOVER_FENCE_ROW_DDL),
        ] {
            assert!(ddl.contains("ON CONFLICT (wf_type, version) DO NOTHING"));
            assert!(ddl.contains(&format!(" {version},")));
            assert!(
                ddl.contains("'retired'"),
                "a freshly seeded predecessor must never be admissible for a start"
            );
            assert!(
                ddl.contains("sentinel:"),
                "the seeded hash must be unmistakable for a real source-derived pin"
            );
            assert!(
                !ddl.contains("DO UPDATE"),
                "the seed never rewrites an existing row"
            );
        }
        assert!(
            SEED_CI_PIPELINE_V6_CUTOVER_FENCE_ROW_DDL.contains(&format!(
                "\n  {},\n",
                crate::ci_runtime_composition::CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION
            )),
            "the binary's production predecessor must have an append-only fence seed"
        );
    }

    #[test]
    fn claim_window_expand_is_online_and_its_check_bound_matches_the_rust_maximum() {
        let ddl = ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL;
        assert!(ddl.contains("ADD COLUMN IF NOT EXISTS claim_window_secs bigint"));
        assert!(ddl.contains("ADD CONSTRAINT job_queue_claim_window_range"));
        assert!(ddl.contains("NOT VALID"));
        assert!(
            ddl.contains(&format!(
                "CHECK (claim_window_secs BETWEEN 1 AND {})",
                crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS
            )),
            "the durable CHECK bound must be the literal form of MAX_CI_JOB_CLAIM_WINDOW_SECS"
        );
        assert!(
            !ddl.contains("claim_window_secs bigint NOT NULL"),
            "the hot-table expand stays nullable until a later bounded backfill/contract"
        );
        assert!(
            !myelin_substrate::is_blocking_alter(ddl)
                && !myelin_substrate::is_blocking_alter(VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL),
            "neither half of the claim-window expand may take a blocking lock on the hot queue"
        );
        assert_eq!(
            VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL,
            "ALTER TABLE job_queue VALIDATE CONSTRAINT job_queue_claim_window_range"
        );
        assert!(ddl.contains("pg_get_constraintdef"));
        assert!(ddl.contains("DIVERGENT definition"));
        assert!(
            ddl.contains(&format!(
                "(claim_window_secs >= 1) AND (claim_window_secs <= {})",
                crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS
            )),
            "the expected-definition literal must carry the same bound as the CHECK it guards"
        );
        assert!(
            !ddl.contains("EXCEPTION WHEN duplicate_object THEN\n  NULL"),
            "a same-named constraint must never be adopted without comparing its definition"
        );
    }

    #[test]
    fn migration_follow_ons_keep_their_dependency_order() {
        let ids: Vec<&str> = ci_controlplane_migrations()
            .0
            .iter()
            .map(|m| m.id)
            .collect();

        let position = |migration_id: &str| {
            ids.iter()
                .position(|id| *id == migration_id)
                .unwrap_or_else(|| panic!("migration {migration_id} is in the set"))
        };
        let claim_window = position(CI_JOB_QUEUE_CLAIM_WINDOW_MIGRATION_ID);
        assert!(
            claim_window > position(CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_MIGRATION_ID),
            "the expand follows the last previously shipped migration"
        );
        assert_eq!(
            &ids[claim_window..claim_window + 2],
            &[
                CI_JOB_QUEUE_CLAIM_WINDOW_MIGRATION_ID,
                CI_JOB_QUEUE_CLAIM_WINDOW_VALIDATE_MIGRATION_ID,
            ],
            "claim-window enforcement expands before it validates"
        );

        let appended = position(CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID);
        assert_eq!(
            &ids[appended..],
            &[
                CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID,
                CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_MIGRATION_ID,
                CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_VALIDATE_MIGRATION_ID,
                CI_JOB_QUEUE_ACTIVATION_READINESS_INDEX_MIGRATION_ID,
                CI_V2_ACTIVATION_READINESS_PROBE_MIGRATION_ID,
                CI_PIPELINE_V3_CUTOVER_FENCE_ROW_MIGRATION_ID,
                CI_SECRET_MIGRATION_ID,
                CI_SECRET_ADMIN_SCOPE_MIGRATION_ID,
                CI_SECRET_ADMIN_UNIQUE_MIGRATION_ID,
                CI_SECRET_BINDING_INTEGRITY_MIGRATION_ID,
                CI_SECRET_TOMBSTONE_MIGRATION_ID,
                CI_SECRET_VERSION_HIGH_WATER_MIGRATION_ID,
                CI_JOB_ACCOUNTING_DISPOSITION_V4_SECRET_RESOLUTION_MIGRATION_ID,
                CI_RUN_SOURCE_REF_MIGRATION_ID,
                CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID,
                CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID,
                CI_RUN_BRANCH_SCOPE_EXPAND_MIGRATION_ID,
                CI_RUN_BRANCH_SCOPE_VALIDATE_MIGRATION_ID,
                CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID,
                CI_PIPELINE_V4_CUTOVER_FENCE_ROW_MIGRATION_ID,
                CI_PIPELINE_V5_CUTOVER_FENCE_ROW_MIGRATION_ID,
                CI_PIPELINE_V6_CUTOVER_FENCE_ROW_MIGRATION_ID,
            ],
            "the append-only tail retains every expand → validate → contract dependency"
        );
    }

    #[test]
    fn source_ref_contract_repairs_intermediate_schemas_without_rewriting_history() {
        assert!(
            ALTER_CI_RUN_ADD_SOURCE_REF_DDL
                .starts_with("ALTER TABLE ci_run ADD COLUMN IF NOT EXISTS source_ref text"),
            "the immutable migration that shipped the column remains byte-for-byte available"
        );

        let repair = REPAIR_CI_RUN_SOURCE_REF_CONSTRAINT_DDL;
        for required in [
            "pg_get_constraintdef",
            "RENAME CONSTRAINT %I TO ci_run_source_ref_shape",
            "ADD CONSTRAINT ci_run_source_ref_shape",
            "NOT VALID",
            "DIVERGENT definition",
        ] {
            assert!(
                repair.contains(required),
                "the repair must contain `{required}`"
            );
        }
        assert!(
            !repair.contains("DROP CONSTRAINT"),
            "an equivalent check is adopted without a gap in enforcement"
        );
        assert_eq!(
            VALIDATE_CI_RUN_SOURCE_REF_CONSTRAINT_DDL,
            "ALTER TABLE ci_run VALIDATE CONSTRAINT ci_run_source_ref_shape"
        );
    }

    #[test]
    fn branch_scope_expands_to_pull_requests_before_the_old_check_retires() {
        let expand = EXPAND_CI_RUN_BRANCH_SCOPE_DDL;
        for required in [
            "ADD CONSTRAINT ci_run_source_ref_shape_v2",
            "trigger_kind IN ('push', 'pull_request')",
            "refs/heads/",
            "NOT VALID",
        ] {
            assert!(
                expand.contains(required),
                "the expand contains `{required}`"
            );
        }
        assert_eq!(
            VALIDATE_CI_RUN_BRANCH_SCOPE_DDL,
            "ALTER TABLE ci_run VALIDATE CONSTRAINT ci_run_source_ref_shape_v2"
        );
        assert_eq!(
            CONTRACT_CI_RUN_BRANCH_SCOPE_DDL,
            "ALTER TABLE ci_run DROP CONSTRAINT ci_run_source_ref_shape; \
             ALTER TABLE ci_run RENAME CONSTRAINT ci_run_source_ref_shape_v2 TO ci_run_source_ref_shape"
        );
    }

    #[test]
    fn the_migration_set_is_forward_only_and_rls_scoped() {
        let migrations = ci_controlplane_migrations();
        assert_eq!(
            migrations.0.len(),
            78,
            "the complete append-only schema includes the current predecessor fence seed"
        );
        fn constraint_names(upper_ddl: &str, keyword: &str) -> Vec<String> {
            upper_ddl
                .match_indices(keyword)
                .map(|(i, _)| {
                    upper_ddl[i + keyword.len()..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_end_matches(',')
                        .to_string()
                })
                .collect()
        }
        for m in &migrations.0 {
            assert!(
                !myelin_substrate::is_destructive(m.ddl),
                "migration {} is forward-only (no DROP): {}",
                m.id,
                m.ddl
            );
            let upper = m.ddl.to_ascii_uppercase();
            for destructive in [
                "DROP TABLE",
                "DROP COLUMN",
                "DROP SCHEMA",
                "DROP FUNCTION",
                "DROP ROLE",
                "DROP INDEX",
                "DROP DATABASE",
                "DROP OWNED",
            ] {
                assert!(
                    !upper.contains(destructive),
                    "no `{destructive}` in migration {}",
                    m.id
                );
            }
            let dropped = constraint_names(&upper, "DROP CONSTRAINT");
            if !dropped.is_empty() {
                const CONSTRAINT_REPLACEMENT_ALLOWLIST: &[&str] = &[
                    CI_JOB_ACCOUNTING_DISPOSITION_V4_SECRET_RESOLUTION_MIGRATION_ID,
                    CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID,
                ];
                assert!(
                    CONSTRAINT_REPLACEMENT_ALLOWLIST.contains(&m.id),
                    "migration {} uses DROP CONSTRAINT but is not on the audited constraint-replacement \
                     allowlist - add it there AND pin its exact DDL below, only after confirming every \
                     re-added constraint is a STRICT SUPERSET of the one it replaces",
                    m.id
                );
                if m.id == CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID {
                    assert_eq!(m.ddl, CONTRACT_CI_RUN_BRANCH_SCOPE_DDL);
                    assert!(
                        upper.contains(
                            "RENAME CONSTRAINT CI_RUN_SOURCE_REF_SHAPE_V2 TO CI_RUN_SOURCE_REF_SHAPE"
                        ),
                        "the validated superset assumes the canonical constraint name"
                    );
                } else {
                    let added = constraint_names(&upper, "ADD CONSTRAINT");
                    for name in &dropped {
                        assert!(
                            added.contains(name),
                            "migration {} drops constraint {name} without re-adding it in the same DDL \
                             (only a data-preserving DROP+ADD replacement is allowed)",
                            m.id
                        );
                    }
                }
            }
            if m.ddl.contains("CREATE TABLE") {
                assert!(
                    m.ddl.contains("myelin_make_tenant_scoped"),
                    "table migration {} installs the platform RLS policy",
                    m.id
                );
            } else if m.id == CI_JOB_RUN_LEDGER_VALIDATION_MIGRATION_ID {
                assert_eq!(m.ddl, VALIDATE_CI_JOB_RUN_LEDGER_INDEX_DDL);
            } else if m.id == CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL);
            } else if m.id == CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL);
            } else if m.id == CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL);
            } else if m.id == CI_RUN_SOURCE_REF_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_RUN_ADD_SOURCE_REF_DDL);
            } else if m.id == CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID {
                assert_eq!(m.ddl, REPAIR_CI_RUN_SOURCE_REF_CONSTRAINT_DDL);
            } else if m.id == CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID {
                assert_eq!(m.ddl, VALIDATE_CI_RUN_SOURCE_REF_CONSTRAINT_DDL);
            } else if m.id == CI_RUN_BRANCH_SCOPE_EXPAND_MIGRATION_ID {
                assert_eq!(m.ddl, EXPAND_CI_RUN_BRANCH_SCOPE_DDL);
            } else if m.id == CI_RUN_BRANCH_SCOPE_VALIDATE_MIGRATION_ID {
                assert_eq!(m.ddl, VALIDATE_CI_RUN_BRANCH_SCOPE_DDL);
            } else if m.id == CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID {
                assert_eq!(m.ddl, CONTRACT_CI_RUN_BRANCH_SCOPE_DDL);
            } else if m.id == CI_JOB_SPEC_STAGE_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_JOB_SPEC_ADD_STAGE_DDL);
            } else if m.id == CI_JOB_ACCOUNTING_SKIPPED_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL);
            } else if m.id == CI_JOB_ACCOUNTING_DISPOSITION_V4_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_DDL);
            } else if m.id == CI_JOB_ACCOUNTING_DISPOSITION_V4_VERDICT_MIGRATION_ID {
                assert_eq!(
                    m.ddl,
                    ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_VERDICT_DDL
                );
            } else if m.id == CI_JOB_ACCOUNTING_DISPOSITION_V4_SECRET_RESOLUTION_MIGRATION_ID {
                assert_eq!(
                    m.ddl,
                    ALTER_CI_JOB_ACCOUNTING_DISPOSITION_V4_SECRET_RESOLUTION_DDL
                );
            } else if m.id == CI_JOB_QUEUE_COMPLETION_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL);
            } else if m.id == CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL);
            } else if m.id == CI_JOB_QUEUE_CLAIM_TIME_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL);
            } else if m.id == CI_JOB_QUEUE_RETRY_ATTEMPTS_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL);
            } else if m.id == CI_JOB_QUEUE_CLAIM_WINDOW_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL);
            } else if m.id == CI_JOB_QUEUE_CLAIM_WINDOW_VALIDATE_MIGRATION_ID {
                assert_eq!(m.ddl, VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL);
            } else if m.id == CI_SCHEDULER_WORKFLOW_VERSION_GRANT_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_WORKFLOW_VERSION_DDL);
            } else if m.id == CI_PIPELINE_VERSION_BACKLOG_PROBE_MIGRATION_ID {
                assert_eq!(m.ddl, CREATE_CI_PIPELINE_VERSION_BACKLOG_PROBE_DDL);
            } else if m.id == CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID {
                assert_eq!(m.ddl, SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL);
            } else if m.id == CI_PIPELINE_V3_CUTOVER_FENCE_ROW_MIGRATION_ID {
                assert_eq!(m.ddl, SEED_CI_PIPELINE_V3_CUTOVER_FENCE_ROW_DDL);
            } else if m.id == CI_PIPELINE_V4_CUTOVER_FENCE_ROW_MIGRATION_ID {
                assert_eq!(m.ddl, SEED_CI_PIPELINE_V4_CUTOVER_FENCE_ROW_DDL);
            } else if m.id == CI_PIPELINE_V5_CUTOVER_FENCE_ROW_MIGRATION_ID {
                assert_eq!(m.ddl, SEED_CI_PIPELINE_V5_CUTOVER_FENCE_ROW_DDL);
            } else if m.id == CI_PIPELINE_V6_CUTOVER_FENCE_ROW_MIGRATION_ID {
                assert_eq!(m.ddl, SEED_CI_PIPELINE_V6_CUTOVER_FENCE_ROW_DDL);
            } else if m.id == CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL);
            } else if m.id == CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_VALIDATE_MIGRATION_ID {
                assert_eq!(m.ddl, VALIDATE_JOB_QUEUE_RESERVATION_WRITE_VERSION_DDL);
            } else if m.id == CI_V2_ACTIVATION_READINESS_PROBE_MIGRATION_ID {
                assert_eq!(m.ddl, CREATE_CI_V2_ACTIVATION_READINESS_PROBE_DDL);
            } else if m.id == CI_SCHEDULER_LEASE_EPOCH_GRANT_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_LEASE_EPOCH_DDL);
            } else if m.id == CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_CLAIM_NONCE_DDL);
            } else if m.id == CI_SCHEDULER_CLAIM_TIME_GRANT_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_CLAIM_TIME_DDL);
            } else if m.id == CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_CI_RUN_DISCOVERY_DDL);
            } else if m.id == CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_CI_WORKFLOW_DISCOVERY_DDL);
            } else if m.id == CI_SCHEDULER_CI_RUN_WORKFLOW_ID_GRANT_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_CI_RUN_WORKFLOW_ID_DDL);
            } else if m.id == CI_SCHEDULER_CI_JOB_REAP_RESET_GRANT_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_CI_JOB_REAP_RESET_DDL);
            } else if m.id == CI_SCHEDULER_PRELAUNCH_USAGE_REAP_GRANT_MIGRATION_ID {
                assert_eq!(m.ddl, GRANT_SCHEDULER_CI_JOB_PRELAUNCH_USAGE_REAP_DDL);
            } else if m.id == CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_JOB_PRELAUNCH_USAGE_ADD_SEAL_DEADLINE_DDL);
            } else if m.id == CI_SECRET_ADMIN_SCOPE_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_SECRET_ADD_ADMIN_SCOPE_DDL);
            } else if m.id == CI_SECRET_BINDING_INTEGRITY_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_SECRET_BINDING_ADD_INTEGRITY_DDL);
            } else if m.id == CI_REGION_SCHEDULER_RLS_MIGRATION_ID {
                assert_eq!(m.ddl, CREATE_CI_REGION_SCHEDULER_RLS_DDL);
            } else {
                assert!(
                    m.ddl.starts_with("CREATE INDEX CONCURRENTLY")
                        || m.ddl.starts_with("CREATE UNIQUE INDEX CONCURRENTLY"),
                    "the only other non-table migrations are concurrent indexes: {}",
                    m.id
                );
                assert_eq!(
                    m.ddl.matches(';').count(),
                    0,
                    "a concurrent index must be one top-level command: {}",
                    m.id
                );
            }
        }
    }

    #[test]
    fn the_runner_admits_the_whole_set() {
        use myelin_substrate::MigrationRunner;
        let migrations = ci_controlplane_migrations();
        let mut runner = MigrationRunner::new();
        runner
            .run(&migrations, &ci_controlplane_hot_tables())
            .expect("the full CI control-plane schema applies forward-only");
        assert_eq!(
            runner.applied().len(),
            79,
            "the runner applied the complete schema plus every additive follow-on"
        );
        assert_eq!(
            runner.applied()[0],
            "ci_0001_ci_run",
            "ci_run is applied first (FK order)"
        );
    }

    #[test]
    fn region_scheduler_boundary_is_additive_restrictive_and_least_privilege() {
        let migrations = ci_controlplane_migrations();
        let workflow_discovery = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID)
            .expect("the workflow-route grant remains present");
        assert_eq!(
            workflow_discovery.id,
            CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID
        );
        assert_eq!(workflow_discovery.table, Some("workflow_run"));
        assert_eq!(
            workflow_discovery.ddl,
            GRANT_SCHEDULER_CI_WORKFLOW_DISCOVERY_DDL
        );
        let run_surface_index = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID)
            .expect("the CT-005 run-surface index remains present");
        assert_eq!(
            run_surface_index.id,
            CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID
        );
        assert_eq!(run_surface_index.table, Some(CI_RUN_TABLE));
        assert_eq!(
            run_surface_index.ddl,
            CREATE_CI_RUN_SURFACE_REPO_CREATED_INDEX_DDL
        );
        let retry_attempts = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_JOB_QUEUE_RETRY_ATTEMPTS_MIGRATION_ID)
            .expect("the retry-attempt accrual column remains present");
        assert_eq!(retry_attempts.id, CI_JOB_QUEUE_RETRY_ATTEMPTS_MIGRATION_ID);
        assert_eq!(retry_attempts.table, Some(JOB_QUEUE_TABLE));
        assert_eq!(retry_attempts.ddl, ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL);
        let workflow_id_grant = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_SCHEDULER_CI_RUN_WORKFLOW_ID_GRANT_MIGRATION_ID)
            .expect("the workflow-identity grant is appended under a fresh id");
        assert_eq!(
            workflow_id_grant.id,
            CI_SCHEDULER_CI_RUN_WORKFLOW_ID_GRANT_MIGRATION_ID
        );
        assert_eq!(workflow_id_grant.table, Some(CI_RUN_TABLE));
        assert_eq!(
            workflow_id_grant.ddl,
            GRANT_SCHEDULER_CI_RUN_WORKFLOW_ID_DDL
        );
        let ci_job_reap_reset_grant = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_SCHEDULER_CI_JOB_REAP_RESET_GRANT_MIGRATION_ID)
            .expect("the ci_job reap-reset grant remains present under its immutable id");
        assert_eq!(
            ci_job_reap_reset_grant.id,
            CI_SCHEDULER_CI_JOB_REAP_RESET_GRANT_MIGRATION_ID
        );
        assert_eq!(ci_job_reap_reset_grant.table, Some(CI_JOB_TABLE));
        assert_eq!(
            ci_job_reap_reset_grant.ddl,
            GRANT_SCHEDULER_CI_JOB_REAP_RESET_DDL
        );
        let active_workflow_index = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_RUN_ACTIVE_WORKFLOW_INDEX_MIGRATION_ID)
            .expect("the active CI workflow lookup is indexed");
        assert_eq!(active_workflow_index.table, Some(CI_RUN_TABLE));
        assert_eq!(
            active_workflow_index.ddl,
            CREATE_CI_RUN_ACTIVE_WORKFLOW_INDEX_DDL
        );
        for required in [
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_run_active_workflow",
            "ON ci_run (tenant_id, region, wf_run_id)",
            "WHERE state = 'running'",
        ] {
            assert!(active_workflow_index.ddl.contains(required));
        }
        let discovery = migrations
            .0
            .iter()
            .find(|migration| migration.id == CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID)
            .expect("the scheduler ci_run discovery grant remains present");
        assert_eq!(discovery.id, CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID);
        assert_eq!(discovery.table, Some(CI_RUN_TABLE));
        assert_eq!(discovery.ddl, GRANT_SCHEDULER_CI_RUN_DISCOVERY_DDL);
        let discovery_index = migrations
            .0
            .iter()
            .find(|m| m.id == CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID)
            .expect("queued-run discovery index remains present");
        assert_eq!(discovery_index.table, Some(CI_RUN_TABLE));
        assert_eq!(discovery_index.ddl, CREATE_CI_RUN_QUEUED_REGION_INDEX_DDL);
        for required in [
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_run_queued_region",
            "ON ci_run (region, created_at, run_id)",
            "INCLUDE (tenant_id)",
            "WHERE state = 'queued'",
        ] {
            assert!(
                discovery_index.ddl.contains(required),
                "queued-run discovery index pins `{required}`"
            );
        }
        let running_index = migrations
            .0
            .iter()
            .find(|m| m.id == CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID)
            .expect("active-workflow recovery index remains present");
        assert_eq!(running_index.table, Some("workflow_run"));
        assert_eq!(
            running_index.ddl,
            CREATE_CI_WORKFLOW_ACTIVE_REGION_INDEX_DDL
        );
        for required in [
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_workflow_active_region",
            "ON workflow_run (region, created_at, tenant_id, run_id)",
            "INCLUDE (partition)",
            "WHERE wf_type = 'ci.pipeline' AND state IN ('running', 'waiting')",
        ] {
            assert!(running_index.ddl.contains(required));
        }
        let epoch_grant = migrations
            .0
            .iter()
            .find(|m| m.id == CI_SCHEDULER_LEASE_EPOCH_GRANT_MIGRATION_ID)
            .expect("immutable epoch grant remains present");
        assert_eq!(epoch_grant.ddl, GRANT_SCHEDULER_LEASE_EPOCH_DDL);
        let nonce_grant = migrations
            .0
            .iter()
            .find(|m| m.id == CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID)
            .expect("immutable nonce grant remains present");
        assert_eq!(nonce_grant.ddl, GRANT_SCHEDULER_CLAIM_NONCE_DDL);
        let claim_time_grant = migrations
            .0
            .iter()
            .find(|m| m.id == CI_SCHEDULER_CLAIM_TIME_GRANT_MIGRATION_ID)
            .expect("claim-time grant is present");
        assert_eq!(claim_time_grant.ddl, GRANT_SCHEDULER_CLAIM_TIME_DDL);
        let scheduler = migrations
            .0
            .iter()
            .find(|m| m.id == CI_REGION_SCHEDULER_RLS_MIGRATION_ID)
            .expect("scheduler boundary is present");
        assert_eq!(scheduler.table, Some(JOB_QUEUE_TABLE));
        assert_eq!(scheduler.ddl, CREATE_CI_REGION_SCHEDULER_RLS_DDL);

        let ddl = scheduler.ddl;
        assert_eq!(ddl.matches("AS PERMISSIVE").count(), 2);
        assert_eq!(ddl.matches("AS RESTRICTIVE").count(), 2);
        assert_eq!(
            ddl.matches("TO myelin_ci_region_scheduler").count(),
            7,
            "four role-targeted policies plus three role-targeted grants"
        );
        for required in [
            "current_setting('myelin.tenant_id', true) = ''",
            "region = public.myelin_ci_scheduler_region()",
            "region = current_setting('myelin.region', true)",
            "GRANT SELECT ON job_queue",
            "GRANT UPDATE (state, lease_owner, lease_expires) ON job_queue",
            "GRANT SELECT ON fair_deficit",
        ] {
            assert!(
                ddl.contains(required),
                "scheduler boundary pins `{required}`"
            );
        }
        for forbidden in [
            "GRANT INSERT",
            "GRANT DELETE",
            "GRANT UPDATE ON fair_deficit",
        ] {
            assert!(
                !ddl.contains(forbidden),
                "scheduler boundary forbids `{forbidden}`"
            );
        }

        let old_ids: Vec<&str> = create_statements().iter().map(|(id, _, _)| *id).collect();
        assert!(!old_ids.contains(&CI_REGION_SCHEDULER_RLS_MIGRATION_ID));

        let discovery_ddl = discovery.ddl;
        assert_eq!(discovery_ddl.matches("AS PERMISSIVE").count(), 1);
        assert_eq!(discovery_ddl.matches("AS RESTRICTIVE").count(), 1);
        for required in [
            "current_setting('myelin.tenant_id', true) = ''",
            "region = public.myelin_ci_scheduler_region()",
            "region = current_setting('myelin.region', true)",
            "GRANT SELECT (tenant_id, region, state, created_at, run_id) ON ci_run",
        ] {
            assert!(
                discovery_ddl.contains(required),
                "ci_run discovery boundary pins `{required}`"
            );
        }
        for required in [
            "CREATE POLICY myelin_ci_scheduler_workflow_discovery_access ON workflow_run",
            "AS PERMISSIVE FOR SELECT TO myelin_ci_region_scheduler",
            "CREATE POLICY myelin_ci_scheduler_workflow_discovery_guard ON workflow_run",
            "AS RESTRICTIVE FOR SELECT TO myelin_ci_region_scheduler",
            "current_setting('myelin.tenant_id', true) = ''",
            "region = public.myelin_ci_scheduler_region()",
            "GRANT SELECT (tenant_id, region, run_id, wf_type, state, partition, created_at)",
            "ON workflow_run TO myelin_ci_region_scheduler",
        ] {
            assert!(workflow_discovery.ddl.contains(required));
        }
        for forbidden in [
            "GRANT INSERT",
            "GRANT UPDATE",
            "GRANT DELETE",
            "GRANT SELECT ON ci_run",
        ] {
            assert!(
                !discovery_ddl.contains(forbidden),
                "ci_run discovery boundary forbids `{forbidden}`"
            );
        }
    }

    #[test]
    fn postgres_init_scheduler_identity_is_server_mapped_and_private() {
        let init = include_str!("../../../scripts/pg-init/00-rls-conventions.sql");
        for required in [
            "CREATE ROLE myelin_ci_region_scheduler NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT",
            "CREATE ROLE myelin_ci_scheduler_fr_par LOGIN PASSWORD 'myelin_ci_scheduler_dev_pw'",
            "WITH INHERIT TRUE, SET FALSE",
            "REVOKE myelin_ci_region_scheduler FROM myelin_app",
            "WHERE mapping.session_role = session_user::name",
            "SECURITY DEFINER",
            "SET search_path = pg_catalog",
            "REVOKE ALL ON FUNCTION public.myelin_ci_scheduler_region() FROM PUBLIC",
            "GRANT EXECUTE ON FUNCTION public.myelin_ci_scheduler_region() TO myelin_ci_region_scheduler",
        ] {
            assert!(init.contains(required), "Postgres init pins `{required}`");
        }
        assert!(init.contains("VALUES ('myelin_ci_scheduler_fr_par', 'fr-par')"));
        assert_eq!(
            init.matches("REVOKE ALL ON TABLE public.myelin_ci_scheduler_region_map FROM")
                .count(),
            4,
            "mapping table is hidden from PUBLIC, app, capability, and login"
        );
    }

    #[test]
    fn a_destructive_rollback_is_refused() {
        use myelin_substrate::MigrationRunner;
        let bad = Migrations::of([Migration::plain("ci_9999_drop", "DROP TABLE ci_run")]);
        let mut runner = MigrationRunner::new();
        let e = runner
            .run(&bad, &ci_controlplane_hot_tables())
            .expect_err("a DROP must be refused");
        assert!(
            e.0.contains("forward-only"),
            "the refusal names forward-only: {}",
            e.0
        );
    }

    #[test]
    fn the_seven_hot_tables_are_declared() {
        let hot = ci_controlplane_hot_tables();
        for t in [
            JOB_QUEUE_TABLE,
            LOG_SEGMENT_TABLE,
            CI_COST_EVENT_TABLE,
            CHECK_ATTEMPT_TABLE,
            CI_RUN_CHECK_ATTEMPT_TABLE,
            CI_JOB_PARENT_ATTEMPT_TABLE,
            CI_JOB_PRELAUNCH_USAGE_TABLE,
            CI_JOB_CREDENTIAL_GENERATION_TABLE,
        ] {
            assert!(hot.is_hot(t), "`{t}` is declared hot (arch 01 §3)");
        }
        assert!(
            !hot.is_hot(ENVIRONMENT_TABLE),
            "environment is NOT a hot table (low write rate)"
        );
    }

    #[test]
    fn the_three_job_queue_indexes_carry_their_predicates() {
        let by_name = |n: &str| {
            CREATE_JOB_QUEUE_INDEXES_DDL
                .iter()
                .find(|(name, _)| *name == n)
                .map(|(_, ddl)| *ddl)
                .unwrap()
        };
        let claimable = by_name(JQ_CLAIMABLE_INDEX);
        assert!(
            claimable.contains("(region, lane, enqueued_at)"),
            "jq_claimable keys (region, lane, enqueued_at) - the in-region claim order"
        );
        assert!(
            claimable.contains("WHERE state = 'queued'"),
            "jq_claimable is queued-only (the claim surface)"
        );
        let serialize = by_name(JQ_SERIALIZE_INDEX);
        assert!(
            serialize.contains("UNIQUE")
                && serialize.contains("concurrency_group")
                && serialize.contains("deploy:%"),
            "jq_serialize is the deploy-serialize running-unique"
        );
        let idem = by_name(JQ_IDEM_INDEX);
        assert!(
            idem.contains("UNIQUE") && idem.contains("(tenant_id, idem_token)"),
            "jq_idem is the per-tenant enqueue-dedup unique"
        );
        let migrations = ci_controlplane_migrations();
        for name in [JQ_CLAIMABLE_INDEX, JQ_SERIALIZE_INDEX, JQ_IDEM_INDEX] {
            let index = migrations
                .0
                .iter()
                .find(|migration| migration.ddl.contains(name))
                .unwrap_or_else(|| panic!("index `{name}` has a migration"));
            assert_eq!(index.table, Some(JOB_QUEUE_TABLE));
            assert_eq!(index.ddl.matches(';').count(), 0);
        }
    }

    #[test]
    fn ci_job_run_ledger_index_is_exact_cell_separate_and_ordered() {
        assert_eq!(CI_JOB_RUN_LEDGER_INDEX, "ci_job_run_ledger");
        assert!(
            !CREATE_CI_JOB_DDL.contains(CI_JOB_RUN_LEDGER_INDEX),
            "the applied ci_0002 table/RLS migration remains byte-identical; index is additive"
        );
        assert_eq!(
            CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL,
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_job_run_ledger ON ci_job (tenant_id, region, run_id)"
        );
        assert_eq!(
            CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL.matches(';').count(),
            0,
            "the concurrent index is one top-level command"
        );

        let migrations = ci_controlplane_migrations();
        let table_pos = migrations
            .0
            .iter()
            .position(|migration| migration.id == "ci_0002_ci_job")
            .expect("immutable ci_job table migration");
        let index_pos = migrations
            .0
            .iter()
            .position(|migration| migration.id == CI_JOB_RUN_LEDGER_INDEX_MIGRATION_ID)
            .expect("separate ci_job ledger-index migration");
        let validation_pos = migrations
            .0
            .iter()
            .position(|migration| migration.id == CI_JOB_RUN_LEDGER_VALIDATION_MIGRATION_ID)
            .expect("separate ci_job ledger-index validation migration");
        let next_table_pos = migrations
            .0
            .iter()
            .position(|migration| migration.id == "ci_0003_check_attempt")
            .expect("next table migration");
        assert_eq!(index_pos, table_pos + 1);
        assert_eq!(validation_pos, index_pos + 1);
        assert_eq!(next_table_pos, validation_pos + 1);
        let migration = &migrations.0[index_pos];
        assert_eq!(migration.table, Some(CI_JOB_TABLE));
        assert_eq!(migration.ddl, CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL);
        let validation = &migrations.0[validation_pos];
        assert_eq!(validation.table, Some(CI_JOB_TABLE));
        assert_eq!(validation.ddl, VALIDATE_CI_JOB_RUN_LEDGER_INDEX_DDL);
        for required_catalog_fragment in [
            "FROM pg_catalog.pg_index AS index_state",
            "JOIN pg_catalog.pg_class AS index_relation",
            "JOIN pg_catalog.pg_class AS table_relation",
            "JOIN pg_catalog.pg_namespace AS relation_namespace",
            "relation_namespace.nspname = current_schema()",
            "table_relation.relname = 'ci_job'",
            "index_relation.relname = 'ci_job_run_ledger'",
            "index_state.indisvalid",
            "index_state.indisready",
            "REINDEX INDEX CONCURRENTLY",
        ] {
            assert!(
                validation.ddl.contains(required_catalog_fragment),
                "validator pins `{required_catalog_fragment}`"
            );
        }
    }

    #[test]
    fn the_frozen_vocabularies_are_check_constraints() {
        let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let ci_run = squash(CREATE_CI_RUN_DDL);
        assert!(
            ci_run.contains("trust_tier text NOT NULL CHECK (trust_tier IN ('trusted','untrusted_fork','self_hosted'))"),
            "ci_run.trust_tier is the frozen three-tier CHECK"
        );
        assert!(
            ci_run.contains("trigger_kind text NOT NULL CHECK (trigger_kind IN ('push','pull_request','issue_transition','manual','agent','schedule'))"),
            "ci_run.trigger_kind is the frozen six-kind CHECK"
        );
        assert!(
            squash(CREATE_JOB_QUEUE_DDL)
                .contains("lane text NOT NULL CHECK (lane IN ('interactive','batch','deploy'))"),
            "job_queue.lane is the frozen three-lane CHECK"
        );
        assert!(
            squash(CREATE_CI_COST_EVENT_DDL)
                .contains("kind text NOT NULL CHECK (kind IN ('ci','agent'))"),
            "ci_cost_event.kind fronts both ci + agent (UNIFY / X-6)"
        );
    }

    #[test]
    fn ci_cost_event_is_ci_namespaced_not_the_storage_name() {
        assert_eq!(CI_COST_EVENT_TABLE, "ci_cost_event");
        assert!(
            CREATE_CI_COST_EVENT_DDL.contains("CREATE TABLE IF NOT EXISTS ci_cost_event ("),
            "the CI metering DDL creates `ci_cost_event`, not `cost_event`"
        );
        assert!(
            !CREATE_CI_COST_EVENT_DDL.contains("EXISTS cost_event ("),
            "the CI metering DDL never creates a bare `cost_event` (Storage's money-ledger name)"
        );
    }

    #[test]
    fn ci_durable_subset_matches_the_full_set_for_the_writer_tables() {
        let full = ci_controlplane_migrations();
        let subset = ci_durable_migrations();
        let subset_ids: Vec<&str> = subset.0.iter().map(|m| m.id).collect();
        assert_eq!(
            subset_ids,
            [
                "ci_0001_ci_run",
                CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID,
                CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID,
                CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID,
                "ci_0003_check_attempt",
                "ci_0003a_ci_run_check_attempt",
                "ci_0014_ci_cost_event",
                CI_RUN_SOURCE_REF_MIGRATION_ID,
                CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID,
                CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID,
                CI_RUN_BRANCH_SCOPE_EXPAND_MIGRATION_ID,
                CI_RUN_BRANCH_SCOPE_VALIDATE_MIGRATION_ID,
                CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID,
            ],
            "the subset is exactly the writer-critical creates plus ci_run's forward ALTERs and repaired provenance contract"
        );
        for m in &subset.0 {
            let full_m = full
                .0
                .iter()
                .find(|f| f.id == m.id)
                .expect("every subset id is in the full control-plane set");
            assert_eq!(
                full_m.ddl, m.ddl,
                "the subset DDL is byte-identical to the full set's (one source of truth): {}",
                m.id
            );
            assert_eq!(full_m.table, m.table, "same table binding: {}", m.id);
        }
        let hot = ci_durable_hot_tables();
        assert!(
            hot.is_hot(CI_COST_EVENT_TABLE)
                && hot.is_hot(CHECK_ATTEMPT_TABLE)
                && hot.is_hot(CI_RUN_CHECK_ATTEMPT_TABLE)
        );
        assert!(!hot.is_hot(CI_RUN_TABLE), "ci_run is not hot");
    }

    #[test]
    fn the_ci_durable_subset_applies_forward_only() {
        use myelin_substrate::MigrationRunner;
        let subset = ci_durable_migrations();
        assert_eq!(
            subset.0.len(),
            13,
            "four writer-critical CI tables plus nine forward ci_run ALTERs"
        );
        for m in &subset.0 {
            assert!(
                !myelin_substrate::is_destructive(m.ddl),
                "subset migration {} is forward-only",
                m.id
            );
            if m.id == CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL);
            } else if m.id == CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL);
            } else if m.id == CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL);
            } else if m.id == CI_RUN_SOURCE_REF_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_RUN_ADD_SOURCE_REF_DDL);
            } else if m.id == CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID {
                assert_eq!(m.ddl, REPAIR_CI_RUN_SOURCE_REF_CONSTRAINT_DDL);
            } else if m.id == CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID {
                assert_eq!(m.ddl, VALIDATE_CI_RUN_SOURCE_REF_CONSTRAINT_DDL);
            } else if m.id == CI_RUN_BRANCH_SCOPE_EXPAND_MIGRATION_ID {
                assert_eq!(m.ddl, EXPAND_CI_RUN_BRANCH_SCOPE_DDL);
            } else if m.id == CI_RUN_BRANCH_SCOPE_VALIDATE_MIGRATION_ID {
                assert_eq!(m.ddl, VALIDATE_CI_RUN_BRANCH_SCOPE_DDL);
            } else if m.id == CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID {
                assert_eq!(m.ddl, CONTRACT_CI_RUN_BRANCH_SCOPE_DDL);
            } else {
                assert!(
                    m.ddl.contains("myelin_make_tenant_scoped"),
                    "subset migration {} installs the platform RLS policy",
                    m.id
                );
            }
        }
        let mut runner = MigrationRunner::new();
        runner
            .run(&subset, &ci_durable_hot_tables())
            .expect("the CI durable writer subset applies forward-only");
        assert_eq!(runner.applied().len(), 13);
    }
}
