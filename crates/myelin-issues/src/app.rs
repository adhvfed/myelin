use crate::migrations::{issues_hot_tables, issues_migrations};
use myelin_events::OutboxStore;
use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
    ServeError, ServeHandle, StoreManifest,
};

pub const SERVICE_NAME: &str = "issues";

fn issues_critical() -> CriticalDependencies {
    CriticalDependencies::new(["identity"])
}

pub fn issues_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: issues_migrations(),
        hot_tables: issues_hot_tables(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::external_relay(outbox),
        critical: issues_critical(),
    }
}

pub fn boot_issues(config: Config, outbox: OutboxStore) -> Result<ServeHandle, ServeError> {
    boot(issues_app_spec(config, outbox))
}

pub fn run_issues(config: Config, outbox: OutboxStore) -> Result<(), ServeError> {
    serve(issues_app_spec(config, outbox))
}

pub async fn run_issues_until_shutdown<F>(
    config: Config,
    outbox: OutboxStore,
    shutdown: F,
) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    myelin_substrate::serve_until_shutdown(issues_app_spec(config, outbox), shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::{ISSUE_CHANGE_LOG_TABLE, ISSUE_RELATION_TABLE, ISSUE_TABLE};
    use myelin_substrate::{Liveness, Surface};

    #[test]
    fn issues_boots_from_serve_appspec_with_three_ports() {
        let handle = boot_issues(Config::default(), OutboxStore::new())
            .expect("the Issue Tracker shell boots from serve(AppSpec)");
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
    }

    #[test]
    fn dead_identity_flips_readiness_not_liveness() {
        let handle = boot_issues(Config::default(), OutboxStore::new()).expect("boot");
        let mh = handle.metrics_health();
        assert!(mh.readiness().is_ready(), "ready while identity is healthy");

        handle.health_probe().mark_down("identity");

        assert!(
            !mh.readiness().is_ready(),
            "a dead authz/identity → not-ready + shed"
        );
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness stays UP (not-ready is NOT not-alive - no restart storm)"
        );
    }

    #[test]
    fn run_issues_runs_lifecycle_and_returns_ok() {
        assert_eq!(
            run_issues(Config::default(), OutboxStore::new()),
            Ok(()),
            "the Issue Tracker shell boots → … → drains cleanly"
        );
    }

    #[tokio::test]
    async fn production_issues_waits_for_shutdown_then_drains() {
        assert_eq!(
            run_issues_until_shutdown(Config::default(), OutboxStore::new(), async {}).await,
            Ok(())
        );
    }

    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_issues(Config("BAD_POOL".into()), OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    #[test]
    fn the_shell_carries_the_complete_spine_and_no_consumers() {
        let spec = issues_app_spec(Config::default(), OutboxStore::new());
        for table in [
            "issue",
            "issue_relation",
            "issue_change_log",
            "scheme",
            "scheme_assignment",
            "cycle",
            "cycle_membership",
            "milestone",
            "prefix_counter",
            "consumer_dedup",
            "outbox",
        ] {
            assert!(
                spec.migrations.0.iter().any(|migration| migration.table == Some(table)),
                "spine table `{table}` is present alongside its standalone online index/expand steps"
            );
        }
        assert!(
            spec.consumers.is_empty(),
            "no consumers at the shell (the rollup/SLA/trigger/feeder consumers are the per-band follow-ons)"
        );
        for t in [ISSUE_TABLE, ISSUE_RELATION_TABLE, ISSUE_CHANGE_LOG_TABLE] {
            assert!(spec.hot_tables.is_hot(t), "`{t}` is declared hot");
        }
        let deps: Vec<&str> = spec.critical.deps().iter().map(|d| d.0.as_str()).collect();
        assert!(
            deps.contains(&"identity"),
            "identity is critical (the authz dependency)"
        );
    }
}
