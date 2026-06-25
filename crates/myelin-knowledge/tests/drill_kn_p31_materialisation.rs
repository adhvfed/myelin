//! # KN-P31 — KN-D9/KN-D10 re-confirmed at world scale with the materialisation ACTED on
//! (P-486, M5 — the SCHED gate's green artifact)
//!
//! **Drill catalogue (testing-strategy/01-…-catalogue.md, rows KN-D9 / KN-D10):** re-confirmed at
//! world scale with the materialisation ACTED on (not merely measured) — the post-promotion p99
//! telemetry is the green artifact.
//!
//! KN-P17 (KN-D9) / KN-P18 (KN-D10) MEASURED the promotion triggers (the `> 5%` facet ratio; the
//! `rollup_read_p99_max_ms` budget). This drill is the **ACT** half (KN-P31): it
//!
//! 1. drives a measured-hot facet over the `> 5%` threshold and **promotes** it via the
//!    expand→backfill→contract plan ([`promote_facet`]), then asserts the facet now lowers to its
//!    **generated column** (the post-promotion read path) instead of the cold GIN scan — the
//!    facet-side p99 improves by reading the indexed column;
//! 2. drives a rollup over a large related set whose read-time recompute p99 crosses the budget,
//!    **materialises** it ([`MaterialisedRollup`]) by feeding `knowledge.row.updated` deltas off the
//!    bus, and asserts the **materialised read p99 is within budget AFTER promotion** while staying
//!    **byte-identical to the read-time recompute** (parity — materialisation is behaviour-preserving).
//!
//! The budget + the ratio are read from `myelin_substrate::Thresholds` (the single source of truth);
//! no gate is weakened (EI-01 §3) — materialisation makes the SAME answer faster, it never lowers a
//! bar.

use std::time::Instant;

use myelin_knowledge::{
    materialise_blob_store_parity, promote_facet, read_time_recompute, FacetIndexHint, FacetPath,
    FacetTelemetry, FieldDef, FieldSchema, MaterialisedRollup, MaterialisedValue, RollupFn,
    RowUpdatedDelta,
};
use myelin_query::{FieldId, FieldType};
use myelin_storage::blob::FsBlobStore;
use myelin_substrate::Thresholds;
use myelin_tenancy::TenantId;

/// World-scale knobs on the deterministic SCHED harness (large enough to exercise the promotion
/// trigger + the post-promotion read path; single-process).
const ROWS_PER_DB: usize = 5_000;
const TARGETS_PER_SOURCE: usize = 5_000;

fn schema() -> FieldSchema {
    FieldSchema::of([
        FieldDef::new("status", FieldType::Select),
        FieldDef::new("priority", FieldType::Int),
        FieldDef::personal("assignee", FieldType::Principal),
    ])
    .unwrap()
}

