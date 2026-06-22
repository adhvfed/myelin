//! # KN-D9 — the flexible-database latency + facet-promotion-trigger drill (KN-P17 / P-307, M3)
//!
//! **Drill catalogue (testing-strategy/01-…-catalogue.md, row KN-D9):** "Filter/sort/group a large
//! multi-tenant database (JSONB + projection + `SetExpr` conjoin) → read-time p99 within budget;
//! measure the >5% facet-promotion trigger. — db-query p99; facet frequency — SCHED."
//!
//! This is the named SCHED drill in the master M3 gate (roadmap §3, KN-M3d). It builds a LARGE
//! multi-tenant flexible database (many rows across several tenants/collections), runs the §4.1
//! `VIEW_QUERY` path — the `QueryAst` filter lowered into JSONB ops over `props`, sorted/grouped,
//! with the `list_objects` `SetExpr` ACL conjoined (permission by construction) — and asserts:
//!
//! - **read-time p99 within budget** — the modelled per-view read cost stays under the
//!   `flex_db.view_read_p99_max_ms` budget READ FROM THE THRESHOLDS FILE (never a hardcoded magic
//!   number). On this floor the cost is the in-memory mirror of the JSONB scan + the ACL conjoin
//!   over the page; the LIVE p99-against-Postgres-at-scale is the `--features integration` proof +
//!   the world-scale re-confirm (KN-P31). The drill GATES that the read is bounded (paginated,
//!   row-capped) and the cost stays within budget on the modelled scale;
//! - **0 leak across a row-restricted db** — the `SetExpr` ACL conjoined into the view means a row
//!   the viewer cannot read is ABSENT from the view AND uncounted by the COUNT (composing with
//!   KN-D5) — measured to be exactly 0 leaked rows over the whole scale set;
//! - **the >5% facet-promotion trigger is MEASURED** — the per-facet view-execution frequency
//!   telemetry reports which facets cross the frozen `> 5%` ratio (read from the file); the
//!   promotion ACT (the generated-column index) is KN-P31 (M5) — here it is measured, not acted on.
//!
//! The budget + the ratio are read from `myelin_substrate::Thresholds` (the single source of truth);
//! a red is a dated `[[claimed_not_proven]]` scorecard row, never a weakened threshold (EI-01 §3).

use std::collections::BTreeMap;
use std::time::Instant;

use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName};
use myelin_knowledge::{
    db_row_id_colref, execute_view_count, execute_view_query, lower_over_db_row_id,
    row_matches_filter, AuthzVisibleIndex, DbRow, FacetTelemetry, FieldDef, FieldSchema, PageBound,
    PropertyBag,
};
use myelin_query::{
    CmpOp, Expr, FieldId, FieldType, FieldValue, OrderKey, Predicate, QueryAst, SortDir, SortSpec,
    ViewKind, ViewSpec,
};
use myelin_substrate::Thresholds;
use myelin_tenancy::{Region, TenantId};

/// The scale knobs (a "large multi-tenant database" on the deterministic CI/SCHED harness — large
/// enough to exercise the filter/sort/group + the ACL conjoin + the facet-frequency window, small
/// enough to stay a single-process drill; the world-scale re-run is KN-P31).
const TENANTS: usize = 8;
const ROWS_PER_DB: usize = 2_000;

fn p(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

/// The collection schema: a Select `status`, an Int `priority`, a Principal `assignee` (PII), a
/// Date `due`, a Text `title`, and a Text `notes` (a deliberately cold facet).
fn schema() -> FieldSchema {
    FieldSchema::of([
        FieldDef::new("status", FieldType::Select),
        FieldDef::new("priority", FieldType::Int),
        FieldDef::personal("assignee", FieldType::Principal),
        FieldDef::new("due", FieldType::Date),
        FieldDef::new("title", FieldType::Text),
        FieldDef::new("notes", FieldType::Text),
    ])
    .unwrap()
}

/// Build one row's property bag deterministically from its index (so the drill is reproducible).
fn row_props(n: usize) -> PropertyBag {
    let mut props: PropertyBag = BTreeMap::new();
    let status = if n.is_multiple_of(3) {
        "open"
    } else {
        "closed"
    };
    props.insert(FieldId::new("status"), FieldValue::Select(status.into()));
    props.insert(FieldId::new("priority"), FieldValue::Int((n % 5) as i64));
    props.insert(
        FieldId::new("assignee"),
        FieldValue::Principal(format!("p:{}", n % 7)),
    );
    props.insert(
        FieldId::new("due"),
        FieldValue::Date(format!("2026-06-{:02}", (n % 28) + 1)),
    );
    props.insert(FieldId::new("title"), FieldValue::Text(format!("Item {n}")));
    props
}

/// The "filter by status, sort by priority, group by status" view (the KN-D9 filter/sort/group).
fn status_open_view() -> ViewSpec {
    ViewSpec {
        kind: ViewKind::Board,
        filter: QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("status".into()),
            rhs: Expr::Lit(myelin_identity::Literal::Str("open".into())),
        })
        .unwrap(),
        group_by: Some(FieldId::new("status")),
        sort: vec![SortSpec {
            field: FieldId::new("priority"),
            dir: SortDir::Desc,
        }],
        visible: vec![FieldId::new("title"), FieldId::new("assignee")],
        order_field: FieldId::new("order_key"),
    }
}

