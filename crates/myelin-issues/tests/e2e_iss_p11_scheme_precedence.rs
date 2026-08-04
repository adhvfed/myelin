use myelin_issues::{
    add_flexible_field, FlexibleField, Reassignment, ResolveContext, Scheme, SchemeAssignment,
    SchemeKind, SchemeResolver,
};
use myelin_query::FieldType;

struct IssueStore {
    rows: Vec<u128>,
    rows_touched: u64,
}

impl IssueStore {
    fn seeded(n: u128) -> Self {
        IssueStore {
            rows: (0..n).collect(),
            rows_touched: 0,
        }
    }

    fn observe_reassignment(&mut self, r: Reassignment) {
        self.rows_touched += r.issue_rows_touched;
    }
}

#[test]
fn chained_reassignment_touches_zero_issue_rows() {
    let mut store = IssueStore::seeded(10_000);
    assert_eq!(
        store.rows.len(),
        10_000,
        "the store has issues that a migration WOULD touch"
    );

    let mut resolver = SchemeResolver::linear_simple();
    let ctx = ResolveContext {
        type_id: 100,
        project_id: 200,
        team_id: 300,
    };

    let default_workflow = resolver.resolve(SchemeKind::Workflow, ctx);
    assert_eq!(
        default_workflow,
        myelin_issues::org_default_scheme_id(SchemeKind::Workflow),
        "a zero-config org resolves to org_default (Linear-simple)"
    );
    assert_eq!(resolver.assignment_count(), 0);

    let r1 = resolver.reassign(SchemeAssignment {
        kind: SchemeKind::Workflow,
        type_id: None,
        project_id: None,
        team_id: None,
        scheme_id: 50_000,
    });
    store.observe_reassignment(r1);
    assert_eq!(
        r1.issue_rows_touched, 0,
        "the org-wide assignment migrated no data"
    );
    assert_eq!(
        resolver.resolve(SchemeKind::Workflow, ctx),
        50_000,
        "the org override is in effect"
    );

    let r2 = resolver.reassign(SchemeAssignment {
        kind: SchemeKind::Workflow,
        type_id: None,
        project_id: Some(200),
        team_id: None,
        scheme_id: 50_001,
    });
    store.observe_reassignment(r2);
    assert_eq!(
        r2.issue_rows_touched, 0,
        "the project reassignment migrated no data"
    );
    assert_eq!(
        resolver.resolve(SchemeKind::Workflow, ctx),
        50_001,
        "the project-scoped scheme wins (most-specific) - a config change, not a migration"
    );

    assert_eq!(
        store.rows_touched, 0,
        "the chained reassignment touched 0 issue rows (the no-config = Linear-simple gate)"
    );
    assert_eq!(
        store.rows.len(),
        10_000,
        "every issue row is untouched (the store is unmigrated)"
    );
}

#[test]
fn cdc_scheme_config_shape_round_trips() {
    let workflow = Scheme {
        scheme_id: 1,
        kind: SchemeKind::Workflow,
        name: "Engineering".into(),
        body: serde_json::json!({
            "states": [
                {"name": "Todo", "category": "unstarted"},
                {"name": "In Progress", "category": "started"},
                {"name": "Done", "category": "completed"}
            ],
            "transitions": [
                {"from": "Todo", "to": "In Progress", "guard": null, "post_actions": []}
            ]
        }),
        version: 1,
    };

    let json = serde_json::to_string(&workflow).expect("the scheme config serializes");
    let back: Scheme = serde_json::from_str(&json).expect("the scheme config parses back");
    assert_eq!(
        back, workflow,
        "the workflow config shape round-trips byte-identically"
    );

    assert!(
        json.contains("\"workflow\""),
        "the kind serializes to its frozen CHECK token"
    );
    assert_eq!(back.kind.wire_token(), "workflow");

    for kind in SchemeKind::all() {
        let s = Scheme {
            scheme_id: 7,
            kind,
            name: format!("{}-scheme", kind.wire_token()),
            body: serde_json::json!({ "kind_body": kind.wire_token() }),
            version: 3,
        };
        let j = serde_json::to_string(&s).expect("serializes");
        let b: Scheme = serde_json::from_str(&j).expect("parses");
        assert_eq!(b, s, "the {kind:?} config shape round-trips");
    }
}

#[test]
fn cdc_flexible_field_shape_is_zero_ddl_and_round_trips() {
    let field = FlexibleField::define("customer_tier", FieldType::Select, "Customer Tier");
    let json = serde_json::to_string(&field).expect("the field def serializes");
    let back: FlexibleField = serde_json::from_str(&json).expect("the field def parses back");
    assert_eq!(back, field, "the flexible-field config shape round-trips");

    let write = add_flexible_field(&field, serde_json::json!("enterprise"));
    assert_eq!(
        write.ddl_statements, 0,
        "a custom-field value write is zero-DDL (design rule 2)"
    );
    assert!(
        write.gin_indexable,
        "the value is immediately filterable over the default issue_props_gin"
    );
}
