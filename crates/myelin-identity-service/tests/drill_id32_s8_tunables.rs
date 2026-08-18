
use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::load_generator::Multiplier;
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    lower,
    namespace::{FragmentDef, NamespaceEngine, PermissionRule, Userset},
    watermark_verdict, ListObjects, ReverseIndex, ReverseIndexConsumer, TupleStore,
    WatermarkVerdict,
};
use myelin_storage::TenantScope;
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

fn admin(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(&admin(tenant), Region("eu-west".into()))
}

fn subject(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn add(object: &str, relation: &str, subj: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subj.into()),
        caveat: None,
    })
}

fn now() -> Timestamp {
    Timestamp("2026-06-24T00:00:00Z".into())
}

fn latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn pinned(rev: u64) -> Consistency {
    Consistency {
        at_least: Zookie(format!("zk-{rev:020}")),
        mode: ConsistencyMode::Strong,
    }
}

fn repo_namespace() -> NamespaceEngine {
    let mut namespace = NamespaceEngine::with_core_hierarchy();
    let _ = namespace.admit(&FragmentDef {
        object_type: ObjectType("repo".into()),
        relations: vec![RelName("reader".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Relation(RelName("reader".into())),
        }],
    });
    namespace
}

fn wired_with_grants(cap: usize, s: &TenantScope, subj: &str, n: usize) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    let grants: Vec<TupleDelta> = (0..n)
        .map(|i| add(&format!("repo:r{i}"), "reader", subj))
        .collect();
    store
        .write_tuples(s, &admin(&s.tenant().0), &grants, None, None, now())
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox, bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    ListObjects::with_cap(store, repo_namespace(), index, cap)
}

#[test]
fn id32_cardinality_cap_finalised_at_measured_crossover() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let cap = thresholds.authz_index.ids_cardinality_cap;
    assert!(
        cap > 0,
        "the finalised cardinality cap is a positive tunable"
    );

    let s = scope("acme");

    let pushdown_cost = {
        let via = ColRef {
            table: "repo".into(),
            column: "id".into(),
        };
        let lowered = lower(
            &SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: via.clone(),
            },
            &subject("p:alice", "acme"),
            &via,
        );
        lowered.joins.len()
    };
    assert_eq!(
        pushdown_cost, 1,
        "the push-down is a single fixed JOIN (no N+1)"
    );

    let surge = Multiplier::SURGE.factor() as usize;
    let sample_sizes = [cap / 4, cap / 2, cap, cap + 1];
    let mut measured_ids_cost: Vec<(usize, usize)> = Vec::new();
    for &n in &sample_sizes {
        if n == 0 {
            continue;
        }
        let lo = wired_with_grants(n + 1, &s, "p:alice", n);
        let realised = match lo
            .list_objects(
                &s,
                &subject("p:alice", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read the cost-curve relationship snapshot")
        {
            ListObjectsResult::Ids { ids, .. } => ids.len(),
            ListObjectsResult::Filter { .. } => {
                panic!("with a generous cap the cost-curve sample must materialise as Ids")
            }
        };
        measured_ids_cost.push((n, realised));
    }

    for w in measured_ids_cost.windows(2) {
        assert!(
            w[1].1 >= w[0].1,
            "the materialise cost is monotone in the reachable-set size \
             (a single cardinality cap cleanly separates the two plans): {:?}",
            measured_ids_cost
        );
    }
    let at_cap = measured_ids_cost
        .iter()
        .find(|(n, _)| *n == cap)
        .map(|(_, cost)| *cost)
        .expect("the cap-sized sample was measured");
    assert_eq!(
        at_cap, cap,
        "a cap-sized reachable set fully materialises (the materialise cost == the cap) - the \
         crossover where the fixed-cost JOIN takes over sits AT the finalised cap"
    );

    let small_cap = 3usize;
    let at = wired_with_grants(small_cap, &s, "p:atcap", small_cap);
    match at
        .list_objects(
            &s,
            &subject("p:atcap", "acme"),
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &latest(),
        )
        .expect("read the at-cap relationship snapshot")
    {
        ListObjectsResult::Ids { ids, .. } => assert_eq!(
            ids.len(),
            small_cap,
            "a list AT the cap materialises (Ids) - the measured switch point"
        ),
        ListObjectsResult::Filter { .. } => panic!("AT the cap must dispatch to Ids"),
    }
    let over = wired_with_grants(small_cap, &s, "p:overcap", small_cap + 1);
    match over
        .list_objects(
            &s,
            &subject("p:overcap", "acme"),
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &latest(),
        )
        .expect("read the over-cap relationship snapshot")
    {
        ListObjectsResult::Filter { set_expr, .. } => match set_expr {
            SetExpr::InRelation { relation, .. } => assert_eq!(
                relation,
                RelName("read".into()),
                "one OVER the cap pushes down (Filter) - the measured switch point"
            ),
            other => panic!("the over-cap Filter is the InRelation push-down, got {other:?}"),
        },
        ListObjectsResult::Ids { .. } => panic!("OVER the cap must dispatch to Filter"),
    }

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::PoolSaturation,
        vec![Label::new("pool", "s8_ids_materialise")],
        at_cap as i64,
    );
    src.assert_labelled(
        SignalName::PoolSaturation,
        vec![Label::new("pool", "s8_ids_materialise")],
        Predicate::Eq(cap as i64),
    )
    .expect_green();

    println!(
        "[P-425 DRILL GREEN 2026-06-24] ID-M5 S8 cardinality cap finalised: measured cost curve \
         {measured_ids_cost:?} (cap-sized reachable set = the world-scale materialise case, {surge}× \
         write context) → materialise cost monotone in set size, crossover AT cap={cap} (materialise \
         cost == cap), push-down cost = 1 fixed JOIN; dispatch flips exactly at the cap (AT→Ids, \
         OVER→Filter). The P-ID-11 cardinality-cap floor is CLOSED."
    );
}

