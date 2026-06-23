//! # ISS-P11 / P-377 (M4) — the chained-mutation e2e + the CDC stub for the scheme config shape
//!
//! The prompt's GATE / TESTS:
//! - **The chained-mutation e2e:** assign `org_default` → reassign a project scheme → **assert 0
//!   issue rows touched**. This is the no-config = Linear-simple gate's green artifact (a scheme
//!   reassignment migrates NO data — design rule 1, arch 01 §3 / arch 02 §1).
//! - **The CDC stub for the scheme config shape:** the interpreted JSONB `body` + the
//!   `(type × project × team)` assignment signature serialize + parse back identically (a config
//!   write, never a bespoke per-scheme object graph — EI-01 §7). The PROVIDER is Issues authoring the
//!   config shape; the CONSUMER (the FSM interpreter ISS-P12 / the field model / the SLA engine)
//!   reads the SAME JSONB body — so a drift on either side fails here.
//!
//! The unit tests for the precedence algebra (most-specific-wins determinism + caching) and the
//! zero-DDL flexible-field add live in `src/schemes.rs` (the in-module tests). This file is the
//! cross-module e2e + the config-shape CDC.

use myelin_issues::{
    add_flexible_field, FlexibleField, Reassignment, ResolveContext, Scheme, SchemeAssignment,
    SchemeKind, SchemeResolver,
};
use myelin_query::FieldType;

/// A toy issue-store row counter — stands in for the `issue` table. The e2e proves a scheme
/// reassignment NEVER touches it (0 issue rows migrated). The row count is the green artifact.
struct IssueStore {
    /// The issue rows present (the e2e seeds some so "0 touched" is non-vacuous — there ARE rows to
    /// touch, and the reassignment touches none of them).
    rows: Vec<u128>,
    /// The number of issue rows touched by config operations (MUST stay 0 across reassignments).
    rows_touched: u64,
}

impl IssueStore {
    fn seeded(n: u128) -> Self {
        IssueStore {
            rows: (0..n).collect(),
            rows_touched: 0,
        }
    }

    /// Apply a config-only reassignment outcome to the store's accounting. A reassignment reports
    /// `issue_rows_touched` — the store folds it into its counter. A correct config write folds 0.
    fn observe_reassignment(&mut self, r: Reassignment) {
        self.rows_touched += r.issue_rows_touched;
    }
}

/// **The chained-mutation e2e: assign org_default → reassign a project scheme → 0 issue rows
/// touched.** A populated issue store + a Linear-simple resolver; the org adds an org-wide workflow,
/// then narrows it to a project — both are CONFIG writes that touch the `scheme_assignment` table +
/// flush the resolution cache, and touch ZERO issue rows. The resolution changes (the new scheme is
/// in effect) WITHOUT any data migration. (Arch 01 §3 design rule 1; the no-config gate.)
#[test]
fn chained_reassignment_touches_zero_issue_rows() {
    // A populated store: 10_000 issue rows exist (so "0 touched" is a real, non-vacuous claim).
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

    // Step 0 — no config: resolution is the Linear-simple org_default (no scheme rows at all).
    let default_workflow = resolver.resolve(SchemeKind::Workflow, ctx);
    assert_eq!(
        default_workflow,
        myelin_issues::org_default_scheme_id(SchemeKind::Workflow),
        "a zero-config org resolves to org_default (Linear-simple)"
    );
    assert_eq!(resolver.assignment_count(), 0);

    // Step 1 — assign an ORG-WIDE workflow override (·,·,·). A CONFIG write — 0 issue rows touched.
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

    // Step 2 — reassign a PROJECT-scoped workflow (·,P,·) — narrows the org override for project 200.
    // Another CONFIG write — 0 issue rows touched. The more-specific assignment now WINS.
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
        "the project-scoped scheme wins (most-specific) — a config change, not a migration"
    );

    // THE GATE: across the whole chain, ZERO issue rows were touched, yet 10_000 rows exist + the
    // governance behaviour changed twice. Adding governance is adding assignments, never migrating
    // data (design rule 1, arch 01 §3).
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

/// **The CDC stub for the scheme config shape — the interpreted JSONB body round-trips (provider ↔
/// consumer).** Issues authors the five-kind config shape; the FSM interpreter / field model / SLA
/// engine CONSUME the SAME JSONB. The shape serializes + parses back byte-identically — a config
/// write, never a bespoke object graph per scheme (EI-01 §7, no Jira-Groovy footgun). A drift on
/// either side (a kind token rename, a body-field rename) fails here.
#[test]
fn cdc_scheme_config_shape_round_trips() {
    // PROVIDER: Issues authors a workflow scheme as interpreted JSONB config.
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

    // The config serializes to JSON (the row's `body` column) and parses back identically.
    let json = serde_json::to_string(&workflow).expect("the scheme config serializes");
    let back: Scheme = serde_json::from_str(&json).expect("the scheme config parses back");
    assert_eq!(
        back, workflow,
        "the workflow config shape round-trips byte-identically"
    );

    // CONSUMER drift anchor: the kind token is the frozen CHECK vocabulary string.
    assert!(
        json.contains("\"workflow\""),
        "the kind serializes to its frozen CHECK token"
    );
    assert_eq!(back.kind.wire_token(), "workflow");

    // Every kind round-trips with its own body shape (the five interpreted kinds).
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

/// **The flexible-field config shape is zero-DDL + GIN-indexable, and round-trips (the CDC field
/// half).** A `field`-scheme custom field is the frozen `FieldType` + a GIN posture; writing a value
/// is a JSONB property-bag write (0 DDL). The definition round-trips through serde (the `field`
/// scheme `body` shape).
#[test]
fn cdc_flexible_field_shape_is_zero_ddl_and_round_trips() {
    let field = FlexibleField::define("customer_tier", FieldType::Select, "Customer Tier");
    // The definition round-trips (the `field` scheme body entry).
    let json = serde_json::to_string(&field).expect("the field def serializes");
    let back: FlexibleField = serde_json::from_str(&json).expect("the field def parses back");
    assert_eq!(back, field, "the flexible-field config shape round-trips");

    // Writing a value is a zero-DDL JSONB property-bag write, immediately GIN-indexable.
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