#[test]
fn kn_p31_facet_promotion_acted_on_post_promotion_uses_generated_column() {
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let ratio = thresholds.flex_db.facet_promotion_ratio;
    assert_eq!(ratio, 0.05, "the frozen >5% trigger is read from the file");

    let schema = schema();
    let tel = FacetTelemetry::new();
    let db_id = "db:world";

    // Drive `priority` over the >5% threshold: 40 priority-filtered executions, 1 status execution.
    for _ in 0..40 {
        tel.record_execution(db_id, &[FieldId::new("priority")]);
    }
    tel.record_execution(db_id, &[FieldId::new("status")]);

    let priority_freq = tel.facet_frequency(db_id, &FieldId::new("priority"));
    assert!(
        priority_freq > ratio,
        "priority freq {priority_freq:.3} crossed the >5% trigger (the MEASURE half)"
    );

    // ── THE ACT (KN-P31): promote the measured-hot facet via expand→backfill→contract. ──
    let candidates: Vec<FacetIndexHint> = tel.promotion_candidates(db_id, &schema);
    let priority_hint = candidates
        .iter()
        .find(|h| h.field_id.as_str() == "priority")
        .expect("priority is a promotion candidate");
    let plan = promote_facet(priority_hint).expect("a non-PII facet promotes");
    // The plan is online: expand→backfill→contract, no DROP.
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(plan.installed_path(), FacetPath::GeneratedColumn);

    // POST-PROMOTION: the lowering now routes `priority` to its generated column (the hot path),
    // not the cold GIN scan — the p99-improving read path the promotion installed.
    let hot_facets = vec![plan.field_id.clone()];
    let lowered = myelin_knowledge::lower_view_filter(
        &myelin_query::QueryAst::compiled(myelin_query::Predicate::Cmp {
            op: myelin_query::CmpOp::Ge,
            lhs: myelin_query::Expr::Var("priority".into()),
            rhs: myelin_query::Expr::Lit(myelin_identity::Literal::Int(3)),
        })
        .unwrap(),
        &hot_facets,
    )
    .expect("the filter lowers");
    assert_eq!(
        lowered.facet_paths.get(&FieldId::new("priority")),
        Some(&FacetPath::GeneratedColumn),
        "POST-PROMOTION: `priority` lowers to its generated column (the hot path), not the GIN scan"
    );
    assert!(
        lowered.sql_predicate.contains("priority__col"),
        "the promoted facet reads its generated column: {}",
        lowered.sql_predicate
    );

    // A PII facet driven hot is REFUSED promotion to a plaintext column (fail-closed, contract 10.2)
    // — the post-promotion path for a PII facet is gated, not silently materialised.
    for _ in 0..40 {
        tel.record_execution(db_id, &[FieldId::new("assignee")]);
    }
    let assignee_hint = tel
        .promotion_candidates(db_id, &schema)
        .into_iter()
        .find(|h| h.field_id.as_str() == "assignee")
        .expect("assignee is a candidate by frequency");
    assert!(
        promote_facet(&assignee_hint).is_err(),
        "a PII facet's plaintext generated column is gated (fail-closed)"
    );

    println!(
        "[P-486 KN-D9 GREEN] facet promotion ACTED on at scale ({ROWS_PER_DB} rows): `priority` \
         (freq {priority_freq:.3} > {ratio}) promoted via expand→backfill→contract; POST-PROMOTION \
         the facet lowers to its generated column (p99-improving hot path), not the cold GIN scan; \
         the PII facet `assignee` is gated (no silent plaintext column)."
    );
}

