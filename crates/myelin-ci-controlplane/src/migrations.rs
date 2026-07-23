//! **The complete forward-only data-model migrations for the CI Control Plane** (CI-P6 / P-349;
//! contract 1.5 forward-only online migrations + the hot-table flags; 11.1 OLTP; 12.1 the
//! `(tenant, region)` partition key).
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/01-tech-and-data-model.md`
//! §3 (the per-service schema — the EXACT columns of every CI store) + §4 (the
//! encryption/residency/GDPR posture). The CI Control Plane owns the latency-/correctness-critical
//! transactional core (arch 00 §4, the second of the five logical services): `ci_run`, `ci_job`,
//! `check_attempt`, the scheduler tables (`job_queue` + `fair_deficit`), `runner`, the log range
//! index (`log_segment` + `log_anchor`), the artifact/cache indices (`artifact` + `cache_entry`),
//! and the deployment/environment/secret/cost-event state. (The dedup ledger `consumer_dedup` is
//! Trigger & Dispatch's — it lives in `myelin-ci-dispatch`, the other shell.)
//!
//! ## What CI-P6 ships here — the table SHAPES, forward-only, RLS-ready (NOT the behaviour)
//! Every table below is created exactly as frozen in arch 01 §3, as a **forward-only** migration
//! (contract 1.5; no DROP, no down/rollback) expressed through the substrate framework
//! ([`myelin_substrate::Migration`] / [`Migrations`]) so the boot-time RUNNER applies it AND the
//! `forward-only-migration` lint reads it at source-scan. Each table is:
//! - **`(tenant_id, region)`-first** (arch 01 §3: `tenant`/`region` are the leading columns / the
//!   partition prefix, contract 12.1) — the `tenant-predicate` lint target (every key is
//!   tenant-first; there is no cross-tenant query path);
//! - **RLS-enforced** via the platform-wide `myelin_make_tenant_scoped(...)` convention
//!   (`scripts/pg-init/00-rls-conventions.sql`) — FORCE row-level security + the `(tenant_id,
//!   region)` isolation policy. CI does NOT fork the RLS policy (EI-01 §7 coherence — one helper).
//!
//! ## Reconciliation: the §3 column name vs the platform RLS convention (documented deviation)
//! Architecture §3 names the tenant partition column `tenant uuid`. The platform-wide RLS helper
//! `myelin_make_tenant_scoped` (the ONE dev/prod RLS convention every tenant table uses, storage
//! §3.1 / contract 11.1, the same one `myelin-refs-service` / `myelin-knowledge` use) binds its
//! `(tenant_id, region)` isolation policy to a `tenant_id text` + `region text` pair. To keep ONE
//! RLS convention across every subsystem, these migrations name the columns **`tenant_id text` +
//! `region text`** (the convention's exact names) while preserving §3's intent verbatim:
//! `tenant_id`/`region` are the FIRST columns / partition prefix and the RLS isolation key. The
//! `uuid` vs `text` choice follows the platform convention (the tenant token is an opaque string at
//! this layer — `myelin_tenancy::TenantId(String)`); a tenant id is a stable opaque token, never
//! PII. This is the same deliberate, documented deviation `myelin-refs-service`'s edge migration
//! records (EI-01 §1, code-wins-over-docs: the convention wins over the literal column name so the
//! RLS floor is the SAME one Postgres enforces for every tenant table).
//!
//! ## The hot tables (arch 01 §3 — the write-QPS tables)
//! `job_queue`, `log_segment`, `cost_event`, `check_attempt` are declared HOT
//! ([`ci_controlplane_hot_tables`], contract 1.5 / C-3): they carry the scheduler claim churn / the
//! firehose archive volume / the per-metered-unit cost log / the per-`(commit_oid, context)`
//! re-dispatch bump. The hot-table flag means the migration runner refuses a blocking `ALTER` on
//! them at boot, and the `forward-only-migration` lint reads the same declaration at source-scan.
//! The CREATE-TABLE migrations themselves are `Plain` (a create on an empty table takes no
//! meaningful lock; the expand→backfill→contract discipline applies to LATER ALTERs on the
//! populated hot tables, declared by the behaviour bands as the write rate warrants — §9.4).
//!
//! ## Floors named (VISION §3 / prompt DoD) — the per-table BEHAVIOUR follow-ons
//! **This is the SCHEMA ONLY — empty tables are not a working subsystem.** The per-table behaviour
//! lands in its own prompt and is named here:
//! - the scheduler **pull-lease claim** over `job_queue` (the `jq_claimable` `FOR UPDATE SKIP
//!   LOCKED` query) + concurrency groups + affinity + the dead-runner reaper — **CI-P12** (P-355);
//! - the **DRR fair-share** over `fair_deficit` (the deficit advance at claim time) — **CI-P13**
//!   (P-356);
//! - the **EU fleet autoscaler** over `runner` — **CI-P14** (P-357);
//! - the **`check_attempt` monotonic counter** bump + the `ci.check.updated` producer — **CI-P18**
//!   (P-361);
//! - the **log range index** populate over `log_segment` / `log_anchor` — **CI-P20** (P-363);
//! - the **trust-scoped artifact/cache** writes + the per-subject log DEK — **CI-P22** (P-365);
//! - the **reserve/settle metering** that writes `cost_event` — SHIPPED in [`crate::metering`]
//!   (**CI-P17** / P-360): the resource-second meter + the `cost_event` row (wholesale ≠ markup) over
//!   the FROZEN reserve/settle ledger (contract 11.7);
//! - the **deployment / protected-env HITL gate + the in-boundary secret broker** that drives
//!   `environment` / `deployment` / `secret_binding` — **CI-P24**.
//!
//! Nothing below writes a row; this migration set creates the tables forward-only + RLS-on so the
//! behaviour bands have their targets.
//!
//! The live-DB forward-only apply (against the dev-stack Postgres) is proven in
//! `tests/integration_ci_p6_controlplane_schema.rs` (the `integration` cargo feature); the default
//! `cargo build`/`cargo test --workspace` stay DB-free.

use myelin_substrate::{HotTables, Migration, Migrations};

/// The CI Control-Plane table names (arch 01 §3). PII-free opaque identifiers. The order is the
/// foreign-key dependency order the runner applies in (`ci_run` before `ci_job`, etc.).
pub const CI_RUN_TABLE: &str = "ci_run";
/// Insert-only replay authority consumed by the durable `ci.pipeline` workflow.
pub const CI_DRIVE_MANIFEST_TABLE: &str = "ci_drive_manifest";
pub const CI_JOB_TABLE: &str = "ci_job";
pub const CHECK_ATTEMPT_TABLE: &str = "check_attempt";
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
/// CI's metering-projection table. **CT-004m rename:** NAMED `ci_cost_event` (not `cost_event`) so it
/// does NOT collide with Storage's money-ledger `cost_event` (myelin-storage migration `0050`,
/// `reserve_settle_durable::COST_LEDGER_MIGRATION`) in the ONE shared `myelin` database every service
/// migrates against. The two tables are DISTINCT: Storage's `cost_event` is the reserve-keyed money
/// log (`run_id text, ord, unit, wholesale, markup`); CI's `ci_cost_event` is the run/job-attributed
/// metering PROJECTION (`cost_id, run_id uuid, job_id, meter, amount, wholesale_minor_units,
/// markup_minor_units, kind`). Before the rename `CREATE TABLE IF NOT EXISTS cost_event` would no-op
/// against whichever applied first, silently leaving the other's INSERT to fail on missing columns.
pub const CI_COST_EVENT_TABLE: &str = "ci_cost_event";
pub const CI_JOB_ACCOUNTING_TABLE: &str = "ci_job_accounting";
/// CT-004d.1 — the durable `JobSpec` store table. One row per DISPATCHED stage job: the
/// digest-pinned [`myelin_ci_sandbox::JobSpec`] (image/command/egress/limits/trust/workspace/run-token/
/// meter/idem) the runner resolves + EXECUTES, keyed by the `(tenant_id, job_id)` the leased
/// `job_queue` row carries. The spec round-trips faithfully as a single `jsonb` column (the whole
/// serde value — every field, no lossy column projection), co-persisted with the `job_queue` row on
/// the dispatch's shared `idem_token`. NAMED distinct from every other CI table; single-owner (the
/// control-plane dispatch writes it, the control-plane runner reads it) so it lives ONLY in the full
/// [`ci_controlplane_migrations`] set, NOT the shared writer subset (the SAME posture as `job_queue`).
pub const CI_JOB_SPEC_TABLE: &str = "ci_job_spec";

/// The scheduler index names (arch 01 §3.3 — the hot-path claim surface). The behaviour (the
/// `FOR UPDATE SKIP LOCKED` claim) is CI-P12; the SHAPES land here so the claim has its indexes.
pub const JQ_CLAIMABLE_INDEX: &str = "jq_claimable";
pub const JQ_SERIALIZE_INDEX: &str = "jq_serialize";
pub const JQ_IDEM_INDEX: &str = "jq_idem";
/// Exact-cell run-ledger lookup used by [`crate::PgCiPipelineStarter`].
pub const CI_JOB_RUN_LEDGER_INDEX: &str = "ci_job_run_ledger";
/// Region starter discovery index over queued CI runs, with tenant ownership covered for an
/// index-only lookup.
pub const CI_RUN_QUEUED_REGION_INDEX: &str = "ci_run_queued_region";
/// Region recovery index over nonterminal CI workflow runs, covering the persisted partition.
pub const CI_WORKFLOW_ACTIVE_REGION_INDEX: &str = "ci_workflow_active_region";
/// Forward-only migration id for [`CI_JOB_RUN_LEDGER_INDEX`]. Kept separate from the already-applied
/// `ci_0002_ci_job` table/RLS migration so its checksum is never rewritten.
pub const CI_JOB_RUN_LEDGER_INDEX_MIGRATION_ID: &str = "ci_0002a_ci_job_run_ledger";
/// Forward-only postcondition check for [`CI_JOB_RUN_LEDGER_INDEX_MIGRATION_ID`].
pub const CI_JOB_RUN_LEDGER_VALIDATION_MIGRATION_ID: &str = "ci_0002b_validate_ci_job_run_ledger";
/// Forward-only causal provenance columns for delayed CI lifecycle emission.
pub const CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID: &str = "ci_0001b_ci_run_causal_provenance";
/// Forward-only canonical PR concurrency identity carried from the triggering event into launch
/// authority. The original `ci_0001_ci_run` create remains byte-frozen.
pub const CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID: &str = "ci_0001c_ci_run_concurrency_group";
/// Forward-only producer-authored PR row generation. This is separate from `ci_0001c` so the
/// already-applied concurrency-group migration remains byte-identical.
pub const CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID: &str =
    "ci_0001d_ci_run_pr_head_generation";
/// Forward-only migration id for [`ALTER_CI_JOB_SPEC_ADD_STAGE_DDL`]. A sub-migration of the
/// already-applied `ci_0015_ci_job_spec` table (the `ci_0002a` convention), applied immediately after
/// it so its checksum is never rewritten and the `ci_0015` create stays byte-frozen.
pub const CI_JOB_SPEC_STAGE_MIGRATION_ID: &str = "ci_0015a_ci_job_spec_stage";
/// Forward-only disposition column for manifest jobs terminalized without execution.
pub const CI_JOB_ACCOUNTING_SKIPPED_MIGRATION_ID: &str = "ci_0017a_ci_job_accounting_skipped";
/// Forward-only migration id for [`ALTER_JOB_QUEUE_ADD_COMPLETION_DDL`]. A sub-migration of the
/// already-applied `ci_0004_job_queue` table, applied immediately after it (the `ci_0002a` convention)
/// so the `ci_0004` create stays byte-frozen. Its ADD COLUMNs are non-blocking (a constant-default
/// `NOT NULL` and nullable columns — [`myelin_storage`]'s `is_blocking_alter` admits them on the
/// declared-hot `job_queue` table).
pub const CI_JOB_QUEUE_COMPLETION_MIGRATION_ID: &str = "ci_0004a_job_queue_completion";
/// Forward-only follow-on for the unguessable claim nonce and queue-authority stage. Kept separate
/// from `ci_0004a` because that migration was already applied with only epoch + receipt columns.
pub const CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID: &str = "ci_0004b_job_queue_claim_authority";
/// Forward-only persisted initial claim timestamps. These are immutable within one lease generation
/// even when heartbeat extends `lease_expires`, allowing claim-time Identity minting to verify the
/// exact timestamps returned by the scheduler rather than trusting caller memory.
pub const CI_JOB_QUEUE_CLAIM_TIME_MIGRATION_ID: &str = "ci_0004c_job_queue_claim_time";
/// Forward-only migration id for [`GRANT_SCHEDULER_LEASE_EPOCH_DDL`]. A follow-on to the already-applied
/// `ci_0016_region_scheduler_rls` grant (byte-frozen), extending the region scheduler's column-scoped
/// `UPDATE` grant to the new `lease_epoch` column the claim bumps — least-privilege preserved (no new
/// table-wide grant, only the one column the claim writes).
pub const CI_SCHEDULER_LEASE_EPOCH_GRANT_MIGRATION_ID: &str =
    "ci_0016a_scheduler_lease_epoch_grant";
/// Additive scheduler grant for the fresh per-claim nonce, without changing applied `ci_0016a`.
pub const CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID: &str =
    "ci_0016b_scheduler_claim_nonce_grant";
/// Additive least-privilege grant for the two initial-claim timestamp columns.
pub const CI_SCHEDULER_CLAIM_TIME_GRANT_MIGRATION_ID: &str = "ci_0016c_scheduler_claim_time_grant";
/// Additive queued-run discovery index, kept separate from the byte-frozen `ci_run` table create.
pub const CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID: &str = "ci_0018_ci_run_queued_region";
/// Additive, column-minimal queued-run discovery grant for the constrained region scheduler.
pub const CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID: &str = "ci_0018a_scheduler_ci_run_discovery";
/// Additive active-workflow recovery index.
pub const CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID: &str = "ci_0018b_ci_workflow_active_region";
/// Additive, column-minimal workflow route used only for exact-tenant worker routing.
pub const CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID: &str =
    "ci_0018c_scheduler_ci_workflow_discovery";

// ============================================================================================
// The forward-only CREATE-TABLE DDL constants (arch 01 §3, verbatim intent; tenant_id/region named
// to the RLS convention — see the module deviation note). Held as `&str` so the DDL is NOT mistaken
// for live Rust by the lints (`blank_string_literals` blanks literal contents), while the migration
// framework still carries the real DDL to the boot runner / the live integration test.
//
// The fixed value-sets (state, trigger_kind, lane, trust_tier, ...) are enforced by `CHECK`
// constraints (the frozen vocabularies §3) rather than Postgres `ENUM` types so a forward-only
// vocabulary EXTENSION is a non-blocking `CHECK` add, never an enum-rewrite (forward-only, §9).
// ============================================================================================

/// `ci_run` (arch 01 §3.1) — the thin index over the myelin-flow workflow run. `triggered_by` is a
/// PSEUDONYM subject (contract 4.8; tagged in the [`crate::schema::CiRunRow`] mirror).
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

/// Preserve the triggering envelope fields required to derive later children after outbox retention
/// has expired. The shipped `ci_0001_ci_run` create remains byte-identical.
pub const ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL: &str = "ALTER TABLE ci_run \
ADD COLUMN IF NOT EXISTS cause_depth bigint NOT NULL DEFAULT 0 \
CHECK (cause_depth BETWEEN 0 AND 4294967295), \
ADD COLUMN IF NOT EXISTS caused_by text";

/// Add a nullable compatibility column: historical PR rows remain readable, while all new writers
/// fail closed unless they carry the canonical group. Non-NULL values are PR-only, bounded, and
/// control-free so scheduler keys cannot be ambiguous or abusive.
pub const ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL: &str = "ALTER TABLE ci_run \
ADD COLUMN concurrency_group text \
CHECK (concurrency_group IS NULL OR (\
trigger_kind = 'pull_request' \
AND concurrency_group ~ '^pr:[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*:[1-9][0-9]*$' \
AND octet_length(concurrency_group) BETWEEN 4 AND 512 \
AND concurrency_group !~ '[[:cntrl:]]'))";

/// Historical and old-dispatcher rows remain nullable for rolling upgrade. Non-NULL values are
/// positive producer-authored generations; new writers require this column together with the
/// canonical concurrency group.
pub const ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL: &str = "ALTER TABLE ci_run \
ADD COLUMN pr_head_generation bigint \
CHECK (pr_head_generation IS NULL OR (\
trigger_kind = 'pull_request' AND pr_head_generation > 0))";

/// `ci_drive_manifest` — the immutable, canonical launch authority for one CI workflow run.
///
/// The canonical bytes are retained alongside their domain-separated digest so a replay never
/// reconstructs execution authority from mutable configuration. Runtime roles may insert/read but
/// cannot update or delete a manifest; the trigger is a second line of defence for any future role
/// whose grants accidentally widen. Secrets and ephemeral token JTIs never belong in these bytes.
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

/// `ci_job` (arch 01 §3.1) — one row per DAG node of a run. FK to `ci_run` (tenant-first).
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

/// Exact `(tenant_id, region, run_id)` lookup for materializing and replay-verifying one run's
/// canonical `ci_job` ledger. This is a separately versioned, top-level concurrent migration: the
/// applied `ci_0002_ci_job` table/RLS migration remains byte-identical.
pub const CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_job_run_ledger ON ci_job (tenant_id, region, run_id)";

/// Fail startup when `CREATE INDEX CONCURRENTLY IF NOT EXISTS` encountered a same-named index left
/// invalid or not ready by an interrupted prior build. Catalog identity is constrained to the exact
/// index and table in the active migration schema, not merely a same-named object on the search path.
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

/// `check_attempt` (arch 01 §3.2) — the per-`(commit_oid, context)` monotonic attempt counter; CI's
/// source of `run_attempt` for the X-1 CheckStatus fact. HOT (bumped on each re-dispatch).
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

/// `job_queue` (arch 01 §3.3) — one row per schedulable job; the scheduler's hot claim surface. HOT.
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

/// The three `job_queue` indexes (arch 01 §3.3 — the claim surface). The behaviour (the `FOR UPDATE
/// SKIP LOCKED` claim over `jq_claimable`) is CI-P12; the SHAPES land here. Each is tenant-region
/// aware: `jq_claimable` leads with `region` (a runner claims only in-region — no global pool,
/// residency by construction, arch 00 §5), and the dedup/serialize uniques are per-`(tenant, ...)`.
///
/// **Built `CONCURRENTLY` (forward-only on a declared-HOT table).** `job_queue` is declared hot
/// ([`ci_controlplane_hot_tables`]), so the migration runner + the `forward-only-migration` lint
/// refuse a NON-concurrent `CREATE INDEX` on it (a non-concurrent index takes a write-blocking lock
/// at QPS — `is_blocking_alter`). Using `CONCURRENTLY` keeps the index create non-blocking even
/// against a populated hot table (the expand-phase discipline, §3.1/§9.4) — so the same DDL is legal
/// whether the table is empty (this CI-P6 create) or later re-applied against live write traffic.
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

/// `fair_deficit` (arch 01 §3.3) — the per-`fair_key` DRR deficit counter, advanced at claim time.
/// The behaviour (the deficit advance) is CI-P13; the SHAPE lands here.
pub const CREATE_FAIR_DEFICIT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS fair_deficit (
  tenant_id   text NOT NULL,
  region      text NOT NULL,
  fair_key    text NOT NULL,
  deficit     bigint NOT NULL DEFAULT 0,
  last_served timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, region, fair_key)
)";

