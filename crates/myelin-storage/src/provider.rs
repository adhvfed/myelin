//! # `SubstrateProvider` — the production composition root / real-pool provider (MR-022, SI-022)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §3.2/§3.3 (config
//! env-first, the bounded OLTP pool, migrate-at-boot) — the P-S12/P-S15 root. Closes census SI-022
//! ("all real backings behind `--features integration`; default build+CI 100% in-memory") at the
//! *production* level: this is the seam every durable store is constructed through where the REAL
//! `PgPool` (+ Valkey, + S3) is built and migrations actually run.
//!
//! ## The gap this fills (the census's P-S12/P-S15 finding)
//! There was no composition root wiring a REAL pool into the substrate: the substrate's
//! [`myelin_substrate::serve`] opens the in-memory [`crate::oltp::OltpPool`] permit MODEL and runs
//! the boot-time [`myelin_substrate::migrations::MigrationRunner`] that executes NO DDL (SI-010). So
//! the "production default path" was in-memory end to end. `SubstrateProvider` is the production
//! composition seam: it reads config from the environment (the dev↔prod CONFIG SWAP,
//! [`myelin_config::MyelinConfig`]), constructs the REAL bounded [`PgPool`] (with reset-on-release
//! wired, [`crate::tenant_tx::connect_pool_with_reset`]), runs migrations against it at startup
//! (deliverable A, [`PgMigrator::apply_validated`] — validate → execute), and hands the pool (and
//! the cache / blob backings) to the stores. **On this path the in-memory impls are explicit
//! test-doubles, not the production default.**
//!
//! ## Scope (the FOUNDATION + the seam, not the store bindings)
//! MR-022 builds the foundation + the seam every durable store plugs into. BINDING the individual
//! stores (identity principal/tuple/revocation, events outbox/dedup, control-plane registry, KMS
//! root) to this provider's pool is **MR-007/008/023/024/025** — those land their store
//! constructors over [`SubstrateProvider::db_pool`] + the [`crate::tenant_tx::with_tenant_tx`]
//! convention. This module deliberately does NOT construct them; it constructs the pool + runs the
//! migrations they will bind to, and exposes the tenant-scoped-transaction convention they acquire
//! through.
//!
//! ## Feature-gated (keeps the default build DB-free)
//! Compiled only under `--features integration` (it pulls the real sqlx client + `myelin-config`).
//! The default `cargo build/test --workspace` compiles none of it, so the unit build stays DB-free
//! (the real-pool path is exercised by the `--features integration` tests the make-it-real gate runs).

use myelin_config::{ConfigError, Mode, MyelinConfig};
use sqlx::postgres::PgPool;

use crate::backend::{self, Backend};
use crate::blob::BlobStore;
use crate::cache::{Cache, CacheError};
use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::pg_migrator::PgMigrator;
use crate::tenant_tx::{connect_pool_with_reset, with_tenant_tx, TxScope};

/// The default bounded pool size the provider opens when a caller does not specify one (the §3.3
/// bounded-pool floor — never unbounded). A service tunes this from its own config.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 32;

