use myelin_events::OutboxStore;
use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, DeclaredStore, HotTables, InternalRpc,
    Migration, Migrations, OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreKind,
    StoreManifest,
};

use crate::holder::SEARCH_INDEX_STORE;

pub const SERVICE_NAME: &str = "search";

pub const SEARCH_INDEX_DIR_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS search_index_directory (
    tenant         TEXT NOT NULL,
    region         TEXT NOT NULL,
    index_dek_ref  TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, region)
);";

pub fn search_service_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0010_search_index_directory",
        SEARCH_INDEX_DIR_MIGRATION,
    )])
}

fn search_stores() -> StoreManifest {
    StoreManifest::of([DeclaredStore::new(
        StoreKind::SearchIndex,
        SEARCH_INDEX_STORE,
    )])
}

fn search_critical() -> CriticalDependencies {
    CriticalDependencies::new(["identity"])
}

pub fn search_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: search_service_migrations(),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: search_stores(),
        outbox: OutboxSpec::external_relay(outbox),
        critical: search_critical(),
    }
}

pub fn boot_search(config: Config, outbox: OutboxStore) -> Result<ServeHandle, ServeError> {
    boot(search_app_spec(config, outbox))
}

pub fn run_search(config: Config, outbox: OutboxStore) -> Result<(), ServeError> {
    serve(search_app_spec(config, outbox))
}

pub async fn run_search_until_shutdown<F>(
    config: Config,
    outbox: OutboxStore,
    shutdown: F,
) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    myelin_substrate::serve_until_shutdown(search_app_spec(config, outbox), shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{HolderRegistration, Liveness, Surface};

    #[test]
    fn search_boots_from_serve_appspec_with_three_ports() {
        let handle = boot_search(Config::default(), OutboxStore::new())
            .expect("the Search shell boots from serve(AppSpec)");
        assert_eq!(handle.name(), SERVICE_NAME, "the deployable service name");

        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports opened (contract 1.2)"
        );

        let mh = handle.metrics_health();
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness = not-wedged (never checks a dependency)"
        );
        assert!(
            mh.readiness().is_ready(),
            "readiness = can-serve-now (all critical deps healthy at boot) - distinct from liveness"
        );

        assert!(
            handle
                .holder_registry()
                .is_registered(StoreKind::SearchIndex, SEARCH_INDEX_STORE),
            "the per-tenant search index auto-registered as a holder (§3.4, GD-3)"
        );
        assert!(
            handle.registered_holders().contains(&HolderRegistration {
                kind: StoreKind::SearchIndex,
                name: SEARCH_INDEX_STORE,
            }),
            "the search-index holder registration receipt is present"
        );
        assert!(
            handle.holder_registered().is_ok(),
            "every declared store registered"
        );
    }

    #[test]
    fn dead_identity_flips_readiness_not_liveness() {
        let handle = boot_search(Config::default(), OutboxStore::new()).expect("boot");
        let mh = handle.metrics_health();
        assert!(mh.readiness().is_ready(), "ready while identity is healthy");

        handle.health_probe().mark_down("identity");

        assert!(
            !mh.readiness().is_ready(),
            "a dead critical dep → not-ready + shed"
        );
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness stays UP (not-ready is NOT not-alive - no restart storm)"
        );
    }

    #[test]
    fn run_search_runs_lifecycle_and_returns_ok() {
        assert_eq!(
            run_search(Config::default(), OutboxStore::new()),
            Ok(()),
            "the Search shell boots → … → drains cleanly"
        );
    }

    #[tokio::test]
    async fn production_search_waits_for_shutdown_then_drains() {
        assert_eq!(
            run_search_until_shutdown(Config::default(), OutboxStore::new(), async {}).await,
            Ok(())
        );
    }

    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_search(Config("BAD_POOL".into()), OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    #[test]
    fn the_index_directory_migration_is_forward_only() {
        assert!(
            !myelin_substrate::is_destructive(SEARCH_INDEX_DIR_MIGRATION),
            "the per-tenant index directory migration is forward-only (a CREATE, never a DROP)"
        );
        assert!(
            SEARCH_INDEX_DIR_MIGRATION.contains("search_index_directory"),
            "the migration creates the per-tenant index directory catalog"
        );
        let spec = search_app_spec(Config::default(), OutboxStore::new());
        assert!(
            spec.migrations
                .0
                .iter()
                .any(|m| m.id == "0010_search_index_directory"),
            "the index-directory migration is in the Search AppSpec's forward-only set"
        );
    }

    #[test]
    fn the_shell_declares_the_index_store_and_no_engine() {
        let spec = search_app_spec(Config::default(), OutboxStore::new());
        assert!(
            spec.stores
                .stores()
                .iter()
                .any(|s| s.kind == StoreKind::SearchIndex),
            "the per-tenant search index store is declared (auto-registered as H7)"
        );
        assert!(
            spec.consumers.is_empty(),
            "no indexer consumer at the shell (SRCH-P06 floor)"
        );
        assert_eq!(
            crate::layout::srch_p03_floors().len(),
            5,
            "the engine-shapes floor is named"
        );
    }
}