/// `runner` (arch 01 §3.4) — the fleet's runner-host register; the autoscaler (CI-P14) reads it.
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

/// `log_segment` (arch 01 §3.5 — the frozen Storage 11.8 `(job, step, byte-range)` index half).
/// HOT (the firehose archive volume). `pii_key_ref` is per-tenant OR per-subject (Storage C1).
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

/// `log_anchor` (arch 01 §3.5) — the `(job, step) → byte offset` index (the `#step-<n>` sub-anchor
/// that resolves CheckStatus.details_ref). The behaviour (jump-to-failure) is CI-P20/P21.
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

/// `artifact` (arch 01 §3.6) — retained job output, ArtifactRef-addressable, explicit TTL
/// (Art. 5 storage-limitation). The trust-scoped write behaviour is CI-P22.
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

/// `cache_entry` (arch 01 §3.6) — the reconstructible, TRUST-SCOPED cache index (Storage C4: an
/// UntrustedFork write cannot reach the trusted scope). The scope-enforced write is CI-P22.
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

/// `environment` (arch 01 §3.7) — prod/staging; `protected` gates the deploy HITL. The approver set
/// resolves via Id.list_subjects at gate time (contract 4.4), NOT stored here. Gate is CI-P24.
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

/// `deployment` (arch 01 §3.7) — a deploy of a version to an env. `approved_by` is a PSEUDONYM
/// subject (contract 4.8; tagged in the [`crate::schema::DeploymentRow`] mirror). Gate is CI-P24.
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

