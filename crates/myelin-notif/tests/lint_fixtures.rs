//! Lint-fixture tests for the Notif data model (NOTIF-P2 / P-180) — the committed-ratchet proof
//! (EI-01 §5) that the THREE schema gates this prompt names are LIVE, not vacuously green:
//! `no-untagged-personal-data`, `residency-pin`, `tenant-predicate`. Each runs the REAL lint
//! ([`myelin_lints`]) over a deliberately-broken (RED) fixture and over a clean (GREEN) fixture, so
//! a regression that lets an untagged-PII column / a region-less store / a tenant-less query slip
//! through fails THIS build (defense in depth: the lint at source-scan, the runner at boot).
//!
//! The fixtures live under `tests/fixtures/*.rs.txt` — the SAME `/fixtures/` convention the workspace
//! lint-gate (`myelin-lints/src/bin/lint-gate.rs`) EXCLUDES from the live scan, so the deliberately
//! red samples here do NOT trip the real CI gate over the workspace (they are scanner DATA, not real
//! crate code). The Notif schema's actual PII columns are tagged in `src/schema.rs`; the actual
//! migrations are tenant-first in `src/migrations.rs` — these fixtures prove the SCANNERS bite.

use std::path::{Path, PathBuf};

use myelin_lints::{no_untagged_personal_data, residency_pin, tenant_predicate};

fn fixture(name: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

/// `no-untagged-personal-data` (contract 1.6 / gdpr §2.1) REJECTS an untagged PII column and ADMITS
/// the same column once `#[personal_data(...)]`-tagged (the canonical multi-line six-tag form the
/// Notif schema uses). A regression that dropped the tag-tracking would leave an un-erasable subject.
#[test]
fn no_untagged_personal_data_rejects_untagged_admits_tagged() {
    let red = fixture("no_untagged_personal_data.notif.red.rs.txt");
    assert!(
        !no_untagged_personal_data().run(&red).is_empty(),
        "an untagged PII column (`display_name`) must be REJECTED by no-untagged-personal-data"
    );
    let green = fixture("no_untagged_personal_data.notif.green.rs.txt");
    assert!(
        no_untagged_personal_data().run(&green).is_empty(),
        "a #[personal_data(...)]-tagged column must be ADMITTED (the Notif schema's tag shape)"
    );
}

/// `residency-pin` (ADR-11) REJECTS a global (region-less) pool and ADMITS a region-pinned one — the
/// EU-sovereign residency floor the nine `(tenant, region)`-first tables rest on. A regression that
/// admitted a global pool would let a tenant's data leave its region.
#[test]
fn residency_pin_rejects_global_pool_admits_region_pinned() {
    let red = fixture("residency_pin.notif.red.rs.txt");
    assert!(
        !residency_pin().run(&red).is_empty(),
        "a global (region-less) pool must be REJECTED by residency-pin (ADR-11)"
    );
    let green = fixture("residency_pin.notif.green.rs.txt");
    assert!(
        residency_pin().run(&green).is_empty(),
        "a region-pinned pool must be ADMITTED by residency-pin"
    );
}

/// `tenant-predicate` (ID-3 / EI-02 §1) REJECTS a tenant-less query and ADMITS a tenant-scoped one —
/// the no-cross-tenant-query-path floor every Notif table's tenant-first PK enforces. A regression
/// that admitted a tenant-less query is the IDOR (F2) bug class.
#[test]
fn tenant_predicate_rejects_tenantless_admits_tenant_scoped() {
    let red = fixture("tenant_predicate.notif.red.rs.txt");
    assert!(
        !tenant_predicate().run(&red).is_empty(),
        "a tenant-less query must be REJECTED by tenant-predicate (ID-3, the IDOR floor)"
    );
    let green = fixture("tenant_predicate.notif.green.rs.txt");
    assert!(
        tenant_predicate().run(&green).is_empty(),
        "a tenant-scoped query must be ADMITTED by tenant-predicate"
    );
}
