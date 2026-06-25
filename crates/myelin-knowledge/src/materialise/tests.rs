//! Unit tests for the facet/rollup materialisation ACT + the object-store BlobStore swap parity
//! (KN-P31 / P-486, M5).
//!
//! - the per-facet expand→backfill→contract promotion plan (ordered, online, PII-gated);
//! - the per-rollup incrementally-maintained materialised aggregate fed off `knowledge.row.updated`
//!   (incremental, parity with the read-time recompute, idempotent on a duplicate delivery);
//! - the object-store BlobStore swap parity (content-addressed put/get byte-identical to the fs
//!   floor — proven fs↔fs deterministically here; the live fs↔S3 proof is the integration test).

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

// ───────────────────────────── per-facet materialisation ───────────────────────────────────────────

#[test]
fn promote_facet_builds_expand_backfill_contract_in_order() {
    let plan = promote_facet(&hint("priority", FieldType::Int, false)).expect("non-PII promotes");
    // The plan is online by construction: exactly Expand → Backfill → Contract, in order.
    assert_eq!(
        plan.phases(),
        vec![
            MigrationPhase::Expand,
            MigrationPhase::Backfill,
            MigrationPhase::Contract
        ],
        "the per-facet promotion is an ordered expand→backfill→contract online migration"
    );
    // The Expand step provisions the generated column + a CONCURRENT index (non-blocking on the hot
    // db_row table — never one blocking ALTER).
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
    // No DROP anywhere — forward-only.
    for step in &plan.steps {
        assert!(
            !step.ddl.to_uppercase().contains("DROP"),
            "no DROP in a forward-only promotion: {}",
            step.ddl
        );
    }
    // Once contracted, the facet lowers to its generated column, not the GIN scan.
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
    // A PII facet cannot be promoted to a plaintext generated column without the field-level caveat.
    let err = promote_facet(&hint("assignee", FieldType::Principal, true)).unwrap_err();
    assert_eq!(
        err,
        FacetPromotionError::PiiFacetGated {
            field: "assignee".into()
        },
        "a PII facet is refused without the cleared caveat (contract 10.2, fail-closed)"
    );
    // With the caveat cleared by the caller, the SAME plan is built (the caveat clearance is the
    // identity tier's ABAC decision; this records it was made).
    let plan = promote_facet_pii_cleared(&hint("assignee", FieldType::Principal, true));
    assert!(plan.personal_data);
    assert_eq!(plan.phases().len(), 3);
}

#[test]
fn facet_ident_is_sanitised_into_the_ddl() {
    // A facet name carrying a non-identifier byte never reaches the DDL un-sanitised (defence in
    // depth — the column identifier is sanitised; the JSONB key path keeps only safe bytes too).
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

// ───────────────────────────── per-rollup materialisation ───────────────────────────────────────────

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
    // Three targets join the src row's rollup_source relation (inserts).
    mat.apply_delta(&delta("src:1", "t:a", None, Some(10)));
    mat.apply_delta(&delta("src:1", "t:b", None, Some(20)));
    mat.apply_delta(&delta("src:1", "t:c", None, Some(30)));
    assert_eq!(mat.read("src:1"), MaterialisedValue::Int(60));
    // Parity: the materialised read equals the read-time recompute over the SAME visible set.
    assert_eq!(
        mat.read("src:1"),
        read_time_recompute(RollupFn::Sum, &[10, 20, 30])
    );
    // A value edit (t:b 20 → 5) is applied as a DELTA — not a full recompute.
    mat.apply_delta(&delta("src:1", "t:b", Some(20), Some(5)));
    assert_eq!(mat.read("src:1"), MaterialisedValue::Int(45));
    assert_eq!(
        mat.read("src:1"),
        read_time_recompute(RollupFn::Sum, &[10, 5, 30])
    );
    // A leave (t:c leaves the relation) removes it.
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
    // Integer floor average: (10+21)/2 == 15.
    assert_eq!(avg.read("s"), MaterialisedValue::Int(15));
    assert_eq!(avg.read("s"), read_time_recompute(RollupFn::Avg, &[10, 21]));

    let mut max = MaterialisedRollup::for_hint(&mat_hint("db:0", "n", 999), RollupFn::Max);
    // Empty set → #EMPTY (parity with the read-time Min/Max diagnostic).
    assert_eq!(max.read("s"), MaterialisedValue::Empty);
    max.apply_delta(&delta("s", "a", None, Some(3)));
    max.apply_delta(&delta("s", "b", None, Some(9)));
    max.apply_delta(&delta("s", "c", None, Some(5)));
    assert_eq!(max.read("s"), MaterialisedValue::Int(9));
    // Remove the current max (b leaves) — Min/Max stays EXACT because the value set is maintained.
    max.apply_delta(&delta("s", "b", Some(9), None));
    assert_eq!(max.read("s"), MaterialisedValue::Int(5));
    assert_eq!(max.read("s"), read_time_recompute(RollupFn::Max, &[3, 5]));

    let mut min = MaterialisedRollup::for_hint(&mat_hint("db:0", "n", 999), RollupFn::Min);
    min.apply_delta(&delta("s", "a", None, Some(3)));
    min.apply_delta(&delta("s", "b", None, Some(9)));
    min.apply_delta(&delta("s", "a", Some(3), None)); // the current min leaves
    assert_eq!(min.read("s"), MaterialisedValue::Int(9));
}

