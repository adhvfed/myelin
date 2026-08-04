use super::*;
use myelin_query::{FieldId, FieldType};
use myelin_storage::blob::FsBlobStore;
use myelin_storage::migration::MigrationPhase;
use myelin_tenancy::TenantId;

use crate::database::FacetIndexHint;
use crate::rollup::{MaterialisationHint, RollupFn};

fn hint(field: &str, ft: FieldType, pii: bool) -> FacetIndexHint {
    FacetIndexHint {
        field_id: FieldId::new(field),
        field_type: ft,
        personal_data: pii,
    }
}

#[test]
fn promote_facet_builds_expand_backfill_contract_in_order() {
    let plan = promote_facet(&hint("priority", FieldType::Int, false)).expect("non-PII promotes");
    assert_eq!(
        plan.phases(),
        vec![
            MigrationPhase::Expand,
            MigrationPhase::Backfill,
            MigrationPhase::Contract
        ],
        "the per-facet promotion is an ordered expand→backfill→contract online migration"
    );
    let expand = &plan.steps[0].ddl;
    assert!(
        expand.contains("priority__col") && expand.contains("GENERATED ALWAYS AS"),
        "Expand adds the generated column: {expand}"
    );
    assert!(
        expand.contains("CREATE INDEX CONCURRENTLY"),
        "Expand builds the index CONCURRENTLY (non-blocking): {expand}"
    );
    assert!(
        expand.contains("(props ->> 'priority')::BIGINT"),
        "an Int facet's generated column casts the JSONB path to BIGINT: {expand}"
    );
    for step in &plan.steps {
        assert!(
            !step.ddl.to_uppercase().contains("DROP"),
            "no DROP in a forward-only promotion: {}",
            step.ddl
        );
    }
    assert_eq!(plan.installed_path(), FacetPath::GeneratedColumn);
}

#[test]
fn promote_text_facet_uses_text_column_no_cast() {
    let plan = promote_facet(&hint("status", FieldType::Select, false)).unwrap();
    let expand = &plan.steps[0].ddl;
    assert!(
        expand.contains("status__col TEXT"),
        "a string-shaped facet's generated column is TEXT: {expand}"
    );
    assert!(
        expand.contains("(props ->> 'status') STORED")
            || expand.contains("AS (props ->> 'status')"),
        "the text column extracts the JSONB path without a numeric cast: {expand}"
    );
}

#[test]
fn promote_bool_facet_casts_to_boolean() {
    let plan = promote_facet(&hint("done", FieldType::Bool, false)).unwrap();
    let expand = &plan.steps[0].ddl;
    assert!(
        expand.contains("done__col BOOLEAN"),
        "a Bool facet's generated column is BOOLEAN: {expand}"
    );
    assert!(
        expand.contains("(props ->> 'done')::BOOLEAN"),
        "the Bool column casts the JSONB path to BOOLEAN (not the bare text path): {expand}"
    );
}

#[test]
fn pii_facet_promotion_is_gated_fail_closed() {
    let err = promote_facet(&hint("assignee", FieldType::Principal, true)).unwrap_err();
    assert_eq!(
        err,
        FacetPromotionError::PiiFacetGated {
            field: "assignee".into()
        },
        "a PII facet is refused without the cleared caveat (contract 10.2, fail-closed)"
    );
    let plan = promote_facet_pii_cleared(&hint("assignee", FieldType::Principal, true));
    assert!(plan.personal_data);
    assert_eq!(plan.phases().len(), 3);
}

#[test]
fn facet_ident_is_sanitised_into_the_ddl() {
    let plan = promote_facet(&hint("pri ority; DROP", FieldType::Int, false)).unwrap();
    let expand = &plan.steps[0].ddl;
    assert!(
        !expand.contains(';') || expand.matches(';').count() == 1,
        "the only ';' is the statement separator, not an injected one: {expand}"
    );
    assert!(
        expand.contains("priorityDROP__col"),
        "the column ident is sanitised to safe bytes: {expand}"
    );
}

fn mat_hint(db: &str, field: &str, p99: u64) -> MaterialisationHint {
    MaterialisationHint {
        db_id: db.into(),
        field: FieldId::new(field),
        measured_p99_ms: p99,
    }
}

fn delta(src: &str, target: &str, old: Option<i64>, new: Option<i64>) -> RowUpdatedDelta {
    RowUpdatedDelta {
        src_row: src.into(),
        target_row: target.into(),
        old_value: old,
        new_value: new,
    }
}

