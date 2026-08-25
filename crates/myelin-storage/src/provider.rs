use myelin_config::{ConfigError, Mode, MyelinConfig};
use sqlx::postgres::PgPool;

use crate::backend::{self, Backend};
use crate::blob::BlobStore;
use crate::cache::{Cache, CacheError};
use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::pg_migrator::PgMigrator;
use crate::tenant_tx::{connect_pool_with_reset, with_tenant_tx, TxScope};

pub const DEFAULT_MAX_CONNECTIONS: u32 = 32;

pub const DATABASE_MAX_CONNECTIONS_ENV: &str = "MYELIN_DATABASE_MAX_CONNECTIONS";

fn configured_database_max_connections() -> Result<u32, ConfigError> {
    match std::env::var(DATABASE_MAX_CONNECTIONS_ENV) {
        Ok(value) => parse_database_max_connections(&value),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_MAX_CONNECTIONS),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid {
            var: DATABASE_MAX_CONNECTIONS_ENV,
            reason: "must be valid UTF-8".into(),
        }),
    }
}

fn parse_database_max_connections(value: &str) -> Result<u32, ConfigError> {
    let parsed = value
        .trim()
        .parse::<u32>()
        .map_err(|_| ConfigError::Invalid {
            var: DATABASE_MAX_CONNECTIONS_ENV,
            reason: "must be a positive integer".into(),
        })?;
    if parsed == 0 {
        return Err(ConfigError::Invalid {
            var: DATABASE_MAX_CONNECTIONS_ENV,
            reason: "must be greater than zero".into(),
        });
    }
    Ok(parsed)
}

#[derive(Debug)]
pub enum ProviderError {
    Config(ConfigError),
    Pg(PgError),
    Bootstrap(BootstrapError),
}

impl core::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProviderError::Config(e) => write!(f, "substrate provider config error: {e}"),
            ProviderError::Pg(e) => write!(f, "substrate provider backend error: {e}"),
            ProviderError::Bootstrap(e) => write!(f, "substrate provider bootstrap error: {e}"),
        }
    }
}

impl std::error::Error for ProviderError {}

impl From<ConfigError> for ProviderError {
    fn from(e: ConfigError) -> Self {
        ProviderError::Config(e)
    }
}

impl From<PgError> for ProviderError {
    fn from(e: PgError) -> Self {
        ProviderError::Pg(e)
    }
}

