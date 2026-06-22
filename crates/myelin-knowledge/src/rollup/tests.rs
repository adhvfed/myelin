//! Unit tests for the read-time formula/rollup engine (KN-P18 / P-308, §4.2):
//! - each [`RollupFn`] (COUNT/SUM/MIN/MAX/AVG) over a permission-filtered related set;
//! - the bounded [`FormulaExpr`] arithmetic evaluator (Int-only; #ERROR fail-closed);
//! - the depth-bound + cycle detection → [`CellValue::Cycle`] (`#CYCLE`, never a loop);
//! - the permission-filtered rollup conjoin (0 rollup leak — a restricted target is uncounted);
//! - the dependency-graph ordering (a formula referencing a rollup referencing a property);
//! - the static cost-bound rejection (an over-budget / over-deep formula is refused at build).

use super::*;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{FieldId, FieldValue};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use crate::database::{DbRelation, RelationKind, RelationStore};

fn viewer(id: &str, tenant: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId(tenant.into()))
}

fn fid(s: &str) -> FieldId {
    FieldId::new(s)
}

/// A test fixture: a source row with N rollup_source edges to target rows carrying an Int `amount`,
/// the viewer granted read of a subset (the rest are the leak witnesses).
struct Fixture {
    tenant: TenantId,
    region: Region,
    relations: RelationStore,
    authz: AuthzVisibleIndex,
    target_props: BTreeMap<String, PropertyBag>,
}

impl Fixture {
    /// Build a fixture: `src` related to `targets` (id, amount); `granted` is the subset the viewer
    /// may read (the rest are present in the relation but ACL-hidden — the leak witnesses).
    fn build(src: &str, targets: &[(&str, i64)], granted: &[&str]) -> Fixture {
        let tenant = TenantId("acme".into());
        let region = Region::new("fr-par");
        let relations = RelationStore::new();
        let authz = AuthzVisibleIndex::new();
        let mut target_props: BTreeMap<String, PropertyBag> = BTreeMap::new();
        let v = viewer("p:viewer", "acme");
        for (i, (id, amount)) in targets.iter().enumerate() {
            relations.relate(
                &tenant,
                DbRelation {
                    relation_id: format!("rel:{i}"),
                    src_row: src.to_string(),
                    dst_ref: ArtifactRef((*id).to_string()),
                    rel: RelationKind::RollupSource,
                },
            );
            let mut props: PropertyBag = BTreeMap::new();
            props.insert(fid("amount"), FieldValue::Int(*amount));
            target_props.insert((*id).to_string(), props);
            if granted.contains(id) {
                authz.grant(&tenant, &region, &v.principal_id.0, "read", id, "zk-0000000001");
            }
        }
        Fixture { tenant, region, relations, authz, target_props }
    }

    fn resolver(&self) -> RollupResolver<'_> {
        RollupResolver::new(&self.tenant, &self.region, &self.relations, &self.authz, &self.target_props)
    }
}

// ───────────────────────────── each RollupFn ──────────────────────────────

#[test]
fn rollup_count_over_visible_related_rows() {
    // 4 targets, 3 granted: COUNT == 3 (the hidden 4th is uncounted).
    let fx = Fixture::build("src:1", &[("t:1", 10), ("t:2", 20), ("t:3", 30), ("t:secret", 99)], &["t:1", "t:2", "t:3"]);
    let schema = FormulaSchema::of([FormulaField {
        field: fid("related_count"),
        expr: FormulaExpr::Rollup { func: RollupFn::Count, target: fid("amount") },
    }])
    .unwrap();
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let out = compute_row(&v, "src:1", &fid("related_count"), &BTreeMap::new(), &schema, &r);
    assert_eq!(out, CellValue::Int(3), "COUNT over the 3 VISIBLE related rows (the secret 4th uncounted)");
}