/// An error constructing or driving the composition root. Loud + typed: a missing prod env var, a
/// failed connection, or a failed migration is a fail-fast at boot (§3.2), never a silent fallback.
#[derive(Debug)]
pub enum ProviderError {
    /// A required env var was absent / invalid (prod fail-fast, [`Mode::RequireEnv`]).
    Config(ConfigError),
    /// The real pool could not be opened or a migration failed against the live DB.
    Pg(PgError),
    /// Split-credential bootstrap validation failed before DDL or runtime handoff.
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

/// Credential-redacted failures from the split-role PostgreSQL bootstrap.
///
/// Variants deliberately carry no DSN or driver error text. A malformed URL, failed login, or
/// authorization refusal must be loud without copying credential-bearing input into logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapError {
    /// Runtime and migration DSNs are byte-for-byte identical.
    CredentialsNotDistinct,
    /// The migration-only connection could not be opened.
    MigrationConnect,
    /// The constrained runtime connection could not be opened.
    RuntimeConnect,
    /// PostgreSQL role/server metadata could not be inspected.
    ValidationProbe,
    /// Runtime and migration credentials did not reach the same database/server/schema identity.
    DatabaseIdentityMismatch,
    /// Both credentials authenticate as the same PostgreSQL role.
    RolesNotDistinct,
    /// The runtime role is a superuser.
    RuntimeSuperuser,
    /// The runtime role can bypass row-level security.
    RuntimeBypassRls,
    /// The runtime role can create databases.
    RuntimeCreateDatabase,
    /// The runtime role can create roles.
    RuntimeCreateRole,
    /// The runtime role owns, or can assume ownership of, the application schema.
    RuntimeOwnsSchema,
    /// The runtime role can create objects in the application schema.
    RuntimeCanCreateSchemaObjects,
    /// The runtime role cannot use the application schema.
    RuntimeCannotUseSchema,
    /// The runtime role is a member of the migration role.
    RuntimeMemberOfMigrationRole,
    /// The runtime role can assume another obviously elevated role.
    RuntimeElevatedMembership,
    /// The migration role cannot own/manage the application schema.
    MigrationCannotManageSchema,
    /// The migration role cannot execute the PostgreSQL advisory-lock primitive.
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

/// The substrate's co-located FOUNDATION migrations every service's DB carries (architecture §3.3):
/// the transactional `outbox` and the `consumer_dedup` tables (the SAME frozen DDL the substrate
/// boot prefixes via [`myelin_substrate::serve`]). The provider runs these at startup so the durable
/// stores have the substrate tables to bind to. A service appends its own migrations after these
/// (the same order the boot-time runner uses).
///
/// REUSES the frozen `myelin_events::{OUTBOX_MIGRATION, CONSUMER_DEDUP_MIGRATION}` (never re-defines
/// the outbox/dedup table shape — EI-01 §7).
pub fn foundation_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0000_outbox", myelin_events::OUTBOX_MIGRATION),
        Migration::plain(
            "0001_consumer_dedup",
            myelin_events::CONSUMER_DEDUP_MIGRATION,
        ),
        // CT-004d.2 chunk 6 / #7b: the durable consumer DEAD-LETTER set — part of the foundation set
        // (like `consumer_dedup`, every service's embedded set needs it) so a dead-lettered event
        // (esp. the H2 panic path) survives a restart.
        Migration::plain(
            "0002_consumer_dead_letter",
            myelin_events::CONSUMER_DEAD_LETTER_MIGRATION,
        ),
        Migration::plain(
            "0003_outbox_quarantine",
            myelin_events::OUTBOX_QUARANTINE_MIGRATION,
        ),
    ])
}

/// The ORDERED list of every durable migration GROUP the platform owns, each returned by its own
/// `*_durable_migrations()` constructor, in strictly-ascending id-range order (W7.2 / doc-18 Part 5).
/// This is the SINGLE authority the aggregate [`all_durable_migrations`] is built FROM: the aggregate
/// is nothing but the flattened concatenation of these groups, so a group **cannot** be in the boot
/// sequence unless it is listed here, and the aggregate can never contain a migration that is not in
/// one of these groups. Adding a new durable subsystem = add its constructor to THIS list (and the
/// `boot_migration_ids_*` unit tests, which iterate the SAME list, keep the aggregate honest).
///
/// | group                              | id range      |
/// |------------------------------------|---------------|
/// | `identity_durable_migrations`      | `0010`–`0019` |
/// | `pseudonym_durable_migrations`     | `0020`–`0022` |
/// | `placement_durable_migrations`     | `0030`–`0039` |
/// | `kms_durable_migrations`           | `0040`–`0042` |
/// | `reserve_settle_durable_migrations`| `0050`        |
/// | `restore_verify_durable_migrations`| `0051`        |
/// | `post_pit_durable_migrations`      | `0052`        |
/// | `bus_erasure_durable_migrations`   | `0053`        |
/// | `hitl_gate_durable_migrations`     | `0054`        |
/// | `cell_root_durable_migrations`     | `0060`        |
/// | `delegation_policy_durable_migrations` | `0061`–`0066` |
/// | `authz_projection_durable_migrations` | `0067`–`0069` |
///
/// The substrate FOUNDATION (`0000`–`0001`, outbox + consumer_dedup) is deliberately NOT in this list:
/// it stays the separate [`foundation_migrations`] / [`SubstrateProvider::migrate_foundation`] call
/// (the substrate boot owns it). The full boot sequence at a service main is therefore
/// `migrate_foundation()` THEN `migrate(&all_durable_migrations(), …)` — foundation + everything else,
/// each exactly once.
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
    ]
}