impl From<BootstrapError> for ProviderError {
    fn from(e: BootstrapError) -> Self {
        ProviderError::Bootstrap(e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapError {
    CredentialsNotDistinct,
    MigrationConnect,
    RuntimeConnect,
    ValidationProbe,
    RuntimeCapabilityUnvalidated,
    DatabaseIdentityMismatch,
    RolesNotDistinct,
    RuntimeSuperuser,
    RuntimeBypassRls,
    RuntimeCreateDatabase,
    RuntimeCreateRole,
    RuntimeOwnsSchema,
    RuntimeCanCreateSchemaObjects,
    RuntimeCannotUseSchema,
    RuntimeMemberOfMigrationRole,
    RuntimeElevatedMembership,
    MigrationCannotManageSchema,
    MigrationCannotUseAdvisoryLock,
}

impl core::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            BootstrapError::CredentialsNotDistinct => {
                "runtime and migration database credentials must be distinct"
            }
            BootstrapError::MigrationConnect => "migration database connection failed",
            BootstrapError::RuntimeConnect => "runtime database connection failed",
            BootstrapError::ValidationProbe => "database role validation probe failed",
            BootstrapError::RuntimeCapabilityUnvalidated => {
                "database provider lacks validated constrained-runtime capability"
            }
            BootstrapError::DatabaseIdentityMismatch => {
                "runtime and migration connections target different database identities"
            }
            BootstrapError::RolesNotDistinct => {
                "runtime and migration credentials authenticate as the same role"
            }
            BootstrapError::RuntimeSuperuser => "runtime database role is a superuser",
            BootstrapError::RuntimeBypassRls => {
                "runtime database role can bypass row-level security"
            }
            BootstrapError::RuntimeCreateDatabase => "runtime database role can create databases",
            BootstrapError::RuntimeCreateRole => "runtime database role can create roles",
            BootstrapError::RuntimeOwnsSchema => {
                "runtime database role owns or can assume ownership of the application schema"
            }
            BootstrapError::RuntimeCanCreateSchemaObjects => {
                "runtime database role can create application-schema objects"
            }
            BootstrapError::RuntimeCannotUseSchema => {
                "runtime database role cannot use the application schema"
            }
            BootstrapError::RuntimeMemberOfMigrationRole => {
                "runtime database role is a member of the migration role"
            }
            BootstrapError::RuntimeElevatedMembership => {
                "runtime database role can assume an elevated role"
            }
            BootstrapError::MigrationCannotManageSchema => {
                "migration database role cannot manage the application schema"
            }
            BootstrapError::MigrationCannotUseAdvisoryLock => {
                "migration database role cannot use the advisory migration lock"
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for BootstrapError {}

pub fn foundation_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0000_outbox", myelin_events::OUTBOX_MIGRATION),
        Migration::plain(
            "0001_consumer_dedup",
            myelin_events::CONSUMER_DEDUP_MIGRATION,
        ),
        Migration::plain(
            "0002_consumer_dead_letter",
            myelin_events::CONSUMER_DEAD_LETTER_MIGRATION,
        ),
        Migration::plain(
            "0003_outbox_quarantine",
            myelin_events::OUTBOX_QUARANTINE_MIGRATION,
        ),
        Migration::plain(
            "0004_consumer_delivery_quarantine",
            myelin_events::CONSUMER_DELIVERY_QUARANTINE_MIGRATION,
        ),
        Migration::plain(
            "0005_outbox_publisher_grants",
            myelin_events::OUTBOX_PUBLISHER_GRANTS_MIGRATION,
        ),
        Migration::plain(
            "0006_outbox_publisher_grant_scope",
            myelin_events::OUTBOX_PUBLISHER_GRANT_SCOPE_MIGRATION,
        ),
        Migration::plain(
            "0007_outbox_value_invariants_expand",
            myelin_events::OUTBOX_VALUE_INVARIANTS_EXPAND_MIGRATION,
        ),
        Migration::plain(
            "0008_outbox_identity_backfill",
            myelin_events::OUTBOX_IDENTITY_BACKFILL_MIGRATION,
        ),
        Migration::plain(
            "0009_outbox_value_invariants_validate",
            myelin_events::OUTBOX_VALUE_INVARIANTS_VALIDATE_MIGRATION,
        ),
        Migration::plain(
            "0009a_outbox_quarantine_resolution",
            myelin_events::OUTBOX_QUARANTINE_RESOLUTION_MIGRATION,
        ),
        Migration::plain(
            "0009b_outbox_chat_aggregate_backfill",
            myelin_events::OUTBOX_CHAT_AGGREGATE_BACKFILL_MIGRATION,
        ),
    ])
}

pub fn durable_migration_groups() -> Vec<Migrations> {
    vec![
        crate::identity_durable::identity_durable_migrations(),
        crate::pseudonym_durable::pseudonym_durable_migrations(),
        crate::placement_durable::placement_durable_migrations(),
        crate::kms_durable::kms_durable_migrations(),
        crate::reserve_settle_durable::reserve_settle_durable_migrations(),
        crate::restore_verify_durable::restore_verify_durable_migrations(),
        crate::reerase_durable::post_pit_durable_migrations(),
        crate::events_durable::bus_erasure_durable_migrations(),
        crate::hitl_gate_durable::hitl_gate_durable_migrations(),
        crate::cell_root_durable::cell_root_durable_migrations(),
        crate::delegation_policy_durable::delegation_policy_durable_migrations(),
        crate::authz_projection_durable::authz_projection_durable_migrations(),
        crate::identity_durable::auth_replay_durable_migrations(),
        crate::agent_wallet::agent_wallet_migrations(),
        crate::identity_durable::identity_project_durable_migrations(),
        crate::identity_durable::identity_agent_durable_migrations(),
        crate::external_agent_run_durable::external_agent_run_durable_migrations(),
        crate::agent_trigger_durable::agent_trigger_durable_migrations(),
        crate::agent_wallet::agent_wallet_charge_migrations(),
        crate::agent_model_step::agent_model_step_migrations(),
        crate::agent_trace_durable::agent_trace_durable_migrations(),
        crate::agent_trigger_durable::agent_trigger_terminal_reason_migrations(),
        crate::agent_tool_effect::agent_tool_effect_migrations(),
        crate::agent_journal_privacy::agent_journal_privacy_migrations(),
        crate::agent_trace_durable::agent_trace_encrypted_only_migrations(),
        crate::agent_trace_durable::agent_trace_erasure_progress_migrations(),
        crate::agent_trigger_durable::agent_trigger_evaluation_diagnostic_migrations(),
        crate::agent_trigger_durable::agent_trigger_owner_list_migrations(),
        crate::reserve_settle_durable::cost_ledger_value_invariant_migrations(),
        crate::identity_durable::identity_tuple_revision_migrations(),
        crate::identity_durable::identity_project_recent_list_migrations(),
        crate::identity_durable::identity_agent_recent_list_migrations(),
        crate::placement_durable::cell_value_invariant_migrations(),
        crate::kms_durable::kms_epoch_invariant_migrations(),
        crate::restore_verify_durable::restore_wal_offset_invariant_migrations(),
        crate::agent_thread_durable::agent_thread_durable_migrations(),
        crate::privacy_request_durable::privacy_request_durable_migrations(),
        crate::agent_trace_durable::agent_trace_erasure_receipt_migrations(),
    ]
}

pub fn all_durable_migrations() -> Migrations {
    Migrations::of(durable_migration_groups().into_iter().flat_map(|g| g.0))
}

#[derive(Debug, PartialEq, Eq)]
struct ConnectionIdentity {
    database: String,
    database_oid: i64,
    server_address: Option<String>,
    server_port: Option<i32>,
    server_version: i32,
    schema: String,
    user: String,
    superuser: bool,
    bypass_rls: bool,
    create_database: bool,
    create_role: bool,
}

type ConnectionIdentityRow = (
    String,
    i64,
    Option<String>,
    Option<i32>,
    i32,
    Option<String>,
    String,
    bool,
    bool,
    bool,
    bool,
);

#[derive(Debug)]
struct SchemaAccess {
    can_assume_owner: bool,
    can_create: bool,
    can_use: bool,
}

async fn connection_identity(pool: &PgPool) -> Result<ConnectionIdentity, BootstrapError> {
    // @tenant-cross-scope: validates PostgreSQL connection and role catalogs before tenant access.
    let row: ConnectionIdentityRow = sqlx::query_as(
        "SELECT current_database() AS database,
                (SELECT oid::bigint FROM pg_database WHERE datname = current_database())
                    AS database_oid,
                inet_server_addr()::text AS server_address,
                inet_server_port() AS server_port,
                current_setting('server_version_num')::integer AS server_version,
                current_schema() AS schema,
                current_user AS user,
                rolsuper AS superuser,
                rolbypassrls AS bypass_rls,
                rolcreatedb AS create_database,
                rolcreaterole AS create_role
           FROM pg_roles
          WHERE rolname = current_user",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| BootstrapError::ValidationProbe)?;

    Ok(ConnectionIdentity {
        database: row.0,
        database_oid: row.1,
        server_address: row.2,
        server_port: row.3,
        server_version: row.4,
        schema: row.5.ok_or(BootstrapError::ValidationProbe)?,
        user: row.6,
        superuser: row.7,
        bypass_rls: row.8,
        create_database: row.9,
        create_role: row.10,
    })
}

async fn schema_access(pool: &PgPool) -> Result<SchemaAccess, BootstrapError> {
    // @tenant-cross-scope: validates current-role schema grants, not application tenant rows.
    let row: (bool, bool, bool) = sqlx::query_as(
        "SELECT pg_has_role(current_user, n.nspowner, 'MEMBER'),
                has_schema_privilege(current_user, n.oid, 'CREATE'),
                has_schema_privilege(current_user, n.oid, 'USAGE')
           FROM pg_namespace n
          WHERE n.oid = current_schema()::regnamespace",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| BootstrapError::ValidationProbe)?;
    Ok(SchemaAccess {
        can_assume_owner: row.0,
        can_create: row.1,
        can_use: row.2,
    })
}

async fn validate_bootstrap_pair(
    migration_pool: &PgPool,
    runtime_pool: &PgPool,
) -> Result<(), BootstrapError> {
    let migration = connection_identity(migration_pool).await?;
    let runtime = connection_identity(runtime_pool).await?;

    if migration.database != runtime.database
        || migration.database_oid != runtime.database_oid
        || migration.server_address != runtime.server_address
        || migration.server_port != runtime.server_port
        || migration.server_version != runtime.server_version
        || migration.schema != runtime.schema
    {
        return Err(BootstrapError::DatabaseIdentityMismatch);
    }
    if migration.user == runtime.user {
        return Err(BootstrapError::RolesNotDistinct);
    }
    if runtime.superuser {
        return Err(BootstrapError::RuntimeSuperuser);
    }
    if runtime.bypass_rls {
        return Err(BootstrapError::RuntimeBypassRls);
    }
    if runtime.create_database {
        return Err(BootstrapError::RuntimeCreateDatabase);
    }
    if runtime.create_role {
        return Err(BootstrapError::RuntimeCreateRole);
    }

    let migration_schema = schema_access(migration_pool).await?;
    if !migration_schema.can_assume_owner
        || !migration_schema.can_create
        || !migration_schema.can_use
    {
        return Err(BootstrapError::MigrationCannotManageSchema);
    }
    // @tenant-cross-scope: validates a PostgreSQL catalog grant for the migration role.
    let can_lock: bool = sqlx::query_scalar(
        "SELECT has_function_privilege(
             current_user,
             'pg_catalog.pg_advisory_lock(bigint)'::regprocedure,
             'EXECUTE'
         )",
    )
    .fetch_one(migration_pool)
    .await
    .map_err(|_| BootstrapError::ValidationProbe)?;
    if !can_lock {
        return Err(BootstrapError::MigrationCannotUseAdvisoryLock);
    }

    let runtime_schema = schema_access(runtime_pool).await?;
    if runtime_schema.can_assume_owner {
        return Err(BootstrapError::RuntimeOwnsSchema);
    }
    if runtime_schema.can_create {
        return Err(BootstrapError::RuntimeCanCreateSchemaObjects);
    }
    if !runtime_schema.can_use {
        return Err(BootstrapError::RuntimeCannotUseSchema);
    }

    // @tenant-cross-scope: rejects runtime membership in the privileged migration role.
    let member_of_migration: bool =
        sqlx::query_scalar("SELECT pg_has_role(current_user, $1, 'MEMBER')")
            .bind(&migration.user)
            .fetch_one(runtime_pool)
            .await
            .map_err(|_| BootstrapError::ValidationProbe)?;
    if member_of_migration {
        return Err(BootstrapError::RuntimeMemberOfMigrationRole);
    }

    // @tenant-cross-scope: rejects any elevated PostgreSQL role membership before serving.
    let elevated_membership: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
               FROM pg_roles candidate
              WHERE candidate.rolname <> current_user
                AND pg_has_role(current_user, candidate.oid, 'MEMBER')
                AND (candidate.rolsuper
                     OR candidate.rolbypassrls
                     OR candidate.rolcreatedb
                     OR candidate.rolcreaterole)
         )",
    )
    .fetch_one(runtime_pool)
    .await
    .map_err(|_| BootstrapError::ValidationProbe)?;
    if elevated_membership {
        return Err(BootstrapError::RuntimeElevatedMembership);
    }

