//! Dedicated PostgreSQL provider for the cross-tenant, region-scoped CI scheduler.
//!
//! The ordinary runtime pool authenticates as the tenant application role and must never claim or
//! reap across tenants. This module opens the separately configured scheduler credential only after
//! migrations, validates its server-owned region mapping and exact least-privilege posture, and
//! exposes the pool solely as [`crate::CiRegionQueueStore`] and
//! [`crate::CiRegionRunDiscovery`].
//!
//! **Named lint exclusion.** This file's SQL reads only PostgreSQL identity, catalog, privilege, and
//! server-owned scheduler-region metadata. Those are database/cell authorization facts spanning
//! roles by design, not tenant-store reads, so no `TenantId` predicate exists to bind. The actual
//! queue data path remains in `job_queue_region.rs` under its mapped-region RLS boundary, while all
//! tenant mutation verbs remain fully linted in `job_queue_store.rs`.

use myelin_config::MyelinConfig;
use myelin_storage::connect_pool_with_reset;
use sqlx::{PgPool, Row};

use crate::{CiRegionQueueStore, CiRegionRunDiscovery};

/// Required production credential for the constrained region scheduler login.
pub const CI_SCHEDULER_DATABASE_URL_ENV: &str = "MYELIN_CI_SCHEDULER_DATABASE_URL";

const SCHEDULER_CAPABILITY_ROLE: &str = "myelin_ci_region_scheduler";
const SCHEDULER_MAX_CONNECTIONS: u32 = 8;

/// Credential-bearing scheduler configuration. Debug output always redacts the DSN.
#[derive(Clone, PartialEq, Eq)]
pub struct CiSchedulerDbConfig {
    database_url: String,
    region: String,
}

impl core::fmt::Debug for CiSchedulerDbConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CiSchedulerDbConfig")
            .field("database_url", &"<redacted>")
            .field("region", &self.region)
            .finish()
    }
}

impl CiSchedulerDbConfig {
    /// Read the required scheduler DSN and prove it is distinct from both platform credentials.
    pub fn from_env(platform: &MyelinConfig) -> Result<Self, CiSchedulerDbError> {
        let value = std::env::var_os(CI_SCHEDULER_DATABASE_URL_ENV)
            .ok_or(CiSchedulerDbError::MissingDatabaseUrl)?;
        let database_url = value
            .into_string()
            .map_err(|_| CiSchedulerDbError::NonUnicodeDatabaseUrl)?;
        Self::from_parts(
            database_url,
            &platform.database_url,
            &platform.database_migration_url,
            platform.region.clone(),
        )
    }

    /// Explicit construction seam used by DB-free and live provider tests.
    pub fn from_parts(
        database_url: String,
        runtime_database_url: &str,
        migration_database_url: &str,
        region: String,
    ) -> Result<Self, CiSchedulerDbError> {
        if database_url.trim().is_empty() {
            return Err(CiSchedulerDbError::EmptyDatabaseUrl);
        }
        if database_url == runtime_database_url || database_url == migration_database_url {
            return Err(CiSchedulerDbError::CredentialsNotDistinct);
        }
        if region.trim().is_empty() {
            return Err(CiSchedulerDbError::EmptyRegion);
        }
        Ok(Self {
            database_url,
            region,
        })
    }
}

/// Credential-redacted scheduler-provider refusal. No variant carries a DSN, database error, role
/// name, or mapped/configured region value.
#[non_exhaustive]
#[derive(Debug, PartialEq, Eq)]
pub enum CiSchedulerDbError {
    MissingDatabaseUrl,
    NonUnicodeDatabaseUrl,
    EmptyDatabaseUrl,
    CredentialsNotDistinct,
    EmptyRegion,
    ConnectionFailed,
    ProbeFailed,
    DatabaseIdentityMismatch,
    RolesNotDistinct,
    LoginRequired,
    IdentityChanged,
    Superuser,
    BypassRls,
    CreateDatabase,
    CreateRole,
    DatabaseCreate,
    PublicSchemaUsageMissing,
    PublicSchemaCreate,
    PublicSchemaOwnerMembership,
    CapabilityMembershipMissing,
    CapabilityNotInherited,
    CapabilityCanSetRole,
    UnexpectedMembership,
    RegionUnmapped,
    RegionMismatch,
    InsufficientPrivileges,
    ExcessPrivileges,
}