/// **The provider-level DURABLE MIGRATION AGGREGATE (W7.2 / doc-18 Part 5 — the boot-migrations fix).**
/// Composes EVERY durable migration group ([`durable_migration_groups`]) into one ordered
/// [`Migrations`] in strictly-ascending id order (`0010`–`0053`), so a single boot call migrates the
/// complete durable schema every service's stores bind to — closing the doc-18 LIVE DEFECT where a
/// service main constructed durable stores (e.g. `PrincipalStore::with_pg`, needing identity
/// `0010`–`0019`) but never migrated their tables, so the first write failed at runtime on a fresh DB.
///
/// Built FROM the group constructors (not a re-listed copy of the ids) so it **cannot drift**: it is
/// exactly the flattened concatenation of [`durable_migration_groups`]. Excludes the substrate
/// FOUNDATION (`0000`–`0001`) — that stays the separate [`SubstrateProvider::migrate_foundation`]
/// call. Apply order at a main: `migrate_foundation()` then `migrate(&all_durable_migrations(), …)`.
///
/// The [`PgMigrator`] is idempotent + advisory-locked + version-recorded, so this is safe to apply at
/// every boot and safe alongside any pre-existing per-group `migrate` call (the aggregate REPLACES the
/// piecemeal calls; a residual one would just no-op).
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

    let member_of_migration: bool =
        sqlx::query_scalar("SELECT pg_has_role(current_user, $1, 'MEMBER')")
            .bind(&migration.user)
            .fetch_one(runtime_pool)
            .await
            .map_err(|_| BootstrapError::ValidationProbe)?;
    if member_of_migration {
        return Err(BootstrapError::RuntimeMemberOfMigrationRole);
    }

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

/// Split-credential PostgreSQL bootstrap. This type is the only owner of the privileged migration
/// pool; it is intentionally not cloneable and exposes no pool accessor.
///
/// Construction opens the migration pool and a short-lived runtime probe, then validates both roles
/// before any migration method can mutate the database. [`PgBootstrap::into_runtime`] opens a fresh
/// runtime pool, repeats the validation, closes the privileged pool, strips the migration DSN, and
/// returns only a serving [`SubstrateProvider`]. The dev migration role is currently a superuser for
/// compatibility with the existing stack; a production migration role should be a dedicated
/// non-superuser that owns the application schema and can execute the advisory-lock functions.
pub struct PgBootstrap {
    migration_pool: PgPool,
    config: MyelinConfig,
    max_connections: u32,
}

impl PgBootstrap {
    /// Build a validated bootstrap from environment-provided split credentials.
    pub async fn from_env(mode: Mode) -> Result<PgBootstrap, ProviderError> {
        Self::connect(MyelinConfig::from_env(mode)?, DEFAULT_MAX_CONNECTIONS).await
    }

    /// Build a validated bootstrap from explicit config. No DDL runs until this returns, and this
    /// returns only after a constrained runtime connection to the same database has been proven.
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