    Ok(())
}

pub struct PgBootstrap {
    migration_pool: PgPool,
    config: MyelinConfig,
    max_connections: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexReadinessSpec<'a> {
    pub index_name: &'a str,
    pub table_name: &'a str,
    pub table_relkind: &'a str,
    pub index_relkind: &'a str,
    pub access_method: &'a str,
    pub key_columns: &'a [&'a str],
    pub predicate: Option<&'a str>,
}

impl<'a> IndexReadinessSpec<'a> {
    pub const fn new(
        index_name: &'a str,
        table_name: &'a str,
        table_relkind: &'a str,
        index_relkind: &'a str,
        access_method: &'a str,
        key_columns: &'a [&'a str],
        predicate: Option<&'a str>,
    ) -> Self {
        Self {
            index_name,
            table_name,
            table_relkind,
            index_relkind,
            access_method,
            key_columns,
            predicate,
        }
    }
}

type IndexReadinessRow = (
    bool,
    bool,
    bool,
    bool,
    String,
    String,
    String,
    String,
    Vec<String>,
    Option<String>,
);

impl PgBootstrap {
    pub async fn from_env(mode: Mode) -> Result<PgBootstrap, ProviderError> {
        Self::connect_configured(MyelinConfig::from_env(mode)?).await
    }

