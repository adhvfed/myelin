use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::{
    compute_row, AuthzVisibleIndex, CellValue, DbRelation, FormulaExpr, FormulaField,
    FormulaSchema, PropertyBag, RelationKind, RelationStore, RollupFn, RollupLatencyTelemetry,
    RollupResolver,
};
use myelin_query::{FieldId, FieldValue};
use myelin_substrate::Thresholds;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

const TENANTS: usize = 8;
const TARGETS_PER_SOURCE: usize = 2_000;

fn p(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

#[test]
fn kn_d10_read_time_rollup_within_budget_zero_leak_materialisation_trigger_measured() {
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file must load");
    let budget_ms = thresholds.flex_db.rollup_read_p99_max_ms;
    assert!(
        budget_ms > 0,
        "the KN-D10 rollup p99 budget is a positive number read from thresholds.toml"
    );

    let region = Region::new("fr-par");

    let schema = FormulaSchema::of([
        FormulaField {
            field: FieldId::new("total"),
            expr: FormulaExpr::Rollup {
                func: RollupFn::Sum,
                target: FieldId::new("amount"),
            },
        },
        FormulaField {
            field: FieldId::new("n"),
            expr: FormulaExpr::Rollup {
                func: RollupFn::Count,
                target: FieldId::new("amount"),
            },
        },
        FormulaField {
            field: FieldId::new("hi"),
            expr: FormulaExpr::Rollup {
                func: RollupFn::Max,
                target: FieldId::new("amount"),
            },
        },
    ])
    .unwrap();

    let mut tel = RollupLatencyTelemetry::new();
    let mut per_read_ms: Vec<f64> = Vec::new();
    let mut total_leaked_value: i64 = 0;

    for t in 0..TENANTS {
        let tenant = TenantId(format!("tenant-{t}"));
        let viewer = p("p:0", tenant.0.as_str());
        let relations = RelationStore::new();
        let authz = AuthzVisibleIndex::new();
        let mut target_props: BTreeMap<String, PropertyBag> = BTreeMap::new();
        let src = format!("src:{t}");

        let mut expected_sum: i64 = 0;
        let mut expected_count: i64 = 0;
        let mut expected_max: i64 = i64::MIN;
        let mut hidden_max: i64 = i64::MIN;

        for n in 0..TARGETS_PER_SOURCE {
            let id = format!("t:{t}:{n}");
            let amount = if n < TARGETS_PER_SOURCE / 2 {
                (n % 50) as i64
            } else {
                1_000_000 + n as i64
            };
            relations.relate(
                &tenant,
                DbRelation {
                    relation_id: format!("rel:{t}:{n}"),
                    src_row: src.clone(),
                    dst_ref: ArtifactRef(id.clone()),
                    rel: RelationKind::RollupSource,
                },
            );
            let mut props: PropertyBag = BTreeMap::new();
            props.insert(FieldId::new("amount"), FieldValue::Int(amount));
            target_props.insert(id.clone(), props);
            if n < TARGETS_PER_SOURCE / 2 {
                authz.grant(
                    &tenant,
                    &region,
                    &viewer.principal_id.0,
                    "read",
                    &id,
                    "zk-0000000001",
                );
                expected_sum += amount;
                expected_count += 1;
                expected_max = expected_max.max(amount);
            } else {
                hidden_max = hidden_max.max(amount);
            }
        }

        let resolver = RollupResolver::new(&tenant, &region, &relations, &authz, &target_props);

        let db_id = format!("db:{}", tenant.0);
        for field in ["total", "n", "hi"] {
            let fid = FieldId::new(field);
            let start = Instant::now();
            let out = compute_row(&viewer, &src, &fid, &BTreeMap::new(), &schema, &resolver);
            let elapsed = start.elapsed();
            per_read_ms.push(elapsed.as_secs_f64() * 1000.0);
            tel.record(&db_id, &fid, elapsed);

            match field {
                "total" => {
                    assert_eq!(
                        out,
                        CellValue::Int(expected_sum),
                        "SUM reflects only the granted half (0 rollup leak)"
                    );
                    if let CellValue::Int(v) = out {
                        total_leaked_value += v - expected_sum;
                    }
                }
                "n" => assert_eq!(
                    out,
                    CellValue::Int(expected_count),
                    "COUNT = the granted half (0 rollup leak)"
                ),
                "hi" => {
                    assert_eq!(
                        out,
                        CellValue::Int(expected_max),
                        "MAX = the visible max, NOT the hidden 1M+ (0 value-disclosure leak)"
                    );
                    assert!(
                        hidden_max > expected_max,
                        "the drill's hidden targets are genuinely higher (a real leak witness)"
                    );
                }
                _ => unreachable!(),
            }
        }
    }

    per_read_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = per_read_ms
        [((per_read_ms.len() as f64 * 0.99).ceil() as usize - 1).min(per_read_ms.len() - 1)];
    assert!(
        p99 <= budget_ms as f64,
        "KN-D10: read-time rollup recompute p99 {p99:.3} ms must be within the {budget_ms} ms budget (from thresholds.toml)"
    );

    assert_eq!(
        total_leaked_value, 0,
        "KN-D10 / KN-D5: 0 rollup leak across the whole multi-tenant scale set"
    );

    let live_candidates = tel.materialisation_candidates(budget_ms);
    assert!(
        live_candidates.is_empty(),
        "the live scale rollups are within budget - none is a materialisation candidate yet: {:?}",
        live_candidates
            .iter()
            .map(|c| (&c.db_id, c.field.to_string(), c.measured_p99_ms))
            .collect::<Vec<_>>()
    );
    let slow_field = FieldId::new("slow_rollup");
    for _ in 0..100 {
        tel.record(
            "db:measured-slow",
            &slow_field,
            Duration::from_millis(budget_ms + 200),
        );
    }
    let candidates = tel.materialisation_candidates(budget_ms);
    let slow = candidates
        .iter()
        .find(|c| c.field == slow_field)
        .expect("a rollup whose read-time recompute p99 crosses the budget is a materialisation candidate (KN-P31)");
    assert_eq!(slow.db_id, "db:measured-slow");
    assert!(
        slow.measured_p99_ms > budget_ms,
        "the hint carries the measured p99 that crossed the budget (the KN-P31 trigger)"
    );

    println!(
        "[P-308 KN-D10 GREEN] read-time rollup engine at scale ({} tenants × {} related rows): \
         the permission-filtered SUM/COUNT/MAX recompute p99 {:.3} ms within the {} ms budget \
         (thresholds.toml); 0 rollup leak across the row-restricted multi-tenant related set \
         (list_objects conjoined, composes KN-D5 - the hidden 1M+ targets never summed/counted/maxed); \
         the materialisation trigger MEASURED - the live rollups are within budget (no premature \
         promotion), a measured-slow rollup (p99 {} ms) is flagged for KN-P31's per-rollup \
         materialised aggregate. Rollups are computed at READ TIME, never stored (KN-3).",
        TENANTS, TARGETS_PER_SOURCE, p99, budget_ms, slow.measured_p99_ms
    );
}