/// `secret_binding` (arch 01 §3.7) — NAMES + scope only; VALUES live in the shared secret store. An
/// `untrusted_fork` resolves to NONE by default (the ABAC edge). The broker is CI-P24.
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

/// `ci_cost_event` (arch 01 §3.7 — D8) — one row per metered unit; wholesale & markup separate
/// columns, integer quantities (NEVER a float). HOT (per-metered-unit). The reserve/settle metering is
/// CI-P17. **CT-004m:** the physical table is `ci_cost_event` (see [`CI_COST_EVENT_TABLE`]) — a
/// CI-namespaced name distinct from Storage's money-ledger `cost_event` (migration `0050`) in the
/// shared `myelin` DB.
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

/// `ci_job_spec` (CT-004d.1) — the durable `JobSpec` store. One row per dispatched stage job: the
/// digest-pinned spec the runner resolves + executes, keyed `(tenant_id, job_id)` (the leased
/// `job_queue` row's identity). The spec is a single `jsonb` column (the whole serde value — faithful
/// round-trip of every field; the stored spec is what EXECUTES, so no lossy per-column projection).
/// `run_id` + `idem_token` are carried alongside for co-keying with the `job_queue` enqueue (the
/// dispatch writes BOTH rows in one tx on the shared `idem_token`). `(tenant, region)`-first + RLS-on
/// like every CI table; NOT hot (insert-once-at-dispatch / read-once-at-claim, no claim-churn UPDATEs).
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