    /// Validate and apply a forward-only migration set through the privileged bootstrap pool.
    pub async fn migrate(
        &self,
        migrations: &Migrations,
        hot_tables: &crate::migration::HotTables,
    ) -> Result<(), ProviderError> {
        PgMigrator::apply_validated(&self.migration_pool, migrations, hot_tables).await?;
        Ok(())
    }

    /// Apply the co-located substrate foundation through the privileged bootstrap pool.
    pub async fn migrate_foundation(&self) -> Result<(), ProviderError> {
        self.migrate(
            &foundation_migrations(),
            &crate::migration::HotTables::none(),
        )
        .await
    }

    /// Consume bootstrap state and return the constrained serving provider. The privileged pool is
    /// closed before this function returns, and its credential is erased from the retained config.
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
        })
    }
}

/// **The production composition root.** Holds the REAL bounded [`PgPool`] (with reset-on-release
/// wired) + the env-driven [`MyelinConfig`] every durable store is constructed through.
#[derive(Clone)]
pub struct SubstrateProvider {
    pool: PgPool,
    config: MyelinConfig,
}

impl SubstrateProvider {
    /// **Boot the composition root from the environment (the prod default path).** Reads
    /// [`MyelinConfig`] (env-first, fail-fast in [`Mode::RequireEnv`]; [`Mode::DevDefaults`] points
    /// at the docker-compose dev stack), opens the REAL bounded pool with reset-on-release wired
    /// ([`connect_pool_with_reset`]), and returns the provider. The caller then runs
    /// [`Self::migrate_foundation`] (+ its own migrations) at startup.
    pub async fn from_env(mode: Mode) -> Result<SubstrateProvider, ProviderError> {
        let config = MyelinConfig::from_env(mode)?;
        let pool = connect_pool_with_reset(
            &config.database_url,
            &config.region,
            DEFAULT_MAX_CONNECTIONS,
        )
        .await?;
        Ok(SubstrateProvider { pool, config })
    }

    /// Build the provider over an EXPLICIT config + pool size (the test seam — e.g. the admin role
    /// for DDL, or a bounded `max_connections` to exercise connection reuse). Opens the REAL pool
    /// with reset-on-release wired.
    pub async fn connect(
        config: MyelinConfig,
        max_connections: u32,
    ) -> Result<SubstrateProvider, ProviderError> {
        let pool =
            connect_pool_with_reset(&config.database_url, &config.region, max_connections).await?;
        Ok(SubstrateProvider { pool, config })
    }

    /// **Run migrations at startup (deliverable A — the SI-010 fix wired into the boot path).**
    /// VALIDATE (forward-only / hot-table, via [`PgMigrator::apply_validated`]) → EXECUTE the DDL
    /// against the live pool under the advisory lock + `myelin_applied_migration` version table.
    /// Forward-only, idempotent, serialized: after this returns the tables exist and a re-run applies
    /// nothing (no error, no duplicate apply).
    pub async fn migrate(
        &self,
        migrations: &Migrations,
        hot_tables: &crate::migration::HotTables,
    ) -> Result<(), ProviderError> {
        PgMigrator::apply_validated(&self.pool, migrations, hot_tables).await?;
        Ok(())
    }

    /// Run the substrate's co-located [`foundation_migrations`] (outbox + consumer_dedup) at startup.
    /// The minimal "the substrate tables exist after boot" call.
    pub async fn migrate_foundation(&self) -> Result<(), ProviderError> {
        self.migrate(
            &foundation_migrations(),
            &crate::migration::HotTables::none(),
        )
        .await
    }

    /// The REAL bounded OLTP pool the durable stores are constructed over (MR-007/008/023/024 build
    /// their store constructors against this + the [`Self::with_tenant_tx`] convention). Named
    /// `db_pool` (not `pool`) deliberately: it is the production pool every acquisition is meant to go
    /// through the tenant-scoped-transaction convention — NOT a bare unscoped hatch.
    pub fn db_pool(&self) -> &PgPool {
        &self.pool
    }