#[test]
fn duplicate_delivery_does_not_double_count() {
    // The bus is at-least-once: a duplicate `knowledge.row.updated` delivery must NOT double-count
    // (the aggregate is idempotent on the target id — a re-applied final state converges).
    let mut sum = MaterialisedRollup::for_hint(&mat_hint("db:0", "total", 999), RollupFn::Sum);
    let d = delta("s", "a", None, Some(10));
    sum.apply_delta(&d);
    sum.apply_delta(&d); // duplicate delivery
    sum.apply_delta(&d); // and again
    assert_eq!(
        sum.read("s"),
        MaterialisedValue::Int(10),
        "a duplicate delivery converges to the same state, never double-counts"
    );
    let mut count = MaterialisedRollup::for_hint(&mat_hint("db:0", "n", 999), RollupFn::Count);
    count.apply_delta(&delta("s", "a", None, Some(1)));
    count.apply_delta(&delta("s", "a", None, Some(1))); // duplicate
    assert_eq!(count.read("s"), MaterialisedValue::Int(1));
}

#[test]
fn materialisation_is_per_rollup_not_wholesale() {
    // Only the source rows that received a delta have a maintained aggregate — the per-rollup,
    // not-wholesale footprint (the prompt's discipline).
    let mut mat = MaterialisedRollup::for_hint(&mat_hint("db:0", "total", 999), RollupFn::Sum);
    assert_eq!(mat.materialised_rows(), 0);
    mat.apply_delta(&delta("src:1", "t:a", None, Some(1)));
    mat.apply_delta(&delta("src:2", "t:b", None, Some(1)));
    assert_eq!(mat.materialised_rows(), 2);
    // A src row never touched reads as the empty aggregate (Count → 0), never an error.
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

// ───────────────────────────── object-store BlobStore swap parity ───────────────────────────────────

#[test]
fn blob_store_swap_is_content_addressed_and_byte_identical() {
    // The CI parity proof runs fs↔fs deterministically: the content address is BLAKE3-of-plaintext
    // (backing-independent), so two BlobStore backings assign the SAME address and round-trip the
    // SAME bytes. The live fs↔S3 proof is the --features integration test.
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

/// A divergent BlobStore that reports a DIFFERENT content address on `put` but round-trips the input
/// bytes faithfully on `get` (so ONLY the `address_identical` conjunct is false). Isolates the first
/// `&&` so a `&&`→`||` flip would wrongly admit it.
struct WrongAddressOnlyStore(FsBlobStore);
/// A divergent BlobStore that reports the CORRECT address but returns the WRONG bytes on `get` (so
/// ONLY the `object_roundtrip_ok` conjunct is false). Isolates the last `&&`.
struct WrongBytesOnlyStore(FsBlobStore);

use myelin_storage::blob::{BlobMeta, ContentHash, Result as BlobResult};

impl myelin_storage::blob::BlobStore for WrongAddressOnlyStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> BlobResult<ContentHash> {
        // Store faithfully (so `get` returns the input), but REPORT a different address.
        self.0.put(tenant, bytes)?;
        Ok(ContentHash::blake3(b"a different payload entirely"))
    }
    fn get(&self, _tenant: &TenantId, _hash: &ContentHash) -> BlobResult<Vec<u8>> {
        // Round-trip the input bytes faithfully (so ONLY the reported address diverges).
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
        // Report the CORRECT address (faithful put) …
        self.0.put(tenant, bytes)
    }
    fn get(&self, _tenant: &TenantId, _hash: &ContentHash) -> BlobResult<Vec<u8>> {
        // … but return the WRONG bytes (only the round-trip conjunct is false).
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
    // ONLY the address conjunct is false (the bytes round-trip faithfully) — a `&&`→`||` flip on the
    // first conjunct would wrongly admit this; the verdict MUST be not byte-identical.
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
    // ONLY the object round-trip conjunct is false (the address matches) — a `&&`→`||` flip on the
    // last conjunct would wrongly admit this; the verdict MUST be not byte-identical.
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
    // Two distinct payloads get distinct addresses in both backings (content-addressing is not a
    // collision); the parity holds for each independently.
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
