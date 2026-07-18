//! Dedicated PostgreSQL provider for the cross-tenant, region-scoped CI scheduler.
//!
//! The ordinary runtime pool authenticates as the tenant application role and must never claim or
//! reap across tenants. This module opens the separately configured scheduler credential only after
//! migrations, validates its server-owned region mapping and exact least-privilege posture, and
//! exposes the pool solely as [`crate::CiRegionQueueStore`].

use myelin_config::MyelinConfig;
use myelin_storage::connect_pool_with_reset;
use sqlx::{PgPool, Row};

use crate::CiRegionQueueStore;

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
                "CI scheduler database role lacks required queue privileges"
            }
            Self::ExcessPrivileges => {
                "CI scheduler database role has privileges outside claim/reap"
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for CiSchedulerDbError {}

/// Validated provider that can yield only the region queue capability, never a raw pool.
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

    /// Produce the only capability the scheduler credential is allowed to drive.
    pub fn region_queue_store(&self) -> CiRegionQueueStore {
        CiRegionQueueStore::with_pg(self.pool.clone())
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
    mapped_region: Option<String>,
    login: bool,
    inherit: bool,
    superuser: bool,
    bypass_rls: bool,
    create_database: bool,
    create_role: bool,
    capability_member: bool,
    capability_usage: bool,
    membership_inherit: bool,
    membership_set: bool,
    unexpected_membership: bool,
    job_select: bool,
    job_update_state: bool,
    job_update_owner: bool,
    job_update_expiry: bool,
    fair_select: bool,
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
    let scheduler = scheduler_probe(scheduler_pool).await?;
    let runtime = runtime_identity(runtime_pool).await?;
    validate_probe(&scheduler, &runtime, configured_region)
}

fn validate_probe(
    scheduler: &SchedulerProbe,
    runtime: &RuntimeIdentity,
    configured_region: &str,
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
    let mapped_region = scheduler
        .mapped_region
        .as_deref()
        .ok_or(CiSchedulerDbError::RegionUnmapped)?;
    if mapped_region != configured_region {
        return Err(CiSchedulerDbError::RegionMismatch);
    }
    if !(scheduler.job_select
        && scheduler.job_update_state
        && scheduler.job_update_owner
        && scheduler.job_update_expiry
        && scheduler.fair_select
        && scheduler.mapping_function_execute)
    {
        return Err(CiSchedulerDbError::InsufficientPrivileges);
    }
    if scheduler.excess_privilege {
        return Err(CiSchedulerDbError::ExcessPrivileges);
    }
    Ok(())
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
                public.myelin_ci_scheduler_region() AS mapped_region,
                login.rolcanlogin AS login,
                login.rolinherit AS inherit,
                login.rolsuper AS superuser,
                login.rolbypassrls AS bypass_rls,
                login.rolcreatedb AS create_database,
                login.rolcreaterole AS create_role,
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
                pg_catalog.has_table_privilege(session_user, 'public.fair_deficit', 'SELECT') AS fair_select,
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
                       AND column_grant.attname NOT IN ('state', 'lease_owner', 'lease_expires')
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
        mapped_region: row.get("mapped_region"),
        login: row.get("login"),
        inherit: row.get("inherit"),
        superuser: row.get("superuser"),
        bypass_rls: row.get("bypass_rls"),
        create_database: row.get("create_database"),
        create_role: row.get("create_role"),
        capability_member: row.get("capability_member"),
        capability_usage: row.get("capability_usage"),
        membership_inherit: row.get("membership_inherit"),
        membership_set: row.get("membership_set"),
        unexpected_membership: row.get("unexpected_membership"),
        job_select: row.get("job_select"),
        job_update_state: row.get("job_update_state"),
        job_update_owner: row.get("job_update_owner"),
        job_update_expiry: row.get("job_update_expiry"),
        fair_select: row.get("fair_select"),
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
