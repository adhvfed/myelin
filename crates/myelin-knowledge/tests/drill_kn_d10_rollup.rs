//! # KN-D10 — the read-time rollup latency + materialisation-trigger drill (KN-P18 / P-308, M3)
//!
//! **Drill catalogue (testing-strategy/01-…-catalogue.md, row KN-D10):** "A rollup over a large
//! related set, computed at read time (permission-filtered) → p99 within budget; measure when
//! incremental materialisation is needed. — rollup p99 — SCHED."
//!
//! This is the named SCHED drill in the master M3 gate (roadmap §3, KN-M3d, the read-time
//! formula/rollup half). It builds a LARGE related set (one source row related to many target rows
//! across several tenants), runs the §4.2 read-time rollup engine — `RollupFn` over the
//! permission-filtered related rows (conjoining the `list_objects` `SetExpr`, so a restricted target
//! is never counted/summed) — and asserts:
//!
//! - **read-time rollup p99 within budget** — the modelled per-rollup read-time recompute cost stays
//!   under the `flex_db.rollup_read_p99_max_ms` budget READ FROM THE THRESHOLDS FILE (never a
//!   hardcoded magic number). On this floor the cost is the in-memory mirror of the permission-
//!   filtered relation read + the aggregate over the visible targets; the LIVE p99-against-Postgres
//!   is the `--features integration` proof + the world-scale re-confirm (KN-P31);
//! - **0 rollup leak across a row-restricted related set** — the `list_objects` `SetExpr` conjoined
//!   into the rollup means a target row the viewer cannot read is uncounted/unsummed (composing with
//!   KN-D5) — measured to be exactly the visible aggregate over the whole scale set;
//! - **the materialisation trigger is MEASURED** — the per-rollup read-time recompute latency
//!   telemetry reports which rollups cross the budget (the per-rollup promotion trigger, KQ-4); the
//!   promotion ACT (the incrementally-maintained materialised aggregate fed off the bus → the OLAP
//!   read store, 11.6) is KN-P31 (M5) — here it is measured, not acted on.
//!
//! The budget is read from `myelin_substrate::Thresholds` (the single source of truth); a red is a
//! dated `[[claimed_not_proven]]` scorecard row, never a weakened threshold (EI-01 §3).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::{
    compute_row, AuthzVisibleIndex, CellValue, DbRelation, FormulaExpr, FormulaField, FormulaSchema,
    PropertyBag, RelationKind, RelationStore, RollupFn, RollupLatencyTelemetry, RollupResolver,
};
use myelin_query::{FieldId, FieldValue};
use myelin_substrate::Thresholds;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

/// The scale knobs (a "large related set" on the deterministic CI/SCHED harness — large enough to
/// exercise the permission-filtered aggregate over many related rows per source, small enough to
/// stay a single-process drill; the world-scale re-run is KN-P31).
const TENANTS: usize = 8;
const TARGETS_PER_SOURCE: usize = 2_000;

fn p(id: &str, tenant: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId(tenant.into()))
}