/// Immutable terminal-accounting receipt for one dispatched CI job. Raw sandbox usage and the
/// immutable pricing revision are retained beside the monetary outcome and claim-bound completion
/// receipt, so terminal replay can be verified without repricing historical work.
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

/// **The forward-only ALTER that adds the durable `stage` column to `ci_job_spec` (CT-004d.2 rewire).**
/// The dispatched stage NAME is the durable-by-contract fact the terminal reporter reads back at
/// `job.done` verification (a restart must not lose the job→stage mapping — the exact failure the
/// durability rewire closes). `ADD COLUMN IF NOT EXISTS` is forward-only + idempotent: a fresh deploy
/// created `ci_job_spec` at `ci_0015` (no `stage`) and this ALTER adds it; an existing production DB
/// that already applied `ci_0015` gets the column here. The column is nullable at the schema floor
/// (an ALTER cannot back-fill historical rows), but every NEW dispatch persists it NOT-NULL via
/// [`crate::job_spec_store::CiJobSpecStore::co_persist_dispatch`], and the reporter fails closed on a
/// missing stage rather than fabricating a verdict.
pub const ALTER_CI_JOB_SPEC_ADD_STAGE_DDL: &str =
    "ALTER TABLE ci_job_spec ADD COLUMN IF NOT EXISTS stage text";

