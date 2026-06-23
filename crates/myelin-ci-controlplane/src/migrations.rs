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
//! - the **reserve/settle metering** that writes `cost_event` — **CI-P17** (P-359 cluster);
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
pub const COST_EVENT_TABLE: &str = "cost_event";

/// The scheduler index names (arch 01 §3.3 — the hot-path claim surface). The behaviour (the
/// `FOR UPDATE SKIP LOCKED` claim) is CI-P12; the SHAPES land here so the claim has its indexes.
pub const JQ_CLAIMABLE_INDEX: &str = "jq_claimable";
pub const JQ_SERIALIZE_INDEX: &str = "jq_serialize";
pub const JQ_IDEM_INDEX: &str = "jq_idem";

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

/// `cost_event` (arch 01 §3.7 — D8) — one row per metered unit; wholesale & markup separate columns,
/// integer quantities (NEVER a float). HOT (per-metered-unit). The reserve/settle metering is CI-P17.
pub const CREATE_COST_EVENT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS cost_event (
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

/// Every CI Control-Plane CREATE-TABLE DDL paired with its table name + a stable migration id, in
/// FK-dependency order (`ci_run` before `ci_job`). The `job_queue` create rides with its three
/// indexes appended (an empty fresh table — no hot-table lock; the create is atomic). One ordered
/// list so [`ci_controlplane_migrations`] builds the [`Migrations`] set + the
/// `forward-only-migration` lint reads the same DDL.
fn create_statements() -> Vec<(&'static str, &'static str, String)> {
    // job_queue: the create + the three indexes, assembled into ONE forward DDL (empty fresh table).
    let mut job_queue_ddl = String::from(CREATE_JOB_QUEUE_DDL);
    job_queue_ddl.push(';');
    for (_name, idx) in CREATE_JOB_QUEUE_INDEXES_DDL {
        job_queue_ddl.push('\n');
        job_queue_ddl.push_str(idx);
        job_queue_ddl.push(';');
    }
    vec![
        (
            "ci_0001_ci_run",
            CI_RUN_TABLE,
            CREATE_CI_RUN_DDL.to_string(),
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
        ("ci_0004_job_queue", JOB_QUEUE_TABLE, job_queue_ddl),
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
            "ci_0014_cost_event",
            COST_EVENT_TABLE,
            CREATE_COST_EVENT_DDL.to_string(),
        ),
    ]
}

/// The RLS scoping DDL for a CI Control-Plane table — the platform-wide `myelin_make_tenant_scoped`
/// convention (FORCE row-level security + the `(tenant_id, region)` isolation policy). CI does NOT
/// fork the RLS policy; it calls the ONE helper every tenant table uses (EI-01 §7).
pub fn make_tenant_scoped_ddl(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

/// **The complete CI Control-Plane forward-only migration set** (contract 1.5 / 11.1; arch 01 §3).
/// One [`Migration`] per table (`Plain` — a CREATE on an empty table is a plain forward migration;
/// no expand→backfill→contract is needed to CREATE), in FK-dependency order, each carrying its
/// CREATE-TABLE DDL + the platform RLS scoping (and, for `job_queue`, the three claim indexes). The
/// runner applies them forward-only at boot; the `forward-only-migration` lint reads the same DDL.
pub fn ci_controlplane_migrations() -> Migrations {
    let mut migrations = Vec::new();
    for (id, table, create) in create_statements() {
        // Each migration: the CREATE (+ any indexes) followed by the platform RLS scoping call.
        let mut ddl = create;
        if !ddl.trim_end().ends_with(';') {
            ddl.push(';');
        }
        ddl.push('\n');
        ddl.push_str(&make_tenant_scoped_ddl(table));
        ddl.push(';');
        // The substrate `Migration` holds `&'static str`; the set is built once at boot/serve, so
        // this is a one-time, bounded leak — the same shape the framework + refs-service expect.
        let ddl: &'static str = Box::leak(ddl.into_boxed_str());
        // `Migration::plain_on` carries the table so the runner can match it against the hot-table
        // declaration (the hot tables refuse a blocking ALTER; the CREATE is admitted as Plain).
        migrations.push(Migration::plain_on(id, ddl, table));
    }
    Migrations::of(migrations)
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
        COST_EVENT_TABLE,
        CHECK_ATTEMPT_TABLE,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **All fourteen CI Control-Plane tables are in the forward-only migration set, FK-ordered.**
    /// The complete arch 01 §3 control-plane schema lands here; `ci_run` precedes `ci_job` (the FK
    /// dependency). This is the prompt's "the complete forward-only data-model migrations" gate.
    #[test]
    fn all_fourteen_controlplane_tables_are_present_fk_ordered() {
        let migrations = ci_controlplane_migrations();
        let tables: Vec<&str> = migrations.0.iter().map(|m| m.table.unwrap()).collect();
        assert_eq!(
            tables,
            vec![
                CI_RUN_TABLE,
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
                COST_EVENT_TABLE,
            ],
            "all 14 control-plane tables, FK-dependency ordered (ci_run before ci_job)"
        );
        // ci_run precedes ci_job (the FK target before the FK source).
        let run_pos = tables.iter().position(|t| *t == CI_RUN_TABLE).unwrap();
        let job_pos = tables.iter().position(|t| *t == CI_JOB_TABLE).unwrap();
        assert!(
            run_pos < job_pos,
            "ci_run is created before ci_job (the FK)"
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

    /// **The migration set applies forward-only (no DROP, no down) — the contract-1.5 floor.** Every
    /// assembled DDL is forward-only-legal (`is_destructive` is false) and carries the platform RLS
    /// scoping. The runner / lint enforce this at boot / source-scan; this is the in-module proof.
    #[test]
    fn the_migration_set_is_forward_only_and_rls_scoped() {
        let migrations = ci_controlplane_migrations();
        assert_eq!(
            migrations.0.len(),
            14,
            "14 forward migrations, one per table"
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
            // every table is made RLS-tenant-scoped via the platform helper.
            assert!(
                m.ddl.contains("myelin_make_tenant_scoped"),
                "migration {} installs the platform RLS policy",
                m.id
            );
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
            14,
            "the runner applied all 14 control-plane migrations"
        );
        assert_eq!(
            runner.applied()[0],
            "ci_0001_ci_run",
            "ci_run is applied first (FK order)"
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
    /// `log_segment` / `cost_event` / `check_attempt` — the write-QPS tables that refuse a blocking
    /// ALTER (the C-3 expand→backfill→contract discipline) at boot.
    #[test]
    fn the_four_hot_tables_are_declared() {
        let hot = ci_controlplane_hot_tables();
        for t in [
            JOB_QUEUE_TABLE,
            LOG_SEGMENT_TABLE,
            COST_EVENT_TABLE,
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
        // The job_queue migration carries the create + all three indexes + the RLS scoping.
        let jq = ci_controlplane_migrations()
            .0
            .into_iter()
            .find(|m| m.table == Some(JOB_QUEUE_TABLE))
            .unwrap();
        for name in [JQ_CLAIMABLE_INDEX, JQ_SERIALIZE_INDEX, JQ_IDEM_INDEX] {
            assert!(jq.ddl.contains(name), "index `{name}` rides the migration");
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
            squash(CREATE_COST_EVENT_DDL)
                .contains("kind text NOT NULL CHECK (kind IN ('ci','agent'))"),
            "cost_event.kind fronts both ci + agent (UNIFY / X-6)"
        );
    }
}