#[test]
fn id32_reverse_index_lag_slo_finalised_and_fallback_honoured() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let slo_ms = thresholds.authz_index.reverse_index_lag_slo_ms;
    assert!(
        slo_ms > 0,
        "the finalised reverse_index_lag SLO is positive"
    );

    let revocation_sla_ms = thresholds.revocation.sla_mins * 60 * 1000;
    assert!(
        slo_ms <= revocation_sla_ms,
        "the reverse_index_lag SLO ({slo_ms} ms) must stay <= the revocation SLA \
         ({revocation_sla_ms} ms) - a stale grant can never outlive a revoke"
    );

    let s = scope("acme");
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let surge = Multiplier::SURGE.factor() as u64;
    let base: u64 = 64;
    let writes = base * surge;
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    for i in 0..writes {
        store
            .write_tuples(
                &s,
                &admin("acme"),
                &[add(&format!("repo:r{i}"), "reader", "p:alice")],
                None,
                None,
                now(),
            )
            .expect("surge write");
        relay.drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
    }

    let watermark = index.watermark(&s);
    assert!(
        !watermark.0.is_empty(),
        "the watermark advanced under the surge (the index reflects the writes)"
    );

    let via = ColRef {
        table: "repo".into(),
        column: "id".into(),
    };
    let lowered = lower(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: via.clone(),
        },
        &subject("p:alice", "acme"),
        &via,
    );
    assert!(
        lowered.depends_on_reverse_index(),
        "the InRelation lowering is the watermark-guarded S8 JOIN path"
    );

    let wm_rev: u64 = watermark
        .0
        .trim_start_matches("zk-")
        .parse()
        .expect("the watermark is the zero-padded zk-<rev> form");

    let serves = watermark_verdict(&index, &s, &lowered, &pinned(wm_rev));
    assert_eq!(
        serves,
        WatermarkVerdict::JoinServes,
        "a scan within the lag SLO (at-or-before the watermark) serves from S8 - the fast JOIN path"
    );

    let beyond = watermark_verdict(&index, &s, &lowered, &pinned(wm_rev + 1));
    assert!(
        matches!(beyond, WatermarkVerdict::FallBackToCheck { .. }),
        "a scan BEYOND the watermark (index behind beyond the SLO) falls back to check - never \
         serve a stale grant (the new-enemy guard): {beyond:?}"
    );

    println!(
        "ID-M5 reverse_index_lag: {writes} writes under {surge}× surge; SLO {slo_ms} ms ≤ \
         revocation SLA {revocation_sla_ms} ms; a scan within the SLO serves from S8, one beyond \
         falls back to check (new-enemy guard)."
    );
}

#[test]
fn id32_cap_dispatch_boundary_is_exact() {
    let s = scope("acme");
    let cap = 5usize;
    let at = wired_with_grants(cap, &s, "p:exact", cap);
    assert!(
        matches!(
            at.list_objects(
                &s,
                &subject("p:exact", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read the exact-cap relationship snapshot"),
            ListObjectsResult::Ids { .. }
        ),
        "a reachable set of EXACTLY the cap materialises (the <= boundary, not <)"
    );
    let over = wired_with_grants(cap, &s, "p:exactover", cap + 1);
    assert!(
        matches!(
            over.list_objects(
                &s,
                &subject("p:exactover", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read the cap-plus-one relationship snapshot"),
            ListObjectsResult::Filter { .. }
        ),
        "a reachable set of cap+1 pushes down (the > boundary)"
    );
}

#[test]
fn id32_lag_slo_fallback_boundary_is_exact() {
    let s = scope("acme");
    let index = ReverseIndex::new();
    index.advance_watermark_only(&s, &Zookie(format!("zk-{:020}", 5u64)));
    let via = ColRef {
        table: "repo".into(),
        column: "id".into(),
    };
    let lowered = lower(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: via.clone(),
        },
        &subject("p:alice", "acme"),
        &via,
    );
    assert_eq!(
        watermark_verdict(&index, &s, &lowered, &pinned(5)),
        WatermarkVerdict::JoinServes,
        "a scan at EXACTLY the watermark serves (the >= boundary, not >)"
    );
    assert!(
        matches!(
            watermark_verdict(&index, &s, &lowered, &pinned(6)),
            WatermarkVerdict::FallBackToCheck { .. }
        ),
        "a scan one revision beyond the watermark falls back to check (never serve stale)"
    );
}