#[test]
fn kn_d9_flex_db_filter_sort_group_within_budget_zero_leak_promotion_trigger_measured() {
    // ── 0. Read the budget + the >5% ratio from the canonical thresholds file (NOT a hardcoded
    //       magic number — the thresholds-file discipline, EI-01 §3). ──────────────────────────────
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file must load");
    let budget_ms = thresholds.flex_db.view_read_p99_max_ms;
    let ratio = thresholds.flex_db.facet_promotion_ratio;
    assert_eq!(
        ratio, 0.05,
        "the frozen 6.3 >5% trigger is read from the file (never weakened)"
    );
    assert!(
        thresholds.flex_db.page_row_cap >= PageBound::MAX,
        "the file's row cap is at least the PageBound::MAX seed (a view read is always row-capped)"
    );

    let schema = schema();
    let view = status_open_view();
    let tel = FacetTelemetry::new();
    let av = AuthzVisibleIndex::new();
    let region = Region::new("fr-par");

    // ── 1. Build a LARGE multi-tenant database: TENANTS tenants × ROWS_PER_DB rows each, every row's
    //       property bag schema-validated (the typed FieldType gate holds at scale). Grant the viewer
    //       read of exactly the FIRST HALF of each tenant's rows (a row-restricted db). ─────────────
    let mut rows_by_tenant: Vec<(TenantId, Vec<DbRow>)> = Vec::new();
    for t in 0..TENANTS {
        let tenant = TenantId(format!("tenant-{t}"));
        let viewer = p("p:0", tenant.0.as_str());
        let mut rows = Vec::new();
        for n in 0..ROWS_PER_DB {
            let props = row_props(n);
            schema
                .validate_props(&props)
                .expect("every row is schema-valid (typed FieldType gate)");
            let id = format!("row:{t}:{n}");
            let row = DbRow::new(id.clone(), props, OrderKey::parse("U").unwrap());
            // Grant read of the first half only — the SECOND half is the leak witness.
            if n < ROWS_PER_DB / 2 {
                av.grant(
                    &tenant,
                    &region,
                    &viewer.principal_id.0,
                    "read",
                    &id,
                    "zk-0000000001",
                );
            }
            rows.push(row);
        }
        rows_by_tenant.push((tenant, rows));
    }

    // ── 2. Run the VIEW_QUERY over EACH tenant's db, measuring the modelled read cost + leak count +
    //       feeding the facet-frequency telemetry. The cost is the in-memory mirror of the JSONB
    //       filter scan + the ACL conjoin over the page (the production path is Postgres; the live
    //       p99 is the integration proof). ─────────────────────────────────────────────────────────
    let acl_set = myelin_identity::SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: db_row_id_colref(),
    };
    let mut per_read_ms: Vec<f64> = Vec::new();
    let mut total_leaked = 0usize;

    for (tenant, rows) in &rows_by_tenant {
        let viewer = p("p:0", tenant.0.as_str());
        let db_id = format!("db:{}", tenant.0);

        // The composed VIEW_QUERY is ONE statement (no N+1) — the §4.1 guarantee.
        let q = execute_view_query(
            &view,
            &acl_set,
            &viewer,
            tenant,
            &db_id,
            &[],
            PageBound::DEFAULT,
        )
        .expect("the VIEW_QUERY composes");
        assert_eq!(
            q.statement_count(),
            1,
            "the VIEW_QUERY is ONE query (no N+1): {}",
            q.sql
        );

        // Measure the modelled read: filter (the bounded QueryAst interpreter over each row's props)
        // AND the ACL conjoin (the in-memory mirror of the authz_visible JOIN) over the page.
        let lowered_acl = lower_over_db_row_id(&acl_set, &viewer);
        let start = Instant::now();

        // Filter the rows by the view filter (the JSONB filter mirror) …
        let mut matched: Vec<&DbRow> = rows
            .iter()
            .filter(|r| row_matches_filter(&view.filter, &r.props).unwrap_or(false))
            .collect();
        // … conjoin the ACL (the row-restricted db — only the granted half survives) …
        let candidate_ids: Vec<&str> = matched.iter().map(|r| r.row_id.as_str()).collect();
        let visible = av.evaluate(tenant, &region, &viewer, &lowered_acl, &candidate_ids);
        matched.retain(|r| visible.iter().any(|v| v == &r.row_id));
        // … sort by priority DESC then the order_key tiebreak (the view sort) …
        let prio = |r: &DbRow| match r.props.get(&FieldId::new("priority")) {
            Some(FieldValue::Int(n)) => *n,
            _ => i64::MIN,
        };
        matched.sort_by(|a, b| prio(b).cmp(&prio(a)).then(a.order_key.cmp(&b.order_key)));
        // … and page (row-capped — the §4.1 step-5 bound).
        let page: Vec<&&DbRow> = matched
            .iter()
            .take(PageBound::DEFAULT.limit as usize)
            .collect();
        let elapsed = start.elapsed();
        per_read_ms.push(elapsed.as_secs_f64() * 1000.0);

        // 0 leak: every row in the page is one the viewer was GRANTED (the second half is absent).
        for r in &page {
            let n: usize = r.row_id.rsplit(':').next().unwrap().parse().unwrap();
            if n >= ROWS_PER_DB / 2 {
                total_leaked += 1;
            }
            assert_eq!(
                r.props.get(&FieldId::new("status")),
                Some(&FieldValue::Select("open".into())),
                "every page row matches the filter (status == open)"
            );
        }

        // Feed the facet-frequency telemetry: this execution referenced `status` (the filter facet).
        let facets: Vec<FieldId> = q.facet_paths.keys().cloned().collect();
        tel.record_execution(&db_id, &facets);

        // Run a permission-correct COUNT too (the KN-D5 count-leak-closed shape) — one statement.
        let count_q =
            execute_view_count(&view, &acl_set, &viewer, tenant, &db_id, &[]).expect("count");
        assert_eq!(
            count_q.statement_count(),
            1,
            "the COUNT is one aggregate query"
        );
        assert!(count_q.is_count);
    }

    // ── 3. Drive a SECOND view that references `priority` heavily on ONE collection so its facet
    //       frequency crosses the >5% trigger (the measured-promotion case). 30 executions of a
    //       priority-filtered view on tenant-0's db: priority is then in 30/(30+1) > 5% of executions. ─
    let priority_view = ViewSpec {
        kind: ViewKind::Table,
        filter: QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Ge,
            lhs: Expr::Var("priority".into()),
            rhs: Expr::Lit(myelin_identity::Literal::Int(3)),
        })
        .unwrap(),
        group_by: None,
        sort: vec![],
        visible: vec![FieldId::new("title")],
        order_field: FieldId::new("order_key"),
    };
    let hot_db = "db:tenant-0";
    for _ in 0..30 {
        let q = execute_view_query(
            &priority_view,
            &acl_set,
            &p("p:0", "tenant-0"),
            &TenantId("tenant-0".into()),
            "db:tenant-0",
            &[],
            PageBound::DEFAULT,
        )
        .unwrap();
        let facets: Vec<FieldId> = q.facet_paths.keys().cloned().collect();
        tel.record_execution(hot_db, &facets);
    }

    // ── 4. THE GATE. ─────────────────────────────────────────────────────────────────────────────
    // (a) read-time p99 within budget (read from the file).
    per_read_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = per_read_ms
        [((per_read_ms.len() as f64 * 0.99).ceil() as usize - 1).min(per_read_ms.len() - 1)];
    assert!(
        p99 <= budget_ms as f64,
        "KN-D9: flex-DB filter/sort/group read p99 {p99:.3} ms must be within the {budget_ms} ms budget (from thresholds.toml)"
    );

    // (b) 0 leak across the row-restricted db (the SetExpr conjoined — no forbidden row in any view).
    assert_eq!(
        total_leaked, 0,
        "KN-D9 / KN-D5: 0 leaked rows across the whole multi-tenant scale set"
    );

    // (c) the >5% facet-promotion trigger is MEASURED (not acted on): `priority` on the hot db
    //     crossed the frozen ratio; the candidate list reports it (the type + PII flag for KN-P31).
    let status_freq = tel.facet_frequency(hot_db, &FieldId::new("status"));
    let priority_freq = tel.facet_frequency(hot_db, &FieldId::new("priority"));
    assert!(
        priority_freq > ratio,
        "priority frequency {priority_freq:.3} crossed the >5% trigger"
    );
    assert!(
        status_freq <= ratio,
        "status frequency {status_freq:.3} did NOT cross it (1 of 31 executions)"
    );
    let candidates: Vec<String> = tel
        .promotion_candidates(hot_db, &schema)
        .into_iter()
        .map(|h| h.field_id.to_string())
        .collect();
    assert!(
        candidates.contains(&"priority".to_string()),
        "the >5% facet `priority` is a promotion candidate (acted on in KN-P31): {candidates:?}"
    );
    assert!(
        !candidates.contains(&"status".to_string()),
        "the cold facet is NOT a candidate"
    );

    println!(
        "[P-307 KN-D9 GREEN] flexible DB at scale ({} tenants × {} rows): filter/sort/group VIEW_QUERY \
         read p99 {:.3} ms within the {} ms budget (thresholds.toml); 0 leaked rows across the \
         row-restricted multi-tenant set (SetExpr conjoined, composes KN-D5); the >5% facet-promotion \
         trigger MEASURED — `priority` (freq {:.3}) is a promotion candidate (acted on in KN-P31), \
         the cold `status` (freq {:.3}) is not. Each VIEW_QUERY + COUNT is ONE statement (no N+1).",
        TENANTS, ROWS_PER_DB, p99, budget_ms, priority_freq, status_freq
    );
}