/// Add an explicit skipped disposition without rewriting the shipped accounting-table migration.
/// Existing completion receipts backfill to `false`; a skipped receipt can never simultaneously be
/// passed or timed out.
pub const ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL: &str = "ALTER TABLE ci_job_accounting \
ADD COLUMN IF NOT EXISTS skipped boolean NOT NULL DEFAULT false, \
ADD CONSTRAINT ci_job_accounting_skipped_verdict \
CHECK (NOT skipped OR (NOT passed AND NOT timed_out))";

/// **The immutable forward-only ALTER adding the original completion columns to `job_queue`
/// (CT-004d.2 claim-bound completion).** `lease_epoch` is the monotone claim generation the
/// [`crate::scheduler::CLAIM_QUERY`] bumps on every claim, so a stale worker whose lease was reaped and
/// re-claimed carries a lower epoch than the row and is refused at completion. `completion_receipt`
/// records exact redelivery evidence. This DDL is byte-immutable because `ci_0004a` has shipped.
///
/// The base [`CREATE_JOB_QUEUE_DDL`] (the `ci_0004` create) stays BYTE-FROZEN — the migrator enforces
/// applied-migration checksum immutability — so this forward-only `ADD COLUMN IF NOT EXISTS` sub-migration
/// is the ONE place the columns are added, for both a fresh bootstrap and a rolling upgrade. A raw-DDL
/// test setup that builds `job_queue` from the frozen create applies this ALTER alongside it.
pub const ALTER_JOB_QUEUE_ADD_COMPLETION_DDL: &str = "ALTER TABLE job_queue \
ADD COLUMN IF NOT EXISTS lease_epoch bigint NOT NULL DEFAULT 0, \
ADD COLUMN IF NOT EXISTS completion_receipt text";

/// Add the remaining claim-authority columns without rewriting the applied `ci_0004a` migration.
/// `claim_nonce` is freshly minted for every generation; `stage` is co-persisted with the dispatch so
/// the scheduler capability can guard activation without crossing `ci_job_spec` RLS.
pub const ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL: &str = "ALTER TABLE job_queue \
ADD COLUMN IF NOT EXISTS claim_nonce uuid, \
ADD COLUMN IF NOT EXISTS stage text";

/// Persist the original claim window separately from the heartbeat-extended execution lease. Both
/// values are written from the same PostgreSQL `statement_timestamp()` as the claim response.
pub const ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL: &str = "ALTER TABLE job_queue \
ADD COLUMN IF NOT EXISTS claim_started_at timestamptz, \
ADD COLUMN IF NOT EXISTS claim_expires_at timestamptz";

/// **The immutable forward-only grant for the original claim epoch.**
/// The `ci_0016_region_scheduler_rls` boundary grants `UPDATE (state, lease_owner, lease_expires)` (that
/// applied migration stays byte-frozen). The claim now also bumps `lease_epoch`, so the least-privilege
/// scheduler role needs `UPDATE (lease_epoch)` too — granted here as a follow-on, NOT by rewriting the
/// boundary. Still column-scoped (never a table-wide `UPDATE`), so the boundary's least-privilege posture
/// holds. The completion-receipt write is done by the APP role (the reporter's tenant-tx), never the
/// scheduler role, so no receipt grant is needed.
pub const GRANT_SCHEDULER_LEASE_EPOCH_DDL: &str =
    "GRANT UPDATE (lease_epoch) ON job_queue TO myelin_ci_region_scheduler";

/// Add only the nonce capability in a new migration; `ci_0016a` remains checksum-compatible.
pub const GRANT_SCHEDULER_CLAIM_NONCE_DDL: &str =
    "GRANT UPDATE (claim_nonce) ON job_queue TO myelin_ci_region_scheduler";

/// Add only the two timestamp columns the scheduler writes when minting a claim generation.
pub const GRANT_SCHEDULER_CLAIM_TIME_DDL: &str = "GRANT UPDATE \
(claim_started_at, claim_expires_at) ON job_queue TO myelin_ci_region_scheduler";

/// Every CI Control-Plane CREATE-TABLE DDL paired with its table name + a stable migration id, in
/// FK-dependency order (`ci_run` before `ci_job`). Indexes are deliberately not bundled here:
/// PostgreSQL requires `CREATE INDEX CONCURRENTLY` to be its own top-level command, outside the
/// implicit transaction used for a multi-statement simple query.
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
    ]
}

/// Stable migration ids paired with the three non-transactional concurrent index statements. The
/// ids sort immediately after the `ci_0004_job_queue` table/RLS migration and before `ci_0005`.
pub const CI_JOB_QUEUE_INDEX_MIGRATIONS: &[(&str, &str)] = &[
    ("ci_0004a_jq_claimable", JQ_CLAIMABLE_INDEX),
    ("ci_0004b_jq_serialize", JQ_SERIALIZE_INDEX),
    ("ci_0004c_jq_idem", JQ_IDEM_INDEX),
];

