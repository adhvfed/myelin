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
/// The shared CI secret-material store. Unlike `secret_binding`, this table contains no manifest
/// binding or plaintext value: only tenant-DEK ciphertext and the envelope fields needed to open it.
pub const CI_SECRET_TABLE: &str = "ci_secret";
/// Additive migration for the encrypted store, appended after every migration present at #34's base.
pub const CI_SECRET_MIGRATION_ID: &str = "ci_0023_ci_secret";
/// Forward-only project ownership metadata required by the authenticated secret-admin surface.
pub const CI_SECRET_ADMIN_SCOPE_MIGRATION_ID: &str = "ci_0024a_secret_admin_scope";
/// Online uniqueness enforcement for managed `(tenant, project, name)` rows.
pub const CI_SECRET_ADMIN_UNIQUE_MIGRATION_ID: &str = "ci_0024b_secret_admin_unique";
/// Forward-only binding backfill + referential-integrity constraint. The applied binding and secret
/// table migrations remain byte-frozen; this is the first migration allowed to connect them.
pub const CI_SECRET_BINDING_INTEGRITY_MIGRATION_ID: &str = "ci_0024c_secret_binding_integrity";
/// Durable high-water marks keep managed-secret versions monotonic across physical deletion and
/// recreation without weakening the binding FK's `ON DELETE CASCADE` semantics.
pub const CI_SECRET_TOMBSTONE_MIGRATION_ID: &str = "ci_0024d_secret_version_tombstone";
pub const CI_SECRET_TOMBSTONE_TABLE: &str = "ci_secret_tombstone";
/// Universal atomic allocator for every `ci_secret` writer. This is deliberately a new migration
/// after the managed-only tombstone migration so all earlier migration bodies remain immutable.
pub const CI_SECRET_VERSION_HIGH_WATER_MIGRATION_ID: &str = "ci_0024e_secret_version_high_water";
pub const CI_SECRET_VERSION_HIGH_WATER_TABLE: &str = "ci_secret_version_high_water";
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
/// CT-007 slice 5b.3-4a.2 — one row per durably-begun claim generation for EITHER job shape (a
/// checkout-bearing job's Hop A/B preparation, or a compute job's workload launch): the common
/// attempt-cap-counting mechanism [`CiAttemptBudgetPolicy::max_parent_attempts`] enforces against,
/// counted as ROWS (never `MAX(lease_epoch)`, never phase rows), so a checkout job's two prelaunch
/// phases count as exactly one attempt while a compute job (which has no phase at all) still counts
/// correctly without inventing a fake phase for it.
pub const CI_JOB_PARENT_ATTEMPT_TABLE: &str = "ci_job_parent_attempt";
/// CT-007 slice 5b.3-4a.2 — the checkout-ONLY child journal of [`CI_JOB_PARENT_ATTEMPT_TABLE`]:
/// exactly the two prelaunch phases (`checkout_transport`, `checkout_materialization`) a
/// checkout-bearing parent attempt runs before its workload. A compute attempt has no child rows at
/// all here — it is fully accounted for by its `ci_job_parent_attempt` row alone.
pub const CI_JOB_PRELAUNCH_USAGE_TABLE: &str = "ci_job_prelaunch_usage";
/// CT-007 phase-credential generations — the append-only credential-generation log. AT MOST ONE
/// immutable row per exact claim and purpose (`checkout_advertise`, `checkout_fetch`,
/// `checkout_materialization`, `workload`), so a claim is structurally bounded to four credentials.
/// There is deliberately NO status column: a generation is CURRENT iff no row with a greater
/// `phase_ordinal` exists for that exact claim, which makes appending the successor the (atomic)
/// revocation of its predecessor at every durable execution gate.
pub const CI_JOB_CREDENTIAL_GENERATION_TABLE: &str = "ci_job_credential_generation";
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
/// Tenant/repository keyset used by the authenticated CT-005 run-list surface.
pub const CI_RUN_SURFACE_REPO_CREATED_INDEX: &str = "ci_run_surface_repo_created";
/// Exact production readiness identity for the authenticated CT-005 run-list index.
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
pub const CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID: &str = "ci_0001d_ci_run_pr_head_generation";
/// Forward-only migration id for [`ALTER_CI_JOB_SPEC_ADD_STAGE_DDL`]. A sub-migration of the
/// already-applied `ci_0015_ci_job_spec` table (the `ci_0002a` convention), applied immediately after
/// it so its checksum is never rewritten and the `ci_0015` create stays byte-frozen.
pub const CI_JOB_SPEC_STAGE_MIGRATION_ID: &str = "ci_0015a_ci_job_spec_stage";
/// Forward-only disposition column for manifest jobs terminalized without execution.
pub const CI_JOB_ACCOUNTING_SKIPPED_MIGRATION_ID: &str = "ci_0017a_ci_job_accounting_skipped";
/// Additive v4 terminal-disposition and receipt columns. The original required v3 receipt remains
/// byte-frozen for rolling compatibility; a v4 row stores both generations.
pub const CI_JOB_ACCOUNTING_DISPOSITION_V4_MIGRATION_ID: &str =
    "ci_0017b_ci_job_accounting_disposition_v4";
/// Forward-only consistency constraint for v4 terminal dispositions. Kept separate because the
/// original v4 column/receipt migration has already been applied and is checksum-immutable.
pub const CI_JOB_ACCOUNTING_DISPOSITION_V4_VERDICT_MIGRATION_ID: &str =
    "ci_0017c_ci_job_accounting_disposition_v4_verdict";
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
/// Additive CT-005 run-list keyset index. Appended after every previously applied migration.
pub const CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID: &str =
    "ci_0018d_ci_run_surface_repo_created";
/// Additive retry-attempt usage accrual for measured infrastructure failures. Appended after every
/// previously applied migration; the byte-frozen `job_queue` create and claim migrations remain
/// unchanged.
pub const CI_JOB_QUEUE_RETRY_ATTEMPTS_MIGRATION_ID: &str = "ci_0018e_job_queue_retry_attempts";
/// Additive partial index for the claim/reap lifecycle conjunction.
pub const CI_RUN_ACTIVE_WORKFLOW_INDEX_MIGRATION_ID: &str = "ci_0018f_ci_run_active_workflow";
/// Additive, column-minimal grant for joining a queue row to its owning CI run. The earlier
/// discovery grant is already applied and remains byte-frozen.
pub const CI_SCHEDULER_CI_RUN_WORKFLOW_ID_GRANT_MIGRATION_ID: &str =
    "ci_0018g_scheduler_ci_run_workflow_id_grant";