    pub async fn connect_configured(config: MyelinConfig) -> Result<PgBootstrap, ProviderError> {
        Self::connect(config, configured_database_max_connections()?).await
    }

    pub async fn connect(
        config: MyelinConfig,
        max_connections: u32,
    ) -> Result<PgBootstrap, ProviderError> {
        if config.database_url == config.database_migration_url {
            return Err(BootstrapError::CredentialsNotDistinct.into());
        }
        let migration_pool = connect_pool_with_reset(
            &config.database_migration_url,
            &config.region,
            max_connections,
        )
        .await
        .map_err(|_| BootstrapError::MigrationConnect)?;
        let runtime_probe =
            match connect_pool_with_reset(&config.database_url, &config.region, 1).await {
                Ok(pool) => pool,
                Err(_) => {
                    migration_pool.close().await;
                    return Err(BootstrapError::RuntimeConnect.into());
                }
            };
        let validation = validate_bootstrap_pair(&migration_pool, &runtime_probe).await;
        runtime_probe.close().await;
        if let Err(error) = validation {
            migration_pool.close().await;
            return Err(error.into());
        }

        Ok(PgBootstrap {
            migration_pool,
            config,
            max_connections: max_connections.max(1),
        })
    }

    pub async fn migrate(
        &self,
        migrations: &Migrations,
        hot_tables: &crate::migration::HotTables,
    ) -> Result<(), ProviderError> {
        PgMigrator::apply_validated(&self.migration_pool, migrations, hot_tables).await?;
        Ok(())
    }