#[test]
fn materialised_sum_is_maintained_incrementally_and_matches_read_time() {
    let mut mat = MaterialisedRollup::for_hint(&mat_hint("db:0", "total", 999), RollupFn::Sum);
    mat.apply_delta(&delta("src:1", "t:a", None, Some(10)));
    mat.apply_delta(&delta("src:1", "t:b", None, Some(20)));
    mat.apply_delta(&delta("src:1", "t:c", None, Some(30)));
    assert_eq!(mat.read("src:1"), MaterialisedValue::Int(60));
    assert_eq!(
        mat.read("src:1"),
        read_time_recompute(RollupFn::Sum, &[10, 20, 30])
    );
    mat.apply_delta(&delta("src:1", "t:b", Some(20), Some(5)));
    assert_eq!(mat.read("src:1"), MaterialisedValue::Int(45));
    assert_eq!(
        mat.read("src:1"),
        read_time_recompute(RollupFn::Sum, &[10, 5, 30])
    );
    mat.apply_delta(&delta("src:1", "t:c", Some(30), None));
    assert_eq!(mat.read("src:1"), MaterialisedValue::Int(15));
    assert_eq!(
        mat.read("src:1"),
        read_time_recompute(RollupFn::Sum, &[10, 5])
    );
}

#[test]
fn materialised_count_and_avg_and_min_max() {
    let mut count = MaterialisedRollup::for_hint(&mat_hint("db:0", "n", 999), RollupFn::Count);
    count.apply_delta(&delta("s", "a", None, Some(7)));
    count.apply_delta(&delta("s", "b", None, Some(7)));
    assert_eq!(count.read("s"), MaterialisedValue::Int(2));

    let mut avg = MaterialisedRollup::for_hint(&mat_hint("db:0", "n", 999), RollupFn::Avg);
    avg.apply_delta(&delta("s", "a", None, Some(10)));
    avg.apply_delta(&delta("s", "b", None, Some(21)));
    assert_eq!(avg.read("s"), MaterialisedValue::Int(15));
    assert_eq!(avg.read("s"), read_time_recompute(RollupFn::Avg, &[10, 21]));

    let mut max = MaterialisedRollup::for_hint(&mat_hint("db:0", "n", 999), RollupFn::Max);
    assert_eq!(max.read("s"), MaterialisedValue::Empty);
    max.apply_delta(&delta("s", "a", None, Some(3)));
    max.apply_delta(&delta("s", "b", None, Some(9)));
    max.apply_delta(&delta("s", "c", None, Some(5)));
    assert_eq!(max.read("s"), MaterialisedValue::Int(9));
    max.apply_delta(&delta("s", "b", Some(9), None));
    assert_eq!(max.read("s"), MaterialisedValue::Int(5));
    assert_eq!(max.read("s"), read_time_recompute(RollupFn::Max, &[3, 5]));

    let mut min = MaterialisedRollup::for_hint(&mat_hint("db:0", "n", 999), RollupFn::Min);
    min.apply_delta(&delta("s", "a", None, Some(3)));
    min.apply_delta(&delta("s", "b", None, Some(9)));
    min.apply_delta(&delta("s", "a", Some(3), None));
    assert_eq!(min.read("s"), MaterialisedValue::Int(9));
}

#[test]
fn duplicate_delivery_does_not_double_count() {
    let mut sum = MaterialisedRollup::for_hint(&mat_hint("db:0", "total", 999), RollupFn::Sum);
    let d = delta("s", "a", None, Some(10));
    sum.apply_delta(&d);
    sum.apply_delta(&d);
    sum.apply_delta(&d);
    assert_eq!(
        sum.read("s"),
        MaterialisedValue::Int(10),
        "a duplicate delivery converges to the same state, never double-counts"
    );
    let mut count = MaterialisedRollup::for_hint(&mat_hint("db:0", "n", 999), RollupFn::Count);
    count.apply_delta(&delta("s", "a", None, Some(1)));
    count.apply_delta(&delta("s", "a", None, Some(1)));
    assert_eq!(count.read("s"), MaterialisedValue::Int(1));
}

#[test]
fn materialisation_is_per_rollup_not_wholesale() {
    let mut mat = MaterialisedRollup::for_hint(&mat_hint("db:0", "total", 999), RollupFn::Sum);
    assert_eq!(mat.materialised_rows(), 0);
    mat.apply_delta(&delta("src:1", "t:a", None, Some(1)));
    mat.apply_delta(&delta("src:2", "t:b", None, Some(1)));
    assert_eq!(mat.materialised_rows(), 2);
    assert_eq!(mat.read("src:never"), MaterialisedValue::Int(0));
}