/// Additive, column-minimal grant letting the dead-runner reaper reset a reaped job's `ci_job` DAG
/// surface row back to `queued` (investigation, 2026-07-25:
/// `job_queue_region.rs::RESET_REAPED_CI_JOB_SURFACE_QUERY`, added alongside the fix for jobs
/// permanently stranded at `ci_job.state = 'running'` after a crash-then-reap). The dedicated,
/// least-privilege scheduler role never held ANY grant on `ci_job` before this — the real production
/// reaper runs through this exact role (`main.rs`'s `region_queue_store` is
/// `scheduler_provider.region_queue_store()`), so without this grant the fix above would have failed
/// in production with "permission denied for table ci_job" the first time it ever reaped a launched
/// job, exactly as `tests/integration_ci_region_scheduler_boundary.rs`'s dedicated-role test caught.
pub const CI_SCHEDULER_CI_JOB_REAP_RESET_GRANT_MIGRATION_ID: &str =
    "ci_0018h_scheduler_ci_job_reap_reset_grant";
/// CT-007 slice 5b.3-4a.2 — the new parent-attempt journal table/RLS migration.
pub const CI_JOB_PARENT_ATTEMPT_MIGRATION_ID: &str = "ci_0019_ci_job_parent_attempt";
/// CT-007 slice 5b.3-4a.2 — the new checkout-phase child journal table/RLS migration.
pub const CI_JOB_PRELAUNCH_USAGE_MIGRATION_ID: &str = "ci_0020_ci_job_prelaunch_usage";
/// Additive, non-blocking partial index over unresolved (`started`) phase rows, for the reaper
/// (CT-007 slice 5b.3-4b) to find and seal abandoned prelaunch work without a full table scan.
pub const CI_JOB_PRELAUNCH_USAGE_REAPER_INDEX_MIGRATION_ID: &str =
    "ci_0020a_ci_job_prelaunch_usage_reaper";
/// Additive, column-minimal scheduler grant for the cross-tenant, single-region reaper scan the
/// preceding index implies.
pub const CI_SCHEDULER_PRELAUNCH_USAGE_REAP_GRANT_MIGRATION_ID: &str =
    "ci_0020b_scheduler_prelaunch_usage_reap_grant";
/// Expand-phase, nullable deadline column for topology-aware prelaunch sealing. New writers always
/// populate it; a NULL legacy row is never guessed abandoned and therefore fails closed until a
/// later bounded backfill/contract migration.
pub const CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_MIGRATION_ID: &str =
    "ci_0020c_ci_job_prelaunch_usage_seal_deadline";
/// Additive, non-blocking partial index for the topology-aware deadline scan.
pub const CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_MIGRATION_ID: &str =
    "ci_0020d_ci_job_prelaunch_usage_seal_deadline_reaper";
/// CT-007 lease/topology reconciliation, expand phase: the nullable, dispatch-derived claim window.
/// Appended after every previously applied id (never inserted among the `ci_0004*` queue
/// migrations, which are checksum-immutable). NULL is legacy-only: every Rust writer populates it,
/// the claim falls back to the flat execution-lease TTL for a legacy row, and checkout composition
/// refuses such a row outright.
pub const CI_JOB_QUEUE_CLAIM_WINDOW_MIGRATION_ID: &str = "ci_0020e_job_queue_claim_window";
/// The online second half: validate the bounded CHECK the expand added `NOT VALID`, so the full-table
/// verification scan runs without holding the write lock the expand would otherwise have taken.
pub const CI_JOB_QUEUE_CLAIM_WINDOW_VALIDATE_MIGRATION_ID: &str =
    "ci_0020f_job_queue_claim_window_validate";
/// Additive, column-minimal grant letting the superseded-definition boot guard read `wf_version`.
/// The already-applied `ci_0018c` workflow-discovery grant stays byte-frozen; this adds the ONE
/// further column that guard needs, so the least-privilege posture is preserved.
pub const CI_SCHEDULER_WORKFLOW_VERSION_GRANT_MIGRATION_ID: &str =
    "ci_0020g_scheduler_workflow_version_grant";
/// The database-wide, boolean-only backlog probe the definition cutover fence calls while holding
/// the `wf_definition` row lock. `wf_definition` is database-GLOBAL, so a merely regional check must
/// never authorize a global status transition.
pub const CI_PIPELINE_VERSION_BACKLOG_PROBE_MIGRATION_ID: &str =
    "ci_0020h_ci_pipeline_version_backlog_probe";
/// Seeds the cutover fence's predecessor row so a fresh database has something to lock. Absence of
/// the predecessor must never be read as "nothing to fence".
pub const CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID: &str =
    "ci_0020i_ci_pipeline_cutover_fence_row";
/// CT-007 phase-credential generations — the new append-only credential-generation table/RLS
/// migration. Purely additive: no existing migration text changes, and the scheduler role receives
/// NO privilege on it at all (neither reaping nor renewal needs it), so the startup
/// excess-privilege probe treats ANY privilege here as excess.
pub const CI_JOB_CREDENTIAL_GENERATION_MIGRATION_ID: &str = "ci_0021_ci_job_credential_generation";
/// **CT-007 slice 5b.3-6e.1 (DORMANT).** The nullable operational-reservation write-version marker.
/// Legacy/V3 writers leave it `NULL`; the eventual V2 writer stores exactly `2`. Nullable so an
/// already-populated hot queue takes no rewrite and older-writer rows stay readable. Appended after
/// every previously applied id — never inserted among the checksum-immutable `ci_0004*` queue
/// migrations. The scheduler receives NO update privilege on it (the excess-privilege probe asserts
/// it explicitly); only the app role's V2 reserve writer ever sets it, in 6e.2.
pub const CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_MIGRATION_ID: &str =
    "ci_0022_job_queue_reservation_write_version";
/// The online second half of the reservation-marker expand: validate the bounded `= 2` CHECK the
/// expand added `NOT VALID`, so the full-table verification scan runs without the write-blocking lock
/// a validated-on-add constraint would take.
pub const CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_VALIDATE_MIGRATION_ID: &str =
    "ci_0022a_job_queue_reservation_write_version_validate";
/// Additive, non-blocking partial index over the UNSAFE non-terminal rows a v3→v4 activation must be
/// clear of: a null claim window OR a reservation marker that is not exactly `2`. The activation
/// readiness probe (`ci_0022c`) and the cutover predicate both count through it, so the clean path
/// never seq-scans the hot queue.
pub const CI_JOB_QUEUE_ACTIVATION_READINESS_INDEX_MIGRATION_ID: &str =
    "ci_0022b_job_queue_activation_readiness_index";
