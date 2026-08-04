use std::path::{Path, PathBuf};

use myelin_lints::{no_untagged_personal_data, residency_pin, tenant_predicate};

fn fixture(name: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

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