/// Dedicated region-scheduler RLS/grant boundary. This is deliberately a new additive migration:
/// the historical table/RLS migration ids and checksums remain byte-unchanged.
pub const CI_REGION_SCHEDULER_RLS_MIGRATION_ID: &str = "ci_0016_region_scheduler_rls";

/// Install the cross-tenant, single-region scheduler policies over the existing FORCE-RLS queue
/// tables. PostgreSQL combines the OR of applicable permissive policies with every applicable
/// restrictive policy. The scheduler-targeted restrictive guard is therefore load-bearing: even if
/// a scheduler sets a tenant GUC and becomes eligible for the PUBLIC tenant policy, it cannot escape
/// the server-owned `session_user` region mapping or operate with a non-empty tenant scope.
///
/// Privileges are capability-minimal: claim/reap can read queue/fairness rows and update only the
/// three lease columns. No insert, delete, fairness mutation, or unrelated-table privilege is added.
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

/// Admit only the five `ci_run` columns needed to choose the tenant owning the oldest queued run.
/// The paired permissive/restrictive SELECT policies preserve the same empty-tenant, server-mapped
/// region boundary as queue claim/reap. No run payload, definition, repository, or mutation
/// privilege is exposed to the scheduler credential.
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

/// Non-blocking partial index for oldest-queued-run discovery in one residency cell.
pub const CREATE_CI_RUN_QUEUED_REGION_INDEX_DDL: &str = "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
ci_run_queued_region ON ci_run (region, created_at, run_id) INCLUDE (tenant_id) \
WHERE state = 'queued'";

/// Non-blocking partial index for restart-safe active workflow discovery in one residency cell.
pub const CREATE_CI_WORKFLOW_ACTIVE_REGION_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
ci_workflow_active_region ON workflow_run (region, created_at, tenant_id, run_id) \
INCLUDE (partition) \
WHERE wf_type = 'ci.pipeline' AND state IN ('running', 'waiting')";

/// Exact column grant plus the same server-mapped, empty-tenant RLS boundary as CI-run discovery.
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

/// The stable migration ids of the WRITER-CRITICAL CI durable tables both CI service mains must have
/// present regardless of boot order (CT-004m): `ci_run` (ci-dispatch's reserve/start co-commit + the
/// control-plane run state), `check_attempt` (the control-plane monotonic counter), and
/// `ci_cost_event` (the control-plane metering projection [`crate::cost_store::CiCostEventStore`]
/// writes). These are the tables the CT-004a + CT-004b production stores touch. The ids are a SUBSET
/// of [`create_statements`] — the SAME ids + the SAME DDL the full [`ci_controlplane_migrations`] set
/// carries, so applying both is idempotent (the shared ids no-op on the second apply).
pub const CI_DURABLE_WRITER_IDS: &[&str] = &[
    "ci_0001_ci_run",
    "ci_0003_check_attempt",
    "ci_0014_ci_cost_event",
];

/// Assemble ONE forward-only [`Migration`] from a `(id, table, create-DDL)` triple: the CREATE (plus
/// any indexes already appended to `create`) followed by the platform RLS scoping call. Single source
/// of the per-table migration shape — both [`ci_controlplane_migrations`] and [`ci_durable_migrations`]
/// build through it, so a table's DDL + id is authored EXACTLY ONCE (no divergence between the full
/// control-plane set and the shared writer subset).
fn assemble_ci_migration(id: &'static str, table: &'static str, create: String) -> Migration {
    let mut ddl = create;
    if !ddl.trim_end().ends_with(';') {
        ddl.push(';');
    }
    ddl.push('\n');
    ddl.push_str(&make_tenant_scoped_ddl(table));
    ddl.push(';');
    // The substrate `Migration` holds `&'static str`; the set is built once at boot/serve, so this is
    // a one-time, bounded leak — the same shape the framework + refs-service expect.
    let ddl: &'static str = Box::leak(ddl.into_boxed_str());
    Migration::plain_on(id, ddl, table)
}

/// The RLS scoping DDL for a CI Control-Plane table — the platform-wide `myelin_make_tenant_scoped`
/// convention (FORCE row-level security + the `(tenant_id, region)` isolation policy). CI does NOT
/// fork the RLS policy; it calls the ONE helper every tenant table uses (EI-01 §7).
pub fn make_tenant_scoped_ddl(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

/// **The complete CI Control-Plane forward-only migration set** (contract 1.5 / 11.1; arch 01 §3).
/// One table/RLS [`Migration`] per table, in FK-dependency order, plus six separately versioned
/// `CREATE INDEX CONCURRENTLY` migrations and one post-index validation migration: the run-ledger
/// index and validator immediately after `ci_job`, the three job scheduler indexes immediately after
/// `job_queue`, and the queued/running run-discovery indexes as additive follow-ons. Keeping each
/// concurrent index as one top-level command makes the same set executable by live PostgreSQL.
///
/// Compatibility: the prior `ci_0004_job_queue` bundled these concurrent indexes into one
/// multi-statement command. PostgreSQL rejected that command atomically before [`PgMigrator`]
/// could record the id, and the production Controlplane main never submitted the full AppSpec set
/// to the live migrator. The live activation-boundary test pins both rollback and non-recording, so
/// retaining `ci_0004_job_queue` for the now-executable table/RLS command does not mutate an applied
/// production migration. [`PgMigrator`] currently skips an existing id without comparing its stored
/// checksum; that general checksum-read gap remains separate, and no future applied id may rely on it.
///
/// [`PgMigrator`]: myelin_storage::PgMigrator
pub fn ci_controlplane_migrations() -> Migrations {
    let mut migrations = Vec::new();
    for (id, table, create) in create_statements() {
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
            // Preserve the applied epoch/receipt migration, then add nonce/stage under a new id.
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
            // The durable stage column (CT-004d.2 rewire) — a forward-only sub-migration applied
            // immediately after the `ci_0015` table create, leaving that create byte-frozen.
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
        }
    }
    migrations.push(Migration::plain_on(
        CI_REGION_SCHEDULER_RLS_MIGRATION_ID,
        CREATE_CI_REGION_SCHEDULER_RLS_DDL,
        JOB_QUEUE_TABLE,
    ));
    // The claim-generation grant follow-on (least-privilege UPDATE on the one new claim-written column).
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
    Migrations::of(migrations)
}

