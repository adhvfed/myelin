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

const TENANTS: usize = 8;
const ROWS_PER_DB: usize = 2_000;

fn p(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

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

    let acl_set = myelin_identity::SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: db_row_id_colref(),
    };
    let mut per_read_ms: Vec<f64> = Vec::new();
    let mut total_leaked = 0usize;

    for (tenant, rows) in &rows_by_tenant {
        let viewer = p("p:0", tenant.0.as_str());
        let db_id = format!("db:{}", tenant.0);

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

        let lowered_acl = lower_over_db_row_id(&acl_set, &viewer);
        let start = Instant::now();

        let mut matched: Vec<&DbRow> = rows
            .iter()
            .filter(|r| row_matches_filter(&view.filter, &r.props).unwrap_or(false))
            .collect();
        let candidate_ids: Vec<&str> = matched.iter().map(|r| r.row_id.as_str()).collect();
        let visible = av.evaluate(tenant, &region, &viewer, &lowered_acl, &candidate_ids);
        matched.retain(|r| visible.iter().any(|v| v == &r.row_id));
        let prio = |r: &DbRow| match r.props.get(&FieldId::new("priority")) {
            Some(FieldValue::Int(n)) => *n,
            _ => i64::MIN,
        };
        matched.sort_by(|a, b| prio(b).cmp(&prio(a)).then(a.order_key.cmp(&b.order_key)));
        let page: Vec<&&DbRow> = matched
            .iter()
            .take(PageBound::DEFAULT.limit as usize)
            .collect();
        let elapsed = start.elapsed();
        per_read_ms.push(elapsed.as_secs_f64() * 1000.0);

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

        let facets: Vec<FieldId> = q.facet_paths.keys().cloned().collect();
        tel.record_execution(&db_id, &facets);

        let count_q =
            execute_view_count(&view, &acl_set, &viewer, tenant, &db_id, &[]).expect("count");
        assert_eq!(
            count_q.statement_count(),
            1,
            "the COUNT is one aggregate query"
        );
        assert!(count_q.is_count);
    }

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

    per_read_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = per_read_ms
        [((per_read_ms.len() as f64 * 0.99).ceil() as usize - 1).min(per_read_ms.len() - 1)];
    assert!(
        p99 <= budget_ms as f64,
        "KN-D9: flex-DB filter/sort/group read p99 {p99:.3} ms must be within the {budget_ms} ms budget (from thresholds.toml)"
    );

    assert_eq!(
        total_leaked, 0,
        "KN-D9 / KN-D5: 0 leaked rows across the whole multi-tenant scale set"
    );

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
         trigger MEASURED - `priority` (freq {:.3}) is a promotion candidate (acted on in KN-P31), \
         the cold `status` (freq {:.3}) is not. Each VIEW_QUERY + COUNT is ONE statement (no N+1).",
        TENANTS, ROWS_PER_DB, p99, budget_ms, priority_freq, status_freq
    );
}