#[test]
fn rollup_sum_min_max_avg_over_visible_numeric_targets() {
    let fx = Fixture::build("src:1", &[("t:1", 10), ("t:2", 20), ("t:3", 30)], &["t:1", "t:2", "t:3"]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let mk = |func: RollupFn| {
        let schema = FormulaSchema::of([FormulaField {
            field: fid("agg"),
            expr: FormulaExpr::Rollup { func, target: fid("amount") },
        }])
        .unwrap();
        compute_row(&v, "src:1", &fid("agg"), &BTreeMap::new(), &schema, &r)
    };
    assert_eq!(mk(RollupFn::Sum), CellValue::Int(60), "SUM = 10+20+30");
    assert_eq!(mk(RollupFn::Min), CellValue::Int(10), "MIN");
    assert_eq!(mk(RollupFn::Max), CellValue::Int(30), "MAX");
    assert_eq!(mk(RollupFn::Avg), CellValue::Int(20), "AVG floor = 60/3");
}

#[test]
fn rollup_min_max_over_empty_visible_set_is_empty_diagnostic() {
    // All targets hidden → the visible set is empty: MIN/MAX → #EMPTY; SUM/COUNT/AVG → 0.
    let fx = Fixture::build("src:1", &[("t:1", 10), ("t:2", 20)], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let mk = |func: RollupFn| {
        let schema = FormulaSchema::of([FormulaField {
            field: fid("agg"),
            expr: FormulaExpr::Rollup { func, target: fid("amount") },
        }])
        .unwrap();
        compute_row(&v, "src:1", &fid("agg"), &BTreeMap::new(), &schema, &r)
    };
    assert_eq!(mk(RollupFn::Min), CellValue::Empty, "MIN over empty → #EMPTY (never a panic)");
    assert_eq!(mk(RollupFn::Max), CellValue::Empty, "MAX over empty → #EMPTY");
    assert_eq!(mk(RollupFn::Sum), CellValue::Int(0), "SUM over empty → 0");
    assert_eq!(mk(RollupFn::Count), CellValue::Int(0), "COUNT over empty → 0");
    assert_eq!(mk(RollupFn::Avg), CellValue::Int(0), "AVG over empty → 0 (no divide-by-zero)");
}

// ───────────────────────── the permission-filtered conjoin (0 rollup leak) ────────────────────────

#[test]
fn rollup_permission_filtered_restricted_target_never_summed_or_counted() {
    // A restricted target (amount 1000) must NOT contribute to the SUM/COUNT for the unauthorized
    // viewer — the 0-rollup-leak gate (composing with KN-D5).
    let fx = Fixture::build("src:1", &[("t:1", 5), ("t:2", 5), ("t:secret", 1000)], &["t:1", "t:2"]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let sum_schema = FormulaSchema::of([FormulaField {
        field: fid("total"),
        expr: FormulaExpr::Rollup { func: RollupFn::Sum, target: fid("amount") },
    }])
    .unwrap();
    let count_schema = FormulaSchema::of([FormulaField {
        field: fid("n"),
        expr: FormulaExpr::Rollup { func: RollupFn::Count, target: fid("amount") },
    }])
    .unwrap();
    let sum = compute_row(&v, "src:1", &fid("total"), &BTreeMap::new(), &sum_schema, &r);
    let count = compute_row(&v, "src:1", &fid("n"), &BTreeMap::new(), &count_schema, &r);
    assert_eq!(sum, CellValue::Int(10), "0 rollup leak: SUM = 10 (5+5), NOT 1010 — the restricted 1000 is uncounted");
    assert_eq!(count, CellValue::Int(2), "0 rollup leak: COUNT = 2, NOT 3 — the restricted row is uncounted");
}

#[test]
fn rollup_max_does_not_leak_a_restricted_higher_target() {
    // The restricted target is the MAX — proving the leak would be a value disclosure, not just a
    // count. MAX over the visible set must NOT reveal the hidden higher value.
    let fx = Fixture::build("src:1", &[("t:1", 7), ("t:2", 9), ("t:secret", 9999)], &["t:1", "t:2"]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([FormulaField {
        field: fid("hi"),
        expr: FormulaExpr::Rollup { func: RollupFn::Max, target: fid("amount") },
    }])
    .unwrap();
    let out = compute_row(&v, "src:1", &fid("hi"), &BTreeMap::new(), &schema, &r);
    assert_eq!(out, CellValue::Int(9), "0 rollup leak: MAX = 9 (visible), NOT 9999 (restricted) — no value disclosure");
}

// ───────────────────────── arithmetic + the dependency-graph ordering ─────────────────────────────

#[test]
fn formula_arithmetic_over_props_and_rollups() {
    // total = SUM(amount) + base; the dependency graph: a formula reads a rollup AND a property.
    let fx = Fixture::build("src:1", &[("t:1", 10), ("t:2", 20)], &["t:1", "t:2"]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([FormulaField {
        field: fid("total"),
        expr: FormulaExpr::Add(
            Box::new(FormulaExpr::Rollup { func: RollupFn::Sum, target: fid("amount") }),
            Box::new(FormulaExpr::Prop(fid("base"))),
        ),
    }])
    .unwrap();
    let mut props: PropertyBag = BTreeMap::new();
    props.insert(fid("base"), FieldValue::Int(100));
    let out = compute_row(&v, "src:1", &fid("total"), &props, &schema, &r);
    assert_eq!(out, CellValue::Int(130), "SUM(amount)=30 + base=100");
}

#[test]
fn formula_referencing_another_formula_resolves_in_dependency_order() {
    // c = b * 2; b = a + 1; a = Prop. The graph must resolve a → b → c.
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([
        FormulaField { field: fid("a"), expr: FormulaExpr::Prop(fid("x")) },
        FormulaField {
            field: fid("b"),
            expr: FormulaExpr::Add(Box::new(FormulaExpr::FormulaRef(fid("a"))), Box::new(FormulaExpr::Lit(Literal::Int(1)))),
        },
        FormulaField {
            field: fid("c"),
            expr: FormulaExpr::Mul(Box::new(FormulaExpr::FormulaRef(fid("b"))), Box::new(FormulaExpr::Lit(Literal::Int(2)))),
        },
    ])
    .unwrap();
    let mut props: PropertyBag = BTreeMap::new();
    props.insert(fid("x"), FieldValue::Int(4));
    let c = compute_row(&v, "src:1", &fid("c"), &props, &schema, &r);
    assert_eq!(c, CellValue::Int(10), "c = (x+1)*2 = (4+1)*2 = 10 (resolved in dependency order)");
}

#[test]
fn formula_divide_by_zero_is_error_never_panic() {
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([FormulaField {
        field: fid("q"),
        expr: FormulaExpr::Div(Box::new(FormulaExpr::Lit(Literal::Int(10))), Box::new(FormulaExpr::Lit(Literal::Int(0)))),
    }])
    .unwrap();
    let out = compute_row(&v, "src:1", &fid("q"), &BTreeMap::new(), &schema, &r);
    assert_eq!(out, CellValue::Error, "divide-by-zero → #ERROR (a diagnostic cell, never a panic)");
}

#[test]
fn formula_arithmetic_results_are_exact() {
    // Pin the exact arithmetic results so a `+`→`-`/`*`→`/`/`/`→`*`/`%` operator mutant is caught
    // (the cost-of-an-aggregate formula uses every op; a mutated op changes the value).
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let mk = |expr: FormulaExpr| {
        let schema = FormulaSchema::of([FormulaField { field: fid("f"), expr }]).unwrap();
        compute_row(&v, "src:1", &fid("f"), &BTreeMap::new(), &schema, &r)
    };
    let lit = |n: i64| Box::new(FormulaExpr::Lit(Literal::Int(n)));
    assert_eq!(mk(FormulaExpr::Add(lit(20), lit(22))), CellValue::Int(42), "20+22=42 (catches +→-/*)");
    assert_eq!(mk(FormulaExpr::Sub(lit(50), lit(8))), CellValue::Int(42), "50-8=42 (catches -→+/*)");
    assert_eq!(mk(FormulaExpr::Mul(lit(6), lit(7))), CellValue::Int(42), "6*7=42 (catches *→+//)");
    assert_eq!(mk(FormulaExpr::Div(lit(84), lit(2))), CellValue::Int(42), "84/2=42 (catches /→*/%)");
    assert_eq!(mk(FormulaExpr::Div(lit(85), lit(2))), CellValue::Int(42), "85/2=42 floor (catches /→%, 85%2=1)");
}

#[test]
fn formula_arithmetic_on_non_int_is_error() {
    // base is a Text property: arithmetic over it is un-evaluable → #ERROR (never coerced).
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([FormulaField {
        field: fid("t"),
        expr: FormulaExpr::Add(Box::new(FormulaExpr::Prop(fid("name"))), Box::new(FormulaExpr::Lit(Literal::Int(1)))),
    }])
    .unwrap();
    let mut props: PropertyBag = BTreeMap::new();
    props.insert(fid("name"), FieldValue::Text("hi".into()));
    let out = compute_row(&v, "src:1", &fid("t"), &props, &schema, &r);
    assert_eq!(out, CellValue::Error, "Text + Int → #ERROR (no silent coercion)");
}

#[test]
fn formula_missing_property_is_error() {
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([FormulaField { field: fid("p"), expr: FormulaExpr::Prop(fid("absent")) }]).unwrap();
    let out = compute_row(&v, "src:1", &fid("p"), &BTreeMap::new(), &schema, &r);
    assert_eq!(out, CellValue::Error, "a missing property → #ERROR (fail-closed)");
}

// ───────────────────────────── the cycle gate (#CYCLE, never a loop) ──────────────────────────────

#[test]
fn direct_self_cycle_is_cycle_diagnostic_never_loops() {
    // a = a → #CYCLE (the simplest cycle).
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([FormulaField { field: fid("a"), expr: FormulaExpr::FormulaRef(fid("a")) }]).unwrap();
    let out = compute_row(&v, "src:1", &fid("a"), &BTreeMap::new(), &schema, &r);
    assert_eq!(out, CellValue::Cycle, "a→a is #CYCLE (the diagnostic cell, never an infinite loop)");
}

#[test]
fn mutual_cycle_is_cycle_diagnostic() {
    // a = b + 1; b = a + 1 → both #CYCLE.
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([
        FormulaField {
            field: fid("a"),
            expr: FormulaExpr::Add(Box::new(FormulaExpr::FormulaRef(fid("b"))), Box::new(FormulaExpr::Lit(Literal::Int(1)))),
        },
        FormulaField {
            field: fid("b"),
            expr: FormulaExpr::Add(Box::new(FormulaExpr::FormulaRef(fid("a"))), Box::new(FormulaExpr::Lit(Literal::Int(1)))),
        },
    ])
    .unwrap();
    assert_eq!(compute_row(&v, "src:1", &fid("a"), &BTreeMap::new(), &schema, &r), CellValue::Cycle, "a↔b mutual cycle → #CYCLE");
    assert_eq!(compute_row(&v, "src:1", &fid("b"), &BTreeMap::new(), &schema, &r), CellValue::Cycle, "from b too → #CYCLE");
}

#[test]
fn cycle_through_arithmetic_propagates_not_masked() {
    // c = (a) * 1 where a cycles — the cycle must propagate through the arithmetic wrapper, not be
    // masked as #ERROR (the cycle diagnosis is the bounded-evaluation green artifact).
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let schema = FormulaSchema::of([
        FormulaField { field: fid("a"), expr: FormulaExpr::FormulaRef(fid("a")) },
        FormulaField {
            field: fid("c"),
            expr: FormulaExpr::Mul(Box::new(FormulaExpr::FormulaRef(fid("a"))), Box::new(FormulaExpr::Lit(Literal::Int(1)))),
        },
    ])
    .unwrap();
    assert_eq!(compute_row(&v, "src:1", &fid("c"), &BTreeMap::new(), &schema, &r), CellValue::Cycle, "a cycle through arithmetic still surfaces #CYCLE");
}

#[test]
fn long_acyclic_chain_resolves_within_depth_bound() {
    // f0 = Prop; f1 = f0 + 1; …; f50 = f49 + 1. A deep BUT acyclic chain resolves (not a false
    // #CYCLE) — the visited-set, not a blanket depth cap, distinguishes a cycle from a deep chain.
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let mut fields = vec![FormulaField { field: fid("f0"), expr: FormulaExpr::Prop(fid("x")) }];
    for i in 1..=50 {
        fields.push(FormulaField {
            field: fid(&format!("f{i}")),
            expr: FormulaExpr::Add(
                Box::new(FormulaExpr::FormulaRef(fid(&format!("f{}", i - 1)))),
                Box::new(FormulaExpr::Lit(Literal::Int(1))),
            ),
        });
    }
    let schema = FormulaSchema::of(fields).unwrap();
    let mut props: PropertyBag = BTreeMap::new();
    props.insert(fid("x"), FieldValue::Int(0));
    let out = compute_row(&v, "src:1", &fid("f50"), &props, &schema, &r);
    assert_eq!(out, CellValue::Int(50), "a 51-deep acyclic chain resolves to 50 (no false #CYCLE)");
}

#[test]
fn acyclic_chain_at_dependency_depth_bound_resolves_pinning_the_guard() {
    // A FormulaRef chain whose depth reaches EXACTLY MAX_DEPENDENCY_DEPTH must still resolve (not a
    // false #CYCLE) — this exercises the `depth > MAX_DEPENDENCY_DEPTH` guard at its boundary so a
    // `>`→`==`/`>=` mutant (which would return #CYCLE at depth == the bound) flips this assertion.
    let fx = Fixture::build("src:1", &[], &[]);
    let v = viewer("p:viewer", "acme");
    let r = fx.resolver();
    let chain = MAX_DEPENDENCY_DEPTH; // the deepest acyclic chain whose walk reaches the bound.
    let mut fields = vec![FormulaField { field: fid("g0"), expr: FormulaExpr::Prop(fid("x")) }];
    for i in 1..=chain {
        fields.push(FormulaField {
            field: fid(&format!("g{i}")),
            expr: FormulaExpr::FormulaRef(fid(&format!("g{}", i - 1))),
        });
    }
    let schema = FormulaSchema::of(fields).unwrap();
    let mut props: PropertyBag = BTreeMap::new();
    props.insert(fid("x"), FieldValue::Int(7));
    let out = compute_row(&v, "src:1", &fid(&format!("g{chain}")), &props, &schema, &r);
    assert_eq!(out, CellValue::Int(7), "a chain reaching the dependency-depth bound resolves (no false #CYCLE at the boundary)");
}

// ───────────────────────────── static cost-bound rejection ────────────────────────────────────────

#[test]
fn over_deep_formula_is_rejected_at_build() {
    // A nested arithmetic tree exceeding MAX_FORMULA_DEPTH is refused at schema build (before any
    // evaluation) — the DoS-hardening surface, fail-closed.
    let mut expr = FormulaExpr::Lit(Literal::Int(1));
    for _ in 0..(MAX_FORMULA_DEPTH + 5) {
        expr = FormulaExpr::Add(Box::new(expr), Box::new(FormulaExpr::Lit(Literal::Int(1))));
    }
    let err = FormulaSchema::of([FormulaField { field: fid("deep"), expr }]).unwrap_err();
    assert!(matches!(err, FormulaSchemaError::TooDeep { .. }), "an over-deep formula is rejected at build: {err}");
}

#[test]
fn formula_exactly_at_depth_bound_is_accepted_one_over_is_rejected() {
    // A tree of EXACTLY MAX_FORMULA_DEPTH is accepted; depth+1 is rejected. This pins the strict
    // `>` boundary (a `>`→`>=`/`==` mutant flips one of these two assertions).
    let build = |depth: usize| {
        // depth-1 nested Adds gives an expression of nesting depth `depth` (each Add is one level,
        // the leaf Lit is the innermost level).
        let mut expr = FormulaExpr::Lit(Literal::Int(1));
        for _ in 0..(depth - 1) {
            expr = FormulaExpr::Add(Box::new(expr), Box::new(FormulaExpr::Lit(Literal::Int(1))));
        }
        FormulaSchema::of([FormulaField { field: fid("d"), expr }])
    };
    assert!(build(MAX_FORMULA_DEPTH).is_ok(), "a tree of EXACTLY MAX_FORMULA_DEPTH is accepted (strict >)");
    assert!(matches!(build(MAX_FORMULA_DEPTH + 1), Err(FormulaSchemaError::TooDeep { .. })), "depth+1 is rejected");
}

/// Build a WIDE, shallow tree of an exact node count via a balanced binary Add tree (depth stays
/// `log2(nodes)`, well within MAX_FORMULA_DEPTH, so the NODE bound — not the depth bound — is what
/// rejects). `leaves` Lit leaves → a balanced Add tree of `2*leaves - 1` nodes.
fn balanced_add_tree(leaves: usize) -> FormulaExpr {
    let mut level: Vec<FormulaExpr> = (0..leaves).map(|_| FormulaExpr::Lit(Literal::Int(1))).collect();
    while level.len() > 1 {
        let mut next = Vec::new();
        let mut it = level.into_iter();
        while let Some(a) = it.next() {
            match it.next() {
                Some(b) => next.push(FormulaExpr::Add(Box::new(a), Box::new(b))),
                None => next.push(a),
            }
        }
        level = next;
    }
    level.pop().unwrap()
}

#[test]
fn formula_node_budget_boundary_is_strict() {
    // A balanced (shallow) tree just AT the node bound is accepted; the next size up (still shallow,
    // so the NODE bound — not depth — is the gate) is rejected with TooLarge. A balanced Add tree of
    // L leaves has 2L-1 nodes and depth ~log2(L) ≪ MAX_FORMULA_DEPTH. Pins the strict
    // `nodes > MAX_FORMULA_NODES` boundary (a `>`→`==`/`>=` mutant flips the accept/reject) AND the
    // node_count arithmetic (a wrong count mis-places the boundary).
    // 2L-1 <= MAX → L <= (MAX+1)/2; the largest accepted L:
    let l_at = MAX_FORMULA_NODES.div_ceil(2); // 2*l_at-1 = MAX (for odd MAX) or MAX-1 (even) — within bound.
    let at_bound = balanced_add_tree(l_at);
    assert!(
        FormulaSchema::of([FormulaField { field: fid("at"), expr: at_bound }]).is_ok(),
        "a balanced tree at the node bound is accepted (strict >)"
    );
    // A clearly-over tree (3x the leaf budget) is rejected on the NODE bound (depth ~log2 still tiny).
    let over = balanced_add_tree(MAX_FORMULA_NODES * 3);
    let err = FormulaSchema::of([FormulaField { field: fid("over"), expr: over }]).unwrap_err();
    assert!(matches!(err, FormulaSchemaError::TooLarge { .. }), "a clearly-over-node-budget tree is rejected with TooLarge (not TooDeep): {err}");
}

#[test]
fn duplicate_formula_field_is_rejected() {
    let err = FormulaSchema::of([
        FormulaField { field: fid("a"), expr: FormulaExpr::Lit(Literal::Int(1)) },
        FormulaField { field: fid("a"), expr: FormulaExpr::Lit(Literal::Int(2)) },
    ])
    .unwrap_err();
    assert!(matches!(err, FormulaSchemaError::DuplicateField(_)), "a duplicate formula field is rejected: {err}");
}

#[test]
fn static_dependency_set_lists_formula_refs() {
    // c = a + (b * 1): the static dependency set is {a, b} (the FormulaRef leaves only).
    let expr = FormulaExpr::Add(
        Box::new(FormulaExpr::FormulaRef(fid("a"))),
        Box::new(FormulaExpr::Mul(
            Box::new(FormulaExpr::FormulaRef(fid("b"))),
            Box::new(FormulaExpr::Lit(Literal::Int(1))),
        )),
    );
    let mut deps = BTreeSet::new();
    expr.formula_refs(&mut deps);
    let names: Vec<String> = deps.iter().map(|f| f.to_string()).collect();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()], "the static dependency set is the FormulaRef leaves");
}

#[test]
fn cell_value_display_renders_diagnostics() {
    assert_eq!(CellValue::Cycle.display(), "#CYCLE");
    assert_eq!(CellValue::Error.display(), "#ERROR");
    assert_eq!(CellValue::Empty.display(), "#EMPTY");
    assert_eq!(CellValue::Int(7).display(), "7");
    assert!(CellValue::Cycle.is_diagnostic() && !CellValue::Int(0).is_diagnostic());
}

// ───────────────────────────── the rollup-latency telemetry (KN-D10 trigger) ──────────────────────

#[test]
fn rollup_latency_telemetry_flags_over_budget_rollup() {
    let mut tel = RollupLatencyTelemetry::new();
    // A fast rollup (within a 250 ms budget) and a slow one (over).
    for _ in 0..100 {
        tel.record("db:fast", &fid("count"), Duration::from_millis(5));
        tel.record("db:slow", &fid("sum"), Duration::from_millis(400));
    }
    let candidates = tel.materialisation_candidates(250);
    let fields: Vec<String> = candidates.iter().map(|c| c.field.to_string()).collect();
    assert!(fields.contains(&"sum".to_string()), "the over-budget rollup is a materialisation candidate (KN-P31): {fields:?}");
    assert!(!fields.contains(&"count".to_string()), "the within-budget rollup is NOT a candidate");
    let slow = candidates.iter().find(|c| c.field.as_str() == "sum").unwrap();
    assert_eq!(slow.db_id, "db:slow");
    assert!(slow.measured_p99_ms > 250, "the hint carries the measured p99 that crossed the budget");
}

#[test]
fn rollup_latency_p99_is_the_99th_percentile_and_strict_budget() {
    let mut tel = RollupLatencyTelemetry::new();
    // 100 samples: 99 at 10 ms, one at 1000 ms. The p99 (99th percentile, index 98 of 0..99) is the
    // 99th-smallest = 10 ms — NOT the max. Pins the percentile index arithmetic (a `*`/`-` mutant in
    // the index calc would pick the 1000 ms max and flip the within-budget verdict).
    for _ in 0..99 {
        tel.record("db:x", &fid("r"), Duration::from_millis(10));
    }
    tel.record("db:x", &fid("r"), Duration::from_millis(1000));
    let p99 = tel.p99_ms("db:x", &fid("r"));
    assert!((9.0..=11.0).contains(&p99), "the p99 is the 99th-percentile (~10 ms), NOT the 1000 ms max: {p99}");
    // A 10 ms p99 is WITHIN a 10 ms budget (strict `>`): not a candidate. At 9 ms budget it crosses.
    assert!(tel.materialisation_candidates(10).is_empty(), "p99==budget is WITHIN budget (strict >): not a candidate");
    assert!(!tel.materialisation_candidates(9).is_empty(), "p99 > budget crosses: a candidate");
}