    pub async fn migrate_foundation(&self) -> Result<(), ProviderError> {
        self.migrate(
            &foundation_migrations(),
            &crate::migration::HotTables::none(),
        )
        .await
    }

    pub async fn verify_index_ready(&self, index_name: &str) -> Result<(), ProviderError> {
        // @tenant-cross-scope: verifies a migration-owned PostgreSQL index catalog entry.
        let ready: Option<bool> = sqlx::query_scalar(
            "SELECT i.indisvalid AND i.indisready
               FROM pg_index i
               JOIN pg_class c ON c.oid = i.indexrelid
               JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE n.nspname = current_schema() AND c.relname = $1",
        )
        .bind(index_name)
        .fetch_optional(&self.migration_pool)
        .await
        .map_err(|_| BootstrapError::ValidationProbe)?;
        if ready == Some(true) {
            Ok(())
        } else {
            Err(BootstrapError::ValidationProbe.into())
        }
    }

    pub async fn verify_index_ready_exact(
        &self,
        expected: IndexReadinessSpec<'_>,
    ) -> Result<(), ProviderError> {
        // @tenant-cross-scope: verifies a migration-owned PostgreSQL index catalogue entry.
        let actual: Option<IndexReadinessRow> = sqlx::query_as(
            "SELECT i.indisvalid,
                        i.indisready,
                        i.indislive,
                        i.indcheckxmin,
                        table_class.relname,
                        table_class.relkind::text,
                        index_class.relkind::text,
                        access_method.amname,
                        ARRAY(
                            SELECT pg_get_indexdef(i.indexrelid, key_number, false)
                                   || CASE
                                          WHEN (i.indoption[key_number - 1] & 1) = 1
                                          THEN ' DESC'
                                          ELSE ''
                                      END
                                   || CASE
                                          WHEN (i.indoption[key_number - 1] & 1) = 1
                                               AND (i.indoption[key_number - 1] & 2) = 0
                                          THEN ' NULLS LAST'
                                          WHEN (i.indoption[key_number - 1] & 1) = 0
                                               AND (i.indoption[key_number - 1] & 2) = 2
                                          THEN ' NULLS FIRST'
                                          ELSE ''
                                      END
                              FROM generate_series(1, i.indnkeyatts::integer) AS key_number
                             ORDER BY key_number
                        ),
                        pg_get_expr(i.indpred, i.indrelid, false)
                   FROM pg_index i
                   JOIN pg_class index_class ON index_class.oid = i.indexrelid
                   JOIN pg_namespace index_namespace
                     ON index_namespace.oid = index_class.relnamespace
                   JOIN pg_class table_class ON table_class.oid = i.indrelid
                   JOIN pg_namespace table_namespace
                     ON table_namespace.oid = table_class.relnamespace
                   JOIN pg_am access_method ON access_method.oid = index_class.relam
                  WHERE index_namespace.nspname = current_schema()
                    AND table_namespace.nspname = current_schema()
                    AND index_class.relname = $1",
        )
        .bind(expected.index_name)
        .fetch_optional(&self.migration_pool)
        .await
        .map_err(|_| BootstrapError::ValidationProbe)?;

        let Some((
            valid,
            ready,
            live,
            check_xmin,
            table_name,
            table_relkind,
            index_relkind,
            access_method,
            key_columns,
            predicate,
        )) = actual
        else {
            return Err(BootstrapError::ValidationProbe.into());
        };
        let expected_keys: Vec<String> = expected
            .key_columns
            .iter()
            .map(|column| (*column).to_string())
            .collect();
        if valid
            && ready
            && live
            && !check_xmin
            && table_name == expected.table_name
            && table_relkind == expected.table_relkind
            && index_relkind == expected.index_relkind
            && access_method == expected.access_method
            && key_columns == expected_keys
            && predicate.as_deref() == expected.predicate
        {
            Ok(())
        } else {
            Err(BootstrapError::ValidationProbe.into())
        }
    }