#[test]
fn kn_d10_read_time_rollup_within_budget_zero_leak_materialisation_trigger_measured() {
    // ── 0. Read the rollup p99 budget from the canonical thresholds file (NOT hardcoded). ──────────
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file must load");
    let budget_ms = thresholds.flex_db.rollup_read_p99_max_ms;
    assert!(budget_ms > 0, "the KN-D10 rollup p99 budget is a positive number read from thresholds.toml");

    let region = Region::new("fr-par");

    // The rollup schema: total = SUM(amount) over the rollup_source relation; n = COUNT; hi = MAX.
    let schema = FormulaSchema::of([
        FormulaField {
            field: FieldId::new("total"),
            expr: FormulaExpr::Rollup { func: RollupFn::Sum, target: FieldId::new("amount") },
        },
        FormulaField {
            field: FieldId::new("n"),
            expr: FormulaExpr::Rollup { func: RollupFn::Count, target: FieldId::new("amount") },
        },
        FormulaField {
            field: FieldId::new("hi"),
            expr: FormulaExpr::Rollup { func: RollupFn::Max, target: FieldId::new("amount") },
        },
    ])
    .unwrap();

    let mut tel = RollupLatencyTelemetry::new();
    let mut per_read_ms: Vec<f64> = Vec::new();
    // The witness: the expected visible SUM/COUNT/MAX per tenant (only the granted half contributes).
    let mut total_leaked_value: i64 = 0;

    // ── 1. Build a LARGE related set per tenant: one source row related to TARGETS_PER_SOURCE target
    //       rows (each with an Int `amount`); grant the viewer read of the FIRST HALF only (the
    //       second half is the leak witness — its amounts must NOT contribute to the rollup). ────────
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
            // A deterministic amount; the SECOND half carries a deliberately HUGE amount so a leak
            // would be a glaring value disclosure (proving 0 leak, not ACL-luck).
            let amount = if n < TARGETS_PER_SOURCE / 2 { (n % 50) as i64 } else { 1_000_000 + n as i64 };
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
                authz.grant(&tenant, &region, &viewer.principal_id.0, "read", &id, "zk-0000000001");
                expected_sum += amount;
                expected_count += 1;
                expected_max = expected_max.max(amount);
            } else {
                hidden_max = hidden_max.max(amount);
            }
        }

        let resolver = RollupResolver::new(&tenant, &region, &relations, &authz, &target_props);

        // ── 2. Run the read-time rollups, measuring the recompute latency. ──────────────────────────
        let db_id = format!("db:{}", tenant.0);
        for field in ["total", "n", "hi"] {
            let fid = FieldId::new(field);
            let start = Instant::now();
            let out = compute_row(&viewer, &src, &fid, &BTreeMap::new(), &schema, &resolver);
            let elapsed = start.elapsed();
            per_read_ms.push(elapsed.as_secs_f64() * 1000.0);
            tel.record(&db_id, &fid, elapsed);

            // 0 rollup leak: the aggregate reflects ONLY the granted half — never the hidden amounts.
            match field {
                "total" => {
                    assert_eq!(out, CellValue::Int(expected_sum), "SUM reflects only the granted half (0 rollup leak)");
                    if let CellValue::Int(v) = out {
                        // If a hidden target leaked, the value would be far larger than the visible sum.
                        total_leaked_value += v - expected_sum;
                    }
                }
                "n" => assert_eq!(out, CellValue::Int(expected_count), "COUNT = the granted half (0 rollup leak)"),
                "hi" => {
                    assert_eq!(out, CellValue::Int(expected_max), "MAX = the visible max, NOT the hidden 1M+ (0 value-disclosure leak)");
                    assert!(hidden_max > expected_max, "the drill's hidden targets are genuinely higher (a real leak witness)");
                }
                _ => unreachable!(),
            }
        }
    }

    // ── 3. THE GATE. ─────────────────────────────────────────────────────────────────────────────
    // (a) read-time rollup p99 within budget (read from the file).
    per_read_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = per_read_ms[((per_read_ms.len() as f64 * 0.99).ceil() as usize - 1).min(per_read_ms.len() - 1)];
    assert!(
        p99 <= budget_ms as f64,
        "KN-D10: read-time rollup recompute p99 {p99:.3} ms must be within the {budget_ms} ms budget (from thresholds.toml)"
    );

    // (b) 0 rollup leak across the row-restricted related set (no hidden target contributed).
    assert_eq!(total_leaked_value, 0, "KN-D10 / KN-D5: 0 rollup leak across the whole multi-tenant scale set");

    // (c) the materialisation trigger is MEASURED (not acted on): no within-budget rollup is a
    //     candidate; the telemetry reports the per-rollup p99 so KN-P31 can act. To PROVE the
    //     trigger fires, inject a synthetic over-budget sample on a separate db (a measured-slow
    //     rollup) — the candidate list reports it (the db + field + measured p99 for KN-P31).
    let live_candidates = tel.materialisation_candidates(budget_ms);
    assert!(
        live_candidates.is_empty(),
        "the live scale rollups are within budget — none is a materialisation candidate yet: {:?}",
        live_candidates.iter().map(|c| (&c.db_id, c.field.to_string(), c.measured_p99_ms)).collect::<Vec<_>>()
    );
    // The measured-slow case (the §4.2 / KQ-4 promotion trigger): a rollup whose read-time recompute
    // p99 crosses the budget is flagged for KN-P31's materialised-aggregate promotion.
    let slow_field = FieldId::new("slow_rollup");
    for _ in 0..100 {
        tel.record("db:measured-slow", &slow_field, Duration::from_millis(budget_ms + 200));
    }
    let candidates = tel.materialisation_candidates(budget_ms);
    let slow = candidates
        .iter()
        .find(|c| c.field == slow_field)
        .expect("a rollup whose read-time recompute p99 crosses the budget is a materialisation candidate (KN-P31)");
    assert_eq!(slow.db_id, "db:measured-slow");
    assert!(slow.measured_p99_ms > budget_ms, "the hint carries the measured p99 that crossed the budget (the KN-P31 trigger)");

    println!(
        "[P-308 KN-D10 GREEN] read-time rollup engine at scale ({} tenants × {} related rows): \
         the permission-filtered SUM/COUNT/MAX recompute p99 {:.3} ms within the {} ms budget \
         (thresholds.toml); 0 rollup leak across the row-restricted multi-tenant related set \
         (list_objects conjoined, composes KN-D5 — the hidden 1M+ targets never summed/counted/maxed); \
         the materialisation trigger MEASURED — the live rollups are within budget (no premature \
         promotion), a measured-slow rollup (p99 {} ms) is flagged for KN-P31's per-rollup \
         materialised aggregate. Rollups are computed at READ TIME, never stored (KN-3).",
        TENANTS, TARGETS_PER_SOURCE, p99, budget_ms, slow.measured_p99_ms
    );
}