/// The database-wide, aggregate-only activation-readiness probe the v3→v4 cutover fence calls while
/// holding the `wf_definition` row lock. Mirrors the `ci_0020h` backlog-probe fence-role hardening
/// EXACTLY (born fence-owned, `SECURITY DEFINER`, `row_security=off`, column-scoped to
/// `job_queue(region, state, claim_window_secs, reservation_write_version)`, `REVOKE PUBLIC`, `GRANT
/// EXECUTE` to `myelin_app`). Returns only an unsafe-row COUNT — never a row, tenant, or payload — so
/// it cannot become a cross-region exfiltration path. The production v3→v4 cutover selects it.
pub const CI_V2_ACTIVATION_READINESS_PROBE_MIGRATION_ID: &str =
    "ci_0022c_ci_v2_activation_readiness_probe";
/// CT-007 6e.2 Stage B: seed the v3 predecessor row the production v3→v4 fence locks. On an existing
/// database this is a no-op; on a fresh database the retired sentinel makes v3 permanently
/// inadmissible while still providing the row-level serialization anchor.
pub const CI_PIPELINE_V3_CUTOVER_FENCE_ROW_MIGRATION_ID: &str =
    "ci_0022d_ci_pipeline_v3_cutover_fence_row";

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

/// Immutable attempt issued to one concrete run/context at dispatch reserve time. `check_attempt`
/// remains the per-commit high-water allocator; this run-scoped row is the authority every later
/// lifecycle fact must reuse even after a newer run advances the high-water row.
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

/// `ci_secret` — tenant-owned secret material sealed through Storage's tenant-DEK column cipher.
/// The plaintext has no column and therefore cannot be persisted accidentally. `pii_key_ref` pins
/// the exact tenant-DEK epoch; `nonce` + `ciphertext` are the complete AES-256-GCM at-rest form.
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

/// Add project ownership without rewriting #34's already-landed migration. Legacy rows remain
/// readable by their existing handles but are deliberately absent from the management surface until
/// explicitly reprovisioned; every newly managed row carries a project and is name-unique there.
pub const ALTER_CI_SECRET_ADD_ADMIN_SCOPE_DDL: &str =
    "ALTER TABLE ci_secret ADD COLUMN IF NOT EXISTS project_id uuid";
pub const CREATE_CI_SECRET_ADMIN_UNIQUE_INDEX_DDL: &str = "\
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS ux_ci_secret_tenant_project_name
  ON ci_secret (tenant_id, project_id, name) WHERE project_id IS NOT NULL";

/// Backfill canonical binding handles, discard irreconcilable pre-existing orphans fail-closed, and
/// make the database own the lifetime edge. The tenant-qualified FK uses `ci_secret`'s immutable
/// primary key and cascades every matching binding, including rows whose non-key metadata was
/// malformed before this constraint existed.
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

/// Backfill the greatest extant or deleted version before any writer can allocate through this
/// table. Thereafter `INSERT .. ON CONFLICT .. max_version + 1 RETURNING` is the single authority,
/// whose row lock serializes concurrent create/update/seal operations for one logical secret.
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

/// Add v4 meaning without rewriting or weakening the shipped v3 receipt column. Historical writers
/// continue inserting only `completion_receipt`; explicitly activated v4 writers retain their
/// deterministic v3 twin there and put the authoritative v4 receipt beside its closed disposition.
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
  ADD CONSTRAINT ci_job_accounting_completion_receipt_v4_unique
    UNIQUE (tenant_id, completion_receipt_v4)";

/// Bind each closed v4 disposition to the legacy verdict columns it is allowed to accompany. This
/// is a separate forward migration so the already-applied v4 column/receipt DDL remains byte-frozen.
pub const ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_VERDICT_DDL: &str = "\
ALTER TABLE ci_job_accounting
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

/// `ci_job_parent_attempt` (CT-007 slice 5b.3-4a.2) — one immutable row per durably-begun claim
/// generation, for EITHER job shape. `max_parent_attempts`/`budget_revision` are the exact values
/// this attempt was durably admitted under (never re-derived from current configuration on replay
/// or reaper reconciliation); the two `UNIQUE` constraints beside the primary key prevent a
/// divergent epoch/nonce pairing for the same job from slipping in as a distinct attempt row.
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

/// `ci_job_prelaunch_usage` (CT-007 slice 5b.3-4a.2) — the checkout-only child journal of
/// `ci_job_parent_attempt`. `started` rows carry a fixed ceiling and null exact usage; `measured`
/// rows carry exact usage (`complete_phase`); `sealed_ceiling` rows are the reaper's conservative
/// fallback when a worker never reports (`seal_phase`) and also carry null exact usage, so
/// reconciliation always falls back to the stored ceiling for a sealed phase. Deliberately NOT a
/// database `CHECK (exact <= ceiling)`: preserving an honest over-ceiling measurement is more
/// important than making the row unwritable (Sol's review).
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

/// `ci_job_credential_generation` (CT-007 phase-credential generations) — one IMMUTABLE row per
/// exact claim and credential purpose.
///
/// **Why there is no status column.** The current generation is defined structurally: the row with
/// the greatest `phase_ordinal` for that exact claim. Appending `checkout_fetch` therefore makes
/// `checkout_advertise` non-current at every execution gate in the same commit that created the
/// successor — an atomic supersession no separate revocation write could give us across two durable
/// systems.
///
/// **Why there is no FK to `ci_job_parent_attempt`.** The production resolver mints the first
/// (advertise) credential while resolving the freshly leased row, BEFORE `begin_parent_attempt` can
/// run, so the first row must be persistable before the accounting parent exists. The execution gate
/// still requires the exact parent and a `started` journal phase, so such a credential is unusable
/// until admission completes; the FK to `ci_run` is retained.
///
/// **Bounded cardinality.** `purpose` is part of the primary key and constrained to four values, so
/// one claim can never hold more than four credential generations, and a same-purpose "rotation" is
/// structurally impossible rather than merely refused in Rust.
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

/// Non-blocking partial index for the reaper (CT-007 slice 5b.3-4b) to find unresolved `started`
/// phase rows without a full table scan, mirroring the `jq_claimable`/`ci_run_active_workflow`
/// partial-index convention.
pub const CREATE_CI_JOB_PRELAUNCH_USAGE_REAPER_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_job_prelaunch_usage_reaper \
ON ci_job_prelaunch_usage (region, started_at) WHERE status = 'started'";

/// CT-007 slice 5b.3-4b.1 expand step: add an immutable, server-derived phase deadline without a
/// blocking hot-table rewrite. The column stays nullable in this deployment so an already-populated
/// journal can roll forward safely; new Rust writers always supply it, the resolver refuses a NULL,
/// and the regional sealer never treats NULL as abandoned. A later bounded backfill/contract can
/// make it structurally NOT NULL after fleet convergence.
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