    pub async fn into_runtime(self) -> Result<SubstrateProvider, ProviderError> {
        let runtime_pool = match connect_pool_with_reset(
            &self.config.database_url,
            &self.config.region,
            self.max_connections,
        )
        .await
        {
            Ok(pool) => pool,
            Err(_) => {
                self.migration_pool.close().await;
                return Err(BootstrapError::RuntimeConnect.into());
            }
        };
        if let Err(error) = validate_bootstrap_pair(&self.migration_pool, &runtime_pool).await {
            runtime_pool.close().await;
            self.migration_pool.close().await;
            return Err(error.into());
        }

        self.migration_pool.close().await;
        debug_assert!(self.migration_pool.is_closed());
        let mut runtime_config = self.config;
        runtime_config.database_migration_url = String::new();
        Ok(SubstrateProvider {
            pool: runtime_pool,
            config: runtime_config,
            runtime_role_validated: true,
        })
    }
}

#[derive(Clone)]
pub struct SubstrateProvider {
    pool: PgPool,
    config: MyelinConfig,
    runtime_role_validated: bool,
}

impl SubstrateProvider {
    pub async fn from_env(mode: Mode) -> Result<SubstrateProvider, ProviderError> {
        let config = MyelinConfig::from_env(mode)?;
        let pool = connect_pool_with_reset(
            &config.database_url,
            &config.region,
            configured_database_max_connections()?,
        )
        .await?;
        Ok(SubstrateProvider {
            pool,
            config,
            runtime_role_validated: false,
        })
    }

    pub async fn connect(
        config: MyelinConfig,
        max_connections: u32,
    ) -> Result<SubstrateProvider, ProviderError> {
        let pool =
            connect_pool_with_reset(&config.database_url, &config.region, max_connections).await?;
        Ok(SubstrateProvider {
            pool,
            config,
            runtime_role_validated: false,
        })
    }

    pub async fn migrate(
        &self,
        migrations: &Migrations,
        hot_tables: &crate::migration::HotTables,
    ) -> Result<(), ProviderError> {
        PgMigrator::apply_validated(&self.pool, migrations, hot_tables).await?;
        Ok(())
    }

    pub async fn migrate_foundation(&self) -> Result<(), ProviderError> {
        self.migrate(
            &foundation_migrations(),
            &crate::migration::HotTables::none(),
        )
        .await
    }

    pub fn db_pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn auxiliary_runtime_lane(
        &self,
        max_connections: u32,
    ) -> Result<SubstrateProvider, ProviderError> {
        self.require_validated_runtime()?;
        let pool = connect_pool_with_reset(
            &self.config.database_url,
            &self.config.region,
            max_connections,
        )
        .await?;
        Ok(SubstrateProvider {
            pool,
            config: self.config.clone(),
            runtime_role_validated: true,
        })
    }

    pub async fn database_is_ready(&self) -> bool {
        // @tenant-cross-scope: a constant readiness query reads no tenant data.
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .is_ok_and(|value| value == 1)
    }

    pub fn config(&self) -> &MyelinConfig {
        &self.config
    }

    pub fn require_validated_runtime(&self) -> Result<(), ProviderError> {
        if self.runtime_role_validated {
            Ok(())
        } else {
            Err(BootstrapError::RuntimeCapabilityUnvalidated.into())
        }
    }