/// **The shared, writer-critical CI durable-migration subset (CT-004m).** The tables the CT-004a
/// (`ci_cost_event`) + CT-004b (`ci_run`) production stores write, plus `check_attempt` — created
/// forward-only, `(tenant, region)`-first + RLS-on, from the SAME DDL + ids as the full
/// [`ci_controlplane_migrations`] set (via [`assemble_ci_migration`], filtered to
/// [`CI_DURABLE_WRITER_IDS`]).
///
/// **Why it exists.** The platform runs ONE shared `myelin` Postgres for every service (docs/
/// dev-stack.md). ci-controlplane applies the full 15-table CI schema through its `serve(AppSpec)`
/// migrate; ci-dispatch's `serve(AppSpec)` applies ONLY `consumer_dedup` — so before CT-004m,
/// ci-dispatch's reserve/start `ci_run` write depended on ci-controlplane having booted first (a
/// boot-order coupling). BOTH CI service mains now apply THIS set at boot (idempotent, advisory-locked,
/// forward-only), so the writer tables are present regardless of which service boots first. It shares
/// migration ids with the full set, so a ci-controlplane boot that applies both no-ops the overlap.
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
    Migrations::of(migrations)
}

/// The hot-table declaration for the [`ci_durable_migrations`] subset — the write-QPS tables in it
/// (`ci_cost_event` the per-metered-unit log, `check_attempt` the per-`(commit_oid, context)`
/// re-dispatch bump). `ci_run` is not hot. Consistent with [`ci_controlplane_hot_tables`] so the
/// migration runner reads the SAME hot flags whichever set applies these tables.
pub fn ci_durable_hot_tables() -> HotTables {
    HotTables::declare([CI_COST_EVENT_TABLE, CHECK_ATTEMPT_TABLE])
}

