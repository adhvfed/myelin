//! Cross-service migration catalog for deployment compatibility audits.
//!
//! Every service writes to one `myelin_applied_migration` ledger, but no production service should
//! depend on sibling services merely to inspect their migration declarations. This test-support
//! leaf assembles the exact sets applied by current `PgBootstrap` composition roots so catalog
//! collisions and dogfood checksum drift can be audited centrally without widening Storage's
//! runtime or ordinary test dependency graph.

use myelin_storage::migration::Migrations;

/// The exact migration sets applied by the current production `PgBootstrap` composition roots.
/// Shared sets appear once; the CI writer subset appears alongside the complete CI set so their
/// intentional exact id/DDL reuse is also checked.
pub fn production_migration_sets() -> Vec<(&'static str, Migrations)> {
    vec![
        (
            "substrate.foundation",
            myelin_storage::foundation_migrations(),
        ),
        (
            "storage.all_durable",
            myelin_storage::all_durable_migrations(),
        ),
        (
            "identity.service",
            myelin_identity_service::identity_service_migrations(),
        ),
        ("issues.service", myelin_issues::issues_migrations()),
        ("flow.service", myelin_flow::migrations::migrations()),
        ("notif.service", myelin_notif::migrations::migrations()),
        ("search.service", myelin_search::search_service_migrations()),
        (
            "knowledge.service",
            myelin_knowledge::knowledge_service_migrations(),
        ),
        (
            "ci.writer_subset",
            myelin_ci_controlplane::ci_durable_migrations(),
        ),
        (
            "ci.controlplane",
            myelin_ci_controlplane::ci_controlplane_migrations(),
        ),
        ("ci.dispatch", myelin_ci_dispatch::dispatch_migrations()),
    ]
}

/// Borrow an owned catalog in the shape accepted by Storage's collision detector.
pub fn borrowed_sets<'a>(sets: &'a [(&'static str, Migrations)]) -> Vec<(&'a str, &'a Migrations)> {
    sets.iter()
        .map(|(name, migrations)| (*name, migrations))
        .collect()
}
