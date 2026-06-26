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
use crate::cache::{Cache, CacheError};
use crate::blob::BlobStore;
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
}

impl core::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProviderError::Config(e) => write!(f, "substrate provider config error: {e}"),
            ProviderError::Pg(e) => write!(f, "substrate provider backend error: {e}"),
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
    ])
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
        let pool =
            connect_pool_with_reset(&config.database_url, &config.region, DEFAULT_MAX_CONNECTIONS)
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
        self.migrate(&foundation_migrations(), &crate::migration::HotTables::none())
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
    pub async fn with_tenant_tx<R, F>(
        &self,
        tenant: &str,
        op: F,
    ) -> Result<R, ProviderError>
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
    pub fn blob_store(
        &self,
        rt: tokio::runtime::Handle,
    ) -> Box<dyn BlobStore + Send + Sync> {
        backend::blob_store(Backend::Real, &self.config, rt)
    }
}