/// Deadline-led replacement for the original started-at discovery index. The old index remains
/// usable during rolling deployment; this new index alone drives 4b.1's safe regional sealer.
pub const CREATE_CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS ci_job_prelaunch_usage_seal_deadline_reaper \
ON ci_job_prelaunch_usage (region, seal_after) WHERE status = 'started' AND seal_after IS NOT NULL";

/// The reaper (CT-007 slice 5b.3-4b) scans for unresolved `started` phase rows CROSS-TENANT within
/// one region -- the same server-mapped, empty-tenant scheduler boundary as `job_queue`'s reap query
/// (Sol's review: the `(region, started_at)` partial index implies exactly this cross-tenant access
/// pattern, which the ordinary per-tenant RLS this table otherwise carries does not admit; without
/// this additive grant the real reaper would fail closed with "permission denied" the first time it
/// tried to seal a row, exactly the gap `GRANT_SCHEDULER_CI_JOB_REAP_RESET_DDL` closed for `ci_job`).
/// `ci_job_parent_attempt` is read-only to the scheduler (it never seals/mutates the parent row);
/// `ci_job_prelaunch_usage` additionally grants column-scoped `UPDATE (status, resolved_at)` --
/// never a table-wide UPDATE, and the transition-guard trigger still enforces every other invariant
/// regardless of which role issues the UPDATE.
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

/// **CT-007 lease/topology reconciliation, expand phase.** The immutable, dispatch-derived claim
/// window (seconds) the claim sizes `claim_expires_at` from. Nullable so an already-populated hot
/// queue takes no rewrite and an older dispatch binary's rows stay readable: the claim COALESCEs a
/// NULL to the flat `$5` execution-lease TTL (byte-identical to pre-slice behaviour), while every
/// new Rust writer supplies a derived value and the checkout composition path refuses a NULL row.
///
/// The CHECK is added `NOT VALID` here and validated by `ci_0020f`, the online idiom for a
/// declared-hot table. Its upper bound is the literal form of
/// [`MAX_CI_JOB_CLAIM_WINDOW_SECS`](crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS); a unit
/// test pins the two equal so a future ceiling change fails loudly instead of silently diverging.
///
/// The constraint add is wrapped in a `DO` block because several live-Postgres test fixtures
/// re-execute this exact DDL text against an already-migrated shared `job_queue`, where a bare
/// `ADD CONSTRAINT` would raise `duplicate_object`. The exception branch does NOT simply swallow
/// that: a same-named constraint carrying a DIFFERENT definition (a hand-patched or divergently
/// deployed bound) would otherwise be silently adopted and then `VALIDATE`d by `ci_0020f`, leaving
/// the durable ceiling disagreeing with Rust while every test still passed. So the branch compares
/// `pg_get_constraintdef` against the exact expected text and re-raises on any divergence — the
/// idempotence is "this precise constraint already exists", never "some constraint by that name".
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

/// The online second half of the claim-window expand: verify the existing rows against the bounded
/// CHECK without the write-blocking lock a validated-on-add constraint would have taken.
pub const VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL: &str = "\
ALTER TABLE job_queue VALIDATE CONSTRAINT job_queue_claim_window_range";

/// **The superseded-definition boot guard's one extra column.** `ci_0018c` already granted the
/// region scheduler `SELECT (tenant_id, region, run_id, wf_type, state, partition, created_at)` on
/// `workflow_run` and remains byte-frozen; the guard additionally needs `wf_version` to tell a
/// stranded `ci.pipeline@N` row from a live `ci.pipeline@N+1` one. Still column-scoped — never a
/// table-wide `SELECT` — and the startup excess-privilege probe's allowed-column set is widened by
/// exactly this one name.
pub const GRANT_SCHEDULER_WORKFLOW_VERSION_DDL: &str =
    "GRANT SELECT (wf_version) ON workflow_run TO myelin_ci_region_scheduler";

/// **The definition cutover fence's database-wide backlog probe (CT-007 lease/topology
/// reconciliation).** `wf_definition` has no tenant or region column — flipping `ci.pipeline@N` to
/// `draining` is a DATABASE-GLOBAL act. A regional `workflow_run` scan therefore must not be the
/// authority for it: one database may serve several independently deployed regions, and a region
/// whose control plane has not yet been upgraded would be silently fenced out with its runs
/// stranded. This function is that global authority.
///
/// It is `SECURITY DEFINER` because the caller (the runtime app role) is `NOBYPASSRLS` and
/// `workflow_run` is FORCE-RLS `(tenant_id, region)`, so no cross-region count is possible under the
/// caller's own privileges.
///
/// **Ownership is load-bearing, not incidental (CT-007 round-3 blocker 2).** A `SECURITY DEFINER`
/// function runs as its OWNER, so its RLS authority is the owner's. The intended production
/// migration role is a non-superuser schema owner WITHOUT `BYPASSRLS` (see `PgBootstrap`) — owning
/// this function as that role would leave the `EXISTS` silently filtered, returning false despite a
/// real backlog, which is a fail-OPEN cutover. So the migration ADOPTS the dedicated
/// `myelin_ci_definition_fence` role (`scripts/pg-init/01-ci-definition-fence.sql`) for the length
/// of one transaction and creates the function under it: the function is born fence-owned, and no
/// ownership transfer is ever performed. A pre-existing function is adopted only when every catalog
/// field matches exactly; any divergence raises rather than being silently replaced.
///
/// `SET row_security = off` is the belt-and-braces half: if this function is ever owned by a role
/// without bypass authority, PostgreSQL raises a LOUD error on any RLS-affected read instead of
/// quietly returning a false negative. Fail-closed beats fail-open even when provisioning drifts.
///
/// The rest of the hardening:
/// - `SET search_path = pg_catalog` plus fully-qualified object names, so no schema-injection can
///   redirect it;
/// - a fixed query with NO dynamic SQL and one bound parameter;
/// - a BOOLEAN result — it returns "does a backlog exist", never a row, a tenant, or a payload, so
///   it cannot become a cross-region data-exfiltration path;
/// - `REVOKE ALL ... FROM PUBLIC`, then `GRANT EXECUTE` to exactly the one runtime role that already
///   registers `wf_definition`;
/// - a positive privilege probe at boot, so a missing grant is loud rather than a silent refusal.
///
/// The predicate matches the `ci_workflow_active_region` partial index's own, for the same
/// index-eligibility reason the regional diagnostic does — the clean path must not seq-scan history.
///
/// The production caller names this function SCHEMA-QUALIFIED
/// (`myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs`), because an unqualified
/// name resolves through `search_path` and a shadowing function that returns `false` instead of
/// raising would be a fail-open cutover. The fully-qualified references inside the body are the
/// separate, complementary guarantee a `SECURITY DEFINER` function must pin so what it READS cannot
/// be redirected. Schema-isolated live tests substitute the call through a dedicated test seam
/// rather than the production resolution being weakened.
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