impl core::fmt::Display for CiSchedulerDbError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            Self::MissingDatabaseUrl => "required CI scheduler database credential is missing",
            Self::NonUnicodeDatabaseUrl => {
                "required CI scheduler database credential contains non-UTF-8 bytes"
            }
            Self::EmptyDatabaseUrl => "required CI scheduler database credential is empty",
            Self::CredentialsNotDistinct => {
                "CI scheduler database credential must differ from runtime and migration credentials"
            }
            Self::EmptyRegion => "configured CI scheduler region is empty",
            Self::ConnectionFailed => "CI scheduler database connection failed",
            Self::ProbeFailed => "CI scheduler database authorization probe failed",
            Self::DatabaseIdentityMismatch => {
                "CI scheduler and runtime credentials target different database identities"
            }
            Self::RolesNotDistinct => {
                "CI scheduler and runtime credentials authenticate as the same role"
            }
            Self::LoginRequired => "CI scheduler credential did not authenticate as a login role",
            Self::IdentityChanged => {
                "CI scheduler authorization probe observed an assumed role identity"
            }
            Self::Superuser => "CI scheduler database role is a superuser",
            Self::BypassRls => "CI scheduler database role can bypass row-level security",
            Self::CreateDatabase => "CI scheduler database role can create databases",
            Self::CreateRole => "CI scheduler database role can create roles",
            Self::DatabaseCreate => {
                "CI scheduler database role can create schemas in the target database"
            }
            Self::PublicSchemaUsageMissing => {
                "CI scheduler database role cannot use the application schema"
            }
            Self::PublicSchemaCreate => {
                "CI scheduler database role can create objects in the application schema"
            }
            Self::PublicSchemaOwnerMembership => {
                "CI scheduler database role is a member of the application schema owner"
            }
            Self::CapabilityMembershipMissing => {
                "CI scheduler login lacks the region-scheduler capability membership"
            }
            Self::CapabilityNotInherited => {
                "CI scheduler capability membership is not inherited"
            }
            Self::CapabilityCanSetRole => {
                "CI scheduler login can assume the capability role"
            }
            Self::UnexpectedMembership => {
                "CI scheduler login has a role membership beyond its scheduler capability"
            }
            Self::RegionUnmapped => "CI scheduler login has no server-owned region mapping",
            Self::RegionMismatch => {
                "CI scheduler server-owned region does not match the configured region"
            }
            Self::InsufficientPrivileges => {
                "CI scheduler database role lacks required queue or run-discovery privileges"
            }
            Self::ExcessPrivileges => {
                "CI scheduler database role has privileges outside claim/reap/run discovery"
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for CiSchedulerDbError {}

/// Validated provider that can yield only scheduler capabilities, never a raw pool.
pub struct CiSchedulerDbProvider {
    pool: PgPool,
}

impl CiSchedulerDbProvider {
    /// Connect after migrations and validate the scheduler login against the active runtime pool.
    pub async fn connect(
        config: CiSchedulerDbConfig,
        runtime_pool: &PgPool,
    ) -> Result<Self, CiSchedulerDbError> {
        let pool = connect_pool_with_reset(
            &config.database_url,
            &config.region,
            SCHEDULER_MAX_CONNECTIONS,
        )
        .await
        .map_err(|_| CiSchedulerDbError::ConnectionFailed)?;
        if let Err(error) = validate_scheduler_pool(&pool, runtime_pool, &config.region).await {
            pool.close().await;
            return Err(error);
        }
        Ok(Self { pool })
    }

    /// Produce the region-wide job claim/reap capability.
    pub fn region_queue_store(&self) -> CiRegionQueueStore {
        CiRegionQueueStore::with_pg(self.pool.clone())
    }

    /// Produce the column-minimal queued-run discovery capability.
    pub fn region_run_discovery(&self) -> CiRegionRunDiscovery {
        CiRegionRunDiscovery::with_pg(self.pool.clone())
    }
}

#[derive(Debug)]
struct SchedulerProbe {
    database: String,
    database_oid: i64,
    server_address: Option<String>,
    server_port: Option<i32>,
    session_user: String,
    current_user: String,
    login: bool,
    inherit: bool,
    superuser: bool,
    bypass_rls: bool,
    create_database: bool,
    create_role: bool,
    database_create: bool,
    public_schema_usage: bool,
    public_schema_create: bool,
    public_schema_owner_member: bool,
    capability_member: bool,
    capability_usage: bool,
    membership_inherit: bool,
    membership_set: bool,
    unexpected_membership: bool,
    job_select: bool,
    job_update_state: bool,
    job_update_owner: bool,
    job_update_expiry: bool,
    job_update_epoch: bool,
    job_update_nonce: bool,
    job_update_claim_started_at: bool,
    job_update_claim_expires_at: bool,
    /// CT-007 lease/topology reconciliation: an EXPLICIT negative. The claim window is immutable
    /// dispatch authority written by the app role; the scheduler only reads it when sizing
    /// `claim_expires_at`. The dynamic excess-column check below would also catch a stray grant, but
    /// a named probe makes the intent unmissable to the next person adding a column grant.
    job_update_claim_window: bool,
    fair_select: bool,
    run_select_tenant: bool,
    run_select_region: bool,
    run_select_state: bool,
    run_select_created_at: bool,
    run_select_run_id: bool,
    run_select_wf_run_id: bool,
    workflow_select_tenant: bool,
    workflow_select_region: bool,
    workflow_select_run_id: bool,
    workflow_select_type: bool,
    workflow_select_state: bool,
    /// CT-007 lease/topology reconciliation: the superseded-definition boot guard's one extra
    /// column (`ci_0020g`). Column-scoped, read-only, and added to the excess-column allowlist by
    /// exactly this one name.
    workflow_select_version: bool,
    workflow_select_partition: bool,
    workflow_select_created_at: bool,
    parent_attempt_select: bool,
    prelaunch_usage_select: bool,
    prelaunch_usage_update_status: bool,
    prelaunch_usage_update_resolved_at: bool,
    /// **CT-007 phase-credential generations: an EXPLICIT negative.** The scheduler role must hold
    /// NO privilege at all on `ci_job_credential_generation` — neither reaping nor renewal reads it,
    /// and the credential log is the durable authority a phase gate consults. The dynamic
    /// unrelated-table check below would also catch a stray grant, but a named probe makes the
    /// intent unmissable to the next person adding a grant migration.
    credential_generation_privilege: bool,
    mapping_function_execute: bool,
    excess_privilege: bool,
}

#[derive(Debug)]
struct RuntimeIdentity {
    database: String,
    database_oid: i64,
    server_address: Option<String>,
    server_port: Option<i32>,
    current_user: String,
}

async fn validate_scheduler_pool(
    scheduler_pool: &PgPool,
    runtime_pool: &PgPool,
    configured_region: &str,
) -> Result<(), CiSchedulerDbError> {
    let runtime = runtime_identity(runtime_pool).await?;
    let scheduler = scheduler_probe(scheduler_pool).await?;
    validate_probe_before_mapping(&scheduler, &runtime)?;
    let mapped_region = scheduler_region(scheduler_pool).await?;
    validate_mapped_region(mapped_region.as_deref(), configured_region)
}

fn validate_probe_before_mapping(
    scheduler: &SchedulerProbe,
    runtime: &RuntimeIdentity,
) -> Result<(), CiSchedulerDbError> {
    if scheduler.database != runtime.database
        || scheduler.database_oid != runtime.database_oid
        || scheduler.server_address != runtime.server_address
        || scheduler.server_port != runtime.server_port
    {
        return Err(CiSchedulerDbError::DatabaseIdentityMismatch);
    }
    if scheduler.session_user == runtime.current_user {
        return Err(CiSchedulerDbError::RolesNotDistinct);
    }
    if scheduler.session_user != scheduler.current_user {
        return Err(CiSchedulerDbError::IdentityChanged);
    }
    if !scheduler.login {
        return Err(CiSchedulerDbError::LoginRequired);
    }
    if scheduler.superuser {
        return Err(CiSchedulerDbError::Superuser);
    }
    if scheduler.bypass_rls {
        return Err(CiSchedulerDbError::BypassRls);
    }
    if scheduler.create_database {
        return Err(CiSchedulerDbError::CreateDatabase);
    }
    if scheduler.create_role {
        return Err(CiSchedulerDbError::CreateRole);
    }
    if scheduler.database_create {
        return Err(CiSchedulerDbError::DatabaseCreate);
    }
    if !scheduler.public_schema_usage {
        return Err(CiSchedulerDbError::PublicSchemaUsageMissing);
    }
    if scheduler.public_schema_create {
        return Err(CiSchedulerDbError::PublicSchemaCreate);
    }
    if scheduler.public_schema_owner_member {
        return Err(CiSchedulerDbError::PublicSchemaOwnerMembership);
    }
    if !scheduler.capability_member {
        return Err(CiSchedulerDbError::CapabilityMembershipMissing);
    }
    if !scheduler.inherit || !scheduler.capability_usage || !scheduler.membership_inherit {
        return Err(CiSchedulerDbError::CapabilityNotInherited);
    }
    if scheduler.membership_set {
        return Err(CiSchedulerDbError::CapabilityCanSetRole);
    }
    if scheduler.unexpected_membership {
        return Err(CiSchedulerDbError::UnexpectedMembership);
    }
    if !(scheduler.job_select
        && scheduler.job_update_state
        && scheduler.job_update_owner
        && scheduler.job_update_expiry
        && scheduler.job_update_epoch
        && scheduler.job_update_nonce
        && scheduler.job_update_claim_started_at
        && scheduler.job_update_claim_expires_at
        && scheduler.fair_select
        && scheduler.run_select_tenant
        && scheduler.run_select_region
        && scheduler.run_select_state
        && scheduler.run_select_created_at
        && scheduler.run_select_run_id
        && scheduler.run_select_wf_run_id
        && scheduler.workflow_select_tenant
        && scheduler.workflow_select_region
        && scheduler.workflow_select_run_id
        && scheduler.workflow_select_type
        && scheduler.workflow_select_state
        && scheduler.workflow_select_version
        && scheduler.workflow_select_partition
        && scheduler.workflow_select_created_at
        && scheduler.parent_attempt_select
        && scheduler.prelaunch_usage_select
        && scheduler.prelaunch_usage_update_status
        && scheduler.prelaunch_usage_update_resolved_at
        && scheduler.mapping_function_execute)
    {
        return Err(CiSchedulerDbError::InsufficientPrivileges);
    }
    // CT-007 lease/topology reconciliation: the claim window is READ-ONLY to the scheduler. The
    // dynamic excess-column check already covers it; this names it so the guarantee is explicit.
    if scheduler.job_update_claim_window {
        return Err(CiSchedulerDbError::ExcessPrivileges);
    }
    // CT-007 phase-credential generations: the credential log is invisible to the scheduler role.
    if scheduler.credential_generation_privilege {
        return Err(CiSchedulerDbError::ExcessPrivileges);
    }
    if scheduler.excess_privilege {
        return Err(CiSchedulerDbError::ExcessPrivileges);
    }
    Ok(())
}

fn validate_mapped_region(
    mapped_region: Option<&str>,
    configured_region: &str,
) -> Result<(), CiSchedulerDbError> {
    let mapped_region = mapped_region.ok_or(CiSchedulerDbError::RegionUnmapped)?;
    if mapped_region != configured_region {
        return Err(CiSchedulerDbError::RegionMismatch);
    }
    Ok(())
}

async fn scheduler_region(pool: &PgPool) -> Result<Option<String>, CiSchedulerDbError> {
    sqlx::query_scalar("SELECT public.myelin_ci_scheduler_region()")
        .fetch_one(pool)
        .await
        .map_err(|_| CiSchedulerDbError::ProbeFailed)
}

async fn runtime_identity(pool: &PgPool) -> Result<RuntimeIdentity, CiSchedulerDbError> {
    let row = sqlx::query(
        "SELECT current_database() AS database,
                (SELECT oid::bigint FROM pg_catalog.pg_database
                  WHERE datname = current_database()) AS database_oid,
                inet_server_addr()::text AS server_address,
                inet_server_port() AS server_port,
                current_user::text AS current_user",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| CiSchedulerDbError::ProbeFailed)?;
    Ok(RuntimeIdentity {
        database: row.get("database"),
        database_oid: row.get("database_oid"),
        server_address: row.get("server_address"),
        server_port: row.get("server_port"),
        current_user: row.get("current_user"),
    })
}

async fn scheduler_probe(pool: &PgPool) -> Result<SchedulerProbe, CiSchedulerDbError> {
    let row = sqlx::query(
        "SELECT current_database() AS database,
                (SELECT oid::bigint FROM pg_catalog.pg_database
                  WHERE datname = current_database()) AS database_oid,
                inet_server_addr()::text AS server_address,
                inet_server_port() AS server_port,
                session_user::text AS session_user,
                current_user::text AS current_user,
                login.rolcanlogin AS login,
                login.rolinherit AS inherit,
                login.rolsuper AS superuser,
                login.rolbypassrls AS bypass_rls,
                login.rolcreatedb AS create_database,
                login.rolcreaterole AS create_role,
                pg_catalog.has_database_privilege(
                  session_user, current_database(), 'CREATE'
                ) AS database_create,
                pg_catalog.has_schema_privilege(
                  session_user, 'public', 'USAGE'
                ) AS public_schema_usage,
                pg_catalog.has_schema_privilege(
                  session_user, 'public', 'CREATE'
                ) AS public_schema_create,
                pg_catalog.pg_has_role(
                  session_user,
                  (SELECT namespace.nspowner
                     FROM pg_catalog.pg_namespace AS namespace
                    WHERE namespace.nspname = 'public'),
                  'MEMBER'
                ) AS public_schema_owner_member,
                pg_catalog.pg_has_role(session_user, $1, 'MEMBER') AS capability_member,
                pg_catalog.pg_has_role(session_user, $1, 'USAGE') AS capability_usage,
                COALESCE(membership.inherit_option, false) AS membership_inherit,
                COALESCE(membership.set_option, false) AS membership_set,
                EXISTS (
                  SELECT 1 FROM pg_catalog.pg_roles AS granted
                   WHERE granted.rolname <> session_user
                     AND granted.rolname <> $1
                     AND pg_catalog.pg_has_role(session_user, granted.oid, 'MEMBER')
                ) AS unexpected_membership,
                pg_catalog.has_table_privilege(session_user, 'public.job_queue', 'SELECT') AS job_select,
                pg_catalog.has_column_privilege(session_user, 'public.job_queue', 'state', 'UPDATE') AS job_update_state,
                pg_catalog.has_column_privilege(session_user, 'public.job_queue', 'lease_owner', 'UPDATE') AS job_update_owner,
                pg_catalog.has_column_privilege(session_user, 'public.job_queue', 'lease_expires', 'UPDATE') AS job_update_expiry,
                pg_catalog.has_column_privilege(session_user, 'public.job_queue', 'lease_epoch', 'UPDATE') AS job_update_epoch,
                pg_catalog.has_column_privilege(session_user, 'public.job_queue', 'claim_nonce', 'UPDATE') AS job_update_nonce,
                pg_catalog.has_column_privilege(session_user, 'public.job_queue', 'claim_started_at', 'UPDATE') AS job_update_claim_started_at,
                pg_catalog.has_column_privilege(session_user, 'public.job_queue', 'claim_expires_at', 'UPDATE') AS job_update_claim_expires_at,
                pg_catalog.has_column_privilege(session_user, 'public.job_queue', 'claim_window_secs', 'UPDATE') AS job_update_claim_window,
                pg_catalog.has_table_privilege(session_user, 'public.fair_deficit', 'SELECT') AS fair_select,
                pg_catalog.has_column_privilege(session_user, 'public.ci_run', 'tenant_id', 'SELECT') AS run_select_tenant,
                pg_catalog.has_column_privilege(session_user, 'public.ci_run', 'region', 'SELECT') AS run_select_region,
                pg_catalog.has_column_privilege(session_user, 'public.ci_run', 'state', 'SELECT') AS run_select_state,
                pg_catalog.has_column_privilege(session_user, 'public.ci_run', 'created_at', 'SELECT') AS run_select_created_at,
                pg_catalog.has_column_privilege(session_user, 'public.ci_run', 'run_id', 'SELECT') AS run_select_run_id,
                pg_catalog.has_column_privilege(session_user, 'public.ci_run', 'wf_run_id', 'SELECT') AS run_select_wf_run_id,
                pg_catalog.has_column_privilege(session_user, 'public.workflow_run', 'tenant_id', 'SELECT') AS workflow_select_tenant,
                pg_catalog.has_column_privilege(session_user, 'public.workflow_run', 'region', 'SELECT') AS workflow_select_region,
                pg_catalog.has_column_privilege(session_user, 'public.workflow_run', 'run_id', 'SELECT') AS workflow_select_run_id,
                pg_catalog.has_column_privilege(session_user, 'public.workflow_run', 'wf_type', 'SELECT') AS workflow_select_type,
                pg_catalog.has_column_privilege(session_user, 'public.workflow_run', 'state', 'SELECT') AS workflow_select_state,
                pg_catalog.has_column_privilege(session_user, 'public.workflow_run', 'wf_version', 'SELECT') AS workflow_select_version,
                pg_catalog.has_column_privilege(session_user, 'public.workflow_run', 'partition', 'SELECT') AS workflow_select_partition,
                pg_catalog.has_column_privilege(session_user, 'public.workflow_run', 'created_at', 'SELECT') AS workflow_select_created_at,
                pg_catalog.has_table_privilege(session_user, 'public.ci_job_parent_attempt', 'SELECT') AS parent_attempt_select,
                pg_catalog.has_table_privilege(session_user, 'public.ci_job_prelaunch_usage', 'SELECT') AS prelaunch_usage_select,
                pg_catalog.has_column_privilege(session_user, 'public.ci_job_prelaunch_usage', 'status', 'UPDATE') AS prelaunch_usage_update_status,
                pg_catalog.has_column_privilege(session_user, 'public.ci_job_prelaunch_usage', 'resolved_at', 'UPDATE') AS prelaunch_usage_update_resolved_at,
                (
                  pg_catalog.has_table_privilege(
                    session_user, 'public.ci_job_credential_generation', 'SELECT')
                  OR pg_catalog.has_table_privilege(
                    session_user, 'public.ci_job_credential_generation', 'INSERT')
                  OR pg_catalog.has_table_privilege(
                    session_user, 'public.ci_job_credential_generation', 'UPDATE')
                  OR pg_catalog.has_table_privilege(
                    session_user, 'public.ci_job_credential_generation', 'DELETE')
                  OR pg_catalog.has_table_privilege(
                    session_user, 'public.ci_job_credential_generation', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(
                    session_user, 'public.ci_job_credential_generation', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(
                    session_user, 'public.ci_job_credential_generation', 'TRIGGER')
                  OR EXISTS (
                    SELECT 1
                      FROM pg_catalog.pg_attribute AS credential_column
                     WHERE credential_column.attrelid =
                           'public.ci_job_credential_generation'::regclass
                       AND credential_column.attnum > 0
                       AND NOT credential_column.attisdropped
                       AND (
                         pg_catalog.has_column_privilege(
                           session_user, credential_column.attrelid,
                           credential_column.attnum, 'SELECT'
                         )
                         OR pg_catalog.has_column_privilege(
                           session_user, credential_column.attrelid,
                           credential_column.attnum, 'INSERT'
                         )
                         OR pg_catalog.has_column_privilege(
                           session_user, credential_column.attrelid,
                           credential_column.attnum, 'UPDATE'
                         )
                         OR pg_catalog.has_column_privilege(
                           session_user, credential_column.attrelid,
                           credential_column.attnum, 'REFERENCES'
                         )
                       )
                  )
                ) AS credential_generation_privilege,
                pg_catalog.has_function_privilege(
                  session_user, 'public.myelin_ci_scheduler_region()'::regprocedure, 'EXECUTE'
                ) AS mapping_function_execute,
                (
                  pg_catalog.has_table_privilege(session_user, 'public.job_queue', 'INSERT')
                  OR pg_catalog.has_table_privilege(session_user, 'public.job_queue', 'UPDATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.job_queue', 'DELETE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.job_queue', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.job_queue', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(session_user, 'public.job_queue', 'TRIGGER')
                  OR EXISTS (
                    SELECT 1
                      FROM pg_catalog.pg_attribute AS column_grant
                     WHERE column_grant.attrelid = 'public.job_queue'::regclass
                       AND column_grant.attnum > 0
                       AND NOT column_grant.attisdropped
                       AND column_grant.attname NOT IN (
                         'state', 'lease_owner', 'lease_expires', 'lease_epoch', 'claim_nonce',
                         'claim_started_at', 'claim_expires_at'
                       )
                       AND pg_catalog.has_column_privilege(
                         session_user, column_grant.attrelid, column_grant.attnum, 'UPDATE'
                       )
                  )
                  OR pg_catalog.has_table_privilege(session_user, 'public.fair_deficit', 'INSERT')
                  OR pg_catalog.has_table_privilege(session_user, 'public.fair_deficit', 'UPDATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.fair_deficit', 'DELETE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.fair_deficit', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.fair_deficit', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(session_user, 'public.fair_deficit', 'TRIGGER')
                  OR pg_catalog.has_table_privilege(session_user, 'public.ci_run', 'INSERT')
                  OR pg_catalog.has_table_privilege(session_user, 'public.ci_run', 'UPDATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.ci_run', 'DELETE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.ci_run', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.ci_run', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(session_user, 'public.ci_run', 'TRIGGER')
                  OR EXISTS (
                    SELECT 1
                      FROM pg_catalog.pg_attribute AS run_column
                     WHERE run_column.attrelid = 'public.ci_run'::regclass
                       AND run_column.attnum > 0
                       AND NOT run_column.attisdropped
                       AND run_column.attname NOT IN (
                         'tenant_id', 'region', 'state', 'created_at', 'run_id', 'wf_run_id'
                       )
                       AND pg_catalog.has_column_privilege(
                         session_user, run_column.attrelid, run_column.attnum, 'SELECT'
                       )
                  )
                  OR EXISTS (
                    SELECT 1
                      FROM pg_catalog.pg_class AS unrelated
                      JOIN pg_catalog.pg_namespace AS namespace
                        ON namespace.oid = unrelated.relnamespace
                     WHERE unrelated.relkind IN ('r', 'p', 'f', 'v', 'm')
                       AND namespace.nspname NOT IN ('pg_catalog', 'information_schema')
                       AND namespace.nspname NOT LIKE 'pg_toast%'
                       AND unrelated.oid NOT IN (
                         'public.job_queue'::regclass,
                         'public.fair_deficit'::regclass,
                         'public.ci_run'::regclass,
                         'public.workflow_run'::regclass,
                         'public.ci_job_parent_attempt'::regclass,
                         'public.ci_job_prelaunch_usage'::regclass,
                         'public.myelin_ci_scheduler_region_map'::regclass
                       )
                       AND (
                         pg_catalog.has_table_privilege(session_user, unrelated.oid, 'SELECT')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'INSERT')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'UPDATE')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'DELETE')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'TRUNCATE')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'REFERENCES')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'TRIGGER')
                       )
                  )
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_parent_attempt', 'INSERT')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_parent_attempt', 'UPDATE')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_parent_attempt', 'DELETE')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_parent_attempt', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_parent_attempt', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_parent_attempt', 'TRIGGER')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_prelaunch_usage', 'INSERT')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_prelaunch_usage', 'UPDATE')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_prelaunch_usage', 'DELETE')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_prelaunch_usage', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_prelaunch_usage', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(
                       session_user, 'public.ci_job_prelaunch_usage', 'TRIGGER')
                  OR EXISTS (
                    SELECT 1
                      FROM pg_catalog.pg_attribute AS prelaunch_column
                     WHERE prelaunch_column.attrelid =
                           'public.ci_job_prelaunch_usage'::regclass
                       AND prelaunch_column.attnum > 0
                       AND NOT prelaunch_column.attisdropped
                       AND prelaunch_column.attname NOT IN ('status', 'resolved_at')
                       AND pg_catalog.has_column_privilege(
                         session_user, prelaunch_column.attrelid,
                         prelaunch_column.attnum, 'UPDATE'
                       )
                  )
                  OR pg_catalog.has_table_privilege(session_user, 'public.workflow_run', 'INSERT')
                  OR pg_catalog.has_table_privilege(session_user, 'public.workflow_run', 'UPDATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.workflow_run', 'DELETE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.workflow_run', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.workflow_run', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(session_user, 'public.workflow_run', 'TRIGGER')
                  OR EXISTS (
                    SELECT 1
                      FROM pg_catalog.pg_attribute AS workflow_column
                     WHERE workflow_column.attrelid = 'public.workflow_run'::regclass
                       AND workflow_column.attnum > 0
                       AND NOT workflow_column.attisdropped
                       AND workflow_column.attname NOT IN (
                         'tenant_id', 'region', 'run_id', 'wf_type', 'wf_version', 'state',
                         'partition', 'created_at'
                       )
                       AND pg_catalog.has_column_privilege(
                         session_user, workflow_column.attrelid, workflow_column.attnum, 'SELECT'
                       )
                  )
                  OR pg_catalog.has_table_privilege(session_user,
                       'public.myelin_ci_scheduler_region_map', 'SELECT')
                  OR pg_catalog.has_table_privilege(session_user,
                       'public.myelin_ci_scheduler_region_map', 'INSERT')
                  OR pg_catalog.has_table_privilege(session_user,
                       'public.myelin_ci_scheduler_region_map', 'UPDATE')
                  OR pg_catalog.has_table_privilege(session_user,
                       'public.myelin_ci_scheduler_region_map', 'DELETE')
                  OR pg_catalog.has_table_privilege(session_user,
                       'public.myelin_ci_scheduler_region_map', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(session_user,
                       'public.myelin_ci_scheduler_region_map', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(session_user,
                       'public.myelin_ci_scheduler_region_map', 'TRIGGER')
                  OR EXISTS (
                    SELECT 1
                      FROM pg_catalog.pg_attribute AS mapping_column
                     WHERE mapping_column.attrelid =
                           'public.myelin_ci_scheduler_region_map'::regclass
                       AND mapping_column.attnum > 0
                       AND NOT mapping_column.attisdropped
                       AND (
                         pg_catalog.has_column_privilege(
                           session_user, mapping_column.attrelid, mapping_column.attnum, 'SELECT'
                         )
                         OR pg_catalog.has_column_privilege(
                           session_user, mapping_column.attrelid, mapping_column.attnum, 'INSERT'
                         )
                         OR pg_catalog.has_column_privilege(
                           session_user, mapping_column.attrelid, mapping_column.attnum, 'UPDATE'
                         )
                         OR pg_catalog.has_column_privilege(
                           session_user, mapping_column.attrelid, mapping_column.attnum, 'REFERENCES'
                         )
                       )
                  )
                ) AS excess_privilege
           FROM pg_catalog.pg_roles AS login
           LEFT JOIN (
             SELECT member_role.rolname AS member_name,
                    granted_role.rolname AS granted_name,
                    auth.inherit_option,
                    auth.set_option
               FROM pg_catalog.pg_auth_members AS auth
               JOIN pg_catalog.pg_roles AS member_role ON member_role.oid = auth.member
               JOIN pg_catalog.pg_roles AS granted_role ON granted_role.oid = auth.roleid
           ) AS membership
             ON membership.member_name = session_user
            AND membership.granted_name = $1
          WHERE login.rolname = session_user",
    )
    .bind(SCHEDULER_CAPABILITY_ROLE)
    .fetch_one(pool)
    .await
    .map_err(|_| CiSchedulerDbError::ProbeFailed)?;

    Ok(SchedulerProbe {
        database: row.get("database"),
        database_oid: row.get("database_oid"),
        server_address: row.get("server_address"),
        server_port: row.get("server_port"),
        session_user: row.get("session_user"),
        current_user: row.get("current_user"),
        login: row.get("login"),
        inherit: row.get("inherit"),
        superuser: row.get("superuser"),
        bypass_rls: row.get("bypass_rls"),
        create_database: row.get("create_database"),
        create_role: row.get("create_role"),
        database_create: row.get("database_create"),
        public_schema_usage: row.get("public_schema_usage"),
        public_schema_create: row.get("public_schema_create"),
        public_schema_owner_member: row.get("public_schema_owner_member"),
        capability_member: row.get("capability_member"),
        capability_usage: row.get("capability_usage"),
        membership_inherit: row.get("membership_inherit"),
        membership_set: row.get("membership_set"),
        unexpected_membership: row.get("unexpected_membership"),
        job_select: row.get("job_select"),
        job_update_state: row.get("job_update_state"),
        job_update_owner: row.get("job_update_owner"),
        job_update_expiry: row.get("job_update_expiry"),
        job_update_epoch: row.get("job_update_epoch"),
        job_update_nonce: row.get("job_update_nonce"),
        job_update_claim_started_at: row.get("job_update_claim_started_at"),
        job_update_claim_expires_at: row.get("job_update_claim_expires_at"),
        job_update_claim_window: row.get("job_update_claim_window"),
        fair_select: row.get("fair_select"),
        run_select_tenant: row.get("run_select_tenant"),
        run_select_region: row.get("run_select_region"),
        run_select_state: row.get("run_select_state"),
        run_select_created_at: row.get("run_select_created_at"),
        run_select_run_id: row.get("run_select_run_id"),
        run_select_wf_run_id: row.get("run_select_wf_run_id"),
        workflow_select_tenant: row.get("workflow_select_tenant"),
        workflow_select_region: row.get("workflow_select_region"),
        workflow_select_run_id: row.get("workflow_select_run_id"),
        workflow_select_type: row.get("workflow_select_type"),
        workflow_select_state: row.get("workflow_select_state"),
        workflow_select_version: row.get("workflow_select_version"),
        workflow_select_partition: row.get("workflow_select_partition"),
        workflow_select_created_at: row.get("workflow_select_created_at"),
        parent_attempt_select: row.get("parent_attempt_select"),
        prelaunch_usage_select: row.get("prelaunch_usage_select"),
        prelaunch_usage_update_status: row.get("prelaunch_usage_update_status"),
        prelaunch_usage_update_resolved_at: row.get("prelaunch_usage_update_resolved_at"),
        credential_generation_privilege: row.get("credential_generation_privilege"),
        mapping_function_execute: row.get("mapping_function_execute"),
        excess_privilege: row.get("excess_privilege"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform() -> MyelinConfig {
        let mut config = MyelinConfig::dev();
        config.database_url = "postgres://app:secret@db/myelin".into();
        config.database_migration_url = "postgres://admin:secret@db/myelin".into();
        config.region = "fr-par".into();
        config
    }

    #[test]
    fn config_requires_a_third_credential_and_redacts_it() {
        let platform = platform();
        assert_eq!(
            CiSchedulerDbConfig::from_parts(
                platform.database_url.clone(),
                &platform.database_url,
                &platform.database_migration_url,
                platform.region.clone(),
            ),
            Err(CiSchedulerDbError::CredentialsNotDistinct)
        );
        let config = CiSchedulerDbConfig::from_parts(
            "postgres://scheduler:DO_NOT_PRINT@db/myelin".into(),
            &platform.database_url,
            &platform.database_migration_url,
            platform.region,
        )
        .unwrap();
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("DO_NOT_PRINT"));
        assert!(!rendered.contains("postgres://"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn every_error_message_is_credential_free() {
        let errors = [
            CiSchedulerDbError::MissingDatabaseUrl,
            CiSchedulerDbError::NonUnicodeDatabaseUrl,
            CiSchedulerDbError::EmptyDatabaseUrl,
            CiSchedulerDbError::CredentialsNotDistinct,
            CiSchedulerDbError::EmptyRegion,
            CiSchedulerDbError::ConnectionFailed,
            CiSchedulerDbError::ProbeFailed,
            CiSchedulerDbError::DatabaseIdentityMismatch,
            CiSchedulerDbError::RolesNotDistinct,
            CiSchedulerDbError::LoginRequired,
            CiSchedulerDbError::IdentityChanged,
            CiSchedulerDbError::Superuser,
            CiSchedulerDbError::BypassRls,
            CiSchedulerDbError::CreateDatabase,
            CiSchedulerDbError::CreateRole,
            CiSchedulerDbError::DatabaseCreate,
            CiSchedulerDbError::PublicSchemaUsageMissing,
            CiSchedulerDbError::PublicSchemaCreate,
            CiSchedulerDbError::PublicSchemaOwnerMembership,
            CiSchedulerDbError::CapabilityMembershipMissing,
            CiSchedulerDbError::CapabilityNotInherited,
            CiSchedulerDbError::CapabilityCanSetRole,
            CiSchedulerDbError::UnexpectedMembership,
            CiSchedulerDbError::RegionUnmapped,
            CiSchedulerDbError::RegionMismatch,
            CiSchedulerDbError::InsufficientPrivileges,
            CiSchedulerDbError::ExcessPrivileges,
        ];
        for error in errors {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("postgres://"));
            assert!(!rendered.contains("password"));
        }
    }
}