    /// The env-driven config (so a store can read the `region` pin / S3 / Redis endpoints).
    pub fn config(&self) -> &MyelinConfig {
        &self.config
    }

    /// **The tenant-scoped-transaction convention (deliverable C), bound to this provider's pool.**
    /// Every durable tenant-scoped store runs its op through here: acquire → BEGIN → set the
    /// `(tenant, region)` GUC transaction-scoped → run `op` → COMMIT, with reset-on-release. `tenant`
    /// is the VERIFIED tenant; `region` defaults to the provider's configured region pin.
    pub async fn with_tenant_tx<R, F>(&self, tenant: &str, op: F) -> Result<R, ProviderError>
    where
        F: for<'c> FnOnce(&'c mut sqlx::PgConnection) -> TxScope<'c, R> + Send,
        R: Send,
    {
        let r = with_tenant_tx(&self.pool, tenant, &self.config.region, op).await?;
        Ok(r)
    }

    /// Build the config-selected [`Cache`] backing (the real Valkey at the config endpoint). The
    /// production composition uses [`Backend::Real`]; the in-memory cache is the explicit test-double.
    pub fn cache(&self, rt: tokio::runtime::Handle) -> Result<Box<dyn Cache>, CacheError> {
        backend::cache(Backend::Real, &self.config, rt)
    }

    /// Build the config-selected [`BlobStore`] backing (the real S3/RustFS at the config endpoint).
    /// The production composition uses [`Backend::Real`]; the fs floor is the explicit test-double.
    pub fn blob_store(&self, rt: tokio::runtime::Handle) -> Box<dyn BlobStore + Send + Sync> {
        backend::blob_store(Backend::Real, &self.config, rt)
    }
}

// =================================================================================================
// W7.2 — the boot-migrations aggregate is well-formed (DB-FREE unit tests, doc-18 Part 5). These run
// on the DEFAULT `cargo test -p myelin-storage` (no `integration` feature, no DB): they inspect only
// the migration ids the constructors return.
// =================================================================================================
#[cfg(test)]
mod boot_migrations_tests {
    use super::*;

    fn ids(m: &Migrations) -> Vec<&'static str> {
        m.0.iter().map(|mg| mg.id).collect()
    }

    /// The aggregate's ids are STRICTLY ASCENDING (so the boot sequence applies FK/trigger deps in
    /// the numerically-ordered order the groups were authored in) and DUPLICATE-FREE (no id is
    /// applied twice — no two groups collide on an id).
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
        // Non-vacuity: the full set is present (identity 0010 … authz projection 0069).
        assert_eq!(*ids.first().unwrap(), "0010_rebac_tuple");
        assert_eq!(*ids.last().unwrap(), "0069_authz_projection_invalidator");
    }

    /// STRUCTURAL anti-drift: the aggregate is EXACTLY the flattened concatenation of every group in
    /// [`durable_migration_groups`], so it is a superset of each group AND contains nothing else. A
    /// newly-authored group that is added to `durable_migration_groups` is folded in automatically;
    /// one that is NOT listed there is neither migrated nor counted here — the two cannot diverge.
    #[test]
    fn aggregate_is_exactly_the_concatenation_of_every_group() {
        let groups = durable_migration_groups();
        // (a) Each group is a contiguous, in-order SUBSET of the aggregate (nothing dropped/reordered).
        let agg = ids(&all_durable_migrations());
        let mut rebuilt: Vec<&'static str> = Vec::new();
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
        // (b) The aggregate contains NOTHING beyond the groups (exact equality of the flattened list).
        assert_eq!(
            agg, rebuilt,
            "the aggregate must be exactly the ordered concatenation of the groups — no drift"
        );
    }

    /// The aggregate is DISJOINT from the substrate FOUNDATION (`0000`/`0001`): the boot sequence
    /// `migrate_foundation()` + `migrate(&all_durable_migrations())` covers each id exactly once, so
    /// keeping foundation a separate call never double-applies.
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