/// **The cutover fence's predecessor row (CT-007 round-3 blocker 1).** The cutover fails closed when
/// `ci.pipeline@2` is absent, because absence is NOT the same as "nothing to fence": with no row
/// there is nothing to lock `FOR UPDATE`, so a concurrently-booting old binary could
/// `register_definition(v2)` with no conflicting lock and reopen late v2 admission, and an orphaned
/// non-terminal v2 run would never be probed.
///
/// This seeds that predecessor row so a genuinely fresh installation has a fence to take, rather
/// than requiring an operator step a fresh install would silently skip. It is chosen over an
/// operator/test-support bootstrap for exactly that reason — a safety precondition that must hold on
/// EVERY database should be established by the same forward-only mechanism that builds the schema.
///
/// Correct on both database ages, which is what makes it safe as an additive migration:
/// - **existing database:** `ci.pipeline@2` already exists in whatever status the fleet left it;
///   `ON CONFLICT DO NOTHING` makes this a strict no-op. Zero behaviour change.
/// - **fresh database:** the row lands as `retired` with a self-describing sentinel hash. `retired`
///   is the honest status — v2 never ran here and must never be admitted — and it makes an old v2
///   binary booting against this database fail LOUDLY (`register_definition` refuses a non-`active`
///   row, and `validate_definition_pin` refuses a non-`active` status for a fresh start) instead of
///   quietly activating itself. The sentinel hash can never equal a real source-derived pin, so it
///   also fails the hash check first.
pub const SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL: &str = "\
INSERT INTO wf_definition (wf_type, version, code_hash, status)
VALUES ('ci.pipeline', 2, 'sentinel:ci-pipeline-v2-never-deployed-on-this-database', 'retired')
ON CONFLICT (wf_type, version) DO NOTHING";

/// The v3 predecessor sentinel for the atomic v3→v4 activation. This must land only with the v4
/// binary: seeding it while v3 is still the current definition would make a fresh database refuse
/// that binary's own registration.
pub const SEED_CI_PIPELINE_V3_CUTOVER_FENCE_ROW_DDL: &str = "\
INSERT INTO wf_definition (wf_type, version, code_hash, status)
VALUES (
  'ci.pipeline',
  3,
  'sentinel:ci-pipeline-v3-never-deployed-on-this-database',
  'retired'
)
ON CONFLICT (wf_type, version) DO NOTHING";

/// **CT-007 slice 5b.3-6e.1 (DORMANT). The operational-reservation write-version marker.** Nullable
/// so an already-populated hot queue takes no rewrite: legacy/V3 writers leave it `NULL`, the eventual
/// V2 writer stores exactly `2`. The CHECK is added `NOT VALID` here and validated by `ci_0022a`, the
/// online idiom for a declared-hot table.
///
/// The constraint add is wrapped in a `DO` block for the SAME reason `claim_window` is: several
/// live-Postgres fixtures re-execute this exact DDL text against an already-migrated shared
/// `job_queue`, where a bare `ADD CONSTRAINT` would raise `duplicate_object`. The exception branch
/// does NOT swallow that — it compares `pg_get_constraintdef` against the exact expected text and
/// re-raises on any divergence, so the idempotence is "this precise constraint already exists", never
/// "some constraint by that name".
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

/// The online second half of the reservation-marker expand: verify the existing rows against the
/// bounded `= 2` CHECK without the write-blocking lock a validated-on-add constraint would have taken.
pub const VALIDATE_JOB_QUEUE_RESERVATION_WRITE_VERSION_DDL: &str = "\
ALTER TABLE job_queue VALIDATE CONSTRAINT job_queue_reservation_write_version_marker";

/// **The activation-readiness partial index (CT-007 slice 5b.3-6e.1).** Covers exactly the UNSAFE
/// non-terminal rows a v3→v4 activation must be clear of — a null claim window, or a reservation
/// marker that is not exactly `2` — so both the database-wide readiness probe (`ci_0022c`) and the
/// cutover predicate answer through an index lookup rather than a hot-queue seq-scan. Non-blocking
/// (`CONCURRENTLY`), one top-level command. The predicate is byte-identical to the probe's `WHERE`.
pub const CREATE_JOB_QUEUE_ACTIVATION_READINESS_INDEX_DDL: &str = "\
CREATE INDEX CONCURRENTLY IF NOT EXISTS job_queue_activation_readiness \
ON job_queue (region) \
WHERE state <> 'terminal' AND (claim_window_secs IS NULL OR reservation_write_version IS DISTINCT FROM 2)";

/// **The v3→v4 activation-readiness probe (CT-007 slice 5b.3-6e.1, DORMANT).** The cutover fence calls
/// this DATABASE-WIDE while holding the `wf_definition` row lock, to answer one aggregate question:
/// "does any non-terminal queue row still lack a claim window or carry a reservation marker other than
/// 2?". `job_queue` is FORCE ROW LEVEL SECURITY `(tenant_id, region)`, and at cutover time there is no
/// tenant/region scope, so a `NOBYPASSRLS` caller could only ever count its own slice — a fail-OPEN
/// activation. This function is the global authority, hardened IDENTICALLY to the `ci_0020h` backlog
/// probe (round-3 blocker 2):
///
/// - **Ownership is load-bearing.** A `SECURITY DEFINER` function runs as its OWNER, so its RLS
///   authority is the owner's. It is BORN owned by the dedicated `myelin_ci_definition_fence`
///   `BYPASSRLS` role (adopted for one transaction via its explicit `SET TRUE` membership), never
///   ownership-transferred, so the silent-adoption hazard of `CREATE OR REPLACE` over a
///   foreign-owned function cannot arise. A pre-existing function is adopted only when every catalog
///   field matches exactly; any divergence raises.
/// - `SET row_security = off` is belt-and-braces: if this were ever owned by a role without bypass
///   authority, PostgreSQL raises LOUDLY on the RLS-affected read instead of quietly returning a
///   false-negative (fail-closed beats fail-open under provisioning drift).
/// - `SET search_path = pg_catalog` plus fully-qualified object names; a fixed query, NO dynamic SQL,
///   NO bound parameter; a BIGINT aggregate result — it returns "how many unsafe rows", never a row,
///   a tenant, or a payload, so it cannot become a cross-region exfiltration path.
/// - `REVOKE ALL … FROM PUBLIC`, then `GRANT EXECUTE` to exactly the one runtime role that runs the
///   cutover; the regional scheduler is NEVER granted cross-region authority.
///
/// The predicate matches the `job_queue_activation_readiness` partial index's own, for index
/// eligibility. The production caller names this function SCHEMA-QUALIFIED
/// (`myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count`); schema-isolated live tests
/// substitute the call through a dedicated seam rather than weakening production resolution.
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