#[test]
fn target_numeric_value_skips_non_int() {
    use myelin_query::FieldValue;
    assert_eq!(target_numeric_value(Some(&FieldValue::Int(42))), Some(42));
    assert_eq!(
        target_numeric_value(Some(&FieldValue::Text("x".into()))),
        None
    );
    assert_eq!(target_numeric_value(None), None);
}

#[test]
fn blob_store_swap_is_content_addressed_and_byte_identical() {
    let fs = FsBlobStore::new();
    let object = FsBlobStore::new();
    let tenant = TenantId("tenant-7".into());
    let bytes = b"the compacted CRDT snapshot bytes (content-addressed, BLAKE3)";
    let verdict =
        materialise_blob_store_parity(&fs, &object, &tenant, bytes).expect("the parity check runs");
    assert_eq!(
        verdict.fs_address, verdict.object_address,
        "the content address is identical across backings (BLAKE3-of-plaintext)"
    );
    assert!(
        verdict.byte_identical,
        "the swap is byte-identical: same address, same bytes back from both stores"
    );
}

struct WrongAddressOnlyStore(FsBlobStore);
struct WrongBytesOnlyStore(FsBlobStore);

use myelin_storage::blob::{BlobMeta, ContentHash, Result as BlobResult};

impl myelin_storage::blob::BlobStore for WrongAddressOnlyStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> BlobResult<ContentHash> {
        self.0.put(tenant, bytes)?;
        Ok(ContentHash::blake3(b"a different payload entirely"))
    }
    fn get(&self, _tenant: &TenantId, _hash: &ContentHash) -> BlobResult<Vec<u8>> {
        Ok(WRONG_ADDR_PAYLOAD.to_vec())
    }
    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> BlobResult<BlobMeta> {
        self.0.head(tenant, hash)
    }
    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> BlobResult<()> {
        self.0.delete(tenant, hash)
    }
}

impl myelin_storage::blob::BlobStore for WrongBytesOnlyStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> BlobResult<ContentHash> {
        self.0.put(tenant, bytes)
    }
    fn get(&self, _tenant: &TenantId, _hash: &ContentHash) -> BlobResult<Vec<u8>> {
        Ok(b"not the input bytes".to_vec())
    }
    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> BlobResult<BlobMeta> {
        self.0.head(tenant, hash)
    }
    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> BlobResult<()> {
        self.0.delete(tenant, hash)
    }
}

const WRONG_ADDR_PAYLOAD: &[u8] = b"parity-test-input";

#[test]
fn blob_store_swap_wrong_address_only_is_not_byte_identical() {
    let fs = FsBlobStore::new();
    let object = WrongAddressOnlyStore(FsBlobStore::new());
    let tenant = TenantId("t".into());
    let verdict = materialise_blob_store_parity(&fs, &object, &tenant, WRONG_ADDR_PAYLOAD).unwrap();
    assert_ne!(verdict.fs_address, verdict.object_address);
    assert!(
        !verdict.byte_identical,
        "a different content address is NOT byte-identical (the address conjunct is load-bearing)"
    );
}

#[test]
fn blob_store_swap_wrong_bytes_only_is_not_byte_identical() {
    let fs = FsBlobStore::new();
    let object = WrongBytesOnlyStore(FsBlobStore::new());
    let tenant = TenantId("t".into());
    let verdict = materialise_blob_store_parity(&fs, &object, &tenant, WRONG_ADDR_PAYLOAD).unwrap();
    assert_eq!(
        verdict.fs_address, verdict.object_address,
        "the address matches (faithful put)"
    );
    assert!(
        !verdict.byte_identical,
        "a wrong round-trip is NOT byte-identical (the round-trip conjunct is load-bearing)"
    );
}

#[test]
fn blob_store_swap_distinct_bytes_distinct_address() {
    let fs = FsBlobStore::new();
    let object = FsBlobStore::new();
    let tenant = TenantId("t".into());
    let a = materialise_blob_store_parity(&fs, &object, &tenant, b"alpha").unwrap();
    let b = materialise_blob_store_parity(&fs, &object, &tenant, b"beta").unwrap();
    assert!(a.byte_identical && b.byte_identical);
    assert_ne!(
        a.fs_address, b.fs_address,
        "distinct bytes → distinct content address (no collision)"
    );
}