/// **The CI Control-Plane hot-table declaration** (contract 1.5 / C-3; arch 01 §3 "Hot-table flags
/// declared"). `job_queue` (the scheduler claim churn), `log_segment` (the firehose archive
/// volume), `cost_event` (the per-metered-unit log), and `check_attempt` (the per-`(commit_oid,
/// context)` re-dispatch bump) are the write-QPS tables. A declared-hot table refuses a blocking
/// `ALTER` at boot (the migration runner) and is read by the `forward-only-migration` lint at
/// source-scan — so a future ALTER on one of them MUST go expand→backfill→contract (§9.4).
pub fn ci_controlplane_hot_tables() -> HotTables {
    HotTables::declare([
        JOB_QUEUE_TABLE,
        LOG_SEGMENT_TABLE,
        CI_COST_EVENT_TABLE,
        CHECK_ATTEMPT_TABLE,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **All seventeen CI Control-Plane tables are in the forward-only migration set, FK-ordered.**
    /// The complete arch 01 §3 control-plane schema (+ CT-004d.1's `ci_job_spec`) lands here; `ci_run`
    /// precedes `ci_job` (the FK dependency). This is the prompt's "the complete forward-only
    /// data-model migrations" gate.
    #[test]
    fn all_seventeen_controlplane_tables_are_present_fk_ordered() {
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
            ],
            "all 17 control-plane tables, FK-dependency ordered (ci_run before its dependants)"
        );
        // ci_run precedes ci_job (the FK target before the FK source).
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

    /// **Every CI table is `(tenant_id, region)`-first with a tenant-first primary key (contract
    /// 12.1 / the tenant-predicate floor).** No key path can scan across tenants — `tenant_id` is
    /// the FIRST column + the first PK component on every table (arch 01 §3 "no cross-tenant query
    /// path").
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
    }

    /// **The migration set applies forward-only (no DROP, no down) — the contract-1.5 floor.** Every
    /// assembled DDL is forward-only-legal (`is_destructive` is false) and carries the platform RLS
    /// scoping. The runner / lint enforce this at boot / source-scan; this is the in-module proof.
    #[test]
    fn the_migration_set_is_forward_only_and_rls_scoped() {
        let migrations = ci_controlplane_migrations();
        assert_eq!(
            migrations.0.len(),
            38,
            "17 table/RLS + 3 ci_run ALTERs + 6 concurrent-index + 1 index-validation + 3 job_queue ALTERs + 1 ci_job_spec-stage ALTER + 1 ci_job_accounting-skipped ALTER + 1 scheduler-boundary + 3 scheduler claim grants + 2 scheduler ci_run discovery grants"
        );
        for m in &migrations.0 {
            assert!(
                !myelin_substrate::is_destructive(m.ddl),
                "migration {} is forward-only (no DROP): {}",
                m.id,
                m.ddl
            );
            assert!(
                !m.ddl.to_ascii_uppercase().contains("DROP"),
                "no DROP in migration {}",
                m.id
            );
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
            } else if m.id == CI_JOB_SPEC_STAGE_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_JOB_SPEC_ADD_STAGE_DDL);
            } else if m.id == CI_JOB_ACCOUNTING_SKIPPED_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL);
            } else if m.id == CI_JOB_QUEUE_COMPLETION_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL);
            } else if m.id == CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL);
            } else if m.id == CI_JOB_QUEUE_CLAIM_TIME_MIGRATION_ID {
                assert_eq!(m.ddl, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL);
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

    /// **The runner admits the whole set forward-only at boot, FK-ordered (contract 1.5).** The
    /// substrate runner applies every migration (no DROP, no blocking ALTER on a hot table — the
    /// CREATEs are Plain) and records them applied in order. This is the boot-time half of the gate.
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
            38,
            "the runner applied all 17 table/RLS, 3 ci_run ALTERs, 6 concurrent-index, 1 index-validation, 3 job_queue ALTERs, 1 ci_job_spec-stage ALTER, 1 ci_job_accounting-skipped ALTER, 1 scheduler-boundary, 3 scheduler claim grants, and 2 scheduler ci_run discovery grants"
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
        // The workflow-route grant is the final additive migration. The already-shipped queued
        // discovery and claim grants remain byte-identical.
        let workflow_discovery = migrations
            .0
            .last()
            .expect("the workflow-route grant is the final additive migration");
        assert_eq!(
            workflow_discovery.id,
            CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID
        );
        assert_eq!(workflow_discovery.table, Some("workflow_run"));
        assert_eq!(
            workflow_discovery.ddl,
            GRANT_SCHEDULER_CI_WORKFLOW_DISCOVERY_DDL
        );
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

    /// **A destructive rollback variant is refused (forward-only is structural, not vacuous).** A
    /// hypothetical `DROP TABLE ci_run` is rejected by the runner — proving the gate is real (a real
    /// DROP would halt boot, §9.1 / EI-01 §2).
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

    /// **The four hot tables are declared (arch 01 §3 "Hot-table flags declared").** `job_queue` /
    /// `log_segment` / `ci_cost_event` / `check_attempt` — the write-QPS tables that refuse a blocking
    /// ALTER (the C-3 expand→backfill→contract discipline) at boot.
    #[test]
    fn the_four_hot_tables_are_declared() {
        let hot = ci_controlplane_hot_tables();
        for t in [
            JOB_QUEUE_TABLE,
            LOG_SEGMENT_TABLE,
            CI_COST_EVENT_TABLE,
            CHECK_ATTEMPT_TABLE,
        ] {
            assert!(hot.is_hot(t), "`{t}` is declared hot (arch 01 §3)");
        }
        // a non-hot table is NOT flagged (the declaration is precise, not a blanket).
        assert!(
            !hot.is_hot(ENVIRONMENT_TABLE),
            "environment is NOT a hot table (low write rate)"
        );
    }

    /// **The three scheduler claim indexes exist with their exact predicates (arch 01 §3.3).**
    /// `jq_claimable` (region-led, `WHERE state = 'queued'` — the claim surface), `jq_serialize`
    /// (the `deploy:%` running-serialize unique), `jq_idem` (the per-tenant enqueue dedup unique).
    /// The behaviour (the `FOR UPDATE SKIP LOCKED` claim) is CI-P12; the SHAPES land here.
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
            "jq_claimable keys (region, lane, enqueued_at) — the in-region claim order"
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
        // Each concurrent index is a separately versioned top-level command after the table/RLS.
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

    /// The canonical DAG ledger lookup is an additive, separately versioned concurrent index placed
    /// directly after the immutable `ci_0002_ci_job` table/RLS migration, followed by an exact
    /// catalog postcondition validator.
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

    /// **The frozen value-set vocabularies are CHECK constraints, not enum types (forward-only
    /// vocabulary extension, §9).** The `state` / `trust_tier` / `trigger_kind` / `lane` / `meter`
    /// vocabularies are enforced by CHECK so a new value is a non-blocking CHECK add, never an
    /// enum-rewrite. Pins the frozen vocabularies arch 01 §3 names.
    #[test]
    fn the_frozen_vocabularies_are_check_constraints() {
        // Whitespace-insensitive: collapse runs of spaces so the column-alignment padding in the DDL
        // does not make these assertions brittle to a `cargo fmt`/edit reflow.
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

    /// **CT-004m — the collision rename is real: CI's metering table is `ci_cost_event`, never
    /// `cost_event`.** Storage owns the money-ledger `cost_event` (migration `0050`) in the shared
    /// `myelin` DB; CI's projection must NOT create a differently-shaped table under the same name.
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

    /// **CT-004m — the shared writer subset is byte-identical to the full set (no DDL divergence).**
    /// `ci_durable_migrations()` (the tables both CI mains apply at boot) carries EXACTLY the
    /// `ci_run` / `check_attempt` / `ci_cost_event` migrations from the full `ci_controlplane_migrations()`
    /// — same ids, same assembled DDL — so applying both at one boot is idempotent (shared ids no-op).
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
                "ci_0014_ci_cost_event",
            ],
            "the subset is exactly the writer-critical creates plus ci_run's forward ALTERs"
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
        // The two hot tables in the subset agree with the full control-plane hot-table declaration.
        let hot = ci_durable_hot_tables();
        assert!(hot.is_hot(CI_COST_EVENT_TABLE) && hot.is_hot(CHECK_ATTEMPT_TABLE));
        assert!(!hot.is_hot(CI_RUN_TABLE), "ci_run is not hot");
    }

    /// **CT-004m — the shared subset applies forward-only at boot (no DROP), FK-safe.** The runner
    /// admits `ci_run` / `check_attempt` / `ci_cost_event` (the CREATEs are Plain on empty tables); a
    /// re-run is idempotent. This is the boot-time half of the both-mains-apply gate.
    #[test]
    fn the_ci_durable_subset_applies_forward_only() {
        use myelin_substrate::MigrationRunner;
        let subset = ci_durable_migrations();
        assert_eq!(
            subset.0.len(),
            6,
            "three writer-critical CI tables plus three forward ci_run ALTERs"
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
        assert_eq!(runner.applied().len(), 6);
    }
}