/// Accumulate exact, immutable failed-attempt receipts on the durable queue row until a later
/// terminal generation settles their aggregate usage. A constant empty-object default is an
/// expand-only metadata change on supported PostgreSQL and keeps existing hot rows readable.
pub const ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL: &str = "ALTER TABLE job_queue \
ADD COLUMN IF NOT EXISTS retry_attempts jsonb NOT NULL DEFAULT '{}'::jsonb";

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

/// Admit only the workflow identity needed for claim/reap lifecycle validation. The scheduler
/// already holds SELECT on the tenant, region, state, and public run identity; this additive grant
/// does not expose definitions, repository metadata, trigger provenance, or mutation.
pub const GRANT_SCHEDULER_CI_RUN_WORKFLOW_ID_DDL: &str =
    "GRANT SELECT (wf_run_id) ON ci_run TO myelin_ci_region_scheduler";

/// Exactly what [`crate::job_queue_region::RESET_REAPED_CI_JOB_SURFACE_QUERY`] needs and nothing
/// more: SELECT on the two join-key columns its `WHERE ... IN (SELECT * FROM UNNEST(...))` reads
/// (`tenant_id`, `job_id`) PLUS the `state` column its OWN `WHERE state = 'running'` filter also
/// reads (an UPDATE's WHERE-clause predicate needs SELECT on every column it inspects, not just the
/// column(s) being written — a real gap the first version of this grant missed, caught by actually
/// re-running the dedicated-role test after applying it, not assumed from the DDL alone), and UPDATE
/// on `state` for the actual write — never a blanket table grant. Without this, the real production
/// reaper (which runs through this exact least-privilege role) would fail closed with "permission
/// denied for table ci_job" the first time it ever reaped a launched job.
pub const GRANT_SCHEDULER_CI_JOB_REAP_RESET_DDL: &str = "\
GRANT SELECT (tenant_id, job_id, state) ON ci_job TO myelin_ci_region_scheduler;
GRANT UPDATE (state) ON ci_job TO myelin_ci_region_scheduler";

/// Keep the claim/reap active-owner check point-addressable inside one tenant and residency cell.
pub const CREATE_CI_RUN_ACTIVE_WORKFLOW_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
ci_run_active_workflow ON ci_run (tenant_id, region, wf_run_id) \
WHERE state = 'running'";

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

/// Non-blocking keyset index for the authenticated per-visible-repository run list.
pub const CREATE_CI_RUN_SURFACE_REPO_CREATED_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS \
ci_run_surface_repo_created ON ci_run \
(tenant_id, region, repo_ref, created_at DESC, run_id DESC) \
WHERE repo_ref IS NOT NULL";

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
    "ci_0003a_ci_run_check_attempt",
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
        // `ci_secret` is a new #34 migration and must remain physically appended after every id
        // already shipped at the base revision. It stays in `create_statements` so all generic table
        // shape/RLS tests cover it, but is assembled once at the tail below.
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
    // CT-007 lease/topology reconciliation. No scheduler grant accompanies these: the claim only
    // READS `claim_window_secs` (covered by the existing table-level SELECT), and the least-privilege
    // probe asserts explicitly that the scheduler role can never UPDATE it.
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
    // CT-007 slice 5b.3-6e.1 (DORMANT). The activation chassis: the nullable reservation-marker
    // column + its online validate, the unsafe-row partial index, and the database-wide readiness
    // probe. Appended after every previously shipped id. No scheduler grant accompanies the column:
    // the scheduler never writes it (the excess-privilege probe asserts it explicitly), and the
    // readiness probe reads it through the bypass-RLS fence role, not the regional scheduler.
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
    HotTables::declare([
        CI_COST_EVENT_TABLE,
        CHECK_ATTEMPT_TABLE,
        CI_RUN_CHECK_ATTEMPT_TABLE,
    ])
}

