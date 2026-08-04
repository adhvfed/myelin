pub mod coverage;
pub mod dependency_direction;
pub mod engine;
pub mod erosion;
pub mod lints;
pub mod production_graph;

pub use coverage::{
    parse_contract_index_rows, parse_manifest, scan, Coverage, CoverageError, ManifestEntry,
    PairEvidence, RowId, ScanReport,
};

pub use engine::{Lint, LintId, Violation};
pub use production_graph::{
    no_bare_tenant_pool, no_in_memory_durable_store, no_permissive_authorizer_in_prod,
    no_structural_crypto_in_prod, production_graph_absence_scanners, NO_BARE_TENANT_POOL,
    NO_IN_MEMORY_DURABLE_STORE, NO_PERMISSIVE_AUTHORIZER_IN_PROD, NO_STRUCTURAL_CRYPTO_IN_PROD,
    PRODUCTION_GRAPH_ABSENCE_SCANNERS,
};
pub use lints::{
    all_twelve, control_plane_pii_free, flow_determinism, forward_only_migration,
    load_bearing_four, no_cross_db, no_cross_sync_cycle, no_host_exec, no_llm_in_platform,
    no_raw_publish, no_untagged_personal_data, remaining_eight, residency_pin,
    search_requires_acl_filter, tenant_predicate, ALL_TWELVE, LOAD_BEARING_FOUR, REMAINING_EIGHT,
};