    pub async fn with_tenant_tx<R, F>(&self, tenant: &str, op: F) -> Result<R, ProviderError>
    where
        F: for<'c> FnOnce(&'c mut sqlx::PgConnection) -> TxScope<'c, R> + Send,
        R: Send,
    {
        let r = with_tenant_tx(&self.pool, tenant, &self.config.region, op).await?;
        Ok(r)
    }

    pub fn cache(&self, rt: tokio::runtime::Handle) -> Result<Box<dyn Cache>, CacheError> {
        backend::cache(Backend::Real, &self.config, rt)
    }

    pub fn blob_store(
        &self,
        rt: tokio::runtime::Handle,
    ) -> Result<Box<dyn BlobStore + Send + Sync>, backend::BackendError> {
        backend::blob_store(Backend::Real, &self.config, rt)
    }
}

#[cfg(test)]
mod boot_migrations_tests {
    use super::*;

    fn ids(m: &Migrations) -> Vec<String> {
        m.0.iter().map(|mg| mg.id.to_string()).collect()
    }

    #[test]
    fn aggregate_ids_are_strictly_ascending_and_duplicate_free() {
        let ids = ids(&all_durable_migrations());
        assert!(!ids.is_empty(), "the aggregate is non-empty");
        for w in ids.windows(2) {
            assert!(
                w[0] < w[1],
                "migration ids must be strictly ascending + duplicate-free, but {:?} !< {:?}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn aggregate_is_exactly_the_concatenation_of_every_group() {
        let groups = durable_migration_groups();
        let agg = ids(&all_durable_migrations());
        let mut rebuilt = Vec::new();
        for g in &groups {
            let g_ids = ids(g);
            assert!(!g_ids.is_empty(), "no group is empty");
            for id in &g_ids {
                assert!(
                    agg.contains(id),
                    "group id {id:?} must appear in the aggregate"
                );
            }
            rebuilt.extend(g_ids);
        }
        assert_eq!(
            agg, rebuilt,
            "the aggregate must be exactly the ordered concatenation of the groups - no drift"
        );
    }

    #[test]
    fn aggregate_is_disjoint_from_the_foundation() {
        let agg = ids(&all_durable_migrations());
        for f in ids(&foundation_migrations()) {
            assert!(
                !agg.contains(&f),
                "foundation id {f:?} must NOT be in the durable aggregate"
            );
        }
    }

    #[test]
    fn database_pool_budget_accepts_only_positive_connection_counts() {
        assert_eq!(parse_database_max_connections("8"), Ok(8));
        assert_eq!(parse_database_max_connections(" 12 "), Ok(12));
        for invalid in ["", "0", "many", "-1"] {
            assert!(
                matches!(
                    parse_database_max_connections(invalid),
                    Err(ConfigError::Invalid {
                        var: DATABASE_MAX_CONNECTIONS_ENV,
                        ..
                    })
                ),
                "{invalid:?} must not become a database pool budget"
            );
        }
    }

    #[tokio::test]
    async fn bootstrap_rejects_identical_credentials_before_connecting() {
        let mut config = MyelinConfig::dev();
        config.database_url = "postgres://runtime:DO_NOT_PRINT_THIS@127.0.0.1:1/myelin".to_string();
        config.database_migration_url = config.database_url.clone();

        let error = match PgBootstrap::connect(config, 1).await {
            Ok(_) => panic!("identical credentials must be rejected before connecting"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            ProviderError::Bootstrap(BootstrapError::CredentialsNotDistinct)
        ));
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("DO_NOT_PRINT_THIS"));
        assert!(!rendered.contains("postgres://"));
    }

    #[test]
    fn every_bootstrap_error_is_credential_free() {
        let errors = [
            BootstrapError::CredentialsNotDistinct,
            BootstrapError::MigrationConnect,
            BootstrapError::RuntimeConnect,
            BootstrapError::ValidationProbe,
            BootstrapError::DatabaseIdentityMismatch,
            BootstrapError::RolesNotDistinct,
            BootstrapError::RuntimeSuperuser,
            BootstrapError::RuntimeBypassRls,
            BootstrapError::RuntimeCreateDatabase,
            BootstrapError::RuntimeCreateRole,
            BootstrapError::RuntimeOwnsSchema,
            BootstrapError::RuntimeCanCreateSchemaObjects,
            BootstrapError::RuntimeCannotUseSchema,
            BootstrapError::RuntimeMemberOfMigrationRole,
            BootstrapError::RuntimeElevatedMembership,
            BootstrapError::MigrationCannotManageSchema,
            BootstrapError::MigrationCannotUseAdvisoryLock,
        ];
        for error in errors {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("postgres://"));
            assert!(!rendered.contains('@'));
        }
    }
}
