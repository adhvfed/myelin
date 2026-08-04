use myelin_storage::migration::Migrations;

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

pub fn borrowed_sets<'a>(sets: &'a [(&'static str, Migrations)]) -> Vec<(&'a str, &'a Migrations)> {
    sets.iter()
        .map(|(name, migrations)| (*name, migrations))
        .collect()
}