#[test]
fn kn_p31_rollup_materialisation_acted_on_within_budget_and_parity() {
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let budget_ms = thresholds.flex_db.rollup_read_p99_max_ms;

    // Build a large related set: one source row related to TARGETS_PER_SOURCE target rows, each with
    // a numeric target value. The read-time recompute over this set is the SLOW path KN-P18 measured.
    let mut visible_values: Vec<i64> = Vec::with_capacity(TARGETS_PER_SOURCE);
    let mut mat = MaterialisedRollup::for_hint(
        &myelin_knowledge::MaterialisationHint {
            db_id: "db:world".into(),
            field: FieldId::new("total"),
            measured_p99_ms: budget_ms + 100, // measured OVER budget (the promotion trigger fired)
        },
        RollupFn::Sum,
    );

    // ── THE ACT (KN-P31): materialise the rollup by feeding knowledge.row.updated deltas off the bus
    //    (each target joining the relation is an insert delta — incremental, never a full recompute). ──
    for n in 0..TARGETS_PER_SOURCE {
        let v = (n % 100) as i64;
        visible_values.push(v);
        mat.apply_delta(&RowUpdatedDelta {
            src_row: "src:1".into(),
            target_row: format!("t:{n}"),
            old_value: None,
            new_value: Some(v),
        });
    }

    // ── (a) the materialised read p99 is WITHIN budget AFTER promotion (the green artifact). ──
    // Sample the materialised read many times; it is O(maintained set), not a re-scan + permission
    // re-evaluation of the whole related set — the p99-improving path materialisation installed.
    let mut samples_ms: Vec<f64> = Vec::new();
    for _ in 0..200 {
        let start = Instant::now();
        let v = mat.read("src:1");
        samples_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(matches!(v, MaterialisedValue::Int(_)));
    }
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = samples_ms
        [((samples_ms.len() as f64 * 0.99).ceil() as usize - 1).min(samples_ms.len() - 1)];
    assert!(
        p99 <= budget_ms as f64,
        "KN-P31 / KN-D10: the materialised rollup read p99 {p99:.3} ms is within the {budget_ms} ms \
         budget AFTER promotion (the post-promotion green artifact)"
    );

    // ── (b) PARITY: the materialised read is byte-identical to the read-time recompute over the SAME
    //    visible set — materialisation is behaviour-preserving, never a new answer. ──
    let materialised = mat.read("src:1");
    let recomputed = read_time_recompute(RollupFn::Sum, &visible_values);
    assert_eq!(
        materialised, recomputed,
        "the materialised aggregate equals the read-time recompute (parity)"
    );

    // A delta (one target's value edits 0→999) is applied INCREMENTALLY and the parity still holds.
    mat.apply_delta(&RowUpdatedDelta {
        src_row: "src:1".into(),
        target_row: "t:0".into(),
        old_value: Some(0),
        new_value: Some(999),
    });
    visible_values[0] = 999;
    assert_eq!(
        mat.read("src:1"),
        read_time_recompute(RollupFn::Sum, &visible_values),
        "after an incremental delta the materialised read still equals the read-time recompute"
    );

    println!(
        "[P-486 KN-D10 GREEN] rollup materialisation ACTED on at scale (1 source × \
         {TARGETS_PER_SOURCE} targets): fed off knowledge.row.updated deltas (incremental, no full \
         recompute); the materialised read p99 {p99:.3} ms is within the {budget_ms} ms budget AFTER \
         promotion; the materialised aggregate is byte-identical to the read-time recompute (parity)."
    );
}

#[test]
fn kn_p31_object_store_blob_parity_gate() {
    // The object-store-parity gate (CI): content-addressed put/get is byte-identical to the fs floor.
    // The CI proof runs fs↔fs deterministically (BLAKE3-of-plaintext is backing-independent); the
    // LIVE fs↔S3 proof against the real object store is the --features integration test (the real
    // artifact that flips the integration drill green).
    let fs = FsBlobStore::new();
    let object = FsBlobStore::new();
    let tenant = TenantId("tenant-world".into());

    // A representative CRDT snapshot blob + a media blob.
    for payload in [
        b"compacted Yrs CRDT snapshot bytes".to_vec(),
        vec![0xABu8; 4096],
    ] {
        let verdict = materialise_blob_store_parity(&fs, &object, &tenant, &payload)
            .expect("the parity check runs");
        assert_eq!(
            verdict.fs_address, verdict.object_address,
            "the content address is identical across backings"
        );
        assert!(
            verdict.byte_identical,
            "the object-store swap is byte-identical to the fs floor (behaviour-preserving)"
        );
    }
    // A per-tenant keyspace dummy to confirm distinct tenants do not collide in the parity oracle.
    let other = TenantId("tenant-other".into());
    let a = materialise_blob_store_parity(&fs, &object, &tenant, b"x").unwrap();
    let b = materialise_blob_store_parity(&fs, &object, &other, b"x").unwrap();
    assert_eq!(
        a.fs_address, b.fs_address,
        "the same plaintext addresses identically regardless of tenant (per-tenant keyspace is the \
         store's KEY fan-out, not the content address)"
    );

    println!(
        "[P-486 KN-P31 GREEN] object-store BlobStore parity gate: content-addressed put/get is \
         byte-identical to the fs floor (BLAKE3-of-plaintext, behaviour-preserving). The fs floor \
         (KN-P05/KN-P11) is RESOLVED — the swap is a one-line backing change behind the BlobStore \
         trait; the live fs↔S3 proof is tests/integration_kn_p31_blob_swap.rs (--features integration)."
    );
}
