//! Lint-fixture tests for the myelin-flow data model (P-FLOW-01 / P-197) — the committed-ratchet
//! proof (EI-01 §5) that the FOUR schema gates this prompt names are LIVE, not vacuously green:
//! `forward-only-migration`, `no-untagged-personal-data`, `tenant-predicate`, `no-cross-db`. Each
//! runs the REAL lint ([`myelin_lints`]) over a deliberately-broken (RED) fixture and over a clean
//! (GREEN) fixture, so a regression that lets a destructive/blocking migration / an untagged-PII
//! column / a tenant-less query / a cross-DB reach slip through fails THIS build (defense in depth:
//! the lint at source-scan, the runner at boot).
//!
//! The fixtures live under `tests/fixtures/*.rs.txt` — the SAME `/fixtures/` convention the
//! workspace lint-gate (`myelin-lints/src/bin/lint-gate.rs`) EXCLUDES from the live scan, so the
//! deliberately red samples here do NOT trip the real CI gate over the workspace (they are scanner
//! DATA, not real crate code). The flow schema's actual inline-PII key-ref columns are tagged in
//! `src/schema.rs`; the actual migrations are tenant-first + forward-only in `src/migrations.rs` —
//! these fixtures prove the SCANNERS bite.

use std::path::{Path, PathBuf};

use myelin_lints::{
    forward_only_migration, no_cross_db, no_untagged_personal_data, tenant_predicate,
};

fn fixture(name: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

/// `forward-only-migration` (contract 1.5 / §9) REJECTS a destructive (`down`) + blocking-`ALTER`
/// migration and ADMITS the expand step (nullable add + `CREATE INDEX CONCURRENTLY`). The flow
/// journal is the source of truth; a destructive/blocking migration over it is the silent-data-loss
/// floor (EI-01 §2 — silent data loss outranks every feature).
#[test]
fn forward_only_migration_rejects_destructive_admits_expand() {
    let red = fixture("forward_only_migration.flow.red.rs.txt");
    assert!(
        !forward_only_migration().run(&red).is_empty(),
        "a down/blocking-ALTER migration must be REJECTED by forward-only-migration (§9, the data-loss floor)"
    );
    let green = fixture("forward_only_migration.flow.green.rs.txt");
    assert!(
        forward_only_migration().run(&green).is_empty(),
        "the expand step (nullable add + CONCURRENTLY index) must be ADMITTED by forward-only-migration"
    );
}

/// `no-untagged-personal-data` (contract 1.6 / gdpr §2.1) REJECTS an untagged inline-PII result
/// body and ADMITS the same column once `#[personal_data(...)]`-tagged with the canonical multi-line
/// six-tag `CryptoShred(subject_dek)` form the flow schema uses on its `result_key_ref` /
/// `payload_key_ref` crypto-shred locators. A regression that dropped the tag-tracking would leave
/// an un-erasable inline-PII result.
#[test]
fn no_untagged_personal_data_rejects_untagged_admits_tagged() {
    let red = fixture("no_untagged_personal_data.flow.red.rs.txt");
    assert!(
        !no_untagged_personal_data().run(&red).is_empty(),
        "an untagged inline-PII column (`message_body`) must be REJECTED by no-untagged-personal-data"
    );
    let green = fixture("no_untagged_personal_data.flow.green.rs.txt");
    assert!(
        no_untagged_personal_data().run(&green).is_empty(),
        "a #[personal_data(...)]-tagged column must be ADMITTED (the flow schema's CryptoShred tag shape)"
    );
}

/// `tenant-predicate` (ID-3 / EI-02 §1) REJECTS a tenant-less query and ADMITS a tenant-scoped one —
/// the no-cross-tenant-query-path floor every flow table's tenant-first PK enforces. A regression
/// that admitted a tenant-less query over `workflow_run` is the IDOR (F2) bug class.
#[test]
fn tenant_predicate_rejects_tenantless_admits_tenant_scoped() {
    let red = fixture("tenant_predicate.flow.red.rs.txt");
    assert!(
        !tenant_predicate().run(&red).is_empty(),
        "a tenant-less query must be REJECTED by tenant-predicate (ID-3, the IDOR floor)"
    );
    let green = fixture("tenant_predicate.flow.green.rs.txt");
    assert!(
        tenant_predicate().run(&green).is_empty(),
        "a tenant-scoped query must be ADMITTED by tenant-predicate"
    );
}

/// `no-cross-db` (ADR-01 / EI-02 §8) REJECTS a reach into a sibling service's internal storage
/// module and ADMITS coupling over the frozen contract surface. The flow engine is Postgres-EMBEDDED
/// in its OWN DB (architecture §2, one DB per service); a cross-DB reach is the coupling bug class.
#[test]
fn no_cross_db_rejects_storage_reach_admits_contract_coupling() {
    let red = fixture("no_cross_db.flow.red.rs.txt");
    assert!(
        !no_cross_db().run(&red).is_empty(),
        "a reach into a sibling service's internal store must be REJECTED by no-cross-db (ADR-01)"
    );
    let green = fixture("no_cross_db.flow.green.rs.txt");
    assert!(
        no_cross_db().run(&green).is_empty(),
        "coupling over the frozen contract surface (ArtifactRef/TenantId) must be ADMITTED by no-cross-db"
    );
}