/// **The CI Control-Plane hot-table declaration** (contract 1.5 / C-3; arch 01 §3 "Hot-table flags
/// declared"). `job_queue` (the scheduler claim churn), `log_segment` (the firehose archive
/// volume), `cost_event` (the per-metered-unit log), `check_attempt` (the per-`(commit_oid, context)`
/// re-dispatch bump), and `ci_run_check_attempt` (immutable per-run issuance) are the write-QPS
/// tables. A declared-hot table refuses a blocking `ALTER` at boot (the migration runner) and is
/// read by the `forward-only-migration` lint at source-scan — so a future ALTER on one of them MUST
/// go expand→backfill→contract (§9.4).
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

    /// **All twenty-three CI Control-Plane tables are in the forward-only migration set, FK-ordered.**
    /// The complete arch 01 §3 control-plane schema (+ CT-004d.1's `ci_job_spec` + CT-007 slice
    /// 5b.3-4a.2's prelaunch-usage journal pair + CT-007's phase-credential generation log) lands
    /// here; `ci_run` precedes `ci_job` (the FK dependency). This is the prompt's "the complete
    /// forward-only data-model migrations" gate.
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

    /// CT-007 slice 5b.3-4a.2: the parent-attempt journal is immutable (insert-only, like
    /// `ci_job_accounting`/`ci_drive_manifest`), FK-anchored to `ci_run`, and its two extra `UNIQUE`
    /// constraints prevent a divergent epoch/nonce pairing for the same job from becoming a second
    /// attempt row (Sol's review).
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

    /// CT-007 slice 5b.3-4a.2: the checkout-phase journal is FK-anchored to the parent-attempt
    /// table, restricted to the two checkout phases and three lifecycle states, has NO database
    /// `exact <= ceiling` constraint (Sol's review: an honest over-ceiling measurement must remain
    /// writable), forbids DELETE (append/transition-only), and guards every UPDATE through a
    /// transition trigger rather than relying on application discipline alone.
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

    /// CT-007 phase-credential generations: the credential log is append-only, purpose-unique,
    /// purpose-to-ordinal checked, structurally bounded to four rows per claim, has NO status column
    /// (current = highest ordinal), and carries NO foreign key to `ci_job_parent_attempt` (the
    /// production resolver mints advertise BEFORE admission can create the parent).
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

    /// The credential-generation table must never receive a scheduler grant: neither reaping nor
    /// renewal reads it, and the startup probe treats ANY privilege on it as excess.
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

    /// **The claim-window expand is online, additive, and its CHECK bound cannot drift from Rust.**
    /// The literal upper bound in the durable constraint is asserted equal to
    /// [`MAX_CI_JOB_CLAIM_WINDOW_SECS`] — a future `MAX_JOB_TIMEOUT_SECS`/headroom change fails here
    /// until a new additive constraint migration is designed, rather than silently admitting windows
    /// the constraint rejects (or rejecting windows Rust considers legal).
    /// **The cutover probe's authority is structural, not incidental (round-3 blocker 2).** Its RLS
    /// power is its OWNER's, so ownership is forced explicitly (a `CREATE OR REPLACE` over a
    /// pre-existing function would otherwise keep that function's old owner), and `row_security=off`
    /// turns a wrongly-owned deployment into a LOUD failure instead of a silent false negative.
    #[test]
    fn the_backlog_probe_is_born_fence_owned_and_never_overwrites_a_divergent_function() {
        let ddl = CREATE_CI_PIPELINE_VERSION_BACKLOG_PROBE_DDL;
        for required in [
            // Born fence-owned: the role is adopted, the function created, the role reset.
            "SET LOCAL ROLE myelin_ci_definition_fence;",
            "RESET ROLE;",
            // The dedicated security schema, never `public`.
            "myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs",
            "SECURITY DEFINER",
            "SET search_path = pg_catalog",
            "SET row_security = off",
            // Verify-and-refuse provisioning, with the exact operator remediation.
            "run scripts/pg-init/01-ci-definition-fence.sql as the database provisioning administrator",
            "passing migration_role=<DATABASE_MIGRATION_URL role>, then retry boot",
            // Exact adopt-or-create, never a blind replace.
            "to_regprocedure",
            "diverges from the expected definition-fence probe",
            // Column-scoped table access, established only once `workflow_run` exists.
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
                "a migration must never provision cluster authority (`{forbidden}`) — it verifies \
                 and names the operator script"
            );
        }
        assert!(
            ddl.contains("TO myelin_app") && !ddl.contains("TO myelin_ci_region_scheduler"),
            "only the runtime role that registers wf_definition may execute the fence's probe"
        );
    }

    /// **CT-007 5b.3-6e.1: the activation-readiness probe mirrors the ci_0020h fence-role hardening
    /// EXACTLY** — born fence-owned, `SECURITY DEFINER`, `row_security=off`, column-scoped to exactly
    /// the four `job_queue` columns, aggregate-only, `REVOKE PUBLIC`, `GRANT EXECUTE` to `myelin_app`
    /// only, never `CREATE OR REPLACE`, never a cluster-authority provision, never a scheduler grant.
    #[test]
    fn the_activation_readiness_probe_is_born_fence_owned_and_column_scoped_to_job_queue() {
        let ddl = CREATE_CI_V2_ACTIVATION_READINESS_PROBE_DDL;
        for required in [
            // Born fence-owned: adopt the role, create the function, reset the role.
            "SET LOCAL ROLE myelin_ci_definition_fence;",
            "RESET ROLE;",
            // The dedicated security schema, never `public`, and the aggregate-only function.
            "myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count",
            "RETURNS bigint",
            "SECURITY DEFINER",
            "SET search_path = pg_catalog",
            "SET row_security = off",
            // Verify-and-refuse provisioning with the exact operator remediation.
            "run scripts/pg-init/01-ci-definition-fence.sql as the database provisioning administrator",
            "passing migration_role=<DATABASE_MIGRATION_URL role>, then retry boot",
            // Exact adopt-or-create, never a blind replace.
            "to_regprocedure",
            "diverges from the expected activation-readiness probe",
            // Column-scoped table access on job_queue, established only once the table exists.
            "REVOKE ALL PRIVILEGES ON TABLE public.job_queue FROM myelin_ci_definition_fence",
            "GRANT SELECT (region, state, claim_window_secs, reservation_write_version)",
            "ON public.job_queue TO myelin_ci_definition_fence",
            // The aggregate unsafe-row body — the SAME predicate as the partial index.
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
        // The probe reads ONLY the four scoped columns — never a payload/identity column.
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

    /// **CT-007 5b.3-6e.1: the reservation-marker expand is online, additive, and marks exactly 2.**
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
        // The idempotent re-apply branch VERIFIES the existing constraint, never adopts it.
        assert!(expand.contains("pg_get_constraintdef"));
        assert!(expand.contains("DIVERGENT definition"));
        assert!(expand.contains("CHECK ((reservation_write_version = 2)) NOT VALID"));
        assert_eq!(
            VALIDATE_JOB_QUEUE_RESERVATION_WRITE_VERSION_DDL,
            "ALTER TABLE job_queue VALIDATE CONSTRAINT job_queue_reservation_write_version_marker"
        );
    }

    /// **CT-007 5b.3-6e.1: the activation-readiness partial index is non-blocking and covers exactly
    /// the unsafe non-terminal rows (same predicate as the probe).**
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

    /// The provisioning script is the ONLY place cluster authority is granted, and the migration's
    /// refusal names it exactly — so an operator reading the failure knows what to run.
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
        // The fence block must be GONE from the general RLS conventions file.
        let conventions = include_str!("../../../scripts/pg-init/00-rls-conventions.sql");
        assert!(
            !conventions.contains("myelin_ci_definition_fence"),
            "the fence provisioning moved to its own operator-runnable file"
        );
    }

    /// The predecessor seed must be additive and self-describing: a no-op on an existing database,
    /// and unmistakably NOT a real definition on a fresh one.
    #[test]
    fn the_cutover_fence_row_seed_is_additive_and_never_admissible() {
        let ddl = SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL;
        assert!(ddl.contains("ON CONFLICT (wf_type, version) DO NOTHING"));
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
        // The idempotent re-apply branch must VERIFY the existing constraint, never adopt it: the
        // normalized `pg_get_constraintdef` text it compares against carries the SAME bound as the
        // `BETWEEN` form above, so the two literals cannot drift apart silently.
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

    /// **The claim-window migrations are appended after every previously shipped id.** Inserting a
    /// new id among the applied `ci_0004*`/`ci_0016*` queue migrations would reorder a checksum-
    /// guarded, already-applied sequence.
    #[test]
    fn claim_window_migrations_are_appended_after_every_shipped_id() {
        let ids: Vec<&str> = ci_controlplane_migrations()
            .0
            .iter()
            .map(|m| m.id)
            .collect();
        let expand = ids
            .iter()
            .position(|id| *id == CI_JOB_QUEUE_CLAIM_WINDOW_MIGRATION_ID)
            .expect("the claim-window expand is in the set");
        let validate = ids
            .iter()
            .position(|id| *id == CI_JOB_QUEUE_CLAIM_WINDOW_VALIDATE_MIGRATION_ID)
            .expect("the claim-window validation is in the set");
        // The claim-window pair sits immediately before the wf_version grant, the backlog probe, the
        // cutover fence-row seed, and the CT-007 5b.3-6e.1 activation-chassis tail (4 migrations).
        assert_eq!(expand, ids.len() - 16);
        assert_eq!(validate, ids.len() - 15);
        // The activation chassis (ci_0022*) is appended AFTER the cutover fence-row seed, in order,
        // followed by the Stage-B sentinel, #34's secret store, then SecretAdmin's online scope,
        // uniqueness, binding-integrity, version-tombstone, and universal-high-water migrations.
        assert_eq!(
            ids[ids.len() - 12],
            CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID,
            "the cutover fence's predecessor-row seed precedes only the ci_0022* chassis"
        );
        assert_eq!(
            &ids[ids.len() - 11..ids.len() - 7],
            &[
                CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_MIGRATION_ID,
                CI_JOB_QUEUE_RESERVATION_WRITE_VERSION_VALIDATE_MIGRATION_ID,
                CI_JOB_QUEUE_ACTIVATION_READINESS_INDEX_MIGRATION_ID,
                CI_V2_ACTIVATION_READINESS_PROBE_MIGRATION_ID,
            ],
            "the CT-007 5b.3-6e.1 activation chassis retains expand→validate→index→probe order"
        );
        assert_eq!(
            ids[ids.len() - 7],
            CI_PIPELINE_V3_CUTOVER_FENCE_ROW_MIGRATION_ID,
            "Stage B appends the retired-v3 sentinel after the activation chassis"
        );
        assert_eq!(
            ids[ids.len() - 6],
            CI_SECRET_MIGRATION_ID,
            "the new encrypted secret store is appended after every migration shipped at the base"
        );
        assert_eq!(
            &ids[ids.len() - 5..],
            &[
                CI_SECRET_ADMIN_SCOPE_MIGRATION_ID,
                CI_SECRET_ADMIN_UNIQUE_MIGRATION_ID,
                CI_SECRET_BINDING_INTEGRITY_MIGRATION_ID,
                CI_SECRET_TOMBSTONE_MIGRATION_ID,
                CI_SECRET_VERSION_HIGH_WATER_MIGRATION_ID,
            ],
            "SecretAdmin appends ownership, uniqueness, binding integrity, tombstones, then the universal version high-water"
        );
        assert!(
            expand
                > ids
                    .iter()
                    .position(|id| *id == CI_JOB_PRELAUNCH_USAGE_SEAL_DEADLINE_INDEX_MIGRATION_ID)
                    .unwrap(),
            "the expand follows the last previously shipped migration"
        );
    }

    /// **The migration set applies forward-only (no DROP, no down) — the contract-1.5 floor.** Every
    /// assembled DDL is forward-only-legal (`is_destructive` is false) and carries the platform RLS
    /// scoping. The runner / lint enforce this at boot / source-scan; this is the in-module proof.
    #[test]
    fn the_migration_set_is_forward_only_and_rls_scoped() {
        let migrations = ci_controlplane_migrations();
        assert_eq!(
            migrations.0.len(),
            69,
            "24 table/RLS (including encrypted secrets, tombstones, and universal high-water) + 3 secret-admin scope/index/integrity migrations + the previously shipped CI follow-ons"
        );
        for m in &migrations.0 {
            assert!(
                !myelin_substrate::is_destructive(m.ddl),
                "migration {} is forward-only (no DROP): {}",
                m.id,
                m.ddl
            );
            // Stricter than `is_destructive` (which names only TABLE/COLUMN) but keyword-anchored:
            // a bare "DROP" substring also matches the catalogue column `attisdropped`, which
            // `ci_0020h` legitimately reads when enumerating live columns to revoke.
            let upper = m.ddl.to_ascii_uppercase();
            for destructive in [
                "DROP TABLE",
                "DROP COLUMN",
                "DROP SCHEMA",
                "DROP FUNCTION",
                "DROP ROLE",
                "DROP INDEX",
                "DROP CONSTRAINT",
                "DROP DATABASE",
                "DROP OWNED",
            ] {
                assert!(
                    !upper.contains(destructive),
                    "no `{destructive}` in migration {}",
                    m.id
                );
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
            69,
            "the runner applied the complete 24-table schema plus every additive follow-on"
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
        // Previously applied discovery/index migrations remain byte-identical. New capabilities
        // are appended under fresh ids.
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

    /// **The seven hot tables are declared (arch 01 §3 "Hot-table flags declared").** `job_queue` /
    /// `log_segment` / `ci_cost_event` / the high-water and run-scoped check-attempt ledgers, plus
    /// (CT-007 slice 5b.3-4a.2) the parent-attempt and checkout-phase journal pair — the write-QPS
    /// tables that refuse a blocking ALTER at boot.
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
    /// `ci_run` / `check_attempt` / `ci_run_check_attempt` / `ci_cost_event` migrations from the
    /// full `ci_controlplane_migrations()` — same ids, same assembled DDL — so applying both at one
    /// boot is idempotent (shared ids no-op).
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
        // The hot tables in the subset agree with the full control-plane declaration.
        let hot = ci_durable_hot_tables();
        assert!(
            hot.is_hot(CI_COST_EVENT_TABLE)
                && hot.is_hot(CHECK_ATTEMPT_TABLE)
                && hot.is_hot(CI_RUN_CHECK_ATTEMPT_TABLE)
        );
        assert!(!hot.is_hot(CI_RUN_TABLE), "ci_run is not hot");
    }

    /// **CT-004m — the shared subset applies forward-only at boot (no DROP), FK-safe.** The runner
    /// admits `ci_run` / `check_attempt` / `ci_run_check_attempt` / `ci_cost_event` (the CREATEs are
    /// Plain on empty tables); a re-run is idempotent. This is the boot-time half of the
    /// both-mains-apply gate.
    #[test]
    fn the_ci_durable_subset_applies_forward_only() {
        use myelin_substrate::MigrationRunner;
        let subset = ci_durable_migrations();
        assert_eq!(
            subset.0.len(),
            7,
            "four writer-critical CI tables plus three forward ci_run ALTERs"
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
        assert_eq!(runner.applied().len(), 7);
    }
}
